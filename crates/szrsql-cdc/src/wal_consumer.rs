//! WAL 消费循环 — Batch 6.1
//!
//! 后台线程 tail WAL 段文件，解码 WalRecord，转换为 CDC SourceEvent 分发。
//! 与 Batch 1 的段文件配合：消费完的段可归档。
//!
//! # 设计要点
//! - `WalConsumer`：轮询 WAL 段文件，解码记录，分发事件
//! - `ConsumerOffset`：记录已消费的段编号 + 文件内偏移
//! - 支持回调模式（每批事件回调）和拉取模式（`poll()`）

use std::path::{Path, PathBuf};
use szrsql_tx::wal::{WalRecord, WalOpType};
use crate::source::{SourceEvent, SourceEventOp};
use crate::decoder::DecodedRow;
use szrsql_types::value::Value;

// =====================================================================
//  ConsumerOffset — 消费位点
// =====================================================================

/// WAL 消费位点 — 记录已消费到哪个段的哪个位置
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerOffset {
    /// 当前消费的段编号
    pub segment: u32,
    /// 段文件内已消费的字节偏移
    pub file_offset: u64,
    /// 最后消费的 LSN
    pub last_lsn: u64,
}

impl ConsumerOffset {
    /// 创建初始位点（从第一段开头）
    pub fn start() -> Self {
        Self { segment: 1, file_offset: 0, last_lsn: 0 }
    }

    /// 从指定段和偏移创建
    pub fn new(segment: u32, file_offset: u64, last_lsn: u64) -> Self {
        Self { segment, file_offset, last_lsn }
    }
}

impl Default for ConsumerOffset {
    fn default() -> Self {
        Self::start()
    }
}

// =====================================================================
//  WalConsumer — WAL 消费器
// =====================================================================

/// WAL 消费器 — 尾随 WAL 段文件，解码记录并转换为 CDC 事件
///
/// # 使用方式
/// ```ignore
/// let mut consumer = WalConsumer::new("/path/to/wal_dir");
/// loop {
///     let events = consumer.poll();
///     for event in events {
///         // 处理 CDC 事件
///     }
///     std::thread::sleep(Duration::from_millis(100));
/// }
/// ```
pub struct WalConsumer {
    /// WAL 段文件目录
    wal_dir: PathBuf,
    /// 当前消费位点
    offset: ConsumerOffset,
    /// 每次 poll 最大返回事件数
    batch_size: usize,
    /// 已消费的总记录数（统计用）
    total_consumed: u64,
}

impl WalConsumer {
    /// 创建 WAL 消费器
    ///
    /// # 参数
    /// - `wal_dir`：WAL 段文件所在目录
    pub fn new<P: AsRef<Path>>(wal_dir: P) -> Self {
        Self {
            wal_dir: wal_dir.as_ref().to_path_buf(),
            offset: ConsumerOffset::start(),
            batch_size: 1000,
            total_consumed: 0,
        }
    }

    /// 设置每次 poll 的最大事件数
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// 从指定位点开始消费
    pub fn from_offset(mut self, offset: ConsumerOffset) -> Self {
        self.offset = offset;
        self
    }

    /// 获取当前消费位点
    pub fn offset(&self) -> &ConsumerOffset {
        &self.offset
    }

    /// 获取已消费总记录数
    pub fn total_consumed(&self) -> u64 {
        self.total_consumed
    }

