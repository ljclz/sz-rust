//! UNDO 日志 + Flashback Query
//!
//! 对应实施进度表 Phase 2.16。
//!
//! 设计参考：
//! - Oracle UNDO 段：保存前镜像（before-image）用于事务回滚和一致性读
//! - PostgreSQL UNDO 链：通过版本链实现 MVCC 和 Flashback Query
//! - MariaDB InnoDB Undo Log：按事务组织 UNDO entries
//!
//! # 核心概念
//!
//! - **UndoEntry**：单条 UNDO 记录，包含 before_value（前镜像）和 after_value（后镜像）
//! - **UndoManager**：管理所有 UNDO entries，支持事务回滚、Flashback Query、自动回收
//! - **版本链**：按 key 组织的 UNDO entries 链，按 LSN 倒序排列（最新在前）
//! - **Flashback Query**：查询 key 在指定 LSN 或时间点的历史值
//! - **UNDO 空间回收**：清理早于 `min_retain_lsn` 的 UNDO entries
//!
//! # 与 MVCC 的关系
//!
//! MVCC 通过 xmin/xmax 实现行版本可见性；UNDO 日志提供更细粒度的"值级"历史，
//! 支持任意时间点的 Flashback Query 和事务级回滚（与 MVCC 的 abort 互补）。

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
// P0-6：使用 parking_lot::RwLock 替代 std::sync::RwLock，消除中毒 panic 风险
use parking_lot::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

// =====================================================================
// UndoOp — UNDO 操作类型
// =====================================================================

/// UNDO 操作类型
///
/// - `Insert`：插入新行；UNDO 时删除该行
/// - `Update`：更新行；UNDO 时恢复 before_value
/// - `Delete`：删除行；UNDO 时恢复 before_value
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoOp {
    Insert,
    Update,
    Delete,
}

impl UndoOp {
    /// 反向操作（UNDO 时执行的操作）
    pub fn reverse(&self) -> &'static str {
        match self {
            UndoOp::Insert => "delete (reverse of insert)",
            UndoOp::Update => "restore before_value (reverse of update)",
            UndoOp::Delete => "restore before_value (reverse of delete)",
        }
    }
}

// =====================================================================
// UndoEntry — 单条 UNDO 记录
// =====================================================================

/// 单条 UNDO 记录
///
/// 记录一次行修改操作的前镜像和后镜像，用于事务回滚和 Flashback Query。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoEntry {
    /// 日志序列号（单调递增，全局唯一）
    pub lsn: u64,
    /// 事务 ID
    pub txn_id: u32,
    /// 行键（格式：`table:row`，如 `users:1`）
    pub key: String,
    /// 操作类型
    pub op: UndoOp,
    /// 修改前的值（None 表示插入前不存在）
    pub before_value: Option<Vec<u8>>,
    /// 修改后的值（None 表示删除后不存在）
    pub after_value: Option<Vec<u8>>,
    /// 创建时间戳（微秒）
    pub timestamp: u64,
    /// 提交时间戳（None 表示未提交，Some(t) 表示已提交）
    pub commit_timestamp: Option<u64>,
}

impl UndoEntry {
    /// 创建新的 UNDO entry（未提交状态）
    pub fn new(
        lsn: u64,
        txn_id: u32,
        key: impl Into<String>,
        op: UndoOp,
        before_value: Option<Vec<u8>>,
        after_value: Option<Vec<u8>>,
    ) -> Self {
        Self {
            lsn,
            txn_id,
            key: key.into(),
            op,
            before_value,
            after_value,
            timestamp: now_micros(),
            commit_timestamp: None,
        }
    }

    /// 是否已提交
    pub fn is_committed(&self) -> bool {
        self.commit_timestamp.is_some()
    }
}

// =====================================================================
// UndoError — UNDO 错误类型
// =====================================================================

/// UNDO 错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UndoError {
    #[error("undo entry with lsn {0} not found")]
    EntryNotFound(u64),
    #[error("transaction {0} not found in undo log")]
    TxnNotFound(u32),
    #[error("transaction {0} already committed")]
    TxnAlreadyCommitted(u32),
    #[error("transaction {0} already aborted/rolled back")]
    TxnAlreadyAborted(u32),
    #[error("key {0} not found in undo log")]
    KeyNotFound(String),
    #[error("no version of key {0} found before lsn {1}")]
    NoVersionBeforeLsn(String, u64),
    #[error("no version of key {0} found before timestamp {1}")]
    NoVersionBeforeTimestamp(String, u64),
    #[error("invalid undo op: {0}")]
    InvalidOp(String),
    #[error("undo entry {0} cannot be purged (still in retention window)")]
    EntryInRetention(u64),
}

// =====================================================================
// HistoryVersion — Flashback Query 返回的历史版本
// =====================================================================

/// Flashback Query 返回的历史版本
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryVersion {
    /// 行键
    pub key: String,
    /// 该版本的值（None 表示该版本时行不存在）
    pub value: Option<Vec<u8>>,
    /// 产生此版本的 LSN
    pub lsn: u64,
    /// 产生此版本的事务 ID
    pub txn_id: u32,
    /// 产生此版本的时间戳
    pub timestamp: u64,
    /// 产生此版本的操作类型
    pub op: UndoOp,
}

// =====================================================================
// RestoreOps — 事务回滚返回的恢复操作列表
// =====================================================================

/// 事务回滚返回的恢复操作列表
///
/// 每个元素为 `(key, restored_value)`：
/// - `restored_value = Some(v)`：将该 key 恢复为 v（UNDO update/delete）
/// - `restored_value = None`：删除该 key（UNDO insert）
pub type RestoreOps = Vec<(String, Option<Vec<u8>>)>;

// =====================================================================
// UndoManager — UNDO 管理器
// =====================================================================

/// UNDO 管理器
///
/// 管理所有 UNDO entries，支持：
/// - 记录插入/更新/删除的 UNDO entries
/// - 事务提交时标记 UNDO entries 为已提交
/// - 事务回滚时按 UNDO entries 恢复 before_value
/// - Flashback Query：查询 key 在指定 LSN/时间点的值
/// - UNDO 空间自动回收：清理早于 `min_retain_lsn` 的 UNDO entries
///
/// **线程安全**：所有公共方法通过 `RwLock` 保证并发安全
pub struct UndoManager {
    /// 所有 UNDO entries，按 LSN 索引（BTreeMap 按 LSN 排序）
    entries: RwLock<BTreeMap<u64, UndoEntry>>,
    /// 按 key 索引的 LSN 列表（按 LSN 升序，BTreeMap 保证顺序）
    key_lsns: RwLock<HashMap<String, Vec<u64>>>,
    /// 按 txn_id 索引的 LSN 列表
    txn_lsns: RwLock<HashMap<u32, Vec<u64>>>,
    /// 已提交事务的提交时间戳（txn_id -> commit_timestamp）
    committed_txns: RwLock<HashMap<u32, u64>>,
    /// 已回滚事务集合
    aborted_txns: RwLock<std::collections::HashSet<u32>>,
    /// 最小保留 LSN（早于此 LSN 的已提交 UNDO entries 可被回收）
    min_retain_lsn: RwLock<u64>,
    /// 下一个 LSN（单调递增）
    next_lsn: AtomicU64,
}

