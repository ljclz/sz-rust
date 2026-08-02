//! SQLite B-tree 页面结构编解码。
//!
//! # 页面类型
//!
//! | 类型代码 | 说明 |
//! |----------|------|
//! | 0x02 | 表内部页（Table Interior）|
//! | 0x05 | 表叶子页（Table Leaf）|
//! | 0x0a | 索引内部页（Index Interior）|
//! | 0x0d | 索引叶子页（Index Leaf）|
//!
//! # 页面头布局
//!
//! 叶子页头（8 字节）：
//! ```text
//! [page_type:1B] [first_freeblock:2B] [cell_count:2B] [cell_content_start:2B] [fragmented_free:1B]
//! ```
//!
//! 内部页头（12 字节）：
//! ```text
//! [page_type:1B] [first_freeblock:2B] [cell_count:2B] [cell_content_start:2B] [fragmented_free:1B] [right_most_ptr:4B]
//! ```
//!
//! # Cell Pointer Array
//!
//! 紧跟页面头，每项 2 字节大端，值为相对页首的偏移量。
//!
//! # Cell 格式
//!
//! - **表叶子 Cell**：`varint(payload_length) + varint(rowid) + payload [+ overflow_page(4B)]`
//! - **表内部 Cell**：`4B(left_child_page) + varint(rowid)`
//! - **索引叶子 Cell**：`varint(payload_length) + payload [+ overflow_page(4B)]`
//! - **索引内部 Cell**：`4B(left_child_page) + varint(payload_length) + payload [+ overflow_page(4B)]`

use crate::varint::{decode_varint, encode_varint};

// =====================================================================
//  常量
// =====================================================================

/// 表内部页类型
pub const PAGE_TYPE_TABLE_INTERIOR: u8 = 0x02;
/// 表叶子页类型
pub const PAGE_TYPE_TABLE_LEAF: u8 = 0x05;
/// 索引内部页类型
pub const PAGE_TYPE_INDEX_INTERIOR: u8 = 0x0a;
/// 索引叶子页类型
pub const PAGE_TYPE_INDEX_LEAF: u8 = 0x0d;

/// 叶子页头大小（字节）
pub const LEAF_HEADER_SIZE: usize = 8;
/// 内部页头大小（字节）
pub const INTERIOR_HEADER_SIZE: usize = 12;

// =====================================================================
//  Cell 结构体
// =====================================================================

/// 表叶子 Cell：存储一行数据。
///
/// 格式：`varint(payload_length) + varint(rowid) + payload`
///
/// 注意：本实现不处理溢出页（overflow page），假设 payload 完整存储在 cell 内。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableLeafCell {
    /// 行 ID（整数主键）
    pub rowid: i64,
    /// Record 字节（payload）
    pub payload: Vec<u8>,
}

impl TableLeafCell {
    /// 编码为字节序列。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // varint(payload_length)
        buf.extend(encode_varint(self.payload.len() as u64));
        // varint(rowid)（i64 转 u64 保持比特模式）
        buf.extend(encode_varint(self.rowid as u64));
        // payload
        buf.extend(&self.payload);
        buf
    }

    /// 从字节切片解码。
    ///
    /// # 返回
    /// - `Some(Self)`：解码成功
    /// - `None`：数据不完整
    pub fn decode(buf: &[u8]) -> Option<Self> {
        // 读取 payload_length
        let (payload_length, len1) = decode_varint(buf)?;
        // 读取 rowid
        let (rowid_u64, len2) = decode_varint(&buf[len1..])?;
        let rowid = rowid_u64 as i64;

        let payload_start = len1 + len2;
        let payload_len = payload_length as usize;

        // 检查 payload 是否完整（不含溢出页处理）
        if buf.len() < payload_start + payload_len {
            return None;
        }

        let payload = buf[payload_start..payload_start + payload_len].to_vec();
        Some(TableLeafCell { rowid, payload })
    }
}

/// 表内部 Cell：B-tree 内部节点的指针项。
///
/// 格式：`4B(left_child_page) + varint(rowid)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableInteriorCell {
    /// 左子页页号
    pub left_child_page: u32,
    /// 行 ID（分隔键）
    pub rowid: i64,
}

impl TableInteriorCell {
    /// 编码为字节序列。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // 4 字节大端左子页号
        buf.extend(&self.left_child_page.to_be_bytes());
        // varint(rowid)
        buf.extend(encode_varint(self.rowid as u64));
        buf
    }

    /// 从字节切片解码。
    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < 4 {
            return None;
        }
        let left_child_page = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let (rowid_u64, _) = decode_varint(&buf[4..])?;
        let rowid = rowid_u64 as i64;
        Some(TableInteriorCell {
            left_child_page,
            rowid,
        })
    }
}

