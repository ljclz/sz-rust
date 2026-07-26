//! SzRSQL 元组存储格式 — 对应 `SzRSQL技术实现方案.md` 9.2 节。
//!
//! TupleHeader（固定 20 字节）+ 变长数据区。
//! Page 内使用 slot directory（从 body 末尾向前生长）管理 tuple 偏移。

use crate::page::{Page, PageError, PAGE_BODY_SIZE};

// =====================================================================
//  常量
// =====================================================================

/// TupleHeader 固定大小：20 字节
pub const TUPLE_HEADER_SIZE: usize = 20;

/// null_bitmap 字节数：8 字节（支持 64 列）
pub const NULL_BITMAP_BYTES: usize = 8;

/// null_bitmap 支持的最大列数
pub const MAX_COLUMNS: usize = NULL_BITMAP_BYTES * 8;

/// Slot directory 每条记录大小：4 字节（offset u16 + length u16）
pub const SLOT_ENTRY_SIZE: usize = 4;

// =====================================================================
//  TupleHeader
// =====================================================================

/// 元组头 — 固定 20 字节
///
/// 编码布局（小端）：
/// ```text
/// Offset  Size  Field
/// 0       4     xmin (u32 LE) — 创建事务 ID
/// 4       4     xmax (u32 LE) — 删除事务 ID（0 = 未删除）
/// 8       2     t_cid (u16 LE) — 命令 ID
/// 10      2     col_count (u16 LE) — 列数量
/// 12      8     null_bitmap — Null 位图（bit=1 表示 NOT NULL）
/// Total:  20 bytes
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleHeader {
    pub xmin: u32,
    pub xmax: u32,
    pub t_cid: u16,
    pub col_count: u16,
    pub null_bitmap: [u8; NULL_BITMAP_BYTES],
}

impl TupleHeader {
    /// 创建新元组头
    pub fn new(xmin: u32, col_count: u16) -> Result<Self, TupleError> {
        if col_count as usize > MAX_COLUMNS {
            return Err(TupleError::TooManyColumns {
                col_count: col_count as usize,
                max: MAX_COLUMNS,
            });
        }
        Ok(Self {
            xmin,
            xmax: 0,
            t_cid: 0,
            col_count,
            null_bitmap: [0xFFu8; NULL_BITMAP_BYTES], // 默认全部 NOT NULL
        })
    }

    /// 标记删除
    pub fn mark_deleted(&mut self, xmax: u32) {
        self.xmax = xmax;
    }

    /// 是否已删除
    pub fn is_deleted(&self) -> bool {
        self.xmax != 0
    }

    /// 设置某列为 NULL
    pub fn set_null(&mut self, col_index: usize) -> Result<(), TupleError> {
        if col_index >= self.col_count as usize {
            return Err(TupleError::ColumnIndexOutOfBounds {
                index: col_index,
                col_count: self.col_count as usize,
            });
        }
        let byte_idx = col_index / 8;
        let bit_idx = col_index % 8;
        self.null_bitmap[byte_idx] &= !(1 << bit_idx);
        Ok(())
    }

    /// 检查某列是否为 NULL
    pub fn is_null(&self, col_index: usize) -> Result<bool, TupleError> {
        if col_index >= self.col_count as usize {
            return Err(TupleError::ColumnIndexOutOfBounds {
                index: col_index,
                col_count: self.col_count as usize,
            });
        }
        let byte_idx = col_index / 8;
        let bit_idx = col_index % 8;
        Ok(self.null_bitmap[byte_idx] & (1 << bit_idx) == 0)
    }

    /// 编码为 20 字节
    pub fn encode(&self) -> [u8; TUPLE_HEADER_SIZE] {
        let mut buf = [0u8; TUPLE_HEADER_SIZE];
        buf[0..4].copy_from_slice(&self.xmin.to_le_bytes());
        buf[4..8].copy_from_slice(&self.xmax.to_le_bytes());
        buf[8..10].copy_from_slice(&self.t_cid.to_le_bytes());
        buf[10..12].copy_from_slice(&self.col_count.to_le_bytes());
        buf[12..20].copy_from_slice(&self.null_bitmap);
        buf
    }

    /// 从 20 字节解码
    pub fn decode(buf: &[u8; TUPLE_HEADER_SIZE]) -> Self {
        let xmin = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let xmax = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let t_cid = u16::from_le_bytes([buf[8], buf[9]]);
        let col_count = u16::from_le_bytes([buf[10], buf[11]]);
        let mut null_bitmap = [0u8; NULL_BITMAP_BYTES];
        null_bitmap.copy_from_slice(&buf[12..20]);
        Self {
            xmin,
            xmax,
            t_cid,
            col_count,
            null_bitmap,
        }
    }
}

// =====================================================================
//  TupleError
// =====================================================================

/// 元组错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TupleError {
    #[error("column index {index} out of bounds (col_count: {col_count})")]
    ColumnIndexOutOfBounds { index: usize, col_count: usize },
    #[error("too many columns: {col_count} (max: {max})")]
    TooManyColumns { col_count: usize, max: usize },
    #[error("slot id {slot_id} out of bounds (tuple_count: {tuple_count})")]
    SlotIdOutOfBounds { slot_id: u16, tuple_count: u16 },
    #[error("tuple too large: {size} bytes (max: {max})")]
    TupleTooLarge { size: usize, max: usize },
    #[error("encoding error: {0}")]
    EncodingError(String),
}

impl From<TupleError> for PageError {
    fn from(e: TupleError) -> Self {
        PageError::DecodingError(e.to_string())
    }
}

// =====================================================================
//  TupleSlot
// =====================================================================

