//! Bitmap 索引 — Phase 6.28
//!
//! 提供 Oracle 风格的 Bitmap 索引功能：
//!
//! - **低基数列优化**：对低基数列（如 status/gender）每个 distinct 值维护一个位图
//! - **等值查询加速**：O(1) 查找位图，O(N/64) 遍历位图返回匹配行
//! - **位图运算**：支持 AND/OR/NOT 组合多谓词（bitmap AND/OR 是 bitmap 索引的核心优势）
//! - **紧凑存储**：N 行 K 个 distinct 值 → N*K/8 字节（低基数时远小于 B-Tree）
//!
//! # 设计
//!
//! - **Bitset**：位图实现（`Vec<u64>`，每 u64 存 64 行），支持 set/get/ones/and/or/not/count
//! - **BitmapIndex**：索引主体，`HashMap<String, (Value, Bitset)>`（String 键由 Value Debug 表示）
//! - **BitmapIndexError**：错误类型
//!
//! # 与 PG/Oracle 的关系
//!
//! - **Oracle**：支持真正的 Bitmap 索引（`CREATE BITMAP INDEX`），适合低基数列 + 只读/OLAP
//! - **PG**：无原生 Bitmap 索引（PG 的 "Bitmap Index Scan" 是运行时从任意索引构建位图，非存储结构）
//! - **MySQL**：8.0+ 不支持 Bitmap 索引
//! - 本实现参考 Oracle 的 Bitmap 索引语义
//!
//! # 适用场景
//!
//! - 低基数列（distinct < 1000）：gender(2)、status(10)、category(50)
//! - OLAP/只读 workload：位图更新代价高（需锁整个位图）
//! - 多谓词 AND/OR：`WHERE status='A' AND region='E'` → 两个位图 AND
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：未集成到 SQL 解析路径，仅提供程序化 API
//! - **无持久化**：纯内存索引，不落盘
//! - **更新代价高**：INSERT/UPDATE/DELETE 需修改位图（高并发写不适用）
//! - **高基数不适用**：distinct 值多时位图数量爆炸，空间和性能劣于 B-Tree
//! - **单列索引**：不支持多列复合 Bitmap（多列通过 AND/OR 运算组合）
//! - **Value 无 Hash/Eq**：使用 Debug 字符串作为 HashMap 键

use crate::executor::ExecutionError;
use std::collections::HashMap;
use szrsql_types::value::Value;

// =====================================================================
//  错误类型
// =====================================================================

/// Bitmap 索引错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BitmapIndexError {
    /// 行索引越界
    #[error("row index out of range: {0} (num_rows={1})")]
    RowIndexOutOfRange(usize, usize),
    /// 值不存在
    #[error("value not found in index: {0}")]
    ValueNotFound(String),
    /// 不支持的类型
    #[error("unsupported value type for bitmap index: {0}")]
    UnsupportedType(String),
    /// 位图长度不匹配（AND/OR 运算时）
    #[error("bitmap length mismatch: {0} != {1}")]
    LengthMismatch(usize, usize),
}

impl From<BitmapIndexError> for ExecutionError {
    fn from(e: BitmapIndexError) -> Self {
        ExecutionError::EvalError(format!("Bitmap index error: {e}"))
    }
}

// =====================================================================
//  Bitset — 位图实现
// =====================================================================

/// 位图 — 使用 `Vec<u64>` 存储，每 u64 存 64 行
///
/// 位 i = 1 表示行 i 匹配该位图对应的值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitset {
    /// 位图数据（每 u64 存 64 位）
    bits: Vec<u64>,
    /// 总位数（行数）
    len: usize,
}

impl Bitset {
    /// 创建指定位数的空位图（所有位为 0）
    pub fn new(len: usize) -> Self {
        let words = len.div_ceil(64);
        Self {
            bits: vec![0; words],
            len,
        }
    }

    /// 创建指定位数的全 1 位图
    pub fn all_ones(len: usize) -> Self {
        let mut bitset = Self::new(len);
        for i in 0..len {
            bitset.set(i);
        }
        bitset
    }

    /// 设置位 i 为 1
    pub fn set(&mut self, idx: usize) {
        debug_assert!(idx < self.len, "idx {idx} >= len {}", self.len);
        let word = idx / 64;
        let bit = idx % 64;
        self.bits[word] |= 1u64 << bit;
    }

    /// 设置位 i 为 0
    pub fn clear(&mut self, idx: usize) {
        debug_assert!(idx < self.len, "idx {idx} >= len {}", self.len);
        let word = idx / 64;
        let bit = idx % 64;
        self.bits[word] &= !(1u64 << bit);
    }

    /// 获取位 i 的值
    pub fn get(&self, idx: usize) -> bool {
        if idx >= self.len {
            return false;
        }
        let word = idx / 64;
        let bit = idx % 64;
        (self.bits[word] >> bit) & 1 == 1
    }