    /// 轮询新事件（非阻塞）
    ///
    /// 从当前位点读取 WAL 记录，转换为 SourceEvent 返回。
    /// 若无新数据则返回空 Vec。
    pub fn poll(&mut self) -> Vec<SourceEvent> {
        let mut events = Vec::new();
        let seg_path = self.segment_path(self.offset.segment);

        if !seg_path.exists() {
            // 尝试下一个段
            let next = self.segment_path(self.offset.segment + 1);
            if next.exists() {
                self.offset.segment += 1;
                self.offset.file_offset = 0;
                return self.poll();
            }
            return events;
        }

        // 读取段文件从当前偏移开始
        let data = match std::fs::read(&seg_path) {
            Ok(d) => d,
            Err(_) => return events,
        };

        let mut pos = self.offset.file_offset as usize;
        while pos < data.len() && events.len() < self.batch_size {
            match WalRecord::decode(&data[pos..]) {
                Ok(record) => {
                    let record_size = record.encoded_size();
                    if let Some(event) = Self::record_to_event(&record) {
                        events.push(event);
                    }
                    self.offset.last_lsn = record.lsn;
                    self.total_consumed += 1;
                    pos += record_size;
                }
                Err(_) => break, // 数据不完整，等待更多写入
            }
        }

        self.offset.file_offset = pos as u64;

        // 检查是否已读完当前段，若读完则尝试切换到下一段
        if pos >= data.len() {
            let next = self.segment_path(self.offset.segment + 1);
            if next.exists() {
                self.offset.segment += 1;
                self.offset.file_offset = 0;
            }
        }

        events
    }

