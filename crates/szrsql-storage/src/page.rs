//! SzRSQL 页格式 — 对应 `SzRSQL技术实现方案.md` 9.2 节。
//!
//! 固定 8KB 页（8192 字节）= 48 字节 PageHeader + 8144 字节 body。
//! 使用 CRC32C 校验和保护页完整性。

use serde::{Deserialize, Serialize};
use tracing::{trace, warn};

// =====================================================================
//  常量
// =====================================================================

/// 页大小：固定 8KB
pub const PAGE_SIZE: usize = 8192;

/// 页头大小：固定 48 字节
pub const PAGE_HEADER_SIZE: usize = 48;

/// 页体大小：8192 - 48 = 8144 字节
pub const PAGE_BODY_SIZE: usize = PAGE_SIZE - PAGE_HEADER_SIZE;

// =====================================================================
//  标志位
// =====================================================================

pub const PAGE_FLAG_DIRTY: u16 = 0x0001;
pub const PAGE_FLAG_COMPRESSED: u16 = 0x0002;
pub const PAGE_FLAG_ENCRYPTED: u16 = 0x0004;

// =====================================================================
//  PageType
// =====================================================================

/// 页类型 — 对应技术方案 9.2 节
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u16)]
pub enum PageType {
    Data = 0,
    Index = 1,
    Undo = 2,
    Fsm = 3,
    Doublewrite = 4,
}

impl PageType {
    /// 从 u16 值构造 PageType，非法值返回 Err
    pub fn from_u16(v: u16) -> Result<Self, PageError> {
        match v {
            0 => Ok(PageType::Data),
            1 => Ok(PageType::Index),
            2 => Ok(PageType::Undo),
            3 => Ok(PageType::Fsm),
            4 => Ok(PageType::Doublewrite),
            _ => Err(PageError::InvalidPageType(v)),
        }
    }

    /// 转为 u16
    pub fn as_u16(self) -> u16 {
        self as u16
    }
}

// =====================================================================
//  PageHeader — 固定 48 字节
// =====================================================================

/// 页头 — 固定 48 字节
///
/// 编码布局（小端）：
/// ```text
/// Offset  Size  Field
/// 0       4     page_id (u32 LE)
/// 4       2     page_type (u16 LE)
/// 6       4     checksum (u32 LE, CRC32C)
/// 10      8     lsn (u64 LE)
/// 18      2     free_offset (u16 LE)
/// 20      2     tuple_count (u16 LE)
/// 22      2     flags (u16 LE)
/// 24      24    reserved (zeroed)
/// Total:  48 bytes
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageHeader {
    pub page_id: u32,
    pub page_type: PageType,
    pub checksum: u32,
    pub lsn: u64,
    pub free_offset: u16,
    pub tuple_count: u16,
    pub flags: u16,
}

impl PageHeader {
    /// 创建新页头，checksum 留空（由 `update_checksum` 填充）
    pub fn new(page_id: u32, page_type: PageType) -> Self {
        Self {
            page_id,
            page_type,
            checksum: 0,
            lsn: 0,
            free_offset: 0,
            tuple_count: 0,
            flags: 0,
        }
    }

    /// 编码到 48 字节缓冲区
    pub fn encode(&self) -> [u8; PAGE_HEADER_SIZE] {
        let mut buf = [0u8; PAGE_HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.page_id.to_le_bytes());
        buf[4..6].copy_from_slice(&self.page_type.as_u16().to_le_bytes());
        buf[6..10].copy_from_slice(&self.checksum.to_le_bytes());
        buf[10..18].copy_from_slice(&self.lsn.to_le_bytes());
        buf[18..20].copy_from_slice(&self.free_offset.to_le_bytes());
        buf[20..22].copy_from_slice(&self.tuple_count.to_le_bytes());
        buf[22..24].copy_from_slice(&self.flags.to_le_bytes());
        // buf[24..48] 已为 0（reserved）
        buf
    }

