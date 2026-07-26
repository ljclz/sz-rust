//! 声明式表分区 — Phase 6.19
//!
//! 提供 PG 风格的声明式分区（Declarative Partitioning）：
//!
//! - **`RangePartition`**：范围分区（`PARTITION BY RANGE (col)`），按列值的范围划分
//! - **`ListPartition`**：列表分区（`PARTITION BY LIST (col)`），按列值的离散值划分
//! - **`HashPartition`**：哈希分区（`PARTITION BY HASH (col)`），按列值的哈希取模划分
//!
//! # 设计
//!
//! - 分区表（PartitionedTable）持有多个分区（Partition），每个分区是一个 `InMemoryTable`
//! - INSERT 时按分区键路由到对应分区（route_row）；未匹配分区时按 PG 语义报错或拒绝
//! - SELECT 时支持分区裁剪（prune_partitions）：根据 WHERE 谓词跳过无关分区
//! - 分区边界由 `PartitionBound` 枚举表示（Range 边界 / List 值列表 / Hash 桶号）
//!
//! # 与 PG 的关系
//!
//! - PG 10+ 支持声明式分区：`CREATE TABLE t (...) PARTITION BY RANGE (col)`
//! - 子分区：`CREATE TABLE t_p0 PARTITION OF t FOR VALUES FROM (MINVALUE) TO (10)`
//! - PG 支持 DEFAULT 分区捕获不匹配行；本实现提供 `default_partition` 选项
//! - PG 分区裁剪发生在规划期（静态裁剪）与执行期（动态裁剪）；本实现提供执行期裁剪
//!
//! # 限制
//!
//! - **无 DDL 集成**：未集成到 `CREATE TABLE ... PARTITION BY` 解析路径（sqlparser 0.53 解析为单个 Expr）
//! - **无子分区**：不支持多级分区（分区再分区）
//! - **无分区维护**：不支持 ATTACH/DETACH PARTITION
//! - **分区裁剪为基本版**：仅支持单列等值/范围谓词，不支持表达式分区键

use crate::executor::{ExecutionError, InMemoryTable, MutableTable, Row, TableStorage};
use crate::expr::compare_values;
use crate::plan::TableSchema;
use szrsql_types::value::Value;

// =====================================================================
//  分区类型枚举
// =====================================================================

/// 分区策略
///
/// 对应 PG 的 `PARTITION BY RANGE/LIST/HASH` 子句。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartitionStrategy {
    /// 范围分区 — `PARTITION BY RANGE (col)`
    Range,
    /// 列表分区 — `PARTITION BY LIST (col)`
    List,
    /// 哈希分区 — `PARTITION BY HASH (col)`
    Hash,
}

impl PartitionStrategy {
    /// 从字符串解析分区策略（大小写不敏感）
    pub fn from_str_ci(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "range" => Some(Self::Range),
            "list" => Some(Self::List),
            "hash" => Some(Self::Hash),
            _ => None,
        }
    }

    /// 返回字符串表示（大写）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Range => "RANGE",
            Self::List => "LIST",
            Self::Hash => "HASH",
        }
    }
}

impl std::fmt::Display for PartitionStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =====================================================================
//  分区边界
// =====================================================================

/// 分区边界定义
///
/// 描述一个分区接收哪些行：
/// - Range：`[lower, upper)` 区间（None 表示无界 / MINVALUE / MAXVALUE）
/// - List：离散值列表（任一匹配即属于此分区）
/// - Hash：`(remainder, modulus)` — `hash(key) % modulus == remainder`
#[derive(Debug, Clone, PartialEq)]
pub enum PartitionBound {
    /// 范围分区边界
    Range {
        /// 下界（None = MINVALUE / -∞，包含）
        lower: Option<Value>,
        /// 上界（None = MAXVALUE / +∞，不包含）
        upper: Option<Value>,
        /// 下界是否包含（true = `[`，false = `(`；PG Range 默认 `[lower, upper)`）
        lower_inc: bool,
        /// 上界是否包含（true = `]`，false = `)`）
        upper_inc: bool,
    },
    /// 列表分区边界 — 值列表
    List(Vec<Value>),
    /// 哈希分区边界 — (余数, 模数)
    Hash {
        /// 余数（`hash(key) % modulus` 必须等于此值）
        remainder: u64,
        /// 模数（分区总数）
        modulus: u64,
    },
}

impl PartitionBound {
    /// 创建范围分区边界 `[lower, upper)`（PG 默认语义）
    pub fn range_half_open(lower: Option<Value>, upper: Option<Value>) -> Self {
        Self::Range {
            lower,
            upper,
            lower_inc: true,
            upper_inc: false,
        }
    }

    /// 创建列表分区边界
    pub fn list(values: Vec<Value>) -> Self {
        Self::List(values)
    }

    /// 创建哈希分区边界
    pub fn hash(remainder: u64, modulus: u64) -> Self {
        Self::Hash { remainder, modulus }
    }

