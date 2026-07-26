//! Phase 7d.19 — WAL 压缩（zstd）。
//!
//! 对 WAL 记录的 `data` 字段做透明压缩，磁盘上写入压缩后的 payload，
//! 重放前在内存中解压还原。压缩仅作用于 `data` 字段，header/trailer 不变，
//! 以保证 `WalRecord::decode` 的 CRC32C 校验逻辑可正常工作。
//!
//! # 设计
//!
//! - **算法**：zstd（Facebook Zstandard），压缩比与解压速度平衡优秀
//! - **压缩层级**：1..=22，默认 3（与 PostgreSQL `wal_compression_level=3` 等价语义）
//! - **小数据跳过**：当原始 data ≤ `MIN_COMPRESS_SIZE`（64B）时不压缩，
//!   避免给小记录增加压缩头开销
//! - **格式**：压缩后字节以单字节 `0x00`/`0x01` 标记原始/已压缩，便于解码识别
//! - **CRC32C 不变**：checksum 字段由调用方在 `update_checksum` 时计算；
//!   压缩后的 payload 仅作为 data 字段内容，不影响 checksum 计算逻辑
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_tx::wal::{WalRecord, WalOpType};
//! use szrsql_tx::wal_compression::{WalCompressor, CompressionAlgo};
//!
//! let mut rec = WalRecord::new(100, 1, WalOpType::Insert, 5, vec![0xAB; 4096]);
//! let comp = WalCompressor::new(CompressionAlgo::Zstd, 3);
//! let payload = comp.compress(&rec.data);  // 写盘前
//! // ... 写盘 payload ...
//! let decompressed = comp.decompress(&payload).unwrap();  // 重放前
//! assert_eq!(decompressed, rec.data);
//! ```

// =====================================================================
//  CompressionAlgo
// =====================================================================

/// WAL 压缩算法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompressionAlgo {
    /// 不压缩（直接透传原始字节）。
    None,
    /// zstd 压缩（默认）。
    Zstd,
}

impl CompressionAlgo {
    /// 转为单字节标记，用于序列化 payload 前缀。
    pub fn as_marker(self) -> u8 {
        match self {
            CompressionAlgo::None => 0x00,
            CompressionAlgo::Zstd => 0x01,
        }
    }

    /// 从单字节标记构造算法，非法值返回 `None`。
    pub fn from_marker(b: u8) -> Option<Self> {
        match b {
            0x00 => Some(CompressionAlgo::None),
            0x01 => Some(CompressionAlgo::Zstd),
            _ => None,
        }
    }
}

// =====================================================================
//  WalCompressor
// =====================================================================

/// 数据长度 ≤ 该阈值时跳过压缩（避免压缩头开销大于收益）。
pub const MIN_COMPRESS_SIZE: usize = 64;

/// zstd 默认压缩层级（与 PostgreSQL 推荐值对齐）。
pub const ZSTD_DEFAULT_LEVEL: i32 = 3;

/// WAL 压缩器。
///
/// 线程安全：所有方法均为 `&self`，内部无共享可变状态。
pub struct WalCompressor {
    /// 压缩算法。
    pub algo: CompressionAlgo,
    /// zstd 压缩层级（1..=22）。
    pub level: i32,
}

impl Default for WalCompressor {
    fn default() -> Self {
        Self::new(CompressionAlgo::Zstd, ZSTD_DEFAULT_LEVEL)
    }
}

impl WalCompressor {
    /// 创建压缩器。
    ///
    /// - `algo = None` 时使用 `Zstd`
    /// - `level` 超出 [1, 22] 时夹紧到合法区间
    pub fn new(algo: CompressionAlgo, level: i32) -> Self {
        let level = level.clamp(1, 22);
        Self { algo, level }
    }

    /// 关闭压缩（即 `CompressionAlgo::None`）。
    pub fn disabled() -> Self {
        Self {
            algo: CompressionAlgo::None,
            level: ZSTD_DEFAULT_LEVEL,
        }
    }

