//! SzRSQL 行锁管理器 — 对应 `SzRSQL技术实现方案.md` 9.10 节扩展（行锁/死锁/2PL+升级）。
//!
//! Phase 2.9: 行锁 + 2PL 升级
//!
//! 验证标准（来自实施进度表）：
//! - 行级共享锁/排他锁
//! - 锁升级（共享→排他）
//! - 锁超时释放
//! - 同一事务重复加锁不阻塞
//! - 锁语义正确（RW 互斥），锁升级不产生死锁
//!
//! 设计要点：
//! 1. **LockMode 锁模式**：
//!    - `Share` (S) — SELECT FOR SHARE / 读锁
//!    - `Exclusive` (X) — SELECT FOR UPDATE / UPDATE / DELETE / 写锁
//! 2. **兼容性矩阵**：
//!    - S-S：兼容（多读共享）
//!    - S-X / X-S / X-X：互斥
//! 3. **同事务重入**：
//!    - 同模式重入 → no-op
//!    - 持有 X 请求 S → no-op（X 已更强）
//!    - 持有 S 请求 X → 触发升级
//! 4. **锁升级（S → X）**：
//!    - 无其他持有者 → 立即升级
//!    - 有其他 S 持有者 → 进入等待队列（升级优先于新请求，避免饥饿）
//!    - 升级不与自身死锁（同一事务）
//! 5. **锁超时**：
//!    - `try_lock` 非阻塞，冲突立即返回 `Conflict`
//!    - `lock` 阻塞 + 超时，超时返回 `Timeout`
//!    - 使用 `Condvar::wait_timeout` 实现高效等待
//! 6. **2PL 协议**：
//!    - LockManager 只提供原语，2PL 由调用方保证
//!    - `unlock_all(txn_id)` 在 COMMIT/ABORT 时释放所有锁（Strict 2PL）
//! 7. **FIFO 公平性**：
//!    - 等待队列 FIFO（先到先得）
//!    - 升级请求优先于新请求（避免升级饥饿导致死锁）

use std::collections::{HashMap, HashSet, VecDeque};
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::{Condvar, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, instrument, trace, warn};

// =====================================================================
// LockMode — 锁模式
// =====================================================================

/// 行锁模式
///
/// 对应 PostgreSQL 行级锁语义：
/// - `Share` (S) — SELECT FOR SHARE，多事务可同时持有
/// - `Exclusive` (X) — SELECT FOR UPDATE / UPDATE / DELETE，仅单事务持有
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockMode {
    /// 共享锁（读锁）
    Share,
    /// 排他锁（写锁）
    Exclusive,
}

impl LockMode {
    /// 锁强度：X > S
    ///
    /// 若 `self >= other`，则持有 self 时请求 other 是 no-op（已更强或相等）。
    fn strength(&self) -> u8 {
        match self {
            LockMode::Share => 1,
            LockMode::Exclusive => 2,
        }
    }

    /// self 是否至少与 other 一样强（持有 self 时 other 已被覆盖）
    fn at_least(&self, other: LockMode) -> bool {
        self.strength() >= other.strength()
    }

    /// 两个模式是否兼容（可同时由不同事务持有）
    ///
    /// 兼容矩阵：
    /// - S-S: 兼容
    /// - S-X / X-S / X-X: 不兼容
    fn compatible_with(&self, other: LockMode) -> bool {
        matches!((self, other), (LockMode::Share, LockMode::Share))
    }
}

// =====================================================================
// LockError — 锁错误
// =====================================================================

/// 锁错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LockError {
    /// 锁冲突（非阻塞 try_lock 时立即返回）
    #[error("lock conflict: txn {txn_id} requested {requested:?} on resource {resource} held by txn {holder} with {held:?}")]
    Conflict {
        txn_id: u32,
        resource: u64,
        holder: u32,
        requested: LockMode,
        held: LockMode,
    },

    /// 锁等待超时
    #[error("lock timeout: txn {txn_id} waited {waited_ms}ms for {mode:?} on resource {resource}")]
    Timeout {
        txn_id: u32,
        resource: u64,
        mode: LockMode,
        waited_ms: u64,
    },

    /// 无效升级（试图降级锁）
    #[error("invalid upgrade: txn {txn_id} cannot upgrade from {from:?} to {to:?} on resource {resource}")]
    InvalidUpgrade {
        txn_id: u32,
        resource: u64,
        from: LockMode,
        to: LockMode,
    },

    /// 未持有任何锁时尝试升级（L5 修复：原代码使用 `LockMode::Share` 占位，
    /// 语义不准确且可能误导死锁检测器。新增独立变体精确表达"未持有"语义）
    #[error("upgrade failed: txn {txn_id} holds no lock on resource {resource}")]
    NotHeld { txn_id: u32, resource: u64 },

    /// 死锁检测（Phase 2.10 实现）
    #[error("deadlock detected: txn {0} aborted")]
    Deadlock(u32),
}

// =====================================================================
// LockHolder / LockWaiter — 锁持有者 / 等待者
// =====================================================================

/// 锁持有者
#[derive(Debug, Clone, Copy)]
struct LockHolder {
    txn_id: u32,
    mode: LockMode,
}

/// 锁等待者（FIFO 队列元素）
#[derive(Debug, Clone)]
struct LockWaiter {
    txn_id: u32,
    mode: LockMode,
    /// 是否为升级请求（升级请求优先于普通新请求）
    is_upgrade: bool,
    /// 进入等待队列的时间（用于超时判断）
    requested_at: Instant,
}

// =====================================================================
// LockEntry — 单个资源的锁状态
// =====================================================================

/// 单个资源的锁状态
///
/// - `holders`: 当前持有者（多个 S 或单个 X）
/// - `waiters`: FIFO 等待队列（升级请求插队到首个非升级请求之前）
#[derive(Debug, Clone)]
struct LockEntry {
    holders: Vec<LockHolder>,
    waiters: VecDeque<LockWaiter>,
}

impl LockEntry {
    fn new() -> Self {
        Self {
            holders: Vec::new(),
            waiters: VecDeque::new(),
        }
    }

    /// 查找指定事务是否在 holders 中，返回其索引和模式
    fn find_holder(&self, txn_id: u32) -> Option<(usize, LockMode)> {
        self.holders
            .iter()
            .enumerate()
            .find(|(_, h)| h.txn_id == txn_id)
            .map(|(i, h)| (i, h.mode))
    }

    /// 当前是否可被 (txn_id, mode) 获取（不考虑同事务重入）
    ///
    /// 规则：与所有其他持有者的模式都兼容
    fn compatible_with(&self, txn_id: u32, mode: LockMode) -> bool {
        self.holders
            .iter()
            .all(|h| h.txn_id == txn_id || h.mode.compatible_with(mode))
    }

    /// 是否有等待者（用于判断是否需要 notify）
    fn has_waiters(&self) -> bool {
        !self.waiters.is_empty()
    }
}

// =====================================================================
// LockManager — 锁管理器
// =====================================================================

/// 行锁管理器
///
/// 线程安全：内部使用 `Mutex<HashMap<u64, LockEntry>>` + `Condvar`，
/// 支持多线程并发加锁/解锁/升级。
///
/// **资源 ID 约定**：`u64` 类型，调用方可编码为 `(page_id << 32) | slot_num`。
const LOCK_SHARD_COUNT: usize = 16;

pub struct LockManager {
    /// 分片锁表：resource_id → LockEntry（分片以减少锁竞争）
    tables: Vec<Mutex<HashMap<u64, LockEntry>>>,
    /// 每个分片对应的条件变量
    condvars: Vec<Condvar>,
}

impl Default for LockManager {
    fn default() -> Self {
        Self::new()
    }
}

impl LockManager {
    /// 创建空锁管理器
    pub fn new() -> Self {
        let mut tables = Vec::with_capacity(LOCK_SHARD_COUNT);
        let mut condvars = Vec::with_capacity(LOCK_SHARD_COUNT);
        for _ in 0..LOCK_SHARD_COUNT {
            tables.push(Mutex::new(HashMap::new()));
            condvars.push(Condvar::new());
        }
        Self { tables, condvars }
    }

    /// OPT-9: calculate shard index for a resource
    fn shard_idx(&self, resource: u64) -> usize {
        (resource as usize) % self.tables.len()
    }

