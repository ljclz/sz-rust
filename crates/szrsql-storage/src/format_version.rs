//! 存储格式版本号定义 — Phase 7e.1
//!
//! 对应 `SzRSQL实施进度.md` Phase 7e.1。
//!
//! # 文件头布局（22 字节，小端）
//!
//! ```text
//! Offset  Size  Field
//! 0       4     magic            (u32 LE) = 0x42445A53 (磁盘字节为 "SZDB")
//! 4       2     format_version   (u16 LE)
//! 6       2     flags            (u16 LE)
//! 8       8     created_at       (u64 LE, Unix 毫秒)
//! 16      4     page_size        (u32 LE)
//! 20      2     reserved         (u16 LE, 必须为 0)
//! Total:  22 bytes
//! ```
//!
//! # 版本检查规则
//!
//! 启动时读取 `.szdb` 文件头部 `format_version`：
//! - 低于 `MIN_SUPPORTED_VERSION`：拒绝启动，提示升级数据文件。
//! - 高于 `CURRENT_VERSION`：拒绝启动，提示升级 SzRSQL 二进制。
//! - 魔数不匹配：拒绝启动，明确报错。
//! - 文件头长度不足：拒绝启动，明确报错。

use serde::{Deserialize, Serialize};

// =====================================================================
//  常量
// =====================================================================

/// 文件魔数：磁盘字节为 ASCII "SZDB"，以小端 u32 表示为 `0x42445A53`
pub const FILE_MAGIC: u32 = 0x4244_5A53;

/// 文件头大小：22 字节
pub const FILE_HEADER_SIZE: usize = 22;

/// 当前格式版本号
pub const CURRENT_VERSION: u16 = 4;

/// 最低支持的格式版本号（低于此值拒绝启动）
pub const MIN_SUPPORTED_VERSION: u16 = 1;

/// 最高支持的格式版本号（高于此值拒绝启动）
pub const MAX_SUPPORTED_VERSION: u16 = CURRENT_VERSION;

/// 标志位：无标志
pub const FILE_FLAG_NONE: u16 = 0x0000;

/// 标志位：大端序（默认小端）
pub const FILE_FLAG_BIG_ENDIAN: u16 = 0x0001;

/// 标志位：文件已加密
pub const FILE_FLAG_ENCRYPTED: u16 = 0x0002;

/// 标志位：文件已压缩
pub const FILE_FLAG_COMPRESSED: u16 = 0x0004;

/// 魔数的 ASCII 表示，用于错误信息
pub const FILE_MAGIC_ASCII: &str = "SZDB";

// =====================================================================
//  VersionError — 版本错误类型
// =====================================================================

/// 版本/格式错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VersionError {
    /// 格式版本过低：需升级数据文件
    #[error(
        "format version {found} is too old: minimum supported is {min}, please upgrade your data file"
    )]
    VersionTooOld { found: u16, min: u16 },

    /// 格式版本过高：需升级 SzRSQL 二进制
    #[error(
        "format version {found} is too new: current supported is {current}, please upgrade SzRSQL binary"
    )]
    VersionTooNew { found: u16, current: u16 },

    /// 魔数不匹配：非 .szdb 文件
    #[error(
        "invalid file magic: expected 0x{expected:08X} ({expected_magic}), found 0x{found:08X}"
    )]
    InvalidMagic {
        expected: u32,
        found: u32,
        expected_magic: String,
    },

    /// 文件头过短：无法解析
    #[error("file header too short: expected >= {expected} bytes, got {actual} bytes")]
    HeaderTooShort { expected: usize, actual: usize },
}

// =====================================================================
//  FileHeader — 文件头（22 字节）
// =====================================================================

/// `.szdb` 文件头（22 字节，小端序）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHeader {
    /// 魔数，必须为 `FILE_MAGIC`
    pub magic: u32,
    /// 格式版本号
    pub format_version: u16,
    /// 标志位
    pub flags: u16,
    /// 创建时间戳（Unix 毫秒）
    pub created_at: u64,
    /// 页大小（字节）
    pub page_size: u32,
    /// 保留字段（必须为 0）
    pub reserved: u16,
}

impl FileHeader {
    /// 创建新文件头
    ///
    /// - `page_size`: 页大小（字节）
    /// - `created_at`: 创建时间戳（Unix 毫秒）
    pub fn new(page_size: u32, created_at: u64) -> Self {
        Self {
            magic: FILE_MAGIC,
            format_version: CURRENT_VERSION,
            flags: FILE_FLAG_NONE,
            created_at,
            page_size,
            reserved: 0,
        }
    }

