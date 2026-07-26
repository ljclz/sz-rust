//! TOAST 超大字段存储（The Oversized-Attribute Storage Technique）— Phase 7d.7
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.7 TOAST 超大字段存储设计。
//!
//! # 设计
//!
//! 借鉴 PostgreSQL TOAST 机制：当字段大小超过阈值（默认 3KB）时，自动将字段
//! 分离到 TOAST 表（独立存储区域），行内仅保留 8 字节指针（toast_id + chunk_count）。
//! 查询时透明重组：从 TOAST 表读取所有 chunks 按 chunk_seq 顺序拼接还原。
//!
//! - **ToastValue** — 字段值（Inline 行内存储 / Toasted TOAST 存储）
//! - **ToastPointer** — 8 字节指针（toast_id 4 字节 + chunk_count 4 字节）
//! - **ToastChunk** — TOAST 数据块（toast_id + chunk_seq + data）
//! - **ToastStorage** — TOAST 存储（toast_id → chunks 有序映射）
//! - **ToastManager** — TOAST 管理器（自动判断阈值，分离/重组透明化）
//!
//! ## 验证标准
//!
//! - 写入 100000 条含 10KB~1MB 大字段的行 → 自动 TOAST 分离存储
//! - 查询时透明重组 → 结果与原始数据一致
//! - 行内仅存 8 字节指针（>3KB 字段）
//! - 3KB 以下字段保持行内存储

use std::collections::{BTreeMap, HashMap};

// =====================================================================
//  常量
// =====================================================================

/// TOAST 阈值 — 超过此大小的字段自动分离到 TOAST 存储（3KB = 3072 字节）
pub const TOAST_THRESHOLD: usize = 3_072;

/// TOAST chunk 大小 — 每块 2KB = 2048 字节（PostgreSQL 默认 2KB）
pub const TOAST_CHUNK_SIZE: usize = 2_048;

/// TOAST 指针大小 — 8 字节（toast_id 4 字节 + chunk_count 4 字节）
pub const TOAST_POINTER_SIZE: usize = 8;

/// TOAST 最大字段大小 — 1GB（理论上限，chunk_count * chunk_size）
pub const TOAST_MAX_FIELD_SIZE: usize = 1_073_741_824;

// =====================================================================
//  ToastValue — 字段值（行内 / TOAST）
// =====================================================================

/// 字段值 — 行内存储或 TOAST 存储
///
/// - **Inline(Vec<u8>)** — 行内存储（字段 <= TOAST_THRESHOLD）
/// - **Toasted(ToastPointer)** — TOAST 存储（字段 > TOAST_THRESHOLD，行内仅 8 字节指针）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastValue {
    /// 行内存储 — 完整数据保存在行中
    Inline(Vec<u8>),
    /// TOAST 存储 — 行内仅存 8 字节指针，实际数据在 TOAST 表
    Toasted(ToastPointer),
}

impl ToastValue {
    /// 构造行内存储值
    pub fn inline(data: Vec<u8>) -> Self {
        ToastValue::Inline(data)
    }

    /// 构造 TOAST 存储值
    pub fn toasted(pointer: ToastPointer) -> Self {
        ToastValue::Toasted(pointer)
    }

    /// 是否行内存储
    pub fn is_inline(&self) -> bool {
        matches!(self, ToastValue::Inline(_))
    }

    /// 是否 TOAST 存储
    pub fn is_toasted(&self) -> bool {
        matches!(self, ToastValue::Toasted(_))
    }

    /// 行内存储的数据大小（字节）
    ///
    /// - Inline → data.len()
    /// - Toasted → TOAST_POINTER_SIZE（8 字节指针）
    pub fn inline_size(&self) -> usize {
        match self {
            ToastValue::Inline(data) => data.len(),
            ToastValue::Toasted(_) => TOAST_POINTER_SIZE,
        }
    }

    /// 获取 TOAST 指针（若为 Toasted）
    pub fn pointer(&self) -> Option<&ToastPointer> {
        match self {
            ToastValue::Toasted(p) => Some(p),
            ToastValue::Inline(_) => None,
        }
    }

    /// 获取行内数据（若为 Inline）
    pub fn inline_data(&self) -> Option<&[u8]> {
        match self {
            ToastValue::Inline(data) => Some(data),
            ToastValue::Toasted(_) => None,
        }
    }
}

// =====================================================================
//  ToastPointer — 8 字节 TOAST 指针
// =====================================================================

/// TOAST 指针 — 8 字节（toast_id 4 字节 + chunk_count 4 字节）
///
/// 行内仅存此指针，实际数据在 TOAST 表中按 chunk_seq 分块存储。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToastPointer {
    /// TOAST ID — 标识一个 TOAST 字段（对应一组 chunks）
    pub toast_id: u32,
    /// chunk 数量 — 该字段被分成多少块
    pub chunk_count: u32,
}

impl ToastPointer {
    /// 构造 TOAST 指针
    pub fn new(toast_id: u32, chunk_count: u32) -> Self {
        Self {
            toast_id,
            chunk_count,
        }
    }

    /// 指针大小（字节） — 固定 8 字节
    pub fn size(&self) -> usize {
        TOAST_POINTER_SIZE
    }

    /// 序列化为 8 字节（toast_id 4 字节小端 + chunk_count 4 字节小端）
    pub fn to_bytes(&self) -> [u8; TOAST_POINTER_SIZE] {
        let mut bytes = [0u8; TOAST_POINTER_SIZE];
        bytes[0..4].copy_from_slice(&self.toast_id.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.chunk_count.to_le_bytes());
        bytes
    }

    /// 从 8 字节反序列化
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ToastError> {
        if bytes.len() < TOAST_POINTER_SIZE {
            return Err(ToastError::InvalidPointerSize {
                expected: TOAST_POINTER_SIZE,
                actual: bytes.len(),
            });
        }
        let toast_id = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let chunk_count = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        Ok(Self {
            toast_id,
            chunk_count,
        })
    }
}

impl std::fmt::Display for ToastPointer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ToastPointer(toast_id={}, chunk_count={})",
            self.toast_id, self.chunk_count
        )
    }
}

// =====================================================================
//  ToastChunk — TOAST 数据块
// =====================================================================

/// TOAST 数据块 — 一个大字段被切成多个 chunk 存储
///
/// 每个 chunk 包含 toast_id（标识字段）+ chunk_seq（块序号）+ data（块数据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToastChunk {
    /// TOAST ID — 标识所属字段
    pub toast_id: u32,
    /// 块序号 — 从 0 开始递增，重组时按序号拼接
    pub chunk_seq: u32,
    /// 块数据 — 固定 TOAST_CHUNK_SIZE（最后一块可能不足）
    pub data: Vec<u8>,
}

impl ToastChunk {
    /// 构造 TOAST 块
    pub fn new(toast_id: u32, chunk_seq: u32, data: Vec<u8>) -> Self {
        Self {
            toast_id,
            chunk_seq,
            data,
        }
    }

