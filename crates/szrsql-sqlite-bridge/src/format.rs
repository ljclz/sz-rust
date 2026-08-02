//! SQLite 文件格式常量与头部编解码。
//!
//! 本模块实现 SQLite 数据库文件头部（前 100 字节）的编解码，
//! 完全遵循 [SQLite 文件格式规范](https://www.sqlite.org/fileformat2.html)。
//!
//! # 文件头部布局（100 字节）
//!
//! | 偏移 | 长度 | 字段 |
//! |------|------|------|
//! | 0 | 16 | 魔数 `"SQLite format 3\0"` |
//! | 16 | 2 | 页面大小（大端，必须为 2 的幂，512..=32768，或 1 表示 65536） |
//! | 18 | 1 | 文件格式写版本（1=legacy，2=WAL） |
//! | 19 | 1 | 文件格式读版本（1=legacy，2=WAL） |
//! | 20 | 1 | 每页末尾保留空间字节数 |
//! | 21 | 1 | 最大嵌入负载比例（必须为 64） |
//! | 22 | 1 | 最小嵌入负载比例（必须为 32） |
//! | 23 | 1 | 叶子负载比例（必须为 32） |
//! | 24 | 4 | 文件变更计数器 |
//! | 28 | 4 | 数据库大小（页数） |
//! | 32 | 4 | 第一个 freelist trunk 页页号 |
//! | 36 | 4 | freelist 总页数 |
//! | 40 | 4 | schema cookie |
//! | 44 | 4 | schema 格式版本（1..=4） |
//! | 48 | 4 | 默认页面缓存大小 |
//! | 52 | 4 | 最大根 b-tree 页页号（auto-vacuum） |
//! | 56 | 4 | 文本编码（1=UTF-8, 2=UTF-16le, 3=UTF-16be） |
//! | 60 | 4 | 用户版本 |
//! | 64 | 4 | 增量 vacuum 模式 |
//! | 68 | 4 | 应用 ID |
//! | 72 | 20 | 保留（必须为零） |
//! | 92 | 4 | version-valid-for |
//! | 96 | 4 | SQLITE_VERSION_NUMBER |

use thiserror::Error;

// =====================================================================
//  常量
// =====================================================================

/// SQLite 文件魔数头部（16 字节，含尾部 NUL）。
pub const MAGIC_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// 默认页面大小（4096 字节）。
pub const PAGE_SIZE_DEFAULT: u16 = 4096;

/// 文件头部长度（100 字节）。
pub const HEADER_SIZE: usize = 100;

/// 文本编码：UTF-8。
pub const TEXT_ENCODING_UTF8: u32 = 1;
/// 文本编码：UTF-16 小端序。
pub const TEXT_ENCODING_UTF16LE: u32 = 2;
/// 文本编码：UTF-16 大端序。
pub const TEXT_ENCODING_UTF16BE: u32 = 3;

/// schema 格式默认版本（4，支持 DESC 索引等最新特性）。
pub const SCHEMA_FORMAT_DEFAULT: u32 = 4;

// =====================================================================
//  错误类型
// =====================================================================

/// SQLite 文件格式错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SqliteFormatError {
    /// 输入数据长度不足 100 字节，无法容纳完整头部。
    #[error("buffer too short: {actual} bytes, need at least {expected} bytes")]
    BufferTooShort {
        /// 实际长度
        actual: usize,
        /// 期望长度
        expected: usize,
    },
    /// 魔数不匹配（不是合法的 SQLite 文件）。
    #[error("invalid magic header: expected {expected:?}, got {actual:?}")]
    InvalidMagic {
        /// 期望的魔数
        expected: Vec<u8>,
        /// 实际的魔数
        actual: Vec<u8>,
    },
    /// 页面大小不合法（必须为 2 的幂且在 512..=65536 范围内）。
    #[error("invalid page size: {0} (must be power of 2 in 512..=65536 or 1 for 65536)")]
    InvalidPageSize(u16),
    /// 文本编码不合法（必须为 1、2 或 3）。
    #[error("invalid text encoding: {0} (must be 1=UTF-8, 2=UTF-16le, 3=UTF-16be)")]
    InvalidTextEncoding(u32),
}