impl Default for UndoManager {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoManager {
    /// 创建新的 UNDO 管理器
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(BTreeMap::new()),
            key_lsns: RwLock::new(HashMap::new()),
            txn_lsns: RwLock::new(HashMap::new()),
            committed_txns: RwLock::new(HashMap::new()),
            aborted_txns: RwLock::new(std::collections::HashSet::new()),
            min_retain_lsn: RwLock::new(0),
            next_lsn: AtomicU64::new(1),
        }
    }

    /// 创建新的 UNDO 管理器，指定初始 LSN
    pub fn with_initial_lsn(initial_lsn: u64) -> Self {
        Self {
            entries: RwLock::new(BTreeMap::new()),
            key_lsns: RwLock::new(HashMap::new()),
            txn_lsns: RwLock::new(HashMap::new()),
            committed_txns: RwLock::new(HashMap::new()),
            aborted_txns: RwLock::new(std::collections::HashSet::new()),
            min_retain_lsn: RwLock::new(initial_lsn),
            next_lsn: AtomicU64::new(initial_lsn + 1),
        }
    }

    // -----------------------------------------------------------------
    // 内部辅助方法
    // -----------------------------------------------------------------

    /// 分配下一个 LSN
    fn alloc_lsn(&self) -> u64 {
        self.next_lsn.fetch_add(1, Ordering::SeqCst)
    }

    /// 检查事务是否已提交
    fn is_txn_committed(&self, txn_id: u32) -> bool {
        self.committed_txns.read().contains_key(&txn_id)
    }

    /// 检查事务是否已回滚
    fn is_txn_aborted(&self, txn_id: u32) -> bool {
        self.aborted_txns.read().contains(&txn_id)
    }

    // -----------------------------------------------------------------
    // UNDO 记录 API
    // -----------------------------------------------------------------

    /// 记录插入操作的 UNDO
    ///
    /// - 插入前：行不存在（before_value = None）
    /// - 插入后：行 = value（after_value = Some(value)）
    /// - UNDO 时：删除该行
    pub fn record_insert(
        &self,
        txn_id: u32,
        key: impl Into<String>,
        value: Vec<u8>,
    ) -> Result<u64, UndoError> {
        self.record_op(txn_id, key, UndoOp::Insert, None, Some(value))
    }

    /// 记录更新操作的 UNDO
    ///
    /// - 更新前：行 = before_value
    /// - 更新后：行 = after_value
    /// - UNDO 时：恢复 before_value
    pub fn record_update(
        &self,
        txn_id: u32,
        key: impl Into<String>,
        before_value: Vec<u8>,
        after_value: Vec<u8>,
    ) -> Result<u64, UndoError> {
        self.record_op(
            txn_id,
            key,
            UndoOp::Update,
            Some(before_value),
            Some(after_value),
        )
    }

    /// 记录删除操作的 UNDO
    ///
    /// - 删除前：行 = before_value
    /// - 删除后：行不存在（after_value = None）
    /// - UNDO 时：恢复 before_value
    pub fn record_delete(
        &self,
        txn_id: u32,
        key: impl Into<String>,
        before_value: Vec<u8>,
    ) -> Result<u64, UndoError> {
        self.record_op(txn_id, key, UndoOp::Delete, Some(before_value), None)
    }

    /// 通用 UNDO 记录方法
    fn record_op(
        &self,
        txn_id: u32,
        key: impl Into<String>,
        op: UndoOp,
        before_value: Option<Vec<u8>>,
        after_value: Option<Vec<u8>>,
    ) -> Result<u64, UndoError> {
        // 校验事务状态
        if self.is_txn_committed(txn_id) {
            return Err(UndoError::TxnAlreadyCommitted(txn_id));
        }
        if self.is_txn_aborted(txn_id) {
            return Err(UndoError::TxnAlreadyAborted(txn_id));
        }

        // 校验操作语义
        match op {
            UndoOp::Insert => {
                if before_value.is_some() {
                    return Err(UndoError::InvalidOp(
                        "Insert op should have before_value=None, got Some".to_string(),
                    ));
                }
                if after_value.is_none() {
                    return Err(UndoError::InvalidOp(
                        "Insert op should have after_value=Some, got None".to_string(),
                    ));
                }
            }
            UndoOp::Update => {
                if before_value.is_none() || after_value.is_none() {
                    return Err(UndoError::InvalidOp(
                        "Update op should have both before_value and after_value".to_string(),
                    ));
                }
            }
            UndoOp::Delete => {
                if before_value.is_none() {
                    return Err(UndoError::InvalidOp(
                        "Delete op should have before_value=Some, got None".to_string(),
                    ));
                }
                if after_value.is_some() {
                    return Err(UndoError::InvalidOp(
                        "Delete op should have after_value=None, got Some".to_string(),
                    ));
                }
            }
        }

        let lsn = self.alloc_lsn();
        let key_str = key.into();
        let entry = UndoEntry::new(lsn, txn_id, key_str.clone(), op, before_value, after_value);

        // 写入 entries
        self.entries.write().insert(lsn, entry);

        // 更新 key_lsns 索引
        self.key_lsns.write().entry(key_str).or_default().push(lsn);

        // 更新 txn_lsns 索引
        self.txn_lsns.write().entry(txn_id).or_default().push(lsn);

        Ok(lsn)
    }

    // -----------------------------------------------------------------
    // 事务提交/回滚 API
    // -----------------------------------------------------------------

    /// 标记事务的 UNDO entries 为已提交
    ///
    /// 提交后这些 entries 可以被 Flashback Query 查询到。
    /// 已提交事务的 UNDO entries 不能再被回滚。
    pub fn commit_txn(&self, txn_id: u32) -> Result<u64, UndoError> {
        if self.is_txn_committed(txn_id) {
            return Err(UndoError::TxnAlreadyCommitted(txn_id));
        }
        if self.is_txn_aborted(txn_id) {
            return Err(UndoError::TxnAlreadyAborted(txn_id));
        }

        let commit_ts = now_micros();
        let mut entries = self.entries.write();

        // 标记该事务的所有 entries 为已提交
        let txn_lsns = self
            .txn_lsns
            .read()
            .get(&txn_id)
            .cloned()
            .unwrap_or_default();

        for lsn in &txn_lsns {
            if let Some(entry) = entries.get_mut(lsn) {
                entry.commit_timestamp = Some(commit_ts);
            }
        }

        // 记录已提交事务
        self.committed_txns.write().insert(txn_id, commit_ts);

        Ok(commit_ts)
    }

    /// 回滚事务：恢复所有 UNDO entries 的 before_value
    ///
    /// 返回回滚操作列表 `(key, restored_value)`，调用方可据此恢复实际数据。
    /// 已回滚事务的 UNDO entries 不会被清理（保留用于审计），
    /// 但不能再次回滚或提交。
    pub fn rollback_txn(&self, txn_id: u32) -> Result<RestoreOps, UndoError> {
        if self.is_txn_committed(txn_id) {
            return Err(UndoError::TxnAlreadyCommitted(txn_id));
        }
        if self.is_txn_aborted(txn_id) {
            return Err(UndoError::TxnAlreadyAborted(txn_id));
        }

        let entries = self.entries.read();
        let txn_lsns = self
            .txn_lsns
            .read()
            .get(&txn_id)
            .cloned()
            .unwrap_or_default();

        if txn_lsns.is_empty() {
            // 无 UNDO entries，但仍需标记为已回滚
            self.aborted_txns.write().insert(txn_id);
            return Ok(Vec::new());
        }

        // 按 LSN 倒序恢复（后改的先恢复，避免中间状态不一致）
        let mut restore_ops = Vec::with_capacity(txn_lsns.len());
        let mut sorted_lsns = txn_lsns.clone();
        sorted_lsns.sort_by(|a, b| b.cmp(a)); // 倒序

        for lsn in sorted_lsns {
            if let Some(entry) = entries.get(&lsn) {
                restore_ops.push((entry.key.clone(), entry.before_value.clone()));
            }
        }

        // 标记为已回滚
        self.aborted_txns.write().insert(txn_id);

        Ok(restore_ops)
    }

    // -----------------------------------------------------------------
    // Flashback Query API
    // -----------------------------------------------------------------

    /// 查询 key 在指定 LSN 时的值
    ///
    /// 返回该 LSN 之前最后一个**已提交**版本对该 key 的修改结果。
    /// - 若最后操作是 Insert/Update：返回 Some(value)
    /// - 若最后操作是 Delete：返回 None（行已不存在）
    /// - 若无任何已提交版本：返回 Err(NoVersionBeforeLsn)
    pub fn flashback_query_at_lsn(
        &self,
        key: &str,
        as_of_lsn: u64,
    ) -> Result<Option<Vec<u8>>, UndoError> {
        let entries = self.entries.read();
        let key_lsns = self.key_lsns.read();

        let lsns = key_lsns
            .get(key)
            .ok_or_else(|| UndoError::KeyNotFound(key.to_string()))?;

        // 找到 <= as_of_lsn 的最大 LSN，且该 entry 必须是已提交的
        // 倒序遍历，找第一个已提交的
        let mut candidate_lsns: Vec<u64> = lsns
            .iter()
            .copied()
            .filter(|&lsn| lsn <= as_of_lsn)
            .collect();
        candidate_lsns.sort_by(|a, b| b.cmp(a)); // 倒序

        for lsn in candidate_lsns {
            if let Some(entry) = entries.get(&lsn) {
                if entry.is_committed() {
                    // 找到该 LSN 对应的已提交版本
                    return Ok(entry.after_value.clone());
                }
            }
        }

        // 没有找到已提交版本
        Err(UndoError::NoVersionBeforeLsn(key.to_string(), as_of_lsn))
    }

    /// 查询 key 在指定时间点的值
    ///
    /// 返回该时间点之前最后一个**已提交**版本对该 key 的修改结果。
    pub fn flashback_query_at_time(
        &self,
        key: &str,
        as_of_timestamp: u64,
    ) -> Result<Option<Vec<u8>>, UndoError> {
        let entries = self.entries.read();
        let key_lsns = self.key_lsns.read();

        let lsns = key_lsns
            .get(key)
            .ok_or_else(|| UndoError::KeyNotFound(key.to_string()))?;

        // 收集所有已提交且 commit_timestamp <= as_of_timestamp 的 entries
        // 按 commit_timestamp 倒序，取第一个
        let mut candidates: Vec<&UndoEntry> = lsns
            .iter()
            .filter_map(|lsn| entries.get(lsn))
            .filter(|e| {
                e.is_committed() && e.commit_timestamp.is_some_and(|ts| ts <= as_of_timestamp)
            })
            .collect();
        candidates.sort_by(|a, b| {
            b.commit_timestamp
                .cmp(&a.commit_timestamp)
                .then_with(|| b.lsn.cmp(&a.lsn))
        });

        if let Some(entry) = candidates.first() {
            return Ok(entry.after_value.clone());
        }

        Err(UndoError::NoVersionBeforeTimestamp(
            key.to_string(),
            as_of_timestamp,
        ))
    }

    /// 获取 key 的所有已提交历史版本（按 LSN 升序）
    pub fn get_history(&self, key: &str) -> Result<Vec<HistoryVersion>, UndoError> {
        let entries = self.entries.read();
        let key_lsns = self.key_lsns.read();

        let lsns = key_lsns
            .get(key)
            .ok_or_else(|| UndoError::KeyNotFound(key.to_string()))?;

        let mut history = Vec::new();
        for &lsn in lsns.iter() {
            if let Some(entry) = entries.get(&lsn) {
                if entry.is_committed() {
                    history.push(HistoryVersion {
                        key: entry.key.clone(),
                        value: entry.after_value.clone(),
                        lsn: entry.lsn,
                        txn_id: entry.txn_id,
                        timestamp: entry.commit_timestamp.unwrap_or(entry.timestamp),
                        op: entry.op,
                    });
                }
            }
        }

        // 按 LSN 升序
        history.sort_by_key(|v| v.lsn);
        Ok(history)
    }

    // -----------------------------------------------------------------
    // UNDO 空间回收 API
    // -----------------------------------------------------------------

    /// 设置最小保留 LSN
    ///
    /// 早于此 LSN 的已提交 UNDO entries 可被 `purge()` 回收。
    /// 通常设置为最老活跃事务的快照 LSN。
    pub fn set_min_retain_lsn(&self, lsn: u64) {
        let mut retain = self.min_retain_lsn.write();
        *retain = lsn;
    }

    /// 获取最小保留 LSN
    pub fn min_retain_lsn(&self) -> u64 {
        *self.min_retain_lsn.read()
    }

    /// 清理早于 `min_retain_lsn` 的已提交 UNDO entries
    ///
    /// 返回清理的 entries 数量。
    /// **注意**：未提交/已回滚的 entries 不会被清理（保留用于审计）。
    pub fn purge(&self) -> Result<usize, UndoError> {
        let min_retain = *self.min_retain_lsn.read();

        let mut entries = self.entries.write();
        let mut key_lsns = self.key_lsns.write();
        let mut txn_lsns = self.txn_lsns.write();

        let mut purged_count = 0;
        let mut lsns_to_purge = Vec::new();

        // 找出所有可回收的 LSN（已提交 + LSN < min_retain）
        for (&lsn, entry) in entries.iter() {
            if lsn < min_retain && entry.is_committed() {
                lsns_to_purge.push(lsn);
            }
        }

        // 从 entries 中删除
        for lsn in &lsns_to_purge {
            entries.remove(lsn);
            purged_count += 1;
        }

        // 从 key_lsns 中删除
        for lsns in key_lsns.values_mut() {
            lsns.retain(|lsn| !lsns_to_purge.contains(lsn));
        }
        // 移除空的 key 条目
        key_lsns.retain(|_, lsns| !lsns.is_empty());

        // 从 txn_lsns 中删除
        for lsns in txn_lsns.values_mut() {
            lsns.retain(|lsn| !lsns_to_purge.contains(lsn));
        }
        // 移除空的 txn 条目
        txn_lsns.retain(|_, lsns| !lsns.is_empty());

        Ok(purged_count)
    }

    // -----------------------------------------------------------------
    // 查询 API
    // -----------------------------------------------------------------

    /// 获取 entry 数量（包含已提交和未提交）
    pub fn entry_count(&self) -> usize {
        self.entries.read().len()
    }

    /// 获取已提交 entry 数量
    pub fn committed_entry_count(&self) -> usize {
        self.entries
            .read()
            .values()
            .filter(|e| e.is_committed())
            .count()
    }

    /// 获取已提交事务数量
    pub fn committed_txn_count(&self) -> usize {
        self.committed_txns.read().len()
    }

    /// 获取已回滚事务数量
    pub fn aborted_txn_count(&self) -> usize {
        self.aborted_txns.read().len()
    }

    /// 获取某事务的 UNDO entries 数量
    pub fn txn_entry_count(&self, txn_id: u32) -> usize {
        self.txn_lsns
            .read()
            .get(&txn_id)
            .map(|lsns| lsns.len())
            .unwrap_or(0)
    }

    /// 获取某 key 的版本数量（包含已提交和未提交）
    pub fn key_version_count(&self, key: &str) -> usize {
        self.key_lsns
            .read()
            .get(key)
            .map(|lsns| lsns.len())
            .unwrap_or(0)
    }

    /// 获取当前最大 LSN
    pub fn current_lsn(&self) -> u64 {
        self.next_lsn.load(Ordering::SeqCst).saturating_sub(1)
    }

    /// 获取事务状态（用于测试和调试）
    pub fn txn_status(&self, txn_id: u32) -> &'static str {
        if self.is_txn_committed(txn_id) {
            "committed"
        } else if self.is_txn_aborted(txn_id) {
            "aborted"
        } else {
            "active"
        }
    }
}