    /// 块数据大小（字节）
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// 是否为满块（== TOAST_CHUNK_SIZE）
    pub fn is_full(&self) -> bool {
        self.data.len() == TOAST_CHUNK_SIZE
    }
}

// =====================================================================
//  ToastError — 错误类型
// =====================================================================

/// TOAST 错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastError {
    /// 字段超过最大限制
    FieldTooLarge { size: usize, max: usize },
    /// TOAST 指针大小无效
    InvalidPointerSize { expected: usize, actual: usize },
    /// TOAST ID 不存在（指针无效或已被删除）
    ToastIdNotFound { toast_id: u32 },
    /// chunk 序号不连续（数据损坏）
    ChunkSequenceBroken {
        toast_id: u32,
        expected_seq: u32,
        actual_seq: u32,
    },
    /// chunk 数量与指针中记录的不一致
    ChunkCountMismatch {
        toast_id: u32,
        expected: u32,
        actual: u32,
    },
}

impl std::fmt::Display for ToastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToastError::FieldTooLarge { size, max } => {
                write!(f, "field too large: {size} bytes (max {max} bytes)")
            }
            ToastError::InvalidPointerSize { expected, actual } => {
                write!(
                    f,
                    "invalid toast pointer size: expected {expected} bytes, got {actual} bytes"
                )
            }
            ToastError::ToastIdNotFound { toast_id } => {
                write!(f, "toast id {toast_id} not found")
            }
            ToastError::ChunkSequenceBroken {
                toast_id,
                expected_seq,
                actual_seq,
            } => {
                write!(
                    f,
                    "toast id {toast_id} chunk sequence broken: expected seq {expected_seq}, got {actual_seq}"
                )
            }
            ToastError::ChunkCountMismatch {
                toast_id,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "toast id {toast_id} chunk count mismatch: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for ToastError {}

// =====================================================================
//  ToastStorage — TOAST 存储
// =====================================================================

/// TOAST 存储 — 按 toast_id 组织的 chunks 有序映射
///
/// 每个 toast_id 对应一个 BTreeMap<chunk_seq, data>，保证按序号顺序读取。
#[derive(Debug, Clone, Default)]
pub struct ToastStorage {
    /// toast_id → chunks 有序映射
    chunks: HashMap<u32, BTreeMap<u32, Vec<u8>>>,
    /// 总 chunk 数
    total_chunks: u64,
    /// 总数据字节数（实际 chunk data 大小总和）
    total_bytes: u64,
}

impl ToastStorage {
    /// 构造空 TOAST 存储
    pub fn new() -> Self {
        Self::default()
    }

    /// 存储一个字段的全部 chunks（覆盖已有）
    ///
    /// 返回 toast_id 与 chunk 数量
    pub fn store_field(&mut self, toast_id: u32, data: &[u8]) -> u32 {
        let chunks = split_into_chunks(data);
        let chunk_count = chunks.len() as u32;

        let mut chunk_map = BTreeMap::new();
        for (seq, chunk_data) in chunks.into_iter().enumerate() {
            let old = chunk_map.insert(seq as u32, chunk_data);
            debug_assert!(old.is_none(), "chunk seq should not duplicate");
        }

        // 统计字节数（减去可能已有的）
        if let Some(old_map) = self.chunks.get(&toast_id) {
            for data in old_map.values() {
                self.total_bytes = self.total_bytes.saturating_sub(data.len() as u64);
                self.total_chunks = self.total_chunks.saturating_sub(1);
            }
        }

        for data in chunk_map.values() {
            self.total_bytes += data.len() as u64;
            self.total_chunks += 1;
        }

        self.chunks.insert(toast_id, chunk_map);
        chunk_count
    }

    /// 读取一个字段（透明重组所有 chunks）
    pub fn load_field(&self, pointer: &ToastPointer) -> Result<Vec<u8>, ToastError> {
        let chunk_map = self
            .chunks
            .get(&pointer.toast_id)
            .ok_or(ToastError::ToastIdNotFound {
                toast_id: pointer.toast_id,
            })?;

        if chunk_map.len() as u32 != pointer.chunk_count {
            return Err(ToastError::ChunkCountMismatch {
                toast_id: pointer.toast_id,
                expected: pointer.chunk_count,
                actual: chunk_map.len() as u32,
            });
        }

        let mut result = Vec::with_capacity(pointer.chunk_count as usize * TOAST_CHUNK_SIZE);
        for (expected_seq, (&seq, data)) in chunk_map.iter().enumerate() {
            let expected_seq = expected_seq as u32;
            if seq != expected_seq {
                return Err(ToastError::ChunkSequenceBroken {
                    toast_id: pointer.toast_id,
                    expected_seq,
                    actual_seq: seq,
                });
            }
            result.extend_from_slice(data);
        }

        Ok(result)
    }

    /// 删除一个字段的所有 chunks
    ///
    /// 返回被删除的 chunk 数
    pub fn delete_field(&mut self, toast_id: u32) -> usize {
        if let Some(chunk_map) = self.chunks.remove(&toast_id) {
            let count = chunk_map.len();
            for data in chunk_map.values() {
                self.total_bytes = self.total_bytes.saturating_sub(data.len() as u64);
            }
            self.total_chunks = self.total_chunks.saturating_sub(count as u64);
            count
        } else {
            0
        }
    }

    /// 是否包含指定 toast_id
    pub fn contains(&self, toast_id: u32) -> bool {
        self.chunks.contains_key(&toast_id)
    }

    /// 指定字段的 chunk 数量
    pub fn chunk_count(&self, toast_id: u32) -> usize {
        self.chunks.get(&toast_id).map_or(0, |m| m.len())
    }

    /// 存储中的字段数（不同 toast_id 数量）
    pub fn field_count(&self) -> usize {
        self.chunks.len()
    }

    /// 总 chunk 数
    pub fn total_chunks(&self) -> u64 {
        self.total_chunks
    }

    /// 总数据字节数（chunk data 总和，非行内字节数）
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// 清空所有 TOAST 存储
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.total_chunks = 0;
        self.total_bytes = 0;
    }
}

// =====================================================================
//  ToastStats — TOAST 统计
// =====================================================================

/// TOAST 统计
#[derive(Debug, Clone, Default)]
pub struct ToastStats {
    /// 行内存储字段数
    pub inline_count: u64,
    /// TOAST 存储字段数
    pub toasted_count: u64,
    /// 行内存储总字节数
    pub inline_bytes: u64,
    /// TOAST 存储总字节数（原始字段大小，非 chunk 大小）
    pub toasted_bytes: u64,
    /// TOAST 节省的行内字节数（toasted_bytes - toasted_count * 8）
    pub saved_inline_bytes: u64,
    /// detoast（重组）次数
    pub detoast_count: u64,
    /// delete（删除）次数
    pub delete_count: u64,
}

impl ToastStats {
    /// 构造空统计
    pub fn new() -> Self {
        Self::default()
    }