// =====================================================================
//  索引 Cell 结构体
// =====================================================================

/// 索引叶子 Cell：存储一条索引键记录。
///
/// 格式：`varint(payload_length) + payload`
///
/// 注意：本实现不处理溢出页（overflow page），假设 payload 完整存储在 cell 内。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexLeafCell {
    /// Record 字节（payload，即索引键列 + rowid 用于唯一性）
    pub payload: Vec<u8>,
}

impl IndexLeafCell {
    /// 编码为字节序列。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // varint(payload_length)
        buf.extend(encode_varint(self.payload.len() as u64));
        // payload
        buf.extend(&self.payload);
        buf
    }

    /// 从字节切片解码。
    pub fn decode(buf: &[u8]) -> Option<Self> {
        let (payload_length, len1) = decode_varint(buf)?;
        let payload_len = payload_length as usize;
        if buf.len() < len1 + payload_len {
            return None;
        }
        let payload = buf[len1..len1 + payload_len].to_vec();
        Some(IndexLeafCell { payload })
    }
}

/// 索引内部 Cell：B-tree 内部节点的指针项 + 索引键。
///
/// 格式：`4B(left_child_page) + varint(payload_length) + payload`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInteriorCell {
    /// 左子页页号
    pub left_child_page: u32,
    /// Record 字节（payload，即索引键列 + rowid 用于唯一性）
    pub payload: Vec<u8>,
}

impl IndexInteriorCell {
    /// 编码为字节序列。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // 4 字节大端左子页号
        buf.extend(&self.left_child_page.to_be_bytes());
        // varint(payload_length)
        buf.extend(encode_varint(self.payload.len() as u64));
        // payload
        buf.extend(&self.payload);
        buf
    }

    /// 从字节切片解码。
    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < 4 {
            return None;
        }
        let left_child_page = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let (payload_length, len1) = decode_varint(&buf[4..])?;
        let payload_len = payload_length as usize;
        let payload_start = 4 + len1;
        if buf.len() < payload_start + payload_len {
            return None;
        }
        let payload = buf[payload_start..payload_start + payload_len].to_vec();
        Some(IndexInteriorCell {
            left_child_page,
            payload,
        })
    }
}

// =====================================================================
//  Cell 枚举
// =====================================================================

/// B-tree Cell 的统一表示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BtreeCell {
    /// 表叶子 Cell
    TableLeaf(TableLeafCell),
    /// 表内部 Cell
    TableInterior(TableInteriorCell),
    /// 索引叶子 Cell
    IndexLeaf(IndexLeafCell),
    /// 索引内部 Cell
    IndexInterior(IndexInteriorCell),
}

// =====================================================================
//  BtreePage 结构体
// =====================================================================

/// SQLite B-tree 页面。
///
/// 支持表 B-tree 和索引 B-tree 的叶子页与内部页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtreePage {
    /// 页面类型（0x02/0x05/0x0a/0x0d）
    pub page_type: u8,
    /// 第一个空闲块偏移（0 表示无空闲块）
    pub first_freeblock: u16,
    /// Cell 数量
    pub cell_count: u16,
    /// Cell 内容区起始偏移（0 表示 65536）
    pub cell_content_start: u16,
    /// 碎片空闲字节数
    pub fragmented_free_bytes: u8,
    /// 最右子页指针（仅内部页有效，叶子页为 0）
    pub right_most_pointer: u32,
    /// Cell 列表
    pub cells: Vec<BtreeCell>,
}

impl BtreePage {
    /// 构造一个空的表叶子页。
    pub fn new_table_leaf() -> Self {
        BtreePage {
            page_type: PAGE_TYPE_TABLE_LEAF,
            first_freeblock: 0,
            cell_count: 0,
            cell_content_start: 0,
            fragmented_free_bytes: 0,
            right_most_pointer: 0,
            cells: Vec::new(),
        }
    }

    /// 构造一个空的表内部页。
    pub fn new_table_interior(right_most_pointer: u32) -> Self {
        BtreePage {
            page_type: PAGE_TYPE_TABLE_INTERIOR,
            first_freeblock: 0,
            cell_count: 0,
            cell_content_start: 0,
            fragmented_free_bytes: 0,
            right_most_pointer,
            cells: Vec::new(),
        }
    }

    /// 构造一个空的索引叶子页。
    pub fn new_index_leaf() -> Self {
        BtreePage {
            page_type: PAGE_TYPE_INDEX_LEAF,
            first_freeblock: 0,
            cell_count: 0,
            cell_content_start: 0,
            fragmented_free_bytes: 0,
            right_most_pointer: 0,
            cells: Vec::new(),
        }
    }