    /// 创建当前版本文件头（使用默认页大小 8192 和时间戳 0）
    pub fn current() -> Self {
        Self::new(8192, 0)
    }

    /// 设置格式版本号（链式）
    pub fn with_version(mut self, version: u16) -> Self {
        self.format_version = version;
        self
    }

    /// 设置标志位（链式）
    pub fn with_flags(mut self, flags: u16) -> Self {
        self.flags = flags;
        self
    }

    /// 设置创建时间戳（链式）
    pub fn with_created_at(mut self, ts: u64) -> Self {
        self.created_at = ts;
        self
    }

    /// 设置页大小（链式）
    pub fn with_page_size(mut self, ps: u32) -> Self {
        self.page_size = ps;
        self
    }

    /// 编码到 22 字节数组（小端序）
    pub fn to_bytes(&self) -> [u8; FILE_HEADER_SIZE] {
        let mut buf = [0u8; FILE_HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.magic.to_le_bytes());
        buf[4..6].copy_from_slice(&self.format_version.to_le_bytes());
        buf[6..8].copy_from_slice(&self.flags.to_le_bytes());
        buf[8..16].copy_from_slice(&self.created_at.to_le_bytes());
        buf[16..20].copy_from_slice(&self.page_size.to_le_bytes());
        buf[20..22].copy_from_slice(&self.reserved.to_le_bytes());
        buf
    }

    /// 从字节切片解析文件头
    ///
    /// 要求 `data.len() >= FILE_HEADER_SIZE`，否则返回 `HeaderTooShort`。
    /// 解析后需调用 `validate()` 校验魔数和版本。
    pub fn from_bytes(data: &[u8]) -> Result<Self, VersionError> {
        if data.len() < FILE_HEADER_SIZE {
            return Err(VersionError::HeaderTooShort {
                expected: FILE_HEADER_SIZE,
                actual: data.len(),
            });
        }
        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let format_version = u16::from_le_bytes([data[4], data[5]]);
        let flags = u16::from_le_bytes([data[6], data[7]]);
        let created_at = u64::from_le_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]);
        let page_size = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);
        let reserved = u16::from_le_bytes([data[20], data[21]]);
        Ok(Self {
            magic,
            format_version,
            flags,
            created_at,
            page_size,
            reserved,
        })
    }

    /// 校验文件头：魔数 + 版本范围
    ///
    /// 校验顺序：先魔数，后版本。
    pub fn validate(&self) -> Result<(), VersionError> {
        if self.magic != FILE_MAGIC {
            return Err(VersionError::InvalidMagic {
                expected: FILE_MAGIC,
                found: self.magic,
                expected_magic: FILE_MAGIC_ASCII.to_string(),
            });
        }
        check_version(self.format_version)
    }
}

