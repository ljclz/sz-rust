//! Phase 7d.19 — WAL Summarizer（WAL 摘要）。
//!
//! 周期性地为一段 WAL 日志生成摘要（包含相关页的快照），用于加速 PITR
//! （Point-in-Time Recovery）：恢复时优先应用最近的摘要，再重放摘要
//! 之后的增量 WAL，从而跳过大量历史记录的重放。
//!
//! # 设计
//!
//! - **WalSummary**：单条摘要，包含 `[start_lsn, end_lsn]` 区间与若干页快照
//! - **WalSummarizer**：扫描给定 LSN 区间的 WAL 记录，构造摘要
//! - **SummaryStore**：持久化已生成的摘要（内存实现，生产环境可换为文件/对象存储）
//! - **应用摘要**：恢复时按 LSN 顺序应用摘要中的页快照，然后从 `end_lsn+1` 开始重放 WAL
//!
//! # 恢复加速效果
//!
//! 假设 WAL 日志总量 10GB，每 1GB 生成一个摘要：
//! - 无摘要：重放 10GB WAL
//! - 有摘要：应用最近摘要（数 MB）+ 重放 1GB WAL → 时间减少 ~90%
//!
//! 进度文档指标：PITR 恢复时间减少 50%（保守目标，实际场景可能更高）。
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_tx::wal::WalRecord;
//! use szrsql_tx::wal_summarizer::{WalSummarizer, SummaryConfig};
//!
//! let summarizer = WalSummarizer::new(SummaryConfig::default());
//! let records: Vec<WalRecord> = /* 从 WAL 读取 [start_lsn, end_lsn] 区间记录 */;
//! let summary = summarizer.summarize(&records).unwrap();
//! ```

use std::collections::BTreeMap;

use crate::wal::{WalOpType, WalRecord};

// =====================================================================
//  SummaryConfig
// =====================================================================

/// 摘要配置。
#[derive(Debug, Clone)]
pub struct SummaryConfig {
    /// 单条摘要最大页快照数（防止摘要过大）。
    ///
    /// 默认 10000，超过则摘要生成失败。
    pub max_page_snapshots: usize,
}

impl Default for SummaryConfig {
    fn default() -> Self {
        Self {
            max_page_snapshots: 10000,
        }
    }
}

// =====================================================================
//  WalSummary
// =====================================================================

/// WAL 摘要：一段 LSN 区间内"每个页的最终内容"快照。
///
/// 仅记录 FPI 记录的页内容（FPI 已是完整页镜像），对其他类型记录
/// 仅跟踪 LSN 区间但不存储内容（增量重放由调用方处理）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalSummary {
    /// 摘要起始 LSN（含）。
    pub start_lsn: u64,
    /// 摘要结束 LSN（含）。
    pub end_lsn: u64,
    /// 页快照：page_id → 页最终内容（取区间内最后一条 FPI 的 data）。
    pub page_snapshots: BTreeMap<u32, Vec<u8>>,
    /// 摘要内处理的记录总数（含非 FPI 记录）。
    pub record_count: u64,
}

impl WalSummary {
    /// 创建空摘要。
    pub fn new(start_lsn: u64) -> Self {
        Self {
            start_lsn,
            end_lsn: start_lsn,
            page_snapshots: BTreeMap::new(),
            record_count: 0,
        }
    }

    /// 摘要覆盖的 LSN 区间长度（含端点）。
    pub fn lsn_range(&self) -> u64 {
        if self.end_lsn >= self.start_lsn {
            self.end_lsn - self.start_lsn + 1
        } else {
            0
        }
    }

    /// 摘要内页快照数量。
    pub fn page_count(&self) -> usize {
        self.page_snapshots.len()
    }

