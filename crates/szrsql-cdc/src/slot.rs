//! 系统级复制槽 — 持久化消费位点，支持崩溃后精确恢复
//!
//! # 设计要点
//!
//! 1. **系统级抽象**：相对 `OffsetStore`（CDC crate 级），ReplicationSlot 是数据库系统级概念，
//!    模拟 PG 的 `pg_replication_slots`，由 `SlotManager` 统一管理
//! 2. **持久化位点**：每个 slot 记录 `confirmed_flush_lsn`（已刷盘到目标端的 LSN）
//! 3. **WAL 保留**：`restart_lsn` 之前的 WAL 可被回收，之后必须保留（防止消费端崩溃后丢数据）
//! 4. **生命周期**：CreateSlot → Active → Inactive → Drop，支持 pause/resume
//! 5. **多消费端**：每个 slot 对应一个独立的消费端（可对应不同的目标端）
//! 6. **崩溃恢复**：进程重启后从持久化文件加载 slots，confirmed_flush_lsn 之前的事件不重投
//!
//! # 与 OffsetStore 的关系
//!
//! - **OffsetStore**：CDC crate 内部实现，consumer_group × partition 级别的位点持久化
//! - **ReplicationSlot**：系统级抽象，对应一个完整的复制链路（源端 → 目标端）
//!   - 内部可使用 OffsetStore 作为持久化机制
//!   - 额外提供 slot 生命周期管理（create/drop/list/pause/resume）
//!
//! # 持久化格式
//!
//! ```json
//! {
//!   "version": 1,
//!   "slots": [
//!     {
//!       "slot_name": "rep_pg_target1",
//!       "target_type": "postgres",
//!       "target_connection": "postgresql://...",
//!       "state": "active",
//!       "restart_lsn": 1000,
//!       "confirmed_flush_lsn": 950,
//!       "created_at": 1700000000000,
//!       "last_active_at": 1700000005000,
//!       "table_filter": ["users", "orders"]
//!     }
//!   ]
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

// =====================================================================
// SlotState — 复制槽状态
// =====================================================================

/// 复制槽状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotState {
    /// 已创建但未启动（可启动）
    Inactive,
    /// 活跃中（消费端连接中）
    Active,
    /// 已暂停（手动暂停，可恢复）
    Paused,
    /// 已删除（保留用于审计，不可恢复）
    Dropped,
}

impl SlotState {
    /// 是否可消费事件
    pub fn can_consume(self) -> bool {
        matches!(self, SlotState::Active)
    }

    /// 转字符串
    pub fn as_str(self) -> &'static str {
        match self {
            SlotState::Inactive => "inactive",
            SlotState::Active => "active",
            SlotState::Paused => "paused",
            SlotState::Dropped => "dropped",
        }
    }
}

impl std::fmt::Display for SlotState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
// ReplicationSlot — 单个复制槽
// =====================================================================

/// 复制槽 — 一个完整的源端→目标端复制链路的位点记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationSlot {
    /// 槽名（唯一标识）
    pub slot_name: String,
    /// 目标端类型（postgres / mysql / kafka / memory）
    pub target_type: String,
    /// 目标端连接串
    pub target_connection: String,
    /// 槽状态
    pub state: SlotState,
    /// 重启 LSN — 此 LSN 之前的 WAL 可被回收
    ///
    /// 通常等于 `confirmed_flush_lsn`，但消费端崩溃重启后可能小于
    pub restart_lsn: u64,
    /// 已刷盘 LSN — 已确认写入目标端的最后一个 LSN
    ///
    /// 此 LSN 之前的事件不会重投（崩溃恢复后从 `confirmed_flush_lsn + 1` 开始）
    pub confirmed_flush_lsn: u64,
    /// 创建时间（Unix 毫秒）
    pub created_at: u64,
    /// 最后活跃时间（Unix 毫秒）
    pub last_active_at: u64,
    /// 表过滤（None 表示复制所有表；Some 表示只复制列表中的表）
    #[serde(default)]
    pub table_filter: Option<HashSet<String>>,
    /// 已复制的统计信息
    #[serde(default)]
    pub stats: SlotStats,
}

/// 复制槽统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlotStats {
    /// 已处理的事件总数
    pub events_processed: u64,
    /// 已处理的事务数（Commit 数）
    pub transactions_processed: u64,
    /// 已处理的字节数
    pub bytes_processed: u64,
    /// 错误次数
    pub error_count: u64,
    /// 最后一次错误消息
    pub last_error: Option<String>,
}