    /// 从 48 字节缓冲区解码
    pub fn decode(buf: &[u8; PAGE_HEADER_SIZE]) -> Result<Self, PageError> {
        let page_id = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let page_type = PageType::from_u16(u16::from_le_bytes([buf[4], buf[5]]))?;
        let checksum = u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]);
        let lsn = u64::from_le_bytes([
            buf[10], buf[11], buf[12], buf[13], buf[14], buf[15], buf[16], buf[17],
        ]);
        let free_offset = u16::from_le_bytes([buf[18], buf[19]]);
        let tuple_count = u16::from_le_bytes([buf[20], buf[21]]);
        let flags = u16::from_le_bytes([buf[22], buf[23]]);
        Ok(Self {
            page_id,
            page_type,
            checksum,
            lsn,
            free_offset,
            tuple_count,
            flags,
        })
    }

    /// 设置 dirty 标志
    pub fn set_dirty(&mut self, v: bool) {
        if v {
            self.flags |= PAGE_FLAG_DIRTY;
        } else {
            self.flags &= !PAGE_FLAG_DIRTY;
        }
    }

    /// 检查 dirty 标志
    pub fn is_dirty(&self) -> bool {
        self.flags & PAGE_FLAG_DIRTY != 0
    }

    /// 设置 compressed 标志
    pub fn set_compressed(&mut self, v: bool) {
        if v {
            self.flags |= PAGE_FLAG_COMPRESSED;
        } else {
            self.flags &= !PAGE_FLAG_COMPRESSED;
        }
    }

    /// 检查 compressed 标志
    pub fn is_compressed(&self) -> bool {
        self.flags & PAGE_FLAG_COMPRESSED != 0
    }

    /// 设置 encrypted 标志
    pub fn set_encrypted(&mut self, v: bool) {
        if v {
            self.flags |= PAGE_FLAG_ENCRYPTED;
        } else {
            self.flags &= !PAGE_FLAG_ENCRYPTED;
        }
    }

    /// 检查 encrypted 标志
    pub fn is_encrypted(&self) -> bool {
        self.flags & PAGE_FLAG_ENCRYPTED != 0
    }
}

// =====================================================================
//  PageError
// =====================================================================

/// 页错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PageError {
    #[error("page is full")]
    PageFull,
    #[error("invalid page type: {0}")]
    InvalidPageType(u16),
    #[error("checksum mismatch: expected {expected:#010x}, actual {actual:#010x}")]
    ChecksumMismatch { expected: u32, actual: u32 },
    #[error("offset out of bounds: offset={offset}, len={len}, capacity={capacity}")]
    OffsetOutOfBounds {
        offset: usize,
        len: usize,
        capacity: usize,
    },
    #[error("decoding error: {0}")]
    DecodingError(String),
}

// =====================================================================
//  Page — 固定 8KB
// =====================================================================

/// SzRSQL 页 — 固定 8KB（8192 字节）
#[derive(Debug, Clone)]
pub struct Page {
    pub header: PageHeader,
    pub body: [u8; PAGE_BODY_SIZE],
}

impl Page {
    /// 创建新页
    pub fn new(page_id: u32, page_type: PageType) -> Self {
        Self {
            header: PageHeader::new(page_id, page_type),
            body: [0u8; PAGE_BODY_SIZE],
        }
    }

    /// 计算页的 CRC32C 校验和
    ///
    /// 算法：将 header 的 checksum 字段（offset 6..10）置 0，
    /// 对整个 8192 字节页计算 CRC32C。
    pub fn compute_checksum(&self) -> u32 {
        let mut buf = [0u8; PAGE_SIZE];
        // 编码 header（checksum 字段为 0，因为 PageHeader::new 设 checksum=0，
        // 但当前 header 可能已有 checksum，所以需要先编码再清零）
        let mut hdr = self.header.encode();
        // 清零 checksum 字段
        hdr[6..10].copy_from_slice(&0u32.to_le_bytes());
        buf[..PAGE_HEADER_SIZE].copy_from_slice(&hdr);
        buf[PAGE_HEADER_SIZE..].copy_from_slice(&self.body);
        crc32c::crc32c(&buf)
    }

    /// 更新页的 checksum 字段为正确值
    pub fn update_checksum(&mut self) {
        let checksum = self.compute_checksum();
        self.header.checksum = checksum;
        trace!(
            page_id = self.header.page_id,
            lsn = self.header.lsn,
            checksum,
            "page checksum updated"
        );
    }

