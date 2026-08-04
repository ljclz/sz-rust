//! Partial Index + Covering Index — Phase 6.20
//!
//! 提供 PG 风格的部分索引（Partial Index）和覆盖索引（Covering Index）：
//!
//! - **Partial Index**：`CREATE INDEX ... WHERE predicate` — 仅索引满足 WHERE 谓词的行
//! - **Covering Index**：`CREATE INDEX ... INCLUDE (cols)` — 索引额外存储 INCLUDE 列，支持 index-only scan（无需回表）
//!
//! # 设计
//!
//! - `PartialCoveringIndex` 是一个 B-Tree 索引，支持多列键 + 可选 INCLUDE 列 + 可选 WHERE 谓词
//! - 键使用 `IndexKey(Vec<Value>)` 包装，通过 `compare_values` 实现全序（不可比类型按 Equal 处理）
//! - 谓词求值复用 `ExprEvaluator` + `RowContext`（从行 + schema 构建）
//! - NULL 键不进索引（与 `InMemoryBTreeIndex` 语义一致）
//! - index-only scan：当请求列全部在 key_columns ∪ included_columns 中时，直接从索引返回数据
//!
//! # 与 PG 的关系
//!
//! - PG 7.2+ 支持部分索引（`CREATE INDEX ... WHERE`）
//! - PG 11+ 支持覆盖索引（`CREATE INDEX ... INCLUDE (cols)`）
//! - PG 部分索引谓词必须仅引用被索引表的列，且只能使用 IMMUTABLE 函数
//! - PG 覆盖索引的 INCLUDE 列不参与排序，仅存储用于 index-only scan
//!
//! # 限制
//!
//! - **无 DDL 集成**：未集成到 `CREATE INDEX` 解析路径（parser 未捕获 WHERE/INCLUDE 子句），仅提供程序化 API
//! - **无唯一约束**：不支持 `CREATE UNIQUE INDEX` 与部分/覆盖索引的组合
//! - **无并发控制**：单线程内存索引，无 MVCC 可见性检查
//! - **NULL 键跳过**：任一键列为 NULL 时整行不进索引（PG 实际索引 NULL，此处简化）

use crate::ast::Expr;
use crate::executor::{ExecutionError, Row, TableStorage};
use crate::expr::{compare_values, ExprEvaluator, RowContext};
use crate::plan::TableSchema;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use szrsql_types::value::Value;

// =====================================================================
//  索引键包装
// =====================================================================

/// 索引键 — 多列值包装，实现 Ord 用于 BTreeMap
///
/// 使用 `compare_values` 做值比较；不可比类型按 `Equal` 处理（保证全序）。
/// 同类型不同值正常比较；不同类型通过类型判别式保证确定性排序。
///
/// 注意：`Eq` 手动实现（`Value` 含 `f64` 无法自动派生 `Eq`）。
/// NaN 键行为未定义（数据库索引通常不处理 NaN）。
#[derive(Debug, Clone, PartialEq)]
pub struct IndexKey(pub Vec<Value>);

impl Eq for IndexKey {}

impl IndexKey {
    /// 创建空键
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// 从值切片创建键
    pub fn from_values(values: &[Value]) -> Self {
        Self(values.to_vec())
    }

    /// 是否包含任一 NULL 值
    pub fn has_null(&self) -> bool {
        self.0.iter().any(|v| matches!(v, Value::Null))
    }

    /// 键值数量
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// 是否为空键
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Default for IndexKey {
    fn default() -> Self {
        Self::new()
    }
}

/// Value 类型判别式 — 保证不同类型有确定性排序
fn value_type_discriminant(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Int64(_) => 2,
        Value::Float64(_) => 3,
        Value::Decimal(_, _) => 4,
        Value::Date(_) => 5,
        Value::Timestamp(_) => 6,
        Value::Text(_) => 7,
        Value::Enum(_) => 8,
        Value::Blob(_) => 9,
        Value::Array(_) => 10,
        Value::Range(_) => 11,
        Value::Json(_) => 12,
        Value::TsVector(_) => 13,
        Value::TsQuery(_) => 14,
        Value::Vector(_) => 15,
        Value::Xml(_) => 16,
    }
}

impl Ord for IndexKey {
    fn cmp(&self, other: &Self) -> Ordering {
        for (a, b) in self.0.iter().zip(other.0.iter()) {
            // 先按类型判别式比较 — 保证不同类型确定性排序
            let type_ord = value_type_discriminant(a).cmp(&value_type_discriminant(b));
            if type_ord != Ordering::Equal {
                return type_ord;
            }
            // 同类型 → 用 compare_values 比较值
            let val_ord = compare_values(a, b).unwrap_or(Ordering::Equal);
            if val_ord != Ordering::Equal {
                return val_ord;
            }
        }
        self.0.len().cmp(&other.0.len())
    }
}

impl PartialOrd for IndexKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// =====================================================================
//  扫描结果
// =====================================================================