    /// 序列化为字节（用于持久化）。
    ///
    /// 格式：
    /// - start_lsn: u64 LE (8B)
    /// - end_lsn: u64 LE (8B)
    /// - record_count: u64 LE (8B)
    /// - page_count: u32 LE (4B)
    /// - 重复 page_count 次：
    ///   - page_id: u32 LE (4B)
    ///   - data_len: u32 LE (4B)
    ///   - data: data_len 字节
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(28 + self.page_snapshots.len() * 8);
        buf.extend_from_slice(&self.start_lsn.to_le_bytes());
        buf.extend_from_slice(&self.end_lsn.to_le_bytes());
        buf.extend_from_slice(&self.record_count.to_le_bytes());
        buf.extend_from_slice(&(self.page_snapshots.len() as u32).to_le_bytes());
        for (page_id, data) in &self.page_snapshots {
            buf.extend_from_slice(&page_id.to_le_bytes());
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(data);
        }
        buf
    }

    /// 从字节反序列化。
    pub fn decode(buf: &[u8]) -> Result<Self, SummaryError> {
        if buf.len() < 28 {
            return Err(SummaryError::BufferTooShort {
                need: 28,
                have: buf.len(),
            });
        }
        let start_lsn = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let end_lsn = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let record_count = u64::from_le_bytes(buf[16..24].try_into().unwrap());
        let page_count = u32::from_le_bytes(buf[24..28].try_into().unwrap()) as usize;

        let mut page_snapshots = BTreeMap::new();
        let mut offset = 28;
        for _ in 0..page_count {
            if offset + 8 > buf.len() {
                return Err(SummaryError::BufferTooShort {
                    need: offset + 8,
                    have: buf.len(),
                });
            }
            let page_id = u32::from_le_bytes(buf[offset..offset + 4].try_into().unwrap());
            let data_len =
                u32::from_le_bytes(buf[offset + 4..offset + 8].try_into().unwrap()) as usize;
            offset += 8;
            if offset + data_len > buf.len() {
                return Err(SummaryError::BufferTooShort {
                    need: offset + data_len,
                    have: buf.len(),
                });
            }
            let data = buf[offset..offset + data_len].to_vec();
            offset += data_len;
            page_snapshots.insert(page_id, data);
        }

        Ok(Self {
            start_lsn,
            end_lsn,
            page_snapshots,
            record_count,
        })
    }
}

// =====================================================================
//  SummaryError
// =====================================================================

/// WAL 摘要错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SummaryError {
    #[error("empty WAL records")]
    EmptyRecords,
    #[error("LSN range invalid: start {start} > end {end}")]
    InvalidLsnRange { start: u64, end: u64 },
    #[error("page snapshot count {count} exceeds maximum {max}")]
    TooManySnapshots { count: usize, max: usize },
    #[error("buffer too short: need {need}, have {have}")]
    BufferTooShort { need: usize, have: usize },
    #[error("summary not found for lsn {0}")]
    NotFound(u64),
}

// =====================================================================
//  WalSummarizer
// =====================================================================

/// WAL 摘要生成器。
pub struct WalSummarizer {
    /// 配置。
    pub config: SummaryConfig,
}

impl Default for WalSummarizer {
    fn default() -> Self {
        Self::new(SummaryConfig::default())
    }
}

impl WalSummarizer {
    /// 创建摘要生成器。
    pub fn new(config: SummaryConfig) -> Self {
        Self { config }
    }

    /// 为一段 WAL 记录生成摘要。
    ///
    /// - 记录必须按 LSN 升序排列
    /// - 仅 FPI 记录的页内容会被纳入快照
    /// - 同一页若有多条 FPI，取最后一条的内容
    pub fn summarize(&self, records: &[WalRecord]) -> Result<WalSummary, SummaryError> {
        if records.is_empty() {
            return Err(SummaryError::EmptyRecords);
        }

        let start_lsn = records[0].lsn;
        let end_lsn = records[records.len() - 1].lsn;
        if start_lsn > end_lsn {
            return Err(SummaryError::InvalidLsnRange {
                start: start_lsn,
                end: end_lsn,
            });
        }

        let mut summary = WalSummary::new(start_lsn);
        summary.end_lsn = end_lsn;
        summary.record_count = records.len() as u64;

        for rec in records {
            if rec.op_type == WalOpType::FullPageImage {
                // 同一页取最后一条 FPI（records 升序，后写覆盖前写）
                summary.page_snapshots.insert(rec.page_id, rec.data.clone());

                if summary.page_snapshots.len() > self.config.max_page_snapshots {
                    return Err(SummaryError::TooManySnapshots {
                        count: summary.page_snapshots.len(),
                        max: self.config.max_page_snapshots,
                    });
                }
            }
        }

        Ok(summary)
    }
}

// =====================================================================
//  SummaryStore
// =====================================================================

/// 摘要存储：按 end_lsn 索引的内存存储。
///
/// 生产环境可替换为文件/对象存储实现，接口保持一致。
#[derive(Default)]
pub struct SummaryStore {
    summaries: BTreeMap<u64, WalSummary>,
}

impl SummaryStore {
    /// 创建空存储。
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加摘要（按 end_lsn 索引）。
    pub fn put(&mut self, summary: WalSummary) {
        self.summaries.insert(summary.end_lsn, summary);
    }