    /// 回调模式消费（阻塞直到停止）
    ///
    /// # 参数
    /// - `callback`：每批事件回调，返回 false 停止消费
    /// - `poll_interval_ms`：无新数据时的轮询间隔（毫秒）
    pub fn consume<F>(&mut self, mut callback: F, poll_interval_ms: u64)
    where
        F: FnMut(&[SourceEvent]) -> bool,
    {
        loop {
            let events = self.poll();
            if events.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(poll_interval_ms));
                continue;
            }
            if !callback(&events) {
                break;
            }
        }
    }

    /// 将 WalRecord 转换为 SourceEvent
    fn record_to_event(record: &WalRecord) -> Option<SourceEvent> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        match record.op_type {
            WalOpType::Insert => {
                let row = Self::decode_row_data(&record.data);
                Some(SourceEvent::insert(
                    record.lsn, "public", &format!("table_{}", record.page_id), row, ts,
                ).with_tx_id(record.tx_id as u64))
            }
            WalOpType::Update => {
                let row = Self::decode_row_data(&record.data);
                Some(SourceEvent {
                    lsn: record.lsn,
                    op: SourceEventOp::Update,
                    schema_name: "public".to_string(),
                    table_name: format!("table_{}", record.page_id),
                    before: None,
                    after: Some(row),
                    tx_id: Some(record.tx_id as u64),
                    timestamp: ts,
                })
            }
            WalOpType::Delete => {
                let row = Self::decode_row_data(&record.data);
                Some(SourceEvent::delete(
                    record.lsn, "public", &format!("table_{}", record.page_id), row, ts,
                ).with_tx_id(record.tx_id as u64))
            }
            WalOpType::Commit => {
                Some(SourceEvent::commit(record.lsn, record.tx_id as u64, ts))
            }
            WalOpType::Abort => {
                Some(SourceEvent::abort(record.lsn, record.tx_id as u64, ts))
            }
            // Checkpoint / FullPageImage / TableData 不产生 CDC 事件
            _ => None,
        }
    }

    /// 解码行数据（简化版：将原始字节包装为 DecodedRow）
    fn decode_row_data(data: &[u8]) -> DecodedRow {
        // 简化实现：将原始数据作为单列 "_raw" 包装
        // 生产环境应根据表 schema 解码各列
        DecodedRow {
            columns: vec![("_raw".to_string(), Value::Blob(data.to_vec()))],
        }
    }

    /// 构建段文件路径
    fn segment_path(&self, segment: u32) -> PathBuf {
        self.wal_dir.join(format!("wal_{:08}.log", segment))
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_tx::wal::WalRecord;

    fn write_test_wal(dir: &Path, segment: u32, records: &[WalRecord]) {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("wal_{:08}.log", segment));
        let mut data = Vec::new();
        for r in records {
            let mut rec = r.clone();
            rec.update_checksum();
            data.extend_from_slice(&rec.encode());
        }
        std::fs::write(&path, &data).unwrap();
    }

    #[test]
    fn consumer_offset_start() {
        let off = ConsumerOffset::start();
        assert_eq!(off.segment, 1);
        assert_eq!(off.file_offset, 0);
        assert_eq!(off.last_lsn, 0);
    }

    #[test]
    fn consumer_poll_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut consumer = WalConsumer::new(dir.path());
        let events = consumer.poll();
        assert!(events.is_empty());
    }

    #[test]
    fn consumer_poll_insert_records() {
        let dir = tempfile::tempdir().unwrap();
        let records = vec![
            WalRecord::new(1, 1, WalOpType::Insert, 10, vec![1, 2, 3]),
            WalRecord::new(2, 1, WalOpType::Insert, 10, vec![4, 5, 6]),
            WalRecord::new(3, 1, WalOpType::Commit, 0, vec![]),
        ];
        write_test_wal(dir.path(), 1, &records);

        let mut consumer = WalConsumer::new(dir.path());
        let events = consumer.poll();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].op, SourceEventOp::Insert);
        assert_eq!(events[1].op, SourceEventOp::Insert);
        assert_eq!(events[2].op, SourceEventOp::Commit);
        assert_eq!(consumer.total_consumed(), 3);
    }

    #[test]
    fn consumer_tracks_offset() {
        let dir = tempfile::tempdir().unwrap();
        let records = vec![
            WalRecord::new(1, 1, WalOpType::Insert, 10, vec![1]),
            WalRecord::new(2, 1, WalOpType::Commit, 0, vec![]),
        ];
        write_test_wal(dir.path(), 1, &records);

        let mut consumer = WalConsumer::new(dir.path());
        consumer.poll();
        assert_eq!(consumer.offset().last_lsn, 2);
        assert!(consumer.offset().file_offset > 0);
    }

    #[test]
    fn consumer_skips_checkpoint_records() {
        let dir = tempfile::tempdir().unwrap();
        let records = vec![
            WalRecord::new(1, 0, WalOpType::Checkpoint, 0, vec![]),
            WalRecord::new(2, 1, WalOpType::Insert, 5, vec![9]),
        ];
        write_test_wal(dir.path(), 1, &records);

        let mut consumer = WalConsumer::new(dir.path());
        let events = consumer.poll();
        // Checkpoint 不产生事件，但 Insert 产生
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].op, SourceEventOp::Insert);
        // 但 total_consumed 包含所有记录
        assert_eq!(consumer.total_consumed(), 2);
    }

    #[test]
    fn consumer_segment_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let seg1 = vec![WalRecord::new(1, 1, WalOpType::Insert, 1, vec![1])];
        let seg2 = vec![WalRecord::new(2, 2, WalOpType::Insert, 2, vec![2])];
        write_test_wal(dir.path(), 1, &seg1);
        write_test_wal(dir.path(), 2, &seg2);

        let mut consumer = WalConsumer::new(dir.path());
        let e1 = consumer.poll();
        assert_eq!(e1.len(), 1);
        assert_eq!(e1[0].lsn, 1);

        // 第二次 poll 应切换到段 2
        let e2 = consumer.poll();
        assert_eq!(e2.len(), 1);
        assert_eq!(e2[0].lsn, 2);
        assert_eq!(consumer.offset().segment, 2);
    }

    #[test]
    fn consumer_batch_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let records: Vec<WalRecord> = (0..10)
            .map(|i| WalRecord::new(i + 1, 1, WalOpType::Insert, 1, vec![i as u8]))
            .collect();
        write_test_wal(dir.path(), 1, &records);

        let mut consumer = WalConsumer::new(dir.path()).with_batch_size(3);
        let e1 = consumer.poll();
        assert_eq!(e1.len(), 3);
        let e2 = consumer.poll();
        assert_eq!(e2.len(), 3);
    }

    #[test]
    fn consumer_from_offset_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let records = vec![
            WalRecord::new(1, 1, WalOpType::Insert, 1, vec![1]),
            WalRecord::new(2, 1, WalOpType::Insert, 1, vec![2]),
            WalRecord::new(3, 1, WalOpType::Commit, 0, vec![]),
        ];
        write_test_wal(dir.path(), 1, &records);

        // 先消费全部
        let mut c1 = WalConsumer::new(dir.path());
        c1.poll();
        let saved = c1.offset().clone();

        // 从位点恢复 — 应无新事件
        let mut c2 = WalConsumer::new(dir.path()).from_offset(saved);
        let events = c2.poll();
        assert!(events.is_empty());
    }
}