    /// 判断值是否属于此分区边界
    ///
    /// - Range：`lower <= v < upper`（考虑包含标志）
    /// - List：`v` 在值列表中
    /// - Hash：`hash(v) % modulus == remainder`
    pub fn contains(&self, value: &Value) -> bool {
        match self {
            Self::Range {
                lower,
                upper,
                lower_inc,
                upper_inc,
            } => {
                // 下界检查
                if let Some(lo) = lower {
                    let ord = compare_values(value, lo);
                    let ok = match ord {
                        Some(o) => {
                            if *lower_inc {
                                o.is_ge()
                            } else {
                                o.is_gt()
                            }
                        }
                        None => return false,
                    };
                    if !ok {
                        return false;
                    }
                }
                // 上界检查
                if let Some(hi) = upper {
                    let ord = compare_values(value, hi);
                    let ok = match ord {
                        Some(o) => {
                            if *upper_inc {
                                o.is_le()
                            } else {
                                o.is_lt()
                            }
                        }
                        None => return false,
                    };
                    if !ok {
                        return false;
                    }
                }
                true
            }
            Self::List(values) => values.iter().any(|v| v == value),
            Self::Hash { remainder, modulus } => {
                if *modulus == 0 {
                    return false;
                }
                let h = hash_value(value);
                h % modulus == *remainder
            }
        }
    }
}

// =====================================================================
//  哈希函数
// =====================================================================

/// 对 Value 计算稳定的哈希值（用于 Hash 分区）
///
/// 使用简单 FNV-1a 变体，保证：
/// - 同一 Value 永远映射到同一哈希
/// - 不同类型相同字节内容可能碰撞（可接受 — PG 也使用类型特定哈希）
pub fn hash_value(value: &Value) -> u64 {
    let mut state: u64 = 0xcbf29ce484222325;
    let prime: u64 = 0x100000001b3;
    match value {
        Value::Null => {
            state ^= 0x00;
            state = state.wrapping_mul(prime);
        }
        Value::Int64(n) => {
            for byte in n.to_le_bytes() {
                state ^= byte as u64;
                state = state.wrapping_mul(prime);
            }
        }
        Value::Float64(f) => {
            for byte in f.to_bits().to_le_bytes() {
                state ^= byte as u64;
                state = state.wrapping_mul(prime);
            }
        }
        Value::Text(s) => {
            for byte in s.as_bytes() {
                state ^= *byte as u64;
                state = state.wrapping_mul(prime);
            }
        }
        Value::Bool(b) => {
            state ^= if *b {
                1u64
            } else {
                0u64
            };
            state = state.wrapping_mul(prime);
        }
        Value::Date(d) => {
            for byte in d.to_le_bytes() {
                state ^= byte as u64;
                state = state.wrapping_mul(prime);
            }
        }
        Value::Timestamp(t) => {
            for byte in t.to_le_bytes() {
                state ^= byte as u64;
                state = state.wrapping_mul(prime);
            }
        }
        Value::Decimal(unscaled, scale) => {
            for byte in unscaled.to_le_bytes() {
                state ^= byte as u64;
                state = state.wrapping_mul(prime);
            }
            state ^= *scale as u64;
            state = state.wrapping_mul(prime);
        }
        Value::Enum(s) => {
            for byte in s.as_bytes() {
                state ^= *byte as u64;
                state = state.wrapping_mul(prime);
            }
        }
        // 其他类型（Blob/Array/Range/Json/TsVector/TsQuery）使用 Debug 字符串哈希
        other => {
            let s = format!("{other:?}");
            for byte in s.as_bytes() {
                state ^= *byte as u64;
                state = state.wrapping_mul(prime);
            }
        }
    }
    state
}

// =====================================================================
//  分区定义
// =====================================================================

/// 单个分区定义
///
/// 一个分区包含：名称 + 边界 + 实际存储（InMemoryTable）
#[derive(Debug)]
pub struct Partition {
    /// 分区名（如 `t_p0`、`t_p1`）
    pub name: String,
    /// 分区边界
    pub bound: PartitionBound,
    /// 分区存储（InMemoryTable）
    pub table: InMemoryTable,
}

impl Partition {
    /// 创建新分区
    pub fn new(name: impl Into<String>, bound: PartitionBound, table: InMemoryTable) -> Self {
        Self {
            name: name.into(),
            bound,
            table,
        }
    }

    /// 判断值是否路由到此分区
    pub fn contains(&self, value: &Value) -> bool {
        self.bound.contains(value)
    }

    /// 分区行数
    pub fn row_count(&self) -> usize {
        self.table.row_count()
    }
}

// =====================================================================
//  分区谓词（用于分区裁剪）
// =====================================================================

/// 分区裁剪谓词
///
/// 从 WHERE 子句提取的、与分区键相关的谓词。
/// 用于确定哪些分区可能包含匹配行。
#[derive(Debug, Clone, PartialEq)]
pub enum PartitionPrunePredicate {
    /// 等值谓词 — `key = value`
    Eq(Value),
    /// 范围谓词 — `key >= lower AND key < upper`（None 表示无界）
    Range {
        /// 下界（None = 无下界）
        lower: Option<Value>,
        /// 上界（None = 无上界）
        upper: Option<Value>,
        /// 下界是否包含
        lower_inc: bool,
        /// 上界是否包含
        upper_inc: bool,
    },
    /// IN 谓词 — `key IN (v1, v2, ...)`
    In(Vec<Value>),
    /// 无约束（扫描所有分区）
    Unconstrained,
}

impl PartitionPrunePredicate {
    /// 创建等值谓词
    pub fn eq(value: Value) -> Self {
        Self::Eq(value)
    }