    /// 位数（行数）
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空（0 位）
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 统计 1 的个数
    pub fn count_ones(&self) -> usize {
        // 最后一个 word 需要屏蔽超出 len 的位
        let full_words = self.len / 64;
        let remainder = self.len % 64;
        let mut count: usize = self.bits[..full_words]
            .iter()
            .map(|w| w.count_ones() as usize)
            .sum();
        if remainder > 0 && full_words < self.bits.len() {
            let mask = (1u64 << remainder) - 1;
            count += (self.bits[full_words] & mask).count_ones() as usize;
        }
        count
    }

    /// 返回所有为 1 的位索引（行索引列表）
    pub fn ones(&self) -> Vec<usize> {
        let mut result = Vec::with_capacity(self.count_ones());
        for (word_idx, &word) in self.bits.iter().enumerate() {
            if word == 0 {
                continue;
            }
            let mut bits = word;
            while bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                let idx = word_idx * 64 + bit;
                if idx < self.len {
                    result.push(idx);
                }
                bits &= bits - 1; // 清除最低位
            }
        }
        result
    }

    /// 按位与（交集）— 两个位图长度必须相同
    pub fn and(&self, other: &Self) -> Result<Self, BitmapIndexError> {
        if self.len != other.len {
            return Err(BitmapIndexError::LengthMismatch(self.len, other.len));
        }
        let mut result = Self::new(self.len);
        for i in 0..self.bits.len() {
            result.bits[i] = self.bits[i] & other.bits[i];
        }
        Ok(result)
    }

    /// 按位或（并集）— 两个位图长度必须相同
    pub fn or(&self, other: &Self) -> Result<Self, BitmapIndexError> {
        if self.len != other.len {
            return Err(BitmapIndexError::LengthMismatch(self.len, other.len));
        }
        let mut result = Self::new(self.len);
        for i in 0..self.bits.len() {
            result.bits[i] = self.bits[i] | other.bits[i];
        }
        Ok(result)
    }

    /// 按位非（补集）— 在 len 范围内取反
    pub fn not(&self) -> Self {
        let mut result = Self::new(self.len);
        let full_words = self.len / 64;
        let remainder = self.len % 64;
        for i in 0..full_words {
            result.bits[i] = !self.bits[i];
        }
        if remainder > 0 && full_words < self.bits.len() {
            let mask = (1u64 << remainder) - 1;
            result.bits[full_words] = (!self.bits[full_words]) & mask;
        }
        result
    }

    /// 按位异或
    pub fn xor(&self, other: &Self) -> Result<Self, BitmapIndexError> {
        if self.len != other.len {
            return Err(BitmapIndexError::LengthMismatch(self.len, other.len));
        }
        let mut result = Self::new(self.len);
        for i in 0..self.bits.len() {
            result.bits[i] = self.bits[i] ^ other.bits[i];
        }
        Ok(result)
    }

    /// 估算字节数
    pub fn size_bytes(&self) -> usize {
        self.bits.len() * 8
    }
}

// =====================================================================
//  BitmapIndex — Bitmap 索引主体
// =====================================================================

/// Bitmap 索引
///
/// 对低基数列，每个 distinct 值维护一个位图。
/// 等值查询通过位图快速定位匹配行。
///
/// # 用法
///
/// ```ignore
/// use szrsql_sql::bitmap_index::*;
/// use szrsql_types::value::Value;
///
/// let mut index = BitmapIndex::new();
/// index.insert(0, Value::Text("active".to_string()));
/// index.insert(1, Value::Text("inactive".to_string()));
/// index.insert(2, Value::Text("active".to_string()));
///
/// // 等值查询
/// let rows = index.eq_query(&Value::Text("active".to_string())).unwrap();
/// assert_eq!(rows, vec![0, 2]);
/// ```
pub struct BitmapIndex {
    /// 值键（Debug 字符串）→ (原始值, 位图)
    bitmaps: HashMap<String, (Value, Bitset)>,
    /// 总行数（所有位图共享同一长度）
    num_rows: usize,
}

impl std::fmt::Debug for BitmapIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitmapIndex")
            .field("num_rows", &self.num_rows)
            .field("cardinality", &self.bitmaps.len())
            .finish()
    }
}

impl BitmapIndex {
    /// 创建空 Bitmap 索引
    pub fn new() -> Self {
        Self {
            bitmaps: HashMap::new(),
            num_rows: 0,
        }
    }

    /// 从行集 + 列索引批量构建
    ///
    /// 提取每行的指定列值构建 Bitmap 索引。
    pub fn build_from_rows(rows: &[crate::executor::Row], col_idx: usize) -> Self {
        let mut index = Self::new();
        for (row_idx, row) in rows.iter().enumerate() {
            match row.get(col_idx) {
                Some(value) => index.insert(row_idx, value.clone()),
                None => index.insert(row_idx, Value::Null),
            }
        }
        index
    }

    /// 插入值到指定行索引
    ///
    /// 若行索引 >= 当前 num_rows，则扩展所有位图到新长度。
    pub fn insert(&mut self, row_idx: usize, value: Value) {
        // 扩展位图长度
        if row_idx >= self.num_rows {
            let new_len = row_idx + 1;
            self.extend_to(new_len);
        }

        let key = value_key(&value);
        match self.bitmaps.get_mut(&key) {
            Some((_, bitset)) => {
                bitset.set(row_idx);
            }
            None => {
                let mut bitset = Bitset::new(self.num_rows);
                bitset.set(row_idx);
                self.bitmaps.insert(key, (value, bitset));
            }
        }
    }

