//! SzRSQL 变更数据捕获（CDC）引擎 — 对应 `SzRSQL实施进度.md` Phase 2.5。
//!
//! Phase 2.5.1: WalObserver trait + CDCEngine
//! Phase 2.5.2: WAL 钩子事件分发（Fuzz：10 线程并发写入，随机注册/注销 Observer）
//! Phase 2.5.3: 变更事件格式定义（ChangeEvent 序列化/反序列化）
//!
//! # 设计要点
//!
//! 1. **复用 Phase 2.4 的 WalObserver + WalObserverManager + WalHookWriter**：
//!    - `szrsql-tx::wal::WalObserver` 是底层 WAL 钩子接口（on_commit/on_rollback）
//!    - `WalObserverManager` 支持多 observer 并发注册/注销/通知
//!    - `WalHookWriter` 包装 WalWriter，自动按 tx_id 缓冲并在 Commit/Abort 时触发钩子
//!
//! 2. **CDC 层抽象**：CdcEngine 实现 `WalObserver`，将底层 WalRecord 转换为高层 ChangeEvent
//!    - ChangeEvent 包含 Insert/Update/Delete/Commit/Abort 五种类型
//!    - CdcEngine 内部维护 `CdcObserverManager`，分发 ChangeEvent 给所有 CDC observers
//!    - CdcObserver trait 提供 `on_event(&self, event: ChangeEvent)` 接口
//!
//! 3. **ChangeEvent 格式**（Phase 2.5.3）：
//!    - 使用 serde 序列化/反序列化（JSON + bincode 双向兼容）
//!    - Insert/Delete：单行数据
//!    - Update：old_row + new_row（前镜像 + 后镜像）
//!    - Commit/Abort：仅 tx_id 和 lsn
//!
//! 4. **并发性**：
//!    - CdcEngine 内部用 `RwLock<Vec<Arc<dyn CdcObserver>>>` 管理观察者
//!    - 分发时同步调用所有 observer，单个 observer panic 不影响其他 observer
//!    - at-least-once 语义：on_commit 同步触发，observer 内部需自行去重
//!
//! 5. **背压预留**：CdcEngine 提供 `pending_event_count()` 接口供后续 Phase 2.5.8 背压处理使用

#![allow(dead_code)]

pub mod backpressure;
pub mod cloud;
pub mod cluster;
pub mod comparison;
pub mod decoder;
pub mod failover;
pub mod migration;
pub mod schema;
pub mod service;
pub mod slot;
pub mod snapshot;
pub mod source;
pub mod target;
pub mod task;

use std::sync::{Arc, Mutex, RwLock};
use szrsql_tx::wal::{WalObserver, WalOpType, WalRecord};

// =====================================================================
// CdcEventOp — CDC 事件操作类型
// =====================================================================

/// CDC 事件操作类型 — 对应 WalOpType 的高层抽象
///
/// 过滤掉 FullPageImage / Checkpoint 等内部记录，只暴露用户可见的 DML 操作
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CdcEventOp {
    /// 插入行
    Insert,
    /// 更新行
    Update,
    /// 删除行
    Delete,
    /// 事务提交
    Commit,
    /// 事务回滚
    Abort,
}

impl CdcEventOp {
    /// 从 WalOpType 构造 CdcEventOp，过滤掉内部记录
    ///
    /// 返回 `None` 表示该 WalOpType 不产生 CDC 事件（如 FullPageImage、Checkpoint）
    pub fn from_wal_op(op: WalOpType) -> Option<Self> {
        match op {
            WalOpType::Insert => Some(CdcEventOp::Insert),
            WalOpType::Update => Some(CdcEventOp::Update),
            WalOpType::Delete => Some(CdcEventOp::Delete),
            WalOpType::Commit => Some(CdcEventOp::Commit),
            WalOpType::Abort => Some(CdcEventOp::Abort),
            WalOpType::FullPageImage | WalOpType::Checkpoint | WalOpType::TableData => None,
        }
    }

    /// 转为字符串（用于 JSON 字段）
    pub fn as_str(self) -> &'static str {
        match self {
            CdcEventOp::Insert => "insert",
            CdcEventOp::Update => "update",
            CdcEventOp::Delete => "delete",
            CdcEventOp::Commit => "commit",
            CdcEventOp::Abort => "abort",
        }
    }
}

// =====================================================================
// ChangeEvent — CDC 变更事件（Phase 2.5.3）
// =====================================================================

/// CDC 变更事件 — 对应一次行级变更或事务状态变更
///
/// **设计**：
/// - `tx_id`：所属事务 ID
/// - `lsn`：WAL 日志序列号（单调递增）
/// - `op`：操作类型（Insert/Update/Delete/Commit/Abort）
/// - `table_id`：目标表 ID（Commit/Abort 时为 None）
/// - `old_row`：前镜像（仅 Update/Delete 有，Insert 为 None）
/// - `new_row`：后镜像（仅 Insert/Update 有，Delete 为 None）
/// - `timestamp`：事件生成时间戳（Unix 毫秒，由 CdcEngine 填充）
/// - `schema_version`：表 schema 版本（Phase 2.5.10，Commit/Abort 时为 None，
///   DML 事件由 `SchemaAwareCdcEngine` 填充为 `Some(version)`，普通 `CdcEngine` 为 None）
///
/// **序列化**（Phase 2.5.3）：
/// - JSON：使用 serde_json，字段名为 snake_case，None 字段输出为 null
/// - bincode：紧凑二进制，用于内部传输
/// - 双向一致：encode → decode 后字段全部保留
///
/// **注**：不使用 `skip_serializing_if = "Option::is_none"`，因为 bincode 1.x 按字段顺序
/// 反序列化，跳过字段会导致错位。None 在 JSON 中输出为 null，在 bincode 中输出为 0 tag + 无数据。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeEvent {
    /// 所属事务 ID
    pub tx_id: u32,
    /// WAL 日志序列号
    pub lsn: u64,
    /// 操作类型
    pub op: CdcEventOp,
    /// 目标表 ID（Commit/Abort 时为 None）
    pub table_id: Option<u32>,
    /// 前镜像（Update/Delete 时为 Some，Insert 时为 None）
    pub old_row: Option<Vec<u8>>,
    /// 后镜像（Insert/Update 时为 Some，Delete 时为 None）
    pub new_row: Option<Vec<u8>>,
    /// 事件生成时间戳（Unix 毫秒）
    pub timestamp: u64,
    /// 表 schema 版本（Phase 2.5.10）
    ///
    /// - `None`：未关联 schema（Commit/Abort 事件，或由普通 `CdcEngine` 产生的事件）
    /// - `Some(v)`：事件产生时该表的 schema 版本（由 `SchemaAwareCdcEngine` 填充）
    ///
    /// 消费者通过比较 `schema_version` 判断是否需要重新获取 schema 解码行数据。
    /// **向后兼容**：旧 JSON/bincode 不含此字段时反序列化为 None。
    #[serde(default)]
    pub schema_version: Option<u64>,
}

impl ChangeEvent {
    /// 创建 Insert 事件
    pub fn insert(tx_id: u32, lsn: u64, table_id: u32, new_row: Vec<u8>, timestamp: u64) -> Self {
        Self {
            tx_id,
            lsn,
            op: CdcEventOp::Insert,
            table_id: Some(table_id),
            old_row: None,
            new_row: Some(new_row),
            timestamp,
            schema_version: None,
        }
    }

    /// 创建 Update 事件
    pub fn update(
        tx_id: u32,
        lsn: u64,
        table_id: u32,
        old_row: Vec<u8>,
        new_row: Vec<u8>,
        timestamp: u64,
    ) -> Self {
        Self {
            tx_id,
            lsn,
            op: CdcEventOp::Update,
            table_id: Some(table_id),
            old_row: Some(old_row),
            new_row: Some(new_row),
            timestamp,
            schema_version: None,
        }
    }

