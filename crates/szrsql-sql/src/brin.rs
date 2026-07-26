//! BRIN 块范围索引（Block Range Index）— Phase 6.27
//!
//! 提供 PG 风格的 BRIN 索引功能：
//!
//! - **块范围摘要**：将表按行块分组，每块存储 min/max/count/null_count
//! - **范围查询加速**：对自然有序数据（如时序）的范围扫描，索引大小 < B-Tree 的 1%
//! - **紧凑存储**：仅存摘要，不存原始值（适合海量有序数据）
//!
//! # 设计
//!
//! - **BlockRange**：单个块的摘要（block_idx/min/max/count/null_count/has_values）
//! - **BrinIndex**：索引主体，含 `Vec<BlockRange>` + 构建中的当前块缓冲
//! - **BrinRange**：范围查询条件（lower/upper，闭区间）
//! - **value_compare**：类型感知 Value 比较（Value 未实现 PartialOrd）
//!
//! # 与 PG 的关系
//!
//! - PG 9.5+ 支持 BRIN 索引
//! - PG 的 BRIN 按"块范围"（默认 128 pages = 1MB）存储 min/max/null bitmap
//! - PG 适用场景：时序数据、日志表（自然有序，海量行）
//! - PG 不适用场景：随机分布数据（块范围 min/max 重叠严重，过滤效果差）
//! - `CREATE INDEX ON t USING BRIN (ts)` → 时序列建 BRIN
//! - 范围查询 `WHERE ts BETWEEN '2024-01-01' AND '2024-01-31'` → BRIN 过滤无关块
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：未集成到 SQL 解析路径，仅提供程序化 API
//! - **无持久化**：纯内存索引，不落盘
//! - **有序假设**：插入顺序即物理顺序（PG 中数据按 page 物理存储，BRIN 假设局部有序）
//! - **单列索引**：不支持多列复合 BRIN
//! - **无反算**：范围查询只返回块索引，需调用方回表扫描对应块
//! - **类型有限**：min/max 比较仅支持 Int64/Float64/Text/Bool/Date/Timestamp/Decimal

use crate::executor::ExecutionError;
use szrsql_types::value::Value;

// =====================================================================
//  错误类型
// =====================================================================

/// BRIN 索引错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrinError {
    /// 块大小必须 > 0
    #[error("block size must be > 0, got {0}")]
    InvalidBlockSize(usize),
    /// 块索引越界
    #[error("block index out of range: {0} (num_blocks={1})")]
    BlockIndexOutOfRange(usize, usize),
    /// 类型不匹配（范围查询的边界类型与索引列类型不一致）
    #[error("type mismatch: cannot compare {0} with {1}")]
    TypeMismatch(String, String),
    /// 索引为空
    #[error("index is empty")]
    EmptyIndex,
    /// 不支持比较的类型
    #[error("unsupported value type for BRIN: {0}")]
    UnsupportedType(String),
}

impl From<BrinError> for ExecutionError {
    fn from(e: BrinError) -> Self {
        ExecutionError::EvalError(format!("BRIN error: {e}"))
    }
}

// =====================================================================
//  类型感知 Value 比较
// =====================================================================