// =====================================================================
//  SqliteHeader 结构体
// =====================================================================

/// SQLite 数据库文件头部（100 字节）。
///
/// 字段命名与 [SQLite 文件格式规范](https://www.sqlite.org/fileformat2.html)
/// 保持一致，便于交叉参考。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteHeader {
    /// 页面大小（字节）。合法值：512、1024、...、32768，或 65536（用 1 表示）。
    pub page_size: u16,
    /// 文件格式写版本（1=legacy，2=WAL）。
    pub write_version: u8,
    /// 文件格式读版本（1=legacy，2=WAL）。
    pub read_version: u8,
    /// 每页末尾保留空间字节数（通常为 0）。
    pub reserved_space: u8,
    /// 文件变更计数器（每次事务提交时递增）。
    pub file_change_counter: u32,
    /// 数据库大小（页数，in-header database size）。
    pub db_size_pages: u32,
    /// 第一个 freelist trunk 页页号（0 表示无 freelist）。
    pub first_freelist_trunk: u32,
    /// freelist 总页数。
    pub freelist_pages: u32,
    /// schema cookie（schema 变更时递增）。
    pub schema_cookie: u32,
    /// schema 格式版本（1..=4）。
    pub schema_format: u32,
    /// 默认页面缓存大小。
    pub default_page_cache_size: u32,
    /// 最大根 b-tree 页页号（auto-vacuum 用，0 表示禁用）。
    pub largest_root_btree: u32,
    /// 文本编码（1=UTF-8, 2=UTF-16le, 3=UTF-16be）。
    pub text_encoding: u32,
    /// 用户版本。
    pub user_version: u32,
    /// 增量 vacuum 模式（0 表示禁用）。
    pub incremental_vacuum: u32,
    /// 应用 ID。
    pub application_id: u32,
    /// version-valid-for（与 file_change_counter 一致时表示数据库大小有效）。
    pub version_valid_for: u32,
    /// SQLITE_VERSION_NUMBER（编译 SQLite 时的版本号）。
    pub sqlite_version_number: u32,
}

impl SqliteHeader {
    /// 构造一个合法的默认 SQLite 文件头部。
    ///
    /// 默认值：
    /// - page_size = 4096
    /// - write/read_version = 1（legacy 模式）
    /// - reserved_space = 0
    /// - text_encoding = UTF-8
    /// - schema_format = 4
    /// - 其他计数器/页号均为 0
    pub fn new() -> Self {
        Self {
            page_size: PAGE_SIZE_DEFAULT,
            write_version: 1,
            read_version: 1,
            reserved_space: 0,
            file_change_counter: 0,
            db_size_pages: 0,
            first_freelist_trunk: 0,
            freelist_pages: 0,
            schema_cookie: 0,
            schema_format: SCHEMA_FORMAT_DEFAULT,
            default_page_cache_size: 0,
            largest_root_btree: 0,
            text_encoding: TEXT_ENCODING_UTF8,
            user_version: 0,
            incremental_vacuum: 0,
            application_id: 0,
            version_valid_for: 0,
            sqlite_version_number: 0,
        }
    }

    /// 编码为 100 字节的大端序字节数组。
    ///
    /// 输出长度严格等于 [`HEADER_SIZE`]（100），未使用的保留区
    /// （偏移 72..92）以零填充。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = vec![0u8; HEADER_SIZE];

        // 偏移 0..16：魔数
        buf[0..16].copy_from_slice(MAGIC_HEADER);

        // 偏移 16..18：页面大小（大端）
        buf[16..18].copy_from_slice(&self.page_size.to_be_bytes());

        // 偏移 18：写版本
        buf[18] = self.write_version;
        // 偏移 19：读版本
        buf[19] = self.read_version;
        // 偏移 20：保留空间
        buf[20] = self.reserved_space;
        // 偏移 21..24：负载比例常量（必须为 64/32/32）
        buf[21] = 64;
        buf[22] = 32;
        buf[23] = 32;