impl ReplicationSlot {
    /// 创建新槽（初始状态 Inactive，restart_lsn = confirmed_flush_lsn = 0）
    pub fn new(
        slot_name: impl Into<String>,
        target_type: impl Into<String>,
        target_connection: impl Into<String>,
    ) -> Self {
        let now = current_millis();
        Self {
            slot_name: slot_name.into(),
            target_type: target_type.into(),
            target_connection: target_connection.into(),
            state: SlotState::Inactive,
            restart_lsn: 0,
            confirmed_flush_lsn: 0,
            created_at: now,
            last_active_at: now,
            table_filter: None,
            stats: SlotStats::default(),
        }
    }

    /// 设置表过滤
    pub fn with_table_filter(mut self, tables: Vec<String>) -> Self {
        self.table_filter = Some(tables.into_iter().collect());
        self
    }

    /// 是否接受该表（表过滤判断）
    pub fn accepts_table(&self, table_name: &str) -> bool {
        match &self.table_filter {
            None => true,
            Some(set) => set.contains(table_name),
        }
    }

    /// 推进 confirmed_flush_lsn
    ///
    /// # 参数
    /// - `lsn`：新的 flush LSN（必须 >= 当前 confirmed_flush_lsn）
    ///
    /// # 返回
    /// - `Ok(())`：推进成功
    /// - `Err(SlotError)`：LSN 倒退
    pub fn advance_flush_lsn(&mut self, lsn: u64) -> Result<(), SlotError> {
        if lsn < self.confirmed_flush_lsn {
            return Err(SlotError::LsnRegression {
                slot: self.slot_name.clone(),
                old: self.confirmed_flush_lsn,
                new: lsn,
            });
        }
        self.confirmed_flush_lsn = lsn;
        self.restart_lsn = lsn;
        self.last_active_at = current_millis();
        Ok(())
    }

    /// 记录已处理事件（统计）
    pub fn record_event(&mut self, bytes: usize) {
        self.stats.events_processed += 1;
        self.stats.bytes_processed += bytes as u64;
        self.last_active_at = current_millis();
    }

    /// 记录已处理事务
    pub fn record_transaction(&mut self) {
        self.stats.transactions_processed += 1;
    }

    /// 记录错误
    pub fn record_error(&mut self, msg: impl Into<String>) {
        self.stats.error_count += 1;
        self.stats.last_error = Some(msg.into());
    }

    /// 是否需要保留该 LSN 的 WAL（restart_lsn 之后必须保留）
    pub fn retains_wal_at(&self, lsn: u64) -> bool {
        lsn >= self.restart_lsn
    }

    /// 滞后量（confirmed_flush_lsn 与某 lsn 的差距）
    pub fn lag(&self, current_lsn: u64) -> u64 {
        current_lsn.saturating_sub(self.confirmed_flush_lsn)
    }
}

// =====================================================================
// SlotError — 复制槽错误
// =====================================================================

/// 复制槽错误
#[derive(Debug, thiserror::Error)]
pub enum SlotError {
    /// 槽已存在
    #[error("slot already exists: {0}")]
    AlreadyExists(String),

    /// 槽不存在
    #[error("slot not found: {0}")]
    NotFound(String),

    /// 槽状态非法（如对 Dropped 槽执行操作）
    #[error("invalid slot state: slot={slot} state={state}")]
    InvalidState { slot: String, state: String },

    /// LSN 倒退
    #[error("lsn regression: slot={slot} old={old} new={new}")]
    LsnRegression { slot: String, old: u64, new: u64 },