    /// 扩展所有位图到新长度
    fn extend_to(&mut self, new_len: usize) {
        let old_len = self.num_rows;
        if new_len <= old_len {
            return;
        }
        for (_, bitset) in self.bitmaps.values_mut() {
            let old_words = bitset.bits.len();
            let new_words = new_len.div_ceil(64);
            if new_words > old_words {
                bitset.bits.resize(new_words, 0);
            }
            bitset.len = new_len;
        }
        self.num_rows = new_len;
    }

    /// 等值查询 — 返回匹配指定值的所有行索引
    pub fn eq_query(&self, value: &Value) -> Result<Vec<usize>, BitmapIndexError> {
        let key = value_key(value);
        match self.bitmaps.get(&key) {
            Some((_, bitset)) => Ok(bitset.ones()),
            None => Err(BitmapIndexError::ValueNotFound(format!("{value:?}"))),
        }
    }

    /// 等值查询 — 返回匹配指定值的位图（用于组合运算）
    pub fn eq_bitmap(&self, value: &Value) -> Result<&Bitset, BitmapIndexError> {
        let key = value_key(value);
        match self.bitmaps.get(&key) {
            Some((_, bitset)) => Ok(bitset),
            None => Err(BitmapIndexError::ValueNotFound(format!("{value:?}"))),
        }
    }

    /// 不等查询 — 返回不等于指定值的所有行索引
    ///
    /// 即该值位图的补集。
    pub fn ne_query(&self, value: &Value) -> Result<Vec<usize>, BitmapIndexError> {
        let key = value_key(value);
        match self.bitmaps.get(&key) {
            Some((_, bitset)) => Ok(bitset.not().ones()),
            None => {
                // 值不存在 → 所有行都不等于该值
                Ok((0..self.num_rows).collect())
            }
        }
    }

    /// IS NULL 查询 — 返回值为 NULL 的所有行索引
    pub fn is_null_query(&self) -> Vec<usize> {
        self.eq_bitmap(&Value::Null)
            .map(|b| b.ones())
            .unwrap_or_default()
    }

    /// IS NOT NULL 查询 — 返回值非 NULL 的所有行索引
    pub fn is_not_null_query(&self) -> Vec<usize> {
        match self.bitmaps.get(&value_key(&Value::Null)) {
            Some((_, bitset)) => bitset.not().ones(),
            None => (0..self.num_rows).collect(),
        }
    }

    /// IN 查询 — 返回匹配任一值的所有行索引（多个位图 OR）
    pub fn in_query(&self, values: &[Value]) -> Result<Vec<usize>, BitmapIndexError> {
        if values.is_empty() {
            return Ok(Vec::new());
        }
        let mut result = Bitset::new(self.num_rows);
        for value in values {
            let bitset = self.eq_bitmap(value)?;
            result = result.or(bitset)?;
        }
        Ok(result.ones())
    }

    /// 获取所有 distinct 值
    pub fn distinct_values(&self) -> Vec<Value> {
        self.bitmaps.values().map(|(v, _)| v.clone()).collect()
    }

    /// 基数（distinct 值数量，含 NULL）
    pub fn cardinality(&self) -> usize {
        self.bitmaps.len()
    }

    /// 总行数
    pub fn num_rows(&self) -> usize {
        self.num_rows
    }

    /// 估算索引字节数
    ///
    /// 每个位图：num_rows/8 字节 + Value 开销。
    pub fn size_bytes(&self) -> usize {
        self.bitmaps
            .values()
            .map(|(_, bitset)| bitset.size_bytes() + 32) // 32 字节 Value 开销估算
            .sum()
    }

    /// 估算等价 B-Tree 字节数（每行 16 字节）
    pub fn estimated_btree_bytes(&self) -> usize {
        self.num_rows * 16
    }

    /// 获取统计信息
    pub fn stats(&self) -> BitmapStats {
        let size_bytes = self.size_bytes();
        let btree_bytes = self.estimated_btree_bytes();
        let compression_ratio = if size_bytes == 0 {
            0.0
        } else {
            btree_bytes as f64 / size_bytes as f64
        };
        BitmapStats {
            cardinality: self.cardinality(),
            num_rows: self.num_rows,
            size_bytes,
            estimated_btree_bytes: btree_bytes,
            compression_ratio,
        }
    }
}