        // 偏移 24..28：文件变更计数器
        buf[24..28].copy_from_slice(&self.file_change_counter.to_be_bytes());
        // 偏移 28..32：数据库大小（页数）
        buf[28..32].copy_from_slice(&self.db_size_pages.to_be_bytes());
        // 偏移 32..36：第一个 freelist trunk 页
        buf[32..36].copy_from_slice(&self.first_freelist_trunk.to_be_bytes());
        // 偏移 36..40：freelist 总页数
        buf[36..40].copy_from_slice(&self.freelist_pages.to_be_bytes());
        // 偏移 40..44：schema cookie
        buf[40..44].copy_from_slice(&self.schema_cookie.to_be_bytes());
        // 偏移 44..48：schema 格式版本
        buf[44..48].copy_from_slice(&self.schema_format.to_be_bytes());
        // 偏移 48..52：默认页面缓存大小
        buf[48..52].copy_from_slice(&self.default_page_cache_size.to_be_bytes());
        // 偏移 52..56：最大根 b-tree 页
        buf[52..56].copy_from_slice(&self.largest_root_btree.to_be_bytes());
        // 偏移 56..60：文本编码
        buf[56..60].copy_from_slice(&self.text_encoding.to_be_bytes());
        // 偏移 60..64：用户版本
        buf[60..64].copy_from_slice(&self.user_version.to_be_bytes());
        // 偏移 64..68：增量 vacuum 模式
        buf[64..68].copy_from_slice(&self.incremental_vacuum.to_be_bytes());
        // 偏移 68..72：应用 ID
        buf[68..72].copy_from_slice(&self.application_id.to_be_bytes());

        // 偏移 72..92：保留区（20 字节），已由 vec![0; ...] 初始化为零

        // 偏移 92..96：version-valid-for
        buf[92..96].copy_from_slice(&self.version_valid_for.to_be_bytes());
        // 偏移 96..100：SQLITE_VERSION_NUMBER
        buf[96..100].copy_from_slice(&self.sqlite_version_number.to_be_bytes());

        buf
    }

    /// 从字节切片解码 SQLite 文件头部。
    ///
    /// # 参数
    /// - `bytes`：至少 100 字节的文件头部数据。
    ///
    /// # 错误
    /// - [`SqliteFormatError::BufferTooShort`]：输入不足 100 字节
    /// - [`SqliteFormatError::InvalidMagic`]：魔数不匹配
    /// - [`SqliteFormatError::InvalidPageSize`]：页面大小不合法
    /// - [`SqliteFormatError::InvalidTextEncoding`]：文本编码不合法
    pub fn decode(bytes: &[u8]) -> Result<Self, SqliteFormatError> {
        if bytes.len() < HEADER_SIZE {
            return Err(SqliteFormatError::BufferTooShort {
                actual: bytes.len(),
                expected: HEADER_SIZE,
            });
        }

        // 校验魔数
        if &bytes[0..16] != MAGIC_HEADER {
            return Err(SqliteFormatError::InvalidMagic {
                expected: MAGIC_HEADER.to_vec(),
                actual: bytes[0..16].to_vec(),
            });
        }

        // 读取页面大小（大端 u16）
        let page_size = u16::from_be_bytes([bytes[16], bytes[17]]);
        // 规范：值 1 表示 65536；其他值必须是 2 的幂且在 512..=32768 范围内
        if !is_valid_page_size(page_size) {
            return Err(SqliteFormatError::InvalidPageSize(page_size));
        }

        // 读取文本编码
        let text_encoding = u32::from_be_bytes([bytes[56], bytes[57], bytes[58], bytes[59]]);
        if !matches!(text_encoding, 1 | 2 | 3) {
            return Err(SqliteFormatError::InvalidTextEncoding(text_encoding));
        }

        Ok(Self {
            page_size,
            write_version: bytes[18],
            read_version: bytes[19],
            reserved_space: bytes[20],
            file_change_counter: u32::from_be_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            db_size_pages: u32::from_be_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
            first_freelist_trunk: u32::from_be_bytes([bytes[32], bytes[33], bytes[34], bytes[35]]),
            freelist_pages: u32::from_be_bytes([bytes[36], bytes[37], bytes[38], bytes[39]]),
            schema_cookie: u32::from_be_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]),
            schema_format: u32::from_be_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]),
            default_page_cache_size: u32::from_be_bytes([
                bytes[48], bytes[49], bytes[50], bytes[51],
            ]),
            largest_root_btree: u32::from_be_bytes([bytes[52], bytes[53], bytes[54], bytes[55]]),
            text_encoding,
            user_version: u32::from_be_bytes([bytes[60], bytes[61], bytes[62], bytes[63]]),
            incremental_vacuum: u32::from_be_bytes([bytes[64], bytes[65], bytes[66], bytes[67]]),
            application_id: u32::from_be_bytes([bytes[68], bytes[69], bytes[70], bytes[71]]),
            version_valid_for: u32::from_be_bytes([bytes[92], bytes[93], bytes[94], bytes[95]]),
            sqlite_version_number: u32::from_be_bytes([bytes[96], bytes[97], bytes[98], bytes[99]]),
        })
    }
}