/// 变长元组存储格式
///
/// 编码布局：
/// ```text
/// [20 bytes: TupleHeader]
/// [2 bytes: fixed_data_len (u16 LE)]
/// [fixed_data_len bytes: fixed_data]
/// [2 bytes: var_count (u16 LE)]
/// [var_count × 4 bytes: (offset u16 LE, length u16 LE) pairs]
/// [2 bytes: var_data_len (u16 LE)]
/// [var_data_len bytes: var_data]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleSlot {
    pub header: TupleHeader,
    pub fixed_data: Vec<u8>,
    pub var_offsets: Vec<(u16, u16)>,
    pub var_data: Vec<u8>,
}

impl TupleSlot {
    /// 创建新元组
    pub fn new(xmin: u32, col_count: u16) -> Result<Self, TupleError> {
        Ok(Self {
            header: TupleHeader::new(xmin, col_count)?,
            fixed_data: Vec::new(),
            var_offsets: Vec::new(),
            var_data: Vec::new(),
        })
    }

    /// 设置 fixed_data
    pub fn with_fixed_data(mut self, data: Vec<u8>) -> Self {
        self.fixed_data = data;
        self
    }

    /// 添加变长列数据，返回列在 var_data 中的 (offset, length)
    pub fn add_var_column(&mut self, data: &[u8]) -> Result<(u16, u16), TupleError> {
        let offset = self.var_data.len();
        let length = data.len();
        if offset + length > u16::MAX as usize {
            return Err(TupleError::TupleTooLarge {
                size: offset + length,
                max: u16::MAX as usize,
            });
        }
        self.var_data.extend_from_slice(data);
        let pair = (offset as u16, length as u16);
        self.var_offsets.push(pair);
        Ok(pair)
    }

    /// 获取变长列数据
    pub fn get_var_column(&self, index: usize) -> Result<&[u8], TupleError> {
        if index >= self.var_offsets.len() {
            return Err(TupleError::ColumnIndexOutOfBounds {
                index,
                col_count: self.var_offsets.len(),
            });
        }
        let (offset, length) = self.var_offsets[index];
        Ok(&self.var_data[offset as usize..(offset + length) as usize])
    }

    /// 计算编码后的字节数
    pub fn encoded_size(&self) -> usize {
        TUPLE_HEADER_SIZE
            + 2
            + self.fixed_data.len()
            + 2
            + self.var_offsets.len() * 4
            + 2
            + self.var_data.len()
    }

    /// 编码为字节向量
    pub fn encode(&self) -> Vec<u8> {
        let size = self.encoded_size();
        let mut buf = Vec::with_capacity(size);
        buf.extend_from_slice(&self.header.encode());
        buf.extend_from_slice(&(self.fixed_data.len() as u16).to_le_bytes());
        buf.extend_from_slice(&self.fixed_data);
        buf.extend_from_slice(&(self.var_offsets.len() as u16).to_le_bytes());
        for (offset, length) in &self.var_offsets {
            buf.extend_from_slice(&offset.to_le_bytes());
            buf.extend_from_slice(&length.to_le_bytes());
        }
        buf.extend_from_slice(&(self.var_data.len() as u16).to_le_bytes());
        buf.extend_from_slice(&self.var_data);
        buf
    }

    /// 从字节切片解码
    pub fn decode(data: &[u8]) -> Result<Self, TupleError> {
        if data.len() < TUPLE_HEADER_SIZE + 6 {
            return Err(TupleError::EncodingError(format!(
                "data too short: {} bytes (min {})",
                data.len(),
                TUPLE_HEADER_SIZE + 6
            )));
        }
        let mut hdr_buf = [0u8; TUPLE_HEADER_SIZE];
        hdr_buf.copy_from_slice(&data[..TUPLE_HEADER_SIZE]);
        let header = TupleHeader::decode(&hdr_buf);

        let mut pos = TUPLE_HEADER_SIZE;

        // fixed_data
        let fixed_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        if pos + fixed_len > data.len() {
            return Err(TupleError::EncodingError("fixed_data truncated".into()));
        }
        let fixed_data = data[pos..pos + fixed_len].to_vec();
        pos += fixed_len;

        // var_offsets
        if pos + 2 > data.len() {
            return Err(TupleError::EncodingError("var_count truncated".into()));
        }
        let var_count = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        if pos + var_count * 4 > data.len() {
            return Err(TupleError::EncodingError("var_offsets truncated".into()));
        }
        let mut var_offsets = Vec::with_capacity(var_count);
        for _ in 0..var_count {
            let offset = u16::from_le_bytes([data[pos], data[pos + 1]]);
            let length = u16::from_le_bytes([data[pos + 2], data[pos + 3]]);
            var_offsets.push((offset, length));
            pos += 4;
        }

        // var_data
        if pos + 2 > data.len() {
            return Err(TupleError::EncodingError("var_data_len truncated".into()));
        }
        let var_data_len = u16::from_le_bytes([data[pos], data[pos + 1]]) as usize;
        pos += 2;
        if pos + var_data_len > data.len() {
            return Err(TupleError::EncodingError("var_data truncated".into()));
        }
        let var_data = data[pos..pos + var_data_len].to_vec();

        Ok(Self {
            header,
            fixed_data,
            var_offsets,
            var_data,
        })
    }
}

// =====================================================================
//  Page — Tuple 管理方法
// =====================================================================

impl Page {
    /// Slot directory 起始偏移（从 body 开头算）
    ///
    /// Slot directory 从 body 末尾向前生长，每条记录 4 字节。
    /// `slot_dir_start = PAGE_BODY_SIZE - tuple_count * SLOT_ENTRY_SIZE`
    fn slot_dir_start(&self) -> usize {
        PAGE_BODY_SIZE - self.header.tuple_count as usize * SLOT_ENTRY_SIZE
    }

    /// 可用空间（tuple 数据区 + slot directory 之间的空闲）
    pub fn available_for_tuple(&self) -> usize {
        self.slot_dir_start()
            .saturating_sub(self.header.free_offset as usize)
    }