impl Default for BitmapIndex {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  BitmapStats — 索引统计
// =====================================================================

/// Bitmap 索引统计信息
#[derive(Debug, Clone, PartialEq)]
pub struct BitmapStats {
    /// 基数（distinct 值数量）
    pub cardinality: usize,
    /// 总行数
    pub num_rows: usize,
    /// 估算索引字节数
    pub size_bytes: usize,
    /// 估算等价 B-Tree 字节数
    pub estimated_btree_bytes: usize,
    /// 压缩比（B-Tree / Bitmap）
    pub compression_ratio: f64,
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 生成 Value 的哈希键（使用 Debug 表示）
///
/// `Value` 未实现 `Hash`/`Eq`，使用 `format!("{v:?}")` 作为 HashMap 键。
fn value_key(value: &Value) -> String {
    format!("{value:?}")
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

    fn make_status_rows() -> Vec<Row> {
        vec![
            vec![Value::Text("active".to_string())],
            vec![Value::Text("inactive".to_string())],
            vec![Value::Text("active".to_string())],
            vec![Value::Text("pending".to_string())],
            vec![Value::Text("active".to_string())],
            vec![Value::Text("inactive".to_string())],
        ]
    }

    // =================================================================
    //  BitmapIndexError 测试
    // =================================================================

    #[test]
    fn test_error_to_execution_error() {
        let err: ExecutionError = BitmapIndexError::ValueNotFound("test".to_string()).into();
        assert!(matches!(err, ExecutionError::EvalError(_)));
    }

    #[test]
    fn test_error_value_not_found() {
        let index = BitmapIndex::new();
        let err = index.eq_query(&Value::Text("x".to_string())).unwrap_err();
        assert!(matches!(err, BitmapIndexError::ValueNotFound(_)));
    }

    #[test]
    fn test_error_length_mismatch() {
        let a = Bitset::new(64);
        let b = Bitset::new(128);
        let err = a.and(&b).unwrap_err();
        assert!(matches!(err, BitmapIndexError::LengthMismatch(64, 128)));
    }

    // =================================================================
    //  Bitset 基础测试
    // =================================================================

    #[test]
    fn test_bitset_new_empty() {
        let bs = Bitset::new(100);
        assert_eq!(bs.len(), 100);
        assert_eq!(bs.count_ones(), 0);
        assert!(bs.ones().is_empty());
    }

    #[test]
    fn test_bitset_set_get() {
        let mut bs = Bitset::new(100);
        bs.set(0);
        bs.set(63);
        bs.set(64);
        bs.set(99);

        assert!(bs.get(0));
        assert!(bs.get(63));
        assert!(bs.get(64));
        assert!(bs.get(99));
        assert!(!bs.get(1));
        assert!(!bs.get(100)); // 越界 → false
    }

    #[test]
    fn test_bitset_clear() {
        let mut bs = Bitset::new(64);
        bs.set(10);
        assert!(bs.get(10));
        bs.clear(10);
        assert!(!bs.get(10));
    }

    #[test]
    fn test_bitset_count_ones() {
        let mut bs = Bitset::new(100);
        bs.set(0);
        bs.set(50);
        bs.set(99);
        assert_eq!(bs.count_ones(), 3);
    }

    #[test]
    fn test_bitset_ones() {
        let mut bs = Bitset::new(100);
        bs.set(0);
        bs.set(63);
        bs.set(64);
        bs.set(99);

        let ones = bs.ones();
        assert_eq!(ones, vec![0, 63, 64, 99]);
    }

    #[test]
    fn test_bitset_all_ones() {
        let bs = Bitset::all_ones(10);
        assert_eq!(bs.count_ones(), 10);
        assert_eq!(bs.ones(), vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn test_bitset_size_bytes() {
        let bs = Bitset::new(128); // 2 words
        assert_eq!(bs.size_bytes(), 16);
    }

    // =================================================================
    //  Bitset 运算测试
    // =================================================================

    #[test]
    fn test_bitset_and() {
        let mut a = Bitset::new(100);
        a.set(0);
        a.set(1);
        a.set(2);

        let mut b = Bitset::new(100);
        b.set(1);
        b.set(2);
        b.set(3);

        let result = a.and(&b).unwrap();
        assert_eq!(result.ones(), vec![1, 2]);
        assert_eq!(result.count_ones(), 2);
    }

    #[test]
    fn test_bitset_or() {
        let mut a = Bitset::new(100);
        a.set(0);
        a.set(1);

        let mut b = Bitset::new(100);
        b.set(1);
        b.set(2);

        let result = a.or(&b).unwrap();
        assert_eq!(result.ones(), vec![0, 1, 2]);
    }

    #[test]
    fn test_bitset_not() {
        let mut a = Bitset::new(10);
        a.set(1);
        a.set(3);
        a.set(5);

        let result = a.not();
        assert_eq!(result.ones(), vec![0, 2, 4, 6, 7, 8, 9]);
        assert_eq!(result.count_ones(), 7);
    }

    #[test]
    fn test_bitset_xor() {
        let mut a = Bitset::new(100);
        a.set(0);
        a.set(1);
        a.set(2);

        let mut b = Bitset::new(100);
        b.set(1);
        b.set(2);
        b.set(3);

        let result = a.xor(&b).unwrap();
        assert_eq!(result.ones(), vec![0, 3]);
    }

    #[test]
    fn test_bitset_and_disjoint() {
        let mut a = Bitset::new(100);
        a.set(0);
        a.set(1);

        let mut b = Bitset::new(100);
        b.set(2);
        b.set(3);

        let result = a.and(&b).unwrap();
        assert_eq!(result.count_ones(), 0);
    }

    #[test]
    fn test_bitset_word_boundary() {
        // 测试 64 位边界
        let mut bs = Bitset::new(130); // 3 words
        bs.set(0);
        bs.set(63);
        bs.set(64);
        bs.set(127);
        bs.set(128);
        bs.set(129);

        assert_eq!(bs.count_ones(), 6);
        assert_eq!(bs.ones(), vec![0, 63, 64, 127, 128, 129]);
    }

    #[test]
    fn test_bitset_not_at_boundary() {
        // 测试 not 在边界处不越界
        let mut bs = Bitset::new(70); // 2 words, 第二个 word 只有 6 位有效
        bs.set(0);
        bs.set(65);

        let result = bs.not();
        // 有效位 0-69，1 被清除，65 被清除，其余为 1
        assert!(!result.get(0));
        assert!(result.get(1));
        assert!(!result.get(65));
        assert!(result.get(69));
        assert_eq!(result.count_ones(), 68); // 70 - 2
    }

    // =================================================================
    //  BitmapIndex 基础测试
    // =================================================================

    #[test]
    fn test_index_new_empty() {
        let index = BitmapIndex::new();
        assert_eq!(index.num_rows(), 0);
        assert_eq!(index.cardinality(), 0);
    }

    #[test]
    fn test_index_insert_single() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Text("a".to_string()));

        assert_eq!(index.num_rows(), 1);
        assert_eq!(index.cardinality(), 1);
    }

    #[test]
    fn test_index_insert_multiple() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Text("active".to_string()));
        index.insert(1, Value::Text("inactive".to_string()));
        index.insert(2, Value::Text("active".to_string()));

