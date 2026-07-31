//! SzRSQL 堆表存储 — Batch 2: P0-B 分页存储引擎。
//!
//! 提供 `HeapTable`：基于 BufferPool 的多页表存储，
//! 将执行器的 `Row (Vec<Value>)` 映射到 Page 内的 TupleSlot 二进制格式。
//!
//! # 架构
//!
//! ```text
//! HeapTable
//! ├── meta: HeapMeta (page_list, row_count, next_page_id)
//! ├── buffer_pool: Arc<BufferPool>
//! └── pages: Vec<u32> (heap page IDs)
//!
//! 每个 Heap Page (8KB):
//! ├── PageHeader (48B)
//! ├── Tuple data (从前往后生长)
//! ├── Free space
//! └── Slot directory (从后往前生长)
//! ```

use crate::buffer::{BufferError, BufferPool};
use crate::page::{Page, PageType};
use crate::tuple::TupleSlot;
use std::sync::Arc;
use tracing::{debug, trace};

// =====================================================================
//  Row 编解码：Vec<Value> <-> TupleSlot
// =====================================================================

/// 将 `szrsql_types::Value` 编码为紧凑二进制（用于 TupleSlot.fixed_data / var_data）
///
/// 编码格式：[1B type_tag][payload]
/// - Null: tag=0, 无 payload
/// - Int64: tag=1, 8B LE
/// - Float64: tag=2, 8B LE
/// - Bool: tag=3, 1B (0/1)
/// - Date: tag=4, 4B LE
/// - Timestamp: tag=5, 8B LE
/// - Text: tag=10, 变长（存入 var_data）
/// - Blob: tag=11, 变长（存入 var_data）
/// - Json: tag=12, 变长（存入 var_data）
/// - 其他: tag=255, 变长 JSON 序列化（存入 var_data）
pub fn encode_value(value: &szrsql_types::value::Value) -> (u8, Vec<u8>, bool) {
    use szrsql_types::value::Value;
    match value {
        Value::Null => (0, vec![], false),
        Value::Int64(v) => (1, v.to_le_bytes().to_vec(), false),
        Value::Float64(v) => (2, v.to_le_bytes().to_vec(), false),
        Value::Bool(v) => (3, vec![*v as u8], false),
        Value::Date(v) => (4, v.to_le_bytes().to_vec(), false),
        Value::Timestamp(v) => (5, v.to_le_bytes().to_vec(), false),
        Value::Text(s) => (10, s.as_bytes().to_vec(), true),
        Value::Blob(b) => (11, b.clone(), true),
        Value::Json(j) => (12, serde_json::to_vec(j).unwrap_or_default(), true),
        // Decimal, Array, Enum, Range, TsVector, TsQuery → JSON 序列化
        other => (
            255,
            serde_json::to_vec(other).unwrap_or_default(),
            true,
        ),
    }
}