    /// 在页中插入一个元组，返回 slot_id
    pub fn insert_tuple(&mut self, tuple: &TupleSlot) -> Result<u16, PageError> {
        let encoded = tuple.encode();
        let tuple_size = encoded.len();

        // 需要空间：tuple 数据 + slot directory 条目
        let needed = tuple_size + SLOT_ENTRY_SIZE;
        if self.available_for_tuple() < needed {
            return Err(PageError::PageFull);
        }

        let slot_id = self.header.tuple_count;
        let offset = self.header.free_offset as usize;

        // 写入 tuple 数据
        self.body[offset..offset + tuple_size].copy_from_slice(&encoded);

        // 写入 slot directory 条目（从末尾向前）
        let slot_pos = self.slot_dir_start() - SLOT_ENTRY_SIZE;
        self.body[slot_pos..slot_pos + 2].copy_from_slice(&(offset as u16).to_le_bytes());
        self.body[slot_pos + 2..slot_pos + 4].copy_from_slice(&(tuple_size as u16).to_le_bytes());

        // 更新 header
        self.header.free_offset = (offset + tuple_size) as u16;
        self.header.tuple_count += 1;

        Ok(slot_id)
    }

    /// 读取 slot directory 条目，返回 (offset, length)
    fn read_slot_entry(&self, slot_id: u16) -> Result<(usize, usize), PageError> {
        if slot_id >= self.header.tuple_count {
            return Err(TupleError::SlotIdOutOfBounds {
                slot_id,
                tuple_count: self.header.tuple_count,
            }
            .into());
        }
        let entry_pos = PAGE_BODY_SIZE - (slot_id as usize + 1) * SLOT_ENTRY_SIZE;
        let offset = u16::from_le_bytes([self.body[entry_pos], self.body[entry_pos + 1]]) as usize;
        let length =
            u16::from_le_bytes([self.body[entry_pos + 2], self.body[entry_pos + 3]]) as usize;
        Ok((offset, length))
    }

    /// 从页中读取元组
    pub fn read_tuple(&self, slot_id: u16) -> Result<TupleSlot, PageError> {
        let (offset, length) = self.read_slot_entry(slot_id)?;
        if offset + length > PAGE_BODY_SIZE {
            return Err(PageError::DecodingError(format!(
                "slot {slot_id} points out of bounds: offset={offset}, length={length}"
            )));
        }
        let data = &self.body[offset..offset + length];
        TupleSlot::decode(data).map_err(PageError::from)
    }

    /// 标记删除（MVCC：设置 xmax，不立即回收空间）
    pub fn mark_tuple_deleted(&mut self, slot_id: u16, xmax: u32) -> Result<(), PageError> {
        let (offset, length) = self.read_slot_entry(slot_id)?;
        let mut tuple = TupleSlot::decode(&self.body[offset..offset + length])?;
        tuple.header.mark_deleted(xmax);
        // 写回（大小不变）
        let encoded = tuple.encode();
        debug_assert_eq!(encoded.len(), length);
        self.body[offset..offset + length].copy_from_slice(&encoded);
        Ok(())
    }

    /// 更新元组（标记旧元组删除 + 插入新元组），返回新 slot_id
    pub fn update_tuple(
        &mut self,
        slot_id: u16,
        new_tuple: &TupleSlot,
        xmax: u32,
    ) -> Result<u16, PageError> {
        self.mark_tuple_deleted(slot_id, xmax)?;
        self.insert_tuple(new_tuple)
    }

    /// 获取页中活跃（未删除）的 tuple slot_id 列表
    pub fn live_slot_ids(&self) -> Result<Vec<u16>, PageError> {
        let mut live = Vec::new();
        for slot_id in 0..self.header.tuple_count {
            let tuple = self.read_tuple(slot_id)?;
            if !tuple.header.is_deleted() {
                live.push(slot_id);
            }
        }
        Ok(live)
    }