    /// 总字段数
    pub fn total_count(&self) -> u64 {
        self.inline_count + self.toasted_count
    }

    /// TOAST 比例（0.0 ~ 1.0）
    pub fn toast_ratio(&self) -> f64 {
        let total = self.total_count();
        if total == 0 {
            return 0.0;
        }
        self.toasted_count as f64 / total as f64
    }

    /// 行内节省率（saved_inline_bytes / (inline_bytes + toasted_bytes)）
    pub fn inline_saving_ratio(&self) -> f64 {
        let total = self.inline_bytes + self.toasted_bytes;
        if total == 0 {
            return 0.0;
        }
        self.saved_inline_bytes as f64 / total as f64
    }
}

// =====================================================================
//  ToastManager — TOAST 管理器
// =====================================================================

/// TOAST 管理器 — 自动判断字段大小，超过阈值则分离到 TOAST 存储
///
/// 透明化操作：
/// - `maybe_toast(data)` → 写入字段，自动判断行内/TOAST
/// - `detoast(value)` → 读取字段，自动重组 TOAST 数据
pub struct ToastManager {
    /// TOAST 存储
    storage: ToastStorage,
    /// 下一个 toast_id（递增）
    next_toast_id: u32,
    /// TOAST 阈值（默认 3KB）
    threshold: usize,
    /// 统计
    stats: ToastStats,
}

impl ToastManager {
    /// 构造默认 TOAST 管理器（阈值 3KB）
    pub fn new() -> Self {
        Self {
            storage: ToastStorage::new(),
            next_toast_id: 1,
            threshold: TOAST_THRESHOLD,
            stats: ToastStats::new(),
        }
    }

    /// 构造自定义阈值的 TOAST 管理器
    pub fn with_threshold(threshold: usize) -> Self {
        Self {
            storage: ToastStorage::new(),
            next_toast_id: 1,
            threshold,
            stats: ToastStats::new(),
        }
    }

    /// 获取阈值
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// 获取存储引用
    pub fn storage(&self) -> &ToastStorage {
        &self.storage
    }

    /// 获取统计引用
    pub fn stats(&self) -> &ToastStats {
        &self.stats
    }

    /// 是否需要 TOAST（字段大小 > 阈值）
    pub fn needs_toast(&self, data_len: usize) -> bool {
        data_len > self.threshold
    }

    /// 写入字段 — 自动判断行内/TOAST
    ///
    /// - 若 data.len() <= threshold → 返回 Inline(data)
    /// - 若 data.len() > threshold → 分离到 TOAST 存储，返回 Toasted(pointer)
    pub fn maybe_toast(&mut self, data: Vec<u8>) -> Result<ToastValue, ToastError> {
        if data.len() > TOAST_MAX_FIELD_SIZE {
            return Err(ToastError::FieldTooLarge {
                size: data.len(),
                max: TOAST_MAX_FIELD_SIZE,
            });
        }

        if data.len() <= self.threshold {
            self.stats.inline_count += 1;
            self.stats.inline_bytes += data.len() as u64;
            Ok(ToastValue::Inline(data))
        } else {
            let toast_id = self.next_toast_id;
            self.next_toast_id += 1;

            let original_size = data.len();
            let chunk_count = self.storage.store_field(toast_id, &data);
            let pointer = ToastPointer::new(toast_id, chunk_count);

            self.stats.toasted_count += 1;
            self.stats.toasted_bytes += original_size as u64;
            self.stats.saved_inline_bytes += (original_size - TOAST_POINTER_SIZE) as u64;

            Ok(ToastValue::Toasted(pointer))
        }
    }

    /// 读取字段 — 透明重组
    ///
    /// - Inline → 直接返回数据
    /// - Toasted → 从 TOAST 存储读取并拼接所有 chunks
    pub fn detoast(&mut self, value: &ToastValue) -> Result<Vec<u8>, ToastError> {
        match value {
            ToastValue::Inline(data) => Ok(data.clone()),
            ToastValue::Toasted(pointer) => {
                self.stats.detoast_count += 1;
                self.storage.load_field(pointer)
            }
        }
    }

    /// 删除字段 — 若为 TOAST 则清理 TOAST 存储
    ///
    /// 返回被删除的 chunk 数（Inline 返回 0）
    pub fn delete(&mut self, value: &ToastValue) -> usize {
        match value {
            ToastValue::Inline(_) => 0,
            ToastValue::Toasted(pointer) => {
                self.stats.delete_count += 1;
                self.storage.delete_field(pointer.toast_id)
            }
        }
    }

    /// 获取存储的可变引用
    pub fn storage_mut(&mut self) -> &mut ToastStorage {
        &mut self.storage
    }

    /// 重置统计
    pub fn reset_stats(&mut self) {
        self.stats = ToastStats::new();
    }
}

