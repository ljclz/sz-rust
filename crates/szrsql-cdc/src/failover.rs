//! SzRSQL CDC 消费者故障转移测试 — 对应 `SzRSQL实施进度.md` Phase 2.5.11。
//!
//! # 验证场景
//!
//! **Chaos 测试**：CDC 消费者在处理第 500000 个事件时崩溃 → 重启 → 从正确 offset 继续 →
//! 验证消费完 1000000 个事件，0 事件丢失, 0 重复。
//!
//! # 设计要点
//!
//! 1. **FailoverConsumer**：模拟真实消费者，包装 `OffsetStore`（持久化 offset）+
//!    `processed_set`（持久化已处理 LSN 集合，模拟下游应用的 idempotent 状态）
//! 2. **崩溃模拟**：通过 `crash_at: Option<u64>` 指定崩溃点，处理到该 LSN 后设置
//!    `is_crashed = true`，后续 `process_event` 立即返回 `Crashed`
//! 3. **恢复流程**：`recover()` 清除崩溃标志；下次 `process_event` 从 `committed_lsn + 1` 继续
//! 4. **Exactly-once 语义**：
//!    - at-least-once：`OffsetStore` 持久化 committed_lsn，崩溃后从 committed+1 重投
//!    - idempotent：`processed_set` 持久化已处理 LSN，重投时去重
//!    - 组合保证 exactly-once：每个事件恰好处理一次
//! 5. **批量提交**：每处理 `commit_batch_size` 个事件提交一次 offset（默认 1000）
//! 6. **持久化**：
//!    - offset 文件：`OffsetStore` 内部原子写入（tmp + rename）
//!    - processed 文件：`FailoverConsumer::flush_processed` 原子写入

use crate::ChangeEvent;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::Mutex;
use szrsql_tx::consumer_offset::{OffsetStore, OffsetStoreError};

// =====================================================================
// ProcessResult — 单个事件处理结果
// =====================================================================

/// 单个事件处理结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessResult {
    /// 正常处理（首次处理，加入 processed set）
    Processed,
    /// 跳过（LSN <= committed_lsn，已提交过）
    SkippedCommitted,
    /// 跳过（重投但已处理过，去重生效）
    SkippedDuplicate,
    /// 消费者已崩溃，未处理
    Crashed,
}

// =====================================================================
// FailoverConsumer — 故障转移消费者
// =====================================================================

/// 故障转移消费者 — 模拟真实 CDC 消费者，支持崩溃/恢复
///
/// **状态**：
/// - `offset_store`：持久化 committed_lsn（OffsetStore）
/// - `processed`：内存中的已处理 LSN 集合（模拟下游应用的 idempotent 状态）
/// - `processed_path`：processed 集合的持久化文件路径
/// - `crash_at`：崩溃点 LSN（处理到此 LSN 后崩溃）
/// - `is_crashed`：崩溃标志
/// - `commit_batch_size`：批量提交间隔（每处理 N 个新事件提交一次 offset）
/// - `processed_since_commit`：自上次提交以来处理的新事件数
///
/// **崩溃语义**：
/// - 处理到 `crash_at` LSN 的事件后，设置 `is_crashed = true`
/// - 后续 `process_event` 立即返回 `Crashed`
/// - `recover()` 清除崩溃标志，重新加载 processed set
///
/// **Exactly-once 保证**：
/// - 崩溃前已 commit 的事件：恢复后从 committed+1 开始，不会重投
/// - 崩溃前已 mark_processed 但未 commit 的事件：恢复后从 committed+1 重投，
///   但 processed set 持久化后能去重
/// - 崩溃前未 mark_processed 的事件：恢复后从 committed+1 重投并处理
pub struct FailoverConsumer {
    /// 消费者组 ID
    consumer_group: String,
    /// 分区 ID（CDC 场景下通常为 table_id）
    partition: u32,
    /// Offset 存储（持久化 committed_lsn）
    offset_store: Arc<OffsetStore>,
    /// 已处理 LSN 集合（模拟下游应用状态，需持久化）
    processed: Mutex<HashSet<u64>>,
    /// processed 集合的持久化文件路径
    processed_path: PathBuf,
    /// 崩溃点 LSN（处理到此 LSN 后崩溃）
    crash_at: Option<u64>,
    /// 崩溃标志
    is_crashed: AtomicBool,
    /// 批量提交间隔
    commit_batch_size: u64,
    /// 自上次提交以来处理的新事件数
    processed_since_commit: AtomicU64,
    /// 总处理事件数（仅统计 Processed 结果）
    total_processed: AtomicU64,
    /// 总跳过事件数（SkippedCommitted + SkippedDuplicate）
    total_skipped: AtomicU64,
}