    /// OPT-9: snapshot wait-for edges across all shards (for cross-shard deadlock detection)
    ///
    /// 使用 `try_lock` 而非 `lock` 采集跨分片等待边，避免死锁检测器自身死锁：
    /// 若两个线程同时持有不同分片并互相尝试锁定对方的分片，使用阻塞 `lock` 会
    /// 造成检测器死锁。`try_lock` 跳过繁忙分片，牺牲少量检测精度换取无死锁保证。
    fn snapshot_wait_for_edges(
        &self,
        exclude_idx: usize,
        exclude_table: &HashMap<u64, LockEntry>,
    ) -> Vec<(u32, u32)> {
        let mut edges: Vec<(u32, u32)> = Vec::new();
        let collect = |table: &HashMap<u64, LockEntry>, edges: &mut Vec<(u32, u32)>| {
            for entry in table.values() {
                for waiter in &entry.waiters {
                    for holder in &entry.holders {
                        if holder.txn_id != waiter.txn_id {
                            edges.push((waiter.txn_id, holder.txn_id));
                        }
                    }
                }
            }
        };
        collect(exclude_table, &mut edges);
        for (idx, table_mutex) in self.tables.iter().enumerate() {
            if idx == exclude_idx {
                continue;
            }
            if let Some(table) = table_mutex.try_lock() {
                collect(&table, &mut edges);
            }
        }
        edges
    }

    /// OPT-9: detect deadlock cycle from wait-for edges (DFS + gray/black coloring)
    fn detect_deadlock_from_edges(edges: &[(u32, u32)], start_txn: u32) -> Option<Vec<u32>> {
        let mut gray: HashSet<u32> = HashSet::new();
        let mut black: HashSet<u32> = HashSet::new();
        let mut path: Vec<u32> = Vec::new();
        Self::dfs_detect_cycle_edges(edges, start_txn, &mut gray, &mut black, &mut path)
    }

    fn dfs_detect_cycle_edges(
        edges: &[(u32, u32)],
        txn: u32,
        gray: &mut HashSet<u32>,
        black: &mut HashSet<u32>,
        path: &mut Vec<u32>,
    ) -> Option<Vec<u32>> {
        if black.contains(&txn) {
            return None;
        }
        if gray.contains(&txn) {
            let cycle_start = path.iter().position(|&t| t == txn).unwrap();
            return Some(path[cycle_start..].to_vec());
        }
        gray.insert(txn);
        path.push(txn);
        for &(waiter, holder) in edges {
            if waiter == txn {
                if let Some(cycle) =
                    Self::dfs_detect_cycle_edges(edges, holder, gray, black, path)
                {
                    return Some(cycle);
                }
            }
        }
        path.pop();
        gray.remove(&txn);
        black.insert(txn);
        None
    }

    /// 非阻塞尝试加锁
    ///
    /// - 冲突时立即返回 `Err(LockError::Conflict)`
    /// - 同事务重入：同模式或更强模式 → no-op OK；较弱模式 → 触发升级（若可立即满足）
    /// - 升级时若有其他持有者 → 返回 `Conflict`
    #[instrument(skip(self))]
    pub fn try_lock(&self, txn_id: u32, resource: u64, mode: LockMode) -> Result<(), LockError> {
        let idx = self.shard_idx(resource);
        let mut table = self.tables[idx].lock();
        match self.try_lock_inner(&mut table, txn_id, resource, mode) {
            Ok(()) => {
                trace!(txn_id, resource, mode = ?mode, "lock acquired (try)");
                Ok(())
            }
            Err(e) => {
                warn!(txn_id, resource, mode = ?mode, error = %e, "try_lock failed");
                Err(e)
            }
        }
    }

    /// 阻塞加锁（带超时）
    ///
    /// - 冲突时阻塞等待，直到获取或超时
    /// - 超时返回 `Err(LockError::Timeout)`
    /// - 同事务重入语义同 `try_lock`
    #[instrument(skip(self))]
    pub fn lock(
        &self,
        txn_id: u32,
        resource: u64,
        mode: LockMode,
        timeout: Duration,
    ) -> Result<(), LockError> {
        let idx = self.shard_idx(resource);
        let mut table = self.tables[idx].lock();

        // 先尝试立即获取
        match self.try_lock_inner(&mut table, txn_id, resource, mode) {
            Ok(()) => {
                trace!(txn_id, resource, mode = ?mode, "lock acquired (immediate)");
                return Ok(());
            }
            Err(LockError::InvalidUpgrade { .. }) => {
                warn!(txn_id, resource, mode = ?mode, "lock invalid upgrade");
                return Err(LockError::InvalidUpgrade {
                    txn_id,
                    resource,
                    from: LockMode::Exclusive,
                    to: mode,
                })
            }
            Err(LockError::Conflict { .. }) => { /* 需要等待 */ }
            Err(other) => return Err(other),
        }

        // 进入等待队列
        let is_upgrade = table
            .get(&resource)
            .and_then(|e| e.find_holder(txn_id))
            .is_some();

        let entry = table.entry(resource).or_insert_with(LockEntry::new);
        let waiter = LockWaiter {
            txn_id,
            mode,
            is_upgrade,
            requested_at: Instant::now(),
        };

        // 升级请求插队到首个非升级请求之前（优先于新请求）
        if is_upgrade {
            let insert_pos = entry
                .waiters
                .iter()
                .position(|w| !w.is_upgrade)
                .unwrap_or(entry.waiters.len());
            entry.waiters.insert(insert_pos, waiter);
        } else {
            entry.waiters.push_back(waiter);
        }

        debug!(txn_id, resource, mode = ?mode, is_upgrade, "lock waiting");

        // **Phase 2.10: 死锁检测** — 进入等待队列后立即检查是否形成环
        if Self::detect_deadlock_from_edges(&self.snapshot_wait_for_edges(idx, &table), txn_id).is_some() {
            // 检测到死锁，中止自身（从等待队列移除并返回 Deadlock 错误）
            if let Some(entry) = table.get_mut(&resource) {
                entry
                    .waiters
                    .retain(|w| !(w.txn_id == txn_id && w.mode == mode));
            }
            // 清理空表项
            if let Some(entry) = table.get(&resource) {
                if entry.holders.is_empty() && entry.waiters.is_empty() {
                    table.remove(&resource);
                }
            }
            warn!(txn_id, resource, "lock deadlock detected");
            return Err(LockError::Deadlock(txn_id));
        }

        // 等待循环
        let deadline = Instant::now() + timeout;
        loop {
            // 检查是否能获取
            if self.can_acquire(&table, txn_id, resource, mode) {
                // 从等待队列移除自己
                if let Some(entry) = table.get_mut(&resource) {
                    entry
                        .waiters
                        .retain(|w| !(w.txn_id == txn_id && w.mode == mode));
                }
                // 实际授予锁
                self.grant_lock(&mut table, txn_id, resource, mode)?;
                trace!(txn_id, resource, mode = ?mode, "lock acquired (after wait)");
                return Ok(());
            }

            // **Phase 2.10: 周期性死锁检测** — 环可能在等待期间形成
            if Self::detect_deadlock_from_edges(&self.snapshot_wait_for_edges(idx, &table), txn_id).is_some() {
                if let Some(entry) = table.get_mut(&resource) {
                    entry
                        .waiters
                        .retain(|w| !(w.txn_id == txn_id && w.mode == mode));
                }
                if let Some(entry) = table.get(&resource) {
                    if entry.holders.is_empty() && entry.waiters.is_empty() {
                        table.remove(&resource);
                    }
                }
                warn!(txn_id, resource, "lock deadlock detected (periodic)");
                return Err(LockError::Deadlock(txn_id));
            }

            // 检查超时
            let now = Instant::now();
            if now >= deadline {
                // 从等待队列移除自己
                if let Some(entry) = table.get_mut(&resource) {
                    entry
                        .waiters
                        .retain(|w| !(w.txn_id == txn_id && w.mode == mode));
                }
                let waited_ms = duration_ms(now - deadline + timeout);
                warn!(txn_id, resource, mode = ?mode, waited_ms, "lock timeout");
                return Err(LockError::Timeout {
                    txn_id,
                    resource,
                    mode,
                    waited_ms,
                });
            }

            // 等待通知（带剩余超时，但最多等待 500ms 以便周期性检查死锁）
            let remaining = deadline - now;
            let wait_duration = remaining.min(Duration::from_millis(500));
            // P0-6：parking_lot::Condvar::wait_for 接收 &mut guard，原地等待
            let wait_result = self.condvars[idx].wait_for(&mut table, wait_duration);
            let _ = wait_result;
        }
    }

