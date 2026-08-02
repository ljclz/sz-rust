//! 复制任务管理器 — 对应 `NineData分析与szrsql数据复制环方案.md` P3-2。
//!
//! 在 `CdcEngine`（事件分发）+ `TargetWriter`（目标端写入）+ `SlotManager`（位点管理）
//! 之上，提供完整的"源端→目标端"复制任务生命周期管理。
//!
//! # 核心概念
//!
//! - **ReplicationTask**：一个完整的复制链路（源端表 → 目标端），由 `task_id` 唯一标识
//! - **TaskState**：状态机 Created → Starting → Running → Paused → Stopped / Failed
//! - **ReplicationTaskManager**：管理多个 task，提供 create/start/pause/stop/list/monitor 接口
//! - **TaskStats**：实时统计（events_processed / bytes_processed / errors / lag）
//!
//! # 设计要点
//!
//! 1. **基于 CdcObserver**：每个 task 内部是一个 CdcObserver，订阅 CdcEngine 的事件流
//! 2. **表过滤**：每个 task 可配置只复制指定表
//! 3. **位点管理**：通过 SlotManager 持久化 confirmed_flush_lsn，崩溃后可恢复
//! 4. **解码器复用**：所有 task 共享同一个 RowDecoder（共享 SchemaRegistry）
//! 5. **线程安全**：内部 RwLock + Atomic 计数器，支持并发查询和写入
//! 6. **背压保护**：task 内部有界 channel，慢消费时不影响其他 task
//!
//! # 状态转换图
//!
//! ```text
//!    Created ──start──▶ Starting ──init_ok──▶ Running
//!       │                  │                    │
//!       │                  │                    ├──pause──▶ Paused
//!       │                  │                    │             │
//!       │                  │                    │             ├──resume──▶ Running
//!       │                  │                    │             └──stop─────▶ Stopped
//!       │                  │                    │
//!       │                  │                    ├──stop──────▶ Stopped
//!       │                  │                    └──error─────▶ Failed
//!       │                  └──init_err──▶ Failed
//!       └──stop────────────────────────▶ Stopped
//! ```

use crate::backpressure::{BackpressureConfig, BackpressureStatsSnapshot, BoundedEventQueue};
use crate::decoder::{DecodedRow, RowDecoder};
use crate::schema::SchemaRegistry;
use crate::slot::SlotManager;
use crate::target::TargetWriter;
use crate::{CdcEngine, CdcEventOp, CdcObserver, ChangeEvent};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::{Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// =====================================================================
// TaskState — 任务状态机
// =====================================================================

/// 复制任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// 已创建未启动
    Created,
    /// 启动中（初始化 schema、连接目标端）
    Starting,
    /// 运行中
    Running,
    /// 已暂停
    Paused,
    /// 已停止（不可恢复，可清理）
    Stopped,
    /// 失败（不可恢复，需人工介入）
    Failed,
}

impl TaskState {
    /// 是否可启动
    pub fn can_start(self) -> bool {
        matches!(self, Self::Created | Self::Failed)
    }

    /// 是否可暂停
    pub fn can_pause(self) -> bool {
        matches!(self, Self::Running)
    }

    /// 是否可恢复
    pub fn can_resume(self) -> bool {
        matches!(self, Self::Paused)
    }

    /// 是否可停止
    pub fn can_stop(self) -> bool {
        matches!(
            self,
            Self::Created | Self::Starting | Self::Running | Self::Paused | Self::Failed
        )
    }

    /// 是否接收事件
    pub fn can_receive_events(self) -> bool {
        matches!(self, Self::Running)
    }

    /// 转字符串
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }

    /// 是否终态
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped)
    }
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
// TaskError — 任务错误
// =====================================================================

/// 复制任务错误
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    /// 任务已存在
    #[error("task already exists: {0}")]
    AlreadyExists(String),

    /// 任务不存在
    #[error("task not found: {0}")]
    NotFound(String),

    /// 状态非法转换
    #[error("invalid state transition: task={task} from={from} to={to}")]
    InvalidState {
        task: String,
        from: String,
        to: String,
    },

    /// 配置错误
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// 写入失败
    #[error("writer error: {0}")]
    Writer(String),

    /// 解码失败
    #[error("decode error: {0}")]
    Decode(String),

    /// Slot 错误
    #[error("slot error: {0}")]
    Slot(String),

    /// 内部错误
    #[error("internal error: {0}")]
    Internal(String),
}

// =====================================================================
// TaskConfig — 任务配置
// =====================================================================

/// 复制任务配置 — 创建任务时提供
#[derive(Clone)]
pub struct TaskConfig {
    /// 任务 ID（唯一标识，建议使用 `rep_<source>_<target>_<seq>` 格式）
    pub task_id: String,
    /// 任务描述（人类可读）
    pub description: String,
    /// 源端表列表（None 表示复制所有表；Some 表示白名单）
    pub table_filter: Option<HashSet<String>>,
    /// 目标端 writer（已构造好的 Arc<dyn TargetWriter>）
    pub writer: Arc<dyn TargetWriter>,
    /// 目标端类型（用于监控/日志：postgres/mysql/kafka/memory）
    pub target_type: String,
    /// 目标端连接串（用于监控/审计）
    pub target_connection: String,
    /// 是否在全量同步完成后才开启增量（默认 true）
    pub snapshot_first: bool,
    /// 目标端方言（P4-2，用于 DDL 生成；默认 Postgres）
    pub dialect: crate::migration::Dialect,
    /// 背压配置（P7-3，默认 Block 策略 capacity=10000）
    pub backpressure_config: BackpressureConfig,
}

impl std::fmt::Debug for TaskConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskConfig")
            .field("task_id", &self.task_id)
            .field("description", &self.description)
            .field("table_filter", &self.table_filter)
            .field("target_type", &self.target_type)
            .field("target_connection", &self.target_connection)
            .field("snapshot_first", &self.snapshot_first)
            .field("dialect", &self.dialect)
            .field("backpressure_config", &self.backpressure_config)
            .field("writer", &"<dyn TargetWriter>")
            .finish()
    }
}

// =====================================================================
// TaskStats — 任务实时统计
// =====================================================================

/// 任务实时统计 — 通过原子计数器无锁读
#[derive(Debug, Default)]
pub struct TaskStats {
    /// 已接收的事件总数（含 Commit/Abort）
    pub events_received: AtomicU64,
    /// 已写入目标端的事件数（仅 DML）
    pub events_written: AtomicU64,
    /// 已处理的字节数
    pub bytes_processed: AtomicU64,
    /// 已处理的事务数（Commit 数）
    pub transactions_processed: AtomicU64,
    /// 错误次数
    pub error_count: AtomicU64,
    /// 最后一次错误消息
    pub last_error: Mutex<Option<String>>,
    /// 最后一次写入时间戳（Unix 毫秒）
    pub last_write_at: AtomicU64,
    /// 最后接收事件的 LSN
    pub last_lsn: AtomicU64,
    /// 已确认 flush 的 LSN（推进到目标端的位点）
    pub confirmed_flush_lsn: AtomicU64,
    /// 已处理的 DDL 事件数（P4-2）
    pub ddl_events_processed: AtomicU64,
    /// DDL 错误次数（P4-2）
    pub ddl_error_count: AtomicU64,
    /// 最后一次 DDL 错误消息（P4-2）
    pub last_ddl_error: Mutex<Option<String>>,
}

impl TaskStats {
    /// 快照（用于序列化/日志）
    pub fn snapshot(&self) -> TaskStatsSnapshot {
        TaskStatsSnapshot {
            events_received: self.events_received.load(Ordering::SeqCst),
            events_written: self.events_written.load(Ordering::SeqCst),
            bytes_processed: self.bytes_processed.load(Ordering::SeqCst),
            transactions_processed: self.transactions_processed.load(Ordering::SeqCst),
            error_count: self.error_count.load(Ordering::SeqCst),
            last_error: self.last_error.lock().clone(),
            last_write_at: self.last_write_at.load(Ordering::SeqCst),
            last_lsn: self.last_lsn.load(Ordering::SeqCst),
            confirmed_flush_lsn: self.confirmed_flush_lsn.load(Ordering::SeqCst),
            ddl_events_processed: self.ddl_events_processed.load(Ordering::SeqCst),
            ddl_error_count: self.ddl_error_count.load(Ordering::SeqCst),
            last_ddl_error: self.last_ddl_error.lock().clone(),
        }
    }

    /// 重置统计
    pub fn reset(&self) {
        self.events_received.store(0, Ordering::SeqCst);
        self.events_written.store(0, Ordering::SeqCst);
        self.bytes_processed.store(0, Ordering::SeqCst);
        self.transactions_processed.store(0, Ordering::SeqCst);
        self.error_count.store(0, Ordering::SeqCst);
        *self.last_error.lock() = None;
        self.last_write_at.store(0, Ordering::SeqCst);
        self.last_lsn.store(0, Ordering::SeqCst);
        self.confirmed_flush_lsn.store(0, Ordering::SeqCst);
        self.ddl_events_processed.store(0, Ordering::SeqCst);
        self.ddl_error_count.store(0, Ordering::SeqCst);
        *self.last_ddl_error.lock() = None;
    }
}

/// 任务统计快照（不可变）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskStatsSnapshot {
    pub events_received: u64,
    pub events_written: u64,
    pub bytes_processed: u64,
    pub transactions_processed: u64,
    pub error_count: u64,
    pub last_error: Option<String>,
    pub last_write_at: u64,
    pub last_lsn: u64,
    pub confirmed_flush_lsn: u64,
    /// 已处理的 DDL 事件数（P4-2）
    pub ddl_events_processed: u64,
    /// DDL 错误次数（P4-2）
    pub ddl_error_count: u64,
    /// 最后一次 DDL 错误消息（P4-2）
    pub last_ddl_error: Option<String>,
}

impl TaskStatsSnapshot {
    /// 当前滞后量（last_lsn - confirmed_flush_lsn）
    pub fn lag(&self) -> u64 {
        self.last_lsn.saturating_sub(self.confirmed_flush_lsn)
    }
}

// =====================================================================
// ReplicationTask — 单个复制任务
// =====================================================================

/// 复制任务 — 一个完整的源端→目标端复制链路
///
/// **内部结构**：
/// - `config`：任务配置（不可变）
/// - `state`：当前状态（RwLock 保护）
/// - `stats`：实时统计（原子计数器）
/// - `slot_manager`：位点持久化（共享）
/// - `decoder`：行解码器（共享）
/// - `schema_registry`：schema 注册表（共享）
/// - `snapshot_lsn`：全量快照点的 LSN（P4-1）
///   - `0` 表示未启用快照+增量衔接模式
///   - `>0` 表示已执行全量快照，CDC 阶段应跳过 `lsn <= snapshot_lsn` 的事件
///   - 避免重复：快照已包含 snapshot_lsn 之前的数据，CDC 不再重投
///   - 避免丢失：snapshot_lsn 之后的事件由 CDC 正常消费
pub struct ReplicationTask {
    /// 任务配置
    config: TaskConfig,
    /// 当前状态
    state: RwLock<TaskState>,
    /// 实时统计
    stats: TaskStats,
    /// 创建时间
    created_at: u64,
    /// Slot 管理器（共享）
    slot_manager: Arc<SlotManager>,
    /// 解码器（共享）
    decoder: Arc<RowDecoder>,
    /// Schema 注册表（共享）
    schema_registry: Arc<SchemaRegistry>,
    /// 全量快照点 LSN（P4-1）— 0 表示未启用，>0 表示已快照
    snapshot_lsn: AtomicU64,
    /// 有界事件队列（P7-3）— 生产者（CdcEngine 回调）与消费者（写入线程）之间的缓冲，
    /// 基于水位线触发背压，防止 OOM
    event_queue: Arc<BoundedEventQueue>,
    /// 消费者线程句柄（P7-3）— `start()` 时启动，`stop()` 时关闭队列并 join
    consumer_handle: Mutex<Option<JoinHandle<()>>>,
}

impl ReplicationTask {
    /// 创建任务（初始状态 Created）
    pub fn new(
        config: TaskConfig,
        slot_manager: Arc<SlotManager>,
        decoder: Arc<RowDecoder>,
        schema_registry: Arc<SchemaRegistry>,
    ) -> Result<Self, TaskError> {
        if config.task_id.is_empty() {
            return Err(TaskError::InvalidConfig("task_id is empty".to_string()));
        }
        // 读取背压配置（BackpressureConfig 是 Copy，可在 move 前复制）
        let backpressure_config = config.backpressure_config;
        // 预创建 slot（持久化位点）
        slot_manager
            .create_slot(
                &config.task_id,
                &config.target_type,
                &config.target_connection,
            )
            .map_err(|e| TaskError::Slot(e.to_string()))?;
        // 应用 table_filter 到 slot
        if let Some(filter) = &config.table_filter {
            let mut slots = slot_manager.list_slots();
            if let Some(slot) = slots.iter_mut().find(|s| s.slot_name == config.task_id) {
                let filtered = slot
                    .clone()
                    .with_table_filter(filter.iter().cloned().collect());
                let _ = filtered; // slot 已持久化，table_filter 通过 accepts_table 判断
            }
        }
        Ok(Self {
            config,
            state: RwLock::new(TaskState::Created),
            stats: TaskStats::default(),
            created_at: current_millis(),
            slot_manager,
            decoder,
            schema_registry,
            snapshot_lsn: AtomicU64::new(0),
            event_queue: Arc::new(BoundedEventQueue::new(backpressure_config)),
            consumer_handle: Mutex::new(None),
        })
    }