impl Default for SqliteHeader {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 判断页面大小是否合法。
///
/// 合法值：1（表示 65536），或 2 的幂且在 512..=32768 范围内。
fn is_valid_page_size(page_size: u16) -> bool {
    match page_size {
        1 => true, // 表示 65536
        512 | 1024 | 2048 | 4096 | 8192 | 16384 | 32768 => true,
        _ => false,
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  常量正确性测试
    // -----------------------------------------------------------------

    #[test]
    fn magic_header_is_16_bytes_with_nul_terminator() {
        // 魔数必须是 16 字节，以 NUL 结尾
        assert_eq!(MAGIC_HEADER.len(), 16);
        assert_eq!(MAGIC_HEADER, b"SQLite format 3\0");
        // 最后一个字节必须是 NUL
        assert_eq!(MAGIC_HEADER[15], 0);
    }

    #[test]
    fn page_size_default_is_power_of_two() {
        // 默认页面大小必须是 2 的幂且在合法范围内
        assert_eq!(PAGE_SIZE_DEFAULT, 4096);
        assert!(PAGE_SIZE_DEFAULT.is_power_of_two());
        assert!((512..=32768).contains(&PAGE_SIZE_DEFAULT));
    }

    #[test]
    fn header_size_is_100() {
        // 头部固定为 100 字节
        assert_eq!(HEADER_SIZE, 100);
    }

    // -----------------------------------------------------------------
    //  编解码往返测试
    // -----------------------------------------------------------------

    #[test]
    fn encode_produces_exactly_100_bytes() {
        // 编码输出长度严格为 100
        let header = SqliteHeader::new();
        let bytes = header.encode();
        assert_eq!(bytes.len(), HEADER_SIZE);
    }

    #[test]
    fn encode_writes_magic_header_at_offset_0() {
        // 编码后前 16 字节必须是魔数
        let header = SqliteHeader::new();
        let bytes = header.encode();
        assert_eq!(&bytes[0..16], MAGIC_HEADER);
    }

    #[test]
    fn roundtrip_default_header_preserves_all_fields() {
        // 默认头部编解码往返：所有字段保持一致
        let original = SqliteHeader::new();
        let encoded = original.encode();
        let decoded = SqliteHeader::decode(&encoded).expect("decode should succeed");
        assert_eq!(original, decoded);
    }

    #[test]
    fn roundtrip_custom_header_preserves_all_fields() {
        // 自定义字段头部编解码往返
        let original = SqliteHeader {
            page_size: 8192,
            write_version: 2,
            read_version: 2,
            reserved_space: 0,
            file_change_counter: 42,
            db_size_pages: 100,
            first_freelist_trunk: 5,
            freelist_pages: 3,
            schema_cookie: 7,
            schema_format: 4,
            default_page_cache_size: 1000,
            largest_root_btree: 0,
            text_encoding: TEXT_ENCODING_UTF8,
            user_version: 0xDEAD_BEEF,
            incremental_vacuum: 0,
            application_id: 0xCAFE_F00D,
            version_valid_for: 42,
            sqlite_version_number: 3_040_001,
        };
        let encoded = original.encode();
        let decoded = SqliteHeader::decode(&encoded).expect("decode should succeed");
        assert_eq!(original, decoded);
    }

    // -----------------------------------------------------------------
    //  错误路径测试
    // -----------------------------------------------------------------

    #[test]
    fn decode_too_short_buffer_returns_error() {
        // 输入不足 100 字节应返回 BufferTooShort
        let short_buf = [0u8; 99];
        let result = SqliteHeader::decode(&short_buf);
        assert!(matches!(
            result,
            Err(SqliteFormatError::BufferTooShort {
                actual: 99,
                expected: 100
            })
        ));
    }

    #[test]
    fn decode_invalid_magic_returns_error() {
        // 魔数错误应返回 InvalidMagic
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[0..16].copy_from_slice(b"NOTSQLite format");
        let result = SqliteHeader::decode(&buf);
        assert!(matches!(
            result,
            Err(SqliteFormatError::InvalidMagic { .. })
        ));
    }

    #[test]
    fn decode_invalid_page_size_returns_error() {
        // 页面大小不合法（如 100，非 2 的幂）应返回 InvalidPageSize
        let mut buf = SqliteHeader::new().encode();
        // 把页面大小改成 100（非 2 的幂）
        buf[16..18].copy_from_slice(&100u16.to_be_bytes());
        let result = SqliteHeader::decode(&buf);
        assert!(matches!(
            result,
            Err(SqliteFormatError::InvalidPageSize(100))
        ));
    }

    #[test]
    fn decode_invalid_text_encoding_returns_error() {
        // 文本编码不合法（如 0 或 4）应返回 InvalidTextEncoding
        let mut buf = SqliteHeader::new().encode();
        // 把文本编码改成 0（不合法）
        buf[56..60].copy_from_slice(&0u32.to_be_bytes());
        let result = SqliteHeader::decode(&buf);
        assert!(matches!(
            result,
            Err(SqliteFormatError::InvalidTextEncoding(0))
        ));
    }

    #[test]
    fn decode_page_size_value_one_represents_65536() {
        // 值 1 是合法的（表示 65536）
        let mut buf = SqliteHeader::new().encode();
        buf[16..18].copy_from_slice(&1u16.to_be_bytes());
        let result = SqliteHeader::decode(&buf);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().page_size, 1);
    }

    #[test]
    fn decode_all_valid_page_sizes_succeed() {
        // 所有合法页面大小都应解码成功
        for &valid_ps in &[512u16, 1024, 2048, 4096, 8192, 16384, 32768, 1] {
            let mut buf = SqliteHeader::new().encode();
            buf[16..18].copy_from_slice(&valid_ps.to_be_bytes());
            let result = SqliteHeader::decode(&buf);
            assert!(
                result.is_ok(),
                "page size {valid_ps} should be valid, got: {:?}",
                result
            );
        }
    }

    // -----------------------------------------------------------------
    //  字段编码位置正确性测试
    // -----------------------------------------------------------------

    #[test]
    fn encode_writes_page_size_at_offset_16_big_endian() {
        // 验证页面大小写入偏移 16，大端序
        let mut header = SqliteHeader::new();
        header.page_size = 0x1234;
        let bytes = header.encode();
        assert_eq!(bytes[16], 0x12);
        assert_eq!(bytes[17], 0x34);
    }

    #[test]
    fn encode_writes_payload_fractions_constants() {
        // 偏移 21/22/23 必须为 64/32/32（SQLite 规范要求）
        let bytes = SqliteHeader::new().encode();
        assert_eq!(bytes[21], 64);
        assert_eq!(bytes[22], 32);
        assert_eq!(bytes[23], 32);
    }

    #[test]
    fn encode_writes_reserved_region_as_zeros() {
        // 偏移 72..92 的 20 字节保留区必须为零
        let bytes = SqliteHeader::new().encode();
        assert!(bytes[72..92].iter().all(|&b| b == 0));
    }
}