/// 类型感知的 Value 比较 — 返回 Ordering
///
/// `Value` 未实现 `PartialOrd`，故在此提供本地比较函数。
/// 仅支持常用类型（Null/Int64/Float64/Text/Bool/Date/Timestamp/Decimal），
/// 跨类型数值比较按 Int64↔Float64↔Decimal 隐式转换，其余按 Debug 字符串排序。
fn value_compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Int64(x), Value::Int64(y)) => x.cmp(y),
        (Value::Float64(x), Value::Float64(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Int64(x), Value::Float64(y)) => {
            (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (Value::Float64(x), Value::Int64(y)) => {
            x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
        }
        (Value::Text(x), Value::Text(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::Timestamp(x), Value::Timestamp(y)) => x.cmp(y),
        (Value::Decimal(x, _), Value::Decimal(y, _)) => x.cmp(y),
        (Value::Int64(x), Value::Decimal(y, _)) => (*x as i128).cmp(y),
        (Value::Decimal(x, _), Value::Int64(y)) => x.cmp(&(*y as i128)),
        _ => format!("{a:?}").cmp(&format!("{b:?}")),
    }
}

/// 取两个非 NULL 值的较小者
fn value_min(a: &Value, b: &Value) -> Value {
    if value_compare(a, b) == std::cmp::Ordering::Less {
        a.clone()
    } else {
        b.clone()
    }
}

/// 取两个非 NULL 值的较大者
fn value_max(a: &Value, b: &Value) -> Value {
    if value_compare(a, b) == std::cmp::Ordering::Greater {
        a.clone()
    } else {
        b.clone()
    }
}

// =====================================================================
//  BlockRange — 块范围摘要
// =====================================================================

/// 单个块的摘要信息
///
/// 对应 PG BRIN 的 `BrinMemTuple`（revmap 指向的摘要元组）。
#[derive(Debug, Clone, PartialEq)]
pub struct BlockRange {
    /// 块索引（从 0 开始）
    pub block_idx: usize,
    /// 块内最小值（NULL 视为不参与；若块全 NULL 则为 None）
    pub min: Option<Value>,
    /// 块内最大值
    pub max: Option<Value>,
    /// 块内总行数（含 NULL）
    pub count: usize,
    /// 块内 NULL 行数
    pub null_count: usize,
}

impl BlockRange {
    /// 是否有非 NULL 值
    pub fn has_values(&self) -> bool {
        self.min.is_some()
    }

    /// 块内非 NULL 行数
    pub fn non_null_count(&self) -> usize {
        self.count - self.null_count
    }

    /// 检查此块是否可能与查询范围相交
    ///
    /// 返回 `true` 表示块可能包含匹配行（需回表验证）。
    /// 返回 `false` 表示块一定不含匹配行（可安全跳过）。
    ///
    /// # 规则
    ///
    /// - 块全 NULL + 无界查询 → true（NULL 可能匹配）
    /// - 块全 NULL + 有界查询 → false（NULL 不在范围内）
    /// - 无界查询 → true
    /// - 仅 lower → 块 max >= lower
    /// - 仅 upper → 块 min <= upper
    /// - 双界 → 块 max >= lower 且 块 min <= upper
    pub fn overlaps(&self, range: &BrinRange) -> bool {
        // 块全 NULL
        if !self.has_values() {
            // 无界查询：NULL 可能匹配（PG 语义：NULL 在无界范围中算匹配）
            return range.lower.is_none() && range.upper.is_none();
        }

        let block_min = self.min.as_ref().unwrap();
        let block_max = self.max.as_ref().unwrap();

        // 检查下界：block_max >= lower
        if let Some(lower) = &range.lower {
            if value_compare(block_max, lower) == std::cmp::Ordering::Less {
                return false; // 块最大值 < 下界 → 无交集
            }
        }

        // 检查上界：block_min <= upper
        if let Some(upper) = &range.upper {
            if value_compare(block_min, upper) == std::cmp::Ordering::Greater {
                return false; // 块最小值 > 上界 → 无交集
            }
        }

        true
    }
}

// =====================================================================
//  BrinRange — 范围查询条件
// =====================================================================

/// 范围查询条件（闭区间 [lower, upper]）
///
/// - `lower = None` 表示无下界（-∞）
/// - `upper = None` 表示无上界（+∞）
/// - 两者皆 None 表示全表扫描（匹配所有块）
#[derive(Debug, Clone, Default)]
pub struct BrinRange {
    /// 下界（含），None 表示无下界
    pub lower: Option<Value>,
    /// 上界（含），None 表示无上界
    pub upper: Option<Value>,
}

impl BrinRange {
    /// 创建空范围（无界，匹配所有）
    pub fn all() -> Self {
        Self {
            lower: None,
            upper: None,
        }
    }

    /// 创建下界范围 [lower, +∞)
    pub fn lower_bound(lower: Value) -> Self {
        Self {
            lower: Some(lower),
            upper: None,
        }
    }

    /// 创建上界范围 (-∞, upper]
    pub fn upper_bound(upper: Value) -> Self {
        Self {
            lower: None,
            upper: Some(upper),
        }
    }

    /// 创建闭区间范围 [lower, upper]
    pub fn between(lower: Value, upper: Value) -> Self {
        Self {
            lower: Some(lower),
            upper: Some(upper),
        }
    }

    /// 是否无界
    pub fn is_unbounded(&self) -> bool {
        self.lower.is_none() && self.upper.is_none()
    }
}

// =====================================================================
//  BrinStats — 索引统计
// =====================================================================

/// BRIN 索引统计信息
#[derive(Debug, Clone, PartialEq)]
pub struct BrinStats {
    /// 块数量
    pub num_blocks: usize,
    /// 每块行数
    pub block_size: usize,
    /// 总行数（含 NULL）
    pub total_rows: usize,
    /// 总 NULL 行数
    pub total_nulls: usize,
    /// 估算索引字节数
    pub size_bytes: usize,
    /// 与全量 B-Tree 的压缩比（B-Tree 估算 / BRIN 估算）
    /// B-Tree 每行约 16 字节（键 + 指针），BRIN 每块约 100 字节
    pub compression_ratio: f64,
}

// =====================================================================
//  BrinIndex — BRIN 索引主体
// =====================================================================

/// BRIN 块范围索引
///
/// 将行按 `block_size` 分块，每块存储 min/max/count/null_count 摘要。
/// 范围查询通过块摘要过滤无关块，减少回表扫描。
///
/// # 用法
///
/// ```ignore
/// use szrsql_sql::brin::*;
/// use szrsql_types::value::Value;
///
/// // 创建索引，每 1000 行一块
/// let mut index = BrinIndex::new(1000).unwrap();
///
/// // 插入有序数据（时序）
/// for i in 0..10000 {
///     index.insert(Value::Int64(i)).unwrap();
/// }
/// index.finish_block(); // 收尾最后一块
///
/// // 范围查询 [3000, 5000]
/// let range = BrinRange::between(Value::Int64(3000), Value::Int64(5000));
/// let matching_blocks = index.range_query(&range).unwrap();
/// // matching_blocks 包含可能含匹配行的块索引
/// ```
#[derive(Debug)]
pub struct BrinIndex {
    /// 每块行数
    block_size: usize,
    /// 已完成的块
    blocks: Vec<BlockRange>,
    // 构建中的当前块缓冲
    current_min: Option<Value>,
    current_max: Option<Value>,
    current_count: usize,
    current_null_count: usize,
}

impl BrinIndex {
    /// 创建空 BRIN 索引
    ///
    /// - `block_size` — 每块行数（必须 > 0）
    pub fn new(block_size: usize) -> Result<Self, BrinError> {
        if block_size == 0 {
            return Err(BrinError::InvalidBlockSize(block_size));
        }
        Ok(Self {
            block_size,
            blocks: Vec::new(),
            current_min: None,
            current_max: None,
            current_count: 0,
            current_null_count: 0,
        })
    }

    /// 从迭代器批量构建索引
    ///
    /// 自动按 `block_size` 分块并完成所有块。
    pub fn build_from_iter<I>(block_size: usize, iter: I) -> Result<Self, BrinError>
    where
        I: IntoIterator<Item = Value>,
    {
        let mut index = Self::new(block_size)?;
        for value in iter {
            index.insert(value)?;
        }
        index.finish_block();
        Ok(index)
    }

    /// 从行集 + 列索引批量构建索引
    ///
    /// 提取每行的指定列值构建 BRIN 索引。
    pub fn build_from_rows(
        block_size: usize,
        rows: &[crate::executor::Row],
        col_idx: usize,
    ) -> Result<Self, BrinError> {
        let mut index = Self::new(block_size)?;
        for row in rows {
            match row.get(col_idx) {
                Some(value) => index.insert(value.clone())?,
                None => index.insert_null(),
            }
        }
        index.finish_block();
        Ok(index)
    }

    /// 插入一个值（假设有序输入）
    ///
    /// 当当前块满时自动关闭并开启新块。
    pub fn insert(&mut self, value: Value) -> Result<(), BrinError> {
        if matches!(value, Value::Null) {
            self.insert_null();
            return Ok(());
        }

        // 校验类型可比较
        Self::check_comparable(&value)?;

        // 更新当前块的 min/max
        match &self.current_min {
            None => {
                self.current_min = Some(value.clone());
                self.current_max = Some(value);
            }
            Some(cur_min) => {
                if value_compare(&value, cur_min) == std::cmp::Ordering::Less {
                    self.current_min = Some(value.clone());
                }
                let cur_max = self.current_max.as_ref().unwrap();
                if value_compare(&value, cur_max) == std::cmp::Ordering::Greater {
                    self.current_max = Some(value);
                }
            }
        }

        self.current_count += 1;
        self.check_block_full();
        Ok(())
    }

    /// 插入 NULL 值
    pub fn insert_null(&mut self) {
        self.current_count += 1;
        self.current_null_count += 1;
        self.check_block_full();
    }

    /// 检查当前块是否已满，若满则关闭
    fn check_block_full(&mut self) {
        if self.current_count >= self.block_size {
            self.finish_block();
        }
    }

    /// 关闭当前块（即使未满），将其加入 blocks
    ///
    /// 若当前块无任何行（count=0），则不添加。
    pub fn finish_block(&mut self) {
        if self.current_count == 0 {
            return;
        }
        let block_idx = self.blocks.len();
        self.blocks.push(BlockRange {
            block_idx,
            min: self.current_min.take(),
            max: self.current_max.take(),
            count: self.current_count,
            null_count: self.current_null_count,
        });
        self.current_count = 0;
        self.current_null_count = 0;
    }

    /// 范围查询 — 返回可能含匹配行的块索引列表
    ///
    /// 调用方需对返回的块回表扫描验证（BRIN 是有损索引）。
    pub fn range_query(&self, range: &BrinRange) -> Result<Vec<usize>, BrinError> {
        if self.blocks.is_empty() {
            return Ok(Vec::new());
        }

        // 校验范围边界类型可比较
        if let Some(lower) = &range.lower {
            Self::check_comparable(lower)?;
        }
        if let Some(upper) = &range.upper {
            Self::check_comparable(upper)?;
        }

        let mut result = Vec::new();
        for block in &self.blocks {
            if block.overlaps(range) {
                result.push(block.block_idx);
            }
        }
        Ok(result)
    }

    /// 获取所有块（只读）
    pub fn blocks(&self) -> &[BlockRange] {
        &self.blocks
    }

    /// 获取指定块（只读）
    pub fn get_block(&self, block_idx: usize) -> Result<&BlockRange, BrinError> {
        self.blocks
            .get(block_idx)
            .ok_or(BrinError::BlockIndexOutOfRange(
                block_idx,
                self.blocks.len(),
            ))
    }

    /// 块数量
    pub fn num_blocks(&self) -> usize {
        self.blocks.len()
    }

    /// 每块行数
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// 总行数（含 NULL，已完成块 + 当前块）
    pub fn total_rows(&self) -> usize {
        self.blocks.iter().map(|b| b.count).sum::<usize>() + self.current_count
    }

    /// 总 NULL 行数
    pub fn total_nulls(&self) -> usize {
        self.blocks.iter().map(|b| b.null_count).sum::<usize>() + self.current_null_count
    }

    /// 估算索引字节数
    ///
    /// 每块：block_idx(8) + min(~16) + max(~16) + count(8) + null_count(8) ≈ 56 字节
    /// 加上 Vec 元数据开销，保守估 ~100 字节/块。
    pub fn size_bytes(&self) -> usize {
        // 每块估算 100 字节（含 Value 序列化开销）
        self.blocks.len() * 100
    }

    /// 估算等价 B-Tree 索引的字节数（用于压缩比计算）
    ///
    /// B-Tree 每行约 16 字节（键 + TID 指针）。
    pub fn estimated_btree_bytes(&self) -> usize {
        self.total_rows() * 16
    }

    /// 获取统计信息
    pub fn stats(&self) -> BrinStats {
        let size_bytes = self.size_bytes();
        let btree_bytes = self.estimated_btree_bytes();
        let compression_ratio = if size_bytes == 0 {
            0.0
        } else {
            btree_bytes as f64 / size_bytes as f64
        };
        BrinStats {
            num_blocks: self.num_blocks(),
            block_size: self.block_size,
            total_rows: self.total_rows(),
            total_nulls: self.total_nulls(),
            size_bytes,
            compression_ratio,
        }
    }

    /// 校验值类型可比较
    fn check_comparable(value: &Value) -> Result<(), BrinError> {
        match value {
            Value::Int64(_)
            | Value::Float64(_)
            | Value::Text(_)
            | Value::Bool(_)
            | Value::Date(_)
            | Value::Timestamp(_)
            | Value::Decimal(_, _) => Ok(()),
            _ => Err(BrinError::UnsupportedType(format!("{value:?}"))),
        }
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Row;

    // -----------------------------------------------------------------
    //  测试辅助
    // -----------------------------------------------------------------

    fn make_int_rows(start: i64, count: usize) -> Vec<Row> {
        (start..start + count as i64)
            .map(|i| vec![Value::Int64(i)])
            .collect()
    }

    // =================================================================
    //  BrinError 测试
    // =================================================================

    #[test]
    fn test_error_invalid_block_size_zero() {
        let err = BrinIndex::new(0).unwrap_err();
        assert!(matches!(err, BrinError::InvalidBlockSize(0)));
    }

    #[test]
    fn test_error_to_execution_error() {
        let err: ExecutionError = BrinError::EmptyIndex.into();
        assert!(matches!(err, ExecutionError::EvalError(_)));
    }

    #[test]
    fn test_error_block_index_out_of_range() {
        let index = BrinIndex::new(10).unwrap();
        let err = index.get_block(0).unwrap_err();
        assert!(matches!(err, BrinError::BlockIndexOutOfRange(0, 0)));
    }

    #[test]
    fn test_error_unsupported_type() {
        let mut index = BrinIndex::new(10).unwrap();
        let err = index
            .insert(Value::Array(vec![Value::Int64(1)]))
            .unwrap_err();
        assert!(matches!(err, BrinError::UnsupportedType(_)));
    }

    #[test]
    fn test_error_type_mismatch_in_query() {
        let mut index = BrinIndex::new(10).unwrap();
        index.insert(Value::Int64(1)).unwrap();
        index.finish_block();
        // 用 Text 范围查询 Int64 索引 — value_compare 会按 Debug 字符串比较，不报错但结果无意义
        // 这里测试 UnsupportedType：用 Array 类型作为边界
        let range = BrinRange::lower_bound(Value::Array(vec![]));
        let err = index.range_query(&range).unwrap_err();
        assert!(matches!(err, BrinError::UnsupportedType(_)));
    }

    // =================================================================
    //  BrinIndex::new 测试
    // =================================================================

    #[test]
    fn test_new_empty() {
        let index = BrinIndex::new(100).unwrap();
        assert_eq!(index.block_size(), 100);
        assert_eq!(index.num_blocks(), 0);
        assert_eq!(index.total_rows(), 0);
        assert_eq!(index.total_nulls(), 0);
    }

    #[test]
    fn test_new_block_size_one() {
        let index = BrinIndex::new(1).unwrap();
        assert_eq!(index.block_size(), 1);
    }

    // =================================================================
    //  插入与分块测试
    // =================================================================

    #[test]
    fn test_insert_single_value() {
        let mut index = BrinIndex::new(100).unwrap();
        index.insert(Value::Int64(42)).unwrap();
        index.finish_block();

        assert_eq!(index.num_blocks(), 1);
        assert_eq!(index.total_rows(), 1);

        let block = index.get_block(0).unwrap();
        assert_eq!(block.min, Some(Value::Int64(42)));
        assert_eq!(block.max, Some(Value::Int64(42)));
        assert_eq!(block.count, 1);
        assert_eq!(block.null_count, 0);
        assert!(block.has_values());
    }

    #[test]
    fn test_insert_multiple_values_same_block() {
        let mut index = BrinIndex::new(100).unwrap();
        index.insert(Value::Int64(30)).unwrap();
        index.insert(Value::Int64(10)).unwrap();
        index.insert(Value::Int64(20)).unwrap();
        index.finish_block();

        assert_eq!(index.num_blocks(), 1);
        let block = index.get_block(0).unwrap();
        assert_eq!(block.min, Some(Value::Int64(10)));
        assert_eq!(block.max, Some(Value::Int64(30)));
        assert_eq!(block.count, 3);
    }

    #[test]
    fn test_insert_auto_block_boundary() {
        // block_size=3，插入 7 个值 → 2 个完整块 + 1 个未完成块
        let mut index = BrinIndex::new(3).unwrap();
        for i in 0..7 {
            index.insert(Value::Int64(i)).unwrap();
        }
        // 前 6 个值自动分 2 块，第 7 个在当前块
        assert_eq!(index.num_blocks(), 2); // 2 个已完成的块
        assert_eq!(index.total_rows(), 7);

        index.finish_block();
        assert_eq!(index.num_blocks(), 3);
    }

    #[test]
    fn test_insert_null_values() {
        let mut index = BrinIndex::new(100).unwrap();
        index.insert(Value::Int64(5)).unwrap();
        index.insert(Value::Null).unwrap(); // NULL
        index.insert(Value::Int64(3)).unwrap();
        index.insert(Value::Null).unwrap(); // NULL
        index.finish_block();

        let block = index.get_block(0).unwrap();
        assert_eq!(block.count, 4);
        assert_eq!(block.null_count, 2);
        assert_eq!(block.min, Some(Value::Int64(3)));
        assert_eq!(block.max, Some(Value::Int64(5)));
        assert_eq!(block.non_null_count(), 2);
        assert!(block.has_values());
    }

    #[test]
    fn test_insert_all_nulls_in_block() {
        let mut index = BrinIndex::new(3).unwrap();
        index.insert_null();
        index.insert_null();
        index.insert_null();
        // block_size=3 已满，自动关闭
        assert_eq!(index.num_blocks(), 1);

        let block = index.get_block(0).unwrap();
        assert_eq!(block.count, 3);
        assert_eq!(block.null_count, 3);
        assert_eq!(block.min, None);
        assert_eq!(block.max, None);
        assert!(!block.has_values());
    }

    #[test]
    fn test_finish_block_empty_no_op() {
        let mut index = BrinIndex::new(10).unwrap();
        index.finish_block(); // 无任何插入
        assert_eq!(index.num_blocks(), 0);
    }

    #[test]
    fn test_finish_block_partial() {
        let mut index = BrinIndex::new(100).unwrap();
        index.insert(Value::Int64(1)).unwrap();
        index.insert(Value::Int64(2)).unwrap();
        index.finish_block(); // 提前关闭（仅 2 行）

        assert_eq!(index.num_blocks(), 1);
        let block = index.get_block(0).unwrap();
        assert_eq!(block.count, 2);
    }

    // =================================================================
    //  build_from_iter 测试
    // =================================================================

    #[test]
    fn test_build_from_iter_basic() {
        let values: Vec<Value> = (0..10).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(3, values).unwrap();

        // 10 个值，block_size=3 → 3,3,3,1 → 4 块
        assert_eq!(index.num_blocks(), 4);
        assert_eq!(index.total_rows(), 10);

        // 第一块 [0,1,2]
        let b0 = index.get_block(0).unwrap();
        assert_eq!(b0.min, Some(Value::Int64(0)));
        assert_eq!(b0.max, Some(Value::Int64(2)));
    }

    #[test]
    fn test_build_from_iter_empty() {
        let index = BrinIndex::build_from_iter(10, std::iter::empty::<Value>()).unwrap();
        assert_eq!(index.num_blocks(), 0);
        assert_eq!(index.total_rows(), 0);
    }

    #[test]
    fn test_build_from_iter_with_nulls() {
        let values = vec![
            Value::Int64(1),
            Value::Null,
            Value::Int64(5),
            Value::Null,
            Value::Int64(3),
        ];
        let index = BrinIndex::build_from_iter(100, values).unwrap();

        assert_eq!(index.num_blocks(), 1);
        let block = index.get_block(0).unwrap();
        assert_eq!(block.count, 5);
        assert_eq!(block.null_count, 2);
        assert_eq!(block.min, Some(Value::Int64(1)));
        assert_eq!(block.max, Some(Value::Int64(5)));
    }

    // =================================================================
    //  build_from_rows 测试
    // =================================================================

    #[test]
    fn test_build_from_rows_basic() {
        let rows = make_int_rows(0, 10);
        let index = BrinIndex::build_from_rows(3, &rows, 0).unwrap();

        assert_eq!(index.num_blocks(), 4);
        assert_eq!(index.total_rows(), 10);
    }

    #[test]
    fn test_build_from_rows_missing_column() {
        // 行缺少列 → 视为 NULL
        let rows: Vec<Row> = vec![
            vec![Value::Int64(1)],
            vec![], // 空行，列索引 0 越界 → NULL
        ];
        let index = BrinIndex::build_from_rows(100, &rows, 0).unwrap();

        let block = index.get_block(0).unwrap();
        assert_eq!(block.count, 2);
        assert_eq!(block.null_count, 1);
    }

    // =================================================================
    //  BlockRange::overlaps 测试
    // =================================================================

    #[test]
    fn test_overlaps_unbounded_matches_all() {
        let block = BlockRange {
            block_idx: 0,
            min: Some(Value::Int64(10)),
            max: Some(Value::Int64(20)),
            count: 5,
            null_count: 0,
        };
        assert!(block.overlaps(&BrinRange::all()));
    }

    #[test]
    fn test_overlaps_lower_bound() {
        let block = BlockRange {
            block_idx: 0,
            min: Some(Value::Int64(10)),
            max: Some(Value::Int64(20)),
            count: 5,
            null_count: 0,
        };
        // lower=15 → block.max(20) >= 15 → true
        assert!(block.overlaps(&BrinRange::lower_bound(Value::Int64(15))));
        // lower=25 → block.max(20) < 25 → false
        assert!(!block.overlaps(&BrinRange::lower_bound(Value::Int64(25))));
        // lower=20 → block.max(20) >= 20 → true（闭区间）
        assert!(block.overlaps(&BrinRange::lower_bound(Value::Int64(20))));
    }

    #[test]
    fn test_overlaps_upper_bound() {
        let block = BlockRange {
            block_idx: 0,
            min: Some(Value::Int64(10)),
            max: Some(Value::Int64(20)),
            count: 5,
            null_count: 0,
        };
        // upper=15 → block.min(10) <= 15 → true
        assert!(block.overlaps(&BrinRange::upper_bound(Value::Int64(15))));
        // upper=5 → block.min(10) > 5 → false
        assert!(!block.overlaps(&BrinRange::upper_bound(Value::Int64(5))));
        // upper=10 → block.min(10) <= 10 → true（闭区间）
        assert!(block.overlaps(&BrinRange::upper_bound(Value::Int64(10))));
    }

    #[test]
    fn test_overlaps_between() {
        let block = BlockRange {
            block_idx: 0,
            min: Some(Value::Int64(10)),
            max: Some(Value::Int64(20)),
            count: 5,
            null_count: 0,
        };
        // [15, 18] → 与 [10, 20] 相交 → true
        assert!(block.overlaps(&BrinRange::between(Value::Int64(15), Value::Int64(18))));
        // [5, 12] → 与 [10, 20] 相交 → true
        assert!(block.overlaps(&BrinRange::between(Value::Int64(5), Value::Int64(12))));
        // [25, 30] → block.max(20) < 25 → false
        assert!(!block.overlaps(&BrinRange::between(Value::Int64(25), Value::Int64(30))));
        // [0, 5] → block.min(10) > 5 → false
        assert!(!block.overlaps(&BrinRange::between(Value::Int64(0), Value::Int64(5))));
        // [10, 20] → 完全匹配 → true
        assert!(block.overlaps(&BrinRange::between(Value::Int64(10), Value::Int64(20))));
    }

    #[test]
    fn test_overlaps_all_null_block() {
        let block = BlockRange {
            block_idx: 0,
            min: None,
            max: None,
            count: 3,
            null_count: 3,
        };
        // 全 NULL 块 + 无界 → true（NULL 可能匹配）
        assert!(block.overlaps(&BrinRange::all()));
        // 全 NULL 块 + 有界 → false（NULL 不在范围内）
        assert!(!block.overlaps(&BrinRange::between(Value::Int64(1), Value::Int64(10))));
        assert!(!block.overlaps(&BrinRange::lower_bound(Value::Int64(1))));
        assert!(!block.overlaps(&BrinRange::upper_bound(Value::Int64(10))));
    }

    // =================================================================
    //  BrinRange 测试
    // =================================================================

    #[test]
    fn test_brin_range_all() {
        let r = BrinRange::all();
        assert!(r.is_unbounded());
        assert!(r.lower.is_none());
        assert!(r.upper.is_none());
    }

    #[test]
    fn test_brin_range_lower_bound() {
        let r = BrinRange::lower_bound(Value::Int64(10));
        assert!(!r.is_unbounded());
        assert_eq!(r.lower, Some(Value::Int64(10)));
        assert!(r.upper.is_none());
    }

    #[test]
    fn test_brin_range_upper_bound() {
        let r = BrinRange::upper_bound(Value::Int64(100));
        assert!(!r.is_unbounded());
        assert!(r.lower.is_none());
        assert_eq!(r.upper, Some(Value::Int64(100)));
    }

    #[test]
    fn test_brin_range_between() {
        let r = BrinRange::between(Value::Int64(10), Value::Int64(100));
        assert!(!r.is_unbounded());
        assert_eq!(r.lower, Some(Value::Int64(10)));
        assert_eq!(r.upper, Some(Value::Int64(100)));
    }

    #[test]
    fn test_brin_range_default() {
        let r = BrinRange::default();
        assert!(r.is_unbounded());
    }

    // =================================================================
    //  range_query 测试
    // =================================================================

    #[test]
    fn test_range_query_empty_index() {
        let index = BrinIndex::new(100).unwrap();
        let result = index.range_query(&BrinRange::all()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_range_query_unbounded_returns_all_blocks() {
        let values: Vec<Value> = (0..10).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(3, values).unwrap();
        // 10 values, block_size=3 → 4 blocks
        let result = index.range_query(&BrinRange::all()).unwrap();
        assert_eq!(result, vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_range_query_between_partial_match() {
        // 块: [0,1,2], [3,4,5], [6,7,8], [9]
        let values: Vec<Value> = (0..10).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(3, values).unwrap();

        // 查询 [4, 7] → 块1[3-5]✓, 块2[6-8]✓, 块0[0-2]✗, 块3[9-9]✗
        let result = index
            .range_query(&BrinRange::between(Value::Int64(4), Value::Int64(7)))
            .unwrap();
        assert_eq!(result, vec![1, 2]);
    }

    #[test]
    fn test_range_query_no_match() {
        let values: Vec<Value> = (0..10).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(3, values).unwrap();

        // 查询 [100, 200] → 无块匹配
        let result = index
            .range_query(&BrinRange::between(Value::Int64(100), Value::Int64(200)))
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_range_query_lower_bound() {
        let values: Vec<Value> = (0..10).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(3, values).unwrap();

        // 查询 >= 7 → 块2[6-8]✓, 块3[9-9]✓
        let result = index
            .range_query(&BrinRange::lower_bound(Value::Int64(7)))
            .unwrap();
        assert_eq!(result, vec![2, 3]);
    }

    #[test]
    fn test_range_query_upper_bound() {
        let values: Vec<Value> = (0..10).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(3, values).unwrap();

        // 查询 <= 2 → 块0[0-2]✓
        let result = index
            .range_query(&BrinRange::upper_bound(Value::Int64(2)))
            .unwrap();
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn test_range_query_exact_boundary() {
        let values: Vec<Value> = (0..10).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(3, values).unwrap();

        // 查询 [2, 3] → 块0[0-2]✓(2), 块1[3-5]✓(3)
        let result = index
            .range_query(&BrinRange::between(Value::Int64(2), Value::Int64(3)))
            .unwrap();
        assert_eq!(result, vec![0, 1]);
    }

    #[test]
    fn test_range_query_skips_all_null_block() {
        let mut index = BrinIndex::new(3).unwrap();
        // 块0: [1, 2, 3]
        index.insert(Value::Int64(1)).unwrap();
        index.insert(Value::Int64(2)).unwrap();
        index.insert(Value::Int64(3)).unwrap();
        // 块1: [NULL, NULL, NULL]
        index.insert_null();
        index.insert_null();
        index.insert_null();
        // 块2: [7, 8, 9]
        index.insert(Value::Int64(7)).unwrap();
        index.insert(Value::Int64(8)).unwrap();
        index.insert(Value::Int64(9)).unwrap();
        index.finish_block();

        // 查询 [5, 10] → 块1(全NULL)✗, 块2[7-9]✓
        let result = index
            .range_query(&BrinRange::between(Value::Int64(5), Value::Int64(10)))
            .unwrap();
        assert_eq!(result, vec![2]);
    }

    // =================================================================
    //  多类型测试
    // =================================================================

    #[test]
    fn test_float64_type() {
        let values = vec![
            Value::Float64(1.5),
            Value::Float64(2.5),
            Value::Float64(0.5),
        ];
        let index = BrinIndex::build_from_iter(100, values).unwrap();

        let block = index.get_block(0).unwrap();
        assert_eq!(block.min, Some(Value::Float64(0.5)));
        assert_eq!(block.max, Some(Value::Float64(2.5)));

        let result = index
            .range_query(&BrinRange::between(
                Value::Float64(1.0),
                Value::Float64(2.0),
            ))
            .unwrap();
        assert_eq!(result, vec![0]); // 块 [0.5, 2.5] 与 [1.0, 2.0] 相交
    }

    #[test]
    fn test_text_type() {
        let values = vec![
            Value::Text("apple".to_string()),
            Value::Text("banana".to_string()),
            Value::Text("cherry".to_string()),
        ];
        let index = BrinIndex::build_from_iter(100, values).unwrap();

        let block = index.get_block(0).unwrap();
        assert_eq!(block.min, Some(Value::Text("apple".to_string())));
        assert_eq!(block.max, Some(Value::Text("cherry".to_string())));

        // 查询 "b" 到 "c"
        let result = index
            .range_query(&BrinRange::between(
                Value::Text("b".to_string()),
                Value::Text("c".to_string()),
            ))
            .unwrap();
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn test_date_type() {
        let values = vec![
            Value::Date(19723), // 2024-01-01
            Value::Date(19724), // 2024-01-02
            Value::Date(19725), // 2024-01-03
        ];
        let index = BrinIndex::build_from_iter(100, values).unwrap();

        let result = index
            .range_query(&BrinRange::between(Value::Date(19724), Value::Date(19725)))
            .unwrap();
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn test_timestamp_type() {
        let values = vec![
            Value::Timestamp(1000000),
            Value::Timestamp(2000000),
            Value::Timestamp(3000000),
        ];
        let index = BrinIndex::build_from_iter(100, values).unwrap();

        let result = index
            .range_query(&BrinRange::lower_bound(Value::Timestamp(2500000)))
            .unwrap();
        assert_eq!(result, vec![0]); // 块 [1M, 3M] 与 >= 2.5M 相交
    }

    #[test]
    fn test_decimal_type() {
        let values = vec![
            Value::Decimal(100, 2), // 1.00
            Value::Decimal(250, 2), // 2.50
            Value::Decimal(500, 2), // 5.00
        ];
        let index = BrinIndex::build_from_iter(100, values).unwrap();

        let block = index.get_block(0).unwrap();
        assert_eq!(block.min, Some(Value::Decimal(100, 2)));
        assert_eq!(block.max, Some(Value::Decimal(500, 2)));

        let result = index
            .range_query(&BrinRange::between(
                Value::Decimal(200, 2),
                Value::Decimal(300, 2),
            ))
            .unwrap();
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn test_mixed_int_float_comparison() {
        // Int64 与 Float64 跨类型比较
        let mut index = BrinIndex::new(100).unwrap();
        index.insert(Value::Int64(1)).unwrap();
        index.insert(Value::Float64(2.5)).unwrap();
        index.insert(Value::Int64(3)).unwrap();
        index.finish_block();

        let block = index.get_block(0).unwrap();
        assert_eq!(block.min, Some(Value::Int64(1)));
        assert_eq!(block.max, Some(Value::Int64(3)));

        // 用 Float64 查询
        let result = index
            .range_query(&BrinRange::between(
                Value::Float64(2.0),
                Value::Float64(2.6),
            ))
            .unwrap();
        assert_eq!(result, vec![0]);
    }

    // =================================================================
    //  统计与大小测试
    // =================================================================

    #[test]
    fn test_size_bytes_empty() {
        let index = BrinIndex::new(100).unwrap();
        assert_eq!(index.size_bytes(), 0);
    }

    #[test]
    fn test_size_bytes_non_empty() {
        let values: Vec<Value> = (0..1000).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(100, values).unwrap();
        // 1000 values / 100 block_size = 10 blocks → 10 * 100 = 1000 bytes
        assert_eq!(index.size_bytes(), 1000);
    }

    #[test]
    fn test_estimated_btree_bytes() {
        let values: Vec<Value> = (0..1000).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(100, values).unwrap();
        // 1000 rows * 16 bytes = 16000 bytes
        assert_eq!(index.estimated_btree_bytes(), 16000);
    }

    #[test]
    fn test_stats_basic() {
        let values: Vec<Value> = (0..1000).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(100, values).unwrap();

        let stats = index.stats();
        assert_eq!(stats.num_blocks, 10);
        assert_eq!(stats.block_size, 100);
        assert_eq!(stats.total_rows, 1000);
        assert_eq!(stats.total_nulls, 0);
        assert_eq!(stats.size_bytes, 1000);
        // compression_ratio = 16000 / 1000 = 16.0
        assert!((stats.compression_ratio - 16.0).abs() < 0.001);
    }

    #[test]
    fn test_stats_with_nulls() {
        let mut index = BrinIndex::new(100).unwrap();
        for i in 0..50 {
            index.insert(Value::Int64(i)).unwrap();
        }
        for _ in 0..50 {
            index.insert_null();
        }
        index.finish_block();

        let stats = index.stats();
        assert_eq!(stats.total_rows, 100);
        assert_eq!(stats.total_nulls, 50);
    }

    #[test]
    fn test_blocks_slice_access() {
        let values: Vec<Value> = (0..10).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(3, values).unwrap();

        let blocks = index.blocks();
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0].block_idx, 0);
        assert_eq!(blocks[3].block_idx, 3);
    }

    // =================================================================
    //  大规模数据测试（压缩比验证）
    // =================================================================

    #[test]
    fn test_large_ordered_data_compression() {
        // 模拟 100000 行时序数据，block_size=1000 → 100 块
        let values: Vec<Value> = (0..100000).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(1000, values).unwrap();

        let stats = index.stats();
        assert_eq!(stats.num_blocks, 100);
        assert_eq!(stats.total_rows, 100000);
        // BRIN: 100 * 100 = 10000 bytes
        // B-Tree: 100000 * 16 = 1600000 bytes
        // 压缩比 = 160x
        assert_eq!(stats.size_bytes, 10000);
        assert!((stats.compression_ratio - 160.0).abs() < 0.001);
    }

    #[test]
    fn test_large_data_range_query_efficiency() {
        // 100000 行，block_size=1000 → 100 块，每块 1000 行
        let values: Vec<Value> = (0..100000).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(1000, values).unwrap();

        // 查询 [50000, 51000] → 应只命中 2 块（块50[50000-50999], 块51[51000-51999]）
        let result = index
            .range_query(&BrinRange::between(
                Value::Int64(50000),
                Value::Int64(51000),
            ))
            .unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&50));
        assert!(result.contains(&51));
    }

    #[test]
    fn test_large_data_range_query_single_block() {
        let values: Vec<Value> = (0..100000).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(1000, values).unwrap();

        // 查询 [500, 600] → 只命中块0[0-999]
        let result = index
            .range_query(&BrinRange::between(Value::Int64(500), Value::Int64(600)))
            .unwrap();
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn test_large_data_range_query_no_match() {
        let values: Vec<Value> = (0..100000).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(1000, values).unwrap();

        let result = index
            .range_query(&BrinRange::between(
                Value::Int64(200000),
                Value::Int64(300000),
            ))
            .unwrap();
        assert!(result.is_empty());
    }

    // =================================================================
    //  E2E 场景测试
    // =================================================================

    #[test]
    fn test_e2e_time_series_simulation() {
        // 模拟时序数据：每天 1000 条，共 10 天 = 10000 条
        // block_size=1000 → 每块一天
        let values: Vec<Value> = (0..10000).map(Value::Timestamp).collect();
        let index = BrinIndex::build_from_iter(1000, values).unwrap();

        assert_eq!(index.num_blocks(), 10);

        // 查询第 3 天 [2000, 2999]
        let result = index
            .range_query(&BrinRange::between(
                Value::Timestamp(2000),
                Value::Timestamp(2999),
            ))
            .unwrap();
        assert_eq!(result, vec![2]); // 块2 = [2000, 2999]
    }

    #[test]
    fn test_e2e_unordered_data_poor_filtering() {
        // 无序数据：块范围 min/max 重叠严重，过滤效果差
        let values = vec![
            Value::Int64(1),
            Value::Int64(100),
            Value::Int64(50),
            Value::Int64(2),
            Value::Int64(99),
            Value::Int64(51),
        ];
        // block_size=2 → 3 块
        let index = BrinIndex::build_from_iter(2, values).unwrap();

        // 每块的 min/max 范围都很宽
        let b0 = index.get_block(0).unwrap();
        assert_eq!(b0.min, Some(Value::Int64(1)));
        assert_eq!(b0.max, Some(Value::Int64(100)));

        // 查询 [50, 55] → 所有块都"可能"匹配（因为范围重叠）
        let result = index
            .range_query(&BrinRange::between(Value::Int64(50), Value::Int64(55)))
            .unwrap();
        // 无序数据 → BRIN 过滤效果差，多数块都被保留
        assert!(result.len() >= 2); // 至少 2 块（可能 3 块）
    }

    #[test]
    fn test_e2e_incremental_build() {
        // 模拟增量插入：先建一部分，再追加
        let mut index = BrinIndex::new(5).unwrap();
        for i in 0..7 {
            index.insert(Value::Int64(i)).unwrap();
        }
        // 7 values, block_size=5 → 1 完整块 + 2 在当前块
        assert_eq!(index.num_blocks(), 1);
        assert_eq!(index.total_rows(), 7);

        // 追加更多
        for i in 7..15 {
            index.insert(Value::Int64(i)).unwrap();
        }
        index.finish_block();
        // 15 values, block_size=5 → 3 块
        assert_eq!(index.num_blocks(), 3);
        assert_eq!(index.total_rows(), 15);
    }

    #[test]
    fn test_e2e_multiple_data_types_separate_indexes() {
        // 不同类型应分别建索引（BRIN 单列）
        let int_values: Vec<Value> = (0..100).map(Value::Int64).collect();
        let text_values: Vec<Value> = (0..100).map(|i| Value::Text(format!("row_{i}"))).collect();

        let int_index = BrinIndex::build_from_iter(10, int_values).unwrap();
        let text_index = BrinIndex::build_from_iter(10, text_values).unwrap();

        // Int 索引查询
        let int_result = int_index
            .range_query(&BrinRange::between(Value::Int64(50), Value::Int64(60)))
            .unwrap();
        assert_eq!(int_result, vec![5, 6]); // 块5[50-59], 块6[60-69]

        // Text 索引查询
        let text_result = text_index
            .range_query(&BrinRange::between(
                Value::Text("row_50".to_string()),
                Value::Text("row_60".to_string()),
            ))
            .unwrap();
        assert!(!text_result.is_empty());
    }

    #[test]
    fn test_e2e_block_count_and_distribution() {
        // 验证块分布正确
        let values: Vec<Value> = (0..25).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(10, values).unwrap();

        // 25 values / 10 per block → 3 块 (10, 10, 5)
        assert_eq!(index.num_blocks(), 3);
        assert_eq!(index.total_rows(), 25);

        let b0 = index.get_block(0).unwrap();
        let b1 = index.get_block(1).unwrap();
        let b2 = index.get_block(2).unwrap();

        assert_eq!(b0.count, 10);
        assert_eq!(b1.count, 10);
        assert_eq!(b2.count, 5); // 最后一块未满

        assert_eq!(b0.min, Some(Value::Int64(0)));
        assert_eq!(b0.max, Some(Value::Int64(9)));
        assert_eq!(b1.min, Some(Value::Int64(10)));
        assert_eq!(b1.max, Some(Value::Int64(19)));
        assert_eq!(b2.min, Some(Value::Int64(20)));
        assert_eq!(b2.max, Some(Value::Int64(24)));
    }

    #[test]
    fn test_e2e_full_scan_vs_brin_filtered() {
        // 模拟 BRIN 过滤效果对比
        let values: Vec<Value> = (0..10000).map(Value::Int64).collect();
        let index = BrinIndex::build_from_iter(100, values).unwrap();

        // 全表扫描：返回所有 100 块
        let full_scan = index.range_query(&BrinRange::all()).unwrap();
        assert_eq!(full_scan.len(), 100);

        // 范围查询 [4500, 4600] → 只命中 2 块
        let filtered = index
            .range_query(&BrinRange::between(Value::Int64(4500), Value::Int64(4600)))
            .unwrap();
        assert_eq!(filtered.len(), 2); // 块45[4400-4499]? 不，块45 = [4500-4599], 块46 = [4600-4699]
                                       // 过滤率 = (100-2)/100 = 98%
        assert!(filtered.len() < full_scan.len());
    }

    #[test]
    fn test_e2e_with_nulls_mixed() {
        // 混合 NULL 和非 NULL 数据
        let mut index = BrinIndex::new(5).unwrap();
        // 块0: [1, 2, NULL, 4, 5]
        index.insert(Value::Int64(1)).unwrap();
        index.insert(Value::Int64(2)).unwrap();
        index.insert_null();
        index.insert(Value::Int64(4)).unwrap();
        index.insert(Value::Int64(5)).unwrap();
        // 块1: [NULL, NULL, 10, 11, NULL]
        index.insert_null();
        index.insert_null();
        index.insert(Value::Int64(10)).unwrap();
        index.insert(Value::Int64(11)).unwrap();
        index.insert_null();
        index.finish_block();

        assert_eq!(index.num_blocks(), 2);
        assert_eq!(index.total_nulls(), 4);

        let b0 = index.get_block(0).unwrap();
        assert_eq!(b0.min, Some(Value::Int64(1)));
        assert_eq!(b0.max, Some(Value::Int64(5)));
        assert_eq!(b0.null_count, 1);

        let b1 = index.get_block(1).unwrap();
        assert_eq!(b1.min, Some(Value::Int64(10)));
        assert_eq!(b1.max, Some(Value::Int64(11)));
        assert_eq!(b1.null_count, 3);

        // 查询 [3, 7] → 块0[1-5]✓, 块1[10-11]✗
        let result = index
            .range_query(&BrinRange::between(Value::Int64(3), Value::Int64(7)))
            .unwrap();
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn test_insert_null_via_insert_method() {
        // insert(Value::Null) 应等价于 insert_null()
        let mut index = BrinIndex::new(100).unwrap();
        index.insert(Value::Null).unwrap();
        index.finish_block();

        let block = index.get_block(0).unwrap();
        assert_eq!(block.count, 1);
        assert_eq!(block.null_count, 1);
        assert!(!block.has_values());
    }

    #[test]
    fn test_value_min_max_helpers() {
        assert_eq!(
            value_min(&Value::Int64(3), &Value::Int64(7)),
            Value::Int64(3)
        );
        assert_eq!(
            value_max(&Value::Int64(3), &Value::Int64(7)),
            Value::Int64(7)
        );
        assert_eq!(
            value_min(&Value::Text("a".to_string()), &Value::Text("b".to_string())),
            Value::Text("a".to_string())
        );
    }

    #[test]
    fn test_check_comparable_valid_types() {
        assert!(BrinIndex::check_comparable(&Value::Int64(1)).is_ok());
        assert!(BrinIndex::check_comparable(&Value::Float64(1.0)).is_ok());
        assert!(BrinIndex::check_comparable(&Value::Text("a".to_string())).is_ok());
        assert!(BrinIndex::check_comparable(&Value::Bool(true)).is_ok());
        assert!(BrinIndex::check_comparable(&Value::Date(1)).is_ok());
        assert!(BrinIndex::check_comparable(&Value::Timestamp(1)).is_ok());
        assert!(BrinIndex::check_comparable(&Value::Decimal(1, 0)).is_ok());
    }

    #[test]
    fn test_check_comparable_invalid_types() {
        assert!(BrinIndex::check_comparable(&Value::Array(vec![])).is_err());
        assert!(BrinIndex::check_comparable(&Value::Json(serde_json::Value::Null)).is_err());
        assert!(BrinIndex::check_comparable(&Value::Null).is_err());
        assert!(BrinIndex::check_comparable(&Value::Blob(vec![])).is_err());
    }

    #[test]
    fn test_block_range_non_null_count() {
        let block = BlockRange {
            block_idx: 0,
            min: Some(Value::Int64(1)),
            max: Some(Value::Int64(10)),
            count: 10,
            null_count: 3,
        };
        assert_eq!(block.non_null_count(), 7);
    }
}