impl FailoverConsumer {
    /// 创建新的故障转移消费者
    ///
    /// **参数**：
    /// - `offset_store`：OffsetStore 实例（持久化或 in-memory）
    /// - `processed_path`：processed 集合的持久化文件路径
    /// - `consumer_group`：消费者组 ID
    /// - `partition`：分区 ID
    pub fn new(
        offset_store: Arc<OffsetStore>,
        processed_path: impl AsRef<Path>,
        consumer_group: &str,
        partition: u32,
    ) -> Self {
        Self {
            consumer_group: consumer_group.to_string(),
            partition,
            offset_store,
            processed: Mutex::new(HashSet::new()),
            processed_path: processed_path.as_ref().to_path_buf(),
            crash_at: None,
            is_crashed: AtomicBool::new(false),
            commit_batch_size: 1000,
            processed_since_commit: AtomicU64::new(0),
            total_processed: AtomicU64::new(0),
            total_skipped: AtomicU64::new(0),
        }
    }

    /// 设置崩溃点（builder 模式）
    ///
    /// 处理到 LSN == crash_at 的事件后，消费者"崩溃"（设置 is_crashed = true）
    pub fn with_crash_point(mut self, crash_at: u64) -> Self {
        self.crash_at = Some(crash_at);
        self
    }

    /// 设置批量提交间隔（builder 模式）
    pub fn with_commit_batch_size(mut self, batch_size: u64) -> Self {
        self.commit_batch_size = batch_size;
        self
    }

    /// 从持久化文件恢复 processed 集合
    ///
    /// **行为**：
    /// - 若文件存在且格式正确，加载 LSN 集合到内存
    /// - 若文件不存在，初始化为空集合
    pub fn load_processed(&self) -> Result<(), std::io::Error> {
        if self.processed_path.exists() {
            let json = std::fs::read_to_string(&self.processed_path)?;
            let vec: Vec<u64> = serde_json::from_str(&json)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let mut processed = self.processed.lock();
            processed.clear();
            for lsn in vec {
                processed.insert(lsn);
            }
        }
        Ok(())
    }

    /// 持久化 processed 集合到文件（原子写入：tmp + rename）
    pub fn flush_processed(&self) -> Result<(), std::io::Error> {
        let processed = self.processed.lock();
        let mut sorted: Vec<u64> = processed.iter().copied().collect();
        sorted.sort_unstable();
        let json = serde_json::to_string(&sorted)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let tmp_path = self.processed_path.with_extension("tmp");
        std::fs::write(&tmp_path, &json)?;
        std::fs::rename(&tmp_path, &self.processed_path)?;
        Ok(())
    }

    /// 恢复：清除崩溃标志
    ///
    /// 调用后消费者可继续处理事件。调用方应先 `load_processed()` 恢复 processed set。
    pub fn recover(&self) {
        self.is_crashed.store(false, Ordering::SeqCst);
    }

    /// 检查是否已崩溃
    pub fn is_crashed(&self) -> bool {
        self.is_crashed.load(Ordering::SeqCst)
    }