        assert_eq!(index.num_rows(), 3);
        assert_eq!(index.cardinality(), 2); // active, inactive
    }

    #[test]
    fn test_index_insert_null() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Text("a".to_string()));
        index.insert(1, Value::Null);
        index.insert(2, Value::Text("a".to_string()));
        index.insert(3, Value::Null);

        assert_eq!(index.cardinality(), 2); // "a", NULL
    }

    #[test]
    fn test_index_insert_extends_bitmap() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Text("a".to_string()));
        assert_eq!(index.num_rows(), 1);

        // 插入到行 5 → 扩展所有位图
        index.insert(5, Value::Text("a".to_string()));
        assert_eq!(index.num_rows(), 6);

        // "a" 的位图应只有 0 和 5 为 1
        let rows = index.eq_query(&Value::Text("a".to_string())).unwrap();
        assert_eq!(rows, vec![0, 5]);
    }

    #[test]
    fn test_index_insert_new_value_extends_correctly() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Text("a".to_string()));
        index.insert(1, Value::Text("a".to_string()));
        // 插入新值到行 3（行 2 未插入，位为 0）
        index.insert(3, Value::Text("b".to_string()));

        assert_eq!(index.num_rows(), 4);

        // "a" 位图：0, 1 为 1，2, 3 为 0
        let a_rows = index.eq_query(&Value::Text("a".to_string())).unwrap();
        assert_eq!(a_rows, vec![0, 1]);

        // "b" 位图：3 为 1，0, 1, 2 为 0
        let b_rows = index.eq_query(&Value::Text("b".to_string())).unwrap();
        assert_eq!(b_rows, vec![3]);
    }

    // =================================================================
    //  build_from_rows 测试
    // =================================================================

    #[test]
    fn test_build_from_rows_basic() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        assert_eq!(index.num_rows(), 6);
        assert_eq!(index.cardinality(), 3); // active, inactive, pending

        let active = index.eq_query(&Value::Text("active".to_string())).unwrap();
        assert_eq!(active, vec![0, 2, 4]);

        let inactive = index
            .eq_query(&Value::Text("inactive".to_string()))
            .unwrap();
        assert_eq!(inactive, vec![1, 5]);

        let pending = index.eq_query(&Value::Text("pending".to_string())).unwrap();
        assert_eq!(pending, vec![3]);
    }

    #[test]
    fn test_build_from_rows_empty() {
        let rows: Vec<Row> = vec![];
        let index = BitmapIndex::build_from_rows(&rows, 0);
        assert_eq!(index.num_rows(), 0);
        assert_eq!(index.cardinality(), 0);
    }

    #[test]
    fn test_build_from_rows_missing_column() {
        let rows: Vec<Row> = vec![
            vec![Value::Text("a".to_string())],
            vec![], // 缺列 → NULL
        ];
        let index = BitmapIndex::build_from_rows(&rows, 0);

        assert_eq!(index.cardinality(), 2); // "a", NULL
        let nulls = index.is_null_query();
        assert_eq!(nulls, vec![1]);
    }

    // =================================================================
    //  eq_query 测试
    // =================================================================

    #[test]
    fn test_eq_query_basic() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let result = index.eq_query(&Value::Text("active".to_string())).unwrap();
        assert_eq!(result, vec![0, 2, 4]);
    }

    #[test]
    fn test_eq_query_not_found() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let err = index
            .eq_query(&Value::Text("deleted".to_string()))
            .unwrap_err();
        assert!(matches!(err, BitmapIndexError::ValueNotFound(_)));
    }

    #[test]
    fn test_eq_query_empty_index() {
        let index = BitmapIndex::new();
        let err = index.eq_query(&Value::Text("x".to_string())).unwrap_err();
        assert!(matches!(err, BitmapIndexError::ValueNotFound(_)));
    }

    #[test]
    fn test_eq_query_null() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Text("a".to_string()));
        index.insert(1, Value::Null);
        index.insert(2, Value::Text("a".to_string()));
        index.insert(3, Value::Null);

        let nulls = index.eq_query(&Value::Null).unwrap();
        assert_eq!(nulls, vec![1, 3]);
    }

    // =================================================================
    //  ne_query 测试
    // =================================================================

    #[test]
    fn test_ne_query_basic() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        // 不等于 "active" → inactive(1,5) + pending(3)
        let result = index.ne_query(&Value::Text("active".to_string())).unwrap();
        assert_eq!(result, vec![1, 3, 5]);
    }

    #[test]
    fn test_ne_query_value_not_present() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        // "deleted" 不存在 → 所有行都不等于它
        let result = index.ne_query(&Value::Text("deleted".to_string())).unwrap();
        assert_eq!(result, vec![0, 1, 2, 3, 4, 5]);
    }

    // =================================================================
    //  IS NULL / IS NOT NULL 测试
    // =================================================================

    #[test]
    fn test_is_null_query() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Text("a".to_string()));
        index.insert(1, Value::Null);
        index.insert(2, Value::Text("b".to_string()));
        index.insert(3, Value::Null);

        let nulls = index.is_null_query();
        assert_eq!(nulls, vec![1, 3]);
    }

    #[test]
    fn test_is_not_null_query() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Text("a".to_string()));
        index.insert(1, Value::Null);
        index.insert(2, Value::Text("b".to_string()));
        index.insert(3, Value::Null);

        let not_nulls = index.is_not_null_query();
        assert_eq!(not_nulls, vec![0, 2]);
    }

    #[test]
    fn test_is_null_query_no_nulls() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let nulls = index.is_null_query();
        assert!(nulls.is_empty());
    }

    #[test]
    fn test_is_not_null_query_all_not_null() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let not_nulls = index.is_not_null_query();
        assert_eq!(not_nulls, vec![0, 1, 2, 3, 4, 5]);
    }

    // =================================================================
    //  IN 查询测试
    // =================================================================

    #[test]
    fn test_in_query_basic() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        // IN ('active', 'pending') → active(0,2,4) + pending(3)
        let result = index
            .in_query(&[
                Value::Text("active".to_string()),
                Value::Text("pending".to_string()),
            ])
            .unwrap();
        assert_eq!(result, vec![0, 2, 3, 4]);
    }

    #[test]
    fn test_in_query_single_value() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let result = index
            .in_query(&[Value::Text("active".to_string())])
            .unwrap();
        assert_eq!(result, vec![0, 2, 4]);
    }

    #[test]
    fn test_in_query_empty_values() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let result = index.in_query(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_in_query_value_not_found() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let err = index
            .in_query(&[Value::Text("deleted".to_string())])
            .unwrap_err();
        assert!(matches!(err, BitmapIndexError::ValueNotFound(_)));
    }

    // =================================================================
    //  distinct_values 测试
    // =================================================================

    #[test]
    fn test_distinct_values() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let values = index.distinct_values();
        assert_eq!(values.len(), 3);
        // 顺序不确定，检查包含
        assert!(values.contains(&Value::Text("active".to_string())));
        assert!(values.contains(&Value::Text("inactive".to_string())));
        assert!(values.contains(&Value::Text("pending".to_string())));
    }

    // =================================================================
    //  统计与大小测试
    // =================================================================

    #[test]
    fn test_size_bytes_empty() {
        let index = BitmapIndex::new();
        assert_eq!(index.size_bytes(), 0);
    }

    #[test]
    fn test_size_bytes_non_empty() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        // 3 个 distinct 值，6 行 → 每个位图 1 word (8 bytes) + 32 Value 开销 = 40
        // 3 * 40 = 120
        assert_eq!(index.size_bytes(), 120);
    }

    #[test]
    fn test_estimated_btree_bytes() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);
        // 6 rows * 16 = 96
        assert_eq!(index.estimated_btree_bytes(), 96);
    }

    #[test]
    fn test_stats_basic() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let stats = index.stats();
        assert_eq!(stats.cardinality, 3);
        assert_eq!(stats.num_rows, 6);
        assert_eq!(stats.size_bytes, 120);
        assert_eq!(stats.estimated_btree_bytes, 96);
        // compression_ratio = 96 / 120 = 0.8（低基数时 Bitmap 比 B-Tree 大）
        assert!((stats.compression_ratio - 0.8).abs() < 0.001);
    }

    // =================================================================
    //  低基数场景测试
    // =================================================================

    #[test]
    fn test_low_cardinality_gender() {
        // 性别列：M/F，1000 行
        let rows: Vec<Row> = (0..1000_i64)
            .map(|i| {
                vec![Value::Text(if i % 2 == 0 {
                    "M".to_string()
                } else {
                    "F".to_string()
                })]
            })
            .collect();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        assert_eq!(index.cardinality(), 2);
        assert_eq!(index.num_rows(), 1000);

        let males = index.eq_query(&Value::Text("M".to_string())).unwrap();
        assert_eq!(males.len(), 500);
        assert!(males.contains(&0));
        assert!(males.contains(&998));

        let females = index.eq_query(&Value::Text("F".to_string())).unwrap();
        assert_eq!(females.len(), 500);
    }

    #[test]
    fn test_low_cardinality_status_10_values() {
        // status 列：10 个值，1000 行
        let rows: Vec<Row> = (0..1000)
            .map(|i| vec![Value::Text(format!("status_{}", i % 10))])
            .collect();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        assert_eq!(index.cardinality(), 10);
        assert_eq!(index.num_rows(), 1000);

        // 每个值 100 行
        for i in 0..10 {
            let result = index.eq_query(&Value::Text(format!("status_{i}"))).unwrap();
            assert_eq!(result.len(), 100);
        }
    }

    #[test]
    fn test_low_cardinality_compression_advantage() {
        // 10000 行，2 个值 → Bitmap 远小于 B-Tree
        let rows: Vec<Row> = (0..10000_i64)
            .map(|i| {
                vec![Value::Text(if i % 2 == 0 {
                    "Y".to_string()
                } else {
                    "N".to_string()
                })]
            })
            .collect();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let stats = index.stats();
        // Bitmap: 2 values * (10000/8 = 1250 bytes + 32) = 2 * 1282 = 2564
        // B-Tree: 10000 * 16 = 160000
        // 压缩比 = 160000 / 2564 ≈ 62.4
        assert!(stats.compression_ratio > 50.0); // 显著优于 B-Tree
    }

    // =================================================================
    //  多类型测试
    // =================================================================

    #[test]
    fn test_int64_type() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Int64(1));
        index.insert(1, Value::Int64(2));
        index.insert(2, Value::Int64(1));
        index.insert(3, Value::Int64(3));

        let ones = index.eq_query(&Value::Int64(1)).unwrap();
        assert_eq!(ones, vec![0, 2]);
    }

    #[test]
    fn test_bool_type() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Bool(true));
        index.insert(1, Value::Bool(false));
        index.insert(2, Value::Bool(true));

        let trues = index.eq_query(&Value::Bool(true)).unwrap();
        assert_eq!(trues, vec![0, 2]);

        let falses = index.eq_query(&Value::Bool(false)).unwrap();
        assert_eq!(falses, vec![1]);
    }

    #[test]
    fn test_mixed_types_separate_values() {
        // Int64(1) 和 Float64(1.0) 是不同的值（Debug 表示不同）
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Int64(1));
        index.insert(1, Value::Float64(1.0));

        assert_eq!(index.cardinality(), 2);

        let int_rows = index.eq_query(&Value::Int64(1)).unwrap();
        assert_eq!(int_rows, vec![0]);

        let float_rows = index.eq_query(&Value::Float64(1.0)).unwrap();
        assert_eq!(float_rows, vec![1]);
    }

    // =================================================================
    //  位图组合运算 E2E 测试
    // =================================================================

    #[test]
    fn test_e2e_bitmap_and_two_columns() {
        // 模拟两列的位图 AND：status='active' AND region='E'
        let mut status_index = BitmapIndex::new();
        let mut region_index = BitmapIndex::new();

        for i in 0..100 {
            status_index.insert(
                i,
                Value::Text(if i.is_multiple_of(3) {
                    "active".to_string()
                } else {
                    "inactive".to_string()
                }),
            );
            region_index.insert(
                i,
                Value::Text(if i.is_multiple_of(5) {
                    "E".to_string()
                } else {
                    "W".to_string()
                }),
            );
        }

        // status='active' AND region='E'
        let active_bm = status_index
            .eq_bitmap(&Value::Text("active".to_string()))
            .unwrap();
        let east_bm = region_index
            .eq_bitmap(&Value::Text("E".to_string()))
            .unwrap();
        let result = active_bm.and(east_bm).unwrap();

        // active: 0,3,6,...,99 (i%3==0) → 34 个
        // east: 0,5,10,...,95 (i%5==0) → 20 个
        // AND: i%3==0 && i%5==0 → i%15==0 → 0,15,30,45,60,75,90 → 7 个
        assert_eq!(result.count_ones(), 7);
        assert!(result.ones().contains(&0));
        assert!(result.ones().contains(&15));
        assert!(result.ones().contains(&90));
    }

    #[test]
    fn test_e2e_bitmap_or_two_columns() {
        let mut status_index = BitmapIndex::new();
        let mut region_index = BitmapIndex::new();

        for i in 0..100 {
            status_index.insert(
                i,
                Value::Text(if i.is_multiple_of(3) {
                    "active".to_string()
                } else {
                    "inactive".to_string()
                }),
            );
            region_index.insert(
                i,
                Value::Text(if i.is_multiple_of(5) {
                    "E".to_string()
                } else {
                    "W".to_string()
                }),
            );
        }

        let active_bm = status_index
            .eq_bitmap(&Value::Text("active".to_string()))
            .unwrap();
        let east_bm = region_index
            .eq_bitmap(&Value::Text("E".to_string()))
            .unwrap();
        let result = active_bm.or(east_bm).unwrap();

        // active(34) + east(20) - both(7) = 47
        assert_eq!(result.count_ones(), 47);
    }

    #[test]
    fn test_e2e_bitmap_not() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        // NOT active → inactive(1,5) + pending(3)
        let active_bm = index.eq_bitmap(&Value::Text("active".to_string())).unwrap();
        let result = active_bm.not();

        assert_eq!(result.ones(), vec![1, 3, 5]);
    }

    #[test]
    fn test_e2e_in_query_as_or() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        // IN ('active', 'pending') 等价于 active OR pending
        let result = index
            .in_query(&[
                Value::Text("active".to_string()),
                Value::Text("pending".to_string()),
            ])
            .unwrap();
        // active(0,2,4) + pending(3) = 4 行
        assert_eq!(result.len(), 4);
        assert!(result.contains(&0));
        assert!(result.contains(&2));
        assert!(result.contains(&3));
        assert!(result.contains(&4));
    }

    // =================================================================
    //  E2E 综合场景测试
    // =================================================================

    #[test]
    fn test_e2e_order_status_workflow() {
        // 模拟订单状态工作流
        let rows: Vec<Row> = vec![
            vec![Value::Text("created".to_string())],   // 0
            vec![Value::Text("created".to_string())],   // 1
            vec![Value::Text("paid".to_string())],      // 2
            vec![Value::Text("paid".to_string())],      // 3
            vec![Value::Text("shipped".to_string())],   // 4
            vec![Value::Text("delivered".to_string())], // 5
            vec![Value::Text("cancelled".to_string())], // 6
            vec![Value::Text("paid".to_string())],      // 7
        ];
        let index = BitmapIndex::build_from_rows(&rows, 0);

        // 查询所有 paid 订单
        let paid = index.eq_query(&Value::Text("paid".to_string())).unwrap();
        assert_eq!(paid, vec![2, 3, 7]);

        // 查询非 cancelled 订单
        let not_cancelled = index
            .ne_query(&Value::Text("cancelled".to_string()))
            .unwrap();
        assert_eq!(not_cancelled.len(), 7);
        assert!(!not_cancelled.contains(&6));

        // 查询活跃订单（created 或 paid）
        let active = index
            .in_query(&[
                Value::Text("created".to_string()),
                Value::Text("paid".to_string()),
            ])
            .unwrap();
        assert_eq!(active.len(), 5); // 0,1,2,3,7

        // 统计
        let stats = index.stats();
        assert_eq!(stats.cardinality, 5); // created, paid, shipped, delivered, cancelled
        assert_eq!(stats.num_rows, 8);
    }

    #[test]
    fn test_e2e_large_scale_low_cardinality() {
        // 50000 行，5 个值
        let rows: Vec<Row> = (0..50000)
            .map(|i| vec![Value::Text(format!("cat_{}", i % 5))])
            .collect();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let stats = index.stats();
        assert_eq!(stats.cardinality, 5);
        assert_eq!(stats.num_rows, 50000);

        // 每个值 10000 行
        for i in 0..5 {
            let result = index.eq_query(&Value::Text(format!("cat_{i}"))).unwrap();
            assert_eq!(result.len(), 10000);
        }

        // Bitmap 优势：5 * (50000/8 + 32) = 5 * 6282 = 31410
        // B-Tree: 50000 * 16 = 800000
        // 压缩比 ≈ 25.5
        assert!(stats.compression_ratio > 20.0);
    }

    #[test]
    fn test_e2e_incremental_insert() {
        let mut index = BitmapIndex::new();

        // 初始插入
        for i in 0..5 {
            index.insert(i, Value::Text("a".to_string()));
        }
        assert_eq!(index.num_rows(), 5);

        // 追加插入新值
        index.insert(5, Value::Text("b".to_string()));
        index.insert(6, Value::Text("b".to_string()));
        assert_eq!(index.num_rows(), 7);

        // "a" 位图应为 0-4
        let a_rows = index.eq_query(&Value::Text("a".to_string())).unwrap();
        assert_eq!(a_rows, vec![0, 1, 2, 3, 4]);

        // "b" 位图应为 5, 6
        let b_rows = index.eq_query(&Value::Text("b".to_string())).unwrap();
        assert_eq!(b_rows, vec![5, 6]);
    }

    #[test]
    fn test_e2e_bitmap_index_debug_format() {
        let mut index = BitmapIndex::new();
        index.insert(0, Value::Text("a".to_string()));
        index.insert(1, Value::Text("b".to_string()));

        let debug = format!("{index:?}");
        assert!(debug.contains("BitmapIndex"));
        assert!(debug.contains("num_rows: 2"));
        assert!(debug.contains("cardinality: 2"));
    }

    #[test]
    fn test_eq_bitmap_returns_reference() {
        let rows = make_status_rows();
        let index = BitmapIndex::build_from_rows(&rows, 0);

        let bitset = index.eq_bitmap(&Value::Text("active".to_string())).unwrap();
        assert_eq!(bitset.len(), 6);
        assert_eq!(bitset.count_ones(), 3);
    }

    #[test]
    fn test_default_trait() {
        let index = BitmapIndex::default();
        assert_eq!(index.num_rows(), 0);
        assert_eq!(index.cardinality(), 0);
    }
}