impl Default for ToastManager {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 将数据切分为 chunks（每块 TOAST_CHUNK_SIZE 字节，最后一块可能不足）
///
/// 空数据返回 1 个空 chunk（保证至少有 1 块，chunk_count >= 1）
pub fn split_into_chunks(data: &[u8]) -> Vec<Vec<u8>> {
    if data.is_empty() {
        return vec![Vec::new()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < data.len() {
        let end = std::cmp::min(start + TOAST_CHUNK_SIZE, data.len());
        chunks.push(data[start..end].to_vec());
        start = end;
    }
    chunks
}

/// 计算 chunk 数量（不实际切分）
pub fn count_chunks(data_len: usize) -> u32 {
    if data_len == 0 {
        return 1;
    }
    data_len.div_ceil(TOAST_CHUNK_SIZE) as u32
}

/// 生成测试用大字段数据
///
/// `size` 为字节大小，内容为 `[0, 1, 2, ..., 255, 0, 1, ...]` 循环填充
pub fn generate_large_field(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

/// 生成测试用大字段数据（带 seed 的伪随机）
///
/// 使用简单 LCG 伪随机生成器，保证可复现
pub fn generate_large_field_seeded(size: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.max(1);
    (0..size)
        .map(|_| {
            // LCG: x = x * 6364136223846793005 + 1442695040888963407
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u8
        })
        .collect()
}

/// 校验重组后数据与原始数据一致
pub fn validate_detoast(original: &[u8], detoasted: &[u8]) -> bool {
    original.len() == detoasted.len() && original == detoasted
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  ToastValue 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_toast_value_inline() {
        let value = ToastValue::inline(vec![1, 2, 3]);
        assert!(value.is_inline());
        assert!(!value.is_toasted());
        assert_eq!(value.inline_size(), 3);
        assert_eq!(value.inline_data(), Some(&[1u8, 2, 3][..]));
        assert!(value.pointer().is_none());
    }

    #[test]
    fn test_toast_value_toasted() {
        let pointer = ToastPointer::new(42, 5);
        let value = ToastValue::toasted(pointer);
        assert!(!value.is_inline());
        assert!(value.is_toasted());
        assert_eq!(value.inline_size(), TOAST_POINTER_SIZE);
        assert!(value.inline_data().is_none());
        assert_eq!(value.pointer(), Some(&pointer));
    }

    #[test]
    fn test_toast_value_eq() {
        let v1 = ToastValue::inline(vec![1, 2, 3]);
        let v2 = ToastValue::inline(vec![1, 2, 3]);
        let v3 = ToastValue::inline(vec![1, 2, 4]);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);

        let p1 = ToastPointer::new(1, 2);
        let p2 = ToastPointer::new(1, 2);
        let p3 = ToastPointer::new(1, 3);
        assert_eq!(ToastValue::toasted(p1), ToastValue::toasted(p2));
        assert_ne!(ToastValue::toasted(p1), ToastValue::toasted(p3));
    }

    // -----------------------------------------------------------------
    //  ToastPointer 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_toast_pointer_new() {
        let pointer = ToastPointer::new(100, 7);
        assert_eq!(pointer.toast_id, 100);
        assert_eq!(pointer.chunk_count, 7);
    }

    #[test]
    fn test_toast_pointer_size() {
        let pointer = ToastPointer::new(1, 1);
        assert_eq!(pointer.size(), 8);
        assert_eq!(pointer.size(), TOAST_POINTER_SIZE);
    }

    #[test]
    fn test_toast_pointer_to_bytes() {
        let pointer = ToastPointer::new(0x12345678, 0x9ABCDEF0);
        let bytes = pointer.to_bytes();
        // 小端序
        assert_eq!(bytes[0..4], [0x78, 0x56, 0x34, 0x12]);
        assert_eq!(bytes[4..8], [0xF0, 0xDE, 0xBC, 0x9A]);
        assert_eq!(bytes.len(), 8);
    }

    #[test]
    fn test_toast_pointer_from_bytes() {
        let original = ToastPointer::new(0x12345678, 0x9ABCDEF0);
        let bytes = original.to_bytes();
        let restored = ToastPointer::from_bytes(&bytes).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn test_toast_pointer_from_bytes_invalid_size() {
        let bytes = [0u8; 4];
        let result = ToastPointer::from_bytes(&bytes);
        assert!(matches!(
            result,
            Err(ToastError::InvalidPointerSize {
                expected: 8,
                actual: 4
            })
        ));
    }

    #[test]
    fn test_toast_pointer_from_bytes_zero() {
        let bytes = [0u8; 8];
        let pointer = ToastPointer::from_bytes(&bytes).unwrap();
        assert_eq!(pointer.toast_id, 0);
        assert_eq!(pointer.chunk_count, 0);
    }

    #[test]
    fn test_toast_pointer_display() {
        let pointer = ToastPointer::new(42, 5);
        let s = format!("{pointer}");
        assert!(s.contains("42"));
        assert!(s.contains("5"));
        assert!(s.contains("ToastPointer"));
    }

    #[test]
    fn test_toast_pointer_roundtrip_many() {
        for toast_id in [0u32, 1, 100, u32::MAX] {
            for chunk_count in [0u32, 1, 100, u32::MAX] {
                let original = ToastPointer::new(toast_id, chunk_count);
                let bytes = original.to_bytes();
                let restored = ToastPointer::from_bytes(&bytes).unwrap();
                assert_eq!(original, restored);
            }
        }
    }

    // -----------------------------------------------------------------
    //  ToastChunk 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_toast_chunk_new() {
        let chunk = ToastChunk::new(10, 3, vec![1, 2, 3]);
        assert_eq!(chunk.toast_id, 10);
        assert_eq!(chunk.chunk_seq, 3);
        assert_eq!(chunk.size(), 3);
    }

    #[test]
    fn test_toast_chunk_size() {
        let chunk = ToastChunk::new(1, 0, vec![0; 100]);
        assert_eq!(chunk.size(), 100);
    }

    #[test]
    fn test_toast_chunk_is_full() {
        let full = ToastChunk::new(1, 0, vec![0; TOAST_CHUNK_SIZE]);
        assert!(full.is_full());

        let partial = ToastChunk::new(1, 0, vec![0; 100]);
        assert!(!partial.is_full());

        let empty = ToastChunk::new(1, 0, vec![]);
        assert!(!empty.is_full());
    }

    #[test]
    fn test_toast_chunk_eq() {
        let c1 = ToastChunk::new(1, 0, vec![1, 2]);
        let c2 = ToastChunk::new(1, 0, vec![1, 2]);
        let c3 = ToastChunk::new(1, 1, vec![1, 2]);
        assert_eq!(c1, c2);
        assert_ne!(c1, c3);
    }

    // -----------------------------------------------------------------
    //  ToastError 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_toast_error_field_too_large() {
        let err = ToastError::FieldTooLarge {
            size: 2_000_000_000,
            max: 1_073_741_824,
        };
        let s = format!("{err}");
        assert!(s.contains("too large"));
        assert!(s.contains("2000000000"));
    }

    #[test]
    fn test_toast_error_invalid_pointer_size() {
        let err = ToastError::InvalidPointerSize {
            expected: 8,
            actual: 4,
        };
        let s = format!("{err}");
        assert!(s.contains("pointer size"));
        assert!(s.contains("8"));
        assert!(s.contains("4"));
    }

    #[test]
    fn test_toast_error_toast_id_not_found() {
        let err = ToastError::ToastIdNotFound { toast_id: 42 };
        let s = format!("{err}");
        assert!(s.contains("42"));
        assert!(s.contains("not found"));
    }

    #[test]
    fn test_toast_error_chunk_sequence_broken() {
        let err = ToastError::ChunkSequenceBroken {
            toast_id: 10,
            expected_seq: 3,
            actual_seq: 5,
        };
        let s = format!("{err}");
        assert!(s.contains("sequence broken"));
        assert!(s.contains("3"));
        assert!(s.contains("5"));
    }

    #[test]
    fn test_toast_error_chunk_count_mismatch() {
        let err = ToastError::ChunkCountMismatch {
            toast_id: 10,
            expected: 5,
            actual: 3,
        };
        let s = format!("{err}");
        assert!(s.contains("count mismatch"));
        assert!(s.contains("5"));
        assert!(s.contains("3"));
    }

    // -----------------------------------------------------------------
    //  ToastStorage 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_toast_storage_new() {
        let storage = ToastStorage::new();
        assert_eq!(storage.field_count(), 0);
        assert_eq!(storage.total_chunks(), 0);
        assert_eq!(storage.total_bytes(), 0);
    }

    #[test]
    fn test_toast_storage_store_small_field() {
        let mut storage = ToastStorage::new();
        let data = vec![1, 2, 3, 4, 5];
        let chunk_count = storage.store_field(1, &data);

        assert_eq!(chunk_count, 1);
        assert!(storage.contains(1));
        assert_eq!(storage.chunk_count(1), 1);
        assert_eq!(storage.field_count(), 1);
        assert_eq!(storage.total_chunks(), 1);
        assert_eq!(storage.total_bytes(), 5);
    }