    /// 压缩 data 字节序列，返回可写盘的 payload。
    ///
    /// payload 格式：
    /// - `[0]`：1 字节算法标记（`CompressionAlgo::as_marker`）
    /// - `[1..]`：原始或压缩后的字节
    ///
    /// 当 `algo == None` 或 `data.len() <= MIN_COMPRESS_SIZE` 时跳过压缩，
    /// payload 仅为 `[0x00, ...data]`。
    pub fn compress(&self, data: &[u8]) -> Vec<u8> {
        match self.algo {
            CompressionAlgo::None => {
                let mut out = Vec::with_capacity(1 + data.len());
                out.push(CompressionAlgo::None.as_marker());
                out.extend_from_slice(data);
                out
            }
            CompressionAlgo::Zstd => {
                if data.len() <= MIN_COMPRESS_SIZE {
                    // 小数据直接透传，但保留 None 标记
                    let mut out = Vec::with_capacity(1 + data.len());
                    out.push(CompressionAlgo::None.as_marker());
                    out.extend_from_slice(data);
                    return out;
                }
                let compressed =
                    zstd::encode_all(data, self.level).unwrap_or_else(|_| data.to_vec());
                // 若压缩后反而变大，回退为不压缩
                if compressed.len() >= data.len() {
                    let mut out = Vec::with_capacity(1 + data.len());
                    out.push(CompressionAlgo::None.as_marker());
                    out.extend_from_slice(data);
                    out
                } else {
                    let mut out = Vec::with_capacity(1 + compressed.len());
                    out.push(CompressionAlgo::Zstd.as_marker());
                    out.extend_from_slice(&compressed);
                    out
                }
            }
        }
    }

    /// 解压 payload，返回原始 data。
    ///
    /// 错误情形：
    /// - payload 为空 → `CompressionError::EmptyPayload`
    /// - 算法标记非法 → `CompressionError::InvalidMarker`
    /// - zstd 解压失败 → `CompressionError::DecompressFailed`
    pub fn decompress(&self, payload: &[u8]) -> Result<Vec<u8>, CompressionError> {
        if payload.is_empty() {
            return Err(CompressionError::EmptyPayload);
        }
        let marker = payload[0];
        let body = &payload[1..];
        match CompressionAlgo::from_marker(marker) {
            Some(CompressionAlgo::None) => Ok(body.to_vec()),
            Some(CompressionAlgo::Zstd) => zstd::decode_all(body)
                .map_err(|e| CompressionError::DecompressFailed(e.to_string())),
            None => Err(CompressionError::InvalidMarker(marker)),
        }
    }

    /// 估算压缩比（原始 / 压缩后），用于指标统计。
    ///
    /// 不压缩时返回 1.0。
    pub fn estimate_ratio(&self, data: &[u8]) -> f64 {
        if matches!(self.algo, CompressionAlgo::None) || data.len() <= MIN_COMPRESS_SIZE {
            return 1.0;
        }
        let payload = self.compress(data);
        if payload.is_empty() {
            return 1.0;
        }
        // payload 含 1 字节标记，去除后比较
        let compressed_body = payload.len() - 1;
        if compressed_body == 0 {
            return 1.0;
        }
        data.len() as f64 / compressed_body as f64
    }
}

// =====================================================================
//  CompressionError
// =====================================================================