// =====================================================================
// 辅助函数
// =====================================================================

/// 当前时间戳（微秒）
fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // =================================================================
    // Phase 2.16 测试模块 — UNDO 日志 + Flashback Query
    //
    // 验证标准（来自实施进度表）：
    // - UNDO 记录写入/回滚/清理
    // - Flashback Query 查询历史版本
    // - UNDO 空间自动回收
    // - 回滚后数据恢复到修改前
    // - Flashback 查询结果正确
    // =================================================================

    mod phase_2_16 {
        use super::*;

        // -----------------------------------------------------------------
        // 1. UNDO 记录写入 — 基础操作
        // -----------------------------------------------------------------

        #[test]
        fn undo_record_insert_basic() {
            let mgr = UndoManager::new();
            let lsn = mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();

            assert!(lsn > 0, "LSN 应为正数");
            assert_eq!(mgr.entry_count(), 1);
            assert_eq!(mgr.txn_entry_count(1), 1);
            assert_eq!(mgr.key_version_count("users:1"), 1);
        }

        #[test]
        fn undo_record_update_basic() {
            let mgr = UndoManager::new();
            let lsn = mgr
                .record_update(1, "users:1", b"alice".to_vec(), b"alice_v2".to_vec())
                .unwrap();

            assert!(lsn > 0);
            assert_eq!(mgr.entry_count(), 1);
        }

        #[test]
        fn undo_record_delete_basic() {
            let mgr = UndoManager::new();
            let lsn = mgr.record_delete(1, "users:1", b"alice".to_vec()).unwrap();

            assert!(lsn > 0);
            assert_eq!(mgr.entry_count(), 1);
        }

        // -----------------------------------------------------------------
        // 2. UNDO 操作语义校验
        // -----------------------------------------------------------------

        #[test]
        fn undo_insert_rejects_before_value() {
            let mgr = UndoManager::new();
            let result = mgr.record_op(
                1,
                "k:1",
                UndoOp::Insert,
                Some(b"x".to_vec()),
                Some(b"y".to_vec()),
            );
            assert!(matches!(result, Err(UndoError::InvalidOp(_))));
        }

        #[test]
        fn undo_insert_rejects_none_after_value() {
            let mgr = UndoManager::new();
            let result = mgr.record_op(1, "k:1", UndoOp::Insert, None, None);
            assert!(matches!(result, Err(UndoError::InvalidOp(_))));
        }

        #[test]
        fn undo_update_rejects_none_before_value() {
            let mgr = UndoManager::new();
            let result = mgr.record_op(1, "k:1", UndoOp::Update, None, Some(b"y".to_vec()));
            assert!(matches!(result, Err(UndoError::InvalidOp(_))));
        }

        #[test]
        fn undo_update_rejects_none_after_value() {
            let mgr = UndoManager::new();
            let result = mgr.record_op(1, "k:1", UndoOp::Update, Some(b"x".to_vec()), None);
            assert!(matches!(result, Err(UndoError::InvalidOp(_))));
        }

        #[test]
        fn undo_delete_rejects_none_before_value() {
            let mgr = UndoManager::new();
            let result = mgr.record_op(1, "k:1", UndoOp::Delete, None, None);
            assert!(matches!(result, Err(UndoError::InvalidOp(_))));
        }

        #[test]
        fn undo_delete_rejects_some_after_value() {
            let mgr = UndoManager::new();
            let result = mgr.record_op(
                1,
                "k:1",
                UndoOp::Delete,
                Some(b"x".to_vec()),
                Some(b"y".to_vec()),
            );
            assert!(matches!(result, Err(UndoError::InvalidOp(_))));
        }

        // -----------------------------------------------------------------
        // 3. UNDO 事务回滚 — 数据恢复
        // -----------------------------------------------------------------

        #[test]
        fn undo_rollback_insert_restores_deletion() {
            // 插入操作回滚 → 恢复为"行不存在"
            let mgr = UndoManager::new();
            mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();

            let restore_ops = mgr.rollback_txn(1).unwrap();
            assert_eq!(restore_ops.len(), 1);
            // 插入的 UNDO：before_value = None（恢复时删除该行）
            assert_eq!(restore_ops[0].0, "users:1");
            assert_eq!(restore_ops[0].1, None);
        }

        #[test]
        fn undo_rollback_update_restores_before_value() {
            // 更新操作回滚 → 恢复 before_value
            let mgr = UndoManager::new();
            mgr.record_update(1, "users:1", b"alice".to_vec(), b"alice_v2".to_vec())
                .unwrap();

            let restore_ops = mgr.rollback_txn(1).unwrap();
            assert_eq!(restore_ops.len(), 1);
            assert_eq!(restore_ops[0].0, "users:1");
            assert_eq!(restore_ops[0].1, Some(b"alice".to_vec()));
        }

        #[test]
        fn undo_rollback_delete_restores_before_value() {
            // 删除操作回滚 → 恢复 before_value
            let mgr = UndoManager::new();
            mgr.record_delete(1, "users:1", b"alice".to_vec()).unwrap();

            let restore_ops = mgr.rollback_txn(1).unwrap();
            assert_eq!(restore_ops.len(), 1);
            assert_eq!(restore_ops[0].0, "users:1");
            assert_eq!(restore_ops[0].1, Some(b"alice".to_vec()));
        }

        #[test]
        fn undo_rollback_multiple_ops_in_reverse_order() {
            // 同一事务多个操作：按 LSN 倒序恢复
            let mgr = UndoManager::new();
            mgr.record_insert(1, "users:1", b"v1".to_vec()).unwrap();
            mgr.record_update(1, "users:1", b"v1".to_vec(), b"v2".to_vec())
                .unwrap();
            mgr.record_update(1, "users:1", b"v2".to_vec(), b"v3".to_vec())
                .unwrap();
            mgr.record_delete(1, "users:1", b"v3".to_vec()).unwrap();

            let restore_ops = mgr.rollback_txn(1).unwrap();
            assert_eq!(restore_ops.len(), 4);
            // 倒序：delete → update(v2→v3) → update(v1→v2) → insert
            assert_eq!(restore_ops[0].1, Some(b"v3".to_vec())); // undo delete → 恢复 v3
            assert_eq!(restore_ops[1].1, Some(b"v2".to_vec())); // undo update(v2→v3) → 恢复 v2
            assert_eq!(restore_ops[2].1, Some(b"v1".to_vec())); // undo update(v1→v2) → 恢复 v1
            assert_eq!(restore_ops[3].1, None); // undo insert → 删除
        }

        #[test]
        fn undo_rollback_empty_txn_succeeds() {
            // 无 UNDO entries 的事务也可以回滚
            let mgr = UndoManager::new();
            let restore_ops = mgr.rollback_txn(1).unwrap();
            assert!(restore_ops.is_empty());
            assert_eq!(mgr.txn_status(1), "aborted");
        }

        // -----------------------------------------------------------------
        // 4. 事务提交/回滚状态机
        // -----------------------------------------------------------------

        #[test]
        fn undo_commit_txn_marks_entries_committed() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();
            mgr.record_update(1, "users:2", b"bob".to_vec(), b"bob_v2".to_vec())
                .unwrap();

            assert_eq!(mgr.committed_entry_count(), 0);
            mgr.commit_txn(1).unwrap();
            assert_eq!(mgr.committed_entry_count(), 2);
            assert_eq!(mgr.txn_status(1), "committed");
        }

        #[test]
        fn undo_commit_already_committed_txn_fails() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "k:1", b"v".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            let result = mgr.commit_txn(1);
            assert!(matches!(result, Err(UndoError::TxnAlreadyCommitted(1))));
        }

        #[test]
        fn undo_rollback_already_committed_txn_fails() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "k:1", b"v".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            let result = mgr.rollback_txn(1);
            assert!(matches!(result, Err(UndoError::TxnAlreadyCommitted(1))));
        }

        #[test]
        fn undo_rollback_already_aborted_txn_fails() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "k:1", b"v".to_vec()).unwrap();
            mgr.rollback_txn(1).unwrap();

            let result = mgr.rollback_txn(1);
            assert!(matches!(result, Err(UndoError::TxnAlreadyAborted(1))));
        }

        #[test]
        fn undo_commit_already_aborted_txn_fails() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "k:1", b"v".to_vec()).unwrap();
            mgr.rollback_txn(1).unwrap();

            let result = mgr.commit_txn(1);
            assert!(matches!(result, Err(UndoError::TxnAlreadyAborted(1))));
        }

        #[test]
        fn undo_record_on_committed_txn_fails() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "k:1", b"v".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            let result = mgr.record_insert(1, "k:2", b"v2".to_vec());
            assert!(matches!(result, Err(UndoError::TxnAlreadyCommitted(1))));
        }

        #[test]
        fn undo_record_on_aborted_txn_fails() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "k:1", b"v".to_vec()).unwrap();
            mgr.rollback_txn(1).unwrap();

            let result = mgr.record_insert(1, "k:2", b"v2".to_vec());
            assert!(matches!(result, Err(UndoError::TxnAlreadyAborted(1))));
        }

        // -----------------------------------------------------------------
        // 5. Flashback Query — 按 LSN 查询历史版本
        // -----------------------------------------------------------------

        #[test]
        fn flashback_query_at_lsn_returns_value() {
            // T1 INSERT users:1 = "alice" + COMMIT
            // Flashback Query at T1's LSN → Some("alice")
            let mgr = UndoManager::new();
            let lsn = mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            let value = mgr.flashback_query_at_lsn("users:1", lsn).unwrap();
            assert_eq!(value, Some(b"alice".to_vec()));
        }

        #[test]
        fn flashback_query_at_lsn_after_update_returns_new_value() {
            // T1 INSERT users:1 = "alice" + COMMIT
            // T2 UPDATE users:1 → "alice_v2" + COMMIT
            // Flashback Query at T2's LSN → Some("alice_v2")
            // Flashback Query at T1's LSN → Some("alice")
            let mgr = UndoManager::new();
            let lsn1 = mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            let lsn2 = mgr
                .record_update(2, "users:1", b"alice".to_vec(), b"alice_v2".to_vec())
                .unwrap();
            mgr.commit_txn(2).unwrap();

            assert_eq!(
                mgr.flashback_query_at_lsn("users:1", lsn1).unwrap(),
                Some(b"alice".to_vec())
            );
            assert_eq!(
                mgr.flashback_query_at_lsn("users:1", lsn2).unwrap(),
                Some(b"alice_v2".to_vec())
            );
        }

        #[test]
        fn flashback_query_at_lsn_after_delete_returns_none() {
            // T1 INSERT + COMMIT
            // T2 DELETE + COMMIT
            // Flashback Query at T2's LSN → None（行已删除）
            let mgr = UndoManager::new();
            mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            let lsn2 = mgr.record_delete(2, "users:1", b"alice".to_vec()).unwrap();
            mgr.commit_txn(2).unwrap();

            let value = mgr.flashback_query_at_lsn("users:1", lsn2).unwrap();
            assert_eq!(value, None);
        }

        #[test]
        fn flashback_query_at_lsn_before_any_op_returns_err() {
            let mgr = UndoManager::new();
            let lsn = mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            // 查询 lsn-1（早于任何操作）
            let result = mgr.flashback_query_at_lsn("users:1", lsn - 1);
            assert!(matches!(result, Err(UndoError::NoVersionBeforeLsn(_, _))));
        }

        #[test]
        fn flashback_query_uncommitted_entry_ignored() {
            // 未提交的 entries 不应被 Flashback Query 返回
            let mgr = UndoManager::new();
            let lsn = mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();
            // 未提交

            let result = mgr.flashback_query_at_lsn("users:1", lsn);
            assert!(matches!(result, Err(UndoError::NoVersionBeforeLsn(_, _))));
        }

        #[test]
        fn flashback_query_unknown_key_returns_err() {
            let mgr = UndoManager::new();
            let result = mgr.flashback_query_at_lsn("users:999", 100);
            assert!(matches!(result, Err(UndoError::KeyNotFound(_))));
        }

        // -----------------------------------------------------------------
        // 6. Flashback Query — 按时间戳查询历史版本
        // -----------------------------------------------------------------

        #[test]
        fn flashback_query_at_time_returns_value() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();
            let commit_ts = mgr.commit_txn(1).unwrap();

            let value = mgr.flashback_query_at_time("users:1", commit_ts).unwrap();
            assert_eq!(value, Some(b"alice".to_vec()));
        }

        #[test]
        fn flashback_query_at_time_after_update_returns_new_value() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();
            let ts1 = mgr.commit_txn(1).unwrap();

            // 等待一小段时间确保 ts2 > ts1
            std::thread::sleep(std::time::Duration::from_micros(10));

            mgr.record_update(2, "users:1", b"alice".to_vec(), b"alice_v2".to_vec())
                .unwrap();
            let ts2 = mgr.commit_txn(2).unwrap();

            assert_eq!(
                mgr.flashback_query_at_time("users:1", ts1).unwrap(),
                Some(b"alice".to_vec())
            );
            assert_eq!(
                mgr.flashback_query_at_time("users:1", ts2).unwrap(),
                Some(b"alice_v2".to_vec())
            );
        }

        #[test]
        fn flashback_query_at_time_before_any_op_returns_err() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();
            let commit_ts = mgr.commit_txn(1).unwrap();

            let result = mgr.flashback_query_at_time("users:1", commit_ts - 1);
            assert!(matches!(
                result,
                Err(UndoError::NoVersionBeforeTimestamp(_, _))
            ));
        }

        // -----------------------------------------------------------------
        // 7. 历史版本查询 get_history
        // -----------------------------------------------------------------

        #[test]
        fn get_history_returns_all_committed_versions() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "users:1", b"v1".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            mgr.record_update(2, "users:1", b"v1".to_vec(), b"v2".to_vec())
                .unwrap();
            mgr.commit_txn(2).unwrap();

            mgr.record_update(3, "users:1", b"v2".to_vec(), b"v3".to_vec())
                .unwrap();
            mgr.commit_txn(3).unwrap();

            let history = mgr.get_history("users:1").unwrap();
            assert_eq!(history.len(), 3);
            assert_eq!(history[0].value, Some(b"v1".to_vec()));
            assert_eq!(history[1].value, Some(b"v2".to_vec()));
            assert_eq!(history[2].value, Some(b"v3".to_vec()));
            // LSN 升序
            assert!(history[0].lsn < history[1].lsn);
            assert!(history[1].lsn < history[2].lsn);
        }

        #[test]
        fn get_history_excludes_uncommitted() {
            let mgr = UndoManager::new();
            mgr.record_insert(1, "users:1", b"v1".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            // 未提交的更新
            mgr.record_update(2, "users:1", b"v1".to_vec(), b"v2".to_vec())
                .unwrap();

            let history = mgr.get_history("users:1").unwrap();
            assert_eq!(history.len(), 1, "只应返回已提交的版本");
            assert_eq!(history[0].value, Some(b"v1".to_vec()));
        }

        #[test]
        fn get_history_unknown_key_returns_err() {
            let mgr = UndoManager::new();
            let result = mgr.get_history("users:999");
            assert!(matches!(result, Err(UndoError::KeyNotFound(_))));
        }

        // -----------------------------------------------------------------
        // 8. UNDO 空间自动回收
        // -----------------------------------------------------------------

        #[test]
        fn purge_removes_old_committed_entries() {
            let mgr = UndoManager::new();

            // 提交若干低 LSN 的 entries
            let _lsn1 = mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();
            let lsn2 = mgr.record_insert(2, "k:2", b"v2".to_vec()).unwrap();
            mgr.commit_txn(2).unwrap();

            // 设置 min_retain_lsn = lsn2 + 1（早于此 LSN 的可回收）
            mgr.set_min_retain_lsn(lsn2 + 1);

            let purged = mgr.purge().unwrap();
            assert_eq!(purged, 2, "应清理 2 个已提交 entries");
            assert_eq!(mgr.entry_count(), 0, "应全部清理");
        }

        #[test]
        fn purge_keeps_uncommitted_entries() {
            let mgr = UndoManager::new();

            // 未提交的 entry
            let _lsn1 = mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();

            // 已提交的 entry
            let lsn2 = mgr.record_insert(2, "k:2", b"v2".to_vec()).unwrap();
            mgr.commit_txn(2).unwrap();

            // 设置 min_retain_lsn > lsn2
            mgr.set_min_retain_lsn(lsn2 + 1);

            let purged = mgr.purge().unwrap();
            assert_eq!(purged, 1, "只应清理 1 个已提交 entry");
            assert_eq!(mgr.entry_count(), 1, "应保留未提交 entry");
            assert_eq!(mgr.txn_entry_count(1), 1);
            assert_eq!(mgr.txn_entry_count(2), 0);
        }

        #[test]
        fn purge_keeps_entries_in_retention_window() {
            let mgr = UndoManager::new();

            let lsn1 = mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            let _lsn2 = mgr.record_insert(2, "k:2", b"v2".to_vec()).unwrap();
            mgr.commit_txn(2).unwrap();

            // 设置 min_retain_lsn = lsn1（lsn1 不应被清理，因为不 < lsn1）
            mgr.set_min_retain_lsn(lsn1);

            let purged = mgr.purge().unwrap();
            assert_eq!(purged, 0, "lsn1 不应被清理（不 < min_retain_lsn）");
            assert_eq!(mgr.entry_count(), 2);
        }

        #[test]
        fn purge_cleans_key_index() {
            let mgr = UndoManager::new();

            mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            assert_eq!(mgr.key_version_count("k:1"), 1);

            mgr.set_min_retain_lsn(mgr.current_lsn() + 1);
            let purged = mgr.purge().unwrap();
            assert_eq!(purged, 1);

            // key 应从索引中移除
            assert_eq!(mgr.key_version_count("k:1"), 0);
        }

        #[test]
        fn purge_cleans_txn_index() {
            let mgr = UndoManager::new();

            mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            assert_eq!(mgr.txn_entry_count(1), 1);

            mgr.set_min_retain_lsn(mgr.current_lsn() + 1);
            let purged = mgr.purge().unwrap();
            assert_eq!(purged, 1);

            // txn 应从索引中移除
            assert_eq!(mgr.txn_entry_count(1), 0);
        }

        // -----------------------------------------------------------------
        // 9. 综合 — 回滚后数据恢复到修改前
        // -----------------------------------------------------------------

        #[test]
        fn rollback_restores_data_to_before_modification() {
            // 模拟完整的修改-回滚流程：
            // 1. 初始状态：users:1 = "alice"
            // 2. T1: UPDATE users:1 → "bob"
            // 3. T1 rollback
            // 4. Flashback Query 验证 users:1 仍是 "alice"
            let mgr = UndoManager::new();

            // 初始状态（假设 users:1 = "alice" 已存在）
            // T1 记录 UPDATE 操作
            mgr.record_update(1, "users:1", b"alice".to_vec(), b"bob".to_vec())
                .unwrap();

            // 回滚 T1
            let restore_ops = mgr.rollback_txn(1).unwrap();
            assert_eq!(restore_ops[0].1, Some(b"alice".to_vec()));

            // 应用恢复操作（实际数据库中由调用方应用）
            // 这里只验证 UNDO 返回的恢复值正确
            assert_eq!(restore_ops[0].1, Some(b"alice".to_vec()));
        }

        #[test]
        fn rollback_then_commit_other_txn_works() {
            // T1 UPDATE users:1 → "bob"（未提交）
            // T1 rollback
            // T2 INSERT users:2 → "carol" + COMMIT
            // T2 Flashback Query 正常工作
            let mgr = UndoManager::new();

            mgr.record_update(1, "users:1", b"alice".to_vec(), b"bob".to_vec())
                .unwrap();
            mgr.rollback_txn(1).unwrap();

            let lsn2 = mgr.record_insert(2, "users:2", b"carol".to_vec()).unwrap();
            mgr.commit_txn(2).unwrap();

            // Flashback Query T2 的 INSERT
            assert_eq!(
                mgr.flashback_query_at_lsn("users:2", lsn2).unwrap(),
                Some(b"carol".to_vec())
            );

            // T1 的 UPDATE 未提交（已回滚），get_history 返回空 Vec（无已提交版本）
            assert!(mgr.get_history("users:1").unwrap().is_empty());
        }

        // -----------------------------------------------------------------
        // 10. 并发 — 多事务同时操作不同 key
        // -----------------------------------------------------------------

        #[test]
        fn concurrent_txns_different_keys_independent() {
            let mgr = UndoManager::new();

            // T1, T2, T3 并发写不同 key
            let lsn1 = mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            let lsn2 = mgr.record_insert(2, "k:2", b"v2".to_vec()).unwrap();
            let lsn3 = mgr.record_insert(3, "k:3", b"v3".to_vec()).unwrap();

            mgr.commit_txn(1).unwrap();
            mgr.commit_txn(2).unwrap();
            mgr.commit_txn(3).unwrap();

            // 各自 Flashback Query 独立
            assert_eq!(
                mgr.flashback_query_at_lsn("k:1", lsn1).unwrap(),
                Some(b"v1".to_vec())
            );
            assert_eq!(
                mgr.flashback_query_at_lsn("k:2", lsn2).unwrap(),
                Some(b"v2".to_vec())
            );
            assert_eq!(
                mgr.flashback_query_at_lsn("k:3", lsn3).unwrap(),
                Some(b"v3".to_vec())
            );
        }

        #[test]
        fn concurrent_txns_same_key_independent_flashback() {
            // T1, T2 顺序写同一 key（模拟并发场景）
            let mgr = UndoManager::new();

            let lsn1 = mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            let lsn2 = mgr
                .record_update(2, "k:1", b"v1".to_vec(), b"v2".to_vec())
                .unwrap();
            mgr.commit_txn(2).unwrap();

            // Flashback Query 不同 LSN 得到不同结果
            assert_eq!(
                mgr.flashback_query_at_lsn("k:1", lsn1).unwrap(),
                Some(b"v1".to_vec())
            );
            assert_eq!(
                mgr.flashback_query_at_lsn("k:1", lsn2).unwrap(),
                Some(b"v2".to_vec())
            );
        }

        // -----------------------------------------------------------------
        // 11. LSN 单调递增
        // -----------------------------------------------------------------

        #[test]
        fn lsn_monotonic_increasing() {
            let mgr = UndoManager::new();

            let lsn1 = mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            let lsn2 = mgr.record_insert(1, "k:2", b"v2".to_vec()).unwrap();
            let lsn3 = mgr.record_insert(1, "k:3", b"v3".to_vec()).unwrap();

            assert!(lsn1 < lsn2);
            assert!(lsn2 < lsn3);
            assert_eq!(mgr.current_lsn(), lsn3);
        }

        // -----------------------------------------------------------------
        // 12. with_initial_lsn 构造
        // -----------------------------------------------------------------

        #[test]
        fn with_initial_lsn_starts_at_correct_value() {
            let mgr = UndoManager::with_initial_lsn(1000);
            assert_eq!(mgr.min_retain_lsn(), 1000);

            let lsn = mgr.record_insert(1, "k:1", b"v".to_vec()).unwrap();
            assert_eq!(lsn, 1001);
        }

        // -----------------------------------------------------------------
        // 13. 完整生命周期：INSERT → UPDATE → DELETE → UNDO 全部
        // -----------------------------------------------------------------

        #[test]
        fn full_lifecycle_insert_update_delete_rollback() {
            let mgr = UndoManager::new();

            // T1: INSERT users:1 = "v1"
            mgr.record_insert(1, "users:1", b"v1".to_vec()).unwrap();
            // T1: UPDATE users:1 → "v2"
            mgr.record_update(1, "users:1", b"v1".to_vec(), b"v2".to_vec())
                .unwrap();
            // T1: UPDATE users:1 → "v3"
            mgr.record_update(1, "users:1", b"v2".to_vec(), b"v3".to_vec())
                .unwrap();
            // T1: DELETE users:1
            mgr.record_delete(1, "users:1", b"v3".to_vec()).unwrap();

            assert_eq!(mgr.txn_entry_count(1), 4);

            // 回滚：倒序恢复
            let restore_ops = mgr.rollback_txn(1).unwrap();
            assert_eq!(restore_ops.len(), 4);
            // 顺序：DELETE → UPDATE(v2→v3) → UPDATE(v1→v2) → INSERT
            assert_eq!(restore_ops[0].1, Some(b"v3".to_vec())); // undo delete
            assert_eq!(restore_ops[1].1, Some(b"v2".to_vec())); // undo update(v2→v3)
            assert_eq!(restore_ops[2].1, Some(b"v1".to_vec())); // undo update(v1→v2)
            assert_eq!(restore_ops[3].1, None); // undo insert

            // T1 已回滚
            assert_eq!(mgr.txn_status(1), "aborted");
            assert_eq!(mgr.aborted_txn_count(), 1);
        }

        // -----------------------------------------------------------------
        // 14. 多 key 混合操作回滚
        // -----------------------------------------------------------------

        #[test]
        fn multi_key_mixed_ops_rollback() {
            let mgr = UndoManager::new();

            // T1 操作多个 key
            mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            mgr.record_update(1, "k:2", b"old2".to_vec(), b"new2".to_vec())
                .unwrap();
            mgr.record_delete(1, "k:3", b"old3".to_vec()).unwrap();
            mgr.record_insert(1, "k:4", b"v4".to_vec()).unwrap();

            let restore_ops = mgr.rollback_txn(1).unwrap();
            assert_eq!(restore_ops.len(), 4);

            // 按 LSN 倒序：k:4 → k:3 → k:2 → k:1
            assert_eq!(restore_ops[0].0, "k:4");
            assert_eq!(restore_ops[0].1, None); // undo insert

            assert_eq!(restore_ops[1].0, "k:3");
            assert_eq!(restore_ops[1].1, Some(b"old3".to_vec())); // undo delete

            assert_eq!(restore_ops[2].0, "k:2");
            assert_eq!(restore_ops[2].1, Some(b"old2".to_vec())); // undo update

            assert_eq!(restore_ops[3].0, "k:1");
            assert_eq!(restore_ops[3].1, None); // undo insert
        }

        // -----------------------------------------------------------------
        // 15. UNDO 与 Flashback Query 协同
        // -----------------------------------------------------------------

        #[test]
        fn undo_and_flashback_query_coexist() {
            // T1 INSERT + COMMIT
            // T2 UPDATE + COMMIT
            // T3 UPDATE (未提交)
            // Flashback Query 看到已提交的 T1, T2，看不到未提交的 T3
            let mgr = UndoManager::new();

            let lsn1 = mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            let lsn2 = mgr
                .record_update(2, "k:1", b"v1".to_vec(), b"v2".to_vec())
                .unwrap();
            mgr.commit_txn(2).unwrap();

            // T3 未提交
            mgr.record_update(3, "k:1", b"v2".to_vec(), b"v3".to_vec())
                .unwrap();

            // Flashback Query
            assert_eq!(
                mgr.flashback_query_at_lsn("k:1", lsn1).unwrap(),
                Some(b"v1".to_vec())
            );
            assert_eq!(
                mgr.flashback_query_at_lsn("k:1", lsn2).unwrap(),
                Some(b"v2".to_vec())
            );

            // T3 未提交，Flashback Query 看不到 v3
            let history = mgr.get_history("k:1").unwrap();
            assert_eq!(history.len(), 2, "只应看到 2 个已提交版本");

            // T3 回滚
            mgr.rollback_txn(3).unwrap();
            // 历史仍只有 2 个版本
            let history = mgr.get_history("k:1").unwrap();
            assert_eq!(history.len(), 2);
        }

        // -----------------------------------------------------------------
        // 16. UndoOp::reverse 反向操作描述
        // -----------------------------------------------------------------

        #[test]
        fn undo_op_reverse_description() {
            assert_eq!(UndoOp::Insert.reverse(), "delete (reverse of insert)");
            assert_eq!(
                UndoOp::Update.reverse(),
                "restore before_value (reverse of update)"
            );
            assert_eq!(
                UndoOp::Delete.reverse(),
                "restore before_value (reverse of delete)"
            );
        }

        // -----------------------------------------------------------------
        // 17. 查询 API
        // -----------------------------------------------------------------

        #[test]
        fn query_apis_work_correctly() {
            let mgr = UndoManager::new();

            // 初始状态
            assert_eq!(mgr.entry_count(), 0);
            assert_eq!(mgr.committed_entry_count(), 0);
            assert_eq!(mgr.committed_txn_count(), 0);
            assert_eq!(mgr.aborted_txn_count(), 0);
            assert_eq!(mgr.current_lsn(), 0);
            assert_eq!(mgr.min_retain_lsn(), 0);
            assert_eq!(mgr.txn_status(1), "active");

            // 记录一些 entries
            mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            mgr.record_update(1, "k:2", b"v2".to_vec(), b"v3".to_vec())
                .unwrap();
            mgr.commit_txn(1).unwrap();

            assert_eq!(mgr.entry_count(), 2);
            assert_eq!(mgr.committed_entry_count(), 2);
            assert_eq!(mgr.committed_txn_count(), 1);
            assert_eq!(mgr.txn_status(1), "committed");
            assert_eq!(mgr.txn_entry_count(1), 2);
            assert_eq!(mgr.key_version_count("k:1"), 1);
            assert_eq!(mgr.key_version_count("k:2"), 1);

            // T2 回滚
            mgr.record_insert(2, "k:3", b"v3".to_vec()).unwrap();
            mgr.rollback_txn(2).unwrap();

            assert_eq!(mgr.aborted_txn_count(), 1);
            assert_eq!(mgr.txn_status(2), "aborted");
        }

        // -----------------------------------------------------------------
        // 18. 跨表 UNDO 操作
        // -----------------------------------------------------------------

        #[test]
        fn cross_table_undo_operations() {
            let mgr = UndoManager::new();

            // T1 跨表操作
            mgr.record_insert(1, "users:1", b"alice".to_vec()).unwrap();
            mgr.record_insert(1, "orders:1", b"order_data".to_vec())
                .unwrap();
            mgr.record_update(1, "users:1", b"alice".to_vec(), b"alice_v2".to_vec())
                .unwrap();
            mgr.commit_txn(1).unwrap();

            // Flashback Query 各表独立
            assert_eq!(
                mgr.flashback_query_at_lsn("users:1", mgr.current_lsn())
                    .unwrap(),
                Some(b"alice_v2".to_vec())
            );
            assert_eq!(
                mgr.flashback_query_at_lsn("orders:1", mgr.current_lsn())
                    .unwrap(),
                Some(b"order_data".to_vec())
            );
        }

        // -----------------------------------------------------------------
        // 19. 大量历史版本查询
        // -----------------------------------------------------------------

        #[test]
        fn many_history_versions_query() {
            let mgr = UndoManager::new();

            // 100 次更新
            let mut prev = b"v0".to_vec();
            for i in 1..=100 {
                let curr = format!("v{}", i).into_bytes();
                mgr.record_update(i, "k:1", prev.clone(), curr.clone())
                    .unwrap();
                mgr.commit_txn(i).unwrap();
                prev = curr;
            }

            let history = mgr.get_history("k:1").unwrap();
            assert_eq!(history.len(), 100);
            assert_eq!(history[0].value, Some(b"v1".to_vec()));
            assert_eq!(history[99].value, Some(b"v100".to_vec()));

            // Flashback Query 最后一个版本
            assert_eq!(
                mgr.flashback_query_at_lsn("k:1", mgr.current_lsn())
                    .unwrap(),
                Some(b"v100".to_vec())
            );
        }

        // -----------------------------------------------------------------
        // 20. UNDO 回收后 Flashback Query 失效
        // -----------------------------------------------------------------

        #[test]
        fn flashback_query_fails_after_purge() {
            let mgr = UndoManager::new();

            let lsn1 = mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            // Flashback Query 正常工作
            assert_eq!(
                mgr.flashback_query_at_lsn("k:1", lsn1).unwrap(),
                Some(b"v1".to_vec())
            );

            // 回收
            mgr.set_min_retain_lsn(lsn1 + 1);
            mgr.purge().unwrap();

            // Flashback Query 应失败（key 已被回收）
            let result = mgr.flashback_query_at_lsn("k:1", lsn1);
            assert!(matches!(result, Err(UndoError::KeyNotFound(_))));
        }

        // -----------------------------------------------------------------
        // 21. 多次 purge 幂等
        // -----------------------------------------------------------------

        #[test]
        fn purge_idempotent() {
            let mgr = UndoManager::new();

            mgr.record_insert(1, "k:1", b"v1".to_vec()).unwrap();
            mgr.commit_txn(1).unwrap();

            mgr.set_min_retain_lsn(mgr.current_lsn() + 1);

            let purged1 = mgr.purge().unwrap();
            assert_eq!(purged1, 1);

            let purged2 = mgr.purge().unwrap();
            assert_eq!(purged2, 0, "第二次 purge 应无清理");
        }

        // -----------------------------------------------------------------
        // 22. set_min_retain_lsn 单调性
        // -----------------------------------------------------------------

        #[test]
        fn set_min_retain_lsn_overwrites() {
            let mgr = UndoManager::new();

            mgr.set_min_retain_lsn(100);
            assert_eq!(mgr.min_retain_lsn(), 100);

            // 可以后续调整
            mgr.set_min_retain_lsn(200);
            assert_eq!(mgr.min_retain_lsn(), 200);

            // 也可以调小（实现允许）
            mgr.set_min_retain_lsn(50);
            assert_eq!(mgr.min_retain_lsn(), 50);
        }
    }
}