    /// 碎片整理：回收已删除 tuple 占用的空间，重新连续排列活跃 tuple。
    ///
    /// 算法（对应 `SzRSQL技术实现方案.md` 9.2 节页格式 — VACUUM/compact）：
    /// 1. 顺序扫描 slot directory，收集所有未删除的 tuple
    /// 2. 清零 body，重置 free_offset=0、tuple_count=0
    /// 3. 按原顺序重新插入活跃 tuple（slot_id 重新编号 0..N）
    ///
    /// 注意：compact 后 slot_id 会改变，调用方需通过 `live_slot_ids()` 重建索引。
    pub fn compact(&mut self) -> Result<(), PageError> {
        // 1. 收集所有活跃 tuple
        let mut live_tuples: Vec<TupleSlot> = Vec::new();
        for slot_id in 0..self.header.tuple_count {
            let tuple = self.read_tuple(slot_id)?;
            if !tuple.header.is_deleted() {
                live_tuples.push(tuple);
            }
        }

        // 2. 清零 body 与 header 计数器
        self.body = [0u8; PAGE_BODY_SIZE];
        self.header.free_offset = 0;
        self.header.tuple_count = 0;

        // 3. 重新插入活跃 tuple
        for tuple in &live_tuples {
            self.insert_tuple(tuple)?;
        }

        Ok(())
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::PageType;

    // -----------------------------------------------------------------
    //  TupleHeader 测试
    // -----------------------------------------------------------------

    #[test]
    fn tuple_header_new_defaults() {
        let hdr = TupleHeader::new(42, 5).unwrap();
        assert_eq!(hdr.xmin, 42);
        assert_eq!(hdr.xmax, 0);
        assert_eq!(hdr.t_cid, 0);
        assert_eq!(hdr.col_count, 5);
        assert!(!hdr.is_deleted());
        // 默认全部 NOT NULL
        for i in 0..5 {
            assert!(!hdr.is_null(i).unwrap());
        }
    }

    #[test]
    fn tuple_header_too_many_columns() {
        let result = TupleHeader::new(1, 65);
        assert!(matches!(
            result,
            Err(TupleError::TooManyColumns {
                col_count: 65,
                max: 64
            })
        ));
    }

    #[test]
    fn tuple_header_max_columns_ok() {
        let hdr = TupleHeader::new(1, 64).unwrap();
        assert_eq!(hdr.col_count, 64);
    }

    #[test]
    fn tuple_header_mark_deleted() {
        let mut hdr = TupleHeader::new(1, 2).unwrap();
        assert!(!hdr.is_deleted());
        hdr.mark_deleted(99);
        assert!(hdr.is_deleted());
        assert_eq!(hdr.xmax, 99);
    }

    #[test]
    fn tuple_header_null_bitmap_set_and_check() {
        let mut hdr = TupleHeader::new(1, 10).unwrap();
        // 初始全部 NOT NULL
        for i in 0..10 {
            assert!(!hdr.is_null(i).unwrap());
        }
        // 设置第 3、7 列为 NULL
        hdr.set_null(3).unwrap();
        hdr.set_null(7).unwrap();
        assert!(hdr.is_null(3).unwrap());
        assert!(hdr.is_null(7).unwrap());
        assert!(!hdr.is_null(0).unwrap());
        assert!(!hdr.is_null(1).unwrap());
        assert!(!hdr.is_null(9).unwrap());
    }

    #[test]
    fn tuple_header_null_bitmap_all_64_columns() {
        let mut hdr = TupleHeader::new(1, 64).unwrap();
        // 设置偶数列为 NULL
        for i in (0..64).step_by(2) {
            hdr.set_null(i).unwrap();
        }
        for i in 0..64 {
            let expected_null = i % 2 == 0;
            assert_eq!(hdr.is_null(i).unwrap(), expected_null, "column {i}");
        }
    }

    #[test]
    fn tuple_header_null_bitmap_column_out_of_bounds() {
        let hdr = TupleHeader::new(1, 5).unwrap();
        assert!(matches!(
            hdr.is_null(5),
            Err(TupleError::ColumnIndexOutOfBounds {
                index: 5,
                col_count: 5
            })
        ));
        assert!(matches!(
            hdr.is_null(100),
            Err(TupleError::ColumnIndexOutOfBounds { .. })
        ));
    }

    #[test]
    fn tuple_header_encode_decode_roundtrip() {
        let mut hdr = TupleHeader::new(12345, 10).unwrap();
        hdr.xmax = 999;
        hdr.t_cid = 7;
        hdr.set_null(2).unwrap();
        hdr.set_null(5).unwrap();

        let buf = hdr.encode();
        assert_eq!(buf.len(), TUPLE_HEADER_SIZE);
        assert_eq!(TUPLE_HEADER_SIZE, 20);

        let back = TupleHeader::decode(&buf);
        assert_eq!(hdr, back);
    }

    #[test]
    fn tuple_header_encode_field_layout() {
        let mut hdr = TupleHeader::new(0x11223344, 0x0020).unwrap(); // col_count=32
        hdr.xmax = 0x778899AA;
        hdr.t_cid = 0xBBCC;
        let buf = hdr.encode();
        // xmin
        assert_eq!(&buf[0..4], &0x11223344u32.to_le_bytes());
        // xmax
        assert_eq!(&buf[4..8], &0x778899AAu32.to_le_bytes());
        // t_cid
        assert_eq!(&buf[8..10], &0xBBCCu16.to_le_bytes());
        // col_count
        assert_eq!(&buf[10..12], &0x0020u16.to_le_bytes());
        // null_bitmap (12..20)
        assert_eq!(&buf[12..20], &hdr.null_bitmap[..]);
    }

    // -----------------------------------------------------------------
    //  TupleSlot 测试
    // -----------------------------------------------------------------

    #[test]
    fn tuple_slot_new_defaults() {
        let slot = TupleSlot::new(1, 3).unwrap();
        assert_eq!(slot.header.xmin, 1);
        assert_eq!(slot.header.col_count, 3);
        assert!(slot.fixed_data.is_empty());
        assert!(slot.var_offsets.is_empty());
        assert!(slot.var_data.is_empty());
    }

    #[test]
    fn tuple_slot_with_fixed_data() {
        let slot = TupleSlot::new(1, 3)
            .unwrap()
            .with_fixed_data(vec![1, 2, 3, 4]);
        assert_eq!(slot.fixed_data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn tuple_slot_add_get_var_column() {
        let mut slot = TupleSlot::new(1, 3).unwrap();
        let (off1, len1) = slot.add_var_column(b"hello").unwrap();
        assert_eq!(off1, 0);
        assert_eq!(len1, 5);

        let (off2, len2) = slot.add_var_column(b"world").unwrap();
        assert_eq!(off2, 5);
        assert_eq!(len2, 5);

        assert_eq!(slot.get_var_column(0).unwrap(), b"hello");
        assert_eq!(slot.get_var_column(1).unwrap(), b"world");
        assert_eq!(slot.var_offsets.len(), 2);
        assert_eq!(slot.var_data, b"helloworld");
    }

    #[test]
    fn tuple_slot_get_var_column_out_of_bounds() {
        let slot = TupleSlot::new(1, 3).unwrap();
        assert!(matches!(
            slot.get_var_column(0),
            Err(TupleError::ColumnIndexOutOfBounds { .. })
        ));
    }

    #[test]
    fn tuple_slot_encoded_size_empty() {
        let slot = TupleSlot::new(1, 0).unwrap();
        // 20 (header) + 2 (fixed_len) + 0 + 2 (var_count) + 0 + 2 (var_data_len) + 0 = 26
        assert_eq!(slot.encoded_size(), 26);
    }

    #[test]
    fn tuple_slot_encoded_size_with_data() {
        let mut slot = TupleSlot::new(1, 3).unwrap();
        slot.fixed_data = vec![0u8; 16];
        slot.add_var_column(b"abc").unwrap();
        slot.add_var_column(b"de").unwrap();
        // 20 + 2 + 16 + 2 + 2*4 + 2 + 5 = 55
        assert_eq!(slot.encoded_size(), 20 + 2 + 16 + 2 + 8 + 2 + 5);
    }

    #[test]
    fn tuple_slot_encode_decode_roundtrip_empty() {
        let slot = TupleSlot::new(42, 0).unwrap();
        let encoded = slot.encode();
        let back = TupleSlot::decode(&encoded).unwrap();
        assert_eq!(slot, back);
    }

    #[test]
    fn tuple_slot_encode_decode_roundtrip_with_data() {
        let mut slot = TupleSlot::new(100, 5).unwrap();
        slot.header.xmax = 200;
        slot.header.t_cid = 3;
        slot.header.set_null(2).unwrap();
        slot.fixed_data = vec![0xAA, 0xBB, 0xCC, 0xDD];
        slot.add_var_column(b"first").unwrap();
        slot.add_var_column(b"second").unwrap();
        slot.add_var_column(b"third").unwrap();

        let encoded = slot.encode();
        let back = TupleSlot::decode(&encoded).unwrap();
        assert_eq!(slot, back);
        assert_eq!(back.get_var_column(0).unwrap(), b"first");
        assert_eq!(back.get_var_column(1).unwrap(), b"second");
        assert_eq!(back.get_var_column(2).unwrap(), b"third");
        assert!(back.header.is_null(2).unwrap());
        assert!(!back.header.is_null(0).unwrap());
    }

    #[test]
    fn tuple_slot_decode_too_short() {
        let result = TupleSlot::decode(&[0u8; 10]);
        assert!(matches!(result, Err(TupleError::EncodingError(_))));
    }

    #[test]
    fn tuple_slot_decode_truncated_fixed_data() {
        // 构造一个声称有 100 字节 fixed_data 但实际只有 5 字节的 buffer
        let mut buf = vec![0u8; TUPLE_HEADER_SIZE];
        buf.extend_from_slice(&100u16.to_le_bytes()); // fixed_data_len = 100
        buf.extend_from_slice(&[1, 2, 3]); // 只有 3 字节
        let result = TupleSlot::decode(&buf);
        assert!(matches!(result, Err(TupleError::EncodingError(_))));
    }

    /// Prove-It：fuzz 发现的 bug — `var_count` 读取越界
    ///
    /// 场景：data.len() = 26（恰好通过初始检查 TUPLE_HEADER_SIZE + 6 = 26），
    /// fixed_len = 4，把 pos 推到 26，读取 data[26]、data[27] 时越界。
    #[test]
    fn tuple_slot_decode_var_count_out_of_bounds_no_panic() {
        // 构造 buffer：header(20) + fixed_len=4(2) + fixed_data(4) = 26 字节
        // 此时 pos=26，data.len()=26，读 var_count 需要 data[26..28] 越界
        let mut buf = vec![0u8; TUPLE_HEADER_SIZE];
        buf.extend_from_slice(&4u16.to_le_bytes()); // fixed_data_len = 4
        buf.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]); // fixed_data
                                                          // 不再追加任何字节 — 此时 data.len() = 26, pos = 26
        assert_eq!(buf.len(), TUPLE_HEADER_SIZE + 6);
        let result = TupleSlot::decode(&buf);
        assert!(
            matches!(result, Err(TupleError::EncodingError(_))),
            "should return EncodingError, not panic"
        );
    }

    /// Prove-It：fuzz 边界 — fixed_len 正好把 pos 推到 data.len() - 1
    #[test]
    fn tuple_slot_decode_var_count_one_byte_short_no_panic() {
        // data.len() = 27，pos=26，读 data[26] OK，data[27] 越界
        let mut buf = vec![0u8; TUPLE_HEADER_SIZE];
        buf.extend_from_slice(&4u16.to_le_bytes()); // fixed_data_len = 4
        buf.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]); // fixed_data
        buf.push(0x01); // 只追加 1 字节，data.len() = 27
        let result = TupleSlot::decode(&buf);
        assert!(
            matches!(result, Err(TupleError::EncodingError(_))),
            "should return EncodingError, not panic"
        );
    }

    // -----------------------------------------------------------------
    //  Page insert/read tuple 测试
    // -----------------------------------------------------------------

    #[test]
    fn page_insert_single_tuple() {
        let mut page = Page::new(0, PageType::Data);
        let tuple = TupleSlot::new(1, 2).unwrap();
        let slot_id = page.insert_tuple(&tuple).unwrap();
        assert_eq!(slot_id, 0);
        assert_eq!(page.header.tuple_count, 1);
    }

    #[test]
    fn page_insert_and_read_tuple() {
        let mut page = Page::new(0, PageType::Data);
        let mut tuple = TupleSlot::new(42, 3).unwrap();
        tuple.fixed_data = vec![0x11, 0x22, 0x33, 0x44];
        tuple.add_var_column(b"test data").unwrap();

        let slot_id = page.insert_tuple(&tuple).unwrap();
        let back = page.read_tuple(slot_id).unwrap();
        assert_eq!(tuple, back);
    }

    #[test]
    fn page_insert_multiple_tuples() {
        let mut page = Page::new(0, PageType::Data);

        for i in 0..10u32 {
            let mut tuple = TupleSlot::new(i, 2).unwrap();
            tuple.fixed_data = vec![i as u8; 4];
            tuple.add_var_column(format!("row_{i}").as_bytes()).unwrap();
            let slot_id = page.insert_tuple(&tuple).unwrap();
            assert_eq!(slot_id, i as u16);
        }

        assert_eq!(page.header.tuple_count, 10);

        // 验证每个 tuple
        for i in 0..10u32 {
            let back = page.read_tuple(i as u16).unwrap();
            assert_eq!(back.header.xmin, i);
            assert_eq!(back.fixed_data, vec![i as u8; 4]);
            assert_eq!(
                back.get_var_column(0).unwrap(),
                format!("row_{i}").as_bytes()
            );
        }
    }

    #[test]
    fn page_read_tuple_slot_out_of_bounds() {
        let page = Page::new(0, PageType::Data);
        let result = page.read_tuple(0);
        assert!(matches!(result, Err(PageError::DecodingError(_))));
    }

    #[test]
    fn page_insert_tuple_page_full() {
        let mut page = Page::new(0, PageType::Data);
        // 创建一个超大 tuple（接近 page body 大小）
        let big_data = vec![0u8; PAGE_BODY_SIZE];
        let mut tuple = TupleSlot::new(1, 1).unwrap();
        tuple.fixed_data = big_data;
        let result = page.insert_tuple(&tuple);
        assert!(matches!(result, Err(PageError::PageFull)));
    }

    // -----------------------------------------------------------------
    //  Page mark_deleted / update_tuple 测试
    // -----------------------------------------------------------------

    #[test]
    fn page_mark_tuple_deleted() {
        let mut page = Page::new(0, PageType::Data);
        let tuple = TupleSlot::new(1, 2).unwrap();
        let slot_id = page.insert_tuple(&tuple).unwrap();

        // 标记删除
        page.mark_tuple_deleted(slot_id, 99).unwrap();

        let back = page.read_tuple(slot_id).unwrap();
        assert!(back.header.is_deleted());
        assert_eq!(back.header.xmax, 99);
        assert_eq!(back.header.xmin, 1); // xmin 不变
    }

    #[test]
    fn page_update_tuple() {
        let mut page = Page::new(0, PageType::Data);

        // 插入原始 tuple
        let mut tuple1 = TupleSlot::new(1, 2).unwrap();
        tuple1.fixed_data = vec![0xAA];
        let slot1 = page.insert_tuple(&tuple1).unwrap();

        // 更新：标记旧 tuple 删除 + 插入新 tuple
        let mut tuple2 = TupleSlot::new(1, 2).unwrap();
        tuple2.fixed_data = vec![0xBB];
        let slot2 = page.update_tuple(slot1, &tuple2, 100).unwrap();

        // 旧 tuple 标记为删除
        let back1 = page.read_tuple(slot1).unwrap();
        assert!(back1.header.is_deleted());
        assert_eq!(back1.header.xmax, 100);

        // 新 tuple 活跃
        let back2 = page.read_tuple(slot2).unwrap();
        assert!(!back2.header.is_deleted());
        assert_eq!(back2.fixed_data, vec![0xBB]);
    }

    #[test]
    fn page_live_slot_ids() {
        let mut page = Page::new(0, PageType::Data);

        // 插入 5 个 tuple
        let mut slots = Vec::new();
        for i in 0..5u32 {
            let tuple = TupleSlot::new(i, 1).unwrap();
            slots.push(page.insert_tuple(&tuple).unwrap());
        }

        // 删除第 1、3 个
        page.mark_tuple_deleted(slots[1], 100).unwrap();
        page.mark_tuple_deleted(slots[3], 100).unwrap();

        let live = page.live_slot_ids().unwrap();
        assert_eq!(live, vec![0, 2, 4]); // slot_ids: 0, 2, 4
    }

    // -----------------------------------------------------------------
    //  跨多个 tuple 的连续存储
    // -----------------------------------------------------------------

    #[test]
    fn page_sequential_tuple_storage() {
        let mut page = Page::new(0, PageType::Data);

        // 连续写入 100 个 tuple
        for i in 0..100u32 {
            let mut tuple = TupleSlot::new(i, 2).unwrap();
            tuple.fixed_data = i.to_le_bytes().to_vec();
            tuple.add_var_column(format!("val_{i}").as_bytes()).unwrap();
            page.insert_tuple(&tuple).unwrap();
        }

        assert_eq!(page.header.tuple_count, 100);

        // 验证全部可正确读取
        for i in 0..100u32 {
            let back = page.read_tuple(i as u16).unwrap();
            assert_eq!(back.header.xmin, i);
            assert_eq!(back.fixed_data, i.to_le_bytes());
            assert_eq!(
                back.get_var_column(0).unwrap(),
                format!("val_{i}").as_bytes()
            );
        }
    }

    #[test]
    fn page_tuple_storage_with_nulls() {
        let mut page = Page::new(0, PageType::Data);

        // 创建一个有 NULL 列的 tuple
        let mut tuple = TupleSlot::new(1, 5).unwrap();
        tuple.header.set_null(1).unwrap();
        tuple.header.set_null(3).unwrap();
        tuple.fixed_data = vec![0xFF; 8];
        tuple.add_var_column(b"non-null").unwrap();

        let slot_id = page.insert_tuple(&tuple).unwrap();
        let back = page.read_tuple(slot_id).unwrap();

        assert_eq!(tuple, back);
        assert!(back.header.is_null(1).unwrap());
        assert!(back.header.is_null(3).unwrap());
        assert!(!back.header.is_null(0).unwrap());
        assert!(!back.header.is_null(2).unwrap());
        assert!(!back.header.is_null(4).unwrap());
    }

    // -----------------------------------------------------------------
    //  边界值测试
    // -----------------------------------------------------------------

    #[test]
    fn page_available_for_tuple_empty_page() {
        let page = Page::new(0, PageType::Data);
        // 空页：可用空间 = PAGE_BODY_SIZE（无 slot directory）
        assert_eq!(page.available_for_tuple(), PAGE_BODY_SIZE);
    }

    #[test]
    fn page_available_for_tuple_decreases() {
        let mut page = Page::new(0, PageType::Data);
        let tuple = TupleSlot::new(1, 1).unwrap();
        let initial = page.available_for_tuple();
        page.insert_tuple(&tuple).unwrap();
        let after = page.available_for_tuple();
        assert!(after < initial);
        // 每个 tuple 占用 encoded_size + 4 (slot entry)
        assert_eq!(initial - after, tuple.encoded_size() + SLOT_ENTRY_SIZE);
    }

    #[test]
    fn page_insert_exact_fit() {
        let mut page = Page::new(0, PageType::Data);
        // 计算能放下的最大 tuple
        let available = page.available_for_tuple();
        // tuple 需要占用 encoded_size + 4 (slot entry)
        // 空 tuple encoded_size = 26，所以 fixed_data 最大 = available - 26 - 4
        let max_fixed = available - 26 - SLOT_ENTRY_SIZE;
        let mut tuple = TupleSlot::new(1, 1).unwrap();
        tuple.fixed_data = vec![0u8; max_fixed];
        assert!(page.insert_tuple(&tuple).is_ok());
    }

    #[test]
    fn page_insert_one_byte_over_fails() {
        let mut page = Page::new(0, PageType::Data);
        let available = page.available_for_tuple();
        let max_fixed = available - 26 - SLOT_ENTRY_SIZE + 1; // 多 1 字节
        let mut tuple = TupleSlot::new(1, 1).unwrap();
        tuple.fixed_data = vec![0u8; max_fixed];
        assert!(matches!(
            page.insert_tuple(&tuple),
            Err(PageError::PageFull)
        ));
    }

    #[test]
    fn page_update_preserves_xmin() {
        let mut page = Page::new(0, PageType::Data);
        let tuple = TupleSlot::new(42, 2).unwrap();
        let slot1 = page.insert_tuple(&tuple).unwrap();

        let mut new_tuple = TupleSlot::new(42, 2).unwrap(); // 同一事务
        new_tuple.fixed_data = vec![0xEE];
        let slot2 = page.update_tuple(slot1, &new_tuple, 99).unwrap();

        let back = page.read_tuple(slot2).unwrap();
        assert_eq!(back.header.xmin, 42);
        assert!(!back.header.is_deleted());
    }

    // -----------------------------------------------------------------
    //  完整生命周期测试
    // -----------------------------------------------------------------

    #[test]
    fn page_tuple_full_lifecycle() {
        let mut page = Page::new(0, PageType::Data);

        // 1. 插入
        let mut t1 = TupleSlot::new(1, 3).unwrap();
        t1.fixed_data = vec![1, 2, 3];
        t1.add_var_column(b"first").unwrap();
        let s1 = page.insert_tuple(&t1).unwrap();

        let mut t2 = TupleSlot::new(2, 3).unwrap();
        t2.fixed_data = vec![4, 5, 6];
        t2.add_var_column(b"second").unwrap();
        let s2 = page.insert_tuple(&t2).unwrap();

        // 2. 读取验证
        assert_eq!(page.read_tuple(s1).unwrap(), t1);
        assert_eq!(page.read_tuple(s2).unwrap(), t2);

        // 3. 更新 t1
        let mut t3 = TupleSlot::new(1, 3).unwrap();
        t3.fixed_data = vec![7, 8, 9];
        t3.add_var_column(b"updated").unwrap();
        let s3 = page.update_tuple(s1, &t3, 100).unwrap();

        // 4. 旧 t1 已删除
        assert!(page.read_tuple(s1).unwrap().header.is_deleted());

        // 5. 活跃列表 = [s2, s3]
        let live = page.live_slot_ids().unwrap();
        assert_eq!(live, vec![s2, s3]);

        // 6. 更新 checksum 并验证
        page.update_checksum();
        assert!(page.verify_checksum().is_ok());
    }

    #[test]
    fn page_tuple_encode_decode_with_checksum() {
        let mut page = Page::new(42, PageType::Data);
        let mut tuple = TupleSlot::new(100, 3).unwrap();
        tuple.fixed_data = vec![0xCA, 0xFE];
        tuple.add_var_column(b"hello world").unwrap();
        tuple.header.set_null(2).unwrap();
        page.insert_tuple(&tuple).unwrap();
        page.update_checksum();

        // 编码 → 解码 → 校验
        let buf = page.encode();
        let back = Page::decode(&buf).unwrap();
        assert!(back.verify_checksum().is_ok());

        // 读取 tuple
        let t = back.read_tuple(0).unwrap();
        assert_eq!(t, tuple);
    }

    // -----------------------------------------------------------------
    //  Page::compact() 测试（Phase 0.8 — 碎片整理）
    // -----------------------------------------------------------------

    #[test]
    fn page_compact_empty_page() {
        let mut page = Page::new(0, PageType::Data);
        page.compact().unwrap();
        assert_eq!(page.header.tuple_count, 0);
        assert_eq!(page.header.free_offset, 0);
        assert_eq!(page.available_for_tuple(), PAGE_BODY_SIZE);
    }

    #[test]
    fn page_compact_no_deleted_tuples_preserves_data() {
        let mut page = Page::new(0, PageType::Data);
        let mut tuples = Vec::new();
        for i in 0..5u32 {
            let mut t = TupleSlot::new(i, 2).unwrap();
            t.fixed_data = vec![i as u8; 4];
            t.add_var_column(format!("row_{i}").as_bytes()).unwrap();
            tuples.push(t);
        }
        for t in &tuples {
            page.insert_tuple(t).unwrap();
        }

        let free_before = page.available_for_tuple();
        page.compact().unwrap();
        let free_after = page.available_for_tuple();

        // 无删除：free space 不变
        assert_eq!(free_before, free_after);
        assert_eq!(page.header.tuple_count, 5);

        // 所有 tuple 仍可正确读取
        for (i, t) in tuples.iter().enumerate() {
            let back = page.read_tuple(i as u16).unwrap();
            assert_eq!(&back, t, "slot {i} data mismatch after compact");
        }
    }

    #[test]
    fn page_compact_reclaims_deleted_space() {
        let mut page = Page::new(0, PageType::Data);

        let mut slots = Vec::new();
        for i in 0..5u32 {
            let mut t = TupleSlot::new(i, 2).unwrap();
            t.fixed_data = vec![i as u8; 100];
            slots.push(page.insert_tuple(&t).unwrap());
        }

        // 删除 3 个
        page.mark_tuple_deleted(slots[1], 100).unwrap();
        page.mark_tuple_deleted(slots[2], 100).unwrap();
        page.mark_tuple_deleted(slots[4], 100).unwrap();

        let free_before = page.available_for_tuple();
        page.compact().unwrap();
        let free_after = page.available_for_tuple();

        // 回收后 free space 应增加
        assert!(
            free_after > free_before,
            "free_after={free_after} should be > free_before={free_before}"
        );
        assert_eq!(page.header.tuple_count, 2);

        let live = page.live_slot_ids().unwrap();
        assert_eq!(live.len(), 2);
    }

    #[test]
    fn page_compact_preserves_live_tuple_data() {
        let mut page = Page::new(0, PageType::Data);

        let mut tuples = Vec::new();
        let mut slots = Vec::new();
        for i in 0..10u32 {
            let mut t = TupleSlot::new(i, 3).unwrap();
            t.fixed_data = vec![i as u8; 8];
            t.add_var_column(format!("data_{i}").as_bytes()).unwrap();
            tuples.push(t);
        }
        for t in &tuples {
            slots.push(page.insert_tuple(t).unwrap());
        }

        // 删除偶数索引（0, 2, 4, 6, 8）
        for i in (0..10).step_by(2) {
            page.mark_tuple_deleted(slots[i], 100).unwrap();
        }

        page.compact().unwrap();

        // 5 个活跃 tuple，新 slot_ids 0..5
        assert_eq!(page.header.tuple_count, 5);
        let live = page.live_slot_ids().unwrap();
        assert_eq!(live, vec![0, 1, 2, 3, 4]);

        // 验证：原索引 1, 3, 5, 7, 9 的数据保持不变
        for (new_idx, &slot_id) in live.iter().enumerate() {
            let t = page.read_tuple(slot_id).unwrap();
            let original_idx = 2 * new_idx + 1;
            assert_eq!(t.header.xmin, original_idx as u32);
            assert_eq!(t.fixed_data, vec![original_idx as u8; 8]);
            assert_eq!(
                t.get_var_column(0).unwrap(),
                format!("data_{original_idx}").as_bytes()
            );
        }
    }

    #[test]
    fn page_compact_all_deleted_resets_page() {
        let mut page = Page::new(0, PageType::Data);

        let mut slots = Vec::new();
        for i in 0..3u32 {
            let t = TupleSlot::new(i, 1).unwrap();
            slots.push(page.insert_tuple(&t).unwrap());
        }
        for s in &slots {
            page.mark_tuple_deleted(*s, 100).unwrap();
        }

        page.compact().unwrap();

        assert_eq!(page.header.tuple_count, 0);
        assert_eq!(page.header.free_offset, 0);
        assert_eq!(page.available_for_tuple(), PAGE_BODY_SIZE);
        assert!(page.live_slot_ids().unwrap().is_empty());
    }

    #[test]
    fn page_compact_idempotent() {
        let mut page = Page::new(0, PageType::Data);

        for i in 0..5u32 {
            let mut t = TupleSlot::new(i, 1).unwrap();
            t.fixed_data = vec![i as u8; 16];
            page.insert_tuple(&t).unwrap();
        }
        page.mark_tuple_deleted(2, 100).unwrap();

        page.compact().unwrap();
        let count_1 = page.header.tuple_count;
        let free_1 = page.available_for_tuple();

        page.compact().unwrap();
        let count_2 = page.header.tuple_count;
        let free_2 = page.available_for_tuple();

        assert_eq!(count_1, count_2);
        assert_eq!(free_1, free_2);
    }

    #[test]
    fn page_compact_then_insert_works() {
        let mut page = Page::new(0, PageType::Data);

        // 插入 3 个 tuple，删除中间一个
        for i in 0..3u32 {
            let mut t = TupleSlot::new(i, 1).unwrap();
            t.fixed_data = vec![i as u8; 32];
            page.insert_tuple(&t).unwrap();
        }
        page.mark_tuple_deleted(1, 100).unwrap();

        page.compact().unwrap();
        assert_eq!(page.header.tuple_count, 2);

        // 再插入新 tuple
        let mut new_t = TupleSlot::new(99, 1).unwrap();
        new_t.fixed_data = vec![0xFF; 32];
        let new_slot = page.insert_tuple(&new_t).unwrap();

        // 验证新 tuple
        let back = page.read_tuple(new_slot).unwrap();
        assert_eq!(back, new_t);

        // 验证旧 tuple 仍可读取
        let live = page.live_slot_ids().unwrap();
        assert_eq!(live.len(), 3);
    }

    #[test]
    fn page_compact_preserves_checksum_validity() {
        let mut page = Page::new(0, PageType::Data);

        for i in 0..5u32 {
            let mut t = TupleSlot::new(i, 1).unwrap();
            t.fixed_data = vec![i as u8; 16];
            page.insert_tuple(&t).unwrap();
        }
        page.mark_tuple_deleted(2, 100).unwrap();
        page.update_checksum();

        page.compact().unwrap();
        page.update_checksum();

        // 编码 → 解码 → 校验
        let buf = page.encode();
        let back = Page::decode(&buf).unwrap();
        assert!(back.verify_checksum().is_ok());
        assert_eq!(back.header.tuple_count, 4);
    }
}