    /// 创建 Delete 事件
    pub fn delete(tx_id: u32, lsn: u64, table_id: u32, old_row: Vec<u8>, timestamp: u64) -> Self {
        Self {
            tx_id,
            lsn,
            op: CdcEventOp::Delete,
            table_id: Some(table_id),
            old_row: Some(old_row),
            new_row: None,
            timestamp,
            schema_version: None,
        }
    }

    /// 创建 Commit 事件
    pub fn commit(tx_id: u32, lsn: u64, timestamp: u64) -> Self {
        Self {
            tx_id,
            lsn,
            op: CdcEventOp::Commit,
            table_id: None,
            old_row: None,
            new_row: None,
            timestamp,
            schema_version: None,
        }
    }

    /// 创建 Abort 事件
    pub fn abort(tx_id: u32, lsn: u64, timestamp: u64) -> Self {
        Self {
            tx_id,
            lsn,
            op: CdcEventOp::Abort,
            table_id: None,
            old_row: None,
            new_row: None,
            timestamp,
            schema_version: None,
        }
    }

    /// 设置 schema 版本（builder 模式，Phase 2.5.10）
    ///
    /// 用于 `SchemaAwareCdcEngine` 在构造事件后附加 schema 版本。
    /// 返回 `Self` 以支持链式调用。
    pub fn with_schema_version(mut self, version: u64) -> Self {
        self.schema_version = Some(version);
        self
    }

    /// 获取 schema 版本
    pub fn schema_version(&self) -> Option<u64> {
        self.schema_version
    }

    /// 从 WalRecord 构造 ChangeEvent
    ///
    /// **转换规则**：
    /// - Insert/Update/Delete：使用 record.data 作为 new_row（简化模型，old_row 暂为 None，
    ///   实际生产中应从 undo log 提取前镜像）
    /// - Commit/Abort：构造对应事件，table_id/old_row/new_row 全为 None
    /// - FullPageImage/Checkpoint：返回 None（不产生 CDC 事件）
    ///
    /// **参数**：`timestamp` 由调用方传入（便于测试时固定时间戳）
    pub fn from_wal_record(record: &WalRecord, timestamp: u64) -> Option<Self> {
        let op = CdcEventOp::from_wal_op(record.op_type)?;
        match op {
            CdcEventOp::Insert => Some(Self::insert(
                record.tx_id,
                record.lsn,
                record.page_id,
                record.data.clone(),
                timestamp,
            )),
            CdcEventOp::Update => Some(Self::update(
                record.tx_id,
                record.lsn,
                record.page_id,
                Vec::new(), // 简化模型：old_row 从 undo log 提取，此处留空
                record.data.clone(),
                timestamp,
            )),
            CdcEventOp::Delete => Some(Self::delete(
                record.tx_id,
                record.lsn,
                record.page_id,
                record.data.clone(),
                timestamp,
            )),
            CdcEventOp::Commit => Some(Self::commit(record.tx_id, record.lsn, timestamp)),
            CdcEventOp::Abort => Some(Self::abort(record.tx_id, record.lsn, timestamp)),
        }
    }