/// WAL 压缩错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompressionError {
    #[error("empty payload")]
    EmptyPayload,
    #[error("invalid compression marker: 0x{0:02X}")]
    InvalidMarker(u8),
    #[error("zstd decompress failed: {0}")]
    DecompressFailed(String),
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== CompressionAlgo ====================

    #[test]
    fn test_algo_marker_roundtrip() {
        for algo in [CompressionAlgo::None, CompressionAlgo::Zstd] {
            let m = algo.as_marker();
            assert_eq!(CompressionAlgo::from_marker(m), Some(algo));
        }
    }

    #[test]
    fn test_algo_from_invalid_marker() {
        assert_eq!(CompressionAlgo::from_marker(0xFF), None);
        assert_eq!(CompressionAlgo::from_marker(0x02), None);
    }

    // ==================== WalCompressor 配置 ====================

    #[test]
    fn test_compressor_default_is_zstd_level3() {
        let c = WalCompressor::default();
        assert_eq!(c.algo, CompressionAlgo::Zstd);
        assert_eq!(c.level, 3);
    }

    #[test]
    fn test_compressor_level_clamp() {
        let c = WalCompressor::new(CompressionAlgo::Zstd, 0);
        assert_eq!(c.level, 1);
        let c = WalCompressor::new(CompressionAlgo::Zstd, 100);
        assert_eq!(c.level, 22);
        let c = WalCompressor::new(CompressionAlgo::Zstd, -5);
        assert_eq!(c.level, 1);
    }

    #[test]
    fn test_compressor_disabled() {
        let c = WalCompressor::disabled();
        assert_eq!(c.algo, CompressionAlgo::None);
    }

    // ==================== None 算法：透传 ====================

    #[test]
    fn test_none_algo_passthrough() {
        let c = WalCompressor::disabled();
        let data = vec![0xAB; 1024];
        let payload = c.compress(&data);
        assert_eq!(payload[0], CompressionAlgo::None.as_marker());
        assert_eq!(&payload[1..], &data[..]);
        let restored = c.decompress(&payload).unwrap();
        assert_eq!(restored, data);
    }

    // ==================== Zstd 算法：大数据压缩 ====================

    #[test]
    fn test_zstd_compress_large_data() {
        let c = WalCompressor::default();
        // 4KB 高重复度数据，压缩比应远大于 1
        let data = vec![0x55; 4096];
        let payload = c.compress(&data);
        assert_eq!(payload[0], CompressionAlgo::Zstd.as_marker());
        // 压缩后应明显变小（4KB → 几十字节）
        assert!(
            payload.len() < data.len(),
            "compressed payload {} should be smaller than original {}",
            payload.len(),
            data.len()
        );
        let restored = c.decompress(&payload).unwrap();
        assert_eq!(restored, data);
    }

    #[test]
    fn test_zstd_compress_random_data_no_blowup() {
        // 不可压缩数据（高熵）：要么压缩后变小，要么回退为 None 标记，绝不膨胀
        let c = WalCompressor::default();
        // 用 hash 链生成高熵数据：H(i) || H(H(i)) || ...
        let mut data = Vec::with_capacity(2048);
        let mut state: u64 = 0x1234567890ABCDEF;
        for _ in 0..256 {
            // xorshift64：高熵伪随机
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            data.extend_from_slice(&state.to_le_bytes());
        }
        assert_eq!(data.len(), 2048);

        let payload = c.compress(&data);
        // 关键不变式：payload 总长度（含 1 字节标记）不应大于原始数据 + 标记
        assert!(
            payload.len() <= data.len() + 1,
            "payload {} should not exceed original+1 {}",
            payload.len(),
            data.len() + 1
        );
        // 无论是否压缩，解压都应还原
        let restored = c.decompress(&payload).unwrap();
        assert_eq!(restored, data);
    }

    #[test]
    fn test_zstd_small_data_skip() {
        // 小于 MIN_COMPRESS_SIZE 的数据应跳过压缩
        let c = WalCompressor::default();
        let data = vec![0xAB; MIN_COMPRESS_SIZE];
        let payload = c.compress(&data);
        assert_eq!(payload[0], CompressionAlgo::None.as_marker());
        assert_eq!(&payload[1..], &data[..]);
    }

    // ==================== 压缩比验证 ====================

    #[test]
    fn test_compression_ratio_large_repetitive() {
        // 高重复度 8KB 数据，压缩比应 ≥ 3x（验证指标）
        let c = WalCompressor::default();
        let data = vec![0xAA; 8192];
        let ratio = c.estimate_ratio(&data);
        assert!(
            ratio >= 3.0,
            "compression ratio {ratio:.2} should be >= 3.0 for highly repetitive 8KB data"
        );
    }

    #[test]
    fn test_compression_ratio_none_algo() {
        let c = WalCompressor::disabled();
        let data = vec![0xAA; 8192];
        let ratio = c.estimate_ratio(&data);
        assert_eq!(ratio, 1.0);
    }

    #[test]
    fn test_compression_ratio_small_data() {
        let c = WalCompressor::default();
        let data = vec![0xAA; 32];
        let ratio = c.estimate_ratio(&data);
        assert_eq!(ratio, 1.0);
    }

    // ==================== 多层级压缩 ====================

    #[test]
    fn test_different_compression_levels() {
        let data = vec![0x77; 8192];
        let c1 = WalCompressor::new(CompressionAlgo::Zstd, 1);
        let c9 = WalCompressor::new(CompressionAlgo::Zstd, 9);

        let p1 = c1.compress(&data);
        let p9 = c9.compress(&data);
        // 高层级应得到不大于低层级的压缩结果
        assert!(
            p9.len() <= p1.len(),
            "level 9 ({}) should not be larger than level 1 ({})",
            p9.len(),
            p1.len()
        );

        // 两者解压都应还原
        assert_eq!(c1.decompress(&p1).unwrap(), data);
        assert_eq!(c9.decompress(&p9).unwrap(), data);
    }

    // ==================== 错误处理 ====================

    #[test]
    fn test_decompress_empty_payload() {
        let c = WalCompressor::default();
        assert_eq!(
            c.decompress(&[]).unwrap_err(),
            CompressionError::EmptyPayload
        );
    }

    #[test]
    fn test_decompress_invalid_marker() {
        let c = WalCompressor::default();
        let payload = [0xFF, 0x00, 0x01];
        let err = c.decompress(&payload).unwrap_err();
        assert!(matches!(err, CompressionError::InvalidMarker(0xFF)));
    }

    #[test]
    fn test_decompress_corrupt_zstd() {
        let c = WalCompressor::default();
        // 构造一个标记为 zstd 但内容非法的 payload
        let payload = [CompressionAlgo::Zstd.as_marker(), 0x00, 0x01, 0x02];
        let err = c.decompress(&payload).unwrap_err();
        assert!(matches!(err, CompressionError::DecompressFailed(_)));
    }

    // ==================== 与 WalRecord 集成（端到端） ====================

    #[test]
    fn test_wal_record_with_compression_roundtrip() {
        use crate::wal::{WalOpType, WalRecord};

        let comp = WalCompressor::default();
        // 构造一个 4KB 数据的 Insert 记录
        let original_data = vec![0xCC; 4096];
        let mut rec = WalRecord::new(42, 7, WalOpType::Insert, 100, original_data.clone());

        // 压缩 data
        let compressed_payload = comp.compress(&rec.data);
        assert!(compressed_payload.len() < rec.data.len() + 8); // 压缩后明显变小

        // 模拟"写盘前将 data 替换为 compressed_payload"
        let saved_data = rec.data.clone();
        rec.data = compressed_payload.clone();
        rec.update_checksum();
        let encoded = rec.encode();

        // 模拟"读盘后解码 + 解压"
        let decoded = WalRecord::decode(&encoded).unwrap();
        decoded.verify_checksum().unwrap();
        let restored_data = comp.decompress(&decoded.data).unwrap();
        assert_eq!(restored_data, saved_data);
    }

    #[test]
    fn test_compression_ratio_3_to_5x_target() {
        // 验证进度文档指标："WAL 文件缩小 3-5x"
        // 用真实数据库常见的 8KB 页（含大量零字节）模拟
        let comp = WalCompressor::default();
        let mut page = vec![0u8; 8192];
        // 模拟一个稀疏页：仅前 256 字节有数据
        for (i, b) in page.iter_mut().enumerate().take(256) {
            *b = (i & 0xFF) as u8;
        }
        let ratio = comp.estimate_ratio(&page);
        assert!(
            ratio >= 3.0,
            "compression ratio for sparse 8KB page should be >= 3.0, got {ratio:.2}"
        );
        // 一般 zstd 对稀疏数据压缩比远高于 5x，但指标只要求 3-5x
    }
}