    /// 持久化 IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// 序列化错误
    #[error("serialize error: {0}")]
    Serialize(#[from] serde_json::Error),
}

// =====================================================================
// SlotManager — 复制槽管理器
// =====================================================================

/// 持久化文件格式
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlotFile {
    /// 文件格式版本
    version: u32,
    /// 所有槽列表
    slots: Vec<ReplicationSlot>,
}

/// 复制槽管理器 — 系统级管理所有复制槽
///
/// **线程安全**：内部用 `RwLock<HashMap<String, ReplicationSlot>>` 支持并发读、互斥写
///
/// **持久化**：所有变更立即 atomic write 到磁盘文件
///
/// **使用方式**：
///
/// ```ignore
/// use szrsql_cdc::slot::{SlotManager, ReplicationSlot};
///
/// let mgr = SlotManager::new("slots.json").unwrap();
///
/// // 创建槽
/// let slot = mgr.create_slot("rep_pg1", "postgres", "postgresql://localhost/db").unwrap();
///
/// // 启动（活跃）
/// mgr.activate_slot("rep_pg1").unwrap();
///
/// // 推进位点
/// mgr.advance_flush_lsn("rep_pg1", 1000).unwrap();
///
/// // 暂停
/// mgr.pause_slot("rep_pg1").unwrap();
///
/// // 列出所有槽
/// let slots = mgr.list_slots();
///
/// // 删除槽
/// mgr.drop_slot("rep_pg1").unwrap();
/// ```
pub struct SlotManager {
    /// 槽存储（slot_name → ReplicationSlot）
    slots: RwLock<HashMap<String, ReplicationSlot>>,
    /// 持久化路径
    path: PathBuf,
    /// 持久化锁（避免并发持久化）
    persist_lock: Mutex<()>,
}

impl SlotManager {
    /// 创建槽管理器，从指定路径加载已存在的 slots
    pub fn new(path: impl AsRef<Path>) -> Result<Self, SlotError> {
        let path = path.as_ref().to_path_buf();
        let slots = if path.exists() {
            Self::load_from_file(&path)?
        } else {
            HashMap::new()
        };
        Ok(Self {
            slots: RwLock::new(slots),
            path,
            persist_lock: Mutex::new(()),
        })
    }

    /// 创建内存中的槽管理器（不持久化，测试用）
    pub fn in_memory() -> Self {
        Self {
            slots: RwLock::new(HashMap::new()),
            path: PathBuf::new(),
            persist_lock: Mutex::new(()),
        }
    }

    /// 加载槽文件
    fn load_from_file(path: &Path) -> Result<HashMap<String, ReplicationSlot>, SlotError> {
        let content = std::fs::read_to_string(path)?;
        if content.trim().is_empty() {
            return Ok(HashMap::new());
        }
        let file: SlotFile = serde_json::from_str(&content)?;
        let mut map = HashMap::with_capacity(file.slots.len());
        for slot in file.slots {
            map.insert(slot.slot_name.clone(), slot);
        }
        Ok(map)
    }

    /// 持久化到文件（atomic write）
    fn persist(&self) -> Result<(), SlotError> {
        if self.path.as_os_str().is_empty() {
            return Ok(()); // 内存模式
        }
        let _guard = self.persist_lock.lock();
        let slots: Vec<ReplicationSlot> = {
            let slots = self.slots.read();
            slots.values().cloned().collect()
        };
        let file = SlotFile { version: 1, slots };
        let json = serde_json::to_string_pretty(&file)?;
        let tmp = self.path.with_extension("tmp");
        // P8-4 安全加固：原子写入 + fsync 确保崩溃不丢位点
        // 1. 写入临时文件
        // 2. fsync 临时文件（确保数据落盘）
        // 3. rename 到目标路径（原子操作）
        // 4. fsync 父目录（确保 rename 的目录条目落盘）
        use std::io::Write;
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?; // fsync 临时文件内容
        }
        std::fs::rename(&tmp, &self.path)?;
        // fsync 父目录以确保 rename 的目录条目持久化
        if let Some(parent) = self.path.parent() {
            if let Ok(dir) = std::fs::File::open(parent) {
                let _ = dir.sync_all(); // 某些平台（Windows）对目录 fsync 为 no-op
            }
        }
        Ok(())
    }

    /// 创建新槽
    ///
    /// # 参数
    /// - `slot_name`：槽名（唯一）
    /// - `target_type`：目标端类型
    /// - `target_connection`：目标端连接串
    pub fn create_slot(
        &self,
        slot_name: impl Into<String>,
        target_type: impl Into<String>,
        target_connection: impl Into<String>,
    ) -> Result<ReplicationSlot, SlotError> {
        let slot_name = slot_name.into();
        let mut slots = self.slots.write();
        if slots.contains_key(&slot_name) {
            return Err(SlotError::AlreadyExists(slot_name));
        }
        let slot = ReplicationSlot::new(slot_name.clone(), target_type, target_connection);
        slots.insert(slot_name, slot.clone());
        drop(slots);
        self.persist()?;
        Ok(slot)
    }