/// 索引扫描结果
///
/// 包含 row_id、键列值和 INCLUDE 列值，用于 index-only scan 重建行。
#[derive(Debug, Clone)]
pub struct ScanResult {
    /// 行 ID
    pub row_id: usize,
    /// 键列值（按 key_columns 顺序）
    pub key_values: Vec<Value>,
    /// INCLUDE 列值（按 included_columns 顺序）
    pub included_values: Vec<Value>,
}

// =====================================================================
//  部分索引 + 覆盖索引
// =====================================================================

/// 部分索引 + 覆盖索引
///
/// 支持：
/// - **部分索引**：`predicate` 为 `Some(expr)` 时，仅索引满足谓词的行
/// - **覆盖索引**：`included_columns` 非空时，索引额外存储这些列的值，支持 index-only scan
///
/// # 示例
///
/// ```
/// use szrsql_sql::partial_covering_index::PartialCoveringIndex;
///
/// // 创建部分索引：仅索引 active=true 的行
/// let mut idx = PartialCoveringIndex::new(
///     "idx_active", "users",
///     vec!["status".into()],
///     vec!["name".into()],  // INCLUDE name
///     None,                  // 无 WHERE（全索引）
/// );
/// ```
pub struct PartialCoveringIndex {
    /// 索引名
    name: String,
    /// 所属表名
    table_name: String,
    /// 键列名列表（参与排序）
    key_columns: Vec<String>,
    /// INCLUDE 列名列表（不参与排序，仅存储用于覆盖扫描）
    included_columns: Vec<String>,
    /// WHERE 谓词（None = 无谓词，索引所有行）
    predicate: Option<Expr>,
    /// 索引存储：IndexKey → 行条目列表 (row_id, included_values)
    index: BTreeMap<IndexKey, Vec<(usize, Vec<Value>)>>,
    /// 已索引行数（不含被谓词过滤的行）
    indexed_count: usize,
}

impl PartialCoveringIndex {
    /// 创建新索引
    ///
    /// - `key_columns`：键列名列表（参与 B-Tree 排序）
    /// - `included_columns`：INCLUDE 列名列表（不参与排序，仅存储用于覆盖扫描）
    /// - `predicate`：WHERE 谓词（None = 无谓词，索引所有行）
    pub fn new(
        name: impl Into<String>,
        table_name: impl Into<String>,
        key_columns: Vec<String>,
        included_columns: Vec<String>,
        predicate: Option<Expr>,
    ) -> Self {
        Self {
            name: name.into(),
            table_name: table_name.into(),
            key_columns,
            included_columns,
            predicate,
            index: BTreeMap::new(),
            indexed_count: 0,
        }
    }

    /// 索引名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 所属表名
    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    /// 键列名列表
    pub fn key_columns(&self) -> &[String] {
        &self.key_columns
    }

    /// INCLUDE 列名列表
    pub fn included_columns(&self) -> &[String] {
        &self.included_columns
    }

    /// 是否有 WHERE 谓词（部分索引）
    pub fn has_predicate(&self) -> bool {
        self.predicate.is_some()
    }

    /// 已索引行数
    pub fn indexed_count(&self) -> usize {
        self.indexed_count
    }

    /// 索引中不同键的数量
    pub fn unique_key_count(&self) -> usize {
        self.index.len()
    }

    /// 是否为覆盖索引（能覆盖给定列集合）
    ///
    /// 当 `required_columns` 全部在 key_columns ∪ included_columns 中时返回 true。
    pub fn is_covering(&self, required_columns: &[&str]) -> bool {
        let all_indexed: Vec<&str> = self
            .key_columns
            .iter()
            .map(|s| s.as_str())
            .chain(self.included_columns.iter().map(|s| s.as_str()))
            .collect();
        required_columns
            .iter()
            .all(|req| all_indexed.iter().any(|idx| idx.eq_ignore_ascii_case(req)))
    }