    /// 处理单个事件
    ///
    /// **流程**：
    /// 1. 若已崩溃，返回 `Crashed`
    /// 2. 若 LSN <= committed_lsn，返回 `SkippedCommitted`
    /// 3. 检查 processed set（持久化，跨 session 去重）：
    ///    - 若已存在，返回 `SkippedDuplicate`
    /// 4. 调用 `mark_processed(lsn)` 去重（in-memory dedup window，session 内去重）：
    ///    - 返回 false 表示已处理过，返回 `SkippedDuplicate`
    ///    - 返回 true 表示首次处理
    /// 5. 若设置了 crash_at 且 LSN == crash_at，触发崩溃：
    ///    - 设置 is_crashed = true
    ///    - 返回 `Crashed`（此事件未真正"完成"，恢复后会重投）
    /// 6. 否则返回 `Processed`
    /// 7. 每 commit_batch_size 个新事件，提交一次 offset
    pub fn process_event(&self, event: &ChangeEvent) -> ProcessResult {
        if self.is_crashed.load(Ordering::SeqCst) {
            return ProcessResult::Crashed;
        }

        let lsn = event.lsn;

        // 检查是否已提交
        let committed = self
            .offset_store
            .get_offset(&self.consumer_group, self.partition);
        if let Some(committed_lsn) = committed {
            if lsn <= committed_lsn {
                self.total_skipped.fetch_add(1, Ordering::SeqCst);
                return ProcessResult::SkippedCommitted;
            }
        }

        // 去重第一层：检查 processed set（持久化，跨 session 去重）
        // 这一层在崩溃恢复后生效：dedup window 丢失，但 processed set 持久化
        if self.is_in_processed_set(lsn) {
            self.total_skipped.fetch_add(1, Ordering::SeqCst);
            return ProcessResult::SkippedDuplicate;
        }

        // 去重第二层：mark_processed（in-memory dedup window，session 内去重）
        // 这一层处理 session 内的重投递（未崩溃情况）
        let is_new = self
            .offset_store
            .mark_processed(&self.consumer_group, self.partition, lsn);
        if !is_new {
            self.total_skipped.fetch_add(1, Ordering::SeqCst);
            return ProcessResult::SkippedDuplicate;
        }

        // 检查崩溃点（在加入 processed set 之前崩溃，恢复后会重投）
        // 注意：此时 mark_processed 已经加入 dedup window，但 dedup window 是内存的，
        // 崩溃后丢失。所以恢复后会重新 mark_processed，返回 true。
        // 但 processed set（持久化）不包含此 LSN（崩溃前未加入），所以不会被去重。
        if let Some(crash_at) = self.crash_at {
            if lsn == crash_at {
                self.is_crashed.store(true, Ordering::SeqCst);
                return ProcessResult::Crashed;
            }
        }

        // 加入 processed set
        {
            let mut processed = self.processed.lock();
            processed.insert(lsn);
        }
        self.total_processed.fetch_add(1, Ordering::SeqCst);

        // 批量提交
        let since = self.processed_since_commit.fetch_add(1, Ordering::SeqCst) + 1;
        if since >= self.commit_batch_size {
            if self
                .offset_store
                .commit_offset(&self.consumer_group, self.partition, lsn)
                .is_err()
            {
                // commit 失败不中断处理，下次重试
            } else {
                self.processed_since_commit.store(0, Ordering::SeqCst);
            }
        }

        ProcessResult::Processed
    }

    /// 提交当前 offset（强制提交，不等待 batch_size）
    pub fn commit(&self, lsn: u64) -> Result<(), OffsetStoreError> {
        self.offset_store
            .commit_offset(&self.consumer_group, self.partition, lsn)?;
        self.processed_since_commit.store(0, Ordering::SeqCst);
        Ok(())
    }