    /// 构造一个空的索引内部页。
    pub fn new_index_interior(right_most_pointer: u32) -> Self {
        BtreePage {
            page_type: PAGE_TYPE_INDEX_INTERIOR,
            first_freeblock: 0,
            cell_count: 0,
            cell_content_start: 0,
            fragmented_free_bytes: 0,
            right_most_pointer,
            cells: Vec::new(),
        }
    }

    /// 是否为内部页。
    pub fn is_interior(&self) -> bool {
        matches!(
            self.page_type,
            PAGE_TYPE_TABLE_INTERIOR | PAGE_TYPE_INDEX_INTERIOR
        )
    }

    /// 是否为索引页。
    pub fn is_index(&self) -> bool {
        matches!(
            self.page_type,
            PAGE_TYPE_INDEX_LEAF | PAGE_TYPE_INDEX_INTERIOR
        )
    }

    /// 页面头大小（8 字节叶子，12 字节内部）。
    pub fn header_size(&self) -> usize {
        if self.is_interior() {
            INTERIOR_HEADER_SIZE
        } else {
            LEAF_HEADER_SIZE
        }
    }

    // -----------------------------------------------------------------
    //  解码
    // -----------------------------------------------------------------

    /// 从字节切片解码 B-tree 页面。
    ///
    /// # 参数
    /// - `buf`：完整页面字节切片
    /// - `header_offset`：B-tree 页面头在 `buf` 中的起始偏移
    ///   （page 1 为 100，其他页为 0）
    ///
    /// # 返回
    /// - `Some(Self)`：解码成功
    /// - `None`：数据不完整或格式错误
    pub fn decode(buf: &[u8], header_offset: usize) -> Option<Self> {
        // 至少需要 8 字节页面头
        if buf.len() < header_offset + LEAF_HEADER_SIZE {
            return None;
        }

        // 读取页面头字段
        let page_type = buf[header_offset];
        let first_freeblock = u16::from_be_bytes([buf[header_offset + 1], buf[header_offset + 2]]);
        let cell_count = u16::from_be_bytes([buf[header_offset + 3], buf[header_offset + 4]]);
        let cell_content_start_raw =
            u16::from_be_bytes([buf[header_offset + 5], buf[header_offset + 6]]);
        // 0 表示 65536
        let cell_content_start = cell_content_start_raw;
        let fragmented_free_bytes = buf[header_offset + 7];

        // 判断页面类型并读取 right_most_pointer
        let is_interior = matches!(
            page_type,
            PAGE_TYPE_TABLE_INTERIOR | PAGE_TYPE_INDEX_INTERIOR
        );
        let header_size = if is_interior {
            INTERIOR_HEADER_SIZE
        } else {
            LEAF_HEADER_SIZE
        };

        if buf.len() < header_offset + header_size {
            return None;
        }

        let right_most_pointer = if is_interior {
            u32::from_be_bytes([
                buf[header_offset + 8],
                buf[header_offset + 9],
                buf[header_offset + 10],
                buf[header_offset + 11],
            ])
        } else {
            0
        };

        // 读取 cell pointer array
        let cell_ptr_start = header_offset + header_size;
        let n = cell_count as usize;
        if buf.len() < cell_ptr_start + 2 * n {
            return None;
        }

        let mut cell_pointers = Vec::with_capacity(n);
        for i in 0..n {
            let ptr =
                u16::from_be_bytes([buf[cell_ptr_start + 2 * i], buf[cell_ptr_start + 2 * i + 1]]);
            cell_pointers.push(ptr as usize);
        }

        // 根据 page_type 解码各 cell
        let mut cells = Vec::with_capacity(n);
        match page_type {
            PAGE_TYPE_TABLE_LEAF => {
                for &ptr in &cell_pointers {
                    if ptr >= buf.len() {
                        return None;
                    }
                    let cell = TableLeafCell::decode(&buf[ptr..])?;
                    cells.push(BtreeCell::TableLeaf(cell));
                }
            }
            PAGE_TYPE_TABLE_INTERIOR => {
                for &ptr in &cell_pointers {
                    if ptr >= buf.len() {
                        return None;
                    }
                    let cell = TableInteriorCell::decode(&buf[ptr..])?;
                    cells.push(BtreeCell::TableInterior(cell));
                }
            }
            PAGE_TYPE_INDEX_LEAF => {
                for &ptr in &cell_pointers {
                    if ptr >= buf.len() {
                        return None;
                    }
                    let cell = IndexLeafCell::decode(&buf[ptr..])?;
                    cells.push(BtreeCell::IndexLeaf(cell));
                }
            }
            PAGE_TYPE_INDEX_INTERIOR => {
                for &ptr in &cell_pointers {
                    if ptr >= buf.len() {
                        return None;
                    }
                    let cell = IndexInteriorCell::decode(&buf[ptr..])?;
                    cells.push(BtreeCell::IndexInterior(cell));
                }
            }
            // 未知页面类型：返回空 cells（保持向后兼容）
            _ => {}
        }

        Some(BtreePage {
            page_type,
            first_freeblock,
            cell_count,
            cell_content_start,
            fragmented_free_bytes,
            right_most_pointer,
            cells,
        })
    }