    /// 删除槽（标记为 Dropped，保留审计）
    pub fn drop_slot(&self, slot_name: &str) -> Result<(), SlotError> {
        let mut slots = self.slots.write();
        let slot = slots
            .get_mut(slot_name)
            .ok_or_else(|| SlotError::NotFound(slot_name.to_string()))?;
        slot.state = SlotState::Dropped;
        drop(slots);
        self.persist()
    }

    /// 物理删除槽（从存储中彻底移除）
    pub fn remove_slot(&self, slot_name: &str) -> Result<(), SlotError> {
        let mut slots = self.slots.write();
        if slots.remove(slot_name).is_none() {
            return Err(SlotError::NotFound(slot_name.to_string()));
        }
        drop(slots);
        self.persist()
    }

    /// 激活槽（Inactive/Paused → Active）
    pub fn activate_slot(&self, slot_name: &str) -> Result<(), SlotError> {
        let mut slots = self.slots.write();
        let slot = slots
            .get_mut(slot_name)
            .ok_or_else(|| SlotError::NotFound(slot_name.to_string()))?;
        match slot.state {
            SlotState::Inactive | SlotState::Paused => {
                slot.state = SlotState::Active;
                slot.last_active_at = current_millis();
                drop(slots);
                self.persist()
            }
            SlotState::Active => Ok(()),
            SlotState::Dropped => Err(SlotError::InvalidState {
                slot: slot_name.to_string(),
                state: "dropped".to_string(),
            }),
        }
    }

    /// 暂停槽（Active → Paused）
    pub fn pause_slot(&self, slot_name: &str) -> Result<(), SlotError> {
        let mut slots = self.slots.write();
        let slot = slots
            .get_mut(slot_name)
            .ok_or_else(|| SlotError::NotFound(slot_name.to_string()))?;
        match slot.state {
            SlotState::Active => {
                slot.state = SlotState::Paused;
                drop(slots);
                self.persist()
            }
            SlotState::Inactive | SlotState::Paused => Ok(()),
            SlotState::Dropped => Err(SlotError::InvalidState {
                slot: slot_name.to_string(),
                state: "dropped".to_string(),
            }),
        }
    }

    /// 推进槽的 confirmed_flush_lsn
    pub fn advance_flush_lsn(&self, slot_name: &str, lsn: u64) -> Result<(), SlotError> {
        let mut slots = self.slots.write();
        let slot = slots
            .get_mut(slot_name)
            .ok_or_else(|| SlotError::NotFound(slot_name.to_string()))?;
        if slot.state == SlotState::Dropped {
            return Err(SlotError::InvalidState {
                slot: slot_name.to_string(),
                state: "dropped".to_string(),
            });
        }
        slot.advance_flush_lsn(lsn)?;
        drop(slots);
        self.persist()
    }

    /// 记录事件处理（统计）
    pub fn record_event(&self, slot_name: &str, bytes: usize) -> Result<(), SlotError> {
        let mut slots = self.slots.write();
        let slot = slots
            .get_mut(slot_name)
            .ok_or_else(|| SlotError::NotFound(slot_name.to_string()))?;
        slot.record_event(bytes);
        drop(slots);
        // 不持久化统计信息（避免频繁 IO），由调用方定期 flush
        Ok(())
    }

    /// 记录事务处理
    pub fn record_transaction(&self, slot_name: &str) -> Result<(), SlotError> {
        let mut slots = self.slots.write();
        let slot = slots
            .get_mut(slot_name)
            .ok_or_else(|| SlotError::NotFound(slot_name.to_string()))?;
        slot.record_transaction();
        drop(slots);
        Ok(())
    }

    /// 记录错误
    pub fn record_error(&self, slot_name: &str, msg: impl Into<String>) -> Result<(), SlotError> {
        let mut slots = self.slots.write();
        let slot = slots
            .get_mut(slot_name)
            .ok_or_else(|| SlotError::NotFound(slot_name.to_string()))?;
        slot.record_error(msg);
        drop(slots);
        self.persist()
    }

    /// 强制持久化统计信息（定期调用）
    pub fn flush_stats(&self) -> Result<(), SlotError> {
        self.persist()
    }

    /// 获取槽信息（只读）
    pub fn get_slot(&self, slot_name: &str) -> Option<ReplicationSlot> {
        self.slots.read().get(slot_name).cloned()
    }