    /// 获取下次应该消费的起始 LSN（committed_lsn + 1）
    pub fn next_lsn(&self) -> u64 {
        self.offset_store
            .get_offset(&self.consumer_group, self.partition)
            .unwrap_or(0)
            + 1
    }

    /// 已处理事件总数（Processed 结果计数）
    pub fn total_processed(&self) -> u64 {
        self.total_processed.load(Ordering::SeqCst)
    }

    /// 已跳过事件总数（SkippedCommitted + SkippedDuplicate）
    pub fn total_skipped(&self) -> u64 {
        self.total_skipped.load(Ordering::SeqCst)
    }

    /// processed set 大小
    pub fn processed_set_size(&self) -> usize {
        self.processed.lock().len()
    }

    /// 检查某个 LSN 是否在 processed set 中
    pub fn is_in_processed_set(&self, lsn: u64) -> bool {
        self.processed.lock().contains(&lsn)
    }

    /// 获取已提交的 LSN
    pub fn committed_lsn(&self) -> Option<u64> {
        self.offset_store
            .get_offset(&self.consumer_group, self.partition)
    }

    /// 消费者组名
    pub fn consumer_group(&self) -> &str {
        &self.consumer_group
    }

    /// 分区 ID
    pub fn partition(&self) -> u32 {
        self.partition
    }
}

// =====================================================================
// 辅助函数
// =====================================================================

/// 生成唯一的临时文件路径（用于 offset 持久化）
pub fn make_temp_path(test_name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id();
    let filename = format!("szrsql_failover_{}_{}_{}.json", test_name, pid, timestamp);
    path.push(filename);
    path
}

/// 清理临时文件（包括 .tmp 文件和 .processed 文件）
pub fn cleanup_temp_files(path: &Path) {
    let _ = std::fs::remove_file(path);
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let processed = path.with_extension("processed");
    let _ = std::fs::remove_file(&processed);
}

/// 生成 processed set 的持久化路径
pub fn processed_path_for(offset_path: &Path) -> PathBuf {
    offset_path.with_extension("processed")
}

/// 构造指定数量的 Insert ChangeEvent 列表
///
/// - `tx_id`：事务 ID（所有事件同一事务）
/// - `start_lsn`：起始 LSN（含）
/// - `count`：事件数量
/// - `table_id`：目标表 ID
/// - `timestamp`：固定时间戳
pub fn make_insert_events(
    tx_id: u32,
    start_lsn: u64,
    count: u64,
    table_id: u32,
    timestamp: u64,
) -> Vec<ChangeEvent> {
    let mut events = Vec::with_capacity(count as usize);
    for i in 0..count {
        let lsn = start_lsn + i;
        let row_data = format!("row_{}", lsn).into_bytes();
        events.push(ChangeEvent::insert(
            tx_id, lsn, table_id, row_data, timestamp,
        ));
    }
    events
}

/// 构造混合 op 类型的 ChangeEvent 列表
///
/// 按 Insert / Update / Delete 循环生成，用于测试 chaos 场景下 op 类型不影响故障转移
pub fn make_mixed_events(
    tx_id: u32,
    start_lsn: u64,
    count: u64,
    table_id: u32,
    timestamp: u64,
) -> Vec<ChangeEvent> {
    let mut events = Vec::with_capacity(count as usize);
    for i in 0..count {
        let lsn = start_lsn + i;
        let row_data = format!("row_{}", lsn).into_bytes();
        let event = match i % 3 {
            0 => ChangeEvent::insert(tx_id, lsn, table_id, row_data, timestamp),
            1 => ChangeEvent::update(
                tx_id,
                lsn,
                table_id,
                format!("old_{}", lsn).into_bytes(),
                row_data,
                timestamp,
            ),
            2 => ChangeEvent::delete(tx_id, lsn, table_id, row_data, timestamp),
            _ => unreachable!(),
        };
        events.push(event);
    }
    events
}

#[cfg(test)]
#[path = "failover_tests.rs"]
mod failover_tests;