    // -----------------------------------------------------------------
    //  编码
    // -----------------------------------------------------------------

    /// 编码为完整页面字节序列。
    ///
    /// # 参数
    /// - `page_size`：页面大小（字节）
    /// - `header_offset`：B-tree 页面头的起始偏移
    ///   （page 1 为 100，其他页为 0）
    ///
    /// # 返回
    /// 长度为 `page_size` 的字节向量。前 `header_offset` 字节为零（供调用方覆写文件头）。
    pub fn encode(&self, page_size: usize, header_offset: usize) -> Vec<u8> {
        let header_size = self.header_size();
        let mut buf = vec![0u8; page_size];

        // 编码所有 cell 为字节
        let mut cell_bytes: Vec<Vec<u8>> = Vec::new();
        for cell in &self.cells {
            match cell {
                BtreeCell::TableLeaf(c) => cell_bytes.push(c.encode()),
                BtreeCell::TableInterior(c) => cell_bytes.push(c.encode()),
                BtreeCell::IndexLeaf(c) => cell_bytes.push(c.encode()),
                BtreeCell::IndexInterior(c) => cell_bytes.push(c.encode()),
            }
        }

        // Cell pointer array 起始位置
        let cell_ptr_start = header_offset + header_size;
        let n = cell_bytes.len();

        // 从页面末尾向前放置 cell 内容
        let mut content_end = page_size;
        let mut cell_offsets: Vec<usize> = Vec::with_capacity(n);
        for bytes in &cell_bytes {
            content_end = content_end.saturating_sub(bytes.len());
            cell_offsets.push(content_end);
        }

        // 计算 cell_content_start
        let cell_content_start_val = if cell_bytes.is_empty() {
            // 空页面：cell_content_start = page_size（或 0 表示 65536）
            if page_size >= 65536 {
                0
            } else {
                page_size as u16
            }
        } else {
            if content_end >= 65536 {
                0
            } else {
                content_end as u16
            }
        };

        // 写入页面头
        buf[header_offset] = self.page_type;
        buf[header_offset + 1..header_offset + 3]
            .copy_from_slice(&self.first_freeblock.to_be_bytes());
        buf[header_offset + 3..header_offset + 5].copy_from_slice(&(n as u16).to_be_bytes());
        buf[header_offset + 5..header_offset + 7]
            .copy_from_slice(&cell_content_start_val.to_be_bytes());
        buf[header_offset + 7] = self.fragmented_free_bytes;

        if self.is_interior() {
            buf[header_offset + 8..header_offset + 12]
                .copy_from_slice(&self.right_most_pointer.to_be_bytes());
        }

        // 写入 cell pointer array（大端 2 字节 × n）
        for (i, &offset) in cell_offsets.iter().enumerate() {
            let off = offset as u16;
            buf[cell_ptr_start + 2 * i..cell_ptr_start + 2 * i + 2]
                .copy_from_slice(&off.to_be_bytes());
        }

        // 写入 cell 内容
        for (i, bytes) in cell_bytes.iter().enumerate() {
            let offset = cell_offsets[i];
            buf[offset..offset + bytes.len()].copy_from_slice(bytes);
        }

        buf
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_types::value::Value;

    // -----------------------------------------------------------------
    //  TableLeafCell 编解码
    // -----------------------------------------------------------------

    #[test]
    fn leaf_cell_encode_decode_roundtrip() {
        let cell = TableLeafCell {
            rowid: 42,
            payload: vec![0x01, 0x02, 0x03],
        };
        let encoded = cell.encode();
        let decoded = TableLeafCell::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, cell);
    }