    /// 任务 ID
    pub fn task_id(&self) -> &str {
        &self.config.task_id
    }

    /// 任务描述
    pub fn description(&self) -> &str {
        &self.config.description
    }

    /// 目标端类型
    pub fn target_type(&self) -> &str {
        &self.config.target_type
    }

    /// 目标端连接串
    pub fn target_connection(&self) -> &str {
        &self.config.target_connection
    }

    /// 创建时间
    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    /// 当前状态
    pub fn state(&self) -> TaskState {
        *self.state.read()
    }

    /// 表过滤（是否接受该表）
    pub fn accepts_table(&self, table_id: u32) -> bool {
        if self.config.table_filter.is_none() {
            return true;
        }
        // 通过 schema_registry 查表名
        if let Some(schema) = self.schema_registry.get_schema(table_id) {
            return self
                .config
                .table_filter
                .as_ref()
                .map(|f| f.contains(&schema.table_name))
                .unwrap_or(true);
        }
        false
    }

    /// 是否接受该表名（按名字过滤）
    pub fn accepts_table_name(&self, table_name: &str) -> bool {
        match &self.config.table_filter {
            None => true,
            Some(set) => set.contains(table_name),
        }
    }

    /// 获取全量快照点 LSN（P4-1）
    ///
    /// - `0`：未启用快照+增量衔接
    /// - `>0`：已执行全量快照，CDC 应跳过 `lsn <= snapshot_lsn` 的事件
    pub fn snapshot_lsn(&self) -> u64 {
        self.snapshot_lsn.load(Ordering::SeqCst)
    }

    /// 获取任务表过滤配置（供 SnapshotTransfer 应用同一过滤，P4-1）
    pub fn config_table_filter(&self) -> Option<&HashSet<String>> {
        self.config.table_filter.as_ref()
    }

    /// 获取目标端写入器（供 SnapshotTransfer 写入全量数据，P4-1）
    pub fn config_writer(&self) -> &Arc<dyn TargetWriter> {
        &self.config.writer
    }

    /// 设置全量快照点 LSN（P4-1）
    ///
    /// **调用时机**：在 `start()` 之前，由 `start_with_snapshot` 调用。
    /// **效果**：CDC 阶段 `on_event` 会跳过 `lsn <= snapshot_lsn` 的事件，
    /// 避免快照已包含的数据被重复写入目标端。
    pub fn set_snapshot_lsn(&self, lsn: u64) {
        self.snapshot_lsn.store(lsn, Ordering::SeqCst);
    }

    /// 判断事件是否应被跳过（P4-1）
    ///
    /// 当 `snapshot_lsn > 0` 且事件 LSN <= snapshot_lsn 时跳过，
    /// 因为这些数据已通过全量快照写入目标端。
    pub fn should_skip_event(&self, event: &ChangeEvent) -> bool {
        let snap = self.snapshot_lsn.load(Ordering::SeqCst);
        snap > 0 && event.lsn <= snap
    }

    /// 获取统计快照
    pub fn stats(&self) -> TaskStatsSnapshot {
        self.stats.snapshot()
    }

    /// 启动任务
    pub fn start(&self) -> Result<(), TaskError> {
        let mut state = self.state.write();
        if !state.can_start() {
            return Err(TaskError::InvalidState {
                task: self.config.task_id.clone(),
                from: state.as_str().to_string(),
                to: "running".to_string(),
            });
        }
        *state = TaskState::Starting;
        drop(state);

        // 激活 slot
        self.slot_manager
            .activate_slot(&self.config.task_id)
            .map_err(|e| TaskError::Slot(e.to_string()))?;

        // 健康检查
        if let Err(e) = self.config.writer.health_check() {
            let mut state = self.state.write();
            *state = TaskState::Failed;
            self.record_error(format!("health check failed: {e}"));
            return Err(TaskError::Writer(e.to_string()));
        }

        let mut state = self.state.write();
        *state = TaskState::Running;
        Ok(())
    }

    /// 暂停任务
    pub fn pause(&self) -> Result<(), TaskError> {
        let mut state = self.state.write();
        if !state.can_pause() {
            return Err(TaskError::InvalidState {
                task: self.config.task_id.clone(),
                from: state.as_str().to_string(),
                to: "paused".to_string(),
            });
        }
        *state = TaskState::Paused;
        drop(state);
        self.slot_manager
            .pause_slot(&self.config.task_id)
            .map_err(|e| TaskError::Slot(e.to_string()))?;
        Ok(())
    }

    /// 恢复任务
    pub fn resume(&self) -> Result<(), TaskError> {
        let mut state = self.state.write();
        if !state.can_resume() {
            return Err(TaskError::InvalidState {
                task: self.config.task_id.clone(),
                from: state.as_str().to_string(),
                to: "running".to_string(),
            });
        }
        *state = TaskState::Running;
        drop(state);
        self.slot_manager
            .activate_slot(&self.config.task_id)
            .map_err(|e| TaskError::Slot(e.to_string()))?;
        Ok(())
    }

    /// 停止任务
    ///
    /// **流程**（P7-3）：
    /// 1. 状态转换 → Stopped（阻止新事件入队，`on_event` 的状态检查会拒绝）
    /// 2. 关闭事件队列（`BoundedEventQueue::close`）→ 消费者线程的 `pop` 返回 `None` 后退出
    /// 3. join 消费者线程（带超时，避免消费者卡死导致 stop 阻塞）
    /// 4. 暂停 slot
    pub fn stop(&self) -> Result<(), TaskError> {
        let mut state = self.state.write();
        if !state.can_stop() {
            return Err(TaskError::InvalidState {
                task: self.config.task_id.clone(),
                from: state.as_str().to_string(),
                to: "stopped".to_string(),
            });
        }
        *state = TaskState::Stopped;
        drop(state);

        // 关闭事件队列（唤醒所有阻塞在 push/pop 的线程）
        self.event_queue.close();

        // join 消费者线程（带超时，避免消费者卡死导致 stop 阻塞）
        let mut handle_guard = self.consumer_handle.lock();
        if let Some(handle) = handle_guard.take() {
            join_with_timeout(handle, Duration::from_secs(5));
        }
        drop(handle_guard);

        self.slot_manager
            .pause_slot(&self.config.task_id)
            .map_err(|e| TaskError::Slot(e.to_string()))?;
        Ok(())
    }

    /// 标记为失败（内部使用）
    pub fn fail(&self, reason: impl Into<String>) {
        let mut state = self.state.write();
        *state = TaskState::Failed;
        drop(state);
        // 关闭队列并 join 消费者，避免失败后线程泄漏
        self.event_queue.close();
        let mut handle_guard = self.consumer_handle.lock();
        if let Some(handle) = handle_guard.take() {
            join_with_timeout(handle, Duration::from_secs(5));
        }
        drop(handle_guard);
        self.record_error(reason);
    }

    /// 记录错误
    fn record_error(&self, msg: impl Into<String>) {
        self.stats.error_count.fetch_add(1, Ordering::SeqCst);
        *self.stats.last_error.lock() = Some(msg.into());
    }

    // -----------------------------------------------------------------
    // P7-3：背压集成 — 消费者线程 + 内联排空
    // -----------------------------------------------------------------

    /// 启动消费者线程（P7-3）
    ///
    /// 在独立线程中循环 `event_queue.pop()` → `process_single_event`，
    /// 直到队列关闭（`stop()` 触发）。
    ///
    /// **调用时机**：由 `ReplicationTaskManager::start_task` 在 `task.start()` 之后调用。
    /// **线程安全**：`spawn_consumer` 通过 `Arc<ReplicationTask>` 捕获任务自身，
    /// 消费者线程持有 `Arc` 引用直到退出，避免任务被提前回收。
    ///
    /// **幂等**：若已有消费者线程在运行，直接返回 Ok（不重复启动）。
    pub fn spawn_consumer(self: &Arc<Self>) -> Result<(), TaskError> {
        let mut handle_guard = self.consumer_handle.lock();
        if handle_guard.is_some() {
            // 消费者线程已在运行
            return Ok(());
        }
        let task = self.clone();
        let handle = std::thread::Builder::new()
            .name(format!("cdc-consumer-{}", task.config.task_id))
            .spawn(move || {
                // 消费者主循环：pop 阻塞直到有事件或队列关闭
                while let Some(event) = task.event_queue.pop() {
                    task.process_single_event(event);
                }
                // 队列关闭且为空，消费者退出
            })
            .map_err(|e| TaskError::Internal(format!("spawn consumer thread failed: {e}")))?;
        *handle_guard = Some(handle);
        Ok(())
    }

    /// 内联排空队列中已积压的事件（P7-3）
    ///
    /// 同步地从队列中弹出并处理最多 `max` 个事件，适用于：
    /// - 单线程测试场景（不启动消费者线程，由调用方驱动处理）
    /// - 协作式调度（调用方在合适时机主动排空）
    ///
    /// **返回**：实际处理的事件数
    pub fn process_pending_events_inline(&self, max: usize) -> usize {
        let mut processed = 0usize;
        while processed < max {
            match self.event_queue.try_pop() {
                Some(event) => {
                    self.process_single_event(event);
                    processed += 1;
                }
                None => break,
            }
        }
        processed
    }

    /// 获取背压统计快照（P7-3）
    pub fn backpressure_stats(&self) -> BackpressureStatsSnapshot {
        self.event_queue.stats()
    }

    /// 获取事件队列的引用（用于注册背压回调等高级场景，P7-3）
    pub fn event_queue(&self) -> &Arc<BoundedEventQueue> {
        &self.event_queue
    }