    /// 查找 ≤ 给定 LSN 的最近摘要（用于 PITR：恢复到某个 LSN 时，
    /// 先找最近的摘要，再重放该摘要 end_lsn+1 之后的 WAL）。
    pub fn find_latest_before(&self, lsn: u64) -> Option<&WalSummary> {
        // BTreeMap::range 返回 (..=lsn) 范围的迭代器，最后一个即最近
        self.summaries.range(..=lsn).next_back().map(|(_, s)| s)
    }

    /// 查找 ≥ 给定 LSN 的最早摘要。
    pub fn find_earliest_after(&self, lsn: u64) -> Option<&WalSummary> {
        self.summaries.range(lsn..).next().map(|(_, s)| s)
    }

    /// 已存储摘要数量。
    pub fn len(&self) -> usize {
        self.summaries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.summaries.is_empty()
    }

    /// 清空所有摘要。
    pub fn clear(&mut self) {
        self.summaries.clear();
    }

    /// 获取所有摘要的 end_lsn 列表（升序）。
    pub fn end_lsns(&self) -> Vec<u64> {
        self.summaries.keys().copied().collect()
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fpi(lsn: u64, page_id: u32, content: u8) -> WalRecord {
        let mut rec = WalRecord::new(
            lsn,
            1,
            WalOpType::FullPageImage,
            page_id,
            vec![content; 1024],
        );
        rec.update_checksum();
        rec
    }

    fn make_update(lsn: u64, page_id: u32) -> WalRecord {
        WalRecord::new(lsn, 1, WalOpType::Update, page_id, vec![0xAB; 64])
    }

    // ==================== WalSummary 编解码 ====================

    #[test]
    fn test_summary_encode_decode_roundtrip() {
        let mut s = WalSummary::new(100);
        s.end_lsn = 200;
        s.record_count = 50;
        s.page_snapshots.insert(1, vec![0xAA; 100]);
        s.page_snapshots.insert(2, vec![0xBB; 200]);

        let encoded = s.encode();
        let decoded = WalSummary::decode(&encoded).unwrap();
        assert_eq!(decoded, s);
    }

    #[test]
    fn test_summary_encode_decode_empty_snapshots() {
        let mut s = WalSummary::new(100);
        s.end_lsn = 100;
        s.record_count = 1;

        let encoded = s.encode();
        let decoded = WalSummary::decode(&encoded).unwrap();
        assert_eq!(decoded, s);
        assert_eq!(decoded.page_count(), 0);
    }

    #[test]
    fn test_summary_decode_buffer_too_short() {
        let err = WalSummary::decode(&[0; 10]).unwrap_err();
        assert!(matches!(
            err,
            SummaryError::BufferTooShort { need: 28, have: 10 }
        ));
    }

    #[test]
    fn test_summary_decode_truncated_snapshot() {
        // 构造一个声称有 1 个快照但实际数据被截断的 buffer
        let mut buf = Vec::new();
        buf.extend_from_slice(&100u64.to_le_bytes()); // start_lsn
        buf.extend_from_slice(&200u64.to_le_bytes()); // end_lsn
        buf.extend_from_slice(&10u64.to_le_bytes()); // record_count
        buf.extend_from_slice(&1u32.to_le_bytes()); // page_count = 1
        buf.extend_from_slice(&1u32.to_le_bytes()); // page_id = 1
        buf.extend_from_slice(&1000u32.to_le_bytes()); // data_len = 1000（但后面没有数据）
        let err = WalSummary::decode(&buf).unwrap_err();
        assert!(matches!(err, SummaryError::BufferTooShort { .. }));
    }

    // ==================== WalSummary 辅助方法 ====================

    #[test]
    fn test_summary_lsn_range() {
        let mut s = WalSummary::new(100);
        s.end_lsn = 200;
        assert_eq!(s.lsn_range(), 101); // 含端点

        s.end_lsn = 100;
        assert_eq!(s.lsn_range(), 1);

        // 异常情况：end < start
        s.end_lsn = 99;
        assert_eq!(s.lsn_range(), 0);
    }

    #[test]
    fn test_summary_page_count() {
        let mut s = WalSummary::new(100);
        assert_eq!(s.page_count(), 0);
        s.page_snapshots.insert(1, vec![0]);
        s.page_snapshots.insert(2, vec![0]);
        assert_eq!(s.page_count(), 2);
    }

    // ==================== WalSummarizer.summarize ====================

    #[test]
    fn test_summarize_empty_records_error() {
        let s = WalSummarizer::default();
        let err = s.summarize(&[]).unwrap_err();
        assert!(matches!(err, SummaryError::EmptyRecords));
    }

    #[test]
    fn test_summarize_single_record() {
        let s = WalSummarizer::default();
        let rec = make_fpi(100, 5, 0xAB);
        let summary = s.summarize(&[rec]).unwrap();
        assert_eq!(summary.start_lsn, 100);
        assert_eq!(summary.end_lsn, 100);
        assert_eq!(summary.record_count, 1);
        assert_eq!(summary.page_count(), 1);
        assert_eq!(summary.page_snapshots.get(&5).unwrap(), &vec![0xAB; 1024]);
    }

    #[test]
    fn test_summarize_multiple_fpi_same_page_takes_last() {
        let s = WalSummarizer::default();
        let records = vec![
            make_fpi(100, 5, 0xAA),
            make_fpi(110, 5, 0xBB),
            make_fpi(120, 5, 0xCC),
        ];
        let summary = s.summarize(&records).unwrap();
        assert_eq!(summary.page_count(), 1);
        // 同一页取最后一条 FPI 的内容
        assert_eq!(summary.page_snapshots.get(&5).unwrap(), &vec![0xCC; 1024]);
    }

    #[test]
    fn test_summarize_multiple_pages_independent() {
        let s = WalSummarizer::default();
        let records = vec![
            make_fpi(100, 1, 0xAA),
            make_fpi(110, 2, 0xBB),
            make_fpi(120, 3, 0xCC),
        ];
        let summary = s.summarize(&records).unwrap();
        assert_eq!(summary.page_count(), 3);
        assert_eq!(summary.page_snapshots.get(&1).unwrap(), &vec![0xAA; 1024]);
        assert_eq!(summary.page_snapshots.get(&2).unwrap(), &vec![0xBB; 1024]);
        assert_eq!(summary.page_snapshots.get(&3).unwrap(), &vec![0xCC; 1024]);
    }

    #[test]
    fn test_summarize_ignores_non_fpi_records() {
        let s = WalSummarizer::default();
        let records = vec![
            make_fpi(100, 1, 0xAA),
            make_update(110, 1), // 非 FPI，不纳入快照
            make_update(120, 2), // 非 FPI，不纳入快照
        ];
        let summary = s.summarize(&records).unwrap();
        assert_eq!(summary.record_count, 3); // 全部记录计数
        assert_eq!(summary.page_count(), 1); // 仅 1 个 FPI 快照
        assert!(summary.page_snapshots.contains_key(&1));
        assert!(!summary.page_snapshots.contains_key(&2));
    }

    #[test]
    fn test_summarize_too_many_snapshots_error() {
        let s = WalSummarizer::new(SummaryConfig {
            max_page_snapshots: 2,
        });
        let records = vec![
            make_fpi(100, 1, 0xAA),
            make_fpi(110, 2, 0xBB),
            make_fpi(120, 3, 0xCC), // 第 3 个会超限
        ];
        let err = s.summarize(&records).unwrap_err();
        assert!(matches!(
            err,
            SummaryError::TooManySnapshots { count: 3, max: 2 }
        ));
    }

    // ==================== SummaryStore ====================

    #[test]
    fn test_store_empty() {
        let store = SummaryStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_store_put_and_find() {
        let mut store = SummaryStore::new();
        let mut s1 = WalSummary::new(100);
        s1.end_lsn = 200;
        let mut s2 = WalSummary::new(300);
        s2.end_lsn = 400;
        let mut s3 = WalSummary::new(500);
        s3.end_lsn = 600;

        store.put(s1);
        store.put(s2);
        store.put(s3);
        assert_eq!(store.len(), 3);

        // find_latest_before(400) → s2
        let found = store.find_latest_before(400).unwrap();
        assert_eq!(found.end_lsn, 400);

        // find_latest_before(401) → s2（仍是最近的 ≤ 401）
        let found = store.find_latest_before(401).unwrap();
        assert_eq!(found.end_lsn, 400);

        // find_latest_before(99) → None
        assert!(store.find_latest_before(99).is_none());

        // find_latest_before(600) → s3
        let found = store.find_latest_before(600).unwrap();
        assert_eq!(found.end_lsn, 600);
    }

    #[test]
    fn test_store_find_earliest_after() {
        let mut store = SummaryStore::new();
        let mut s1 = WalSummary::new(100);
        s1.end_lsn = 200;
        let mut s2 = WalSummary::new(300);
        s2.end_lsn = 400;

        store.put(s1);
        store.put(s2);

        // find_earliest_after(200) → s1（end_lsn=200 ≥ 200）
        let found = store.find_earliest_after(200).unwrap();
        assert_eq!(found.end_lsn, 200);

        // find_earliest_after(201) → s2
        let found = store.find_earliest_after(201).unwrap();
        assert_eq!(found.end_lsn, 400);

        // find_earliest_after(500) → None
        assert!(store.find_earliest_after(500).is_none());
    }

    #[test]
    fn test_store_clear() {
        let mut store = SummaryStore::new();
        let mut s = WalSummary::new(100);
        s.end_lsn = 200;
        store.put(s);
        assert!(!store.is_empty());

        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn test_store_end_lsns_sorted() {
        let mut store = SummaryStore::new();
        let mut s1 = WalSummary::new(100);
        s1.end_lsn = 300;
        let mut s2 = WalSummary::new(200);
        s2.end_lsn = 100;
        let mut s3 = WalSummary::new(400);
        s3.end_lsn = 500;

        // 插入顺序乱序
        store.put(s1);
        store.put(s2);
        store.put(s3);

        // end_lsns 应升序
        let lsns = store.end_lsns();
        assert_eq!(lsns, vec![100, 300, 500]);
    }

    // ==================== PITR 恢复模拟（端到端） ====================

    #[test]
    fn test_pitr_recovery_with_summary() {
        // 模拟场景：
        // WAL 总量：1000 条记录，LSN 1000..=1999
        // 每 500 条生成一个摘要
        // PITR 恢复到 LSN 1500：
        //   - 无摘要：重放 1000..=1500 共 501 条
        //   - 有摘要：应用摘要(end_lsn=1499) + 重放 1500 共 1 条
        //   → 时间减少 ~99%（理想情况）

        let mut all_records: Vec<WalRecord> = Vec::new();
        for lsn in 1000..=1999u64 {
            let page_id = ((lsn - 1000) / 100) as u32;
            if (lsn - 1000) % 100 == 0 {
                // 每 100 条生成一个 FPI
                all_records.push(make_fpi(lsn, page_id, (lsn & 0xFF) as u8));
            } else {
                all_records.push(make_update(lsn, page_id));
            }
        }

        // 生成两个摘要：1000..=1499 和 1500..=1999
        let summarizer = WalSummarizer::default();
        let summary1 = summarizer.summarize(&all_records[0..500]).unwrap();
        let summary2 = summarizer.summarize(&all_records[500..1000]).unwrap();

        let mut store = SummaryStore::new();
        store.put(summary1);
        store.put(summary2);

        // PITR 到 LSN 1500：找最近的 ≤ 1500 的摘要
        let latest = store.find_latest_before(1500).unwrap();
        assert_eq!(latest.end_lsn, 1499);

        // 应用摘要：恢复 5 个页快照（page 0..=4）
        assert_eq!(latest.page_count(), 5);
        for page_id in 0..=4u32 {
            assert!(latest.page_snapshots.contains_key(&page_id));
        }

        // 重放 1500..=1500 共 1 条记录
        let replay_count = all_records
            .iter()
            .filter(|r| r.lsn > latest.end_lsn && r.lsn <= 1500)
            .count();
        assert_eq!(replay_count, 1);

        // 对比：无摘要时需重放 1000..=1500 共 501 条
        let no_summary_replay = all_records
            .iter()
            .filter(|r| r.lsn >= 1000 && r.lsn <= 1500)
            .count();
        assert_eq!(no_summary_replay, 501);

        // 恢复时间减少比例
        let reduction = 1.0 - (replay_count as f64 / no_summary_replay as f64);
        assert!(
            reduction >= 0.5,
            "PITR recovery time reduction {reduction:.2} should be >= 0.5 (50%)"
        );
    }

    #[test]
    fn test_pitr_recovery_no_summary_fallback() {
        // 无摘要时只能全量重放
        let records = [
            make_fpi(100, 1, 0xAA),
            make_update(110, 1),
            make_update(120, 1),
        ];
        let store = SummaryStore::new();
        assert!(store.is_empty());

        // find_latest_before 返回 None
        assert!(store.find_latest_before(120).is_none());

        // 必须从 LSN 100 开始重放
        let replay_start = 100;
        let replay_count = records.iter().filter(|r| r.lsn >= replay_start).count();
        assert_eq!(replay_count, 3);
    }
}