    #[test]
    fn leaf_cell_empty_payload() {
        let cell = TableLeafCell {
            rowid: 1,
            payload: vec![],
        };
        let encoded = cell.encode();
        let decoded = TableLeafCell::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, cell);
    }

    #[test]
    fn leaf_cell_large_rowid() {
        let cell = TableLeafCell {
            rowid: i64::MAX,
            payload: vec![0xDE, 0xAD],
        };
        let encoded = cell.encode();
        let decoded = TableLeafCell::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, cell);
    }

    #[test]
    fn leaf_cell_negative_rowid() {
        let cell = TableLeafCell {
            rowid: -1,
            payload: vec![0x00],
        };
        let encoded = cell.encode();
        let decoded = TableLeafCell::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded.rowid, -1);
        assert_eq!(decoded.payload, vec![0x00]);
    }

    #[test]
    fn leaf_cell_decode_empty_buffer_returns_none() {
        assert_eq!(TableLeafCell::decode(&[]), None);
    }

    // -----------------------------------------------------------------
    //  TableInteriorCell 编解码
    // -----------------------------------------------------------------

    #[test]
    fn interior_cell_encode_decode_roundtrip() {
        let cell = TableInteriorCell {
            left_child_page: 5,
            rowid: 100,
        };
        let encoded = cell.encode();
        let decoded = TableInteriorCell::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, cell);
    }

    #[test]
    fn interior_cell_large_page_number() {
        let cell = TableInteriorCell {
            left_child_page: u32::MAX,
            rowid: i64::MAX,
        };
        let encoded = cell.encode();
        let decoded = TableInteriorCell::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, cell);
    }

    #[test]
    fn interior_cell_decode_short_buffer_returns_none() {
        assert_eq!(TableInteriorCell::decode(&[0, 0, 0]), None);
    }

    // -----------------------------------------------------------------
    //  BtreePage 空页面
    // -----------------------------------------------------------------

    #[test]
    fn new_table_leaf_is_empty() {
        let page = BtreePage::new_table_leaf();
        assert_eq!(page.page_type, PAGE_TYPE_TABLE_LEAF);
        assert_eq!(page.cell_count, 0);
        assert!(page.cells.is_empty());
        assert!(!page.is_interior());
    }

    #[test]
    fn new_table_interior_has_right_pointer() {
        let page = BtreePage::new_table_interior(42);
        assert_eq!(page.page_type, PAGE_TYPE_TABLE_INTERIOR);
        assert_eq!(page.right_most_pointer, 42);
        assert!(page.is_interior());
    }

    // -----------------------------------------------------------------
    //  BtreePage 编解码往返（叶子页）
    // -----------------------------------------------------------------

    #[test]
    fn page_encode_decode_empty_leaf_roundtrip() {
        let page = BtreePage::new_table_leaf();
        let page_size = 4096;
        let encoded = page.encode(page_size, 0);
        assert_eq!(encoded.len(), page_size);

        let decoded = BtreePage::decode(&encoded, 0).expect("decode should succeed");
        assert_eq!(decoded.page_type, PAGE_TYPE_TABLE_LEAF);
        assert_eq!(decoded.cell_count, 0);
        assert!(decoded.cells.is_empty());
    }

    #[test]
    fn page_encode_decode_single_cell_leaf_roundtrip() {
        use crate::record::encode_record;
        let values = vec![Value::Int64(42), Value::Text("hello".to_string())];
        let payload = encode_record(&values);

        let cell = TableLeafCell { rowid: 1, payload };
        let mut page = BtreePage::new_table_leaf();
        page.cells.push(BtreeCell::TableLeaf(cell));

        let page_size = 4096;
        let encoded = page.encode(page_size, 0);
        let decoded = BtreePage::decode(&encoded, 0).expect("decode should succeed");

        assert_eq!(decoded.page_type, PAGE_TYPE_TABLE_LEAF);
        assert_eq!(decoded.cells.len(), 1);

        if let BtreeCell::TableLeaf(leaf) = &decoded.cells[0] {
            assert_eq!(leaf.rowid, 1);
            // 验证 record 内容
            use crate::record::decode_record;
            let decoded_values = decode_record(&leaf.payload).expect("record decode");
            assert_eq!(
                decoded_values,
                vec![Value::Int64(42), Value::Text("hello".to_string())]
            );
        } else {
            panic!("expected TableLeaf cell");
        }
    }

    #[test]
    fn page_encode_decode_multiple_cells_leaf_roundtrip() {
        use crate::record::encode_record;
        let mut page = BtreePage::new_table_leaf();

        for i in 1..=5 {
            let values = vec![Value::Int64(i)];
            let payload = encode_record(&values);
            page.cells
                .push(BtreeCell::TableLeaf(TableLeafCell { rowid: i, payload }));
        }

        let page_size = 4096;
        let encoded = page.encode(page_size, 0);
        let decoded = BtreePage::decode(&encoded, 0).expect("decode should succeed");

        assert_eq!(decoded.cells.len(), 5);
        for (i, cell) in decoded.cells.iter().enumerate() {
            if let BtreeCell::TableLeaf(leaf) = cell {
                assert_eq!(leaf.rowid, (i + 1) as i64);
            } else {
                panic!("expected TableLeaf cell");
            }
        }
    }

    // -----------------------------------------------------------------
    //  BtreePage 编解码往返（内部页）
    // -----------------------------------------------------------------

    #[test]
    fn page_encode_decode_interior_roundtrip() {
        let mut page = BtreePage::new_table_interior(99); // right_most = 99
        page.cells.push(BtreeCell::TableInterior(TableInteriorCell {
            left_child_page: 2,
            rowid: 10,
        }));
        page.cells.push(BtreeCell::TableInterior(TableInteriorCell {
            left_child_page: 3,
            rowid: 20,
        }));

        let page_size = 4096;
        let encoded = page.encode(page_size, 0);
        let decoded = BtreePage::decode(&encoded, 0).expect("decode should succeed");

        assert_eq!(decoded.page_type, PAGE_TYPE_TABLE_INTERIOR);
        assert_eq!(decoded.right_most_pointer, 99);
        assert_eq!(decoded.cells.len(), 2);

        if let BtreeCell::TableInterior(c) = &decoded.cells[0] {
            assert_eq!(c.left_child_page, 2);
            assert_eq!(c.rowid, 10);
        } else {
            panic!("expected TableInterior cell");
        }
    }

    // -----------------------------------------------------------------
    //  Page 1 编解码（带文件头偏移）
    // -----------------------------------------------------------------

    #[test]
    fn page1_encode_decode_with_header_offset() {
        let mut page = BtreePage::new_table_leaf();
        page.cells.push(BtreeCell::TableLeaf(TableLeafCell {
            rowid: 1,
            payload: vec![0x01],
        }));

        let page_size = 4096;
        let header_offset = 100; // 文件头 100 字节
        let encoded = page.encode(page_size, header_offset);

        // 前 100 字节应为零（供文件头覆写）
        assert!(encoded[..100].iter().all(|&b| b == 0));

        // 解码时应跳过文件头
        let decoded = BtreePage::decode(&encoded, header_offset).expect("decode should succeed");
        assert_eq!(decoded.page_type, PAGE_TYPE_TABLE_LEAF);
        assert_eq!(decoded.cells.len(), 1);
    }

    // -----------------------------------------------------------------
    //  cell_content_start 测试
    // -----------------------------------------------------------------

    #[test]
    fn cell_content_start_correct_for_non_empty_page() {
        let mut page = BtreePage::new_table_leaf();
        page.cells.push(BtreeCell::TableLeaf(TableLeafCell {
            rowid: 1,
            payload: vec![0x01, 0x02, 0x03],
        }));

        let encoded = page.encode(4096, 0);
        // cell_content_start 应在偏移 5 处读取（2 字节大端）
        let cs = u16::from_be_bytes([encoded[5], encoded[6]]);
        assert!(cs > 8 + 2); // 大于 header + cell_ptr_array
        assert!(cs < 4096); // 小于 page_size
    }

    #[test]
    fn cell_content_start_zero_for_empty_page_65536() {
        let page = BtreePage::new_table_leaf();
        let encoded = page.encode(65536, 0);
        let cs = u16::from_be_bytes([encoded[5], encoded[6]]);
        assert_eq!(cs, 0); // 0 表示 65536
    }

    #[test]
    fn cell_content_start_page_size_for_empty_small_page() {
        let page = BtreePage::new_table_leaf();
        let encoded = page.encode(4096, 0);
        let cs = u16::from_be_bytes([encoded[5], encoded[6]]);
        assert_eq!(cs, 4096);
    }

    // -----------------------------------------------------------------
    //  cell pointer array 正确性
    // -----------------------------------------------------------------

    #[test]
    fn cell_pointers_are_valid_offsets() {
        use crate::record::encode_record;
        let mut page = BtreePage::new_table_leaf();
        for i in 1..=3 {
            let payload = encode_record(&[Value::Int64(i)]);
            page.cells
                .push(BtreeCell::TableLeaf(TableLeafCell { rowid: i, payload }));
        }

        let encoded = page.encode(4096, 0);
        let cell_count = u16::from_be_bytes([encoded[3], encoded[4]]);
        assert_eq!(cell_count, 3);

        // 读取 cell pointer array
        let ptr_start = 8; // header_size for leaf
        for i in 0..3 {
            let ptr =
                u16::from_be_bytes([encoded[ptr_start + 2 * i], encoded[ptr_start + 2 * i + 1]])
                    as usize;
            // 每个指针应指向有效的 cell 数据
            assert!(ptr > ptr_start + 6); // 大于 header + cell_ptr_array
            assert!(ptr < 4096);
            // 在该偏移处应能解码出 TableLeafCell
            let cell = TableLeafCell::decode(&encoded[ptr..]);
            assert!(cell.is_some(), "cell at pointer {ptr} should be decodable");
        }
    }

    // -----------------------------------------------------------------
    //  错误处理
    // -----------------------------------------------------------------

    #[test]
    fn decode_buffer_too_short_returns_none() {
        assert_eq!(BtreePage::decode(&[0x0d], 0), None);
    }

    #[test]
    fn decode_invalid_page_type_returns_page_with_no_cells() {
        // 构造一个未知页面类型的页面头
        let mut buf = vec![0u8; 4096];
        buf[0] = 0xFF; // 未知类型
        buf[3] = 0; // cell_count = 0
        let page = BtreePage::decode(&buf, 0);
        // 应该能解码（只是没有 cells）
        assert!(page.is_some());
        let page = page.unwrap();
        assert_eq!(page.page_type, 0xFF);
        assert!(page.cells.is_empty());
    }

    // -----------------------------------------------------------------
    //  小页面测试
    // -----------------------------------------------------------------

    #[test]
    fn small_page_512_bytes_roundtrip() {
        let mut page = BtreePage::new_table_leaf();
        page.cells.push(BtreeCell::TableLeaf(TableLeafCell {
            rowid: 1,
            payload: vec![0x01],
        }));

        let encoded = page.encode(512, 0);
        let decoded = BtreePage::decode(&encoded, 0).expect("decode should succeed");
        assert_eq!(decoded.cells.len(), 1);
    }

    // -----------------------------------------------------------------
    //  页面头字段验证
    // -----------------------------------------------------------------

    #[test]
    fn page_header_fields_correct() {
        let mut page = BtreePage::new_table_leaf();
        page.first_freeblock = 0;
        page.fragmented_free_bytes = 0;
        page.cells.push(BtreeCell::TableLeaf(TableLeafCell {
            rowid: 1,
            payload: vec![0xAB],
        }));

        let encoded = page.encode(4096, 0);
        // 验证页面头字段
        assert_eq!(encoded[0], PAGE_TYPE_TABLE_LEAF); // page_type
        assert_eq!(encoded[1], 0); // first_freeblock high
        assert_eq!(encoded[2], 0); // first_freeblock low
        assert_eq!(encoded[3], 0); // cell_count high
        assert_eq!(encoded[4], 1); // cell_count low
        assert_eq!(encoded[7], 0); // fragmented_free_bytes
    }

    #[test]
    fn interior_page_header_has_right_most_pointer() {
        let page = BtreePage::new_table_interior(0xDEAD_BEEF);
        let encoded = page.encode(4096, 0);
        // right_most_pointer 在偏移 8..12
        let rmp = u32::from_be_bytes([encoded[8], encoded[9], encoded[10], encoded[11]]);
        assert_eq!(rmp, 0xDEAD_BEEF);
    }

    #[test]
    fn leaf_page_header_has_no_right_most_pointer() {
        let page = BtreePage::new_table_leaf();
        let encoded = page.encode(4096, 0);
        // 叶子页头只有 8 字节，偏移 8..12 应为 cell pointer array 或零
        // right_most_pointer 字段不存在
        assert_eq!(encoded[8], 0); // cell_ptr_array[0] high 或零
    }

    // -----------------------------------------------------------------
    //  索引 Cell 编解码测试
    // -----------------------------------------------------------------

    #[test]
    fn index_leaf_cell_encode_decode_roundtrip() {
        let cell = IndexLeafCell {
            payload: vec![0x01, 0x02, 0x03, 0x04],
        };
        let encoded = cell.encode();
        let decoded = IndexLeafCell::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, cell);
    }

    #[test]
    fn index_leaf_cell_empty_payload() {
        let cell = IndexLeafCell { payload: vec![] };
        let encoded = cell.encode();
        let decoded = IndexLeafCell::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, cell);
    }

    #[test]
    fn index_leaf_cell_decode_empty_buffer_returns_none() {
        assert_eq!(IndexLeafCell::decode(&[]), None);
    }

    #[test]
    fn index_interior_cell_encode_decode_roundtrip() {
        let cell = IndexInteriorCell {
            left_child_page: 7,
            payload: vec![0xAA, 0xBB, 0xCC],
        };
        let encoded = cell.encode();
        let decoded = IndexInteriorCell::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, cell);
    }

    #[test]
    fn index_interior_cell_large_page_number() {
        let cell = IndexInteriorCell {
            left_child_page: u32::MAX,
            payload: vec![0x00],
        };
        let encoded = cell.encode();
        let decoded = IndexInteriorCell::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded, cell);
    }

    #[test]
    fn index_interior_cell_decode_short_buffer_returns_none() {
        assert_eq!(IndexInteriorCell::decode(&[0, 0, 0]), None);
    }

    // -----------------------------------------------------------------
    //  索引页面编解码往返测试
    // -----------------------------------------------------------------

    #[test]
    fn index_leaf_page_encode_decode_roundtrip() {
        let mut page = BtreePage::new_index_leaf();
        page.cells.push(BtreeCell::IndexLeaf(IndexLeafCell {
            payload: vec![0x01, 0x02],
        }));
        page.cells.push(BtreeCell::IndexLeaf(IndexLeafCell {
            payload: vec![0x03, 0x04, 0x05],
        }));

        let encoded = page.encode(4096, 0);
        let decoded = BtreePage::decode(&encoded, 0).expect("decode should succeed");

        assert_eq!(decoded.page_type, PAGE_TYPE_INDEX_LEAF);
        assert!(!decoded.is_interior());
        assert!(decoded.is_index());
        assert_eq!(decoded.cells.len(), 2);

        if let BtreeCell::IndexLeaf(c) = &decoded.cells[0] {
            assert_eq!(c.payload, vec![0x01, 0x02]);
        } else {
            panic!("expected IndexLeaf cell");
        }
        if let BtreeCell::IndexLeaf(c) = &decoded.cells[1] {
            assert_eq!(c.payload, vec![0x03, 0x04, 0x05]);
        } else {
            panic!("expected IndexLeaf cell");
        }
    }

    #[test]
    fn index_interior_page_encode_decode_roundtrip() {
        let mut page = BtreePage::new_index_interior(99); // right_most = 99
        page.cells.push(BtreeCell::IndexInterior(IndexInteriorCell {
            left_child_page: 2,
            payload: vec![0x10, 0x20],
        }));
        page.cells.push(BtreeCell::IndexInterior(IndexInteriorCell {
            left_child_page: 3,
            payload: vec![0x30, 0x40],
        }));

        let encoded = page.encode(4096, 0);
        let decoded = BtreePage::decode(&encoded, 0).expect("decode should succeed");

        assert_eq!(decoded.page_type, PAGE_TYPE_INDEX_INTERIOR);
        assert!(decoded.is_interior());
        assert!(decoded.is_index());
        assert_eq!(decoded.right_most_pointer, 99);
        assert_eq!(decoded.cells.len(), 2);

        if let BtreeCell::IndexInterior(c) = &decoded.cells[0] {
            assert_eq!(c.left_child_page, 2);
            assert_eq!(c.payload, vec![0x10, 0x20]);
        } else {
            panic!("expected IndexInterior cell");
        }
        if let BtreeCell::IndexInterior(c) = &decoded.cells[1] {
            assert_eq!(c.left_child_page, 3);
            assert_eq!(c.payload, vec![0x30, 0x40]);
        } else {
            panic!("expected IndexInterior cell");
        }
    }

    #[test]
    fn index_leaf_page_empty_roundtrip() {
        let page = BtreePage::new_index_leaf();
        let encoded = page.encode(4096, 0);
        let decoded = BtreePage::decode(&encoded, 0).expect("decode should succeed");
        assert_eq!(decoded.page_type, PAGE_TYPE_INDEX_LEAF);
        assert!(decoded.cells.is_empty());
    }

    #[test]
    fn index_page_with_record_payload_roundtrip() {
        use crate::record::encode_record;
        // 索引键 = (col1, rowid) 用于唯一性
        let key_values = vec![Value::Int64(42), Value::Int64(1)];
        let payload = encode_record(&key_values);

        let mut page = BtreePage::new_index_leaf();
        page.cells.push(BtreeCell::IndexLeaf(IndexLeafCell {
            payload: payload.clone(),
        }));

        let encoded = page.encode(4096, 0);
        let decoded = BtreePage::decode(&encoded, 0).expect("decode should succeed");

        if let BtreeCell::IndexLeaf(c) = &decoded.cells[0] {
            assert_eq!(c.payload, payload);
            use crate::record::decode_record;
            let decoded_values = decode_record(&c.payload).expect("record decode");
            assert_eq!(decoded_values, key_values);
        } else {
            panic!("expected IndexLeaf cell");
        }
    }
}