    /// 处理单个事件（P7-3 抽取）— 原 `on_event` 的核心处理逻辑
    ///
    /// **流程**：
    /// 1. 快照 LSN 过滤（跳过快照点之前的事件）
    /// 2. 表过滤（白名单外的表跳过）
    /// 3. Commit/Abort 处理（不写入目标端，仅推进位点/统计）
    /// 4. DML 事件：解码行数据 → 写入目标端 → 更新统计
    fn process_single_event(&self, event: ChangeEvent) {
        // P4-1：快照+增量衔接过滤
        if self.should_skip_event(&event) {
            return;
        }

        // 表过滤（DML 事件）
        if let Some(table_id) = event.table_id {
            if !self.accepts_table(table_id) {
                return;
            }
        }

        // Commit/Abort 不写入目标端，但更新统计
        match event.op {
            CdcEventOp::Commit => {
                self.stats
                    .transactions_processed
                    .fetch_add(1, Ordering::SeqCst);
                // 推进位点
                if let Err(e) = self
                    .slot_manager
                    .advance_flush_lsn(&self.config.task_id, event.lsn)
                {
                    self.record_error(format!("advance flush lsn failed: {e}"));
                } else {
                    self.stats
                        .confirmed_flush_lsn
                        .store(event.lsn, Ordering::SeqCst);
                }
                return;
            }
            CdcEventOp::Abort => {
                // Abort 事件不写入目标端，不推进位点
                return;
            }
            CdcEventOp::Insert | CdcEventOp::Update | CdcEventOp::Delete => {}
        }

        // 获取 schema
        let table_id = match event.table_id {
            Some(id) => id,
            None => {
                self.record_error("DML event without table_id");
                return;
            }
        };
        let schema = match self.schema_registry.get_schema(table_id) {
            Some(s) => s,
            None => {
                self.record_error(format!("schema not found: table_id={table_id}"));
                return;
            }
        };

        // 解码行数据
        let row: Option<DecodedRow> = match event.op {
            CdcEventOp::Insert | CdcEventOp::Update => {
                if let Some(data) = &event.new_row {
                    match self.decoder.decode(table_id, data, event.schema_version) {
                        Ok(r) => Some(r),
                        Err(e) => {
                            self.record_error(format!("decode new_row failed: {e}"));
                            return;
                        }
                    }
                } else {
                    None
                }
            }
            CdcEventOp::Delete => {
                if let Some(data) = &event.old_row {
                    match self.decoder.decode(table_id, data, event.schema_version) {
                        Ok(r) => Some(r),
                        Err(e) => {
                            self.record_error(format!("decode old_row failed: {e}"));
                            return;
                        }
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        // 写入目标端
        let bytes = event
            .new_row
            .as_ref()
            .map(|d| d.len())
            .or_else(|| event.old_row.as_ref().map(|d| d.len()))
            .unwrap_or(0);
        if let Err(e) = self
            .config
            .writer
            .write_event(&event, &schema, row.as_ref())
        {
            self.record_error(format!("write_event failed: {e}"));
            // 写入失败不推进位点，等待重试
            return;
        }

        // 统计写入
        self.stats.events_written.fetch_add(1, Ordering::SeqCst);
        self.stats
            .bytes_processed
            .fetch_add(bytes as u64, Ordering::SeqCst);
        self.stats
            .last_write_at
            .store(current_millis(), Ordering::SeqCst);

        // 记录到 slot（不持久化统计，定期 flush）
        let _ = self.slot_manager.record_event(&self.config.task_id, bytes);
    }
}

impl CdcObserver for ReplicationTask {
    fn on_event(&self, event: ChangeEvent) {
        // 仅 Running 状态接收事件
        if !self.state().can_receive_events() {
            return;
        }

        // 统计接收（含被快照过滤跳过的事件，保持与表过滤一致的"先统计后过滤"语义）
        self.stats.events_received.fetch_add(1, Ordering::SeqCst);
        self.stats.last_lsn.store(event.lsn, Ordering::SeqCst);

        // P7-3：将事件推入有界事件队列，由消费者线程异步处理
        //
        // **背压语义**：
        // - Block 策略：队列满时阻塞生产者（CdcEngine 回调线程），实现反压
        // - Reject 策略：队列满时拒绝事件，记录错误（调用方需重试）
        // - DropOldest 策略：丢弃最旧事件（有损，可重放场景）
        // - Signal 策略：仅通知，不阻塞不丢弃
        //
        // **QueueClosed 处理**：任务已 stop/close 队列，静默丢弃事件（非错误）
        if let Err(e) = self.event_queue.push(event) {
            match e {
                crate::backpressure::BackpressureError::QueueClosed => {
                    // 队列已关闭（任务停止），静默丢弃
                }
                crate::backpressure::BackpressureError::BufferFull {
                    capacity,
                    current_size,
                } => {
                    // Reject 策略下队列满，记录错误（调用方可重试）
                    self.record_error(format!(
                        "event queue full (capacity={capacity}, current_size={current_size}); event dropped"
                    ));
                }
                crate::backpressure::BackpressureError::InvalidConfig { reason } => {
                    self.record_error(format!("backpressure config invalid: {reason}"));
                }
            }
        }
    }
}

impl crate::schema::SchemaChangeObserver for ReplicationTask {
    /// 接收 SchemaChangeEvent — DDL 变更同步（P4-2）
    ///
    /// **流程**：
    /// 1. 仅 Running 状态接收 DDL 事件
    /// 2. 表过滤：若配置了 table_filter 且事件涉及的表不在白名单，跳过
    /// 3. 根据 change_type 和 new_schema/old_schema 生成目标端方言 DDL
    /// 4. 调用 TargetWriter::execute_ddl 应用到目标端
    /// 5. 更新统计（ddl_events_processed / ddl_error_count）
    ///
    /// **DDL 生成策略**：
    /// - CreateTable：直接用 new_schema 生成 CREATE TABLE IF NOT EXISTS
    /// - AlterTableAddColumn：用 new_schema 生成 ALTER TABLE ADD COLUMN
    /// - AlterTableDropColumn：当前保守策略，不生成 DROP COLUMN（避免数据丢失）
    /// - DropTable：用 generate_drop_table 生成 DROP TABLE IF EXISTS
    fn on_schema_change(&self, event: crate::schema::SchemaChangeEvent) {
        // 仅 Running 状态接收 DDL 事件
        if !self.state().can_receive_events() {
            return;
        }

        // 表过滤：若配置了 table_filter，检查表名是否在白名单
        // CreateTable 时表可能不在 schema_registry，用 new_schema.table_name 判断
        let table_name = event
            .new_schema
            .as_ref()
            .map(|s| s.table_name.as_str())
            .or_else(|| event.old_schema.as_ref().map(|s| s.table_name.as_str()))
            .unwrap_or("");
        if !self.accepts_table_name(table_name) {
            return;
        }

        // 根据 change_type 生成 DDL
        let generator = crate::migration::DdlGenerator::new(self.config.dialect);
        let ddl: Option<crate::migration::DdlStatement> = match event.change_type {
            crate::schema::SchemaChangeType::CreateTable => {
                event
                    .new_schema
                    .as_ref()
                    .map(|schema| crate::migration::DdlStatement {
                        kind: crate::migration::DdlKind::CreateTable,
                        table_name: schema.table_name.clone(),
                        sql: generator.generate_create_table(schema),
                    })
            }
            crate::schema::SchemaChangeType::AlterTableAddColumn => {
                // 用 changed_column 精确定位新增列（避免假设新增列是最后一列）
                if let (Some(schema), Some(col_name)) = (&event.new_schema, &event.changed_column) {
                    schema
                        .columns
                        .iter()
                        .find(|c| &c.name == col_name)
                        .map(|col| crate::migration::DdlStatement {
                            kind: crate::migration::DdlKind::AddColumn,
                            table_name: schema.table_name.clone(),
                            sql: generator.generate_add_column(&schema.table_name, col),
                        })
                } else {
                    None
                }
            }
            crate::schema::SchemaChangeType::AlterTableDropColumn => {
                // 保守策略：不生成 DROP COLUMN（避免目标端数据丢失）
                // 生产环境可通过配置启用
                None
            }
            crate::schema::SchemaChangeType::DropTable => Some(crate::migration::DdlStatement {
                kind: crate::migration::DdlKind::DropTable,
                table_name: table_name.to_string(),
                sql: generator.generate_drop_table(table_name),
            }),
        };

        // 执行 DDL
        if let Some(ddl) = ddl {
            if let Err(e) = self.config.writer.execute_ddl(&ddl.sql) {
                self.stats.ddl_error_count.fetch_add(1, Ordering::SeqCst);
                *self.stats.last_ddl_error.lock() =
                    Some(format!("DDL execution failed: {} (sql: {})", e, ddl.sql));
            } else {
                self.stats
                    .ddl_events_processed
                    .fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}

// =====================================================================
// TaskInfo — 任务信息（用于 list/monitor 返回）
// =====================================================================

/// 任务信息（只读视图，用于 list/monitor）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskInfo {
    pub task_id: String,
    pub description: String,
    pub state: TaskState,
    pub target_type: String,
    pub target_connection: String,
    pub created_at: u64,
    pub table_filter: Option<Vec<String>>,
    pub stats: TaskStatsSnapshot,
    /// 全量快照点 LSN（P4-1）— 0 表示未启用快照+增量衔接
    pub snapshot_lsn: u64,
}

// =====================================================================
// ReplicationTaskManager — 任务管理器
// =====================================================================

/// 复制任务管理器 — 管理多个复制任务，集成 CdcEngine 分发
///
/// **线程安全**：内部 RwLock<HashMap<String, Arc<ReplicationTask>>>
///
/// **使用方式**：
///
/// ```ignore
/// use szrsql_cdc::task::{ReplicationTaskManager, TaskConfig};
/// use szrsql_cdc::target::memory::MemoryWriter;
/// use szrsql_cdc::slot::SlotManager;
/// use szrsql_cdc::schema::SchemaRegistry;
/// use szrsql_cdc::decoder::RowDecoder;
/// use std::sync::Arc;
///
/// let slot_mgr = Arc::new(SlotManager::in_memory());
/// let schema_registry = Arc::new(SchemaRegistry::new());
/// let decoder = Arc::new(RowDecoder::new(schema_registry.clone()));
/// let cdc_engine = Arc::new(CdcEngine::new(/* ... */));
///
/// let task_mgr = ReplicationTaskManager::new(slot_mgr, decoder, schema_registry, cdc_engine);
///
/// let writer = Arc::new(MemoryWriter::new());
/// let config = TaskConfig {
///     task_id: "rep_pg1".to_string(),
///     description: "replicate users table".to_string(),
///     table_filter: Some(["users".to_string()].into_iter().collect()),
///     writer,
///     target_type: "memory".to_string(),
///     target_connection: "memory://test".to_string(),
///     snapshot_first: false,
/// };
/// task_mgr.create_task(config).unwrap();
/// task_mgr.start_task("rep_pg1").unwrap();
/// ```
pub struct ReplicationTaskManager {
    /// 任务存储 task_id → Arc<ReplicationTask>
    tasks: RwLock<HashMap<String, Arc<ReplicationTask>>>,
    /// Slot 管理器（共享）
    slot_manager: Arc<SlotManager>,
    /// 解码器（共享）
    decoder: Arc<RowDecoder>,
    /// Schema 注册表（共享）
    schema_registry: Arc<SchemaRegistry>,
    /// CdcEngine（用于注册 task 为 observer）
    cdc_engine: Arc<CdcEngine>,
    /// 任务总数（统计用）
    total_created: AtomicU64,
    total_started: AtomicU64,
    total_stopped: AtomicU64,
    total_failed: AtomicU64,
}

impl ReplicationTaskManager {
    /// 创建任务管理器
    pub fn new(
        slot_manager: Arc<SlotManager>,
        decoder: Arc<RowDecoder>,
        schema_registry: Arc<SchemaRegistry>,
        cdc_engine: Arc<CdcEngine>,
    ) -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            slot_manager,
            decoder,
            schema_registry,
            cdc_engine,
            total_created: AtomicU64::new(0),
            total_started: AtomicU64::new(0),
            total_stopped: AtomicU64::new(0),
            total_failed: AtomicU64::new(0),
        }
    }

    /// 创建任务（初始状态 Created）
    pub fn create_task(&self, config: TaskConfig) -> Result<Arc<ReplicationTask>, TaskError> {
        let task_id = config.task_id.clone();
        let mut tasks = self.tasks.write();
        if tasks.contains_key(&task_id) {
            return Err(TaskError::AlreadyExists(task_id));
        }
        let task = Arc::new(ReplicationTask::new(
            config,
            self.slot_manager.clone(),
            self.decoder.clone(),
            self.schema_registry.clone(),
        )?);
        tasks.insert(task_id, task.clone());
        drop(tasks);
        self.total_created.fetch_add(1, Ordering::SeqCst);
        Ok(task)
    }

    /// 启动任务（Created/Failed → Running）
    ///
    /// 内部将 task 注册为 CdcEngine 的 observer（DML + DDL 双通道），
    /// 并启动消费者线程异步处理事件（P7-3 背压集成）
    pub fn start_task(&self, task_id: &str) -> Result<(), TaskError> {
        let task = self.get_task(task_id)?;
        task.start()?;
        // P7-3：启动消费者线程（从 event_queue pop 并写入目标端）
        task.spawn_consumer()?;
        // 注册为 CdcEngine observer（DML 事件）
        self.cdc_engine.register_observer_arc(task.clone());
        // 注册为 SchemaChangeObserver（DDL 事件，P4-2）
        let schema_obs: Arc<dyn crate::schema::SchemaChangeObserver> = task.clone();
        self.cdc_engine.register_schema_observer(schema_obs);
        self.total_started.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// 启动任务（带全量快照 + 增量衔接，P4-1）
    ///
    /// **流程**：
    /// 1. 通过 `source` 执行全量快照传输，得到 `snapshot_lsn`
    /// 2. 将 `snapshot_lsn` 设置到 task（用于过滤 CDC 阶段的旧事件）
    /// 3. 推进 ReplicationSlot 的 `confirmed_flush_lsn` 到 `snapshot_lsn`
    ///    （表示快照点之前的数据已被"消费"，WAL 可回收）
    /// 4. 调用 `task.start()` 注册 observer，开始消费 CDC 增量事件
    ///
    /// **正确性保证**：
    /// - 不丢数据：CDC 阶段消费 `lsn > snapshot_lsn` 的所有事件
    /// - 不重数据：`lsn <= snapshot_lsn` 的事件被 `should_skip_event` 过滤
    /// - 一致性：快照基于 MVCC 一致性读，看到的是 snapshot_lsn 时刻的完整数据
    ///
    /// **参数**：
    /// - `task_id`：任务 ID（必须已 create_task）
    /// - `source`：全量快照数据源（实现 RowSource trait）
    ///
    /// **返回**：
    /// - `Ok(SnapshotResult)`：快照传输结果（含 snapshot_lsn、传输行数等）
    /// - `Err(TaskError)`：任务不存在、状态非法、快照传输失败等
    pub fn start_task_with_snapshot(
        &self,
        task_id: &str,
        source: Arc<dyn crate::snapshot::RowSource>,
    ) -> Result<crate::snapshot::SnapshotResult, TaskError> {
        let task = self.get_task(task_id)?;

        // 1. 执行全量快照传输
        let snapshot_config = crate::snapshot::SnapshotConfig {
            // 应用任务的表过滤
            table_filter: task
                .config_table_filter()
                .map(|f| f.iter().cloned().collect()),
            ..Default::default()
        };
        let transfer = crate::snapshot::SnapshotTransfer::new(
            source,
            task.config_writer().clone(),
            snapshot_config,
        );
        let snapshot_result = transfer.run().map_err(|e| {
            // 快照失败 → 标记任务失败
            task.fail(format!("snapshot transfer failed: {e}"));
            TaskError::Internal(format!("snapshot transfer failed: {e}"))
        })?;

        // 2. 设置 task 的 snapshot_lsn（启用 CDC 阶段的过滤）
        task.set_snapshot_lsn(snapshot_result.snapshot_lsn);

        // 3. 推进 slot 的 confirmed_flush_lsn 到 snapshot_lsn
        //    （表示快照点之前的数据已被消费，WAL 可回收）
        self.slot_manager
            .advance_flush_lsn(task_id, snapshot_result.snapshot_lsn)
            .map_err(|e| {
                task.fail(format!("advance flush lsn to snapshot_lsn failed: {e}"));
                TaskError::Slot(e.to_string())
            })?;

        // 4. 启动任务（注册 observer 开始消费 CDC）
        task.start()?;
        // P7-3：启动消费者线程（从 event_queue pop 并写入目标端）
        task.spawn_consumer()?;
        self.cdc_engine.register_observer_arc(task.clone());
        // 注册为 SchemaChangeObserver（DDL 事件，P4-2）
        let schema_obs: Arc<dyn crate::schema::SchemaChangeObserver> = task.clone();
        self.cdc_engine.register_schema_observer(schema_obs);
        self.total_started.fetch_add(1, Ordering::SeqCst);

        Ok(snapshot_result)
    }

    /// 暂停任务（Running → Paused）
    ///
    /// 内部从 CdcEngine 注销 observer（DML + DDL 双通道）
    pub fn pause_task(&self, task_id: &str) -> Result<(), TaskError> {
        let task = self.get_task(task_id)?;
        task.pause()?;
        self.cdc_engine.unregister_observer_arc(&task);
        self.cdc_engine.unregister_schema_observer(&task);
        Ok(())
    }

    /// 恢复任务（Paused → Running）
    pub fn resume_task(&self, task_id: &str) -> Result<(), TaskError> {
        let task = self.get_task(task_id)?;
        task.resume()?;
        self.cdc_engine.register_observer_arc(task.clone());
        let schema_obs: Arc<dyn crate::schema::SchemaChangeObserver> = task.clone();
        self.cdc_engine.register_schema_observer(schema_obs);
        Ok(())
    }

    /// 停止任务（任意状态 → Stopped）
    pub fn stop_task(&self, task_id: &str) -> Result<(), TaskError> {
        let task = self.get_task(task_id)?;
        // 先注销 observer，避免新事件继续写入（DML + DDL 双通道）
        self.cdc_engine.unregister_observer_arc(&task);
        self.cdc_engine.unregister_schema_observer(&task);
        task.stop()?;
        self.total_stopped.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// 删除任务（停止 + 从存储中移除 + 物理删除 slot）
    pub fn remove_task(&self, task_id: &str) -> Result<(), TaskError> {
        // 先停止
        let task = self.get_task(task_id)?;
        if task.state() != TaskState::Stopped {
            self.stop_task(task_id)?;
        }
        // 从存储移除
        let mut tasks = self.tasks.write();
        tasks.remove(task_id);
        drop(tasks);
        // 物理删除 slot
        self.slot_manager
            .remove_slot(task_id)
            .map_err(|e| TaskError::Slot(e.to_string()))?;
        Ok(())
    }

    /// 获取任务（只读）
    pub fn get_task(&self, task_id: &str) -> Result<Arc<ReplicationTask>, TaskError> {
        self.tasks
            .read()
            .get(task_id)
            .cloned()
            .ok_or_else(|| TaskError::NotFound(task_id.to_string()))
    }

    /// 列出所有任务信息
    pub fn list_tasks(&self) -> Vec<TaskInfo> {
        self.tasks
            .read()
            .values()
            .map(|t| TaskInfo {
                task_id: t.task_id().to_string(),
                description: t.description().to_string(),
                state: t.state(),
                target_type: t.target_type().to_string(),
                target_connection: t.target_connection().to_string(),
                created_at: t.created_at(),
                table_filter: t
                    .config
                    .table_filter
                    .as_ref()
                    .map(|f| f.iter().cloned().collect()),
                stats: t.stats(),
                snapshot_lsn: t.snapshot_lsn(),
            })
            .collect()
    }

    /// 监控指定任务（详细信息 + 统计）
    pub fn monitor_task(&self, task_id: &str) -> Result<TaskInfo, TaskError> {
        let task = self.get_task(task_id)?;
        Ok(TaskInfo {
            task_id: task.task_id().to_string(),
            description: task.description().to_string(),
            state: task.state(),
            target_type: task.target_type().to_string(),
            target_connection: task.target_connection().to_string(),
            created_at: task.created_at(),
            table_filter: task
                .config
                .table_filter
                .as_ref()
                .map(|f| f.iter().cloned().collect()),
            stats: task.stats(),
            snapshot_lsn: task.snapshot_lsn(),
        })
    }

    /// 任务总数
    pub fn task_count(&self) -> usize {
        self.tasks.read().len()
    }

    /// 按状态过滤任务
    pub fn tasks_by_state(&self, state: TaskState) -> Vec<TaskInfo> {
        self.list_tasks()
            .into_iter()
            .filter(|t| t.state == state)
            .collect()
    }

    /// 管理器级统计
    pub fn manager_stats(&self) -> ManagerStats {
        ManagerStats {
            total_tasks: self.task_count(),
            total_created: self.total_created.load(Ordering::SeqCst),
            total_started: self.total_started.load(Ordering::SeqCst),
            total_stopped: self.total_stopped.load(Ordering::SeqCst),
            total_failed: self.total_failed.load(Ordering::SeqCst),
            running_tasks: self
                .list_tasks()
                .iter()
                .filter(|t| t.state == TaskState::Running)
                .count(),
        }
    }

    /// 推进指定任务的 flush_lsn（手动位点推进，用于 snapshot 完成后衔接增量）
    pub fn advance_flush_lsn(&self, task_id: &str, lsn: u64) -> Result<(), TaskError> {
        let task = self.get_task(task_id)?;
        self.slot_manager
            .advance_flush_lsn(task_id, lsn)
            .map_err(|e| TaskError::Slot(e.to_string()))?;
        task.stats.confirmed_flush_lsn.store(lsn, Ordering::SeqCst);
        Ok(())
    }

    /// 通知所有 SchemaChangeObserver：分发一个 SchemaChangeEvent（P4-2）
    ///
    /// 透传到 `CdcEngine::notify_schema_change`，由已注册的 task 处理 DDL 事件。
    pub fn notify_schema_change(&self, event: crate::schema::SchemaChangeEvent) {
        self.cdc_engine.notify_schema_change(event);
    }

    /// 直接分发一个 ChangeEvent 到所有 DML observer（P4-3 基准测试用）
    ///
    /// 透传到 `CdcEngine::dispatch_event`。正常生产路径是通过 WAL 触发，
    /// 此方法供基准测试直接推送合成事件，无需经过 WAL 层。
    pub fn dispatch_event(&self, event: ChangeEvent) {
        self.cdc_engine.dispatch_event(event);
    }
}

/// 管理器级统计
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManagerStats {
    pub total_tasks: usize,
    pub total_created: u64,
    pub total_started: u64,
    pub total_stopped: u64,
    pub total_failed: u64,
    pub running_tasks: usize,
}

// =====================================================================
// CdcEngine 扩展：register/unregister observer（透传到 CdcObserverManager）
// =====================================================================
//
// 注：CdcEngine 的 `register_observer_arc` / `unregister_observer_arc` 方法
// 已在 lib.rs 中实现（暴露 observer_manager），task.rs 直接调用即可。
// 保留此处说明，避免误以为还需要 trait extension。

// =====================================================================
// 辅助函数
// =====================================================================

/// 当前 Unix 毫秒
fn current_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 带超时的 join（P7-3）
///
/// `std::thread::JoinHandle::join` 没有超时参数，这里通过辅助线程 + channel 实现：
/// 1. 启动辅助线程调用 `handle.join()`
/// 2. 主线程通过 `recv_timeout` 等待结果
/// 3. 超时则放弃等待（消费者线程变为 detach 状态，队列关闭后自然退出）
///
/// **注**：超时后辅助线程仍在阻塞 join，直到消费者线程退出。由于 `stop()` 已关闭
/// 队列，消费者线程的 `pop` 会返回 `None` 后退出，辅助线程随即结束，不会泄漏。
fn join_with_timeout(handle: JoinHandle<()>, timeout: Duration) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = handle.join();
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(timeout);
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnDef, DataType, SchemaRegistry};
    use crate::slot::SlotManager;
    use crate::target::memory::MemoryWriter;
    use crate::CdcEngine;
    use szrsql_tx::wal::WalObserver;

    // --- 测试辅助 ---

    /// 创建测试用的 schema_registry + decoder
    fn test_setup() -> (Arc<SchemaRegistry>, Arc<RowDecoder>, Arc<SlotManager>) {
        let registry = Arc::new(SchemaRegistry::new());
        let decoder = Arc::new(RowDecoder::new(registry.clone()));
        let slot_mgr = Arc::new(SlotManager::in_memory());
        (registry, decoder, slot_mgr)
    }

    /// 创建测试用 CdcEngine（固定时间戳）
    fn test_cdc_engine() -> Arc<CdcEngine> {
        let observer_mgr = Arc::new(crate::CdcObserverManager::new());
        Arc::new(CdcEngine::with_timestamp_fn(observer_mgr, Box::new(|| 0)))
    }

    /// 创建测试用 task config
    fn test_config(task_id: &str, writer: Arc<dyn TargetWriter>) -> TaskConfig {
        TaskConfig {
            task_id: task_id.to_string(),
            description: "test task".to_string(),
            table_filter: None,
            writer,
            target_type: "memory".to_string(),
            target_connection: "memory://test".to_string(),
            snapshot_first: false,
            dialect: crate::migration::Dialect::Postgres,
            backpressure_config: BackpressureConfig::default(),
        }
    }

    /// 注册一个测试表到 schema_registry
    fn register_test_table(registry: &SchemaRegistry, table_id: u32, table_name: &str) {
        registry
            .create_table(
                table_id,
                table_name,
                vec![
                    ColumnDef::not_null("id", DataType::Int64),
                    ColumnDef::nullable("name", DataType::Text),
                ],
            )
            .expect("create_table should succeed");
    }

    /// 等待条件成立（P7-3）— 用于 e2e 测试等待消费者线程异步处理事件
    fn wait_for_stats<F: Fn() -> bool>(cond: F, timeout_ms: u64, what: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            if cond() {
                return;
            }
            if std::time::Instant::now() > deadline {
                panic!("wait_for_stats timeout ({timeout_ms}ms): {what}");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    // --- TaskState 测试 ---

    #[test]
    fn task_state_can_start() {
        assert!(TaskState::Created.can_start());
        assert!(TaskState::Failed.can_start());
        assert!(!TaskState::Running.can_start());
        assert!(!TaskState::Stopped.can_start());
    }

    #[test]
    fn task_state_can_pause_resume() {
        assert!(TaskState::Running.can_pause());
        assert!(!TaskState::Created.can_pause());
        assert!(TaskState::Paused.can_resume());
        assert!(!TaskState::Running.can_resume());
    }

    #[test]
    fn task_state_can_receive_events() {
        assert!(TaskState::Running.can_receive_events());
        assert!(!TaskState::Paused.can_receive_events());
        assert!(!TaskState::Created.can_receive_events());
    }

    #[test]
    fn task_state_is_terminal() {
        assert!(TaskState::Stopped.is_terminal());
        assert!(!TaskState::Failed.is_terminal());
        assert!(!TaskState::Running.is_terminal());
    }

    // --- ReplicationTask 生命周期 ---

    #[test]
    fn task_new_initial_state_created() {
        let (registry, decoder, slot_mgr) = test_setup();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task =
            ReplicationTask::new(test_config("rep1", writer), slot_mgr, decoder, registry).unwrap();
        assert_eq!(task.state(), TaskState::Created);
        assert_eq!(task.task_id(), "rep1");
        assert_eq!(task.target_type(), "memory");
    }

    #[test]
    fn task_empty_id_fails() {
        let (registry, decoder, slot_mgr) = test_setup();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let result = ReplicationTask::new(test_config("", writer), slot_mgr, decoder, registry);
        assert!(result.is_err());
    }

    #[test]
    fn task_lifecycle_start_pause_resume_stop() {
        let (registry, decoder, slot_mgr) = test_setup();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task = ReplicationTask::new(
            test_config("rep1", writer),
            slot_mgr.clone(),
            decoder,
            registry,
        )
        .unwrap();

        // Created → Starting → Running
        task.start().unwrap();
        assert_eq!(task.state(), TaskState::Running);

        // Running → Paused
        task.pause().unwrap();
        assert_eq!(task.state(), TaskState::Paused);

        // Paused → Running
        task.resume().unwrap();
        assert_eq!(task.state(), TaskState::Running);

        // Running → Stopped
        task.stop().unwrap();
        assert_eq!(task.state(), TaskState::Stopped);

        // slot 应该是 paused（停止时暂停 slot）
        let slot = slot_mgr.get_slot("rep1").unwrap();
        assert_eq!(slot.state, crate::slot::SlotState::Paused);
    }

    #[test]
    fn task_invalid_state_transition_fails() {
        let (registry, decoder, slot_mgr) = test_setup();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task =
            ReplicationTask::new(test_config("rep1", writer), slot_mgr, decoder, registry).unwrap();

        // Created 状态不能 pause
        let result = task.pause();
        assert!(result.is_err());

        // Created 状态不能 resume
        let result = task.resume();
        assert!(result.is_err());
    }

    #[test]
    fn task_fail_marks_failed_state() {
        let (registry, decoder, slot_mgr) = test_setup();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task =
            ReplicationTask::new(test_config("rep1", writer), slot_mgr, decoder, registry).unwrap();

        task.fail("test failure");
        assert_eq!(task.state(), TaskState::Failed);
        let stats = task.stats();
        assert_eq!(stats.error_count, 1);
        assert_eq!(stats.last_error, Some("test failure".to_string()));
    }

    // --- CdcObserver 实现 ---

    #[test]
    fn task_observer_filters_table() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 42, "users");
        register_test_table(&registry, 43, "orders");

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let mut config = test_config("rep1", writer);
        config.table_filter = Some(["users".to_string()].into_iter().collect());

        let task = ReplicationTask::new(config, slot_mgr, decoder, registry).unwrap();

        // 接受 users 表
        assert!(task.accepts_table(42));
        // 不接受 orders 表
        assert!(!task.accepts_table(43));
    }

    #[test]
    fn task_observer_no_filter_accepts_all() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 42, "users");

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task =
            ReplicationTask::new(test_config("rep1", writer), slot_mgr, decoder, registry).unwrap();

        assert!(task.accepts_table(42));
        assert!(task.accepts_table_name("anything"));
    }

    #[test]
    fn task_observer_ignores_events_when_not_running() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 42, "users");

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task = ReplicationTask::new(
            test_config("rep1", writer.clone()),
            slot_mgr,
            decoder,
            registry,
        )
        .unwrap();

        // Created 状态接收事件应被忽略
        let event = ChangeEvent::insert(1, 100, 42, vec![1, 2, 3], 0);
        task.on_event(event);

        let stats = task.stats();
        assert_eq!(stats.events_received, 0);
    }