    /// 验证页的 checksum
    pub fn verify_checksum(&self) -> Result<(), PageError> {
        let expected = self.compute_checksum();
        if self.header.checksum == expected {
            trace!(
                page_id = self.header.page_id,
                lsn = self.header.lsn,
                "page checksum verified"
            );
            Ok(())
        } else {
            warn!(
                page_id = self.header.page_id,
                lsn = self.header.lsn,
                expected = self.header.checksum,
                actual = expected,
                "page checksum mismatch"
            );
            Err(PageError::ChecksumMismatch {
                expected: self.header.checksum,
                actual: expected,
            })
        }
    }

    /// 编码整个页为 8192 字节
    pub fn encode(&self) -> [u8; PAGE_SIZE] {
        let mut buf = [0u8; PAGE_SIZE];
        buf[..PAGE_HEADER_SIZE].copy_from_slice(&self.header.encode());
        buf[PAGE_HEADER_SIZE..].copy_from_slice(&self.body);
        trace!(
            page_id = self.header.page_id,
            lsn = self.header.lsn,
            tuple_count = self.header.tuple_count,
            "page encoded to bytes"
        );
        buf
    }

    /// 从 8192 字节解码
    pub fn decode(buf: &[u8; PAGE_SIZE]) -> Result<Self, PageError> {
        let mut hdr_buf = [0u8; PAGE_HEADER_SIZE];
        hdr_buf.copy_from_slice(&buf[..PAGE_HEADER_SIZE]);
        let header = PageHeader::decode(&hdr_buf)?;
        let mut body = [0u8; PAGE_BODY_SIZE];
        body.copy_from_slice(&buf[PAGE_HEADER_SIZE..]);
        trace!(
            page_id = header.page_id,
            lsn = header.lsn,
            tuple_count = header.tuple_count,
            "page decoded from bytes"
        );
        Ok(Self { header, body })
    }

    /// 向 body 写入数据（从 free_offset 开始）
    ///
    /// 返回写入起始偏移，更新 free_offset 和 tuple_count
    pub fn append_body(&mut self, data: &[u8]) -> Result<u16, PageError> {
        let offset = self.header.free_offset as usize;
        let end = offset + data.len();
        if end > PAGE_BODY_SIZE {
            return Err(PageError::OffsetOutOfBounds {
                offset,
                len: data.len(),
                capacity: PAGE_BODY_SIZE,
            });
        }
        self.body[offset..end].copy_from_slice(data);
        self.header.free_offset = end as u16;
        self.header.tuple_count += 1;
        Ok(offset as u16)
    }

    /// 从 body 指定偏移读取数据
    pub fn read_body(&self, offset: u16, len: usize) -> Result<&[u8], PageError> {
        let start = offset as usize;
        let end = start + len;
        if end > PAGE_BODY_SIZE {
            return Err(PageError::OffsetOutOfBounds {
                offset: start,
                len,
                capacity: PAGE_BODY_SIZE,
            });
        }
        Ok(&self.body[start..end])
    }

    /// 获取 body 剩余可用空间
    pub fn free_space(&self) -> usize {
        PAGE_BODY_SIZE - self.header.free_offset as usize
    }

    /// 重置页到初始状态
    pub fn reset(&mut self) {
        self.header.lsn = 0;
        self.header.free_offset = 0;
        self.header.tuple_count = 0;
        self.header.flags = 0;
        self.body = [0u8; PAGE_BODY_SIZE];
    }

    /// 获取 body 的可变切片（整个 body）
    pub fn body_mut(&mut self) -> &mut [u8; PAGE_BODY_SIZE] {
        &mut self.body
    }
}

impl PartialEq for Page {
    fn eq(&self, other: &Self) -> bool {
        self.header == other.header && self.body[..] == other.body[..]
    }
}