impl Default for FileHeader {
    fn default() -> Self {
        Self::current()
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 检查版本号是否在支持范围内
///
/// - `version < MIN_SUPPORTED_VERSION` → `VersionTooOld`
/// - `version > CURRENT_VERSION` → `VersionTooNew`
/// - 否则 → `Ok(())`
pub fn check_version(version: u16) -> Result<(), VersionError> {
    if version < MIN_SUPPORTED_VERSION {
        Err(VersionError::VersionTooOld {
            found: version,
            min: MIN_SUPPORTED_VERSION,
        })
    } else if version > CURRENT_VERSION {
        Err(VersionError::VersionTooNew {
            found: version,
            current: CURRENT_VERSION,
        })
    } else {
        Ok(())
    }
}

/// 解析并校验文件头（一步到位）
///
/// 等价于 `FileHeader::from_bytes(data)?.validate()`，
/// 但 `validate()` 返回的是已解析的 `FileHeader`。
pub fn parse_and_validate(data: &[u8]) -> Result<FileHeader, VersionError> {
    let header = FileHeader::from_bytes(data)?;
    header.validate()?;
    Ok(header)
}

/// 返回版本号的描述信息
pub fn version_description(version: u16) -> &'static str {
    match version {
        1 => "v1: initial format (basic page layout, no checksum)",
        2 => "v2: added CRC32C checksum to page header",
        3 => "v3: added TOAST support for large tuples",
        4 => "v4: current format (full feature set, encryption/compression flags)",
        _ => "unknown version",
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  常量测试
    // -----------------------------------------------------------------

    #[test]
    fn file_magic_value() {
        // 磁盘字节（小端）应为 ASCII "SZDB"
        assert_eq!(FILE_MAGIC, 0x4244_5A53);
        assert_eq!(&FILE_MAGIC.to_le_bytes(), b"SZDB");
        assert_eq!(u32::from_le_bytes(*b"SZDB"), FILE_MAGIC);
    }

    #[test]
    fn header_size_is_22() {
        assert_eq!(FILE_HEADER_SIZE, 22);
        assert_eq!(FILE_HEADER_SIZE, 4 + 2 + 2 + 8 + 4 + 2);
    }

    #[test]
    fn version_constants_consistent() {
        assert_eq!(MIN_SUPPORTED_VERSION, 1);
        assert_eq!(MAX_SUPPORTED_VERSION, CURRENT_VERSION);
    }

    // -----------------------------------------------------------------
    //  check_version 测试
    // -----------------------------------------------------------------

    #[test]
    fn check_version_current_ok() {
        assert!(check_version(CURRENT_VERSION).is_ok());
    }

    #[test]
    fn check_version_min_ok() {
        assert!(check_version(MIN_SUPPORTED_VERSION).is_ok());
    }

    #[test]
    fn check_version_too_old() {
        let err = check_version(0).unwrap_err();
        assert!(matches!(
            err,
            VersionError::VersionTooOld {
                found: 0,
                min: MIN_SUPPORTED_VERSION
            }
        ));
    }

    #[test]
    fn check_version_too_new() {
        let err = check_version(CURRENT_VERSION + 1).unwrap_err();
        match err {
            VersionError::VersionTooNew { found, current } => {
                assert_eq!(found, CURRENT_VERSION + 1);
                assert_eq!(current, CURRENT_VERSION);
            }
            other => panic!("expected VersionTooNew, got {:?}", other),
        }
    }

    #[test]
    fn check_version_boundary_min_minus_one() {
        assert!(matches!(
            check_version(MIN_SUPPORTED_VERSION - 1),
            Err(VersionError::VersionTooOld { .. })
        ));
    }

    #[test]
    fn check_version_boundary_current_plus_one() {
        assert!(matches!(
            check_version(CURRENT_VERSION + 1),
            Err(VersionError::VersionTooNew { .. })
        ));
    }

    #[test]
    fn check_version_all_valid_pass() {
        for v in MIN_SUPPORTED_VERSION..=CURRENT_VERSION {
            assert!(check_version(v).is_ok(), "version {} should be valid", v);
        }
    }

    // -----------------------------------------------------------------
    //  FileHeader 构造测试
    // -----------------------------------------------------------------

    #[test]
    fn file_header_new() {
        let hdr = FileHeader::new(4096, 1234567890);
        assert_eq!(hdr.magic, FILE_MAGIC);
        assert_eq!(hdr.format_version, CURRENT_VERSION);
        assert_eq!(hdr.flags, FILE_FLAG_NONE);
        assert_eq!(hdr.created_at, 1234567890);
        assert_eq!(hdr.page_size, 4096);
        assert_eq!(hdr.reserved, 0);
    }

    #[test]
    fn file_header_current() {
        let hdr = FileHeader::current();
        assert_eq!(hdr.magic, FILE_MAGIC);
        assert_eq!(hdr.format_version, CURRENT_VERSION);
        assert_eq!(hdr.page_size, 8192);
        assert_eq!(hdr.created_at, 0);
        assert_eq!(hdr.reserved, 0);
    }

    #[test]
    fn file_header_with_version() {
        let hdr = FileHeader::current().with_version(2);
        assert_eq!(hdr.format_version, 2);
    }

    #[test]
    fn file_header_with_flags() {
        let hdr = FileHeader::current().with_flags(FILE_FLAG_ENCRYPTED | FILE_FLAG_COMPRESSED);
        assert_eq!(hdr.flags, FILE_FLAG_ENCRYPTED | FILE_FLAG_COMPRESSED);
    }

    #[test]
    fn file_header_with_created_at() {
        let hdr = FileHeader::current().with_created_at(99999);
        assert_eq!(hdr.created_at, 99999);
    }

    #[test]
    fn file_header_with_page_size() {
        let hdr = FileHeader::current().with_page_size(16384);
        assert_eq!(hdr.page_size, 16384);
    }

    #[test]
    fn file_header_default_is_current() {
        let default_hdr = FileHeader::default();
        let current_hdr = FileHeader::current();
        assert_eq!(default_hdr, current_hdr);
    }

    // -----------------------------------------------------------------
    //  序列化/反序列化测试
    // -----------------------------------------------------------------

    #[test]
    fn to_bytes_length_is_22() {
        let hdr = FileHeader::current();
        let bytes = hdr.to_bytes();
        assert_eq!(bytes.len(), FILE_HEADER_SIZE);
    }

    #[test]
    fn roundtrip_current_version() {
        let hdr = FileHeader::new(8192, 1_700_000_000_000).with_flags(FILE_FLAG_COMPRESSED);
        let bytes = hdr.to_bytes();
        let back = FileHeader::from_bytes(&bytes).unwrap();
        assert_eq!(hdr, back);
    }

    #[test]
    fn from_bytes_too_short() {
        let data = [0u8; FILE_HEADER_SIZE - 1];
        let err = FileHeader::from_bytes(&data).unwrap_err();
        match err {
            VersionError::HeaderTooShort { expected, actual } => {
                assert_eq!(expected, FILE_HEADER_SIZE);
                assert_eq!(actual, FILE_HEADER_SIZE - 1);
            }
            other => panic!("expected HeaderTooShort, got {:?}", other),
        }
    }

    #[test]
    fn from_bytes_empty() {
        let data: [u8; 0] = [];
        let err = FileHeader::from_bytes(&data).unwrap_err();
        assert!(matches!(
            err,
            VersionError::HeaderTooShort {
                expected: FILE_HEADER_SIZE,
                actual: 0
            }
        ));
    }

    #[test]
    fn from_bytes_exact_size() {
        let hdr = FileHeader::current();
        let bytes = hdr.to_bytes();
        // 精确 22 字节应可解析
        let back = FileHeader::from_bytes(&bytes).unwrap();
        assert_eq!(hdr, back);
    }

    #[test]
    fn from_bytes_extra_data_ignored() {
        let hdr = FileHeader::current();
        let mut data = hdr.to_bytes().to_vec();
        data.extend_from_slice(&[0xFF; 100]); // 额外数据
        let back = FileHeader::from_bytes(&data).unwrap();
        assert_eq!(hdr, back);
    }

    #[test]
    fn magic_bytes_layout() {
        let hdr = FileHeader::current();
        let bytes = hdr.to_bytes();
        // offset 0..4 = magic (LE) = "SZDB"
        assert_eq!(&bytes[0..4], b"SZDB");
    }

    #[test]
    fn version_bytes_layout() {
        let hdr = FileHeader::current().with_version(3);
        let bytes = hdr.to_bytes();
        // offset 4..6 = format_version (LE) = 3
        assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 3);
    }

    // -----------------------------------------------------------------
    //  validate 测试
    // -----------------------------------------------------------------

    #[test]
    fn validate_current_ok() {
        let hdr = FileHeader::current();
        assert!(hdr.validate().is_ok());
    }

    #[test]
    fn validate_invalid_magic() {
        let mut hdr = FileHeader::current();
        hdr.magic = 0xDEAD_BEEF;
        let err = hdr.validate().unwrap_err();
        assert!(matches!(
            err,
            VersionError::InvalidMagic {
                expected: FILE_MAGIC,
                found: 0xDEAD_BEEF,
                ..
            }
        ));
    }

    #[test]
    fn validate_version_too_old() {
        let hdr = FileHeader::current().with_version(0);
        let err = hdr.validate().unwrap_err();
        assert!(matches!(err, VersionError::VersionTooOld { found: 0, .. }));
    }

    #[test]
    fn validate_version_too_new() {
        let hdr = FileHeader::current().with_version(CURRENT_VERSION + 1);
        let err = hdr.validate().unwrap_err();
        match err {
            VersionError::VersionTooNew { found, .. } => {
                assert_eq!(found, CURRENT_VERSION + 1);
            }
            other => panic!("expected VersionTooNew, got {:?}", other),
        }
    }

    #[test]
    fn validate_magic_checked_before_version() {
        // 魔数错 + 版本错 → 应先报魔数错
        let mut hdr = FileHeader::current();
        hdr.magic = 0x1234_5678;
        hdr.format_version = 0; // 也无效
        let err = hdr.validate().unwrap_err();
        assert!(matches!(err, VersionError::InvalidMagic { .. }));
    }

    // -----------------------------------------------------------------
    //  parse_and_validate 测试
    // -----------------------------------------------------------------

    #[test]
    fn parse_and_validate_ok() {
        let hdr = FileHeader::current();
        let bytes = hdr.to_bytes();
        let back = parse_and_validate(&bytes).unwrap();
        assert_eq!(hdr, back);
    }

    #[test]
    fn parse_and_validate_too_short() {
        let data = [0u8; 10];
        assert!(matches!(
            parse_and_validate(&data).unwrap_err(),
            VersionError::HeaderTooShort { .. }
        ));
    }

    #[test]
    fn parse_and_validate_bad_magic() {
        let mut hdr = FileHeader::current();
        hdr.magic = 0xFFFF_FFFF;
        let bytes = hdr.to_bytes();
        assert!(matches!(
            parse_and_validate(&bytes).unwrap_err(),
            VersionError::InvalidMagic { .. }
        ));
    }

    #[test]
    fn parse_and_validate_old_version() {
        let hdr = FileHeader::current().with_version(0);
        let bytes = hdr.to_bytes();
        assert!(matches!(
            parse_and_validate(&bytes).unwrap_err(),
            VersionError::VersionTooOld { .. }
        ));
    }

    #[test]
    fn parse_and_validate_new_version() {
        let hdr = FileHeader::current().with_version(CURRENT_VERSION + 1);
        let bytes = hdr.to_bytes();
        assert!(matches!(
            parse_and_validate(&bytes).unwrap_err(),
            VersionError::VersionTooNew { .. }
        ));
    }

    // -----------------------------------------------------------------
    //  version_description 测试
    // -----------------------------------------------------------------

    #[test]
    fn version_description_known() {
        assert!(!version_description(1).is_empty());
        assert!(!version_description(2).is_empty());
        assert!(!version_description(3).is_empty());
        assert!(!version_description(4).is_empty());
    }

    #[test]
    fn version_description_unknown() {
        assert_eq!(version_description(99), "unknown version");
        assert_eq!(version_description(0), "unknown version");
    }

    #[test]
    fn version_description_current() {
        let desc = version_description(CURRENT_VERSION);
        assert!(desc.contains("current"));
    }

    // -----------------------------------------------------------------
    //  集成测试
    // -----------------------------------------------------------------

    #[test]
    fn full_workflow_create_serialize_parse_validate() {
        let original = FileHeader::new(16384, 1_700_000_000_000)
            .with_flags(FILE_FLAG_ENCRYPTED)
            .with_version(CURRENT_VERSION);
        let bytes = original.to_bytes();
        assert_eq!(bytes.len(), FILE_HEADER_SIZE);

        let parsed = FileHeader::from_bytes(&bytes).unwrap();
        assert_eq!(original, parsed);

        parsed.validate().unwrap();
    }

    #[test]
    fn all_supported_versions_roundtrip() {
        for v in MIN_SUPPORTED_VERSION..=CURRENT_VERSION {
            let hdr = FileHeader::current().with_version(v);
            let bytes = hdr.to_bytes();
            let back = parse_and_validate(&bytes).unwrap();
            assert_eq!(back.format_version, v);
        }
    }

    #[test]
    fn error_messages_descriptive() {
        let old_err = check_version(0).unwrap_err().to_string();
        assert!(old_err.contains("too old"));
        assert!(old_err.contains("upgrade your data file"));

        let new_err = check_version(CURRENT_VERSION + 1).unwrap_err().to_string();
        assert!(new_err.contains("too new"));
        assert!(new_err.contains("upgrade SzRSQL binary"));

        let magic_err = {
            let mut h = FileHeader::current();
            h.magic = 0;
            h.validate().unwrap_err().to_string()
        };
        assert!(magic_err.contains("invalid file magic"));
        assert!(magic_err.contains("SZDB"));

        let short_err = FileHeader::from_bytes(&[0u8; 5]).unwrap_err().to_string();
        assert!(short_err.contains("too short"));
        assert!(short_err.contains("22"));
    }

    #[test]
    fn flag_constants_distinct() {
        assert_ne!(FILE_FLAG_NONE, FILE_FLAG_BIG_ENDIAN);
        assert_ne!(FILE_FLAG_NONE, FILE_FLAG_ENCRYPTED);
        assert_ne!(FILE_FLAG_NONE, FILE_FLAG_COMPRESSED);
        assert_ne!(FILE_FLAG_BIG_ENDIAN, FILE_FLAG_ENCRYPTED);
        assert_ne!(FILE_FLAG_BIG_ENDIAN, FILE_FLAG_COMPRESSED);
        assert_ne!(FILE_FLAG_ENCRYPTED, FILE_FLAG_COMPRESSED);
    }

    #[test]
    fn header_with_all_flags_roundtrip() {
        let all_flags = FILE_FLAG_BIG_ENDIAN | FILE_FLAG_ENCRYPTED | FILE_FLAG_COMPRESSED;
        let hdr = FileHeader::current().with_flags(all_flags);
        let bytes = hdr.to_bytes();
        let back = parse_and_validate(&bytes).unwrap();
        assert_eq!(back.flags, all_flags);
    }

    #[test]
    fn reserved_field_zero_on_new() {
        let hdr = FileHeader::current();
        assert_eq!(hdr.reserved, 0);
        let bytes = hdr.to_bytes();
        // offset 20..22 = reserved (LE) = 0
        assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 0);
    }
}