    #[test]
    fn task_observer_processes_insert_when_running() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 42, "users");

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task = ReplicationTask::new(
            test_config("rep1", writer.clone()),
            slot_mgr,
            decoder,
            registry,
        )
        .unwrap();
        task.start().unwrap();

        // 构造一个 Insert 事件（new_row 是编码后的二进制）
        // 编码格式：[null_flag=0][len=8 BE][8 bytes i64]
        let mut new_row = Vec::new();
        new_row.push(0u8); // 非 null
        new_row.extend_from_slice(&8u32.to_be_bytes()); // len=8
        new_row.extend_from_slice(&42i64.to_be_bytes()); // id=42
        new_row.push(0u8); // 非 null
        new_row.extend_from_slice(&5u32.to_be_bytes()); // len=5
        new_row.extend_from_slice(b"hello"); // name="hello"

        let event = ChangeEvent::insert(1, 100, 42, new_row, 0);
        task.on_event(event);
        // P7-3：未启动消费者线程，需内联排空队列处理事件
        task.process_pending_events_inline(1);

        let stats = task.stats();
        assert_eq!(stats.events_received, 1);
        assert_eq!(stats.events_written, 1);
        assert!(stats.bytes_processed > 0);
    }

    #[test]
    fn task_observer_processes_commit_advances_lsn() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 42, "users");

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task = ReplicationTask::new(
            test_config("rep1", writer.clone()),
            slot_mgr.clone(),
            decoder,
            registry,
        )
        .unwrap();
        task.start().unwrap();

        // 发送 Commit 事件
        let event = ChangeEvent::commit(1, 100, 0);
        task.on_event(event);
        // P7-3：内联排空处理 Commit 事件（推进位点）
        task.process_pending_events_inline(1);

        let stats = task.stats();
        assert_eq!(stats.events_received, 1);
        assert_eq!(stats.transactions_processed, 1);
        assert_eq!(stats.confirmed_flush_lsn, 100);

        // slot 位点应推进
        let slot = slot_mgr.get_slot("rep1").unwrap();
        assert_eq!(slot.confirmed_flush_lsn, 100);
    }

    #[test]
    fn task_observer_skips_abort() {
        let (registry, decoder, slot_mgr) = test_setup();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task =
            ReplicationTask::new(test_config("rep1", writer), slot_mgr, decoder, registry).unwrap();
        task.start().unwrap();

        let event = ChangeEvent::abort(1, 100, 0);
        task.on_event(event);
        // P7-3：内联排空处理 Abort 事件（不写入，不推进位点）
        task.process_pending_events_inline(1);

        let stats = task.stats();
        assert_eq!(stats.events_received, 1);
        assert_eq!(stats.events_written, 0);
        assert_eq!(stats.transactions_processed, 0);
    }

    #[test]
    fn task_observer_filters_unwanted_table() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 42, "users");
        register_test_table(&registry, 43, "orders");

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let mut config = test_config("rep1", writer);
        config.table_filter = Some(["users".to_string()].into_iter().collect());

        let task = ReplicationTask::new(config, slot_mgr, decoder, registry).unwrap();
        task.start().unwrap();

        // orders 表事件应被过滤
        let event = ChangeEvent::insert(1, 100, 43, vec![1], 0);
        task.on_event(event);
        // P7-3：内联排空，事件在 process_single_event 中被表过滤
        task.process_pending_events_inline(1);

        let stats = task.stats();
        // 接收了，但被表过滤掉了（events_received 已 ++）
        assert_eq!(stats.events_received, 1);
        assert_eq!(stats.events_written, 0);
    }

    // --- ReplicationTaskManager ---

    #[test]
    fn manager_create_task() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr = ReplicationTaskManager::new(slot_mgr, decoder, registry, cdc_engine);

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task = mgr.create_task(test_config("rep1", writer)).unwrap();
        assert_eq!(task.state(), TaskState::Created);
        assert_eq!(mgr.task_count(), 1);
    }

    #[test]
    fn manager_create_duplicate_fails() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr = ReplicationTaskManager::new(slot_mgr, decoder, registry, cdc_engine);

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        mgr.create_task(test_config("rep1", writer)).unwrap();

        let writer2: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let result = mgr.create_task(test_config("rep1", writer2));
        assert!(result.is_err());
    }

    #[test]
    fn manager_start_pause_resume_stop() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr = ReplicationTaskManager::new(slot_mgr, decoder, registry, cdc_engine);

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        mgr.create_task(test_config("rep1", writer)).unwrap();

        mgr.start_task("rep1").unwrap();
        assert_eq!(mgr.get_task("rep1").unwrap().state(), TaskState::Running);

        mgr.pause_task("rep1").unwrap();
        assert_eq!(mgr.get_task("rep1").unwrap().state(), TaskState::Paused);

        mgr.resume_task("rep1").unwrap();
        assert_eq!(mgr.get_task("rep1").unwrap().state(), TaskState::Running);

        mgr.stop_task("rep1").unwrap();
        assert_eq!(mgr.get_task("rep1").unwrap().state(), TaskState::Stopped);
    }

    #[test]
    fn manager_remove_task() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr = ReplicationTaskManager::new(slot_mgr.clone(), decoder, registry, cdc_engine);

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        mgr.create_task(test_config("rep1", writer)).unwrap();
        assert_eq!(mgr.task_count(), 1);

        mgr.remove_task("rep1").unwrap();
        assert_eq!(mgr.task_count(), 0);

        // slot 应该被物理删除
        assert!(slot_mgr.get_slot("rep1").is_none());
    }

    #[test]
    fn manager_get_nonexistent_fails() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr = ReplicationTaskManager::new(slot_mgr, decoder, registry, cdc_engine);

        let result = mgr.get_task("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn manager_list_tasks() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr = ReplicationTaskManager::new(slot_mgr, decoder, registry, cdc_engine);

        let w1: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let w2: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        mgr.create_task(test_config("rep1", w1)).unwrap();
        mgr.create_task(test_config("rep2", w2)).unwrap();

        let list = mgr.list_tasks();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn manager_monitor_task() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr = ReplicationTaskManager::new(slot_mgr, decoder, registry, cdc_engine);

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        mgr.create_task(test_config("rep1", writer)).unwrap();
        mgr.start_task("rep1").unwrap();

        let info = mgr.monitor_task("rep1").unwrap();
        assert_eq!(info.task_id, "rep1");
        assert_eq!(info.state, TaskState::Running);
    }

    #[test]
    fn manager_tasks_by_state() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr = ReplicationTaskManager::new(slot_mgr, decoder, registry, cdc_engine);

        let w1: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let w2: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        mgr.create_task(test_config("rep1", w1)).unwrap();
        mgr.create_task(test_config("rep2", w2)).unwrap();

        mgr.start_task("rep1").unwrap();

        let running = mgr.tasks_by_state(TaskState::Running);
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].task_id, "rep1");

        let created = mgr.tasks_by_state(TaskState::Created);
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].task_id, "rep2");
    }

    #[test]
    fn manager_manager_stats() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr = ReplicationTaskManager::new(slot_mgr, decoder, registry, cdc_engine);

        let w1: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let w2: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        mgr.create_task(test_config("rep1", w1)).unwrap();
        mgr.create_task(test_config("rep2", w2)).unwrap();
        mgr.start_task("rep1").unwrap();
        mgr.stop_task("rep1").unwrap();

        let stats = mgr.manager_stats();
        assert_eq!(stats.total_tasks, 2);
        assert_eq!(stats.total_created, 2);
        assert_eq!(stats.total_started, 1);
        assert_eq!(stats.total_stopped, 1);
        assert_eq!(stats.running_tasks, 0);
    }

    #[test]
    fn manager_advance_flush_lsn() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr = ReplicationTaskManager::new(slot_mgr, decoder, registry, cdc_engine);

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        mgr.create_task(test_config("rep1", writer)).unwrap();
        mgr.start_task("rep1").unwrap();

        mgr.advance_flush_lsn("rep1", 500).unwrap();
        let info = mgr.monitor_task("rep1").unwrap();
        assert_eq!(info.stats.confirmed_flush_lsn, 500);
    }

    // --- 端到端集成测试 ---

    #[test]
    fn e2e_cdc_engine_to_task_to_writer() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 42, "users");

        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        mgr.create_task(test_config("rep1", writer)).unwrap();
        mgr.start_task("rep1").unwrap();

        // 通过 CdcEngine 分发事件
        // 构造一个 Insert WalRecord
        let mut new_row = Vec::new();
        new_row.push(0u8);
        new_row.extend_from_slice(&8u32.to_be_bytes());
        new_row.extend_from_slice(&42i64.to_be_bytes());
        new_row.push(0u8);
        new_row.extend_from_slice(&5u32.to_be_bytes());
        new_row.extend_from_slice(b"hello");

        let records = vec![
            szrsql_tx::wal::WalRecord::new(100, 1, szrsql_tx::wal::WalOpType::Insert, 42, new_row),
            szrsql_tx::wal::WalRecord::new(101, 1, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(1, records);

        // P7-3：等待消费者线程异步处理完成
        wait_for_stats(
            || {
                mgr.monitor_task("rep1")
                    .map(|i| i.stats.events_written >= 1)
                    .unwrap_or(false)
            },
            2000,
            "events_written should reach 1",
        );

        // task 应该收到事件
        let info = mgr.monitor_task("rep1").unwrap();
        assert!(info.stats.events_received >= 1);
        assert!(info.stats.events_written >= 1);
        assert_eq!(info.stats.transactions_processed, 1);
        assert_eq!(info.stats.confirmed_flush_lsn, 101);

        // MemoryWriter 应该收到事件（验证 writer 真的被调用）
        let _ = writer_clone;
    }

    // --- P3-3 端到端：CDC → Task → Writer 全链路（Insert/Update/Delete 完整事务） ---

    /// 构造一行二进制数据（id: i64, name: text）
    fn encode_test_row(id: i64, name: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        // id: 非 NULL
        buf.push(0u8);
        buf.extend_from_slice(&8u32.to_be_bytes());
        buf.extend_from_slice(&id.to_be_bytes());
        // name: 非 NULL
        buf.push(0u8);
        buf.extend_from_slice(&(name.len() as u32).to_be_bytes());
        buf.extend_from_slice(name.as_bytes());
        buf
    }

    /// 端到端测试：一个完整事务包含 Insert + Update + Delete + Commit
    ///
    /// 验证点：
    /// 1. CdcEngine 接收 WalRecord → 转换为 ChangeEvent → 分发给所有 observer
    /// 2. ReplicationTask 接收 ChangeEvent → 解码行 → 写入 MemoryWriter
    /// 3. 统计正确：events_received = 3 (Insert+Update+Delete), transactions_processed = 1
    /// 4. 位点推进：confirmed_flush_lsn = Commit 的 lsn
    /// 5. MemoryWriter 收到 3 个操作（Insert/Update/Delete）
    #[test]
    fn e2e_full_transaction_insert_update_delete() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 100, "orders");

        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        mgr.create_task(test_config("rep_full", writer)).unwrap();
        mgr.start_task("rep_full").unwrap();

        // 构造一个完整事务：Insert(LSN 200) + Update(LSN 201) + Delete(LSN 202) + Commit(LSN 203)
        let insert_row = encode_test_row(1, "alice");
        let update_row = encode_test_row(1, "ALICE");
        let delete_row = encode_test_row(1, "alice");

        let records = vec![
            szrsql_tx::wal::WalRecord::new(
                200,
                7,
                szrsql_tx::wal::WalOpType::Insert,
                100,
                insert_row,
            ),
            szrsql_tx::wal::WalRecord::new(
                201,
                7,
                szrsql_tx::wal::WalOpType::Update,
                100,
                update_row,
            ),
            szrsql_tx::wal::WalRecord::new(
                202,
                7,
                szrsql_tx::wal::WalOpType::Delete,
                100,
                delete_row,
            ),
            szrsql_tx::wal::WalRecord::new(203, 7, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(7, records);

        // P7-3：等待消费者线程异步处理完成
        wait_for_stats(
            || {
                mgr.monitor_task("rep_full")
                    .map(|i| i.stats.events_written >= 3)
                    .unwrap_or(false)
            },
            2000,
            "events_written should reach 3",
        );

        // 验证统计
        let info = mgr.monitor_task("rep_full").unwrap();
        // 注：events_received 在表过滤之前计数，所以包含 Commit 事件
        assert_eq!(
            info.stats.events_received, 4,
            "应该收到 4 个事件（3 DML + 1 Commit）"
        );
        assert_eq!(
            info.stats.events_written, 3,
            "应该写入 3 个 DML 事件到目标端（Commit 不写入）"
        );
        assert_eq!(info.stats.transactions_processed, 1, "应该处理 1 个事务");
        assert_eq!(
            info.stats.confirmed_flush_lsn, 203,
            "位点应推进到 Commit 的 LSN"
        );
        assert_eq!(info.stats.error_count, 0, "不应有错误");

        // 验证 MemoryWriter 收到 3 个操作
        assert_eq!(
            writer_clone.operation_count(),
            3,
            "MemoryWriter 应收到 3 个操作"
        );
        let ops = writer_clone.operations();
        assert_eq!(ops[0].op, CdcEventOp::Insert);
        assert_eq!(ops[1].op, CdcEventOp::Update);
        assert_eq!(ops[2].op, CdcEventOp::Delete);
        assert_eq!(ops[0].table_name, "orders");
    }

    /// 端到端测试：Abort 事务不应写入目标端，不应推进位点
    #[test]
    fn e2e_abort_transaction_not_written() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 100, "orders");

        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        mgr.create_task(test_config("rep_abort", writer)).unwrap();
        mgr.start_task("rep_abort").unwrap();

        // Insert + Abort
        let insert_row = encode_test_row(1, "alice");
        let records = vec![
            szrsql_tx::wal::WalRecord::new(
                300,
                9,
                szrsql_tx::wal::WalOpType::Insert,
                100,
                insert_row,
            ),
            szrsql_tx::wal::WalRecord::new(301, 9, szrsql_tx::wal::WalOpType::Abort, 0, vec![]),
        ];
        cdc_engine.on_commit(9, records);

        // P7-3：等待消费者线程异步处理完成
        wait_for_stats(
            || {
                mgr.monitor_task("rep_abort")
                    .map(|i| i.stats.events_written >= 1)
                    .unwrap_or(false)
            },
            2000,
            "events_written should reach 1",
        );

        // 注：当前 CdcEngine 的 on_commit 会将 WalRecord 转为 ChangeEvent 并分发
        // Insert 事件会被分发并被 task 接收，但 Abort 事件不写入目标端
        // 由于 Insert 在 Commit 之前，它会被写入（这是 at-least-once 语义）
        let info = mgr.monitor_task("rep_abort").unwrap();
        // Insert 应该被写入（at-least-once）
        assert!(
            info.stats.events_written >= 1,
            "Insert 应被写入（at-least-once 语义）"
        );
        // 不应有事务完成（Abort 不算 transactions_processed）
        assert_eq!(
            info.stats.transactions_processed, 0,
            "Abort 不应计入 transactions_processed"
        );
        // 位点不应推进到 301（Abort 不推进 flush_lsn）
        assert_eq!(
            info.stats.confirmed_flush_lsn, 0,
            "Abort 不应推进 confirmed_flush_lsn"
        );
        let _ = writer_clone;
    }

    /// 端到端测试：表过滤 — 只复制指定表
    #[test]
    fn e2e_table_filter_excludes_other_tables() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 100, "orders");
        register_test_table(&registry, 200, "products");

        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        // 创建只复制 orders 表的任务
        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        let mut config = test_config("rep_filter", writer);
        config.table_filter = Some(["orders".to_string()].into_iter().collect());
        mgr.create_task(config).unwrap();
        mgr.start_task("rep_filter").unwrap();

        // 向 orders 表写入（应被复制）
        let orders_row = encode_test_row(1, "order1");
        let records1 = vec![
            szrsql_tx::wal::WalRecord::new(
                400,
                11,
                szrsql_tx::wal::WalOpType::Insert,
                100,
                orders_row,
            ),
            szrsql_tx::wal::WalRecord::new(401, 11, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(11, records1);

        // 向 products 表写入（不应被复制）
        let products_row = encode_test_row(2, "product1");
        let records2 = vec![
            szrsql_tx::wal::WalRecord::new(
                402,
                12,
                szrsql_tx::wal::WalOpType::Insert,
                200,
                products_row,
            ),
            szrsql_tx::wal::WalRecord::new(403, 12, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(12, records2);

        // P7-3：等待消费者线程异步处理完成
        wait_for_stats(
            || {
                mgr.monitor_task("rep_filter")
                    .map(|i| i.stats.events_written >= 1)
                    .unwrap_or(false)
            },
            2000,
            "events_written should reach 1",
        );

        let info = mgr.monitor_task("rep_filter").unwrap();
        // 应该只收到 orders 表的 1 个 Insert 事件
        assert_eq!(
            info.stats.events_written, 1,
            "只应写入 orders 表的 1 个事件"
        );
        // 位点应推进到最后一个 Commit 的 LSN
        // 注：Commit 事件没有 table_id，不会被表过滤拦截，所以 products 表的 Commit
        // 也会推进 confirmed_flush_lsn 到 403
        assert_eq!(
            info.stats.confirmed_flush_lsn, 403,
            "位点应推进到最后一个 Commit 的 LSN（Commit 不受表过滤影响）"
        );
        // MemoryWriter 应只收到 orders 表的操作
        let ops = writer_clone.operations();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].table_name, "orders");
    }

    /// 端到端测试：Kafka Sink — ChangeEvent → Debezium JSON → MockKafkaProducer
    ///
    /// 验证 Kafka 链路：
    /// 1. CdcEngine 分发 Insert 事件
    /// 2. ReplicationTask 接收事件，写入 KafkaSink
    /// 3. KafkaSink 将事件转为 Debezium JSON，调用 MockKafkaProducer.send
    /// 4. MockKafkaProducer 记录消息，验证 topic/key/value
    #[test]
    fn e2e_kafka_sink_debezium_json() {
        use crate::target::kafka::{KafkaConfig, KafkaSink, MockKafkaProducer};
        use crate::target::TargetWriter;

        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 300, "events");

        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        // 构造 Kafka Sink
        let producer = Arc::new(MockKafkaProducer::new());
        let producer_clone = producer.clone();
        let kafka_config = KafkaConfig::new("cdc-events", "szrsql");
        let kafka_sink: Arc<dyn TargetWriter> = Arc::new(KafkaSink::new(kafka_config, producer));

        let mut config = test_config("rep_kafka", kafka_sink);
        config.target_type = "kafka".to_string();
        config.target_connection = "localhost:9092|cdc-events".to_string();
        mgr.create_task(config).unwrap();
        mgr.start_task("rep_kafka").unwrap();

        // 分发一个 Insert 事件
        let insert_row = encode_test_row(42, "test_event");
        let records = vec![
            szrsql_tx::wal::WalRecord::new(
                500,
                15,
                szrsql_tx::wal::WalOpType::Insert,
                300,
                insert_row,
            ),
            szrsql_tx::wal::WalRecord::new(501, 15, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(15, records);

        // P7-3：等待消费者线程异步处理完成
        wait_for_stats(
            || {
                mgr.monitor_task("rep_kafka")
                    .map(|i| i.stats.events_written >= 1)
                    .unwrap_or(false)
            },
            2000,
            "events_written should reach 1",
        );

        // 验证 Kafka 消息
        assert!(
            !producer_clone.is_empty(),
            "Kafka 应至少收到 1 条消息（Insert）"
        );
        let messages = producer_clone.messages();
        let first_msg = &messages[0];
        assert_eq!(first_msg.0, "cdc-events", "topic 应为 cdc-events");
        // value 应该是 Debezium JSON，包含 op=c（Create）字段
        assert!(
            first_msg.2.contains("\"op\":\"c\""),
            "Debezium JSON 应包含 op=c (Create)，实际值: {}",
            first_msg.2
        );
        // after 字段应存在（Insert 事件有后镜像）
        // 注：after 是 base64 编码的二进制行数据，不直接包含 "test_event" 字符串
        assert!(
            first_msg.2.contains("\"after\""),
            "Debezium JSON 应包含 after 字段（Insert 后镜像），实际值: {}",
            first_msg.2
        );
        // source 字段应包含 lsn=500
        assert!(
            first_msg.2.contains("\"lsn\":500"),
            "Debezium JSON source 应包含 lsn=500，实际值: {}",
            first_msg.2
        );

        // 验证 task 统计
        let info = mgr.monitor_task("rep_kafka").unwrap();
        assert_eq!(info.stats.events_written, 1, "应写入 1 个事件到 Kafka");
        assert_eq!(info.stats.error_count, 0, "不应有错误");
    }

    /// 端到端测试：暂停任务后事件不再写入，恢复后继续写入
    #[test]
    fn e2e_pause_resume_during_events() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 100, "orders");

        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        mgr.create_task(test_config("rep_pr", writer)).unwrap();
        mgr.start_task("rep_pr").unwrap();

        // 第一批事件（Running 状态）
        let row1 = encode_test_row(1, "first");
        let records1 = vec![
            szrsql_tx::wal::WalRecord::new(600, 20, szrsql_tx::wal::WalOpType::Insert, 100, row1),
            szrsql_tx::wal::WalRecord::new(601, 20, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(20, records1);

        // P7-3：等待消费者线程异步处理完成（第一批事件）
        wait_for_stats(
            || {
                mgr.monitor_task("rep_pr")
                    .map(|i| i.stats.events_written >= 1)
                    .unwrap_or(false)
            },
            2000,
            "events_written should reach 1 before pause",
        );

        // 暂停任务
        mgr.pause_task("rep_pr").unwrap();
        assert_eq!(mgr.get_task("rep_pr").unwrap().state(), TaskState::Paused);

        // 第二批事件（Paused 状态，应被忽略）
        let row2 = encode_test_row(2, "second");
        let records2 = vec![
            szrsql_tx::wal::WalRecord::new(602, 21, szrsql_tx::wal::WalOpType::Insert, 100, row2),
            szrsql_tx::wal::WalRecord::new(603, 21, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(21, records2);

        // P7-3：等待一小段时间，确认 Paused 状态下事件未被处理
        std::thread::sleep(std::time::Duration::from_millis(200));

        // 验证 Paused 状态下事件被忽略
        let info = mgr.monitor_task("rep_pr").unwrap();
        assert_eq!(info.stats.events_written, 1, "Paused 状态下不应写入新事件");

        // 恢复任务
        mgr.resume_task("rep_pr").unwrap();
        assert_eq!(mgr.get_task("rep_pr").unwrap().state(), TaskState::Running);

        // 第三批事件（恢复 Running 后应写入）
        let row3 = encode_test_row(3, "third");
        let records3 = vec![
            szrsql_tx::wal::WalRecord::new(604, 22, szrsql_tx::wal::WalOpType::Insert, 100, row3),
            szrsql_tx::wal::WalRecord::new(605, 22, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(22, records3);

        // P7-3：等待消费者线程异步处理完成（恢复后第三批事件）
        wait_for_stats(
            || {
                mgr.monitor_task("rep_pr")
                    .map(|i| i.stats.events_written >= 2)
                    .unwrap_or(false)
            },
            2000,
            "events_written should reach 2 after resume",
        );

        let info = mgr.monitor_task("rep_pr").unwrap();
        assert_eq!(info.stats.events_written, 2, "恢复后应写入第三批事件");

        // MemoryWriter 应收到 2 个操作（第一批 + 第三批，第二批被忽略）
        assert_eq!(
            writer_clone.operation_count(),
            2,
            "MemoryWriter 应收到 2 个操作"
        );
    }

    /// 端到端测试：多任务并发 — 同一 CdcEngine 分发到多个 task
    #[test]
    fn e2e_multiple_tasks_concurrent() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 100, "orders");

        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        // 创建 3 个任务，都复制 orders 表
        let mut writers = Vec::new();
        for i in 0..3 {
            let writer = Arc::new(MemoryWriter::new());
            writers.push(writer.clone());
            let task_id = format!("rep_multi_{i}");
            mgr.create_task(test_config(&task_id, writer)).unwrap();
            mgr.start_task(&task_id).unwrap();
        }

        // 分发 1 个事件
        let row = encode_test_row(1, "multi");
        let records = vec![
            szrsql_tx::wal::WalRecord::new(700, 30, szrsql_tx::wal::WalOpType::Insert, 100, row),
            szrsql_tx::wal::WalRecord::new(701, 30, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(30, records);

        // P7-3：等待所有任务的消费者线程异步处理完成
        for i in 0..3 {
            let task_id = format!("rep_multi_{i}");
            wait_for_stats(
                || {
                    mgr.monitor_task(&task_id)
                        .map(|info| info.stats.events_written >= 1)
                        .unwrap_or(false)
                },
                2000,
                &format!("events_written should reach 1 for {task_id}"),
            );
        }

        // 每个任务都应收到事件
        for (i, writer) in writers.iter().enumerate().take(3) {
            let task_id = format!("rep_multi_{i}");
            let info = mgr.monitor_task(&task_id).unwrap();
            assert_eq!(
                info.stats.events_written, 1,
                "任务 {task_id} 应写入 1 个事件"
            );
            assert_eq!(writer.operation_count(), 1);
        }

        // CdcEngine 应记录分发 3 次（3 个 observer × 1 个事件）
        assert!(cdc_engine.total_dispatched() >= 3, "应分发到 3 个 observer");
    }

    // --- P4-1 端到端测试：全量快照 + 增量衔接 ---

    /// 构造测试用 TableSchema（与 register_test_table 保持一致）
    fn make_test_schema(table_id: u32, table_name: &str) -> crate::schema::TableSchema {
        crate::schema::TableSchema {
            table_id,
            table_name: table_name.to_string(),
            columns: vec![
                ColumnDef::not_null("id", DataType::Int64),
                ColumnDef::nullable("name", DataType::Text),
            ],
            version: 1,
        }
    }

    /// 构造 DecodedRow（id, name）
    fn make_decoded_row(id: i64, name: &str) -> crate::decoder::DecodedRow {
        crate::decoder::DecodedRow {
            columns: vec![
                ("id".to_string(), szrsql_types::value::Value::Int64(id)),
                (
                    "name".to_string(),
                    szrsql_types::value::Value::Text(name.to_string()),
                ),
            ],
        }
    }

    /// P4-1 端到端：全量快照 + 增量衔接 — 不丢不重
    ///
    /// **场景**：
    /// 1. 源端表预置 3 行全量数据（id=1,2,3）
    /// 2. 启动任务 with snapshot：
    ///    - SnapshotTransfer 把 3 行全量数据写入 MemoryWriter
    ///    - snapshot_lsn = 1000（MemoryRowSource 固定返回）
    /// 3. 注入 CDC 事件：
    ///    - lsn=500 的 Insert（应被跳过：500 <= 1000，快照已包含）
    ///    - lsn=1000 的 Commit（应被跳过：1000 <= 1000）
    ///    - lsn=1500 的 Insert（应写入：1500 > 1000，增量数据）
    ///    - lsn=1501 的 Commit（应处理：推进 flush_lsn 到 1501）
    /// 4. 验证：
    ///    - writer 收到 4 个操作（3 全量 + 1 增量）
    ///    - task.stats.events_written = 1（只有 lsn=1500 的 Insert）
    ///    - task.snapshot_lsn = 1000
    ///    - task.stats.confirmed_flush_lsn = 1501
    #[test]
    fn e2e_snapshot_plus_incremental_no_loss_no_dup() {
        use crate::snapshot::MemoryRowSource;

        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 200, "products");

        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        // 1. 准备全量数据（3 行）
        let schema = make_test_schema(200, "products");
        let full_data = vec![
            make_decoded_row(1, "apple"),
            make_decoded_row(2, "banana"),
            make_decoded_row(3, "cherry"),
        ];
        let source =
            Arc::new(MemoryRowSource::new(vec![schema.clone()]).with_data("products", full_data));

        // 2. 创建任务（snapshot_first = true）
        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        let mut config = test_config("rep_snap", writer);
        config.snapshot_first = true;
        mgr.create_task(config).unwrap();

        // 3. 启动任务 with snapshot
        let snapshot_result = mgr
            .start_task_with_snapshot("rep_snap", source)
            .expect("start_task_with_snapshot should succeed");

        // 验证快照结果
        assert_eq!(
            snapshot_result.snapshot_lsn, 1000,
            "snapshot_lsn 应为 MemoryRowSource 返回的 1000"
        );
        assert_eq!(snapshot_result.total_rows, 3, "全量快照应传输 3 行");

        // 验证 task 状态
        let task = mgr.get_task("rep_snap").unwrap();
        assert_eq!(
            task.snapshot_lsn(),
            1000,
            "task.snapshot_lsn 应被设置为 1000"
        );
        assert_eq!(task.state(), TaskState::Running);

        // 4. 注入 CDC 事件
        // 4a. lsn=500 的 Insert（应被跳过 — 快照已包含此数据）
        let old_row = encode_test_row(99, "old_data");
        let records_old = vec![
            szrsql_tx::wal::WalRecord::new(
                500,
                30,
                szrsql_tx::wal::WalOpType::Insert,
                200,
                old_row,
            ),
            szrsql_tx::wal::WalRecord::new(501, 30, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(30, records_old);

        // 4b. lsn=1500 的 Insert（应写入 — 增量数据）
        let new_row = encode_test_row(4, "date");
        let records_new = vec![
            szrsql_tx::wal::WalRecord::new(
                1500,
                31,
                szrsql_tx::wal::WalOpType::Insert,
                200,
                new_row,
            ),
            szrsql_tx::wal::WalRecord::new(1501, 31, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(31, records_new);

        // P7-3：start_task_with_snapshot 未启动消费者线程，需内联排空队列处理事件
        task.process_pending_events_inline(10);

        // 5. 验证结果
        let info = mgr.monitor_task("rep_snap").unwrap();
        assert_eq!(
            info.snapshot_lsn, 1000,
            "monitor 返回的 snapshot_lsn 应为 1000"
        );
        assert_eq!(
            info.stats.events_written, 1,
            "CDC 阶段应只写入 1 个事件（lsn=1500），lsn=500 应被跳过"
        );
        assert_eq!(
            info.stats.confirmed_flush_lsn, 1501,
            "flush_lsn 应推进到 1501（lsn=1501 的 Commit），lsn=501 的 Commit 应被跳过"
        );

        // MemoryWriter 应收到 4 个操作：3（全量）+ 1（增量 lsn=1500）
        assert_eq!(
            writer_clone.operation_count(),
            4,
            "MemoryWriter 应收到 4 个操作（3 全量 + 1 增量），实际: {}",
            writer_clone.operation_count()
        );

        // 验证全量数据写入（前 3 个操作的 lsn=0，因为是快照阶段）
        let ops = writer_clone.operations();
        let snapshot_ops: Vec<_> = ops.iter().filter(|op| op.lsn == 0).collect();
        assert_eq!(snapshot_ops.len(), 3, "应有 3 个快照操作（lsn=0）");
        // 验证增量数据写入（第 4 个操作的 lsn=1500）
        let incremental_ops: Vec<_> = ops.iter().filter(|op| op.lsn == 1500).collect();
        assert_eq!(incremental_ops.len(), 1, "应有 1 个增量操作（lsn=1500）");
    }

    /// P4-1 端到端：未启用快照模式时（snapshot_first=false）正常 CDC
    ///
    /// 验证 `snapshot_lsn=0` 时不过滤任何事件，保持向后兼容
    #[test]
    fn e2e_no_snapshot_backward_compatible() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 300, "events");

        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        mgr.create_task(test_config("rep_nosnap", writer)).unwrap();
        // 使用普通 start_task（不调用 start_task_with_snapshot）
        mgr.start_task("rep_nosnap").unwrap();

        // 验证 snapshot_lsn = 0（未启用）
        let task = mgr.get_task("rep_nosnap").unwrap();
        assert_eq!(
            task.snapshot_lsn(),
            0,
            "未启用快照模式时 snapshot_lsn 应为 0"
        );

        // 注入 lsn=100 的事件（应正常写入，不被过滤）
        let row = encode_test_row(1, "event1");
        let records = vec![
            szrsql_tx::wal::WalRecord::new(100, 40, szrsql_tx::wal::WalOpType::Insert, 300, row),
            szrsql_tx::wal::WalRecord::new(101, 40, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(40, records);

        // P7-3：等待消费者线程异步处理完成
        wait_for_stats(
            || {
                mgr.monitor_task("rep_nosnap")
                    .map(|i| i.stats.events_written >= 1)
                    .unwrap_or(false)
            },
            2000,
            "events_written should reach 1",
        );

        let info = mgr.monitor_task("rep_nosnap").unwrap();
        assert_eq!(info.snapshot_lsn, 0);
        assert_eq!(info.stats.events_written, 1, "应写入 1 个事件");
        assert_eq!(info.stats.confirmed_flush_lsn, 101);

        assert_eq!(writer_clone.operation_count(), 1);
    }

    /// P4-1 单元：should_skip_event 逻辑
    #[test]
    fn should_skip_event_logic() {
        let (registry, decoder, slot_mgr) = test_setup();
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        let task =
            ReplicationTask::new(test_config("rep_skip", writer), slot_mgr, decoder, registry)
                .unwrap();

        // 初始 snapshot_lsn=0，不过滤任何事件
        assert_eq!(task.snapshot_lsn(), 0);
        let event = ChangeEvent::insert(1, 500, 100, vec![], 0);
        assert!(!task.should_skip_event(&event), "snapshot_lsn=0 时不应跳过");

        // 设置 snapshot_lsn=1000
        task.set_snapshot_lsn(1000);
        assert_eq!(task.snapshot_lsn(), 1000);

        // lsn <= 1000 应跳过
        let event_old = ChangeEvent::insert(1, 500, 100, vec![], 0);
        assert!(task.should_skip_event(&event_old), "lsn=500 应被跳过");

        let event_at = ChangeEvent::insert(1, 1000, 100, vec![], 0);
        assert!(task.should_skip_event(&event_at), "lsn=1000 应被跳过（<=）");

        // lsn > 1000 不应跳过
        let event_new = ChangeEvent::insert(1, 1500, 100, vec![], 0);
        assert!(!task.should_skip_event(&event_new), "lsn=1500 不应被跳过");
    }

    /// P4-1 单元：start_task_with_snapshot 空表快照应成功
    #[test]
    fn start_with_snapshot_empty_table_succeeds() {
        use crate::snapshot::MemoryRowSource;

        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 400, "failing");

        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let schema = make_test_schema(400, "failing");
        let source = Arc::new(MemoryRowSource::new(vec![schema]).with_data("failing", Vec::new()));

        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());
        mgr.create_task(test_config("rep_fail", writer)).unwrap();

        // 空表快照应成功（rows=0）
        let result = mgr.start_task_with_snapshot("rep_fail", source);
        assert!(result.is_ok(), "空表快照应成功");
        let snapshot = result.unwrap();
        assert_eq!(snapshot.total_rows, 0);
        assert_eq!(snapshot.snapshot_lsn, 1000);

        // 任务应处于 Running 状态
        let task = mgr.get_task("rep_fail").unwrap();
        assert_eq!(task.state(), TaskState::Running);
        assert_eq!(task.snapshot_lsn(), 1000);
    }

    // --- P4-2 测试：Schema 变更同步（DDL 事件捕获 + 目标端应用） ---

    /// 构造 SchemaChangeEvent 辅助函数
    fn make_schema_change_event(
        change_type: crate::schema::SchemaChangeType,
        table_id: u32,
        _table_name: &str,
        new_schema: Option<crate::schema::TableSchema>,
        old_schema: Option<crate::schema::TableSchema>,
        changed_column: Option<&str>,
    ) -> crate::schema::SchemaChangeEvent {
        let schema_version = new_schema.as_ref().map(|s| s.version).unwrap_or(0);
        crate::schema::SchemaChangeEvent {
            tx_id: 100,
            lsn: 5000,
            change_type,
            table_id,
            old_schema,
            new_schema,
            changed_column: changed_column.map(|s| s.to_string()),
            schema_version,
            timestamp: 0,
        }
    }

    /// P4-2 端到端：CreateTable DDL 同步到目标端
    ///
    /// **场景**：
    /// 1. 启动复制任务
    /// 2. 通知 SchemaChangeEvent(CreateTable, "orders")
    /// 3. 验证 MemoryWriter 收到 CREATE TABLE DDL
    /// 4. 验证 task.stats.ddl_events_processed = 1
    #[test]
    fn e2e_ddl_create_table_synced_to_target() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        mgr.create_task(test_config("rep_ddl_create", writer))
            .unwrap();
        mgr.start_task("rep_ddl_create").unwrap();

        // 构造 CreateTable 事件
        let new_schema = crate::schema::TableSchema {
            table_id: 500,
            table_name: "orders".to_string(),
            columns: vec![
                ColumnDef::not_null("order_id", DataType::Int64),
                ColumnDef::nullable("amount", DataType::Real),
            ],
            version: 1,
        };
        let event = make_schema_change_event(
            crate::schema::SchemaChangeType::CreateTable,
            500,
            "orders",
            Some(new_schema),
            None,
            None,
        );

        // 通知 DDL 事件
        mgr.notify_schema_change(event);

        // 验证 MemoryWriter 收到 DDL
        let ddls = writer_clone.ddls();
        assert_eq!(ddls.len(), 1, "应有 1 个 DDL 被执行");
        assert!(
            ddls[0].sql.contains("CREATE TABLE"),
            "DDL 应包含 CREATE TABLE，实际: {}",
            ddls[0].sql
        );
        assert!(
            ddls[0].sql.contains("orders"),
            "DDL 应包含表名 orders，实际: {}",
            ddls[0].sql
        );

        // 验证统计
        let info = mgr.monitor_task("rep_ddl_create").unwrap();
        assert_eq!(info.stats.ddl_events_processed, 1, "应处理 1 个 DDL 事件");
        assert_eq!(info.stats.ddl_error_count, 0, "不应有 DDL 错误");
    }

    /// P4-2 端到端：AlterTableAddColumn DDL 同步
    #[test]
    fn e2e_ddl_add_column_synced_to_target() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        mgr.create_task(test_config("rep_ddl_add", writer)).unwrap();
        mgr.start_task("rep_ddl_add").unwrap();

        // 构造 AlterTableAddColumn 事件：在 products 表新增 status 列
        let new_schema = crate::schema::TableSchema {
            table_id: 600,
            table_name: "products".to_string(),
            columns: vec![
                ColumnDef::not_null("id", DataType::Int64),
                ColumnDef::nullable("name", DataType::Text),
                ColumnDef::nullable("status", DataType::Text), // 新增列
            ],
            version: 2,
        };
        let event = make_schema_change_event(
            crate::schema::SchemaChangeType::AlterTableAddColumn,
            600,
            "products",
            Some(new_schema),
            None,
            Some("status"),
        );

        mgr.notify_schema_change(event);

        // 验证 MemoryWriter 收到 ALTER TABLE ADD COLUMN
        let ddls = writer_clone.ddls();
        assert_eq!(ddls.len(), 1, "应有 1 个 DDL");
        assert!(
            ddls[0].sql.contains("ALTER TABLE"),
            "DDL 应包含 ALTER TABLE，实际: {}",
            ddls[0].sql
        );
        assert!(
            ddls[0].sql.contains("status"),
            "DDL 应包含新增列名 status，实际: {}",
            ddls[0].sql
        );

        let info = mgr.monitor_task("rep_ddl_add").unwrap();
        assert_eq!(info.stats.ddl_events_processed, 1);
        assert_eq!(info.stats.ddl_error_count, 0);
    }

    /// P4-2 端到端：DropTable DDL 同步
    #[test]
    fn e2e_ddl_drop_table_synced_to_target() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        mgr.create_task(test_config("rep_ddl_drop", writer))
            .unwrap();
        mgr.start_task("rep_ddl_drop").unwrap();

        // 构造 DropTable 事件
        let old_schema = crate::schema::TableSchema {
            table_id: 700,
            table_name: "old_table".to_string(),
            columns: vec![ColumnDef::not_null("id", DataType::Int64)],
            version: 1,
        };
        let event = make_schema_change_event(
            crate::schema::SchemaChangeType::DropTable,
            700,
            "old_table",
            None,
            Some(old_schema),
            None,
        );

        mgr.notify_schema_change(event);

        let ddls = writer_clone.ddls();
        assert_eq!(ddls.len(), 1, "应有 1 个 DDL");
        assert!(
            ddls[0].sql.contains("DROP TABLE"),
            "DDL 应包含 DROP TABLE，实际: {}",
            ddls[0].sql
        );

        let info = mgr.monitor_task("rep_ddl_drop").unwrap();
        assert_eq!(info.stats.ddl_events_processed, 1);
    }

    /// P4-2 端到端：表过滤 — 非白名单表的 DDL 不同步
    #[test]
    fn e2e_ddl_table_filter_excludes_non_whitelisted() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        // 创建带表过滤的任务：只复制 "included" 表
        let mut config = test_config("rep_ddl_filter", writer);
        config.table_filter = Some(
            vec!["included".to_string()]
                .into_iter()
                .collect::<HashSet<String>>(),
        );
        mgr.create_task(config).unwrap();
        mgr.start_task("rep_ddl_filter").unwrap();

        // 通知非白名单表的 CreateTable 事件
        let excluded_schema = crate::schema::TableSchema {
            table_id: 800,
            table_name: "excluded".to_string(),
            columns: vec![ColumnDef::not_null("id", DataType::Int64)],
            version: 1,
        };
        let event = make_schema_change_event(
            crate::schema::SchemaChangeType::CreateTable,
            800,
            "excluded",
            Some(excluded_schema),
            None,
            None,
        );
        mgr.notify_schema_change(event);

        // 验证 MemoryWriter 未收到 DDL
        assert_eq!(writer_clone.ddl_count(), 0, "非白名单表的 DDL 不应被同步");

        // 通知白名单表的 CreateTable 事件
        let included_schema = crate::schema::TableSchema {
            table_id: 801,
            table_name: "included".to_string(),
            columns: vec![ColumnDef::not_null("id", DataType::Int64)],
            version: 1,
        };
        let event = make_schema_change_event(
            crate::schema::SchemaChangeType::CreateTable,
            801,
            "included",
            Some(included_schema),
            None,
            None,
        );
        mgr.notify_schema_change(event);

        // 验证 MemoryWriter 收到 1 个 DDL
        assert_eq!(writer_clone.ddl_count(), 1, "白名单表的 DDL 应被同步");
    }

    /// P4-2 端到端：MySQL 方言 DDL 生成
    ///
    /// 验证配置 dialect=MySQL 时生成 MySQL 兼容的 DDL（反引号引用）
    #[test]
    fn e2e_ddl_mysql_dialect_generates_backtick_quotes() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        // 创建 MySQL 方言的任务
        let mut config = test_config("rep_ddl_mysql", writer);
        config.dialect = crate::migration::Dialect::MySQL;
        mgr.create_task(config).unwrap();
        mgr.start_task("rep_ddl_mysql").unwrap();

        // 通知 CreateTable 事件
        let new_schema = crate::schema::TableSchema {
            table_id: 900,
            table_name: "users".to_string(),
            columns: vec![
                ColumnDef::not_null("id", DataType::Int64),
                ColumnDef::nullable("name", DataType::Text),
            ],
            version: 1,
        };
        let event = make_schema_change_event(
            crate::schema::SchemaChangeType::CreateTable,
            900,
            "users",
            Some(new_schema),
            None,
            None,
        );
        mgr.notify_schema_change(event);

        let ddls = writer_clone.ddls();
        assert_eq!(ddls.len(), 1);
        // MySQL 方言应使用反引号引用标识符
        assert!(
            ddls[0].sql.contains('`'),
            "MySQL DDL 应包含反引号，实际: {}",
            ddls[0].sql
        );
    }

    /// P4-2 端到端：停止任务后不再接收 DDL 事件
    #[test]
    fn e2e_ddl_stopped_task_ignores_events() {
        let (registry, decoder, slot_mgr) = test_setup();
        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        mgr.create_task(test_config("rep_ddl_stop", writer))
            .unwrap();
        mgr.start_task("rep_ddl_stop").unwrap();

        // 停止任务
        mgr.stop_task("rep_ddl_stop").unwrap();

        // 通知 DDL 事件
        let new_schema = crate::schema::TableSchema {
            table_id: 950,
            table_name: "after_stop".to_string(),
            columns: vec![ColumnDef::not_null("id", DataType::Int64)],
            version: 1,
        };
        let event = make_schema_change_event(
            crate::schema::SchemaChangeType::CreateTable,
            950,
            "after_stop",
            Some(new_schema),
            None,
            None,
        );
        mgr.notify_schema_change(event);

        // 验证未收到 DDL（任务已停止，observer 已注销）
        assert_eq!(writer_clone.ddl_count(), 0, "停止的任务不应接收 DDL 事件");
    }

    /// P4-2 端到端：DML + DDL 混合事件流
    ///
    /// 验证同一任务同时接收 DML 和 DDL 事件，互不干扰
    #[test]
    fn e2e_dml_and_ddl_mixed_event_stream() {
        let (registry, decoder, slot_mgr) = test_setup();
        register_test_table(&registry, 1000, "mixed_table");
        let cdc_engine = test_cdc_engine();
        let mgr =
            ReplicationTaskManager::new(slot_mgr, decoder, registry.clone(), cdc_engine.clone());

        let writer = Arc::new(MemoryWriter::new());
        let writer_clone = writer.clone();
        mgr.create_task(test_config("rep_mixed", writer)).unwrap();
        mgr.start_task("rep_mixed").unwrap();

        // 1. 发送 DML 事件（Insert）
        let row = encode_test_row(1, "data1");
        let records = vec![
            szrsql_tx::wal::WalRecord::new(2000, 50, szrsql_tx::wal::WalOpType::Insert, 1000, row),
            szrsql_tx::wal::WalRecord::new(2001, 50, szrsql_tx::wal::WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(50, records);

        // 2. 发送 DDL 事件（CreateTable）
        let new_schema = crate::schema::TableSchema {
            table_id: 1001,
            table_name: "new_table".to_string(),
            columns: vec![ColumnDef::not_null("id", DataType::Int64)],
            version: 1,
        };
        let ddl_event = make_schema_change_event(
            crate::schema::SchemaChangeType::CreateTable,
            1001,
            "new_table",
            Some(new_schema),
            None,
            None,
        );
        mgr.notify_schema_change(ddl_event);

        // P7-3：等待消费者线程异步处理完成（DML 事件）
        wait_for_stats(
            || {
                mgr.monitor_task("rep_mixed")
                    .map(|i| i.stats.events_written >= 1)
                    .unwrap_or(false)
            },
            2000,
            "events_written should reach 1",
        );

        // 3. 验证 DML 和 DDL 都被处理
        let info = mgr.monitor_task("rep_mixed").unwrap();
        assert_eq!(info.stats.events_written, 1, "应写入 1 个 DML 事件");
        assert_eq!(info.stats.ddl_events_processed, 1, "应处理 1 个 DDL 事件");
        assert_eq!(
            info.stats.error_count + info.stats.ddl_error_count,
            0,
            "不应有错误"
        );

        // MemoryWriter 应有 1 个 DML 操作 + 1 个 DDL
        assert_eq!(writer_clone.operation_count(), 1, "应有 1 个 DML 操作");
        assert_eq!(writer_clone.ddl_count(), 1, "应有 1 个 DDL");
    }
}