    /// 列出所有槽（只读副本）
    pub fn list_slots(&self) -> Vec<ReplicationSlot> {
        self.slots.read().values().cloned().collect()
    }

    /// 列出活跃槽
    pub fn list_active_slots(&self) -> Vec<ReplicationSlot> {
        self.slots
            .read()
            .values()
            .filter(|s| s.state == SlotState::Active)
            .cloned()
            .collect()
    }

    /// 槽数量
    pub fn slot_count(&self) -> usize {
        self.slots.read().len()
    }

    /// 获取最小 restart_lsn（WAL 回收时使用：所有槽的 restart_lsn 最小值之前的 WAL 可回收）
    pub fn min_restart_lsn(&self) -> Option<u64> {
        self.slots
            .read()
            .values()
            .filter(|s| s.state != SlotState::Dropped)
            .map(|s| s.restart_lsn)
            .min()
    }

    /// 检查指定 LSN 的 WAL 是否需要保留（任何活跃槽需要则保留）
    pub fn retains_wal_at(&self, lsn: u64) -> bool {
        self.slots
            .read()
            .values()
            .any(|s| s.state != SlotState::Dropped && s.retains_wal_at(lsn))
    }

    /// 重置槽（清除位点，重新从 0 开始）
    pub fn reset_slot(&self, slot_name: &str) -> Result<(), SlotError> {
        let mut slots = self.slots.write();
        let slot = slots
            .get_mut(slot_name)
            .ok_or_else(|| SlotError::NotFound(slot_name.to_string()))?;
        if slot.state == SlotState::Dropped {
            return Err(SlotError::InvalidState {
                slot: slot_name.to_string(),
                state: "dropped".to_string(),
            });
        }
        slot.restart_lsn = 0;
        slot.confirmed_flush_lsn = 0;
        slot.stats = SlotStats::default();
        drop(slots);
        self.persist()
    }
}

// =====================================================================
// 辅助函数
// =====================================================================