    /// 创建范围谓词 `[lower, upper)`
    pub fn range_half_open(lower: Option<Value>, upper: Option<Value>) -> Self {
        Self::Range {
            lower,
            upper,
            lower_inc: true,
            upper_inc: false,
        }
    }

    /// 创建 IN 谓词
    pub fn in_list(values: Vec<Value>) -> Self {
        Self::In(values)
    }

    /// 判断此谓词与给定分区边界是否可能匹配（用于裁剪）
    ///
    /// 返回 true 表示此分区可能包含匹配行（不能裁剪）。
    pub fn may_match(&self, bound: &PartitionBound) -> bool {
        match (self, bound) {
            (
                Self::Eq(v),
                PartitionBound::Range {
                    lower,
                    upper,
                    lower_inc,
                    upper_inc,
                },
            ) => {
                // 等值 v：检查 v 是否在 [lower, upper) 区间
                if let Some(lo) = lower {
                    let ord = compare_values(v, lo);
                    let ok = match ord {
                        Some(o) => {
                            if *lower_inc {
                                o.is_ge()
                            } else {
                                o.is_gt()
                            }
                        }
                        None => return true, // 类型不可比 — 保守不裁剪
                    };
                    if !ok {
                        return false;
                    }
                }
                if let Some(hi) = upper {
                    let ord = compare_values(v, hi);
                    let ok = match ord {
                        Some(o) => {
                            if *upper_inc {
                                o.is_le()
                            } else {
                                o.is_lt()
                            }
                        }
                        None => return true,
                    };
                    if !ok {
                        return false;
                    }
                }
                true
            }
            (Self::Eq(v), PartitionBound::List(values)) => values.iter().any(|x| x == v),
            (Self::Eq(v), PartitionBound::Hash { remainder, modulus }) => {
                if *modulus == 0 {
                    return true;
                }
                hash_value(v) % modulus == *remainder
            }
            (
                Self::Range {
                    lower: plo,
                    upper: pup,
                    lower_inc: pli,
                    upper_inc: pupi,
                },
                PartitionBound::Range {
                    lower: blo,
                    upper: bup,
                    lower_inc: bli,
                    upper_inc: bupi,
                },
            ) => {
                // 区间相交检测：两个区间 [a_lo, a_hi) 与 [b_lo, b_hi) 相交
                // 当且仅当 a_lo < b_hi AND b_lo < a_hi（考虑包含标志）
                // 检查谓词下界是否在分区上界之前
                if let (Some(plo), Some(bup)) = (plo, bup) {
                    let ord = compare_values(plo, bup);
                    let ok = match ord {
                        Some(o) => {
                            // plo < bup（若 plo == bup，需两者都包含才相交）
                            if o == std::cmp::Ordering::Equal {
                                *pli && *bupi
                            } else {
                                o.is_lt()
                            }
                        }
                        None => return true,
                    };
                    if !ok {
                        return false;
                    }
                }
                // 检查分区下界是否在谓词上界之前
                if let (Some(blo), Some(pup)) = (blo, pup) {
                    let ord = compare_values(blo, pup);
                    let ok = match ord {
                        Some(o) => {
                            if o == std::cmp::Ordering::Equal {
                                *bli && *pupi
                            } else {
                                o.is_lt()
                            }
                        }
                        None => return true,
                    };
                    if !ok {
                        return false;
                    }
                }
                true
            }
            (Self::Range { .. }, PartitionBound::List(_)) => {
                // Range 谓词与 List 分区 — 保守不裁剪
                true
            }
            (Self::Range { .. }, PartitionBound::Hash { .. }) => {
                // Range 谓词与 Hash 分区 — 保守不裁剪
                true
            }
            (Self::In(values), PartitionBound::Range { .. }) => {
                // IN 谓词与 Range 分区 — 任一值可能落在区间内则不裁剪
                values.iter().any(|v| {
                    let eq_pred = Self::Eq(v.clone());
                    eq_pred.may_match(bound)
                })
            }
            (Self::In(values), PartitionBound::List(list_values)) => {
                // IN 谓词与 List 分区 — 交集非空则不裁剪
                values.iter().any(|v| list_values.iter().any(|x| x == v))
            }
            (Self::In(values), PartitionBound::Hash { remainder, modulus }) => {
                if *modulus == 0 {
                    return true;
                }
                values.iter().any(|v| hash_value(v) % modulus == *remainder)
            }
            (Self::Unconstrained, _) => true,
        }
    }
}

// =====================================================================
//  分区表
// =====================================================================