    #[test]
    fn test_toast_storage_store_large_field() {
        let mut storage = ToastStorage::new();
        let data = generate_large_field(TOAST_CHUNK_SIZE * 3 + 500); // 3.5 chunks
        let expected_chunks = count_chunks(data.len());
        let chunk_count = storage.store_field(1, &data);

        assert_eq!(chunk_count, expected_chunks);
        assert_eq!(chunk_count, 4);
        assert_eq!(storage.chunk_count(1), 4);
        assert_eq!(storage.total_chunks(), 4);
        assert_eq!(storage.total_bytes(), data.len() as u64);
    }

    #[test]
    fn test_toast_storage_store_empty_field() {
        let mut storage = ToastStorage::new();
        let chunk_count = storage.store_field(1, &[]);

        assert_eq!(chunk_count, 1);
        assert_eq!(storage.chunk_count(1), 1);
        assert_eq!(storage.total_bytes(), 0);
    }

    #[test]
    fn test_toast_storage_load_field() {
        let mut storage = ToastStorage::new();
        let data = generate_large_field(5000);
        let chunk_count = storage.store_field(1, &data);
        let pointer = ToastPointer::new(1, chunk_count);

        let loaded = storage.load_field(&pointer).unwrap();
        assert_eq!(loaded, data);
    }

    #[test]
    fn test_toast_storage_load_field_not_found() {
        let storage = ToastStorage::new();
        let pointer = ToastPointer::new(999, 1);
        let result = storage.load_field(&pointer);

        assert!(matches!(
            result,
            Err(ToastError::ToastIdNotFound { toast_id: 999 })
        ));
    }

    #[test]
    fn test_toast_storage_load_field_count_mismatch() {
        let mut storage = ToastStorage::new();
        let data = vec![1, 2, 3];
        let actual_count = storage.store_field(1, &data);
        let pointer = ToastPointer::new(1, actual_count + 1); // 错误的 chunk_count

        let result = storage.load_field(&pointer);
        assert!(matches!(
            result,
            Err(ToastError::ChunkCountMismatch {
                toast_id: 1,
                expected: _,
                actual: _
            })
        ));
    }

    #[test]
    fn test_toast_storage_delete_field() {
        let mut storage = ToastStorage::new();
        let data = generate_large_field(5000);
        storage.store_field(1, &data);
        assert!(storage.contains(1));

        let deleted = storage.delete_field(1);
        assert_eq!(deleted, 3); // 5000 / 2048 = 2 full + 1 partial = 3 chunks
        assert!(!storage.contains(1));
        assert_eq!(storage.field_count(), 0);
        assert_eq!(storage.total_chunks(), 0);
        assert_eq!(storage.total_bytes(), 0);
    }

    #[test]
    fn test_toast_storage_delete_nonexistent() {
        let mut storage = ToastStorage::new();
        let deleted = storage.delete_field(999);
        assert_eq!(deleted, 0);
    }

    #[test]
    fn test_toast_storage_overwrite_field() {
        let mut storage = ToastStorage::new();
        let data1 = generate_large_field(5000);
        let data2 = generate_large_field(3000);

        storage.store_field(1, &data1);
        let bytes_after_first = storage.total_bytes();

        storage.store_field(1, &data2);
        assert_eq!(storage.chunk_count(1), 2); // 3000 / 2048 = 2 chunks
        assert!(storage.total_bytes() < bytes_after_first);

        let pointer = ToastPointer::new(1, 2);
        let loaded = storage.load_field(&pointer).unwrap();
        assert_eq!(loaded, data2);
    }

    #[test]
    fn test_toast_storage_multiple_fields() {
        let mut storage = ToastStorage::new();
        let data1 = generate_large_field(1000);
        let data2 = generate_large_field(5000);
        let data3 = generate_large_field(10000);

        let c1 = storage.store_field(1, &data1);
        let c2 = storage.store_field(2, &data2);
        let c3 = storage.store_field(3, &data3);

        assert_eq!(storage.field_count(), 3);
        assert_eq!(storage.total_chunks(), c1 as u64 + c2 as u64 + c3 as u64);

        let p1 = ToastPointer::new(1, c1);
        let p2 = ToastPointer::new(2, c2);
        let p3 = ToastPointer::new(3, c3);

        assert_eq!(storage.load_field(&p1).unwrap(), data1);
        assert_eq!(storage.load_field(&p2).unwrap(), data2);
        assert_eq!(storage.load_field(&p3).unwrap(), data3);
    }

    #[test]
    fn test_toast_storage_clear() {
        let mut storage = ToastStorage::new();
        storage.store_field(1, &generate_large_field(5000));
        storage.store_field(2, &generate_large_field(3000));

        storage.clear();
        assert_eq!(storage.field_count(), 0);
        assert_eq!(storage.total_chunks(), 0);
        assert_eq!(storage.total_bytes(), 0);
    }

    #[test]
    fn test_toast_storage_contains() {
        let mut storage = ToastStorage::new();
        storage.store_field(1, &[1, 2, 3]);

        assert!(storage.contains(1));
        assert!(!storage.contains(2));
    }