/// 获取当前 Unix 毫秒时间戳
fn current_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut tmp = std::env::temp_dir();
        tmp.push(format!(
            "szrsql_slot_test_{}_{}_{:x}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("unknown").len(),
            id
        ));
        tmp
    }

    #[test]
    fn slot_state_can_consume() {
        assert!(SlotState::Active.can_consume());
        assert!(!SlotState::Inactive.can_consume());
        assert!(!SlotState::Paused.can_consume());
        assert!(!SlotState::Dropped.can_consume());
    }

    #[test]
    fn slot_new_initial_state() {
        let slot = ReplicationSlot::new("test", "postgres", "postgresql://localhost/db");
        assert_eq!(slot.state, SlotState::Inactive);
        assert_eq!(slot.restart_lsn, 0);
        assert_eq!(slot.confirmed_flush_lsn, 0);
        assert_eq!(slot.stats.events_processed, 0);
    }

    #[test]
    fn slot_with_table_filter() {
        let slot = ReplicationSlot::new("test", "postgres", "postgresql://localhost/db")
            .with_table_filter(vec!["users".to_string(), "orders".to_string()]);
        assert!(slot.accepts_table("users"));
        assert!(slot.accepts_table("orders"));
        assert!(!slot.accepts_table("products"));
    }

    #[test]
    fn slot_no_filter_accepts_all() {
        let slot = ReplicationSlot::new("test", "postgres", "postgresql://localhost/db");
        assert!(slot.accepts_table("anything"));
    }

    #[test]
    fn slot_advance_flush_lsn() {
        let mut slot = ReplicationSlot::new("test", "postgres", "postgresql://localhost/db");
        slot.advance_flush_lsn(100).unwrap();
        assert_eq!(slot.confirmed_flush_lsn, 100);
        assert_eq!(slot.restart_lsn, 100);

        slot.advance_flush_lsn(200).unwrap();
        assert_eq!(slot.confirmed_flush_lsn, 200);

        // LSN 倒退应失败
        let result = slot.advance_flush_lsn(150);
        assert!(result.is_err());
    }

    #[test]
    fn slot_record_event_and_transaction() {
        let mut slot = ReplicationSlot::new("test", "postgres", "postgresql://localhost/db");
        slot.record_event(100);
        slot.record_event(200);
        slot.record_transaction();

        assert_eq!(slot.stats.events_processed, 2);
        assert_eq!(slot.stats.bytes_processed, 300);
        assert_eq!(slot.stats.transactions_processed, 1);
    }

    #[test]
    fn slot_retains_wal_at() {
        let mut slot = ReplicationSlot::new("test", "postgres", "postgresql://localhost/db");
        slot.advance_flush_lsn(1000).unwrap();

        // restart_lsn = 1000，所以 999 可回收，1000/1001 必须保留
        assert!(!slot.retains_wal_at(999));
        assert!(slot.retains_wal_at(1000));
        assert!(slot.retains_wal_at(1001));
    }

    #[test]
    fn slot_lag() {
        let mut slot = ReplicationSlot::new("test", "postgres", "postgresql://localhost/db");
        slot.advance_flush_lsn(1000).unwrap();
        assert_eq!(slot.lag(1500), 500);
        assert_eq!(slot.lag(500), 0); // saturating
    }

    #[test]
    fn slot_manager_in_memory_create() {
        let mgr = SlotManager::in_memory();
        let slot = mgr
            .create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();
        assert_eq!(slot.slot_name, "rep1");
        assert_eq!(slot.state, SlotState::Inactive);
        assert_eq!(mgr.slot_count(), 1);
    }

    #[test]
    fn slot_manager_duplicate_create_fails() {
        let mgr = SlotManager::in_memory();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();
        let result = mgr.create_slot("rep1", "postgres", "postgresql://localhost/db2");
        assert!(result.is_err());
    }

    #[test]
    fn slot_manager_activate_pause() {
        let mgr = SlotManager::in_memory();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();

        mgr.activate_slot("rep1").unwrap();
        assert_eq!(mgr.get_slot("rep1").unwrap().state, SlotState::Active);

        mgr.pause_slot("rep1").unwrap();
        assert_eq!(mgr.get_slot("rep1").unwrap().state, SlotState::Paused);

        // 再次激活
        mgr.activate_slot("rep1").unwrap();
        assert_eq!(mgr.get_slot("rep1").unwrap().state, SlotState::Active);
    }

    #[test]
    fn slot_manager_drop_removed() {
        let mgr = SlotManager::in_memory();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();

        mgr.drop_slot("rep1").unwrap();
        assert_eq!(mgr.get_slot("rep1").unwrap().state, SlotState::Dropped);

        // Dropped 槽不能激活
        let result = mgr.activate_slot("rep1");
        assert!(result.is_err());

        // 物理删除
        mgr.remove_slot("rep1").unwrap();
        assert!(mgr.get_slot("rep1").is_none());
    }

    #[test]
    fn slot_manager_advance_lsn() {
        let mgr = SlotManager::in_memory();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();

        mgr.advance_flush_lsn("rep1", 500).unwrap();
        assert_eq!(mgr.get_slot("rep1").unwrap().confirmed_flush_lsn, 500);

        // LSN 倒退失败
        let result = mgr.advance_flush_lsn("rep1", 400);
        assert!(result.is_err());
    }

    #[test]
    fn slot_manager_record_stats() {
        let mgr = SlotManager::in_memory();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();

        mgr.record_event("rep1", 100).unwrap();
        mgr.record_event("rep1", 200).unwrap();
        mgr.record_transaction("rep1").unwrap();

        let slot = mgr.get_slot("rep1").unwrap();
        assert_eq!(slot.stats.events_processed, 2);
        assert_eq!(slot.stats.bytes_processed, 300);
        assert_eq!(slot.stats.transactions_processed, 1);
    }

    #[test]
    fn slot_manager_list_active() {
        let mgr = SlotManager::in_memory();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();
        mgr.create_slot("rep2", "mysql", "mysql://localhost/db")
            .unwrap();
        mgr.create_slot("rep3", "kafka", "kafka://localhost:9092")
            .unwrap();

        mgr.activate_slot("rep1").unwrap();
        mgr.activate_slot("rep3").unwrap();

        let active = mgr.list_active_slots();
        assert_eq!(active.len(), 2);
        let names: Vec<_> = active.iter().map(|s| s.slot_name.as_str()).collect();
        assert!(names.contains(&"rep1"));
        assert!(names.contains(&"rep3"));
    }

    #[test]
    fn slot_manager_min_restart_lsn() {
        let mgr = SlotManager::in_memory();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();
        mgr.create_slot("rep2", "mysql", "mysql://localhost/db")
            .unwrap();

        mgr.advance_flush_lsn("rep1", 500).unwrap();
        mgr.advance_flush_lsn("rep2", 300).unwrap();

        assert_eq!(mgr.min_restart_lsn(), Some(300));
    }

    #[test]
    fn slot_manager_retains_wal_at() {
        let mgr = SlotManager::in_memory();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();
        mgr.advance_flush_lsn("rep1", 500).unwrap();

        // rep1 restart_lsn = 500，所以 499 可回收
        assert!(!mgr.retains_wal_at(499));
        assert!(mgr.retains_wal_at(500));
        assert!(mgr.retains_wal_at(501));
    }

    #[test]
    fn slot_manager_reset_slot() {
        let mgr = SlotManager::in_memory();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();
        mgr.advance_flush_lsn("rep1", 1000).unwrap();
        mgr.record_event("rep1", 100).unwrap();

        mgr.reset_slot("rep1").unwrap();
        let slot = mgr.get_slot("rep1").unwrap();
        assert_eq!(slot.confirmed_flush_lsn, 0);
        assert_eq!(slot.restart_lsn, 0);
        assert_eq!(slot.stats.events_processed, 0);
    }

    #[test]
    fn slot_manager_persist_and_reload() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);

        {
            let mgr = SlotManager::new(&path).unwrap();
            mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
                .unwrap();
            mgr.activate_slot("rep1").unwrap();
            mgr.advance_flush_lsn("rep1", 1000).unwrap();

            // 文件应该存在
            assert!(path.exists());
        }

        // 重新加载
        {
            let mgr = SlotManager::new(&path).unwrap();
            let slot = mgr.get_slot("rep1").unwrap();
            assert_eq!(slot.slot_name, "rep1");
            assert_eq!(slot.state, SlotState::Active);
            assert_eq!(slot.confirmed_flush_lsn, 1000);
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn slot_manager_persist_atomic() {
        // 验证持久化是原子写入（无 .tmp 残留）
        let path = temp_path();
        let _ = std::fs::remove_file(&path);

        let mgr = SlotManager::new(&path).unwrap();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();

        assert!(path.exists());
        let tmp_path = path.with_extension("tmp");
        assert!(!tmp_path.exists());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn slot_manager_not_found_errors() {
        let mgr = SlotManager::in_memory();

        assert!(mgr.get_slot("nonexistent").is_none());
        assert!(mgr.activate_slot("nonexistent").is_err());
        assert!(mgr.pause_slot("nonexistent").is_err());
        assert!(mgr.drop_slot("nonexistent").is_err());
        assert!(mgr.advance_flush_lsn("nonexistent", 100).is_err());
        assert!(mgr.record_event("nonexistent", 100).is_err());
    }

    #[test]
    fn slot_manager_record_error() {
        let mgr = SlotManager::in_memory();
        mgr.create_slot("rep1", "postgres", "postgresql://localhost/db")
            .unwrap();

        mgr.record_error("rep1", "connection timeout").unwrap();
        let slot = mgr.get_slot("rep1").unwrap();
        assert_eq!(slot.stats.error_count, 1);
        assert_eq!(
            slot.stats.last_error,
            Some("connection timeout".to_string())
        );
    }

    #[test]
    fn slot_manager_load_empty_file() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);

        // 写入空文件
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"").unwrap();

        let mgr = SlotManager::new(&path).unwrap();
        assert_eq!(mgr.slot_count(), 0);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn slot_manager_load_corrupted_file() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);

        std::fs::write(&path, b"not json").unwrap();

        let result = SlotManager::new(&path);
        assert!(result.is_err());
    }

    #[test]
    fn slot_manager_table_filter_persistence() {
        let path = temp_path();
        let _ = std::fs::remove_file(&path);

        {
            let mgr = SlotManager::new(&path).unwrap();
            let slot = ReplicationSlot::new("rep1", "postgres", "postgresql://localhost/db")
                .with_table_filter(vec!["users".to_string(), "orders".to_string()]);
            // 直接插入到内部存储
            {
                let mut slots = mgr.slots.write();
                slots.insert("rep1".to_string(), slot);
            }
            mgr.persist().unwrap();
        }

        // 重新加载
        {
            let mgr = SlotManager::new(&path).unwrap();
            let slot = mgr.get_slot("rep1").unwrap();
            assert!(slot.accepts_table("users"));
            assert!(slot.accepts_table("orders"));
            assert!(!slot.accepts_table("products"));
        }

        std::fs::remove_file(&path).ok();
    }
}