    /// 序列化为 JSON 字符串（Phase 2.5.3）
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// 从 JSON 字符串反序列化（Phase 2.5.3）
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// 序列化为 bincode 二进制（紧凑格式，Phase 2.5.3）
    pub fn to_bincode(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// 从 bincode 二进制反序列化（Phase 2.5.3）
    pub fn from_bincode(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }
}

// =====================================================================
// CdcObserver — CDC 层观察者 trait
// =====================================================================

/// CDC 层观察者 trait — 接收 ChangeEvent
///
/// **与 WalObserver 的区别**：
/// - `WalObserver` 接收原始 WalRecord（页级，含 Commit/Abort 等所有记录）
/// - `CdcObserver` 接收高层 ChangeEvent（行级，仅 DML + Commit/Abort，过滤掉 FullPageImage 等）
///
/// **语义**：at-least-once — observer 内部需基于 lsn 去重
///
/// **线程安全**：实现者必须是 `Send + Sync`，回调可能在 CdcEngine 锁内同步触发
pub trait CdcObserver: Send + Sync {
    /// 接收一个 ChangeEvent
    fn on_event(&self, event: ChangeEvent);
}

// =====================================================================
// CdcObserverManager — CDC 观察者管理器
// =====================================================================

/// CDC 观察者管理器 — 管理多个 CdcObserver，提供 register/unregister/notify
///
/// **设计**（与 WalObserverManager 同风格）：
/// 1. **多观察者**：支持注册多个 observer，所有 observer 独立接收事件
/// 2. **线程安全**：内部 `RwLock<Vec<Arc<dyn CdcObserver>>>` 支持并发读写
/// 3. **弱引用去重**：unregister 通过 Arc 数据指针地址比较
/// 4. **同步触发**：notify 同步调用所有 observer（at-least-once 语义）
/// 5. **panic 隔离**：单个 observer panic 不影响其他 observer（catch_unwind）
pub struct CdcObserverManager {
    observers: RwLock<Vec<Arc<dyn CdcObserver>>>,
    /// 已分发的事件总数（统计用）
    total_dispatched: std::sync::atomic::AtomicU64,
}

impl Default for CdcObserverManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CdcObserverManager {
    /// 创建空的观察者管理器
    pub fn new() -> Self {
        Self {
            observers: RwLock::new(Vec::new()),
            total_dispatched: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// 注册观察者
    ///
    /// 返回 `true` 表示注册成功；若已注册相同指针的 observer，返回 `false`
    pub fn register(&self, observer: Arc<dyn CdcObserver>) -> bool {
        let mut observers = self.observers.write().unwrap();
        let target_addr = arc_data_addr(&observer);
        if observers.iter().any(|o| arc_data_addr(o) == target_addr) {
            return false;
        }
        observers.push(observer);
        true
    }

    /// 注销观察者
    ///
    /// 返回 `true` 表示注销成功；若未找到，返回 `false`
    pub fn unregister<O: CdcObserver + 'static>(&self, observer: &Arc<O>) -> bool {
        let mut observers = self.observers.write().unwrap();
        let target_addr = arc_data_addr(observer);
        let original_len = observers.len();
        observers.retain(|o| arc_data_addr(o) != target_addr);
        observers.len() < original_len
    }

    /// 通知所有观察者：分发一个 ChangeEvent
    ///
    /// 同步调用每个 observer 的 `on_event`。单个 observer 的 panic 会被
    /// `catch_unwind` 捕获，不影响其他 observer 的通知。
    pub fn notify(&self, event: ChangeEvent) {
        let observers = self.observers.read().unwrap();
        let count = observers.len();
        for observer in observers.iter() {
            // 每个 observer 独立 clone 一份 event
            let event_clone = event.clone();
            // catch_unwind 隔离 panic
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                observer.on_event(event_clone);
            }));
        }
        self.total_dispatched
            .fetch_add(count as u64, std::sync::atomic::Ordering::SeqCst);
    }

    /// 获取已注册的观察者数量
    pub fn observer_count(&self) -> usize {
        self.observers.read().unwrap().len()
    }

    /// 获取已分发的（observer × event）总次数
    pub fn total_dispatched(&self) -> u64 {
        self.total_dispatched
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// 提取 `Arc<T>` 内部数据的地址（与 WalObserverManager 同实现）
fn arc_data_addr<T: ?Sized>(arc: &Arc<T>) -> usize {
    Arc::as_ptr(arc) as *const () as usize
}

// =====================================================================
// CdcEngine — CDC 引擎（实现 WalObserver）
// =====================================================================

/// CDC 引擎 — 实现 `WalObserver`，将 WalRecord 转换为 ChangeEvent 分发给 CDC observers
///
/// **设计**：
/// 1. **实现 WalObserver**：可作为 `WalObserverManager::register` 的参数
/// 2. **事件转换**：on_commit 时将 WalRecord 列表转为 ChangeEvent 列表，过滤掉无意义记录
/// 3. **分发**：通过内部 `CdcObserverManager` 分发给所有 CDC observers
/// 4. **时间戳**：使用 `SystemTime::now()` 填充 ChangeEvent.timestamp（可注入便于测试）
/// 5. **统计**：提供 `pending_event_count()` / `total_processed()` 供背压监控
///
/// **使用示例**：
/// ```ignore
/// use szrsql_cdc::{CdcEngine, CdcObserverManager, CdcObserver, ChangeEvent};
/// use szrsql_tx::wal::{WalObserverManager, WalHookWriter, WalWriter};
/// use std::sync::Arc;
///
/// let cdc_mgr = Arc::new(CdcObserverManager::new());
/// // 注册自定义 observer
/// cdc_mgr.register(Arc::new(MyObserver));
///
/// let engine = Arc::new(CdcEngine::new(cdc_mgr.clone()));
///
/// // 将 engine 注册为 WalObserver
/// let wal_mgr = Arc::new(WalObserverManager::new());
/// wal_mgr.register(engine.clone());
///
/// // WalHookWriter 触发 on_commit → CdcEngine 转换 → CdcObserver 分发
/// ```
pub struct CdcEngine {
    /// CDC 观察者管理器（共享所有权）
    observer_manager: Arc<CdcObserverManager>,
    /// Schema 变更观察者管理器（P4-2，DDL 事件分发）
    schema_change_manager: Arc<schema::SchemaChangeObserverManager>,
    /// 时间戳注入函数（便于测试固定时间戳）
    timestamp_fn: Box<dyn Fn() -> u64 + Send + Sync>,
    /// 已处理的 WalRecord 总数
    total_processed: std::sync::atomic::AtomicU64,
    /// 当前缓冲中的事件数（pending，背压监控用）
    pending_events: std::sync::atomic::AtomicU64,
    /// P7-1：全局 LSN 计数器（跨 Executor 共享，保证 LSN 单调递增）
    lsn_counter: std::sync::atomic::AtomicU64,
    /// Batch 3：事务级 CDC 事件缓冲（txn_id → 待分发事件列表）
    ///
    /// 显式事务（BEGIN...COMMIT）期间，DML 产生的 CDC 事件暂存于此，
    /// 直到 COMMIT 成功后统一分发（消除脏读）；ROLLBACK 时直接丢弃。
    /// autocommit 模式不经过此缓冲，直接 dispatch_event。
    txn_buffers: std::sync::Mutex<std::collections::HashMap<u32, Vec<ChangeEvent>>>,
}

impl CdcEngine {
    /// 创建 CDC 引擎，使用 SystemTime 作为时间戳源
    pub fn new(observer_manager: Arc<CdcObserverManager>) -> Self {
        Self::with_timestamp_fn(
            observer_manager,
            Box::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0)
            }),
        )
    }

    /// 创建 CDC 引擎，注入自定义时间戳函数（便于测试固定时间戳）
    pub fn with_timestamp_fn(
        observer_manager: Arc<CdcObserverManager>,
        timestamp_fn: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            observer_manager,
            schema_change_manager: Arc::new(schema::SchemaChangeObserverManager::new()),
            timestamp_fn,
            total_processed: std::sync::atomic::AtomicU64::new(0),
            pending_events: std::sync::atomic::AtomicU64::new(0),
            lsn_counter: std::sync::atomic::AtomicU64::new(1),
            txn_buffers: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 获取已处理的 WalRecord 总数
    pub fn total_processed(&self) -> u64 {
        self.total_processed
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// P7-1：获取当前时间戳（供外部调用方构造 ChangeEvent 时使用）
    pub fn current_timestamp(&self) -> u64 {
        (self.timestamp_fn)()
    }

    /// P7-1：生成下一个全局 LSN（跨 Executor 共享，保证单调递增）
    ///
    /// 所有共享同一 CdcEngine 的 Executor 实例都会从此计数器获取唯一 LSN，
    /// 确保变更事件的全局有序性。生产环境中 LSN 应来自 WAL，当前为简化模型。
    pub fn next_lsn(&self) -> u64 {
        self.lsn_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// 获取当前缓冲中的事件数（pending）
    ///
    /// 注：当前实现为同步分发，pending 始终为 0；预留接口供后续 Phase 2.5.8 异步分发背压处理
    pub fn pending_event_count(&self) -> u64 {
        self.pending_events
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 获取已分发的（observer × event）总次数（透传到 CdcObserverManager）
    pub fn total_dispatched(&self) -> u64 {
        self.observer_manager.total_dispatched()
    }

    /// 获取 CDC 观察者数量
    pub fn observer_count(&self) -> usize {
        self.observer_manager.observer_count()
    }

    /// 直接分发一个 ChangeEvent（P4-3 基准测试与端到端测试用）
    ///
    /// 同步调用所有已注册 observer 的 `on_event`。
    /// 正常生产路径是通过 `WalObserver::on_commit` 触发，此方法供测试/基准测试
    /// 直接推送合成事件，无需经过 WAL 层。
    pub fn dispatch_event(&self, event: ChangeEvent) {
        self.pending_events
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.observer_manager.notify(event);
        self.pending_events
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Batch 3：将 CDC 事件暂存到事务缓冲（COMMIT 后分发）
    ///
    /// 显式事务（BEGIN...COMMIT）期间，DML 产生的事件不立即分发，
    /// 而是按 txn_id 缓冲。COMMIT 成功后调用 `commit_txn` 统一分发；
    /// ROLLBACK 时调用 `abort_txn` 丢弃。
    pub fn stage_event(&self, txn_id: u32, event: ChangeEvent) {
        let mut buffers = self.txn_buffers.lock().unwrap_or_else(|e| e.into_inner());
        buffers.entry(txn_id).or_default().push(event);
    }

    /// Batch 3：事务提交后，将缓冲的事件统一分发给所有 observer
    ///
    /// 返回分发的事件数量。若 txn_id 无缓冲事件，返回 0。
    pub fn commit_txn(&self, txn_id: u32) -> usize {
        let events = {
            let mut buffers = self.txn_buffers.lock().unwrap_or_else(|e| e.into_inner());
            buffers.remove(&txn_id).unwrap_or_default()
        };
        let count = events.len();
        for event in events {
            self.dispatch_event(event);
        }
        count
    }

    /// P2-2：将指定 tx_id 的所有 staged 事件统一分发到 observer（autocommit 模式使用）
    ///
    /// # 设计目的
    /// autocommit 模式下，DML 产生的 CDC 事件不再逐条同步分发，而是先 stage 到
    /// 缓冲区（虚拟 tx_id=1，见 `Executor::dispatch_cdc_*`），在语句执行完成后
    /// 由 `Executor::flush_autocommit_cdc_events` 调用此方法一次性分发，减少每行的
    /// 同步开销（observer 锁获取/释放、catch_unwind 等）。
    ///
    /// # 与 `commit_txn` 的语义区别
    /// - `commit_txn`：用于显式事务提交后分发（语义：事务已提交，变更可见）
    /// - `flush_staged_events`：用于 autocommit 模式语句执行完成后分发
    ///   （语义：语句已完成，将暂存事件统一分发以降低同步开销）
    ///
    /// 两者底层逻辑一致：取出指定 tx_id 的缓冲事件并逐一分发。本方法复用
    /// `commit_txn` 的实现以避免代码重复。
    ///
    /// # 参数
    /// - `tx_id`：要 flush 的事务 ID（autocommit 模式下为虚拟 tx_id=1）
    ///
    /// # 返回
    /// 分发的事件数量。若 tx_id 无缓冲事件，返回 0（no-op）。
    pub fn flush_staged_events(&self, tx_id: u32) -> usize {
        // 复用 commit_txn 的分发逻辑：取出缓冲事件并逐一分发
        self.commit_txn(tx_id)
    }

    /// Batch 3：事务回滚时，丢弃缓冲的 CDC 事件（不分发）
    ///
    /// 返回丢弃的事件数量。若 txn_id 无缓冲事件，返回 0。
    pub fn abort_txn(&self, txn_id: u32) -> usize {
        let mut buffers = self.txn_buffers.lock().unwrap_or_else(|e| e.into_inner());
        buffers.remove(&txn_id).map(|v| v.len()).unwrap_or(0)
    }

    /// Batch 3：查询指定事务的缓冲事件数（测试/监控用）
    pub fn staged_event_count(&self, txn_id: u32) -> usize {
        let buffers = self.txn_buffers.lock().unwrap_or_else(|e| e.into_inner());
        buffers.get(&txn_id).map(|v| v.len()).unwrap_or(0)
    }

    /// 注册 CdcObserver（透传到 CdcObserverManager）
    ///
    /// 供 `ReplicationTaskManager` 注册复制任务为 observer 使用。
    /// 返回 `true` 表示注册成功；若已注册相同指针的 observer，返回 `false`。
    pub fn register_observer_arc(&self, observer: Arc<dyn CdcObserver>) -> bool {
        self.observer_manager.register(observer)
    }

    /// 注销 CdcObserver（透传到 CdcObserverManager）
    ///
    /// 供 `ReplicationTaskManager` 注销复制任务使用。
    /// 返回 `true` 表示注销成功；若未找到，返回 `false`。
    pub fn unregister_observer_arc<O: CdcObserver + 'static>(&self, observer: &Arc<O>) -> bool {
        self.observer_manager.unregister(observer)
    }

    /// 注册 SchemaChangeObserver（P4-2，DDL 事件分发）
    ///
    /// 供 `ReplicationTaskManager` 注册复制任务为 schema 变更观察者使用。
    /// 返回 `true` 表示注册成功；若已注册相同指针的 observer，返回 `false`。
    pub fn register_schema_observer(
        &self,
        observer: Arc<dyn schema::SchemaChangeObserver>,
    ) -> bool {
        self.schema_change_manager.register(observer)
    }

    /// 注销 SchemaChangeObserver（P4-2）
    ///
    /// 供 `ReplicationTaskManager` 注销复制任务使用。
    /// 返回 `true` 表示注销成功；若未找到，返回 `false`。
    pub fn unregister_schema_observer<O: schema::SchemaChangeObserver + 'static>(
        &self,
        observer: &Arc<O>,
    ) -> bool {
        self.schema_change_manager.unregister(observer)
    }

    /// 通知所有 SchemaChangeObserver：分发一个 SchemaChangeEvent（P4-2）
    ///
    /// **调用时机**：上层（如 SQL 执行器执行 DDL 时）调用此方法通知 CDC 引擎。
    /// CDC 引擎将事件分发给所有已注册的 SchemaChangeObserver（如 ReplicationTask）。
    ///
    /// **参数**：
    /// - `event`：SchemaChangeEvent（CreateTable/AlterTable/DropTable）
    pub fn notify_schema_change(&self, event: schema::SchemaChangeEvent) {
        self.schema_change_manager.notify(event);
    }

    /// 获取已注册的 SchemaChangeObserver 数量（P4-2）
    pub fn schema_observer_count(&self) -> usize {
        self.schema_change_manager.observer_count()
    }

    /// 获取已分发的 SchemaChangeEvent 总次数（P4-2）
    pub fn schema_change_dispatched(&self) -> u64 {
        self.schema_change_manager.total_dispatched()
    }

    /// 将一批 WalRecord 转换为 ChangeEvent 并分发
    ///
    /// **流程**：
    /// 1. 遍历 records，调用 `ChangeEvent::from_wal_record` 转换
    /// 2. 过滤掉返回 None 的记录（FullPageImage、Checkpoint）
    /// 3. 对每个 ChangeEvent 调用 `CdcObserverManager::notify` 分发
    /// 4. 更新统计计数
    fn process_records(&self, records: Vec<WalRecord>) {
        let timestamp = (self.timestamp_fn)();
        let count = records.len() as u64;
        for record in records.iter() {
            if let Some(event) = ChangeEvent::from_wal_record(record, timestamp) {
                self.pending_events
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.observer_manager.notify(event);
                self.pending_events
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        self.total_processed
            .fetch_add(count, std::sync::atomic::Ordering::SeqCst);
    }
}

impl WalObserver for CdcEngine {
    fn on_commit(&self, _tx_id: u32, records: Vec<WalRecord>) {
        self.process_records(records);
    }

    fn on_rollback(&self, tx_id: u32) {
        // on_rollback 不携带 records，仅构造一个 Abort 事件分发
        let timestamp = (self.timestamp_fn)();
        let event = ChangeEvent::abort(tx_id, 0, timestamp);
        self.pending_events
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.observer_manager.notify(event);
        self.pending_events
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

// =====================================================================
// 辅助：收集型 CdcObserver（测试用）
// =====================================================================

/// 收集型 CdcObserver — 将接收到的所有 ChangeEvent 存入 Mutex<Vec>
///
/// 主要用于测试：注册后可通过 `events()` 获取所有接收的事件
pub struct CollectingObserver {
    events: Mutex<Vec<ChangeEvent>>,
}

impl CollectingObserver {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    /// 获取已接收的事件快照（clone）
    pub fn events(&self) -> Vec<ChangeEvent> {
        self.events.lock().unwrap().clone()
    }

    /// 获取已接收的事件数量
    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.events.lock().unwrap().is_empty()
    }

    /// 清空已接收的事件
    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

impl Default for CollectingObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl CdcObserver for CollectingObserver {
    fn on_event(&self, event: ChangeEvent) {
        self.events.lock().unwrap().push(event);
    }
}

// =====================================================================
// 辅助：计数型 CdcObserver（测试用）
// =====================================================================

/// 计数型 CdcObserver — 仅统计接收的事件数量，不存储事件本身
///
/// 用于并发测试：避免 Mutex<Vec> 在高并发下成为瓶颈
pub struct CountingObserver {
    count: std::sync::atomic::AtomicU64,
    /// 按 op 分类的计数
    insert_count: std::sync::atomic::AtomicU64,
    update_count: std::sync::atomic::AtomicU64,
    delete_count: std::sync::atomic::AtomicU64,
    commit_count: std::sync::atomic::AtomicU64,
    abort_count: std::sync::atomic::AtomicU64,
}

impl CountingObserver {
    pub fn new() -> Self {
        Self {
            count: std::sync::atomic::AtomicU64::new(0),
            insert_count: std::sync::atomic::AtomicU64::new(0),
            update_count: std::sync::atomic::AtomicU64::new(0),
            delete_count: std::sync::atomic::AtomicU64::new(0),
            commit_count: std::sync::atomic::AtomicU64::new(0),
            abort_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn count(&self) -> u64 {
        self.count.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn insert_count(&self) -> u64 {
        self.insert_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn update_count(&self) -> u64 {
        self.update_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn delete_count(&self) -> u64 {
        self.delete_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn commit_count(&self) -> u64 {
        self.commit_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn abort_count(&self) -> u64 {
        self.abort_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Default for CountingObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl CdcObserver for CountingObserver {
    fn on_event(&self, event: ChangeEvent) {
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match event.op {
            CdcEventOp::Insert => {
                self.insert_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            CdcEventOp::Update => {
                self.update_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            CdcEventOp::Delete => {
                self.delete_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            CdcEventOp::Commit => {
                self.commit_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            CdcEventOp::Abort => {
                self.abort_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }
}

// =====================================================================
// 返回 crate 版本号（保留 workspace 骨架冒烟测试）
// =====================================================================

/// 返回 crate 版本号，供 workspace 骨架冒烟测试使用。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod cdc_fuzz;

/// Debezium JSON 适配器 — 将 ChangeEvent 转换为 Debezium Connect 官方 JSON 格式
///
/// 在生产代码中可用（Kafka Sink 等模块依赖），同时保留测试模块引用。
pub mod debezium;

#[cfg(test)]
mod debezium_avro;

#[cfg(test)]
mod e2e_tests;

/// P4-3 性能基准测试模块
///
/// 所有测试标记为 `#[ignore]`，需显式触发：
/// `cargo test -p szrsql-cdc --release --lib benchmarks -- --ignored --nocapture`
#[cfg(test)]
mod benchmarks;

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_tx::wal::{WalObserverManager, WalOpType, WalRecord};

    /// 版本号非空（保留 workspace 骨架冒烟测试）
    #[test]
    fn version_returns_nonempty() {
        assert!(!version().is_empty());
    }

    // =================================================================
    // Phase 2.5.1: WalObserver trait + CDCEngine 测试
    // =================================================================

    mod phase_2_5_1 {
        use super::*;
        use std::sync::Arc;

        // -----------------------------------------------------------------
        // 1. CdcEventOp 转换
        // -----------------------------------------------------------------

        #[test]
        fn cdc_event_op_from_wal_op_insert() {
            assert_eq!(
                CdcEventOp::from_wal_op(WalOpType::Insert),
                Some(CdcEventOp::Insert)
            );
        }

        #[test]
        fn cdc_event_op_from_wal_op_update() {
            assert_eq!(
                CdcEventOp::from_wal_op(WalOpType::Update),
                Some(CdcEventOp::Update)
            );
        }

        #[test]
        fn cdc_event_op_from_wal_op_delete() {
            assert_eq!(
                CdcEventOp::from_wal_op(WalOpType::Delete),
                Some(CdcEventOp::Delete)
            );
        }

        #[test]
        fn cdc_event_op_from_wal_op_commit() {
            assert_eq!(
                CdcEventOp::from_wal_op(WalOpType::Commit),
                Some(CdcEventOp::Commit)
            );
        }

        #[test]
        fn cdc_event_op_from_wal_op_abort() {
            assert_eq!(
                CdcEventOp::from_wal_op(WalOpType::Abort),
                Some(CdcEventOp::Abort)
            );
        }

        #[test]
        fn cdc_event_op_from_wal_op_filters_full_page_image() {
            assert_eq!(CdcEventOp::from_wal_op(WalOpType::FullPageImage), None);
        }

        #[test]
        fn cdc_event_op_from_wal_op_filters_checkpoint() {
            assert_eq!(CdcEventOp::from_wal_op(WalOpType::Checkpoint), None);
        }

        #[test]
        fn cdc_event_op_as_str_correct() {
            assert_eq!(CdcEventOp::Insert.as_str(), "insert");
            assert_eq!(CdcEventOp::Update.as_str(), "update");
            assert_eq!(CdcEventOp::Delete.as_str(), "delete");
            assert_eq!(CdcEventOp::Commit.as_str(), "commit");
            assert_eq!(CdcEventOp::Abort.as_str(), "abort");
        }

        // -----------------------------------------------------------------
        // 2. ChangeEvent 构造器
        // -----------------------------------------------------------------

        #[test]
        fn change_event_insert_constructor() {
            let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            assert_eq!(event.tx_id, 1);
            assert_eq!(event.lsn, 100);
            assert_eq!(event.op, CdcEventOp::Insert);
            assert_eq!(event.table_id, Some(42));
            assert_eq!(event.old_row, None);
            assert_eq!(event.new_row, Some(vec![1, 2, 3]));
            assert_eq!(event.timestamp, 12345);
        }

        #[test]
        fn change_event_update_constructor() {
            let event = ChangeEvent::update(1, 100, 42, vec![1], vec![2], 12345);
            assert_eq!(event.op, CdcEventOp::Update);
            assert_eq!(event.old_row, Some(vec![1]));
            assert_eq!(event.new_row, Some(vec![2]));
        }

        #[test]
        fn change_event_delete_constructor() {
            let event = ChangeEvent::delete(1, 100, 42, vec![1, 2], 12345);
            assert_eq!(event.op, CdcEventOp::Delete);
            assert_eq!(event.old_row, Some(vec![1, 2]));
            assert_eq!(event.new_row, None);
        }

        #[test]
        fn change_event_commit_constructor() {
            let event = ChangeEvent::commit(1, 100, 12345);
            assert_eq!(event.op, CdcEventOp::Commit);
            assert_eq!(event.table_id, None);
            assert_eq!(event.old_row, None);
            assert_eq!(event.new_row, None);
        }

        #[test]
        fn change_event_abort_constructor() {
            let event = ChangeEvent::abort(1, 100, 12345);
            assert_eq!(event.op, CdcEventOp::Abort);
            assert_eq!(event.table_id, None);
        }

        // -----------------------------------------------------------------
        // 3. ChangeEvent::from_wal_record
        // -----------------------------------------------------------------

        #[test]
        fn change_event_from_wal_record_insert() {
            let record = WalRecord::new(100, 1, WalOpType::Insert, 42, vec![1, 2, 3]);
            let event = ChangeEvent::from_wal_record(&record, 12345).unwrap();
            assert_eq!(event.op, CdcEventOp::Insert);
            assert_eq!(event.tx_id, 1);
            assert_eq!(event.lsn, 100);
            assert_eq!(event.table_id, Some(42));
            assert_eq!(event.new_row, Some(vec![1, 2, 3]));
            assert_eq!(event.timestamp, 12345);
        }

        #[test]
        fn change_event_from_wal_record_update() {
            let record = WalRecord::new(100, 1, WalOpType::Update, 42, vec![9, 9]);
            let event = ChangeEvent::from_wal_record(&record, 12345).unwrap();
            assert_eq!(event.op, CdcEventOp::Update);
            assert_eq!(event.new_row, Some(vec![9, 9]));
        }

        #[test]
        fn change_event_from_wal_record_delete() {
            let record = WalRecord::new(100, 1, WalOpType::Delete, 42, vec![1]);
            let event = ChangeEvent::from_wal_record(&record, 12345).unwrap();
            assert_eq!(event.op, CdcEventOp::Delete);
            assert_eq!(event.old_row, Some(vec![1]));
            assert_eq!(event.new_row, None);
        }

        #[test]
        fn change_event_from_wal_record_commit() {
            let record = WalRecord::new(100, 1, WalOpType::Commit, 0, vec![]);
            let event = ChangeEvent::from_wal_record(&record, 12345).unwrap();
            assert_eq!(event.op, CdcEventOp::Commit);
            assert_eq!(event.table_id, None);
        }

        #[test]
        fn change_event_from_wal_record_abort() {
            let record = WalRecord::new(100, 1, WalOpType::Abort, 0, vec![]);
            let event = ChangeEvent::from_wal_record(&record, 12345).unwrap();
            assert_eq!(event.op, CdcEventOp::Abort);
        }

        #[test]
        fn change_event_from_wal_record_filters_full_page_image() {
            let record = WalRecord::new(100, 1, WalOpType::FullPageImage, 42, vec![0; 8192]);
            assert!(ChangeEvent::from_wal_record(&record, 12345).is_none());
        }

        #[test]
        fn change_event_from_wal_record_filters_checkpoint() {
            let record = WalRecord::new(100, 1, WalOpType::Checkpoint, 0, vec![]);
            assert!(ChangeEvent::from_wal_record(&record, 12345).is_none());
        }

        // -----------------------------------------------------------------
        // 4. CdcObserverManager register/unregister/notify
        // -----------------------------------------------------------------

        #[test]
        fn observer_manager_new_is_empty() {
            let mgr = CdcObserverManager::new();
            assert_eq!(mgr.observer_count(), 0);
            assert_eq!(mgr.total_dispatched(), 0);
        }

        #[test]
        fn observer_manager_register_increases_count() {
            let mgr = CdcObserverManager::new();
            let obs = Arc::new(CollectingObserver::new());
            assert!(mgr.register(obs.clone()));
            assert_eq!(mgr.observer_count(), 1);
        }

        #[test]
        fn observer_manager_register_duplicate_returns_false() {
            let mgr = CdcObserverManager::new();
            let obs = Arc::new(CollectingObserver::new());
            assert!(mgr.register(obs.clone()));
            assert!(!mgr.register(obs.clone())); // 重复注册返回 false
            assert_eq!(mgr.observer_count(), 1);
        }

        #[test]
        fn observer_manager_unregister_decreases_count() {
            let mgr = CdcObserverManager::new();
            let obs = Arc::new(CollectingObserver::new());
            mgr.register(obs.clone());
            assert!(mgr.unregister(&obs));
            assert_eq!(mgr.observer_count(), 0);
        }

        #[test]
        fn observer_manager_unregister_nonexistent_returns_false() {
            let mgr = CdcObserverManager::new();
            let obs = Arc::new(CollectingObserver::new());
            assert!(!mgr.unregister(&obs)); // 未注册返回 false
        }

        #[test]
        fn observer_manager_notify_single_observer() {
            let mgr = CdcObserverManager::new();
            let obs = Arc::new(CollectingObserver::new());
            mgr.register(obs.clone());

            let event = ChangeEvent::insert(1, 100, 42, vec![1], 12345);
            mgr.notify(event);

            assert_eq!(obs.len(), 1);
            assert_eq!(obs.events()[0].op, CdcEventOp::Insert);
            assert_eq!(mgr.total_dispatched(), 1);
        }

        #[test]
        fn observer_manager_notify_multiple_observers_independent() {
            let mgr = CdcObserverManager::new();
            let obs1 = Arc::new(CollectingObserver::new());
            let obs2 = Arc::new(CollectingObserver::new());
            let obs3 = Arc::new(CollectingObserver::new());
            mgr.register(obs1.clone());
            mgr.register(obs2.clone());
            mgr.register(obs3.clone());

            let event = ChangeEvent::insert(1, 100, 42, vec![1], 12345);
            mgr.notify(event);

            // 3 个 observer 各自独立接收
            assert_eq!(obs1.len(), 1);
            assert_eq!(obs2.len(), 1);
            assert_eq!(obs3.len(), 1);
            assert_eq!(mgr.total_dispatched(), 3);
        }

        #[test]
        fn observer_manager_notify_no_observers_noop() {
            let mgr = CdcObserverManager::new();
            let event = ChangeEvent::insert(1, 100, 42, vec![1], 12345);
            mgr.notify(event);
            assert_eq!(mgr.total_dispatched(), 0);
        }

        #[test]
        fn observer_manager_notify_panic_isolated() {
            /// panic observer：on_event 时 panic
            struct PanicObserver;
            impl CdcObserver for PanicObserver {
                fn on_event(&self, _event: ChangeEvent) {
                    panic!("intentional panic");
                }
            }

            let mgr = CdcObserverManager::new();
            let panic_obs = Arc::new(PanicObserver);
            let collect_obs = Arc::new(CollectingObserver::new());
            mgr.register(panic_obs);
            mgr.register(collect_obs.clone());

            // notify 不应因 panic_obs 的 panic 而中断
            let event = ChangeEvent::insert(1, 100, 42, vec![1], 12345);
            mgr.notify(event);

            // collect_obs 仍然收到事件
            assert_eq!(collect_obs.len(), 1);
        }

        // -----------------------------------------------------------------
        // 5. CdcEngine 实现 WalObserver
        // -----------------------------------------------------------------

        #[test]
        fn cdc_engine_new_initial_state() {
            let mgr = Arc::new(CdcObserverManager::new());
            let engine = CdcEngine::new(mgr.clone());
            assert_eq!(engine.total_processed(), 0);
            assert_eq!(engine.pending_event_count(), 0);
            assert_eq!(engine.total_dispatched(), 0);
            assert_eq!(engine.observer_count(), 0);
        }

        #[test]
        fn cdc_engine_on_commit_dispatches_events() {
            let mgr = Arc::new(CdcObserverManager::new());
            let obs = Arc::new(CollectingObserver::new());
            mgr.register(obs.clone());

            // 固定时间戳
            let engine = CdcEngine::with_timestamp_fn(mgr.clone(), Box::new(|| 99999));

            // 构造事务的 WAL 记录：1 Insert + 1 Update + 1 Delete + 1 Commit
            let records = vec![
                WalRecord::new(100, 1, WalOpType::Insert, 42, vec![1]),
                WalRecord::new(101, 1, WalOpType::Update, 42, vec![2]),
                WalRecord::new(102, 1, WalOpType::Delete, 42, vec![3]),
                WalRecord::new(103, 1, WalOpType::Commit, 0, vec![]),
            ];

            engine.on_commit(1, records);

            // 应产生 4 个事件（Insert + Update + Delete + Commit）
            assert_eq!(obs.len(), 4);
            assert_eq!(obs.events()[0].op, CdcEventOp::Insert);
            assert_eq!(obs.events()[1].op, CdcEventOp::Update);
            assert_eq!(obs.events()[2].op, CdcEventOp::Delete);
            assert_eq!(obs.events()[3].op, CdcEventOp::Commit);
            assert_eq!(engine.total_processed(), 4);
            assert_eq!(engine.total_dispatched(), 4);
        }

        #[test]
        fn cdc_engine_on_commit_filters_full_page_image() {
            let mgr = Arc::new(CdcObserverManager::new());
            let obs = Arc::new(CollectingObserver::new());
            mgr.register(obs.clone());

            let engine = CdcEngine::with_timestamp_fn(mgr.clone(), Box::new(|| 0));

            let records = vec![
                WalRecord::new(100, 1, WalOpType::Insert, 42, vec![1]),
                WalRecord::new(101, 1, WalOpType::FullPageImage, 42, vec![0; 100]),
                WalRecord::new(102, 1, WalOpType::Commit, 0, vec![]),
            ];

            engine.on_commit(1, records);

            // FullPageImage 被过滤，只剩 Insert + Commit
            assert_eq!(obs.len(), 2);
            assert_eq!(obs.events()[0].op, CdcEventOp::Insert);
            assert_eq!(obs.events()[1].op, CdcEventOp::Commit);
        }

        #[test]
        fn cdc_engine_on_commit_filters_checkpoint() {
            let mgr = Arc::new(CdcObserverManager::new());
            let obs = Arc::new(CollectingObserver::new());
            mgr.register(obs.clone());

            let engine = CdcEngine::with_timestamp_fn(mgr.clone(), Box::new(|| 0));

            let records = vec![
                WalRecord::new(100, 1, WalOpType::Insert, 42, vec![1]),
                WalRecord::new(101, 1, WalOpType::Checkpoint, 0, vec![]),
                WalRecord::new(102, 1, WalOpType::Commit, 0, vec![]),
            ];

            engine.on_commit(1, records);

            assert_eq!(obs.len(), 2); // Insert + Commit，Checkpoint 被过滤
        }

        #[test]
        fn cdc_engine_on_commit_no_observers_noop() {
            let mgr = Arc::new(CdcObserverManager::new());
            let engine = CdcEngine::with_timestamp_fn(mgr.clone(), Box::new(|| 0));

            let records = vec![WalRecord::new(100, 1, WalOpType::Commit, 0, vec![])];
            engine.on_commit(1, records);

            // 无 observer，notify 是 noop，但 total_processed 仍计数
            assert_eq!(engine.total_processed(), 1);
            assert_eq!(engine.total_dispatched(), 0);
        }

        #[test]
        fn cdc_engine_on_rollback_dispatches_abort_event() {
            let mgr = Arc::new(CdcObserverManager::new());
            let obs = Arc::new(CollectingObserver::new());
            mgr.register(obs.clone());

            let engine = CdcEngine::with_timestamp_fn(mgr.clone(), Box::new(|| 0));

            engine.on_rollback(42);

            assert_eq!(obs.len(), 1);
            assert_eq!(obs.events()[0].op, CdcEventOp::Abort);
            assert_eq!(obs.events()[0].tx_id, 42);
        }

        #[test]
        fn cdc_engine_multiple_observers_all_receive() {
            let mgr = Arc::new(CdcObserverManager::new());
            let obs1 = Arc::new(CountingObserver::new());
            let obs2 = Arc::new(CountingObserver::new());
            let obs3 = Arc::new(CountingObserver::new());
            mgr.register(obs1.clone());
            mgr.register(obs2.clone());
            mgr.register(obs3.clone());

            let engine = CdcEngine::with_timestamp_fn(mgr.clone(), Box::new(|| 0));

            let records = vec![
                WalRecord::new(100, 1, WalOpType::Insert, 42, vec![1]),
                WalRecord::new(101, 1, WalOpType::Commit, 0, vec![]),
            ];

            engine.on_commit(1, records);

            // 3 个 observer 都收到 2 个事件
            assert_eq!(obs1.count(), 2);
            assert_eq!(obs2.count(), 2);
            assert_eq!(obs3.count(), 2);
            assert_eq!(obs1.insert_count(), 1);
            assert_eq!(obs1.commit_count(), 1);
            assert_eq!(engine.total_dispatched(), 6); // 3 observers × 2 events
        }

        // -----------------------------------------------------------------
        // 6. CdcEngine 集成 WalObserverManager
        // -----------------------------------------------------------------

        #[test]
        fn cdc_engine_integrates_with_wal_observer_manager() {
            // 验证 CdcEngine 可作为 WalObserver 注册到 WalObserverManager
            let cdc_mgr = Arc::new(CdcObserverManager::new());
            let obs = Arc::new(CollectingObserver::new());
            cdc_mgr.register(obs.clone());

            let engine = Arc::new(CdcEngine::with_timestamp_fn(
                cdc_mgr.clone(),
                Box::new(|| 0),
            ));

            let wal_mgr = Arc::new(WalObserverManager::new());
            assert!(wal_mgr.register(engine.clone()));
            assert_eq!(wal_mgr.observer_count(), 1);

            // 模拟事务提交：通知 WalObserverManager
            let records = vec![
                WalRecord::new(100, 1, WalOpType::Insert, 42, vec![1]),
                WalRecord::new(101, 1, WalOpType::Commit, 0, vec![]),
            ];
            wal_mgr.notify_commit(1, records);

            // CdcEngine 应收到通知并分发 ChangeEvent
            assert_eq!(obs.len(), 2); // Insert + Commit
            assert_eq!(engine.total_processed(), 2);
        }

        #[test]
        fn cdc_engine_integrates_with_wal_observer_manager_rollback() {
            let cdc_mgr = Arc::new(CdcObserverManager::new());
            let obs = Arc::new(CollectingObserver::new());
            cdc_mgr.register(obs.clone());

            let engine = Arc::new(CdcEngine::with_timestamp_fn(
                cdc_mgr.clone(),
                Box::new(|| 0),
            ));

            let wal_mgr = Arc::new(WalObserverManager::new());
            wal_mgr.register(engine.clone());

            wal_mgr.notify_rollback(99);

            assert_eq!(obs.len(), 1);
            assert_eq!(obs.events()[0].op, CdcEventOp::Abort);
            assert_eq!(obs.events()[0].tx_id, 99);
        }

        // -----------------------------------------------------------------
        // 7. CollectingObserver / CountingObserver 辅助测试
        // -----------------------------------------------------------------

        #[test]
        fn collecting_observer_basic_operations() {
            let obs = CollectingObserver::new();
            assert!(obs.is_empty());

            let event = ChangeEvent::insert(1, 100, 42, vec![1], 0);
            obs.on_event(event);
            assert_eq!(obs.len(), 1);
            assert!(!obs.is_empty());

            obs.clear();
            assert!(obs.is_empty());
        }

        #[test]
        fn counting_observer_counts_by_op() {
            let obs = CountingObserver::new();
            obs.on_event(ChangeEvent::insert(1, 1, 1, vec![], 0));
            obs.on_event(ChangeEvent::insert(1, 2, 1, vec![], 0));
            obs.on_event(ChangeEvent::update(1, 3, 1, vec![], vec![], 0));
            obs.on_event(ChangeEvent::delete(1, 4, 1, vec![], 0));
            obs.on_event(ChangeEvent::commit(1, 5, 0));
            obs.on_event(ChangeEvent::abort(1, 6, 0));

            assert_eq!(obs.count(), 6);
            assert_eq!(obs.insert_count(), 2);
            assert_eq!(obs.update_count(), 1);
            assert_eq!(obs.delete_count(), 1);
            assert_eq!(obs.commit_count(), 1);
            assert_eq!(obs.abort_count(), 1);
        }
    }

    // =================================================================
    // Phase 2.5.3: ChangeEvent 序列化/反序列化（与 2.5.1 同文件交付）
    // =================================================================

    mod phase_2_5_3 {
        use super::*;

        // -----------------------------------------------------------------
        // JSON 序列化
        // -----------------------------------------------------------------

        #[test]
        fn change_event_json_insert_roundtrip() {
            let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            let json = event.to_json().unwrap();
            let decoded = ChangeEvent::from_json(&json).unwrap();
            assert_eq!(event, decoded);
        }

        #[test]
        fn change_event_json_update_roundtrip() {
            let event = ChangeEvent::update(1, 100, 42, vec![1], vec![2], 12345);
            let json = event.to_json().unwrap();
            let decoded = ChangeEvent::from_json(&json).unwrap();
            assert_eq!(event, decoded);
        }

        #[test]
        fn change_event_json_delete_roundtrip() {
            let event = ChangeEvent::delete(1, 100, 42, vec![1, 2], 12345);
            let json = event.to_json().unwrap();
            let decoded = ChangeEvent::from_json(&json).unwrap();
            assert_eq!(event, decoded);
        }

        #[test]
        fn change_event_json_commit_roundtrip() {
            let event = ChangeEvent::commit(1, 100, 12345);
            let json = event.to_json().unwrap();
            let decoded = ChangeEvent::from_json(&json).unwrap();
            assert_eq!(event, decoded);
        }

        #[test]
        fn change_event_json_abort_roundtrip() {
            let event = ChangeEvent::abort(1, 100, 12345);
            let json = event.to_json().unwrap();
            let decoded = ChangeEvent::from_json(&json).unwrap();
            assert_eq!(event, decoded);
        }

        #[test]
        fn change_event_json_fields_present() {
            let event = ChangeEvent::insert(7, 999, 42, vec![1, 2], 12345);
            let json = event.to_json().unwrap();
            assert!(json.contains("\"tx_id\":7"));
            assert!(json.contains("\"lsn\":999"));
            assert!(json.contains("\"op\":\"insert\""));
            assert!(json.contains("\"table_id\":42"));
            assert!(json.contains("\"new_row\":[1,2]"));
            assert!(json.contains("\"timestamp\":12345"));
            // old_row 为 None，输出为 null
            assert!(json.contains("\"old_row\":null"));
        }

        #[test]
        fn change_event_json_commit_no_table_id_no_rows() {
            let event = ChangeEvent::commit(1, 100, 12345);
            let json = event.to_json().unwrap();
            assert!(json.contains("\"op\":\"commit\""));
            // None 字段输出为 null
            assert!(json.contains("\"table_id\":null"));
            assert!(json.contains("\"old_row\":null"));
            assert!(json.contains("\"new_row\":null"));
        }

        // -----------------------------------------------------------------
        // bincode 序列化
        // -----------------------------------------------------------------

        #[test]
        fn change_event_bincode_insert_roundtrip() {
            let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            let bytes = event.to_bincode().unwrap();
            let decoded = ChangeEvent::from_bincode(&bytes).unwrap();
            assert_eq!(event, decoded);
        }

        #[test]
        fn change_event_bincode_update_roundtrip() {
            let event = ChangeEvent::update(1, 100, 42, vec![1], vec![2], 12345);
            let bytes = event.to_bincode().unwrap();
            let decoded = ChangeEvent::from_bincode(&bytes).unwrap();
            assert_eq!(event, decoded);
        }

        #[test]
        fn change_event_bincode_delete_roundtrip() {
            let event = ChangeEvent::delete(1, 100, 42, vec![1, 2], 12345);
            let bytes = event.to_bincode().unwrap();
            let decoded = ChangeEvent::from_bincode(&bytes).unwrap();
            assert_eq!(event, decoded);
        }

        #[test]
        fn change_event_bincode_commit_roundtrip() {
            let event = ChangeEvent::commit(1, 100, 12345);
            let bytes = event.to_bincode().unwrap();
            let decoded = ChangeEvent::from_bincode(&bytes).unwrap();
            assert_eq!(event, decoded);
        }

        #[test]
        fn change_event_bincode_abort_roundtrip() {
            let event = ChangeEvent::abort(1, 100, 12345);
            let bytes = event.to_bincode().unwrap();
            let decoded = ChangeEvent::from_bincode(&bytes).unwrap();
            assert_eq!(event, decoded);
        }

        #[test]
        fn change_event_bincode_compact_size() {
            // bincode 应比 JSON 紧凑
            let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 12345);
            let json_size = event.to_json().unwrap().len();
            let bincode_size = event.to_bincode().unwrap().len();
            assert!(
                bincode_size < json_size,
                "bincode ({}) should be smaller than JSON ({})",
                bincode_size,
                json_size
            );
        }

        // -----------------------------------------------------------------
        // JSON ↔ bincode 双向一致
        // -----------------------------------------------------------------

        #[test]
        fn change_event_json_and_bincode_produce_same_event() {
            let event = ChangeEvent::update(42, 999, 100, vec![1, 2], vec![3, 4], 88888);
            let json_decoded = ChangeEvent::from_json(&event.to_json().unwrap()).unwrap();
            let bincode_decoded = ChangeEvent::from_bincode(&event.to_bincode().unwrap()).unwrap();
            assert_eq!(json_decoded, bincode_decoded);
            assert_eq!(json_decoded, event);
        }

        // -----------------------------------------------------------------
        // 序列化异常处理
        // -----------------------------------------------------------------

        #[test]
        fn change_event_from_json_invalid_returns_error() {
            let result = ChangeEvent::from_json("not a valid json");
            assert!(result.is_err());
        }

        #[test]
        fn change_event_from_json_missing_field_returns_error() {
            // 缺少 tx_id 字段
            let json = r#"{"lsn":100,"op":"insert","timestamp":0}"#;
            let result = ChangeEvent::from_json(json);
            assert!(result.is_err());
        }

        #[test]
        fn change_event_from_bincode_invalid_returns_error() {
            let result = ChangeEvent::from_bincode(&[0xFF, 0xFF, 0xFF]);
            assert!(result.is_err());
        }

        // -----------------------------------------------------------------
        // 批量序列化
        // -----------------------------------------------------------------

        #[test]
        fn change_event_batch_json_roundtrip() {
            let events = vec![
                ChangeEvent::insert(1, 100, 42, vec![1], 0),
                ChangeEvent::update(1, 101, 42, vec![1], vec![2], 0),
                ChangeEvent::delete(1, 102, 42, vec![2], 0),
                ChangeEvent::commit(1, 103, 0),
            ];
            let json = serde_json::to_string(&events).unwrap();
            let decoded: Vec<ChangeEvent> = serde_json::from_str(&json).unwrap();
            assert_eq!(events, decoded);
        }

        #[test]
        fn change_event_batch_bincode_roundtrip() {
            let events = vec![
                ChangeEvent::insert(1, 100, 42, vec![1], 0),
                ChangeEvent::update(1, 101, 42, vec![1], vec![2], 0),
                ChangeEvent::delete(1, 102, 42, vec![2], 0),
                ChangeEvent::commit(1, 103, 0),
            ];
            let bytes = bincode::serialize(&events).unwrap();
            let decoded: Vec<ChangeEvent> = bincode::deserialize(&bytes).unwrap();
            assert_eq!(events, decoded);
        }

        // -----------------------------------------------------------------
        // 序列化保留所有字段（完整性测试）
        // -----------------------------------------------------------------

        #[test]
        fn change_event_json_preserves_all_fields() {
            let event = ChangeEvent::update(
                42,            // tx_id
                999,           // lsn
                100,           // table_id
                vec![1, 2, 3], // old_row
                vec![4, 5, 6], // new_row
                88888,         // timestamp
            );
            let json = event.to_json().unwrap();
            let decoded = ChangeEvent::from_json(&json).unwrap();
            assert_eq!(decoded.tx_id, 42);
            assert_eq!(decoded.lsn, 999);
            assert_eq!(decoded.op, CdcEventOp::Update);
            assert_eq!(decoded.table_id, Some(100));
            assert_eq!(decoded.old_row, Some(vec![1, 2, 3]));
            assert_eq!(decoded.new_row, Some(vec![4, 5, 6]));
            assert_eq!(decoded.timestamp, 88888);
        }
    }

    // =================================================================
    // Batch 3: CDC COMMIT 后分发 — 事务缓冲测试
    // =================================================================

    mod batch3_txn_buffer {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        /// 计数 observer：记录收到的事件数
        struct CountingObserver {
            count: AtomicUsize,
        }
        impl CountingObserver {
            fn new() -> Self {
                Self { count: AtomicUsize::new(0) }
            }
            fn count(&self) -> usize {
                self.count.load(Ordering::SeqCst)
            }
        }
        impl CdcObserver for CountingObserver {
            fn on_event(&self, _event: ChangeEvent) {
                self.count.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn make_engine() -> (CdcEngine, Arc<CountingObserver>) {
            let mgr = Arc::new(CdcObserverManager::new());
            let observer = Arc::new(CountingObserver::new());
            mgr.register(observer.clone());
            let engine = CdcEngine::new(mgr);
            (engine, observer)
        }

        #[test]
        fn stage_event_does_not_dispatch_immediately() {
            let (engine, observer) = make_engine();
            let event = ChangeEvent::insert(10, 1, 42, vec![1, 2], 100);
            engine.stage_event(10, event);
            // 事件已缓冲，observer 未收到
            assert_eq!(observer.count(), 0);
            assert_eq!(engine.staged_event_count(10), 1);
        }

        #[test]
        fn commit_txn_flushes_buffered_events() {
            let (engine, observer) = make_engine();
            engine.stage_event(10, ChangeEvent::insert(10, 1, 42, vec![1], 100));
            engine.stage_event(10, ChangeEvent::update(10, 2, 42, vec![1], vec![2], 101));
            engine.stage_event(10, ChangeEvent::delete(10, 3, 42, vec![2], 102));
            assert_eq!(observer.count(), 0);

            let dispatched = engine.commit_txn(10);
            assert_eq!(dispatched, 3);
            assert_eq!(observer.count(), 3);
            // 缓冲已清空
            assert_eq!(engine.staged_event_count(10), 0);
        }

        #[test]
        fn abort_txn_discards_buffered_events() {
            let (engine, observer) = make_engine();
            engine.stage_event(20, ChangeEvent::insert(20, 1, 42, vec![1], 100));
            engine.stage_event(20, ChangeEvent::insert(20, 2, 42, vec![2], 101));
            assert_eq!(observer.count(), 0);

            let discarded = engine.abort_txn(20);
            assert_eq!(discarded, 2);
            // observer 未收到任何事件
            assert_eq!(observer.count(), 0);
            assert_eq!(engine.staged_event_count(20), 0);
        }

        #[test]
        fn autocommit_dispatch_event_immediately() {
            let (engine, observer) = make_engine();
            let event = ChangeEvent::insert(1, 1, 42, vec![1], 100);
            engine.dispatch_event(event);
            // autocommit 模式立即分发
            assert_eq!(observer.count(), 1);
        }

        #[test]
        fn multiple_transactions_isolated() {
            let (engine, observer) = make_engine();
            // txn 10 和 txn 20 并行缓冲
            engine.stage_event(10, ChangeEvent::insert(10, 1, 42, vec![1], 100));
            engine.stage_event(20, ChangeEvent::insert(20, 2, 42, vec![2], 101));
            engine.stage_event(10, ChangeEvent::insert(10, 3, 42, vec![3], 102));

            // txn 20 回滚
            let discarded = engine.abort_txn(20);
            assert_eq!(discarded, 1);
            assert_eq!(observer.count(), 0);

            // txn 10 提交
            let dispatched = engine.commit_txn(10);
            assert_eq!(dispatched, 2);
            assert_eq!(observer.count(), 2);
        }

        #[test]
        fn commit_txn_no_buffer_returns_zero() {
            let (engine, _observer) = make_engine();
            assert_eq!(engine.commit_txn(999), 0);
        }

        #[test]
        fn abort_txn_no_buffer_returns_zero() {
            let (engine, _observer) = make_engine();
            assert_eq!(engine.abort_txn(999), 0);
        }
    }
}