    // -----------------------------------------------------------------
    //  ToastStats 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_toast_stats_default() {
        let stats = ToastStats::default();
        assert_eq!(stats.inline_count, 0);
        assert_eq!(stats.toasted_count, 0);
        assert_eq!(stats.total_count(), 0);
        assert_eq!(stats.toast_ratio(), 0.0);
        assert_eq!(stats.inline_saving_ratio(), 0.0);
    }

    #[test]
    fn test_toast_stats_total_count() {
        let mut stats = ToastStats::new();
        stats.inline_count = 30;
        stats.toasted_count = 70;
        assert_eq!(stats.total_count(), 100);
    }

    #[test]
    fn test_toast_stats_toast_ratio() {
        let mut stats = ToastStats::new();
        stats.inline_count = 30;
        stats.toasted_count = 70;
        let ratio = stats.toast_ratio();
        assert!((ratio - 0.7).abs() < 1e-9);
    }

    #[test]
    fn test_toast_stats_toast_ratio_zero() {
        let stats = ToastStats::new();
        assert_eq!(stats.toast_ratio(), 0.0);
    }

    #[test]
    fn test_toast_stats_inline_saving_ratio() {
        let mut stats = ToastStats::new();
        stats.inline_bytes = 1000;
        stats.toasted_bytes = 9000;
        stats.saved_inline_bytes = 9000 - 8; // 1 个 TOAST 字段，8 字节指针
        let ratio = stats.inline_saving_ratio();
        // saved / (inline + toasted) = (9000 - 8) / 10000
        assert!((ratio - (9000.0 - 8.0) / 10000.0).abs() < 1e-9);
    }

    #[test]
    fn test_toast_stats_inline_saving_ratio_zero() {
        let stats = ToastStats::new();
        assert_eq!(stats.inline_saving_ratio(), 0.0);
    }

    // -----------------------------------------------------------------
    //  ToastManager 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_toast_manager_new() {
        let manager = ToastManager::new();
        assert_eq!(manager.threshold(), TOAST_THRESHOLD);
        assert_eq!(manager.stats().total_count(), 0);
    }

    #[test]
    fn test_toast_manager_with_threshold() {
        let manager = ToastManager::with_threshold(1024);
        assert_eq!(manager.threshold(), 1024);
    }

    #[test]
    fn test_toast_manager_needs_toast() {
        let manager = ToastManager::new();
        assert!(!manager.needs_toast(0));
        assert!(!manager.needs_toast(TOAST_THRESHOLD));
        assert!(manager.needs_toast(TOAST_THRESHOLD + 1));
        assert!(manager.needs_toast(100_000));
    }

    #[test]
    fn test_toast_manager_maybe_toast_inline() {
        let mut manager = ToastManager::new();
        let data = vec![1, 2, 3, 4, 5];
        let value = manager.maybe_toast(data.clone()).unwrap();

        assert!(value.is_inline());
        assert_eq!(value.inline_data(), Some(&data[..]));
        assert_eq!(manager.stats().inline_count, 1);
        assert_eq!(manager.stats().toasted_count, 0);
        assert_eq!(manager.stats().inline_bytes, 5);
    }

    #[test]
    fn test_toast_manager_maybe_toast_toasted() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(10_000); // > 3KB
        let value = manager.maybe_toast(data.clone()).unwrap();

        assert!(value.is_toasted());
        let pointer = value.pointer().unwrap();
        assert_eq!(pointer.chunk_count, 5); // 10000 / 2048 = 4 full + 1 partial = 5
        assert_eq!(pointer.toast_id, 1);

        assert_eq!(manager.stats().inline_count, 0);
        assert_eq!(manager.stats().toasted_count, 1);
        assert_eq!(manager.stats().toasted_bytes, 10_000);
        assert_eq!(manager.stats().saved_inline_bytes, 10_000 - 8);
    }

    #[test]
    fn test_toast_manager_maybe_toast_boundary_at_threshold() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(TOAST_THRESHOLD); // 恰好 3KB
        let value = manager.maybe_toast(data).unwrap();
        assert!(value.is_inline()); // 恰好等于阈值 → 行内
    }

    #[test]
    fn test_toast_manager_maybe_toast_boundary_above_threshold() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(TOAST_THRESHOLD + 1); // 3KB + 1
        let value = manager.maybe_toast(data).unwrap();
        assert!(value.is_toasted()); // 超过阈值 → TOAST
    }

    #[test]
    fn test_toast_manager_maybe_toast_empty() {
        let mut manager = ToastManager::new();
        let value = manager.maybe_toast(Vec::new()).unwrap();
        assert!(value.is_inline());
        assert!(value.inline_data().unwrap().is_empty());
    }

    #[test]
    fn test_toast_manager_maybe_toast_field_too_large() {
        let mut manager = ToastManager::new();
        let huge = vec![0u8; TOAST_MAX_FIELD_SIZE + 1];
        let result = manager.maybe_toast(huge);
        assert!(matches!(result, Err(ToastError::FieldTooLarge { .. })));
    }

    #[test]
    fn test_toast_manager_detoast_inline() {
        let mut manager = ToastManager::new();
        let data = vec![1, 2, 3];
        let value = manager.maybe_toast(data.clone()).unwrap();

        let detoasted = manager.detoast(&value).unwrap();
        assert_eq!(detoasted, data);
        assert_eq!(manager.stats().detoast_count, 0); // Inline 不计 detoast
    }

    #[test]
    fn test_toast_manager_detoast_toasted() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(10_000);
        let value = manager.maybe_toast(data.clone()).unwrap();

        let detoasted = manager.detoast(&value).unwrap();
        assert_eq!(detoasted, data);
        assert_eq!(manager.stats().detoast_count, 1);
    }

    #[test]
    fn test_toast_manager_detoast_many_times() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(8_000);
        let value = manager.maybe_toast(data.clone()).unwrap();

        for _ in 0..10 {
            let detoasted = manager.detoast(&value).unwrap();
            assert_eq!(detoasted, data);
        }
        assert_eq!(manager.stats().detoast_count, 10);
    }

    #[test]
    fn test_toast_manager_delete_inline() {
        let mut manager = ToastManager::new();
        let value = manager.maybe_toast(vec![1, 2, 3]).unwrap();
        let deleted = manager.delete(&value);
        assert_eq!(deleted, 0); // Inline 不删 chunks
    }

    #[test]
    fn test_toast_manager_delete_toasted() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(5_000);
        let value = manager.maybe_toast(data).unwrap();
        let expected_chunks = value.pointer().unwrap().chunk_count as usize;

        let deleted = manager.delete(&value);
        assert_eq!(deleted, expected_chunks);
        assert_eq!(manager.stats().delete_count, 1);
        assert_eq!(manager.storage().field_count(), 0);
    }

    #[test]
    fn test_toast_manager_toast_id_incremental() {
        let mut manager = ToastManager::new();
        let v1 = manager.maybe_toast(generate_large_field(5_000)).unwrap();
        let v2 = manager.maybe_toast(generate_large_field(5_000)).unwrap();
        let v3 = manager.maybe_toast(generate_large_field(5_000)).unwrap();

        assert_eq!(v1.pointer().unwrap().toast_id, 1);
        assert_eq!(v2.pointer().unwrap().toast_id, 2);
        assert_eq!(v3.pointer().unwrap().toast_id, 3);
    }

    #[test]
    fn test_toast_manager_reset_stats() {
        let mut manager = ToastManager::new();
        manager.maybe_toast(vec![1, 2, 3]).unwrap();
        manager.maybe_toast(generate_large_field(5_000)).unwrap();
        assert!(manager.stats().total_count() > 0);

        manager.reset_stats();
        assert_eq!(manager.stats().total_count(), 0);
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_split_into_chunks_empty() {
        let chunks = split_into_chunks(&[]);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].is_empty());
    }

    #[test]
    fn test_split_into_chunks_single() {
        let data = vec![1, 2, 3];
        let chunks = split_into_chunks(&data);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], data);
    }

    #[test]
    fn test_split_into_chunks_exact_multiple() {
        let data = generate_large_field(TOAST_CHUNK_SIZE * 3);
        let chunks = split_into_chunks(&data);
        assert_eq!(chunks.len(), 3);
        for chunk in &chunks {
            assert_eq!(chunk.len(), TOAST_CHUNK_SIZE);
        }
    }

    #[test]
    fn test_split_into_chunks_partial_last() {
        let data = generate_large_field(TOAST_CHUNK_SIZE * 2 + 500);
        let chunks = split_into_chunks(&data);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), TOAST_CHUNK_SIZE);
        assert_eq!(chunks[1].len(), TOAST_CHUNK_SIZE);
        assert_eq!(chunks[2].len(), 500);
    }

    #[test]
    fn test_split_into_chunks_reconstruct() {
        let data = generate_large_field(10_000);
        let chunks = split_into_chunks(&data);
        let reconstructed: Vec<u8> = chunks.into_iter().flatten().collect();
        assert_eq!(reconstructed, data);
    }

    #[test]
    fn test_count_chunks() {
        assert_eq!(count_chunks(0), 1);
        assert_eq!(count_chunks(1), 1);
        assert_eq!(count_chunks(TOAST_CHUNK_SIZE), 1);
        assert_eq!(count_chunks(TOAST_CHUNK_SIZE + 1), 2);
        assert_eq!(count_chunks(TOAST_CHUNK_SIZE * 3), 3);
        assert_eq!(count_chunks(TOAST_CHUNK_SIZE * 3 + 1), 4);
    }

    #[test]
    fn test_count_chunks_matches_split() {
        for size in [0, 1, 100, 2048, 2049, 4096, 4097, 10000, 100000] {
            let expected = split_into_chunks(&generate_large_field(size)).len() as u32;
            assert_eq!(count_chunks(size), expected, "size={size}");
        }
    }

    #[test]
    fn test_generate_large_field() {
        let data = generate_large_field(100);
        assert_eq!(data.len(), 100);
        // 内容应为 [0, 1, 2, ..., 99]
        for (i, &b) in data.iter().enumerate() {
            assert_eq!(b, (i % 256) as u8);
        }
    }

    #[test]
    fn test_generate_large_field_seeded() {
        let data1 = generate_large_field_seeded(1000, 42);
        let data2 = generate_large_field_seeded(1000, 42);
        assert_eq!(data1, data2); // 相同 seed 应产生相同数据

        let data3 = generate_large_field_seeded(1000, 43);
        assert_ne!(data1, data3); // 不同 seed 应产生不同数据
    }

    #[test]
    fn test_validate_detoast_valid() {
        let original = generate_large_field(5000);
        let detoasted = original.clone();
        assert!(validate_detoast(&original, &detoasted));
    }

    #[test]
    fn test_validate_detoast_invalid_length() {
        let original = vec![1, 2, 3];
        let detoasted = vec![1, 2];
        assert!(!validate_detoast(&original, &detoasted));
    }

    #[test]
    fn test_validate_detoast_invalid_content() {
        let original = vec![1, 2, 3];
        let detoasted = vec![1, 2, 4];
        assert!(!validate_detoast(&original, &detoasted));
    }

    #[test]
    fn test_validate_detoast_empty() {
        assert!(validate_detoast(&[], &[]));
    }

    // -----------------------------------------------------------------
    //  集成测试
    // -----------------------------------------------------------------

    /// 集成测试：完整 TOAST 工作流（小字段 + 大字段混合）
    #[test]
    fn test_integration_full_workflow() {
        let mut manager = ToastManager::new();

        // 小字段 → 行内
        let small_data = vec![1, 2, 3, 4, 5];
        let small_value = manager.maybe_toast(small_data.clone()).unwrap();
        assert!(small_value.is_inline());
        assert_eq!(small_value.inline_size(), 5);

        // 大字段 → TOAST
        let large_data = generate_large_field(50_000);
        let large_value = manager.maybe_toast(large_data.clone()).unwrap();
        assert!(large_value.is_toasted());
        assert_eq!(large_value.inline_size(), 8); // 行内仅 8 字节指针

        // detoast 透明重组
        let small_back = manager.detoast(&small_value).unwrap();
        let large_back = manager.detoast(&large_value).unwrap();
        assert_eq!(small_back, small_data);
        assert_eq!(large_back, large_data);

        // 统计验证
        assert_eq!(manager.stats().inline_count, 1);
        assert_eq!(manager.stats().toasted_count, 1);
        assert_eq!(manager.stats().detoast_count, 1); // 仅 Toasted 计 detoast，Inline 不计
    }

    /// 集成测试：10KB ~ 1MB 大字段自动 TOAST
    #[test]
    fn test_integration_large_fields_10kb_to_1mb() {
        let mut manager = ToastManager::new();
        let sizes = [
            10 * 1024,   // 10KB
            50 * 1024,   // 50KB
            100 * 1024,  // 100KB
            500 * 1024,  // 500KB
            1024 * 1024, // 1MB
        ];

        let mut values = Vec::new();
        let mut originals = Vec::new();

        for (i, &size) in sizes.iter().enumerate() {
            let data = generate_large_field_seeded(size, i as u64);
            let value = manager.maybe_toast(data.clone()).unwrap();

            assert!(value.is_toasted(), "size {size} should be toasted");
            assert_eq!(
                value.inline_size(),
                8,
                "inline size should be 8 bytes for size {size}"
            );

            values.push(value);
            originals.push(data);
        }

        // 透明重组验证
        for (i, (value, original)) in values.iter().zip(originals.iter()).enumerate() {
            let detoasted = manager.detoast(value).unwrap();
            assert!(
                validate_detoast(original, &detoasted),
                "detoast failed for size {}",
                sizes[i]
            );
        }

        assert_eq!(manager.stats().toasted_count, sizes.len() as u64);
    }

    /// 集成测试：边界场景 — 恰好 3KB（行内）vs 3KB+1（TOAST）
    #[test]
    fn test_integration_boundary_3kb() {
        let mut manager = ToastManager::new();

        let exactly_threshold = generate_large_field(TOAST_THRESHOLD);
        let value_at = manager.maybe_toast(exactly_threshold.clone()).unwrap();
        assert!(value_at.is_inline());
        assert_eq!(value_at.inline_size(), TOAST_THRESHOLD);

        let just_above = generate_large_field(TOAST_THRESHOLD + 1);
        let value_above = manager.maybe_toast(just_above.clone()).unwrap();
        assert!(value_above.is_toasted());
        assert_eq!(value_above.inline_size(), 8);
    }

    /// 集成测试：100000 条混合大小字段
    #[test]
    fn test_integration_100000_mixed_fields() {
        let mut manager = ToastManager::new();
        let count: u64 = 100_000;
        let mut values = Vec::with_capacity(count as usize);

        for i in 0..count {
            // 交替写入小字段和大字段
            let data = if i % 2 == 0 {
                vec![i as u8; 100] // 小字段
            } else {
                generate_large_field_seeded(10 * 1024, i) // 10KB 大字段
            };
            let value = manager.maybe_toast(data).unwrap();
            values.push(value);
        }

        // 统计验证
        assert_eq!(manager.stats().total_count(), count);
        assert_eq!(manager.stats().inline_count, count / 2);
        assert_eq!(manager.stats().toasted_count, count / 2);

        // 抽样验证 detoast
        for i in [0u64, 1, 999, 1000, 99999] {
            let original = if i % 2 == 0 {
                vec![i as u8; 100]
            } else {
                generate_large_field_seeded(10 * 1024, i)
            };
            let detoasted = manager.detoast(&values[i as usize]).unwrap();
            assert_eq!(detoasted, original, "detoast mismatch at index {i}");
        }
    }

    /// 集成测试：行内仅存 8 字节指针（TOAST 字段）
    #[test]
    fn test_integration_inline_pointer_only_8_bytes() {
        let mut manager = ToastManager::new();
        let sizes = [3073, 5000, 10_000, 100_000, 1_000_000];

        for size in sizes {
            let data = generate_large_field(size);
            let value = manager.maybe_toast(data).unwrap();

            assert!(value.is_toasted());
            assert_eq!(
                value.inline_size(),
                8,
                "size {size} inline should be 8 bytes"
            );

            // 指针可序列化为 8 字节
            let pointer = value.pointer().unwrap();
            let bytes = pointer.to_bytes();
            assert_eq!(bytes.len(), 8);
        }
    }

    /// 集成测试：删除 TOAST 字段后存储清理
    #[test]
    fn test_integration_delete_clears_storage() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(50_000);
        let value = manager.maybe_toast(data).unwrap();

        let initial_chunks = manager.storage().total_chunks();
        assert!(initial_chunks > 0);

        let deleted = manager.delete(&value);
        assert_eq!(deleted as u64, initial_chunks);
        assert_eq!(manager.storage().total_chunks(), 0);
        assert_eq!(manager.storage().field_count(), 0);
    }

    /// 集成测试：指针序列化/反序列化往返
    #[test]
    fn test_integration_pointer_serialization_roundtrip() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(10_000);
        let value = manager.maybe_toast(data.clone()).unwrap();

        let pointer = *value.pointer().unwrap();
        let bytes = pointer.to_bytes();

        // 模拟行内存储 8 字节 → 反序列化 → detoast
        let restored_pointer = ToastPointer::from_bytes(&bytes).unwrap();
        let restored_value = ToastValue::Toasted(restored_pointer);
        let detoasted = manager.detoast(&restored_value).unwrap();

        assert_eq!(detoasted, data);
    }

    /// 集成测试：自定义阈值
    #[test]
    fn test_integration_custom_threshold() {
        let mut manager = ToastManager::with_threshold(1024); // 1KB 阈值

        let small = vec![0; 1024]; // 恰好 1KB
        let medium = vec![0; 1025]; // 1KB + 1
        let large = vec![0; 10_000]; // 10KB

        let v1 = manager.maybe_toast(small).unwrap();
        let v2 = manager.maybe_toast(medium).unwrap();
        let v3 = manager.maybe_toast(large).unwrap();

        assert!(v1.is_inline());
        assert!(v2.is_toasted());
        assert!(v3.is_toasted());
    }

    /// 集成测试：TOAST 节省行内空间验证
    #[test]
    fn test_integration_space_saving() {
        let mut manager = ToastManager::new();

        // 写入 100 个 10KB 字段
        let field_size = 10_000usize;
        let count: u64 = 100;
        for i in 0..count {
            let data = generate_large_field_seeded(field_size, i);
            manager.maybe_toast(data).unwrap();
        }

        let stats = manager.stats();
        assert_eq!(stats.toasted_count, count);

        // 原始总大小
        let original_total = field_size as u64 * count;

        // 行内大小（仅指针）
        let inline_total = count * TOAST_POINTER_SIZE as u64;

        // 节省率
        let saving_ratio = 1.0 - (inline_total as f64 / original_total as f64);
        assert!(
            saving_ratio > 0.99,
            "saving ratio should be > 99%, got {saving_ratio}"
        );

        // saved_inline_bytes 统计正确
        let expected_saved = (field_size - TOAST_POINTER_SIZE) as u64 * count;
        assert_eq!(stats.saved_inline_bytes, expected_saved);
    }

    /// 集成测试：corruption 检测 — chunk_count 不匹配
    #[test]
    fn test_integration_corruption_chunk_count() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(10_000);
        let value = manager.maybe_toast(data).unwrap();
        let pointer = value.pointer().unwrap();

        // 伪造错误的 chunk_count
        let bad_pointer = ToastPointer::new(pointer.toast_id, pointer.chunk_count + 1);
        let bad_value = ToastValue::Toasted(bad_pointer);

        let result = manager.detoast(&bad_value);
        assert!(matches!(result, Err(ToastError::ChunkCountMismatch { .. })));
    }

    /// 集成测试：corruption 检测 — toast_id 不存在
    #[test]
    fn test_integration_corruption_missing_toast_id() {
        let mut manager = ToastManager::new();
        let bad_pointer = ToastPointer::new(9999, 1);
        let bad_value = ToastValue::Toasted(bad_pointer);

        let result = manager.detoast(&bad_value);
        assert!(matches!(
            result,
            Err(ToastError::ToastIdNotFound { toast_id: 9999 })
        ));
    }

    /// 集成测试：完整生命周期（写入 → 读取 → 删除）
    #[test]
    fn test_integration_lifecycle() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(20_000);

        // 写入
        let value = manager.maybe_toast(data.clone()).unwrap();
        assert!(value.is_toasted());

        // 读取
        let detoasted = manager.detoast(&value).unwrap();
        assert_eq!(detoasted, data);

        // 删除
        let deleted = manager.delete(&value);
        assert!(deleted > 0);

        // 再读应失败
        let result = manager.detoast(&value);
        assert!(matches!(result, Err(ToastError::ToastIdNotFound { .. })));
    }

    /// 集成测试：多次 detoast 同一字段（缓存场景）
    #[test]
    fn test_integration_multiple_detoast_same_field() {
        let mut manager = ToastManager::new();
        let data = generate_large_field(50_000);
        let value = manager.maybe_toast(data.clone()).unwrap();

        // 连续 detoast 100 次，结果应一致
        for _ in 0..100 {
            let detoasted = manager.detoast(&value).unwrap();
            assert_eq!(detoasted, data);
        }
        assert_eq!(manager.stats().detoast_count, 100);
    }

    /// 大规模测试：100000 条 10KB~1MB 大字段（#[ignore] 默认跳过）
    #[test]
    #[ignore = "大规模测试：100000 条 10KB~1MB 大字段 TOAST"]
    fn test_integration_large_scale_100000_fields() {
        let mut manager = ToastManager::new();
        let count: u64 = 100_000;
        let sizes: Vec<usize> = (0..count)
            .map(|i| 10 * 1024 + ((i % 100) as usize) * 10 * 1024) // 10KB ~ 1MB
            .collect();

        let mut values = Vec::with_capacity(count as usize);
        let mut originals = Vec::with_capacity(count as usize);

        for (i, &size) in sizes.iter().enumerate() {
            let data = generate_large_field_seeded(size, i as u64);
            let value = manager.maybe_toast(data.clone()).unwrap();
            assert!(value.is_toasted());
            assert_eq!(value.inline_size(), 8);
            values.push(value);
            originals.push(data);
        }

        // 全部 detoast 验证
        for (i, (value, original)) in values.iter().zip(originals.iter()).enumerate() {
            let detoasted = manager.detoast(value).unwrap();
            assert_eq!(detoasted, *original, "mismatch at index {i}");
        }

        // 统计验证
        assert_eq!(manager.stats().toasted_count, count);
        assert_eq!(manager.stats().detoast_count, count);
    }
}