/// 分区表 — 持有多个分区并提供路由/裁剪能力
///
/// 使用方式：
/// ```ignore
/// use szrsql_sql::partition::*;
/// use szrsql_sql::executor::InMemoryTable;
/// use szrsql_sql::plan::TableSchema;
///
/// let schema = build_schema();
/// let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::Range, 0);
/// pt.add_partition(Partition::new(
///     "t_p0",
///     PartitionBound::range_half_open(None, Some(Value::Int64(10))),
///     InMemoryTable::new(schema.clone()),
/// ));
/// pt.add_partition(Partition::new(
///     "t_p1",
///     PartitionBound::range_half_open(Some(Value::Int64(10)), None),
///     InMemoryTable::new(schema.clone()),
/// ));
///
/// // 路由 INSERT
/// pt.route_and_insert(vec![Value::Int64(5)]).unwrap();   // → t_p0
/// pt.route_and_insert(vec![Value::Int64(15)]).unwrap();  // → t_p1
///
/// // 裁剪 SELECT
/// let pruned = pt.prune_partitions(&PartitionPrunePredicate::eq(Value::Int64(15)));
/// assert_eq!(pruned.len(), 1);  // 仅 t_p1
/// ```
pub struct PartitionedTable {
    /// 分区表名
    pub name: String,
    /// 表 Schema（所有分区共享）
    pub schema: TableSchema,
    /// 分区策略
    pub strategy: PartitionStrategy,
    /// 分区键列索引（单列分区）
    pub key_column: usize,
    /// 分区列表（按添加顺序）
    pub partitions: Vec<Partition>,
    /// 默认分区（可选）— 捕获不匹配任何分区的行
    pub default_partition: Option<InMemoryTable>,
}

impl PartitionedTable {
    /// 创建新的分区表
    pub fn new(
        name: impl Into<String>,
        schema: TableSchema,
        strategy: PartitionStrategy,
        key_column: usize,
    ) -> Self {
        Self {
            name: name.into(),
            schema,
            strategy,
            key_column,
            partitions: Vec::new(),
            default_partition: None,
        }
    }

    /// 添加分区
    pub fn add_partition(&mut self, partition: Partition) {
        self.partitions.push(partition);
    }

    /// 设置默认分区
    pub fn set_default_partition(&mut self, table: InMemoryTable) {
        self.default_partition = Some(table);
    }

    /// 路由一行到正确分区（按分区键值）
    ///
    /// 返回目标分区索引（在 `partitions` 中的位置）。
    /// 若无匹配分区且有默认分区，返回 `None`（表示路由到默认分区）。
    /// 若无匹配分区且无默认分区，返回错误。
    pub fn route_row(&self, row: &[Value]) -> Result<Option<usize>, ExecutionError> {
        let key = row.get(self.key_column).cloned().unwrap_or(Value::Null);
        for (i, p) in self.partitions.iter().enumerate() {
            if p.contains(&key) {
                return Ok(Some(i));
            }
        }
        if self.default_partition.is_some() {
            return Ok(None);
        }
        Err(ExecutionError::InvalidArgument(format!(
            "no partition found for key {:?} in partitioned table {}",
            key, self.name
        )))
    }

    /// 路由并插入一行
    ///
    /// 根据分区键值将行路由到正确分区并插入。
    /// 若无匹配分区但有默认分区，插入到默认分区。
    pub fn route_and_insert(&mut self, row: Vec<Value>) -> Result<usize, ExecutionError> {
        let target_idx = self.route_row(&row)?;
        match target_idx {
            Some(i) => {
                let partition = &mut self.partitions[i];
                partition.table.insert_row(row);
                Ok(i)
            }
            None => {
                let default_table = self
                    .default_partition
                    .as_mut()
                    .expect("default_partition should exist when route_row returns None");
                default_table.insert_row(row);
                // 返回 usize::MAX 表示默认分区
                Ok(usize::MAX)
            }
        }
    }

    /// 分区裁剪 — 根据谓词返回需要扫描的分区索引列表
    ///
    /// 若谓词为 `Unconstrained`，返回全部分区。
    pub fn prune_partitions(&self, predicate: &PartitionPrunePredicate) -> Vec<usize> {
        if matches!(predicate, PartitionPrunePredicate::Unconstrained) {
            return (0..self.partitions.len()).collect();
        }
        let mut result = Vec::new();
        for (i, p) in self.partitions.iter().enumerate() {
            if predicate.may_match(&p.bound) {
                result.push(i);
            }
        }
        result
    }

    /// 扫描所有分区的所有行（无裁剪）
    pub fn scan_all(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for p in &self.partitions {
            rows.extend(p.table.scan_iter());
        }
        if let Some(default) = &self.default_partition {
            rows.extend(default.scan_iter());
        }
        rows
    }

    /// 扫描指定分区索引列表的行（裁剪后扫描）
    pub fn scan_partitions(&self, indices: &[usize]) -> Vec<Row> {
        let mut rows = Vec::new();
        for &i in indices {
            if let Some(p) = self.partitions.get(i) {
                rows.extend(p.table.scan_iter());
            }
        }
        rows
    }

    /// 总行数（所有分区 + 默认分区）
    pub fn total_row_count(&self) -> usize {
        let count: usize = self.partitions.iter().map(|p| p.row_count()).sum();
        count
            + self
                .default_partition
                .as_ref()
                .map(|t| t.row_count())
                .unwrap_or(0)
    }

    /// 分区数量（不含默认分区）
    pub fn partition_count(&self) -> usize {
        self.partitions.len()
    }

    /// 按名称查找分区索引
    pub fn find_partition_by_name(&self, name: &str) -> Option<usize> {
        self.partitions
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// 获取分区键列名
    pub fn key_column_name(&self) -> &str {
        self.schema
            .columns
            .get(self.key_column)
            .map(|c| c.name.as_str())
            .unwrap_or("")
    }
}

// =====================================================================
//  范围分区辅助构造
// =====================================================================

/// 范围分区边界辅助构造
pub mod range_bound {
    use super::PartitionBound;
    use szrsql_types::value::Value;