    /// 从行中提取键值
    fn extract_key_values(
        schema: &TableSchema,
        row: &Row,
        key_columns: &[String],
    ) -> Result<Vec<Value>, ExecutionError> {
        key_columns
            .iter()
            .map(|col| {
                let idx = schema
                    .columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(col))
                    .ok_or_else(|| {
                        ExecutionError::InvalidArgument(format!(
                            "key column '{}' not found in table schema",
                            col
                        ))
                    })?;
                row.get(idx).cloned().ok_or_else(|| {
                    ExecutionError::InvalidArgument(format!(
                        "column index {} out of bounds (row has {} columns)",
                        idx,
                        row.len()
                    ))
                })
            })
            .collect()
    }

    /// 从行中提取 INCLUDE 列值
    fn extract_included_values(
        schema: &TableSchema,
        row: &Row,
        included_columns: &[String],
    ) -> Result<Vec<Value>, ExecutionError> {
        Self::extract_key_values(schema, row, included_columns)
    }

    /// 从行 + schema 构建求值上下文
    fn build_row_context(schema: &TableSchema, row: &Row) -> RowContext {
        let mut ctx = RowContext::new();
        let pairs = schema
            .columns
            .iter()
            .zip(row.iter())
            .map(|(col, val)| (col.name.clone(), val.clone()));
        ctx.with_all(pairs);
        ctx
    }

    /// 评估 WHERE 谓词是否匹配行
    ///
    /// - 谓词为 None → 匹配（索引所有行）
    /// - 谓词求值为 Bool(true) → 匹配
    /// - 谓词求值为 Bool(false) / Null / 错误 → 不匹配
    fn predicate_matches(predicate: &Option<Expr>, schema: &TableSchema, row: &Row) -> bool {
        let Some(expr) = predicate else {
            return true; // 无谓词 → 索引所有行
        };
        let ctx = Self::build_row_context(schema, row);
        matches!(ExprEvaluator::eval(expr, &ctx), Ok(Value::Bool(true)))
    }

    /// 插入单行到索引
    ///
    /// - 如果谓词不匹配 → 跳过（不索引）
    /// - 如果任一键列为 NULL → 跳过（与 InMemoryBTreeIndex 语义一致）
    /// - 否则 → 插入索引
    pub fn insert(
        &mut self,
        schema: &TableSchema,
        row_id: usize,
        row: &Row,
    ) -> Result<bool, ExecutionError> {
        // 谓词检查
        if !Self::predicate_matches(&self.predicate, schema, row) {
            return Ok(false);
        }
        // 提取键值
        let key_values = Self::extract_key_values(schema, row, &self.key_columns)?;
        // NULL 键跳过
        if key_values.iter().any(|v| matches!(v, Value::Null)) {
            return Ok(false);
        }
        // 提取 INCLUDE 列值
        let included_values = Self::extract_included_values(schema, row, &self.included_columns)?;
        // 插入索引
        let key = IndexKey(key_values);
        self.index
            .entry(key)
            .or_default()
            .push((row_id, included_values));
        self.indexed_count += 1;
        Ok(true)
    }

    /// 批量构建索引：从表数据构建
    ///
    /// 遍历表所有行，按谓词过滤后插入索引。
    /// 返回已索引的行数。
    pub fn build_from_table(&mut self, table: &dyn TableStorage) -> Result<usize, ExecutionError> {
        let schema = table.schema();
        let mut count = 0;
        for (row_id, row) in table.scan_with_ids() {
            if self.insert(schema, row_id, &row)? {
                count += 1;
            }
        }
        Ok(count)
    }

    /// 点查：返回所有匹配给定键值的扫描结果
    ///
    /// 键值数量必须与 key_columns 数量一致。
    pub fn point_lookup(&self, key_values: &[Value]) -> Vec<ScanResult> {
        if key_values.len() != self.key_columns.len() {
            return Vec::new();
        }
        let key = IndexKey(key_values.to_vec());
        self.index
            .get(&key)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(row_id, included)| ScanResult {
                        row_id: *row_id,
                        key_values: key.0.clone(),
                        included_values: included.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 范围查询 [low, high]（含两端）：返回所有匹配键的扫描结果（按键升序）
    ///
    /// low 和 high 的键值数量必须与 key_columns 数量一致。
    /// 若 low > high（键比较），返回空。
    pub fn range_lookup(&self, low: &[Value], high: &[Value]) -> Vec<ScanResult> {
        if low.len() != self.key_columns.len() || high.len() != self.key_columns.len() {
            return Vec::new();
        }
        let low_key = IndexKey(low.to_vec());
        let high_key = IndexKey(high.to_vec());
        if low_key > high_key {
            return Vec::new();
        }
        self.index
            .range(low_key..=high_key)
            .flat_map(|(key, entries)| {
                entries.iter().map(|(row_id, included)| ScanResult {
                    row_id: *row_id,
                    key_values: key.0.clone(),
                    included_values: included.clone(),
                })
            })
            .collect()
    }

    /// 全索引扫描：返回所有索引项（按键升序）
    pub fn scan_all(&self) -> Vec<ScanResult> {
        self.index
            .iter()
            .flat_map(|(key, entries)| {
                entries.iter().map(|(row_id, included)| ScanResult {
                    row_id: *row_id,
                    key_values: key.0.clone(),
                    included_values: included.clone(),
                })
            })
            .collect()
    }

    /// Index-only scan：点查 + 覆盖检查
    ///
    /// 如果索引覆盖 `required_columns`，直接从索引重建行数据（无需回表）。
    /// 返回 `Some(rows)` 如果覆盖；返回 `None` 如果不覆盖（需要回表）。
    ///
    /// 重建的行按 `required_columns` 顺序排列。
    pub fn index_only_scan(
        &self,
        key_values: &[Value],
        required_columns: &[&str],
    ) -> Option<Vec<Vec<Value>>> {
        if !self.is_covering(required_columns) {
            return None;
        }
        let results = self.point_lookup(key_values);
        let rows = results
            .iter()
            .map(|r| self.reconstruct_row(r, required_columns))
            .collect();
        Some(rows)
    }

    /// Index-only scan：范围查询 + 覆盖检查
    ///
    /// 如果索引覆盖 `required_columns`，直接从索引重建行数据（无需回表）。
    pub fn index_only_scan_range(
        &self,
        low: &[Value],
        high: &[Value],
        required_columns: &[&str],
    ) -> Option<Vec<Vec<Value>>> {
        if !self.is_covering(required_columns) {
            return None;
        }
        let results = self.range_lookup(low, high);
        let rows = results
            .iter()
            .map(|r| self.reconstruct_row(r, required_columns))
            .collect();
        Some(rows)
    }

    /// Index-only scan：全索引扫描 + 覆盖检查
    pub fn index_only_scan_all(&self, required_columns: &[&str]) -> Option<Vec<Vec<Value>>> {
        if !self.is_covering(required_columns) {
            return None;
        }
        let results = self.scan_all();
        let rows = results
            .iter()
            .map(|r| self.reconstruct_row(r, required_columns))
            .collect();
        Some(rows)
    }

    /// 从扫描结果重建行数据（按 required_columns 顺序）
    fn reconstruct_row(&self, result: &ScanResult, required_columns: &[&str]) -> Vec<Value> {
        required_columns
            .iter()
            .map(|req| {
                // 先在键列中查找
                for (i, key_col) in self.key_columns.iter().enumerate() {
                    if key_col.eq_ignore_ascii_case(req) {
                        return result.key_values.get(i).cloned().unwrap_or(Value::Null);
                    }
                }
                // 再在 INCLUDE 列中查找
                for (i, inc_col) in self.included_columns.iter().enumerate() {
                    if inc_col.eq_ignore_ascii_case(req) {
                        return result
                            .included_values
                            .get(i)
                            .cloned()
                            .unwrap_or(Value::Null);
                    }
                }
                Value::Null // 不应该发生（is_covering 已验证）
            })
            .collect()
    }

    /// 清空索引
    pub fn clear(&mut self) {
        self.index.clear();
        self.indexed_count = 0;
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{ColumnDefinition, Expr, TableName};
    use crate::executor::{InMemoryTable, MutableTable};
    use szrsql_types::value::ColumnType;

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    /// 创建测试表 schema: (id INT, status TEXT, active BOOL, name TEXT, age INT)
    fn make_test_schema() -> TableSchema {
        TableSchema {
            name: TableName::new("users"),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("status", ColumnType::Text),
                ColumnDefinition::new("active", ColumnType::Bool),
                ColumnDefinition::new("name", ColumnType::Text),
                ColumnDefinition::new("age", ColumnType::Int64),
            ],
        }
    }

    /// 创建测试表并插入数据
    fn make_test_table() -> InMemoryTable {
        let schema = make_test_schema();
        let mut table = InMemoryTable::new(schema);
        // id=1, status='active', active=true, name='Alice', age=30
        table.insert_row(vec![
            Value::Int64(1),
            Value::Text("active".into()),
            Value::Bool(true),
            Value::Text("Alice".into()),
            Value::Int64(30),
        ]);
        // id=2, status='inactive', active=false, name='Bob', age=25
        table.insert_row(vec![
            Value::Int64(2),
            Value::Text("inactive".into()),
            Value::Bool(false),
            Value::Text("Bob".into()),
            Value::Int64(25),
        ]);
        // id=3, status='active', active=true, name='Carol', age=35
        table.insert_row(vec![
            Value::Int64(3),
            Value::Text("active".into()),
            Value::Bool(true),
            Value::Text("Carol".into()),
            Value::Int64(35),
        ]);
        // id=4, status='pending', active=false, name='Dave', age=40
        table.insert_row(vec![
            Value::Int64(4),
            Value::Text("pending".into()),
            Value::Bool(false),
            Value::Text("Dave".into()),
            Value::Int64(40),
        ]);
        table
    }

    /// 构建 `active = true` 谓词表达式
    fn make_active_predicate() -> Expr {
        // active = true → BinaryOp(Eq, Identifier("active"), Literal(Bool(true)))
        use crate::ast::{BinaryOp, Expr};
        Expr::BinaryOp {
            left: Box::new(Expr::Identifier(vec!["active".into()])),
            op: BinaryOp::Eq,
            right: Box::new(Expr::Literal(Value::Bool(true))),
        }
    }

    /// 构建 `age > 28` 谓词表达式
    fn make_age_gt_28_predicate() -> Expr {
        use crate::ast::{BinaryOp, Expr};
        Expr::BinaryOp {
            left: Box::new(Expr::Identifier(vec!["age".into()])),
            op: BinaryOp::Gt,
            right: Box::new(Expr::Literal(Value::Int64(28))),
        }
    }

    // -----------------------------------------------------------------
    //  IndexKey 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_index_key_ord_basic() {
        let k1 = IndexKey::from_values(&[Value::Int64(1)]);
        let k2 = IndexKey::from_values(&[Value::Int64(2)]);
        let k3 = IndexKey::from_values(&[Value::Int64(1)]);
        assert!(k1 < k2);
        assert!(k1 == k3);
        assert!(k2 > k1);
    }

    #[test]
    fn test_index_key_ord_multi_column() {
        let k1 = IndexKey::from_values(&[Value::Int64(1), Value::Int64(10)]);
        let k2 = IndexKey::from_values(&[Value::Int64(1), Value::Int64(20)]);
        let k3 = IndexKey::from_values(&[Value::Int64(2), Value::Int64(5)]);
        assert!(k1 < k2); // 同第一列，第二列 10 < 20
        assert!(k2 < k3); // 第一列 1 < 2
    }

    #[test]
    fn test_index_key_has_null() {
        let k1 = IndexKey::from_values(&[Value::Int64(1), Value::Null]);
        assert!(k1.has_null());
        let k2 = IndexKey::from_values(&[Value::Int64(1), Value::Int64(2)]);
        assert!(!k2.has_null());
    }

    #[test]
    fn test_index_key_type_discriminant() {
        // 不同类型通过判别式排序，不依赖 compare_values
        let k_int = IndexKey::from_values(&[Value::Int64(100)]);
        let k_text = IndexKey::from_values(&[Value::Text("zzz".into())]);
        let k_bool = IndexKey::from_values(&[Value::Bool(true)]);
        // 判别式：Bool(1) < Int64(2) < Text(7)
        assert!(k_bool < k_int);
        assert!(k_int < k_text);
    }

    // -----------------------------------------------------------------
    //  is_covering 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_is_covering_key_only() {
        let idx = PartialCoveringIndex::new("idx1", "t", vec!["id".into()], vec![], None);
        // 仅键列 → 覆盖 id
        assert!(idx.is_covering(&["id"]));
        // 不覆盖 name
        assert!(!idx.is_covering(&["name"]));
    }

    #[test]
    fn test_is_covering_with_include() {
        let idx = PartialCoveringIndex::new(
            "idx1",
            "t",
            vec!["id".into()],
            vec!["name".into(), "age".into()],
            None,
        );
        // 键 + INCLUDE → 覆盖 id, name, age
        assert!(idx.is_covering(&["id", "name", "age"]));
        // 顺序无关
        assert!(idx.is_covering(&["age", "id"]));
        // 不覆盖 status
        assert!(!idx.is_covering(&["id", "status"]));
    }

    #[test]
    fn test_is_covering_empty_required() {
        let idx = PartialCoveringIndex::new("idx1", "t", vec!["id".into()], vec![], None);
        // 空需求 → 总是覆盖
        assert!(idx.is_covering(&[]));
    }

    // -----------------------------------------------------------------
    //  部分索引测试
    // -----------------------------------------------------------------

    #[test]
    fn test_partial_index_build_from_table() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_active",
            "users",
            vec!["id".into()],
            vec![],
            Some(make_active_predicate()),
        );
        let count = idx.build_from_table(&table).unwrap();
        // active=true 的行：id=1, id=3 → 2 行
        assert_eq!(count, 2);
        assert_eq!(idx.indexed_count(), 2);

        // 验证：点查 id=1 存在
        let results = idx.point_lookup(&[Value::Int64(1)]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row_id, 0);

        // 验证：点查 id=2 不存在（被谓词过滤）
        let results = idx.point_lookup(&[Value::Int64(2)]);
        assert!(results.is_empty());

        // 验证：点查 id=3 存在
        let results = idx.point_lookup(&[Value::Int64(3)]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row_id, 2);
    }

    #[test]
    fn test_partial_index_predicate_false_not_indexed() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_active",
            "users",
            vec!["id".into()],
            vec![],
            Some(make_active_predicate()),
        );
        idx.build_from_table(&table).unwrap();

        // id=2 active=false → 不在索引中
        assert!(idx.point_lookup(&[Value::Int64(2)]).is_empty());
        // id=4 active=false → 不在索引中
        assert!(idx.point_lookup(&[Value::Int64(4)]).is_empty());
    }

    #[test]
    fn test_partial_index_no_predicate_indexes_all() {
        let table = make_test_table();
        let mut idx =
            PartialCoveringIndex::new("idx_all", "users", vec!["id".into()], vec![], None);
        let count = idx.build_from_table(&table).unwrap();
        // 无谓词 → 索引所有 4 行
        assert_eq!(count, 4);
        // 所有 id 都能查到
        for id in [1, 2, 3, 4] {
            assert!(!idx.point_lookup(&[Value::Int64(id)]).is_empty());
        }
    }

    #[test]
    fn test_partial_index_complex_predicate() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_age_gt_28",
            "users",
            vec!["id".into()],
            vec![],
            Some(make_age_gt_28_predicate()),
        );
        let count = idx.build_from_table(&table).unwrap();
        // age > 28: Alice(30), Carol(35), Dave(40) → 3 行；Bob(25) 不满足
        assert_eq!(count, 3);

        // Bob(id=2, age=25) 不在索引中
        assert!(idx.point_lookup(&[Value::Int64(2)]).is_empty());
        // Alice(id=1, age=30) 在索引中
        assert!(!idx.point_lookup(&[Value::Int64(1)]).is_empty());
    }

    // -----------------------------------------------------------------
    //  覆盖索引测试
    // -----------------------------------------------------------------

    #[test]
    fn test_covering_index_included_values() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_covering",
            "users",
            vec!["id".into()],
            vec!["name".into(), "age".into()],
            None,
        );
        idx.build_from_table(&table).unwrap();

        // 点查 id=1 → 返回 INCLUDE 列值
        let results = idx.point_lookup(&[Value::Int64(1)]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].included_values.len(), 2);
        assert_eq!(results[0].included_values[0], Value::Text("Alice".into()));
        assert_eq!(results[0].included_values[1], Value::Int64(30));
    }

    #[test]
    fn test_covering_index_only_scan() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_covering",
            "users",
            vec!["id".into()],
            vec!["name".into()],
            None,
        );
        idx.build_from_table(&table).unwrap();

        // index-only scan：请求 id, name → 覆盖 → 无需回表
        let rows = idx.index_only_scan(&[Value::Int64(3)], &["id", "name"]);
        assert!(rows.is_some());
        let rows = rows.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::Int64(3)); // id
        assert_eq!(rows[0][1], Value::Text("Carol".into())); // name
    }

    #[test]
    fn test_covering_index_only_scan_not_covering() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_partial_cover",
            "users",
            vec!["id".into()],
            vec!["name".into()],
            None,
        );
        idx.build_from_table(&table).unwrap();

        // 请求 status → 不在索引中 → 返回 None（需要回表）
        let rows = idx.index_only_scan(&[Value::Int64(1)], &["id", "status"]);
        assert!(rows.is_none());
    }

    #[test]
    fn test_covering_index_only_scan_range() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_range_cover",
            "users",
            vec!["id".into()],
            vec!["name".into(), "age".into()],
            None,
        );
        idx.build_from_table(&table).unwrap();

        // 范围查询 [2, 4] → 覆盖 id, name, age
        let rows = idx.index_only_scan_range(
            &[Value::Int64(2)],
            &[Value::Int64(4)],
            &["id", "name", "age"],
        );
        assert!(rows.is_some());
        let rows = rows.unwrap();
        assert_eq!(rows.len(), 3); // id=2, 3, 4
                                   // 验证按键升序
        assert_eq!(rows[0][0], Value::Int64(2));
        assert_eq!(rows[0][1], Value::Text("Bob".into()));
        assert_eq!(rows[0][2], Value::Int64(25));
        assert_eq!(rows[1][0], Value::Int64(3));
        assert_eq!(rows[1][1], Value::Text("Carol".into()));
        assert_eq!(rows[2][0], Value::Int64(4));
        assert_eq!(rows[2][1], Value::Text("Dave".into()));
    }

    #[test]
    fn test_covering_index_only_scan_all() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_scan_all",
            "users",
            vec!["id".into()],
            vec!["name".into()],
            None,
        );
        idx.build_from_table(&table).unwrap();

        let rows = idx.index_only_scan_all(&["id", "name"]);
        assert!(rows.is_some());
        let rows = rows.unwrap();
        assert_eq!(rows.len(), 4); // 所有 4 行
                                   // 按键升序
        assert_eq!(rows[0][0], Value::Int64(1));
        assert_eq!(rows[1][0], Value::Int64(2));
        assert_eq!(rows[2][0], Value::Int64(3));
        assert_eq!(rows[3][0], Value::Int64(4));
    }

    // -----------------------------------------------------------------
    //  部分索引 + 覆盖索引组合测试
    // -----------------------------------------------------------------

    #[test]
    fn test_combined_partial_and_covering() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_active_name",
            "users",
            vec!["id".into()],
            vec!["name".into()],
            Some(make_active_predicate()),
        );
        let count = idx.build_from_table(&table).unwrap();
        // active=true: id=1(Alice), id=3(Carol) → 2 行
        assert_eq!(count, 2);

        // index-only scan: 请求 id, name → 覆盖
        let rows = idx.index_only_scan(&[Value::Int64(1)], &["id", "name"]);
        assert!(rows.is_some());
        let rows = rows.unwrap();
        assert_eq!(rows[0][1], Value::Text("Alice".into()));

        // id=2 (Bob, active=false) 不在索引中
        let rows = idx.index_only_scan(&[Value::Int64(2)], &["id", "name"]);
        assert!(rows.is_some());
        assert!(rows.unwrap().is_empty());

        // 请求 age → 不覆盖
        let rows = idx.index_only_scan(&[Value::Int64(1)], &["id", "age"]);
        assert!(rows.is_none());
    }

    // -----------------------------------------------------------------
    //  多列键测试
    // -----------------------------------------------------------------

    #[test]
    fn test_multi_column_key_point_lookup() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_multi",
            "users",
            vec!["status".into(), "age".into()],
            vec!["name".into()],
            None,
        );
        idx.build_from_table(&table).unwrap();

        // 点查 (active, 30) → Alice
        let results = idx.point_lookup(&[Value::Text("active".into()), Value::Int64(30)]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].included_values[0], Value::Text("Alice".into()));

        // 点查 (active, 35) → Carol
        let results = idx.point_lookup(&[Value::Text("active".into()), Value::Int64(35)]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].included_values[0], Value::Text("Carol".into()));

        // 点查 (inactive, 25) → Bob
        let results = idx.point_lookup(&[Value::Text("inactive".into()), Value::Int64(25)]);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_multi_column_key_range_lookup() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_multi_range",
            "users",
            vec!["status".into(), "age".into()],
            vec![],
            None,
        );
        idx.build_from_table(&table).unwrap();

        // 范围查询 [("active", 0), ("active", 100)] → Alice(30), Carol(35)
        let results = idx.range_lookup(
            &[Value::Text("active".into()), Value::Int64(0)],
            &[Value::Text("active".into()), Value::Int64(100)],
        );
        assert_eq!(results.len(), 2);
    }

    // -----------------------------------------------------------------
    //  点查/范围查/全扫测试
    // -----------------------------------------------------------------

    #[test]
    fn test_point_lookup_not_found() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new("idx1", "users", vec!["id".into()], vec![], None);
        idx.build_from_table(&table).unwrap();

        // 查不存在的键
        assert!(idx.point_lookup(&[Value::Int64(999)]).is_empty());
    }

    #[test]
    fn test_point_lookup_wrong_key_length() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx1",
            "users",
            vec!["id".into(), "status".into()],
            vec![],
            None,
        );
        idx.build_from_table(&table).unwrap();

        // 键长度不匹配 → 返回空
        assert!(idx.point_lookup(&[Value::Int64(1)]).is_empty());
    }

    #[test]
    fn test_range_lookup_basic() {
        let table = make_test_table();
        let mut idx =
            PartialCoveringIndex::new("idx_range", "users", vec!["id".into()], vec![], None);
        idx.build_from_table(&table).unwrap();

        // 范围 [2, 3] → id=2, id=3
        let results = idx.range_lookup(&[Value::Int64(2)], &[Value::Int64(3)]);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].key_values[0], Value::Int64(2));
        assert_eq!(results[1].key_values[0], Value::Int64(3));
    }

    #[test]
    fn test_range_lookup_empty_range() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new("idx1", "users", vec!["id".into()], vec![], None);
        idx.build_from_table(&table).unwrap();

        // low > high → 空
        let results = idx.range_lookup(&[Value::Int64(3)], &[Value::Int64(1)]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_range_lookup_no_matches() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new("idx1", "users", vec!["id".into()], vec![], None);
        idx.build_from_table(&table).unwrap();

        // 范围 [100, 200] → 无匹配
        let results = idx.range_lookup(&[Value::Int64(100)], &[Value::Int64(200)]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_scan_all_ordered() {
        let table = make_test_table();
        let mut idx =
            PartialCoveringIndex::new("idx_scan", "users", vec!["id".into()], vec![], None);
        idx.build_from_table(&table).unwrap();

        let results = idx.scan_all();
        assert_eq!(results.len(), 4);
        // 按键升序
        assert_eq!(results[0].key_values[0], Value::Int64(1));
        assert_eq!(results[1].key_values[0], Value::Int64(2));
        assert_eq!(results[2].key_values[0], Value::Int64(3));
        assert_eq!(results[3].key_values[0], Value::Int64(4));
    }

    // -----------------------------------------------------------------
    //  NULL 键测试
    // -----------------------------------------------------------------

    #[test]
    fn test_null_key_not_indexed() {
        let schema = make_test_schema();
        let mut table = InMemoryTable::new(schema);
        // id=NULL, status='x', active=true, name='X', age=1
        table.insert_row(vec![
            Value::Null,
            Value::Text("x".into()),
            Value::Bool(true),
            Value::Text("X".into()),
            Value::Int64(1),
        ]);
        // id=5, status='y', active=true, name='Y', age=2
        table.insert_row(vec![
            Value::Int64(5),
            Value::Text("y".into()),
            Value::Bool(true),
            Value::Text("Y".into()),
            Value::Int64(2),
        ]);

        let mut idx = PartialCoveringIndex::new("idx1", "users", vec!["id".into()], vec![], None);
        let count = idx.build_from_table(&table).unwrap();
        // id=NULL 不进索引，只有 id=5 进索引
        assert_eq!(count, 1);
    }

    // -----------------------------------------------------------------
    //  重复键测试
    // -----------------------------------------------------------------

    #[test]
    fn test_duplicate_keys() {
        let schema = make_test_schema();
        let mut table = InMemoryTable::new(schema);
        // 两行 status='active'
        table.insert_row(vec![
            Value::Int64(1),
            Value::Text("active".into()),
            Value::Bool(true),
            Value::Text("A".into()),
            Value::Int64(10),
        ]);
        table.insert_row(vec![
            Value::Int64(2),
            Value::Text("active".into()),
            Value::Bool(true),
            Value::Text("B".into()),
            Value::Int64(20),
        ]);

        let mut idx = PartialCoveringIndex::new(
            "idx_status",
            "users",
            vec!["status".into()],
            vec!["name".into()],
            None,
        );
        idx.build_from_table(&table).unwrap();

        // 点查 status='active' → 返回 2 条
        let results = idx.point_lookup(&[Value::Text("active".into())]);
        assert_eq!(results.len(), 2);
    }

    // -----------------------------------------------------------------
    //  插入后构建测试
    // -----------------------------------------------------------------

    #[test]
    fn test_insert_respects_predicate() {
        let table = make_test_table();
        let schema = table.schema().clone();
        let mut idx = PartialCoveringIndex::new(
            "idx_active",
            "users",
            vec!["id".into()],
            vec![],
            Some(make_active_predicate()),
        );
        idx.build_from_table(&table).unwrap();
        assert_eq!(idx.indexed_count(), 2);

        // 手动插入 active=false 的行 → 不进索引
        let inserted = idx
            .insert(
                &schema,
                10,
                &vec![
                    Value::Int64(10),
                    Value::Text("inactive".into()),
                    Value::Bool(false),
                    Value::Text("Z".into()),
                    Value::Int64(99),
                ],
            )
            .unwrap();
        assert!(!inserted); // 被谓词过滤
        assert_eq!(idx.indexed_count(), 2); // 未增加

        // 手动插入 active=true 的行 → 进索引
        let inserted = idx
            .insert(
                &schema,
                11,
                &vec![
                    Value::Int64(11),
                    Value::Text("active".into()),
                    Value::Bool(true),
                    Value::Text("W".into()),
                    Value::Int64(88),
                ],
            )
            .unwrap();
        assert!(inserted);
        assert_eq!(idx.indexed_count(), 3);
    }

    // -----------------------------------------------------------------
    //  空索引测试
    // -----------------------------------------------------------------

    #[test]
    fn test_empty_index() {
        let idx = PartialCoveringIndex::new("idx_empty", "users", vec!["id".into()], vec![], None);
        assert_eq!(idx.indexed_count(), 0);
        assert!(idx.point_lookup(&[Value::Int64(1)]).is_empty());
        assert!(idx.scan_all().is_empty());
    }

    // -----------------------------------------------------------------
    //  元数据测试
    // -----------------------------------------------------------------

    #[test]
    fn test_metadata() {
        let idx = PartialCoveringIndex::new(
            "idx_test",
            "users",
            vec!["id".into(), "status".into()],
            vec!["name".into()],
            Some(make_active_predicate()),
        );
        assert_eq!(idx.name(), "idx_test");
        assert_eq!(idx.table_name(), "users");
        assert_eq!(idx.key_columns().len(), 2);
        assert_eq!(idx.included_columns().len(), 1);
        assert!(idx.has_predicate());
    }

    #[test]
    fn test_clear() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new("idx1", "users", vec!["id".into()], vec![], None);
        idx.build_from_table(&table).unwrap();
        assert_eq!(idx.indexed_count(), 4);

        idx.clear();
        assert_eq!(idx.indexed_count(), 0);
        assert!(idx.scan_all().is_empty());
    }

    #[test]
    fn test_unique_key_count() {
        let table = make_test_table();
        let mut idx =
            PartialCoveringIndex::new("idx_status", "users", vec!["status".into()], vec![], None);
        idx.build_from_table(&table).unwrap();
        // 4 行，status: active, inactive, active, pending → 3 个唯一键
        assert_eq!(idx.unique_key_count(), 3);
    }

    // -----------------------------------------------------------------
    //  重建行顺序测试
    // -----------------------------------------------------------------

    #[test]
    fn test_reconstruct_row_order() {
        let table = make_test_table();
        let mut idx = PartialCoveringIndex::new(
            "idx_order",
            "users",
            vec!["id".into()],
            vec!["name".into(), "age".into()],
            None,
        );
        idx.build_from_table(&table).unwrap();

        // 请求 [name, id, age] — 顺序与索引列顺序不同
        let rows = idx.index_only_scan(&[Value::Int64(1)], &["name", "id", "age"]);
        assert!(rows.is_some());
        let rows = rows.unwrap();
        assert_eq!(rows[0][0], Value::Text("Alice".into())); // name
        assert_eq!(rows[0][1], Value::Int64(1)); // id
        assert_eq!(rows[0][2], Value::Int64(30)); // age
    }
}