/// 从二进制解码回 `Value`
pub fn decode_value(tag: u8, data: &[u8]) -> szrsql_types::value::Value {
    use szrsql_types::value::Value;
    match tag {
        0 => Value::Null,
        1 => {
            let v = i64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
            Value::Int64(v)
        }
        2 => {
            let v = f64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
            Value::Float64(v)
        }
        3 => Value::Bool(data.first() == Some(&1)),
        4 => {
            let v = i32::from_le_bytes(data[0..4].try_into().unwrap_or([0; 4]));
            Value::Date(v)
        }
        5 => {
            let v = i64::from_le_bytes(data[0..8].try_into().unwrap_or([0; 8]));
            Value::Timestamp(v)
        }
        10 => Value::Text(String::from_utf8_lossy(data).into_owned()),
        11 => Value::Blob(data.to_vec()),
        12 => {
            let j: serde_json::Value = serde_json::from_slice(data).unwrap_or_default();
            Value::Json(j)
        }
        255 => {
            serde_json::from_slice(data).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    }
}

/// 将一行 `Vec<Value>` 编码为 TupleSlot
///
/// 布局：
/// - fixed_data: 每列 [1B tag][8B payload]（定长部分，Null/Int64/Float64/Bool/Date/Timestamp）
/// - var_data: 变长列数据（Text/Blob/Json 等）
/// - var_offsets: 变长列在 var_data 中的 (offset, length)
pub fn row_to_tuple(row: &[szrsql_types::value::Value], xmin: u32) -> TupleSlot {
    let col_count = row.len() as u16;
    let mut tuple = TupleSlot::new(xmin, col_count).unwrap_or_else(|_| {
        TupleSlot::new(xmin, 0).unwrap()
    });

    let mut fixed = Vec::with_capacity(row.len() * 9);
    for value in row {
        let (tag, payload, is_var) = encode_value(value);
        if value == &szrsql_types::value::Value::Null {
            // Null: tag=0, 8B zero padding（保持定长对齐）
            fixed.push(0u8);
            fixed.extend_from_slice(&[0u8; 8]);
        } else if is_var {
            // 变长列：fixed 区存 tag + var_index（u64 LE 占位，实际是 var_offsets 索引）
            let var_idx = tuple.var_offsets.len();
            let _ = tuple.add_var_column(&payload);
            fixed.push(tag);
            fixed.extend_from_slice(&(var_idx as u64).to_le_bytes());
        } else {
            // 定长列：tag + payload（补齐到 8B）
            fixed.push(tag);
            let mut padded = [0u8; 8];
            let len = payload.len().min(8);
            padded[..len].copy_from_slice(&payload[..len]);
            fixed.extend_from_slice(&padded);
        }
    }
    tuple.fixed_data = fixed;

    // 设置 null_bitmap
    for (i, value) in row.iter().enumerate() {
        if value == &szrsql_types::value::Value::Null {
            let _ = tuple.header.set_null(i);
        }
    }

    tuple
}

/// 从 TupleSlot 解码回一行 `Vec<Value>`
pub fn tuple_to_row(tuple: &TupleSlot) -> Vec<szrsql_types::value::Value> {
    let col_count = tuple.header.col_count as usize;
    let mut row = Vec::with_capacity(col_count);
    let mut var_idx = 0usize;

    for i in 0..col_count {
        // 检查 null_bitmap
        if tuple.header.is_null(i).unwrap_or(false) {
            row.push(szrsql_types::value::Value::Null);
            // 跳过 fixed 区的 9 字节（tag + 8B padding）
            continue;
        }

        let fixed_offset = i * 9;
        if fixed_offset + 9 > tuple.fixed_data.len() {
            row.push(szrsql_types::value::Value::Null);
            continue;
        }

        let tag = tuple.fixed_data[fixed_offset];
        let payload = &tuple.fixed_data[fixed_offset + 1..fixed_offset + 9];

        match tag {
            0 => row.push(szrsql_types::value::Value::Null),
            10 | 11 | 12 | 255 => {
                // 变长列：payload 中存的是 var_offsets 索引
                let idx = u64::from_le_bytes(payload.try_into().unwrap_or([0; 8])) as usize;
                if let Ok(var_data) = tuple.get_var_column(idx) {
                    row.push(decode_value(tag, var_data));
                } else {
                    row.push(szrsql_types::value::Value::Null);
                }
                var_idx += 1;
            }
            _ => {
                row.push(decode_value(tag, payload));
            }
        }
    }
    let _ = var_idx;
    row
}

// =====================================================================
//  HeapTable — 基于 BufferPool 的多页表存储
// =====================================================================

/// 堆表错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HeapError {
    #[error("buffer pool error: {0}")]
    BufferPool(#[from] BufferError),
    #[error("page error: {0}")]
    Page(String),
    #[error("row_id {0} not found")]
    RowNotFound(usize),
    #[error("table is full: no space for new page")]
    TableFull,
}

/// 堆表：基于 BufferPool 的多页行存储
///
/// 每个 heap page 存储多个 TupleSlot，通过 BufferPool 管理页面缓存。
/// row_id 编码为 `(page_index << 16) | slot_id`，支持直接定位。
pub struct HeapTable {
    /// 表名
    name: String,
    /// 列数（用于编解码）
    col_count: u16,
    /// BufferPool 引用
    buffer_pool: Arc<BufferPool>,
    /// 已分配的 heap page ID 列表（按顺序）
    page_ids: Vec<u32>,
    /// 下一个可分配的 page_id
    next_page_id: u32,
    /// 活跃行数（不含已删除）
    row_count: usize,
}

impl HeapTable {
    /// 创建新堆表
    pub fn new(name: &str, col_count: u16, buffer_pool: Arc<BufferPool>) -> Self {
        Self {
            name: name.to_string(),
            col_count,
            buffer_pool,
            page_ids: Vec::new(),
            next_page_id: 0,
            row_count: 0,
        }
    }

    /// 表名
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 活跃行数
    pub fn row_count(&self) -> usize {
        self.row_count
    }

    /// 已分配的页数
    pub fn page_count(&self) -> usize {
        self.page_ids.len()
    }

    /// 分配新的 heap page，返回 page_id
    fn allocate_page(&mut self) -> Result<u32, HeapError> {
        let page_id = self.next_page_id;
        self.next_page_id += 1;
        let page = Page::new(page_id, PageType::Data);
        self.buffer_pool.put_page(page_id, page)?;
        self.page_ids.push(page_id);
        trace!(page_id, table = %self.name, "heap page allocated");
        Ok(page_id)
    }

    /// 插入一行，返回 row_id
    ///
    /// row_id 编码：`(page_index << 16) | slot_id`
    pub fn insert_row(
        &mut self,
        row: &[szrsql_types::value::Value],
        xmin: u32,
    ) -> Result<usize, HeapError> {
        let tuple = row_to_tuple(row, xmin);

        // 尝试在最后一个 page 插入
        if let Some(&last_page_id) = self.page_ids.last() {
            let mut page = self.buffer_pool.read_page(last_page_id)?;
            match page.insert_tuple(&tuple) {
                Ok(slot_id) => {
                    self.buffer_pool.put_page(last_page_id, page)?;
                    self.row_count += 1;
                    let page_index = self.page_ids.len() - 1;
                    return Ok((page_index << 16) | slot_id as usize);
                }
                Err(_) => {
                    // 页满，写回后分配新页
                    self.buffer_pool.put_page(last_page_id, page)?;
                }
            }
        }

        // 分配新页
        let new_page_id = self.allocate_page()?;
        let mut page = self.buffer_pool.read_page(new_page_id)?;
        let slot_id = page
            .insert_tuple(&tuple)
            .map_err(|e| HeapError::Page(e.to_string()))?;
        self.buffer_pool.put_page(new_page_id, page)?;
        self.row_count += 1;
        let page_index = self.page_ids.len() - 1;
        Ok((page_index << 16) | slot_id as usize)
    }

    /// 通过 row_id 读取一行
    pub fn get_row(&self, row_id: usize) -> Result<Option<Vec<szrsql_types::value::Value>>, HeapError> {
        let page_index = row_id >> 16;
        let slot_id = (row_id & 0xFFFF) as u16;

        if page_index >= self.page_ids.len() {
            return Ok(None);
        }
        let page_id = self.page_ids[page_index];
        let page = self.buffer_pool.read_page(page_id)?;

        match page.read_tuple(slot_id) {
            Ok(tuple) => {
                if tuple.header.is_deleted() {
                    Ok(None)
                } else {
                    Ok(Some(tuple_to_row(&tuple)))
                }
            }
            Err(_) => Ok(None),
        }
    }

    /// 标记删除一行
    pub fn delete_row(&mut self, row_id: usize, xmax: u32) -> Result<bool, HeapError> {
        let page_index = row_id >> 16;
        let slot_id = (row_id & 0xFFFF) as u16;

        if page_index >= self.page_ids.len() {
            return Ok(false);
        }
        let page_id = self.page_ids[page_index];
        let mut page = self.buffer_pool.read_page(page_id)?;

        match page.mark_tuple_deleted(slot_id, xmax) {
            Ok(()) => {
                self.buffer_pool.put_page(page_id, page)?;
                self.row_count = self.row_count.saturating_sub(1);
                Ok(true)
            }
            Err(_) => {
                self.buffer_pool.put_page(page_id, page)?;
                Ok(false)
            }
        }
    }

    /// 更新一行（标记旧行删除 + 插入新行），返回新 row_id
    pub fn update_row(
        &mut self,
        row_id: usize,
        new_row: &[szrsql_types::value::Value],
        xmin: u32,
        xmax: u32,
    ) -> Result<Option<usize>, HeapError> {
        // 标记旧行删除
        if !self.delete_row(row_id, xmax)? {
            return Ok(None);
        }
        // 插入新行
        let new_id = self.insert_row(new_row, xmin)?;
        Ok(Some(new_id))
    }

    /// 顺序扫描所有活跃行，返回 (row_id, row) 迭代器
    pub fn scan_all(&self) -> Result<Vec<(usize, Vec<szrsql_types::value::Value>)>, HeapError> {
        let mut results = Vec::new();
        for (page_index, &page_id) in self.page_ids.iter().enumerate() {
            let page = self.buffer_pool.read_page(page_id)?;
            for slot_id in 0..page.header.tuple_count {
                if let Ok(tuple) = page.read_tuple(slot_id) {
                    if !tuple.header.is_deleted() {
                        let row_id = (page_index << 16) | slot_id as usize;
                        results.push((row_id, tuple_to_row(&tuple)));
                    }
                }
            }
        }
        Ok(results)
    }

    /// 获取所有活跃行的 xmin/xmax 版本信息（MVCC 用）
    pub fn scan_with_versions(
        &self,
    ) -> Result<Vec<(usize, Vec<szrsql_types::value::Value>, u32, u32)>, HeapError> {
        let mut results = Vec::new();
        for (page_index, &page_id) in self.page_ids.iter().enumerate() {
            let page = self.buffer_pool.read_page(page_id)?;
            for slot_id in 0..page.header.tuple_count {
                if let Ok(tuple) = page.read_tuple(slot_id) {
                    if !tuple.header.is_deleted() {
                        let row_id = (page_index << 16) | slot_id as usize;
                        results.push((
                            row_id,
                            tuple_to_row(&tuple),
                            tuple.header.xmin,
                            tuple.header.xmax,
                        ));
                    }
                }
            }
        }
        Ok(results)
    }

    /// 碎片整理：对所有页执行 compact
    pub fn vacuum(&mut self) -> Result<usize, HeapError> {
        let mut reclaimed = 0;
        for &page_id in &self.page_ids {
            let mut page = self.buffer_pool.read_page(page_id)?;
            let before = page.header.tuple_count as usize;
            let _ = page.compact();
            let after = page.header.tuple_count as usize;
            reclaimed += before.saturating_sub(after);
            self.buffer_pool.put_page(page_id, page)?;
        }
        if reclaimed > 0 {
            debug!(reclaimed, table = %self.name, "heap vacuum completed");
        }
        Ok(reclaimed)
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{BufferPool, InMemoryPageLoader};
    use szrsql_types::value::Value;

    fn make_pool() -> Arc<BufferPool> {
        let loader = Arc::new(InMemoryPageLoader::new());
        Arc::new(BufferPool::new(64, loader).unwrap())
    }

    #[test]
    fn row_encode_decode_roundtrip_int() {
        let row = vec![Value::Int64(42), Value::Bool(true), Value::Null];
        let tuple = row_to_tuple(&row, 1);
        let decoded = tuple_to_row(&tuple);
        assert_eq!(decoded[0], Value::Int64(42));
        assert_eq!(decoded[1], Value::Bool(true));
        assert_eq!(decoded[2], Value::Null);
    }

    #[test]
    fn row_encode_decode_roundtrip_text() {
        let row = vec![Value::Text("hello".into()), Value::Int64(99)];
        let tuple = row_to_tuple(&row, 5);
        let decoded = tuple_to_row(&tuple);
        assert_eq!(decoded[0], Value::Text("hello".into()));
        assert_eq!(decoded[1], Value::Int64(99));
    }

    #[test]
    fn row_encode_decode_float_date() {
        let row = vec![Value::Float64(3.14), Value::Date(19000), Value::Timestamp(1_700_000_000)];
        let tuple = row_to_tuple(&row, 1);
        let decoded = tuple_to_row(&tuple);
        assert_eq!(decoded[0], Value::Float64(3.14));
        assert_eq!(decoded[1], Value::Date(19000));
        assert_eq!(decoded[2], Value::Timestamp(1_700_000_000));
    }

    #[test]
    fn heap_table_insert_and_get() {
        let pool = make_pool();
        let mut table = HeapTable::new("test", 3, pool);
        let row = vec![Value::Int64(1), Value::Text("alice".into()), Value::Bool(true)];
        let row_id = table.insert_row(&row, 1).unwrap();
        assert_eq!(table.row_count(), 1);

        let fetched = table.get_row(row_id).unwrap().unwrap();
        assert_eq!(fetched[0], Value::Int64(1));
        assert_eq!(fetched[1], Value::Text("alice".into()));
        assert_eq!(fetched[2], Value::Bool(true));
    }

    #[test]
    fn heap_table_delete() {
        let pool = make_pool();
        let mut table = HeapTable::new("test", 2, pool);
        let row = vec![Value::Int64(10), Value::Text("bob".into())];
        let row_id = table.insert_row(&row, 1).unwrap();
        assert_eq!(table.row_count(), 1);

        assert!(table.delete_row(row_id, 2).unwrap());
        assert_eq!(table.row_count(), 0);
        assert!(table.get_row(row_id).unwrap().is_none());
    }

    #[test]
    fn heap_table_update() {
        let pool = make_pool();
        let mut table = HeapTable::new("test", 2, pool);
        let row = vec![Value::Int64(1), Value::Text("old".into())];
        let row_id = table.insert_row(&row, 1).unwrap();

        let new_row = vec![Value::Int64(1), Value::Text("new".into())];
        let new_id = table.update_row(row_id, &new_row, 2, 2).unwrap().unwrap();

        assert!(table.get_row(row_id).unwrap().is_none());
        let fetched = table.get_row(new_id).unwrap().unwrap();
        assert_eq!(fetched[1], Value::Text("new".into()));
    }

    #[test]
    fn heap_table_scan_all() {
        let pool = make_pool();
        let mut table = HeapTable::new("test", 1, pool);
        for i in 0..10 {
            table.insert_row(&[Value::Int64(i)], 1).unwrap();
        }
        // 删除偶数行
        for i in (0..10).step_by(2) {
            let row_id = i; // page_index=0, slot_id=i
            table.delete_row(row_id, 2).unwrap();
        }
        let rows = table.scan_all().unwrap();
        assert_eq!(rows.len(), 5);
        for (_, row) in &rows {
            let v = match &row[0] {
                Value::Int64(n) => *n,
                _ => panic!("unexpected"),
            };
            assert!(v % 2 == 1);
        }
    }

    #[test]
    fn heap_table_multi_page() {
        let pool = make_pool();
        let mut table = HeapTable::new("big", 1, pool);
        // 插入足够多的行以触发多页
        for i in 0..1000 {
            table
                .insert_row(&[Value::Text(format!("row_{:04}", i))], 1)
                .unwrap();
        }
        assert!(table.page_count() > 1);
        assert_eq!(table.row_count(), 1000);

        let rows = table.scan_all().unwrap();
        assert_eq!(rows.len(), 1000);
    }

    #[test]
    fn heap_table_vacuum() {
        let pool = make_pool();
        let mut table = HeapTable::new("test", 1, pool);
        for i in 0..20 {
            table.insert_row(&[Value::Int64(i)], 1).unwrap();
        }
        // 删除一半
        for i in 0..10 {
            table.delete_row(i, 2).unwrap();
        }
        let reclaimed = table.vacuum().unwrap();
        assert!(reclaimed >= 10);
    }
}