    /// `FROM (lo) TO (hi)` — `[lo, hi)`（PG 默认语义）
    pub fn from_to(lo: Value, hi: Value) -> PartitionBound {
        PartitionBound::range_half_open(Some(lo), Some(hi))
    }

    /// `FROM MINVALUE TO (hi)` — `(-∞, hi)`
    pub fn minvalue_to(hi: Value) -> PartitionBound {
        PartitionBound::range_half_open(None, Some(hi))
    }

    /// `FROM (lo) TO MAXVALUE` — `[lo, +∞)`
    pub fn to_maxvalue(lo: Value) -> PartitionBound {
        PartitionBound::range_half_open(Some(lo), None)
    }

    /// `FROM MINVALUE TO MAXVALUE` — `(-∞, +∞)`
    pub fn full_range() -> PartitionBound {
        PartitionBound::range_half_open(None, None)
    }
}

// =====================================================================
//  哈希分区辅助构造
// =====================================================================

/// 哈希分区边界辅助构造
///
/// 为 N 个哈希分区生成边界列表。
/// 每个分区 i 的边界为 `Hash { remainder: i, modulus: N }`。
pub fn hash_partitions_bounds(count: u64) -> Vec<PartitionBound> {
    if count == 0 {
        return Vec::new();
    }
    (0..count).map(|i| PartitionBound::hash(i, count)).collect()
}

// =====================================================================
//  单元测试 — 内置测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_schema(name: &str) -> TableSchema {
        use crate::ast::{ColumnDefinition, TableName};
        use szrsql_types::value::ColumnType;
        TableSchema {
            name: TableName::new(name),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("val", ColumnType::Int64),
            ],
        }
    }

    #[test]
    fn test_strategy_from_str() {
        assert_eq!(
            PartitionStrategy::from_str_ci("range"),
            Some(PartitionStrategy::Range)
        );
        assert_eq!(
            PartitionStrategy::from_str_ci("LIST"),
            Some(PartitionStrategy::List)
        );
        assert_eq!(
            PartitionStrategy::from_str_ci("Hash"),
            Some(PartitionStrategy::Hash)
        );
        assert_eq!(PartitionStrategy::from_str_ci("foo"), None);
    }

    #[test]
    fn test_strategy_display() {
        assert_eq!(PartitionStrategy::Range.to_string(), "RANGE");
        assert_eq!(PartitionStrategy::List.to_string(), "LIST");
        assert_eq!(PartitionStrategy::Hash.to_string(), "HASH");
    }

    #[test]
    fn test_range_bound_contains() {
        let b = range_bound::from_to(Value::Int64(10), Value::Int64(20));
        assert!(b.contains(&Value::Int64(10))); // 下界包含
        assert!(b.contains(&Value::Int64(15)));
        assert!(!b.contains(&Value::Int64(20))); // 上界不包含
        assert!(!b.contains(&Value::Int64(5)));
        assert!(!b.contains(&Value::Int64(25)));
    }

    #[test]
    fn test_range_bound_minvalue() {
        let b = range_bound::minvalue_to(Value::Int64(10));
        assert!(b.contains(&Value::Int64(-100)));
        assert!(b.contains(&Value::Int64(0)));
        assert!(b.contains(&Value::Int64(9)));
        assert!(!b.contains(&Value::Int64(10)));
    }

    #[test]
    fn test_range_bound_maxvalue() {
        let b = range_bound::to_maxvalue(Value::Int64(10));
        assert!(b.contains(&Value::Int64(10)));
        assert!(b.contains(&Value::Int64(100)));
        assert!(b.contains(&Value::Int64(1000000)));
        assert!(!b.contains(&Value::Int64(9)));
    }

    #[test]
    fn test_list_bound_contains() {
        let b = PartitionBound::list(vec![
            Value::Text("a".into()),
            Value::Text("b".into()),
            Value::Text("c".into()),
        ]);
        assert!(b.contains(&Value::Text("a".into())));
        assert!(b.contains(&Value::Text("b".into())));
        assert!(b.contains(&Value::Text("c".into())));
        assert!(!b.contains(&Value::Text("d".into())));
        assert!(!b.contains(&Value::Int64(1)));
    }

    #[test]
    fn test_hash_bound_contains() {
        // modulus=4, remainder=2
        let b = PartitionBound::hash(2, 4);
        for v in &[0i64, 1, 2, 3, 4, 5, 100, 1000] {
            let h = hash_value(&Value::Int64(*v));
            let expected = h % 4 == 2;
            assert_eq!(
                b.contains(&Value::Int64(*v)),
                expected,
                "v={v}, h={h}, h%4={}, expected={expected}",
                h % 4
            );
        }
    }

    #[test]
    fn test_hash_value_stable() {
        // 同一值多次哈希结果一致
        let v = Value::Int64(42);
        let h1 = hash_value(&v);
        let h2 = hash_value(&v);
        assert_eq!(h1, h2);

        // 不同值大概率不同哈希
        let h3 = hash_value(&Value::Int64(43));
        assert_ne!(h1, h3);

        // 文本哈希
        let h4 = hash_value(&Value::Text("hello".into()));
        let h5 = hash_value(&Value::Text("hello".into()));
        assert_eq!(h4, h5);
        assert_ne!(h4, hash_value(&Value::Text("world".into())));
    }

    #[test]
    fn test_partitioned_table_range_routing() {
        let schema = make_schema("t");
        let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::Range, 0);
        pt.add_partition(Partition::new(
            "t_p0",
            range_bound::minvalue_to(Value::Int64(10)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p1",
            range_bound::from_to(Value::Int64(10), Value::Int64(20)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p2",
            range_bound::to_maxvalue(Value::Int64(20)),
            InMemoryTable::new(schema.clone()),
        ));

        // 路由测试
        assert_eq!(
            pt.route_and_insert(vec![Value::Int64(5), Value::Int64(100)])
                .unwrap(),
            0
        );
        assert_eq!(
            pt.route_and_insert(vec![Value::Int64(15), Value::Int64(200)])
                .unwrap(),
            1
        );
        assert_eq!(
            pt.route_and_insert(vec![Value::Int64(25), Value::Int64(300)])
                .unwrap(),
            2
        );

        // 验证分区数据
        assert_eq!(pt.partitions[0].row_count(), 1);
        assert_eq!(pt.partitions[1].row_count(), 1);
        assert_eq!(pt.partitions[2].row_count(), 1);
        assert_eq!(pt.total_row_count(), 3);
    }

    #[test]
    fn test_partitioned_table_range_pruning() {
        let schema = make_schema("t");
        let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::Range, 0);
        pt.add_partition(Partition::new(
            "t_p0",
            range_bound::minvalue_to(Value::Int64(10)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p1",
            range_bound::from_to(Value::Int64(10), Value::Int64(20)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p2",
            range_bound::to_maxvalue(Value::Int64(20)),
            InMemoryTable::new(schema.clone()),
        ));

        // 等值裁剪：key = 15 → 仅 t_p1
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::eq(Value::Int64(15)));
        assert_eq!(pruned, vec![1]);

        // 等值裁剪：key = 5 → 仅 t_p0
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::eq(Value::Int64(5)));
        assert_eq!(pruned, vec![0]);

        // 等值裁剪：key = 25 → 仅 t_p2
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::eq(Value::Int64(25)));
        assert_eq!(pruned, vec![2]);

        // 无约束：全部分区
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::Unconstrained);
        assert_eq!(pruned, vec![0, 1, 2]);
    }

    #[test]
    fn test_partitioned_table_range_range_pruning() {
        let schema = make_schema("t");
        let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::Range, 0);
        pt.add_partition(Partition::new(
            "t_p0",
            range_bound::minvalue_to(Value::Int64(10)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p1",
            range_bound::from_to(Value::Int64(10), Value::Int64(20)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p2",
            range_bound::from_to(Value::Int64(20), Value::Int64(30)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p3",
            range_bound::to_maxvalue(Value::Int64(30)),
            InMemoryTable::new(schema.clone()),
        ));

        // 范围谓词 [12, 18) → 与 t_p1 [10, 20) 相交
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::range_half_open(
            Some(Value::Int64(12)),
            Some(Value::Int64(18)),
        ));
        assert_eq!(pruned, vec![1]);

        // 范围谓词 [5, 25) → 与 t_p0, t_p1, t_p2 相交
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::range_half_open(
            Some(Value::Int64(5)),
            Some(Value::Int64(25)),
        ));
        assert_eq!(pruned, vec![0, 1, 2]);

        // 范围谓词 [100, 200) → 与 t_p3 相交
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::range_half_open(
            Some(Value::Int64(100)),
            Some(Value::Int64(200)),
        ));
        assert_eq!(pruned, vec![3]);

        // 范围谓词 (-∞, 5) → 与 t_p0 相交
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::range_half_open(
            None,
            Some(Value::Int64(5)),
        ));
        assert_eq!(pruned, vec![0]);
    }

    #[test]
    fn test_partitioned_table_in_predicate_pruning() {
        let schema = make_schema("t");
        let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::Range, 0);
        pt.add_partition(Partition::new(
            "t_p0",
            range_bound::minvalue_to(Value::Int64(10)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p1",
            range_bound::from_to(Value::Int64(10), Value::Int64(20)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p2",
            range_bound::to_maxvalue(Value::Int64(20)),
            InMemoryTable::new(schema.clone()),
        ));

        // IN (5, 15, 25) → 与所有分区相交
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::in_list(vec![
            Value::Int64(5),
            Value::Int64(15),
            Value::Int64(25),
        ]));
        assert_eq!(pruned, vec![0, 1, 2]);

        // IN (5, 8) → 仅 t_p0
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::in_list(vec![
            Value::Int64(5),
            Value::Int64(8),
        ]));
        assert_eq!(pruned, vec![0]);

        // IN (15) → 仅 t_p1
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::in_list(vec![Value::Int64(15)]));
        assert_eq!(pruned, vec![1]);
    }

    #[test]
    fn test_partitioned_table_list_routing() {
        use crate::ast::{ColumnDefinition, TableName};
        use szrsql_types::value::ColumnType;
        let schema = TableSchema {
            name: TableName::new("t"),
            columns: vec![
                ColumnDefinition::new("region", ColumnType::Text),
                ColumnDefinition::new("val", ColumnType::Int64),
            ],
        };
        let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::List, 0);
        pt.add_partition(Partition::new(
            "t_east",
            PartitionBound::list(vec![Value::Text("NY".into()), Value::Text("MA".into())]),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_west",
            PartitionBound::list(vec![Value::Text("CA".into()), Value::Text("WA".into())]),
            InMemoryTable::new(schema.clone()),
        ));

        // 路由
        assert_eq!(
            pt.route_and_insert(vec![Value::Text("NY".into()), Value::Int64(1)])
                .unwrap(),
            0
        );
        assert_eq!(
            pt.route_and_insert(vec![Value::Text("CA".into()), Value::Int64(2)])
                .unwrap(),
            1
        );
        assert_eq!(pt.partitions[0].row_count(), 1);
        assert_eq!(pt.partitions[1].row_count(), 1);

        // 不匹配的值应报错（无默认分区）
        let result = pt.route_and_insert(vec![Value::Text("TX".into()), Value::Int64(3)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_partitioned_table_list_with_default() {
        use crate::ast::{ColumnDefinition, TableName};
        use szrsql_types::value::ColumnType;
        let schema = TableSchema {
            name: TableName::new("t"),
            columns: vec![
                ColumnDefinition::new("region", ColumnType::Text),
                ColumnDefinition::new("val", ColumnType::Int64),
            ],
        };
        let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::List, 0);
        pt.add_partition(Partition::new(
            "t_east",
            PartitionBound::list(vec![Value::Text("NY".into())]),
            InMemoryTable::new(schema.clone()),
        ));
        pt.set_default_partition(InMemoryTable::new(schema.clone()));

        // 匹配分区
        assert_eq!(
            pt.route_and_insert(vec![Value::Text("NY".into()), Value::Int64(1)])
                .unwrap(),
            0
        );
        // 不匹配 → 默认分区
        let idx = pt
            .route_and_insert(vec![Value::Text("TX".into()), Value::Int64(2)])
            .unwrap();
        assert_eq!(idx, usize::MAX);

        assert_eq!(pt.partitions[0].row_count(), 1);
        assert_eq!(pt.default_partition.as_ref().unwrap().row_count(), 1);
        assert_eq!(pt.total_row_count(), 2);
    }

    #[test]
    fn test_partitioned_table_hash_routing() {
        let schema = make_schema("t");
        let bounds = hash_partitions_bounds(4);
        let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::Hash, 0);
        for (i, bound) in bounds.into_iter().enumerate() {
            pt.add_partition(Partition::new(
                format!("t_p{i}"),
                bound,
                InMemoryTable::new(schema.clone()),
            ));
        }

        // 路由 100 行，验证每行都路由到正确分区
        for n in 0..100i64 {
            let row = vec![Value::Int64(n), Value::Int64(n * 2)];
            let expected_h = hash_value(&Value::Int64(n));
            let expected_partition = (expected_h % 4) as usize;
            let actual = pt.route_and_insert(row).unwrap();
            assert_eq!(actual, expected_partition, "n={n}, h={expected_h}");
        }

        // 验证总行数
        assert_eq!(pt.total_row_count(), 100);
    }

    #[test]
    fn test_partitioned_table_hash_pruning() {
        let schema = make_schema("t");
        let bounds = hash_partitions_bounds(4);
        let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::Hash, 0);
        for (i, bound) in bounds.into_iter().enumerate() {
            pt.add_partition(Partition::new(
                format!("t_p{i}"),
                bound,
                InMemoryTable::new(schema.clone()),
            ));
        }

        // 等值裁剪：key = 42 → 仅一个分区
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::eq(Value::Int64(42)));
        assert_eq!(pruned.len(), 1);
        let expected_h = hash_value(&Value::Int64(42));
        let expected_idx = (expected_h % 4) as usize;
        assert_eq!(pruned[0], expected_idx);

        // 无约束 → 全部分区
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::Unconstrained);
        assert_eq!(pruned.len(), 4);
    }

    #[test]
    fn test_partitioned_table_scan_all() {
        let schema = make_schema("t");
        let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::Range, 0);
        pt.add_partition(Partition::new(
            "t_p0",
            range_bound::minvalue_to(Value::Int64(10)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p1",
            range_bound::to_maxvalue(Value::Int64(10)),
            InMemoryTable::new(schema.clone()),
        ));

        pt.route_and_insert(vec![Value::Int64(5), Value::Int64(1)])
            .unwrap();
        pt.route_and_insert(vec![Value::Int64(15), Value::Int64(2)])
            .unwrap();
        pt.route_and_insert(vec![Value::Int64(8), Value::Int64(3)])
            .unwrap();
        pt.route_and_insert(vec![Value::Int64(20), Value::Int64(4)])
            .unwrap();

        let all = pt.scan_all();
        assert_eq!(all.len(), 4);

        // 按裁剪后扫描 — key=15 落在 t_p1 [10, +∞)，该分区有 2 行（15 和 20）
        let pruned = pt.prune_partitions(&PartitionPrunePredicate::eq(Value::Int64(15)));
        assert_eq!(pruned, vec![1]);
        let scanned = pt.scan_partitions(&pruned);
        assert_eq!(scanned.len(), 2);
        // 第一行是 (15, 2)
        assert_eq!(scanned[0][0], Value::Int64(15));
        assert_eq!(scanned[0][1], Value::Int64(2));
    }

    #[test]
    fn test_find_partition_by_name() {
        let schema = make_schema("t");
        let mut pt = PartitionedTable::new("t", schema.clone(), PartitionStrategy::Range, 0);
        pt.add_partition(Partition::new(
            "t_p0",
            range_bound::minvalue_to(Value::Int64(10)),
            InMemoryTable::new(schema.clone()),
        ));
        pt.add_partition(Partition::new(
            "t_p1",
            range_bound::to_maxvalue(Value::Int64(10)),
            InMemoryTable::new(schema.clone()),
        ));

        assert_eq!(pt.find_partition_by_name("t_p0"), Some(0));
        assert_eq!(pt.find_partition_by_name("t_p1"), Some(1));
        assert_eq!(pt.find_partition_by_name("T_P0"), Some(0)); // 大小写不敏感
        assert_eq!(pt.find_partition_by_name("t_p2"), None);
    }

    #[test]
    fn test_route_row_no_partition_error() {
        let schema = make_schema("t");
        let mut pt = PartitionedTable::new("t", schema, PartitionStrategy::Range, 0);
        pt.add_partition(Partition::new(
            "t_p0",
            range_bound::from_to(Value::Int64(0), Value::Int64(10)),
            InMemoryTable::new(make_schema("t")),
        ));

        // 5 落在 [0, 10) → 成功
        assert!(pt.route_row(&[Value::Int64(5), Value::Int64(0)]).is_ok());
        // -1 不落在任何分区 → 错误
        assert!(pt.route_row(&[Value::Int64(-1), Value::Int64(0)]).is_err());
        // 15 不落在任何分区 → 错误
        assert!(pt.route_row(&[Value::Int64(15), Value::Int64(0)]).is_err());
    }

    #[test]
    fn test_hash_partitions_bounds_helper() {
        let bounds = hash_partitions_bounds(3);
        assert_eq!(bounds.len(), 3);
        for (i, b) in bounds.iter().enumerate() {
            match b {
                PartitionBound::Hash { remainder, modulus } => {
                    assert_eq!(*remainder, i as u64);
                    assert_eq!(*modulus, 3);
                }
                _ => panic!("expected Hash bound"),
            }
        }

        // count=0 → 空列表
        assert!(hash_partitions_bounds(0).is_empty());
    }

    #[test]
    fn test_partitioned_table_key_column_name() {
        let schema = make_schema("t");
        let pt = PartitionedTable::new("t", schema, PartitionStrategy::Range, 0);
        assert_eq!(pt.key_column_name(), "id");
    }

    #[test]
    fn test_range_bound_with_inclusive_bounds() {
        // 自定义包含标志：[10, 20]（两端都包含）
        let b = PartitionBound::Range {
            lower: Some(Value::Int64(10)),
            upper: Some(Value::Int64(20)),
            lower_inc: true,
            upper_inc: true,
        };
        assert!(b.contains(&Value::Int64(10)));
        assert!(b.contains(&Value::Int64(15)));
        assert!(b.contains(&Value::Int64(20))); // 上界包含
        assert!(!b.contains(&Value::Int64(21)));
    }

    #[test]
    fn test_prune_predicate_eq_may_match_range() {
        let bound = range_bound::from_to(Value::Int64(10), Value::Int64(20));
        let pred = PartitionPrunePredicate::eq(Value::Int64(15));
        assert!(pred.may_match(&bound));

        let pred = PartitionPrunePredicate::eq(Value::Int64(5));
        assert!(!pred.may_match(&bound));

        let pred = PartitionPrunePredicate::eq(Value::Int64(25));
        assert!(!pred.may_match(&bound));
    }

    #[test]
    fn test_prune_predicate_range_may_match_range() {
        // 分区 [10, 20)
        let bound = range_bound::from_to(Value::Int64(10), Value::Int64(20));

        // 谓词 [15, 18) → 相交
        let pred = PartitionPrunePredicate::range_half_open(
            Some(Value::Int64(15)),
            Some(Value::Int64(18)),
        );
        assert!(pred.may_match(&bound));

        // 谓词 [20, 30) → 不相交（上界 20 不包含）
        let pred = PartitionPrunePredicate::range_half_open(
            Some(Value::Int64(20)),
            Some(Value::Int64(30)),
        );
        assert!(!pred.may_match(&bound));

        // 谓词 [0, 5) → 不相交
        let pred =
            PartitionPrunePredicate::range_half_open(Some(Value::Int64(0)), Some(Value::Int64(5)));
        assert!(!pred.may_match(&bound));

        // 谓词 (-∞, 15) → 相交（覆盖 10-14）
        let pred = PartitionPrunePredicate::range_half_open(None, Some(Value::Int64(15)));
        assert!(pred.may_match(&bound));
    }

    #[test]
    fn test_prune_predicate_in_may_match_list() {
        let bound = PartitionBound::list(vec![Value::Text("a".into()), Value::Text("b".into())]);
        // IN ('a', 'c') → 交集 'a'
        let pred = PartitionPrunePredicate::in_list(vec![
            Value::Text("a".into()),
            Value::Text("c".into()),
        ]);
        assert!(pred.may_match(&bound));

        // IN ('c', 'd') → 无交集
        let pred = PartitionPrunePredicate::in_list(vec![
            Value::Text("c".into()),
            Value::Text("d".into()),
        ]);
        assert!(!pred.may_match(&bound));
    }
}