impl Eq for Page {}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  PageType 测试
    // -----------------------------------------------------------------

    #[test]
    fn page_type_all_variants_from_u16() {
        assert_eq!(PageType::from_u16(0).unwrap(), PageType::Data);
        assert_eq!(PageType::from_u16(1).unwrap(), PageType::Index);
        assert_eq!(PageType::from_u16(2).unwrap(), PageType::Undo);
        assert_eq!(PageType::from_u16(3).unwrap(), PageType::Fsm);
        assert_eq!(PageType::from_u16(4).unwrap(), PageType::Doublewrite);
    }

    #[test]
    fn page_type_from_u16_invalid_returns_error() {
        assert!(matches!(
            PageType::from_u16(5),
            Err(PageError::InvalidPageType(5))
        ));
        assert!(matches!(
            PageType::from_u16(u16::MAX),
            Err(PageError::InvalidPageType(_))
        ));
    }

    #[test]
    fn page_type_as_u16_roundtrip() {
        for pt in [
            PageType::Data,
            PageType::Index,
            PageType::Undo,
            PageType::Fsm,
            PageType::Doublewrite,
        ] {
            assert_eq!(PageType::from_u16(pt.as_u16()).unwrap(), pt);
        }
    }

    // -----------------------------------------------------------------
    //  PageHeader 编码/解码
    // -----------------------------------------------------------------

    #[test]
    fn page_header_encode_size_is_48() {
        let hdr = PageHeader::new(42, PageType::Data);
        let buf = hdr.encode();
        assert_eq!(buf.len(), PAGE_HEADER_SIZE);
        assert_eq!(PAGE_HEADER_SIZE, 48);
    }

    #[test]
    fn page_header_encode_decode_roundtrip() {
        let hdr = PageHeader {
            page_id: 12345,
            page_type: PageType::Index,
            checksum: 0xDEADBEEF,
            lsn: 0x1234_5678_9ABC_DEF0,
            free_offset: 1024,
            tuple_count: 100,
            flags: PAGE_FLAG_DIRTY | PAGE_FLAG_COMPRESSED,
        };
        let buf = hdr.encode();
        let back = PageHeader::decode(&buf).unwrap();
        assert_eq!(hdr, back);
    }

    #[test]
    fn page_header_new_defaults() {
        let hdr = PageHeader::new(1, PageType::Data);
        assert_eq!(hdr.page_id, 1);
        assert_eq!(hdr.page_type, PageType::Data);
        assert_eq!(hdr.checksum, 0);
        assert_eq!(hdr.lsn, 0);
        assert_eq!(hdr.free_offset, 0);
        assert_eq!(hdr.tuple_count, 0);
        assert_eq!(hdr.flags, 0);
    }

    #[test]
    fn page_header_decode_invalid_page_type() {
        let mut buf = [0u8; PAGE_HEADER_SIZE];
        buf[4..6].copy_from_slice(&99u16.to_le_bytes()); // 非法 page_type
        let result = PageHeader::decode(&buf);
        assert!(matches!(result, Err(PageError::InvalidPageType(99))));
    }

    #[test]
    fn page_header_reserved_bytes_are_zero() {
        let hdr = PageHeader::new(1, PageType::Data);
        let buf = hdr.encode();
        // reserved: bytes 24..48
        for &b in &buf[24..48] {
            assert_eq!(b, 0, "reserved bytes should be zero");
        }
    }

    // -----------------------------------------------------------------
    //  PageHeader 标志位
    // -----------------------------------------------------------------

    #[test]
    fn page_header_flags_dirty() {
        let mut hdr = PageHeader::new(1, PageType::Data);
        assert!(!hdr.is_dirty());
        hdr.set_dirty(true);
        assert!(hdr.is_dirty());
        assert!(hdr.flags & PAGE_FLAG_DIRTY != 0);
        hdr.set_dirty(false);
        assert!(!hdr.is_dirty());
    }

    #[test]
    fn page_header_flags_compressed() {
        let mut hdr = PageHeader::new(1, PageType::Data);
        assert!(!hdr.is_compressed());
        hdr.set_compressed(true);
        assert!(hdr.is_compressed());
        hdr.set_compressed(false);
        assert!(!hdr.is_compressed());
    }

    #[test]
    fn page_header_flags_encrypted() {
        let mut hdr = PageHeader::new(1, PageType::Data);
        assert!(!hdr.is_encrypted());
        hdr.set_encrypted(true);
        assert!(hdr.is_encrypted());
        hdr.set_encrypted(false);
        assert!(!hdr.is_encrypted());
    }

    #[test]
    fn page_header_flags_combined() {
        let mut hdr = PageHeader::new(1, PageType::Data);
        hdr.set_dirty(true);
        hdr.set_compressed(true);
        hdr.set_encrypted(true);
        assert!(hdr.is_dirty());
        assert!(hdr.is_compressed());
        assert!(hdr.is_encrypted());
        // 清除一个不影响其他
        hdr.set_dirty(false);
        assert!(!hdr.is_dirty());
        assert!(hdr.is_compressed());
        assert!(hdr.is_encrypted());
    }

    // -----------------------------------------------------------------
    //  Page 创建/编码/解码
    // -----------------------------------------------------------------

    #[test]
    fn page_new_defaults() {
        let page = Page::new(42, PageType::Data);
        assert_eq!(page.header.page_id, 42);
        assert_eq!(page.header.page_type, PageType::Data);
        assert_eq!(page.header.free_offset, 0);
        assert_eq!(page.header.tuple_count, 0);
        assert_eq!(page.header.lsn, 0);
        assert_eq!(page.header.flags, 0);
        assert_eq!(page.body.len(), PAGE_BODY_SIZE);
        assert_eq!(PAGE_BODY_SIZE, 8144);
        // body 全零
        for &b in &page.body[..] {
            assert_eq!(b, 0);
        }
    }

    #[test]
    fn page_size_is_8192() {
        assert_eq!(PAGE_SIZE, 8192);
        assert_eq!(PAGE_HEADER_SIZE + PAGE_BODY_SIZE, PAGE_SIZE);
    }

    #[test]
    fn page_encode_size_is_8192() {
        let page = Page::new(1, PageType::Data);
        let buf = page.encode();
        assert_eq!(buf.len(), PAGE_SIZE);
        assert_eq!(buf.len(), 8192);
    }

    #[test]
    fn page_encode_decode_roundtrip() {
        let mut page = Page::new(42, PageType::Index);
        page.header.lsn = 1000;
        page.header.free_offset = 512;
        page.header.tuple_count = 10;
        page.header.set_dirty(true);
        page.body[0..4].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        page.update_checksum();

        let buf = page.encode();
        let back = Page::decode(&buf).unwrap();
        assert_eq!(page, back);
    }

    #[test]
    fn page_encode_decode_all_page_types() {
        for pt in [
            PageType::Data,
            PageType::Index,
            PageType::Undo,
            PageType::Fsm,
            PageType::Doublewrite,
        ] {
            let page = Page::new(1, pt);
            let buf = page.encode();
            let back = Page::decode(&buf).unwrap();
            assert_eq!(back.header.page_type, pt);
        }
    }

    // -----------------------------------------------------------------
    //  校验和测试
    // -----------------------------------------------------------------

    #[test]
    fn page_checksum_correct_after_update() {
        let mut page = Page::new(1, PageType::Data);
        page.header.lsn = 42;
        page.update_checksum();
        assert!(page.verify_checksum().is_ok());
    }

    #[test]
    fn page_checksum_detects_header_corruption() {
        let mut page = Page::new(1, PageType::Data);
        page.update_checksum();
        // 翻转 header 中的 lsn 字段的一位
        page.header.lsn ^= 1;
        assert!(page.verify_checksum().is_err());
    }

    #[test]
    fn page_checksum_detects_body_corruption() {
        let mut page = Page::new(1, PageType::Data);
        page.body[0] = 0x42;
        page.update_checksum();
        // 翻转 body 中的一位
        page.body[0] ^= 1;
        let result = page.verify_checksum();
        assert!(matches!(result, Err(PageError::ChecksumMismatch { .. })));
    }

    #[test]
    fn page_checksum_detects_page_type_corruption() {
        let mut page = Page::new(1, PageType::Data);
        page.update_checksum();
        page.header.page_type = PageType::Index;
        assert!(page.verify_checksum().is_err());
    }

    #[test]
    fn page_checksum_detects_flags_corruption() {
        let mut page = Page::new(1, PageType::Data);
        page.update_checksum();
        page.header.set_dirty(true);
        assert!(page.verify_checksum().is_err());
    }

    #[test]
    fn page_checksum_changes_with_lsn() {
        let mut page = Page::new(1, PageType::Data);
        page.update_checksum();
        let cs1 = page.header.checksum;
        page.header.lsn = 999;
        page.update_checksum();
        let cs2 = page.header.checksum;
        assert_ne!(cs1, cs2);
    }

    #[test]
    fn page_checksum_zero_page_valid() {
        let mut page = Page::new(0, PageType::Data);
        page.update_checksum();
        assert!(page.verify_checksum().is_ok());
    }

    #[test]
    fn page_checksum_error_message_contains_values() {
        let mut page = Page::new(1, PageType::Data);
        page.update_checksum();
        page.body[0] = 0xFF;
        let err = page.verify_checksum().unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("checksum mismatch"));
        assert!(msg.contains("expected"));
        assert!(msg.contains("actual"));
    }

    // -----------------------------------------------------------------
    //  Page body 写入/读取
    // -----------------------------------------------------------------

    #[test]
    fn page_append_body_returns_offset() {
        let mut page = Page::new(1, PageType::Data);
        let data = [1u8, 2, 3, 4];
        let offset = page.append_body(&data).unwrap();
        assert_eq!(offset, 0);
        assert_eq!(page.header.free_offset, 4);
        assert_eq!(page.header.tuple_count, 1);
    }

    #[test]
    fn page_append_body_multiple_writes() {
        let mut page = Page::new(1, PageType::Data);
        let data1 = [1u8, 2];
        let data2 = [3u8, 4, 5];
        let data3 = [6u8];

        let off1 = page.append_body(&data1).unwrap();
        let off2 = page.append_body(&data2).unwrap();
        let off3 = page.append_body(&data3).unwrap();

        assert_eq!(off1, 0);
        assert_eq!(off2, 2);
        assert_eq!(off3, 5);
        assert_eq!(page.header.free_offset, 6);
        assert_eq!(page.header.tuple_count, 3);
    }

    #[test]
    fn page_read_body_returns_correct_data() {
        let mut page = Page::new(1, PageType::Data);
        let data = [0xAA, 0xBB, 0xCC, 0xDD];
        let offset = page.append_body(&data).unwrap();
        let read = page.read_body(offset, data.len()).unwrap();
        assert_eq!(read, &data);
    }

    #[test]
    fn page_append_body_exceeds_capacity_returns_error() {
        let mut page = Page::new(1, PageType::Data);
        let big_data = vec![0u8; PAGE_BODY_SIZE + 1];
        let result = page.append_body(&big_data);
        assert!(matches!(result, Err(PageError::OffsetOutOfBounds { .. })));
    }

    #[test]
    fn page_append_body_exact_capacity_succeeds() {
        let mut page = Page::new(1, PageType::Data);
        let data = vec![0u8; PAGE_BODY_SIZE];
        let result = page.append_body(&data);
        assert!(result.is_ok());
        assert_eq!(page.header.free_offset, PAGE_BODY_SIZE as u16);
    }

    #[test]
    fn page_read_body_out_of_bounds_returns_error() {
        let page = Page::new(1, PageType::Data);
        let result = page.read_body(PAGE_BODY_SIZE as u16 - 4, 10);
        assert!(matches!(result, Err(PageError::OffsetOutOfBounds { .. })));
    }

    #[test]
    fn page_free_space_calculation() {
        let mut page = Page::new(1, PageType::Data);
        assert_eq!(page.free_space(), PAGE_BODY_SIZE);
        page.append_body(&[0u8; 100]).unwrap();
        assert_eq!(page.free_space(), PAGE_BODY_SIZE - 100);
    }

    // -----------------------------------------------------------------
    //  Page reset
    // -----------------------------------------------------------------

    #[test]
    fn page_reset_clears_state() {
        let mut page = Page::new(1, PageType::Data);
        page.append_body(&[1, 2, 3, 4]).unwrap();
        page.header.lsn = 999;
        page.header.set_dirty(true);
        assert!(page.header.tuple_count > 0);

        page.reset();

        assert_eq!(page.header.lsn, 0);
        assert_eq!(page.header.free_offset, 0);
        assert_eq!(page.header.tuple_count, 0);
        assert_eq!(page.header.flags, 0);
        for &b in &page.body[..] {
            assert_eq!(b, 0);
        }
    }

    // -----------------------------------------------------------------
    //  多类型 Page 共存
    // -----------------------------------------------------------------

    #[test]
    fn page_multiple_types_coexist() {
        let pages = [
            Page::new(0, PageType::Data),
            Page::new(1, PageType::Index),
            Page::new(2, PageType::Undo),
            Page::new(3, PageType::Fsm),
            Page::new(4, PageType::Doublewrite),
        ];
        assert_eq!(pages[0].header.page_type, PageType::Data);
        assert_eq!(pages[1].header.page_type, PageType::Index);
        assert_eq!(pages[2].header.page_type, PageType::Undo);
        assert_eq!(pages[3].header.page_type, PageType::Fsm);
        assert_eq!(pages[4].header.page_type, PageType::Doublewrite);
        assert_eq!(pages[0].header.page_id, 0);
        assert_eq!(pages[4].header.page_id, 4);
    }

    #[test]
    fn page_encode_decode_multiple_types() {
        for (id, pt) in [
            (0u32, PageType::Data),
            (1, PageType::Index),
            (2, PageType::Undo),
            (3, PageType::Fsm),
            (4, PageType::Doublewrite),
        ] {
            let mut page = Page::new(id, pt);
            page.header.lsn = id as u64 * 100;
            page.append_body(&[id as u8; 10]).unwrap();
            page.update_checksum();

            let buf = page.encode();
            let back = Page::decode(&buf).unwrap();
            assert_eq!(page, back);
            assert!(back.verify_checksum().is_ok());
        }
    }

    // -----------------------------------------------------------------
    //  边界值测试
    // -----------------------------------------------------------------

    #[test]
    fn page_header_page_id_zero() {
        let hdr = PageHeader::new(0, PageType::Data);
        let buf = hdr.encode();
        let back = PageHeader::decode(&buf).unwrap();
        assert_eq!(back.page_id, 0);
    }

    #[test]
    fn page_header_page_id_max() {
        let hdr = PageHeader::new(u32::MAX, PageType::Data);
        let buf = hdr.encode();
        let back = PageHeader::decode(&buf).unwrap();
        assert_eq!(back.page_id, u32::MAX);
    }

    #[test]
    fn page_header_lsn_max() {
        let mut hdr = PageHeader::new(1, PageType::Data);
        hdr.lsn = u64::MAX;
        let buf = hdr.encode();
        let back = PageHeader::decode(&buf).unwrap();
        assert_eq!(back.lsn, u64::MAX);
    }

    #[test]
    fn page_header_free_offset_max() {
        let mut hdr = PageHeader::new(1, PageType::Data);
        hdr.free_offset = u16::MAX;
        let buf = hdr.encode();
        let back = PageHeader::decode(&buf).unwrap();
        assert_eq!(back.free_offset, u16::MAX);
    }

    #[test]
    fn page_header_tuple_count_max() {
        let mut hdr = PageHeader::new(1, PageType::Data);
        hdr.tuple_count = u16::MAX;
        let buf = hdr.encode();
        let back = PageHeader::decode(&buf).unwrap();
        assert_eq!(back.tuple_count, u16::MAX);
    }

    #[test]
    fn page_header_flags_max() {
        let mut hdr = PageHeader::new(1, PageType::Data);
        hdr.flags = u16::MAX;
        let buf = hdr.encode();
        let back = PageHeader::decode(&buf).unwrap();
        assert_eq!(back.flags, u16::MAX);
    }

    // -----------------------------------------------------------------
    //  body_mut 测试
    // -----------------------------------------------------------------

    #[test]
    fn page_body_mut_allows_direct_write() {
        let mut page = Page::new(1, PageType::Data);
        let body = page.body_mut();
        body[0] = 0xAA;
        body[1] = 0xBB;
        assert_eq!(page.body[0], 0xAA);
        assert_eq!(page.body[1], 0xBB);
    }

    // -----------------------------------------------------------------
    //  encode → decode → verify 完整流程
    // -----------------------------------------------------------------

    #[test]
    fn page_full_lifecycle_encode_decode_verify() {
        let mut page = Page::new(42, PageType::Data);
        page.header.lsn = 12345;
        page.append_body(&[0xCA, 0xFE, 0xBA, 0xBE]).unwrap();
        page.append_body(&[0xDE, 0xAD]).unwrap();
        page.header.set_dirty(true);
        page.update_checksum();

        // 编码
        let buf = page.encode();
        assert_eq!(buf.len(), PAGE_SIZE);

        // 解码
        let back = Page::decode(&buf).unwrap();
        assert_eq!(page, back);

        // 校验
        assert!(back.verify_checksum().is_ok());

        // 验证 body 内容
        assert_eq!(&back.body[0..4], &[0xCA, 0xFE, 0xBA, 0xBE]);
        assert_eq!(&back.body[4..6], &[0xDE, 0xAD]);
    }

    #[test]
    fn page_checksum_after_encode_decode() {
        let mut page = Page::new(1, PageType::Data);
        page.append_body(&[1, 2, 3]).unwrap();
        page.update_checksum();

        let buf = page.encode();
        let back = Page::decode(&buf).unwrap();
        // 编码/解码后 checksum 仍然有效
        assert!(back.verify_checksum().is_ok());
    }
}