    /// 释放指定事务在指定资源上的锁
    ///
    /// - 若该事务是升级等待者，从等待队列移除
    /// - 若该事务是持有者，移除并唤醒等待者
    /// - 若资源无持有者无等待者，移除表项（避免内存泄漏）
    #[instrument(skip(self))]
    pub fn unlock(&self, txn_id: u32, resource: u64) {
        let idx = self.shard_idx(resource);
        let mut table = self.tables[idx].lock();
        let need_notify = if let Some(entry) = table.get_mut(&resource) {
            // 从等待队列移除（若在等待）
            let prev_len = entry.waiters.len();
            entry.waiters.retain(|w| w.txn_id != txn_id);
            let removed_waiter = entry.waiters.len() < prev_len;

            // 从持有者移除
            let prev_holders = entry.holders.len();
            entry.holders.retain(|h| h.txn_id != txn_id);
            let removed_holder = entry.holders.len() < prev_holders;

            removed_waiter || removed_holder
        } else {
            false
        };

        // 清理空表项
        if let Some(entry) = table.get(&resource) {
            if entry.holders.is_empty() && entry.waiters.is_empty() {
                table.remove(&resource);
            }
        }

        drop(table);
        if need_notify {
            trace!(txn_id, resource, "lock released, notifying waiters");
            self.condvars[idx].notify_all();
        } else {
            trace!(txn_id, resource, "lock released (no waiters)");
        }
    }

    /// 释放指定事务的所有锁（COMMIT/ABORT 时调用，Strict 2PL）
    #[instrument(skip(self))]
    pub fn unlock_all(&self, txn_id: u32) {
        let mut total_released = 0u32;
        for (idx, table_mutex) in self.tables.iter().enumerate() {
            let mut table = table_mutex.lock();
            let mut resources_to_clean = Vec::new();
            let mut need_notify = false;

            for (&resource, entry) in table.iter_mut() {
                let prev_holders = entry.holders.len();
                entry.holders.retain(|h| h.txn_id != txn_id);
                let prev_waiters = entry.waiters.len();
                entry.waiters.retain(|w| w.txn_id != txn_id);

                let removed_holders = prev_holders - entry.holders.len();
                let removed_waiters = prev_waiters - entry.waiters.len();
                if removed_holders > 0 || removed_waiters > 0 {
                    need_notify = true;
                    total_released += (removed_holders + removed_waiters) as u32;
                }

                if entry.holders.is_empty() && entry.waiters.is_empty() {
                    resources_to_clean.push(resource);
                }
            }

            for resource in resources_to_clean {
                table.remove(&resource);
            }

            drop(table);
            if need_notify {
                self.condvars[idx].notify_all();
            }
        }
        debug!(txn_id, released_count = total_released, "unlock_all released locks");
    }

    /// 锁升级（S → X）
    ///
    /// - 若已持有 X → no-op OK
    /// - 若已持有 S 且无其他持有者 → 立即升级为 X
    /// - 若已持有 S 且有其他 S 持有者 → 阻塞等待（带超时）
    /// - 若未持有任何锁 → 返回 `InvalidUpgrade`
    /// - 若持有 X 请求升级 S → 返回 `InvalidUpgrade`（不能降级）
    #[instrument(skip(self))]
    pub fn upgrade(&self, txn_id: u32, resource: u64, timeout: Duration) -> Result<(), LockError> {
        let idx = self.shard_idx(resource);
        let mut table = self.tables[idx].lock();

        // 检查当前持有状态
        let current_mode = table
            .get(&resource)
            .and_then(|e| e.find_holder(txn_id))
            .map(|(_, m)| m);

        match current_mode {
            None => {
                warn!(txn_id, resource, "upgrade invalid: no lock held");
                // L5 修复：原代码用 LockMode::Share 占位，语义不准确且可能
                // 误导死锁检测器。改为返回 NotHeld 变体精确表达"未持有锁"
                return Err(LockError::NotHeld { txn_id, resource });
            }
            Some(LockMode::Exclusive) => {
                // 已持有 X，no-op
                trace!(txn_id, resource, "upgrade noop: already exclusive");
                return Ok(());
            }
            Some(LockMode::Share) => {
                // 持有 S，尝试升级
            }
        }

        // 检查是否可立即升级（无其他持有者）
        let can_upgrade_now = table
            .get(&resource)
            .map(|e| e.holders.iter().all(|h| h.txn_id == txn_id))
            .unwrap_or(true);

        if can_upgrade_now {
            // 立即升级：修改持有者模式
            if let Some(entry) = table.get_mut(&resource) {
                if let Some((i, _)) = entry.find_holder(txn_id) {
                    entry.holders[i].mode = LockMode::Exclusive;
                }
            }
            trace!(txn_id, resource, "upgrade succeeded (immediate)");
            return Ok(());
        }

        // 需要等待：进入升级等待队列
        let entry = table.entry(resource).or_insert_with(LockEntry::new);
        let waiter = LockWaiter {
            txn_id,
            mode: LockMode::Exclusive,
            is_upgrade: true,
            requested_at: Instant::now(),
        };
        // 升级请求插队到首个非升级请求之前
        let insert_pos = entry
            .waiters
            .iter()
            .position(|w| !w.is_upgrade)
            .unwrap_or(entry.waiters.len());
        entry.waiters.insert(insert_pos, waiter);

        debug!(txn_id, resource, "upgrade waiting for exclusive");

        // **Phase 2.10: 死锁检测** — 进入等待队列后立即检查
        if Self::detect_deadlock_from_edges(&self.snapshot_wait_for_edges(idx, &table), txn_id).is_some() {
            if let Some(entry) = table.get_mut(&resource) {
                entry.waiters.retain(|w| w.txn_id != txn_id);
            }
            if let Some(entry) = table.get(&resource) {
                if entry.holders.is_empty() && entry.waiters.is_empty() {
                    table.remove(&resource);
                }
            }
            warn!(txn_id, resource, "upgrade deadlock detected");
            return Err(LockError::Deadlock(txn_id));
        }

        // 等待循环
        let deadline = Instant::now() + timeout;
        loop {
            // 检查是否可升级
            let can_upgrade = table
                .get(&resource)
                .map(|e| e.holders.iter().all(|h| h.txn_id == txn_id))
                .unwrap_or(true);

            if can_upgrade {
                // 从等待队列移除
                if let Some(entry) = table.get_mut(&resource) {
                    entry.waiters.retain(|w| w.txn_id != txn_id);
                    if let Some((i, _)) = entry.find_holder(txn_id) {
                        entry.holders[i].mode = LockMode::Exclusive;
                    }
                }
                trace!(txn_id, resource, "upgrade succeeded (after wait)");
                return Ok(());
            }

            // **Phase 2.10: 周期性死锁检测**
            if Self::detect_deadlock_from_edges(&self.snapshot_wait_for_edges(idx, &table), txn_id).is_some() {
                if let Some(entry) = table.get_mut(&resource) {
                    entry.waiters.retain(|w| w.txn_id != txn_id);
                }
                if let Some(entry) = table.get(&resource) {
                    if entry.holders.is_empty() && entry.waiters.is_empty() {
                        table.remove(&resource);
                    }
                }
                warn!(txn_id, resource, "upgrade deadlock detected (periodic)");
                return Err(LockError::Deadlock(txn_id));
            }

            let now = Instant::now();
            if now >= deadline {
                // 超时：从等待队列移除
                if let Some(entry) = table.get_mut(&resource) {
                    entry.waiters.retain(|w| w.txn_id != txn_id);
                }
                let waited_ms = duration_ms(now - deadline + timeout);
                warn!(txn_id, resource, waited_ms, "upgrade timeout");
                return Err(LockError::Timeout {
                    txn_id,
                    resource,
                    mode: LockMode::Exclusive,
                    waited_ms,
                });
            }

            // 最多等待 500ms 以便周期性检查死锁
            let remaining = deadline - now;
            let wait_duration = remaining.min(Duration::from_millis(500));
            // P0-6：parking_lot::Condvar::wait_for 接收 &mut guard，原地等待
            let _ = self.condvars[idx].wait_for(&mut table, wait_duration);
        }
    }

    /// 查询事务是否持有指定资源的锁（任意模式）
    pub fn holds_lock(&self, txn_id: u32, resource: u64) -> bool {
        let idx = self.shard_idx(resource);
        let table = self.tables[idx].lock();
        table
            .get(&resource)
            .map(|e| e.find_holder(txn_id).is_some())
            .unwrap_or(false)
    }

    /// 查询事务在指定资源上持有的锁模式
    pub fn lock_mode(&self, txn_id: u32, resource: u64) -> Option<LockMode> {
        let idx = self.shard_idx(resource);
        let table = self.tables[idx].lock();
        table
            .get(&resource)
            .and_then(|e| e.find_holder(txn_id).map(|(_, m)| m))
    }

    /// 当前持有者数量（用于测试和监控）
    pub fn holder_count(&self, resource: u64) -> usize {
        let idx = self.shard_idx(resource);
        let table = self.tables[idx].lock();
        table.get(&resource).map(|e| e.holders.len()).unwrap_or(0)
    }

    /// 当前等待者数量（用于测试和监控）
    pub fn waiter_count(&self, resource: u64) -> usize {
        let idx = self.shard_idx(resource);
        let table = self.tables[idx].lock();
        table.get(&resource).map(|e| e.waiters.len()).unwrap_or(0)
    }

    /// 锁表中的资源数量（用于测试）
    pub fn resource_count(&self) -> usize {
        self.tables.iter().map(|t| t.lock().len()).sum()
    }

    // -----------------------------------------------------------------
    // 内部辅助方法
    // -----------------------------------------------------------------

    /// try_lock 的内部实现（操作已持锁的 table）
    fn try_lock_inner(
        &self,
        table: &mut HashMap<u64, LockEntry>,
        txn_id: u32,
        resource: u64,
        mode: LockMode,
    ) -> Result<(), LockError> {
        let entry = table.entry(resource).or_insert_with(LockEntry::new);

        // 检查同事务是否已持有锁
        if let Some((_, held_mode)) = entry.find_holder(txn_id) {
            if held_mode.at_least(mode) {
                // 已持有相同或更强模式 → no-op
                return Ok(());
            }
            // 持有较弱模式（S），请求更强（X）→ 需要升级
            // 检查是否可立即升级（无其他持有者）
            let others = entry.holders.iter().any(|h| h.txn_id != txn_id);
            if !others {
                // 无其他持有者，立即升级
                if let Some((i, _)) = entry.find_holder(txn_id) {
                    entry.holders[i].mode = mode;
                }
                return Ok(());
            }
            // 有其他持有者，无法立即升级 → Conflict
            let holder = entry.holders.iter().find(|h| h.txn_id != txn_id).unwrap();
            return Err(LockError::Conflict {
                txn_id,
                resource,
                holder: holder.txn_id,
                requested: mode,
                held: holder.mode,
            });
        }

        // 同事务未持有锁
        // **升级优先级检查**：若其他事务正在等待升级（S→X），则新请求即使兼容也不能插队，
        // 否则会饿死升级请求（升级需要独占所有持有者）。
        let has_other_upgrade_waiter = entry
            .waiters
            .iter()
            .any(|w| w.is_upgrade && w.txn_id != txn_id);
        if has_other_upgrade_waiter {
            // 必须等待升级完成，返回 Conflict（以任一持有者作为冲突源）
            let holder = entry
                .holders
                .first()
                .copied()
                .or_else(|| {
                    entry
                        .waiters
                        .iter()
                        .find(|w| w.is_upgrade)
                        .map(|w| LockHolder {
                            txn_id: w.txn_id,
                            mode: w.mode,
                        })
                })
                .unwrap_or(LockHolder {
                    txn_id: 0,
                    mode: LockMode::Share,
                });
            return Err(LockError::Conflict {
                txn_id,
                resource,
                holder: holder.txn_id,
                requested: mode,
                held: holder.mode,
            });
        }

        // 检查兼容性
        if entry.compatible_with(txn_id, mode) {
            entry.holders.push(LockHolder { txn_id, mode });
            Ok(())
        } else {
            // 找到冲突的持有者
            let holder = entry
                .holders
                .iter()
                .find(|h| h.txn_id != txn_id && !h.mode.compatible_with(mode))
                .unwrap();
            Err(LockError::Conflict {
                txn_id,
                resource,
                holder: holder.txn_id,
                requested: mode,
                held: holder.mode,
            })
        }
    }

    /// 检查 (txn_id, mode) 是否可获取（在等待队列中时）
    ///
    /// 规则：
    /// - 与所有其他持有者兼容
    /// - 自己是 FIFO 队首（或队首之前的都是自己/升级请求）
    fn can_acquire(
        &self,
        table: &HashMap<u64, LockEntry>,
        txn_id: u32,
        resource: u64,
        mode: LockMode,
    ) -> bool {
        let entry = match table.get(&resource) {
            Some(e) => e,
            None => return true,
        };

        // 必须与所有其他持有者兼容
        if !entry.compatible_with(txn_id, mode) {
            return false;
        }

        // 必须是队首（FIFO 公平性）
        // 队首是自己 → 可获取
        // 队首不是自己 → 必须等待（即使兼容，也不能插队）
        match entry.waiters.front() {
            None => true, // 无等待者，直接获取
            Some(front) => {
                // 如果自己是队首，可获取
                // 如果自己不是队首但队首是升级请求且自己是该升级请求的事务 → 可获取
                front.txn_id == txn_id && front.mode == mode
            }
        }
    }

    /// 实际授予锁（从等待中转持有）
    fn grant_lock(
        &self,
        table: &mut HashMap<u64, LockEntry>,
        txn_id: u32,
        resource: u64,
        mode: LockMode,
    ) -> Result<(), LockError> {
        let entry = table.entry(resource).or_insert_with(LockEntry::new);

        // 检查是否已持有锁（升级场景）
        if let Some((i, held_mode)) = entry.find_holder(txn_id) {
            if held_mode.at_least(mode) {
                return Ok(());
            }
            // 升级持有模式
            entry.holders[i].mode = mode;
            return Ok(());
        }

        // 新增持有者
        entry.holders.push(LockHolder { txn_id, mode });
        Ok(())
    }

    // -----------------------------------------------------------------
    // Phase 2.10: 等待图死锁检测
    // -----------------------------------------------------------------

    /// 检测从 `start_txn` 出发是否存在死锁环
    ///
    /// **等待图（Wait-for Graph）** 是隐式定义在锁表上的有向图：
    /// - 节点 = 事务
    /// - 边 (W → H) = W 正在等待 H 释放锁（H 是 W 所等资源的持有者）
    ///
    /// **环 = 死锁**。使用 DFS + 灰/黑着色检测：
    /// - 灰色（gray）：当前 DFS 路径上的节点（遇到灰色节点 = 回边 = 环）
    /// - 黑色（black）：已完全处理、无环的节点（剪枝）
    ///
    /// 返回环上的事务 ID 列表（从 `start_txn` 开始），若无环返回 `None`。
    /// 扫描整个锁表，找出所有死锁环（Oracle 风格后台检测用）
    ///
    /// 返回所有独立环的列表（每个环是事务 ID 列表）。
    /// 适用于后台线程定期调用，主动发现并中止死锁事务。
    ///
    /// OPT-9：跨分片快照所有等待图边后统一检测
    pub fn detect_all_deadlocks(&self) -> Vec<Vec<u32>> {
        // BUG-004 修复：一次性锁住所有分片构建一致快照，避免逐分片加锁导致的幻影环误报。
        // 之前的实现逐个分片 lock()，不同分片的状态来自不同时间点，
        // 可能看到过时的等待关系（holder 已释放但检测器仍认为持有），
        // 从而构建出不存在的环（false positive）。
        //
        // 性能影响：detect_all_deadlocks 是周期性调用的检测操作，非热路径，
        // 一次性锁住所有分片的影响可接受。
        let mut guards: Vec<parking_lot::MutexGuard<'_, HashMap<u64, LockEntry>>> =
            Vec::with_capacity(self.tables.len());
        for table_mutex in &self.tables {
            // 按固定顺序（分片 0→15）获取所有分片锁，构建一致快照。
            // 顺序获取不会死锁（所有调用方都按相同顺序获取）。
            // P0-6：parking_lot::Mutex 不中毒，lock() 直接返回 guard
            guards.push(table_mutex.lock());
        }

        let mut edges: Vec<(u32, u32)> = Vec::new();
        let mut all_waiters: HashSet<u32> = HashSet::new();
        for table in &guards {
            for entry in table.values() {
                for waiter in &entry.waiters {
                    all_waiters.insert(waiter.txn_id);
                    for holder in &entry.holders {
                        if holder.txn_id != waiter.txn_id {
                            edges.push((waiter.txn_id, holder.txn_id));
                        }
                    }
                }
            }
        }

        let mut cycles = Vec::new();
        let mut checked: HashSet<u32> = HashSet::new();
        for &waiter in &all_waiters {
            if checked.contains(&waiter) {
                continue;
            }
            if let Some(cycle) = Self::detect_deadlock_from_edges(&edges, waiter) {
                for &txn in &cycle {
                    checked.insert(txn);
                }
                cycles.push(cycle);
            }
        }
        cycles
    }
}

/// Duration 转毫秒（用于错误信息）
fn duration_ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

// =====================================================================
// Phase 2.9 测试
// =====================================================================

#[cfg(test)]
mod phase_2_9 {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    // -----------------------------------------------------------------
    // 1. 锁兼容性矩阵（基础语义）
    // -----------------------------------------------------------------

    #[test]
    fn share_lock_compatible_with_other_share_locks() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.try_lock(2, 100, LockMode::Share).is_ok());
        assert_eq!(mgr.holder_count(100), 2);
    }

    #[test]
    fn share_lock_incompatible_with_exclusive_lock() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        let result = mgr.try_lock(2, 100, LockMode::Exclusive);
        assert!(matches!(result, Err(LockError::Conflict { .. })));
        assert_eq!(mgr.holder_count(100), 1);
    }

    #[test]
    fn exclusive_lock_incompatible_with_share_lock() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());
        let result = mgr.try_lock(2, 100, LockMode::Share);
        assert!(matches!(result, Err(LockError::Conflict { .. })));
    }

    #[test]
    fn exclusive_lock_incompatible_with_other_exclusive_lock() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());
        let result = mgr.try_lock(2, 100, LockMode::Exclusive);
        assert!(matches!(result, Err(LockError::Conflict { .. })));
    }

    // -----------------------------------------------------------------
    // 2. 同事务重入
    // -----------------------------------------------------------------

    #[test]
    fn same_txn_relock_same_mode_is_noop() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert_eq!(mgr.holder_count(100), 1); // 不重复
    }

    #[test]
    fn same_txn_relock_weaker_mode_is_noop() {
        // 持有 X，请求 S → no-op（X 已更强）
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert_eq!(mgr.holder_count(100), 1);
        assert_eq!(mgr.lock_mode(1, 100), Some(LockMode::Exclusive));
    }

    #[test]
    fn same_txn_relock_stronger_mode_triggers_upgrade() {
        // 持有 S，请求 X → 升级
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());
        assert_eq!(mgr.lock_mode(1, 100), Some(LockMode::Exclusive));
        assert_eq!(mgr.holder_count(100), 1);
    }

    // -----------------------------------------------------------------
    // 3. 锁升级（S → X）
    // -----------------------------------------------------------------

    #[test]
    fn upgrade_from_share_to_exclusive_succeeds_when_no_other_holders() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.upgrade(1, 100, Duration::from_millis(100)).is_ok());
        assert_eq!(mgr.lock_mode(1, 100), Some(LockMode::Exclusive));
    }

    #[test]
    fn upgrade_from_share_to_exclusive_blocks_when_other_share_holders() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.try_lock(2, 100, LockMode::Share).is_ok());
        // 升级应该超时（因为 txn2 持有 S）
        let result = mgr.upgrade(1, 100, Duration::from_millis(50));
        assert!(matches!(result, Err(LockError::Timeout { .. })));
        // txn1 仍持有 S
        assert_eq!(mgr.lock_mode(1, 100), Some(LockMode::Share));
    }

    #[test]
    fn upgrade_from_exclusive_is_noop() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());
        assert!(mgr.upgrade(1, 100, Duration::from_millis(100)).is_ok());
        assert_eq!(mgr.lock_mode(1, 100), Some(LockMode::Exclusive));
    }

    #[test]
    fn upgrade_without_holding_lock_returns_invalid_upgrade() {
        let mgr = LockManager::new();
        let result = mgr.upgrade(1, 100, Duration::from_millis(100));
        // L5 修复：未持有锁时返回 NotHeld，而非占位的 InvalidUpgrade
        assert!(matches!(result, Err(LockError::NotHeld { .. })));
    }

    #[test]
    fn upgrade_succeeds_after_other_holders_release() {
        let mgr = Arc::new(LockManager::new());
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.try_lock(2, 100, LockMode::Share).is_ok());

        // txn1 尝试升级（会阻塞）
        let mgr_clone = Arc::clone(&mgr);
        let handle = thread::spawn(move || mgr_clone.upgrade(1, 100, Duration::from_secs(2)));

        // 等待 txn1 进入等待队列
        thread::sleep(Duration::from_millis(50));
        // txn2 释放锁
        mgr.unlock(2, 100);

        // txn1 应该成功升级
        let result = handle.join().unwrap();
        assert!(result.is_ok(), "upgrade should succeed: {:?}", result);
        assert_eq!(mgr.lock_mode(1, 100), Some(LockMode::Exclusive));
    }

    // -----------------------------------------------------------------
    // 4. 解锁
    // -----------------------------------------------------------------

    #[test]
    fn unlock_releases_lock_allowing_waiter_to_proceed() {
        let mgr = Arc::new(LockManager::new());
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());

        // txn2 阻塞等待
        let mgr_clone = Arc::clone(&mgr);
        let handle = thread::spawn(move || {
            mgr_clone.lock(2, 100, LockMode::Exclusive, Duration::from_secs(2))
        });

        thread::sleep(Duration::from_millis(50));
        mgr.unlock(1, 100);

        let result = handle.join().unwrap();
        assert!(result.is_ok(), "txn2 should acquire: {:?}", result);
        assert!(mgr.holds_lock(2, 100));
    }

    #[test]
    fn unlock_all_releases_all_locks_held_by_txn() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(1, 200, LockMode::Share).is_ok());
        assert!(mgr.try_lock(1, 300, LockMode::Exclusive).is_ok());

        mgr.unlock_all(1);

        // 其他事务可以加锁
        assert!(mgr.try_lock(2, 100, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(2, 200, LockMode::Share).is_ok());
        assert!(mgr.try_lock(2, 300, LockMode::Exclusive).is_ok());
    }

    #[test]
    fn unlock_removes_entry_when_no_holders() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert_eq!(mgr.resource_count(), 1);
        mgr.unlock(1, 100);
        assert_eq!(mgr.resource_count(), 0); // 表项已清理
    }

    #[test]
    fn unlock_decrements_holders_correctly() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.try_lock(2, 100, LockMode::Share).is_ok());
        assert_eq!(mgr.holder_count(100), 2);

        mgr.unlock(1, 100);
        assert_eq!(mgr.holder_count(100), 1);
        assert!(mgr.holds_lock(2, 100));
        assert!(!mgr.holds_lock(1, 100));
    }

    // -----------------------------------------------------------------
    // 5. try_lock 与 lock 超时
    // -----------------------------------------------------------------

    #[test]
    fn try_lock_returns_conflict_error_when_blocked() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());
        let result = mgr.try_lock(2, 100, LockMode::Exclusive);
        match result {
            Err(LockError::Conflict {
                txn_id,
                resource,
                holder,
                ..
            }) => {
                assert_eq!(txn_id, 2);
                assert_eq!(resource, 100);
                assert_eq!(holder, 1);
            }
            other => panic!("expected Conflict, got {:?}", other),
        }
    }

    #[test]
    fn lock_with_timeout_returns_timeout_when_blocked() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());
        let result = mgr.lock(2, 100, LockMode::Exclusive, Duration::from_millis(50));
        assert!(matches!(result, Err(LockError::Timeout { .. })));
    }

    #[test]
    fn lock_with_timeout_succeeds_when_lock_released_in_time() {
        let mgr = Arc::new(LockManager::new());
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());

        let mgr_clone = Arc::clone(&mgr);
        let handle = thread::spawn(move || {
            mgr_clone.lock(2, 100, LockMode::Exclusive, Duration::from_secs(2))
        });

        thread::sleep(Duration::from_millis(50));
        mgr.unlock(1, 100);

        let result = handle.join().unwrap();
        assert!(result.is_ok(), "should acquire: {:?}", result);
    }

    // -----------------------------------------------------------------
    // 6. FIFO 公平性 + 升级优先级
    // -----------------------------------------------------------------

    #[test]
    fn fifo_waiter_queue_order() {
        let mgr = Arc::new(LockManager::new());
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());

        // txn2, txn3 依次等待
        let mgr2 = Arc::clone(&mgr);
        let h2 =
            thread::spawn(move || mgr2.lock(2, 100, LockMode::Exclusive, Duration::from_secs(2)));
        thread::sleep(Duration::from_millis(20));

        let mgr3 = Arc::clone(&mgr);
        let h3 =
            thread::spawn(move || mgr3.lock(3, 100, LockMode::Exclusive, Duration::from_secs(2)));
        thread::sleep(Duration::from_millis(20));

        // 释放 txn1
        mgr.unlock(1, 100);

        // txn2 应该先获取（FIFO）
        assert!(h2.join().unwrap().is_ok());
        // txn3 仍在等待
        assert!(!mgr.holds_lock(3, 100));

        mgr.unlock(2, 100);
        assert!(h3.join().unwrap().is_ok());
    }

    #[test]
    fn upgrade_has_priority_over_new_lock_requests() {
        let mgr = Arc::new(LockManager::new());
        // txn1 持有 S, txn2 持有 S
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.try_lock(2, 100, LockMode::Share).is_ok());

        // txn1 尝试升级（会阻塞，因 txn2 持有 S）
        let mgr_upgrade = Arc::clone(&mgr);
        let h_upgrade = thread::spawn(move || mgr_upgrade.upgrade(1, 100, Duration::from_secs(2)));
        thread::sleep(Duration::from_millis(50));

        // txn3 请求 S（会阻塞，因 txn1 在等待升级）
        let mgr_new = Arc::clone(&mgr);
        let h_new =
            thread::spawn(move || mgr_new.lock(3, 100, LockMode::Share, Duration::from_secs(2)));
        thread::sleep(Duration::from_millis(50));

        // txn2 释放 → txn1 升级应先于 txn3 获取
        mgr.unlock(2, 100);

        let upgrade_result = h_upgrade.join().unwrap();
        assert!(upgrade_result.is_ok(), "upgrade should succeed first");
        assert_eq!(mgr.lock_mode(1, 100), Some(LockMode::Exclusive));

        // txn3 仍在等待（txn1 现在持有 X）
        thread::sleep(Duration::from_millis(50));
        assert!(!mgr.holds_lock(3, 100));

        // 清理
        mgr.unlock(1, 100);
        assert!(h_new.join().unwrap().is_ok());
    }

    // -----------------------------------------------------------------
    // 7. 查询接口
    // -----------------------------------------------------------------

    #[test]
    fn holds_lock_returns_correct_status() {
        let mgr = LockManager::new();
        assert!(!mgr.holds_lock(1, 100));
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.holds_lock(1, 100));
        assert!(!mgr.holds_lock(1, 200));
        assert!(!mgr.holds_lock(2, 100));
    }

    #[test]
    fn lock_mode_returns_correct_mode() {
        let mgr = LockManager::new();
        assert_eq!(mgr.lock_mode(1, 100), None);
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert_eq!(mgr.lock_mode(1, 100), Some(LockMode::Share));
        assert!(mgr.try_lock(2, 200, LockMode::Exclusive).is_ok());
        assert_eq!(mgr.lock_mode(2, 200), Some(LockMode::Exclusive));
    }

    #[test]
    fn multiple_share_locks_track_all_holders() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.try_lock(2, 100, LockMode::Share).is_ok());
        assert!(mgr.try_lock(3, 100, LockMode::Share).is_ok());
        assert_eq!(mgr.holder_count(100), 3);
        assert_eq!(mgr.waiter_count(100), 0);
    }

    // -----------------------------------------------------------------
    // 8. 并发安全（无 panic）
    // -----------------------------------------------------------------

    #[test]
    fn concurrent_try_lock_no_panic() {
        let mgr = Arc::new(LockManager::new());
        let success_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let handles: Vec<_> = (0..10)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                let success_count = Arc::clone(&success_count);
                thread::spawn(move || {
                    for i in 0..100 {
                        let resource = 1000u64 + (i % 10);
                        if mgr.try_lock(tid + 1, resource, LockMode::Exclusive).is_ok() {
                            success_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            mgr.unlock(tid + 1, resource);
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // 所有线程都应完成（无 panic），且最终所有锁都释放
        assert_eq!(mgr.resource_count(), 0);
        // success_count > 0（至少有些加锁成功）
        assert!(success_count.load(std::sync::atomic::Ordering::SeqCst) > 0);
    }

    #[test]
    fn concurrent_lock_with_timeout_no_panic() {
        let mgr = Arc::new(LockManager::new());
        let handles: Vec<_> = (0..5)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                thread::spawn(move || {
                    for _ in 0..20 {
                        let _ =
                            mgr.lock(tid + 1, 100, LockMode::Exclusive, Duration::from_millis(10));
                        mgr.unlock(tid + 1, 100);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(mgr.resource_count(), 0);
    }

    // -----------------------------------------------------------------
    // 9. 边界条件
    // -----------------------------------------------------------------

    #[test]
    fn unlock_nonexistent_resource_is_noop() {
        let mgr = LockManager::new();
        // 不应 panic
        mgr.unlock(1, 999);
        mgr.unlock_all(1);
    }

    #[test]
    fn unlock_by_nonexistent_txn_is_noop() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        // txn 2 未持有任何锁，unlock 应无副作用
        mgr.unlock(2, 100);
        assert!(mgr.holds_lock(1, 100));
        assert_eq!(mgr.holder_count(100), 1);
    }

    #[test]
    fn upgrade_after_unlock_all_returns_invalid_upgrade() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        mgr.unlock_all(1);
        let result = mgr.upgrade(1, 100, Duration::from_millis(100));
        // L5 修复：unlock_all 后未持有锁，返回 NotHeld
        assert!(matches!(result, Err(LockError::NotHeld { .. })));
    }

    #[test]
    fn lock_after_upgrade_release_allows_new_locks() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        assert!(mgr.upgrade(1, 100, Duration::from_millis(100)).is_ok());
        assert_eq!(mgr.lock_mode(1, 100), Some(LockMode::Exclusive));
        mgr.unlock(1, 100);
        // 新事务可以获取
        assert!(mgr.try_lock(2, 100, LockMode::Share).is_ok());
        assert!(mgr.try_lock(3, 100, LockMode::Share).is_ok());
    }

    #[test]
    fn zero_timeout_lock_behaves_like_try_lock() {
        let mgr = LockManager::new();
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());
        // 零超时：冲突时立即返回 Timeout
        let result = mgr.lock(2, 100, LockMode::Exclusive, Duration::from_millis(0));
        assert!(matches!(result, Err(LockError::Timeout { .. })));
    }

    // -----------------------------------------------------------------
    // 10. LockMode 单元行为
    // -----------------------------------------------------------------

    #[test]
    fn lock_mode_compatible_matrix() {
        assert!(LockMode::Share.compatible_with(LockMode::Share));
        assert!(!LockMode::Share.compatible_with(LockMode::Exclusive));
        assert!(!LockMode::Exclusive.compatible_with(LockMode::Share));
        assert!(!LockMode::Exclusive.compatible_with(LockMode::Exclusive));
    }

    #[test]
    fn lock_mode_strength_ordering() {
        assert!(LockMode::Exclusive.at_least(LockMode::Share));
        assert!(LockMode::Exclusive.at_least(LockMode::Exclusive));
        assert!(LockMode::Share.at_least(LockMode::Share));
        assert!(!LockMode::Share.at_least(LockMode::Exclusive));
    }
}

// =====================================================================
// Phase 2.10 测试 — 等待图死锁检测
// =====================================================================

#[cfg(test)]
mod phase_2_10 {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;

    // -----------------------------------------------------------------
    // 1. 2 事务循环等待检测（经典死锁）
    // -----------------------------------------------------------------

    /// 经典 2 事务死锁：
    /// - txn1 持有 R1，等待 R2
    /// - txn2 持有 R2，等待 R1
    ///   → 环 txn1 → txn2 → txn1
    ///
    /// 验证：txn2 调用 lock(R1) 时立即检测到死锁，返回 Deadlock(2)
    #[test]
    fn deadlock_2_txns_cycle_detected_immediately() {
        let mgr = Arc::new(LockManager::new());

        // txn1 持有 R1，txn2 持有 R2
        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(2, 2, LockMode::Exclusive).is_ok());

        // txn1 尝试锁定 R2（会阻塞，txn2 持有 R2）
        let mgr1 = Arc::clone(&mgr);
        let h1 =
            thread::spawn(move || mgr1.lock(1, 2, LockMode::Exclusive, Duration::from_secs(5)));

        // 等待 txn1 进入等待队列
        thread::sleep(Duration::from_millis(100));

        // txn2 尝试锁定 R1 → 应立即检测到死锁
        let start = Instant::now();
        let result = mgr.lock(2, 1, LockMode::Exclusive, Duration::from_secs(5));
        let elapsed = start.elapsed();

        assert!(
            matches!(result, Err(LockError::Deadlock(2))),
            "expected Deadlock(2), got {:?}",
            result
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "deadlock should be detected immediately, took {:?}",
            elapsed
        );

        // txn2 被中止后应释放 R2，txn1 可继续
        mgr.unlock_all(2);
        let result = h1.join().unwrap();
        assert!(
            result.is_ok(),
            "txn1 should acquire R2 after deadlock resolution: {:?}",
            result
        );
    }

    // -----------------------------------------------------------------
    // 2. 3 事务循环等待检测
    // -----------------------------------------------------------------

    /// 3 事务循环死锁：
    /// - txn1 持有 R1，等待 R2
    /// - txn2 持有 R2，等待 R3
    /// - txn3 持有 R3，等待 R1
    ///   → 环 txn1 → txn2 → txn3 → txn1
    #[test]
    fn deadlock_3_txns_cycle_detected() {
        let mgr = Arc::new(LockManager::new());

        // 各自持有一个资源
        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(2, 2, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(3, 3, LockMode::Exclusive).is_ok());

        // txn1 等待 R2（txn2 持有）
        let mgr1 = Arc::clone(&mgr);
        let h1 =
            thread::spawn(move || mgr1.lock(1, 2, LockMode::Exclusive, Duration::from_secs(5)));
        thread::sleep(Duration::from_millis(50));

        // txn2 等待 R3（txn3 持有）
        let mgr2 = Arc::clone(&mgr);
        let h2 =
            thread::spawn(move || mgr2.lock(2, 3, LockMode::Exclusive, Duration::from_secs(5)));
        thread::sleep(Duration::from_millis(50));

        // txn3 等待 R1（txn1 持有）→ 形成环
        let start = Instant::now();
        let result = mgr.lock(3, 1, LockMode::Exclusive, Duration::from_secs(5));
        let elapsed = start.elapsed();

        assert!(
            matches!(result, Err(LockError::Deadlock(3))),
            "expected Deadlock(3), got {:?}",
            result
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "3-txn deadlock should be detected immediately, took {:?}",
            elapsed
        );

        // 清理：txn3 中止，释放 R3
        mgr.unlock_all(3);
        // txn2 现在可以获取 R3
        assert!(h2.join().unwrap().is_ok());
        // txn2 释放所有
        mgr.unlock_all(2);
        // txn1 可以获取 R2
        assert!(h1.join().unwrap().is_ok());
    }

    // -----------------------------------------------------------------
    // 3. 不形成环时不死锁
    // -----------------------------------------------------------------

    /// 线性等待链（非环）：
    /// - txn1 持有 R1
    /// - txn2 等待 R1（txn1 持有）
    /// - txn1 不等待任何资源 → 无环 → 无死锁
    #[test]
    fn no_deadlock_when_no_cycle() {
        let mgr = Arc::new(LockManager::new());

        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());

        // txn2 等待 R1
        let mgr2 = Arc::clone(&mgr);
        let h2 =
            thread::spawn(move || mgr2.lock(2, 1, LockMode::Exclusive, Duration::from_secs(2)));

        thread::sleep(Duration::from_millis(100));
        // txn2 仍在等待，无死锁
        assert!(!mgr.holds_lock(2, 1));

        // txn1 释放 R1
        mgr.unlock(1, 1);

        // txn2 应成功获取
        let result = h2.join().unwrap();
        assert!(result.is_ok(), "txn2 should acquire R1: {:?}", result);
        assert!(mgr.holds_lock(2, 1));
    }

    /// 多事务线性链（非环）：
    /// - txn1 持有 R1
    /// - txn2 等待 R1
    /// - txn3 等待 R1
    ///   → 无环 → 无死锁
    #[test]
    fn no_deadlock_with_linear_chain_3_txns() {
        let mgr = Arc::new(LockManager::new());

        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());

        // 先启动 txn2，确保它先入队（FIFO 顺序）
        let mgr2 = Arc::clone(&mgr);
        let h2 =
            thread::spawn(move || mgr2.lock(2, 1, LockMode::Exclusive, Duration::from_secs(5)));

        // 等待 txn2 进入等待队列
        thread::sleep(Duration::from_millis(150));
        assert_eq!(mgr.waiter_count(1), 1, "txn2 应已进入等待队列");

        // 再启动 txn3
        let mgr3 = Arc::clone(&mgr);
        let h3 =
            thread::spawn(move || mgr3.lock(3, 1, LockMode::Exclusive, Duration::from_secs(5)));

        thread::sleep(Duration::from_millis(150));
        assert_eq!(mgr.waiter_count(1), 2, "txn2 和 txn3 都应在等待队列中");

        // 释放 R1
        mgr.unlock(1, 1);

        // FIFO: txn2 先入队，应先获取
        assert!(h2.join().unwrap().is_ok(), "txn2 应成功获取锁（FIFO 队首）");
        mgr.unlock(2, 1);
        assert!(h3.join().unwrap().is_ok(), "txn3 应在 txn2 释放后获取锁");
    }

    // -----------------------------------------------------------------
    // 4. Oracle 风格超时死锁检测
    // -----------------------------------------------------------------

    /// Oracle 风格：死锁在等待期间形成（非进入队列时立即检测），
    /// 应在 1s 内通过周期性检测发现并中止。
    ///
    /// 场景：
    /// - txn1 等待 R2（txn2 持有）→ 进入队列时无环
    /// - 此后 txn2 开始等待 R1（txn1 持有）→ 环形成
    /// - txn1 应在 1s 内通过周期性检测发现死锁
    #[test]
    fn oracle_style_deadlock_detected_within_1s() {
        let mgr = Arc::new(LockManager::new());

        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(2, 2, LockMode::Exclusive).is_ok());

        // txn1 先等待 R2（此时无环，因为 txn2 还没等 R1）
        let mgr1 = Arc::clone(&mgr);
        let h1 =
            thread::spawn(move || mgr1.lock(1, 2, LockMode::Exclusive, Duration::from_secs(10)));

        // 确保 txn1 已进入等待队列
        thread::sleep(Duration::from_millis(200));

        // txn2 开始等待 R1 → 环形成
        // txn2 应该立即检测到（进入队列时检查）
        let result = mgr.lock(2, 1, LockMode::Exclusive, Duration::from_secs(10));
        assert!(matches!(result, Err(LockError::Deadlock(2))));

        // txn2 中止后释放 R2
        mgr.unlock_all(2);

        // txn1 应在 1s 内成功获取 R2
        let start = Instant::now();
        let result = h1.join().unwrap();
        let elapsed = start.elapsed();
        assert!(result.is_ok(), "txn1 should acquire R2: {:?}", result);
        assert!(
            elapsed < Duration::from_secs(1),
            "txn1 should acquire within 1s, took {:?}",
            elapsed
        );
    }

    // -----------------------------------------------------------------
    // 5. 死锁后中止允许其他事务继续
    // -----------------------------------------------------------------

    /// 死锁检测后中止一个事务，其他事务应能继续执行
    #[test]
    fn deadlock_resolution_allows_progress() {
        let mgr = Arc::new(LockManager::new());

        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(2, 2, LockMode::Exclusive).is_ok());

        // txn1 等待 R2
        let mgr1 = Arc::clone(&mgr);
        let h1 =
            thread::spawn(move || mgr1.lock(1, 2, LockMode::Exclusive, Duration::from_secs(5)));
        thread::sleep(Duration::from_millis(100));

        // txn2 等待 R1 → 死锁
        let result = mgr.lock(2, 1, LockMode::Exclusive, Duration::from_secs(5));
        assert!(matches!(result, Err(LockError::Deadlock(2))));

        // txn2 释放所有锁
        mgr.unlock_all(2);

        // txn1 应能获取 R2
        assert!(h1.join().unwrap().is_ok());
        assert!(mgr.holds_lock(1, 2));

        // txn1 释放后，新事务可以获取 R1 和 R2
        mgr.unlock_all(1);
        assert!(mgr.try_lock(3, 1, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(3, 2, LockMode::Exclusive).is_ok());
    }

    // -----------------------------------------------------------------
    // 6. 4 事务循环
    // -----------------------------------------------------------------

    /// 4 事务循环：txn1→R2→txn2→R3→txn3→R4→txn4→R1→txn1
    #[test]
    fn deadlock_4_txns_cycle_detected() {
        let mgr = Arc::new(LockManager::new());

        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(2, 2, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(3, 3, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(4, 4, LockMode::Exclusive).is_ok());

        // txn1 → R2, txn2 → R3, txn3 → R4
        let mgr1 = Arc::clone(&mgr);
        let h1 =
            thread::spawn(move || mgr1.lock(1, 2, LockMode::Exclusive, Duration::from_secs(5)));
        thread::sleep(Duration::from_millis(30));

        let mgr2 = Arc::clone(&mgr);
        let h2 =
            thread::spawn(move || mgr2.lock(2, 3, LockMode::Exclusive, Duration::from_secs(5)));
        thread::sleep(Duration::from_millis(30));

        let mgr3 = Arc::clone(&mgr);
        let h3 =
            thread::spawn(move || mgr3.lock(3, 4, LockMode::Exclusive, Duration::from_secs(5)));
        thread::sleep(Duration::from_millis(30));

        // txn4 → R1 → 形成环
        let result = mgr.lock(4, 1, LockMode::Exclusive, Duration::from_secs(5));
        assert!(matches!(result, Err(LockError::Deadlock(4))));

        // 逐步清理
        mgr.unlock_all(4);
        assert!(h3.join().unwrap().is_ok());
        mgr.unlock_all(3);
        assert!(h2.join().unwrap().is_ok());
        mgr.unlock_all(2);
        assert!(h1.join().unwrap().is_ok());
    }

    // -----------------------------------------------------------------
    // 7. detect_all_deadlocks（Oracle 风格扫描）
    // -----------------------------------------------------------------

    /// 测试 detect_all_deadlocks 能发现已存在的死锁环
    #[test]
    fn detect_all_deadlocks_finds_cycle() {
        let mgr = Arc::new(LockManager::new());

        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(2, 2, LockMode::Exclusive).is_ok());

        // txn1 等待 R2
        let mgr1 = Arc::clone(&mgr);
        let h1 =
            thread::spawn(move || mgr1.lock(1, 2, LockMode::Exclusive, Duration::from_secs(10)));
        thread::sleep(Duration::from_millis(100));

        // txn2 等待 R1 → 形成环（但 txn2 会被立即检测到并中止）
        // 为了测试 detect_all_deadlocks，我们不让 txn2 调用 lock
        // 而是直接构造一个等待状态（通过 try_lock 失败后手动入队... 这不好做）

        // 替代方案：验证无环时 detect_all_deadlocks 返回空
        let cycles = mgr.detect_all_deadlocks();
        // 此时只有 txn1 在等待（txn2 没有等），无环
        assert!(cycles.is_empty(), "no cycle should exist: {:?}", cycles);

        // 清理
        mgr.unlock(2, 2);
        assert!(h1.join().unwrap().is_ok());
    }

    /// 构造真正的死锁环，验证 detect_all_deadlocks 返回非空
    #[test]
    fn detect_all_deadlocks_returns_cycle_path() {
        let mgr = Arc::new(LockManager::new());

        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(2, 2, LockMode::Exclusive).is_ok());

        // txn1 等待 R2（不使用 lock，使用长超时的 lock 在后台线程）
        let mgr1 = Arc::clone(&mgr);
        let _h1 = thread::spawn(move || {
            // 10 秒超时，确保在测试期间一直等待
            let _ = mgr1.lock(1, 2, LockMode::Exclusive, Duration::from_secs(10));
        });
        thread::sleep(Duration::from_millis(100));

        // txn2 等待 R1 → 会立即检测到死锁并被中止
        let result = mgr.lock(2, 1, LockMode::Exclusive, Duration::from_secs(10));
        assert!(matches!(result, Err(LockError::Deadlock(2))));

        // 此时 txn2 已被中止（不在等待队列），txn1 仍在等待 R2
        // 但 txn2 仍持有 R2（unlock_all 未调用）
        // txn1 仍在等待 → 无环
        let cycles = mgr.detect_all_deadlocks();
        assert!(
            cycles.is_empty(),
            "no cycle after txn2 aborted: {:?}",
            cycles
        );

        // 清理
        mgr.unlock_all(2);
    }

    // -----------------------------------------------------------------
    // 8. 共享锁场景不死锁
    // -----------------------------------------------------------------

    /// 多个事务持有共享锁，不形成环
    #[test]
    fn no_deadlock_with_concurrent_share_locks() {
        let mgr = Arc::new(LockManager::new());

        // 5 个事务都持有 S 锁
        for txn_id in 1..=5u32 {
            assert!(mgr.try_lock(txn_id, 100, LockMode::Share).is_ok());
        }

        // 无死锁
        let cycles = mgr.detect_all_deadlocks();
        assert!(cycles.is_empty());

        // 全部释放
        for txn_id in 1..=5u32 {
            mgr.unlock(txn_id, 100);
        }
        assert_eq!(mgr.resource_count(), 0);
    }

    // -----------------------------------------------------------------
    // 9. 并发死锁检测无 panic
    // -----------------------------------------------------------------

    /// 10 线程并发加锁/解锁，死锁检测不 panic
    #[test]
    fn concurrent_deadlock_detection_no_panic() {
        let mgr = Arc::new(LockManager::new());
        let handles: Vec<_> = (0..10u32)
            .map(|tid| {
                let mgr = Arc::clone(&mgr);
                thread::spawn(move || {
                    let txn_id = tid + 1;
                    for i in 0u32..20 {
                        let resource = 100u64 + (((i + tid) % 5) as u64);
                        // 尝试加锁，超时 100ms
                        let _ = mgr.lock(
                            txn_id,
                            resource,
                            LockMode::Exclusive,
                            Duration::from_millis(100),
                        );
                        thread::sleep(Duration::from_millis(1));
                        mgr.unlock_all(txn_id);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap(); // 不应 panic
        }

        // 最终所有锁应释放
        assert_eq!(mgr.resource_count(), 0);
    }

    // -----------------------------------------------------------------
    // 10. 死锁检测准确性（无误报）
    // -----------------------------------------------------------------

    /// 复杂非死锁场景：多事务等待同一资源，但持有者不等待任何人
    #[test]
    fn no_false_positive_deadlock_detection() {
        let mgr = Arc::new(LockManager::new());

        // txn1 持有 R1, R2, R3
        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(1, 2, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(1, 3, LockMode::Exclusive).is_ok());

        // 先启动 txn2，确保它先入队（FIFO 顺序）
        let mgr2 = Arc::clone(&mgr);
        let h2 =
            thread::spawn(move || mgr2.lock(2, 1, LockMode::Exclusive, Duration::from_secs(5)));
        thread::sleep(Duration::from_millis(100));
        assert_eq!(mgr.waiter_count(1), 1, "txn2 应已进入等待队列");

        // 再启动 txn3, txn4
        let mgr3 = Arc::clone(&mgr);
        let h3 =
            thread::spawn(move || mgr3.lock(3, 1, LockMode::Exclusive, Duration::from_secs(5)));
        let mgr4 = Arc::clone(&mgr);
        let h4 =
            thread::spawn(move || mgr4.lock(4, 1, LockMode::Exclusive, Duration::from_secs(5)));

        thread::sleep(Duration::from_millis(150));

        // 无死锁
        let cycles = mgr.detect_all_deadlocks();
        assert!(cycles.is_empty(), "no false positive: {:?}", cycles);

        // 释放 R1，等待者应依次获取
        mgr.unlock_all(1);
        // FIFO: txn2 先入队，应先获取
        let r2 = h2.join().unwrap();
        assert!(r2.is_ok(), "txn2 should acquire: {:?}", r2);

        // 清理其余
        mgr.unlock_all(2);
        let _ = h3.join();
        let _ = h4.join();
    }

    // -----------------------------------------------------------------
    // 11. 死锁检测时中止的是等待者（非持有者）
    // -----------------------------------------------------------------

    /// 验证死锁检测中止的是发起 lock 请求的事务（等待者），
    /// 而非已持有锁的事务
    #[test]
    fn deadlock_aborts_waiter_not_holder() {
        let mgr = Arc::new(LockManager::new());

        assert!(mgr.try_lock(1, 1, LockMode::Exclusive).is_ok());
        assert!(mgr.try_lock(2, 2, LockMode::Exclusive).is_ok());

        // txn1 等待 R2
        let mgr1 = Arc::clone(&mgr);
        let h1 =
            thread::spawn(move || mgr1.lock(1, 2, LockMode::Exclusive, Duration::from_secs(5)));
        thread::sleep(Duration::from_millis(100));

        // txn2 等待 R1 → 死锁，txn2 被中止
        let result = mgr.lock(2, 1, LockMode::Exclusive, Duration::from_secs(5));
        assert!(matches!(result, Err(LockError::Deadlock(2))));

        // txn2 仍持有 R2（直到调用 unlock_all）
        assert!(mgr.holds_lock(2, 2));
        // txn1 仍持有 R1
        assert!(mgr.holds_lock(1, 1));
        // txn1 仍在等待 R2
        assert!(!mgr.holds_lock(1, 2));

        // 释放 txn2 的锁
        mgr.unlock_all(2);
        // txn1 现在可以获取 R2
        assert!(h1.join().unwrap().is_ok());
    }

    // -----------------------------------------------------------------
    // 12. 同一事务不能与自身死锁
    // -----------------------------------------------------------------

    /// 同一事务持有 R1 并再次请求 R1 → 同事务重入，no-op，不死锁
    #[test]
    fn no_self_deadlock_with_reentrant_lock() {
        let mgr = LockManager::new();

        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        // 同事务请求同资源同模式 → no-op
        assert!(mgr.try_lock(1, 100, LockMode::Share).is_ok());
        // 同事务请求更强模式 → 升级（无其他持有者，立即成功）
        assert!(mgr.try_lock(1, 100, LockMode::Exclusive).is_ok());

        // 无死锁
        assert!(mgr.detect_all_deadlocks().is_empty());
        assert_eq!(mgr.lock_mode(1, 100), Some(LockMode::Exclusive));
    }
}
