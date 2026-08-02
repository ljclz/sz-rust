//! SzRSQL CDC 背压处理 — 对应 `SzRSQL实施进度.md` Phase 2.5.8。
//!
//! 当生产速度 >> 消费速度时，CDC 事件会在缓冲区堆积，最终导致 OOM。
//! 本模块实现基于水位线的背压机制：缓冲区满时通知 WAL 降低提交速度。
//!
//! # 核心概念
//!
//! - **BoundedEventQueue**：有界事件队列，内部 `Mutex<VecDeque>` + Condvar
//!   - `push` 阻塞直到有空间（Block 策略）
//!   - `try_push` 非阻塞，满时按策略处理（DropOldest / Reject / Signal）
//! - **BackpressureConfig**：配置（capacity / high_watermark / low_watermark / strategy）
//! - **BackpressureState**：状态机（Normal / BackpressureActive）
//! - **BackpressureCallback** trait：背压触发/解除/丢弃/拒绝回调
//! - **BackpressureStats**：统计（推送/弹出/丢弃/拒绝/背压次数）
//!
//! # 设计要点
//!
//! 1. **水位线机制**：
//!    - `current_size >= high_watermark` → 触发背压（通知 WAL 降速）
//!    - `current_size <= low_watermark` → 解除背压
//!    - 滞回（hysteresis）避免在水位线附近频繁切换状态，区间为 (low, high) 开区间
//!
//! 2. **背压策略**：
//!    - `Block`：`push` 阻塞直到有空间（推荐，at-least-once 语义）
//!    - `DropOldest`：丢弃最旧事件（有损，适用于可重放的流）
//!    - `Reject`：拒绝新事件（返回错误，调用方可重试）
//!    - `Signal`：仅发送信号，不阻塞不丢弃（协作式背压）
//!
//! 3. **线程安全**：
//!    - `Mutex<VecDeque<ChangeEvent>>` 保护队列
//!    - `Condvar` 实现阻塞 push/pop
//!    - `AtomicU64` 统计计数器（无锁读）
//!
//! 4. **回调机制**：
//!    - `on_backpressure_start`：背压触发时调用（通知 WAL 降速）
//!    - `on_backpressure_end`：背压解除时调用（通知 WAL 恢复速度）
//!    - `on_event_dropped`：丢弃事件时调用（审计日志）
//!    - `on_event_rejected`：拒绝事件时调用（审计日志）
//!
//! 5. **Stress 验证**：
//!    - 生产速度 1000000 TPS，消费速度 100000 TPS → 缓冲区满 → 背压触发 → 不 OOM
//!    - 验证：缓冲区大小始终 <= capacity，背压触发次数 > 0，无 panic

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::{Condvar, Mutex, RwLock};

use crate::ChangeEvent;

// =====================================================================
// 背压配置
// =====================================================================

/// 背压策略 — 缓冲区满时如何处理新事件
///
/// **选择建议**：
/// - **Block**：推荐默认值，保证 at-least-once 语义，但会阻塞生产者线程
/// - **DropOldest**：适用于可重放的流（如 CDC，可从 WAL 重新读取），有损
/// - **Reject**：调用方可重试，适用于要求严格不阻塞的场景
/// - **Signal**：仅协作式通知，不阻塞不丢弃，依赖生产者自觉降速
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackpressureStrategy {
    /// 阻塞生产者直到有空间（推荐，at-least-once 语义）
    #[default]
    Block,
    /// 丢弃最旧的事件（有损，适用于可重放的流）
    DropOldest,
    /// 拒绝新事件（返回错误，调用方可重试）
    Reject,
    /// 仅发送信号通知生产者降速（协作式背压）
    Signal,
}

/// 背压配置
///
/// **字段**：
/// - `capacity`：缓冲区最大容量（必须 > 0）
/// - `high_watermark`：高水位（触发背压的阈值，必须 <= capacity）
/// - `low_watermark`：低水位（解除背压的阈值，必须 < high_watermark）
/// - `strategy`：背压策略
///
/// **不变量**（构造时校验）：
/// - `0 < low_watermark < high_watermark <= capacity`
///
/// **推荐配置**：
/// - `capacity = 10000`，`high_watermark = 8000`，`low_watermark = 2000`
/// - 滞回区间 = 6000，避免在水位线附近频繁切换状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackpressureConfig {
    /// 缓冲区最大容量
    pub capacity: usize,
    /// 高水位（触发背压）
    pub high_watermark: usize,
    /// 低水位（解除背压）
    pub low_watermark: usize,
    /// 背压策略
    pub strategy: BackpressureStrategy,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            capacity: 10_000,
            high_watermark: 8_000,
            low_watermark: 2_000,
            strategy: BackpressureStrategy::Block,
        }
    }
}

impl BackpressureConfig {
    /// 创建配置并校验不变量
    ///
    /// **错误**：
    /// - `capacity == 0`
    /// - `high_watermark > capacity`
    /// - `low_watermark >= high_watermark`
    /// - `low_watermark == 0`（必须 > 0，否则背压永不解除）
    pub fn new(
        capacity: usize,
        high_watermark: usize,
        low_watermark: usize,
        strategy: BackpressureStrategy,
    ) -> Result<Self, BackpressureError> {
        if capacity == 0 {
            return Err(BackpressureError::InvalidConfig {
                reason: "capacity must be > 0".to_string(),
            });
        }
        if high_watermark > capacity {
            return Err(BackpressureError::InvalidConfig {
                reason: format!(
                    "high_watermark ({high_watermark}) must be <= capacity ({capacity})"
                ),
            });
        }
        if low_watermark == 0 {
            return Err(BackpressureError::InvalidConfig {
                reason: "low_watermark must be > 0".to_string(),
            });
        }
        if low_watermark >= high_watermark {
            return Err(BackpressureError::InvalidConfig {
                reason: format!(
                    "low_watermark ({low_watermark}) must be < high_watermark ({high_watermark})"
                ),
            });
        }
        Ok(Self {
            capacity,
            high_watermark,
            low_watermark,
            strategy,
        })
    }
}

// =====================================================================
// 背压状态
// =====================================================================

/// 背压状态 — 状态机
///
/// **状态转换**：
/// - `Normal` → `BackpressureActive`：当 `current_size >= high_watermark`
/// - `BackpressureActive` → `Normal`：当 `current_size <= low_watermark`
///
/// **滞回**：高水位触发，低水位解除，避免在水位线附近频繁切换
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackpressureState {
    /// 正常状态（缓冲区未满）
    #[default]
    Normal,
    /// 背压已触发（缓冲区达高水位）
    BackpressureActive,
}

impl BackpressureState {
    /// 是否处于背压状态
    pub fn is_active(self) -> bool {
        matches!(self, Self::BackpressureActive)
    }

    /// 转为字符串（用于日志和序列化）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::BackpressureActive => "backpressure_active",
        }
    }
}

// =====================================================================
// 背压统计
// =====================================================================

/// 背压统计 — 实时监控指标
///
/// **字段**：
/// - `current_size`：当前缓冲区大小
/// - `capacity`：最大容量
/// - `high_watermark` / `low_watermark`：水位线
/// - `state`：当前状态
/// - `total_pushed`：总推送数（成功入队）
/// - `total_popped`：总弹出数（成功出队）
/// - `total_dropped`：总丢弃数（DropOldest 策略）
/// - `total_rejected`：总拒绝数（Reject 策略）
/// - `backpressure_count`：背压触发次数（Normal → BackpressureActive 转换次数）
/// - `peak_size`：历史峰值大小
///
/// **注**：所有计数器使用 `AtomicU64`，支持无锁并发读
#[derive(Debug)]
pub struct BackpressureStats {
    /// 当前缓冲区大小
    current_size: AtomicU64,
    /// 最大容量
    capacity: AtomicU64,
    /// 高水位
    high_watermark: AtomicU64,
    /// 低水位
    low_watermark: AtomicU64,
    /// 当前状态（0 = Normal, 1 = BackpressureActive）
    state: AtomicU64,
    /// 总推送数
    total_pushed: AtomicU64,
    /// 总弹出数
    total_popped: AtomicU64,
    /// 总丢弃数
    total_dropped: AtomicU64,
    /// 总拒绝数
    total_rejected: AtomicU64,
    /// 背压触发次数
    backpressure_count: AtomicU64,
    /// 历史峰值大小
    peak_size: AtomicU64,
}

impl Default for BackpressureStats {
    fn default() -> Self {
        Self::new()
    }
}

impl BackpressureStats {
    /// 创建空统计
    pub fn new() -> Self {
        Self {
            current_size: AtomicU64::new(0),
            capacity: AtomicU64::new(0),
            high_watermark: AtomicU64::new(0),
            low_watermark: AtomicU64::new(0),
            state: AtomicU64::new(0),
            total_pushed: AtomicU64::new(0),
            total_popped: AtomicU64::new(0),
            total_dropped: AtomicU64::new(0),
            total_rejected: AtomicU64::new(0),
            backpressure_count: AtomicU64::new(0),
            peak_size: AtomicU64::new(0),
        }
    }

    /// 从配置初始化统计
    pub fn from_config(config: &BackpressureConfig) -> Self {
        let stats = Self::new();
        stats
            .capacity
            .store(config.capacity as u64, Ordering::SeqCst);
        stats
            .high_watermark
            .store(config.high_watermark as u64, Ordering::SeqCst);
        stats
            .low_watermark
            .store(config.low_watermark as u64, Ordering::SeqCst);
        stats
    }

    /// 当前缓冲区大小
    pub fn current_size(&self) -> u64 {
        self.current_size.load(Ordering::SeqCst)
    }

    /// 最大容量
    pub fn capacity(&self) -> u64 {
        self.capacity.load(Ordering::SeqCst)
    }

    /// 高水位
    pub fn high_watermark(&self) -> u64 {
        self.high_watermark.load(Ordering::SeqCst)
    }

    /// 低水位
    pub fn low_watermark(&self) -> u64 {
        self.low_watermark.load(Ordering::SeqCst)
    }

    /// 当前状态
    pub fn state(&self) -> BackpressureState {
        match self.state.load(Ordering::SeqCst) {
            1 => BackpressureState::BackpressureActive,
            _ => BackpressureState::Normal,
        }
    }

    /// 总推送数
    pub fn total_pushed(&self) -> u64 {
        self.total_pushed.load(Ordering::SeqCst)
    }

    /// 总弹出数
    pub fn total_popped(&self) -> u64 {
        self.total_popped.load(Ordering::SeqCst)
    }

    /// 总丢弃数
    pub fn total_dropped(&self) -> u64 {
        self.total_dropped.load(Ordering::SeqCst)
    }

    /// 总拒绝数
    pub fn total_rejected(&self) -> u64 {
        self.total_rejected.load(Ordering::SeqCst)
    }

    /// 背压触发次数
    pub fn backpressure_count(&self) -> u64 {
        self.backpressure_count.load(Ordering::SeqCst)
    }

    /// 历史峰值大小
    pub fn peak_size(&self) -> u64 {
        self.peak_size.load(Ordering::SeqCst)
    }

    /// 是否处于背压状态
    pub fn is_backpressure_active(&self) -> bool {
        self.state().is_active()
    }

    /// 缓冲区使用率（0.0 ~ 1.0）
    pub fn utilization(&self) -> f64 {
        let cap = self.capacity();
        if cap == 0 {
            return 0.0;
        }
        self.current_size() as f64 / cap as f64
    }

    /// 快照所有指标（用于序列化/日志）
    pub fn snapshot(&self) -> BackpressureStatsSnapshot {
        BackpressureStatsSnapshot {
            current_size: self.current_size(),
            capacity: self.capacity(),
            high_watermark: self.high_watermark(),
            low_watermark: self.low_watermark(),
            state: self.state(),
            total_pushed: self.total_pushed(),
            total_popped: self.total_popped(),
            total_dropped: self.total_dropped(),
            total_rejected: self.total_rejected(),
            backpressure_count: self.backpressure_count(),
            peak_size: self.peak_size(),
            utilization: self.utilization(),
        }
    }
}

/// 背压统计快照（某一时刻的不可变快照）
#[derive(Debug, Clone, PartialEq)]
pub struct BackpressureStatsSnapshot {
    pub current_size: u64,
    pub capacity: u64,
    pub high_watermark: u64,
    pub low_watermark: u64,
    pub state: BackpressureState,
    pub total_pushed: u64,
    pub total_popped: u64,
    pub total_dropped: u64,
    pub total_rejected: u64,
    pub backpressure_count: u64,
    pub peak_size: u64,
    pub utilization: f64,
}

impl BackpressureStatsSnapshot {
    /// 是否处于背压状态
    pub fn is_backpressure_active(&self) -> bool {
        self.state.is_active()
    }

    /// 当前缓冲区大小
    pub fn current_size(&self) -> u64 {
        self.current_size
    }

    /// 最大容量
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// 高水位
    pub fn high_watermark(&self) -> u64 {
        self.high_watermark
    }

    /// 低水位
    pub fn low_watermark(&self) -> u64 {
        self.low_watermark
    }

    /// 总推送数
    pub fn total_pushed(&self) -> u64 {
        self.total_pushed
    }

    /// 总弹出数
    pub fn total_popped(&self) -> u64 {
        self.total_popped
    }

    /// 总丢弃数
    pub fn total_dropped(&self) -> u64 {
        self.total_dropped
    }

    /// 总拒绝数
    pub fn total_rejected(&self) -> u64 {
        self.total_rejected
    }

    /// 背压触发次数
    pub fn backpressure_count(&self) -> u64 {
        self.backpressure_count
    }

    /// 历史峰值大小
    pub fn peak_size(&self) -> u64 {
        self.peak_size
    }

    /// 缓冲区使用率（0.0 ~ 1.0）
    pub fn utilization(&self) -> f64 {
        self.utilization
    }
}

// =====================================================================
// 背压错误
// =====================================================================

/// 背压错误
#[derive(Debug, thiserror::Error)]
pub enum BackpressureError {
    /// 无效配置
    #[error("invalid backpressure config: {reason}")]
    InvalidConfig { reason: String },

    /// 缓冲区已满（Reject 策略）
    #[error("buffer full (capacity={capacity}, current_size={current_size})")]
    BufferFull {
        capacity: usize,
        current_size: usize,
    },

    /// 队列已关闭（不再接受新事件）
    #[error("queue closed")]
    QueueClosed,
}

// =====================================================================
// 背压回调 trait
// =====================================================================

/// 背压回调 — 监听背压事件
///
/// **回调时机**：
/// - `on_backpressure_start`：Normal → BackpressureActive 转换时
/// - `on_backpressure_end`：BackpressureActive → Normal 转换时
/// - `on_event_dropped`：DropOldest 策略丢弃事件时
/// - `on_event_rejected`：Reject 策略拒绝事件时
///
/// **线程安全**：实现者必须是 `Send + Sync`，回调在队列锁内同步触发
///
/// **注**：回调内不应执行耗时操作，避免阻塞生产者/消费者线程
pub trait BackpressureCallback: Send + Sync {
    /// 背压触发（Normal → BackpressureActive）
    ///
    /// **典型实现**：通知 WAL 降低提交速度
    fn on_backpressure_start(&self, _stats: &BackpressureStatsSnapshot) {}

    /// 背压解除（BackpressureActive → Normal）
    ///
    /// **典型实现**：通知 WAL 恢复提交速度
    fn on_backpressure_end(&self, _stats: &BackpressureStatsSnapshot) {}

    /// 事件被丢弃（DropOldest 策略）
    ///
    /// **典型实现**：记录审计日志、发出告警
    fn on_event_dropped(&self, _event: &ChangeEvent) {}

    /// 事件被拒绝（Reject 策略）
    ///
    /// **典型实现**：记录审计日志、调用方决定是否重试
    fn on_event_rejected(&self, _event: &ChangeEvent) {}
}

/// 空回调（默认实现，不做任何事）
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopCallback;

impl BackpressureCallback for NoopCallback {}

/// 计数型回调 — 统计各类回调被触发的次数（测试用）
#[derive(Debug, Default)]
pub struct CountingCallback {
    /// 背压触发次数
    start_count: AtomicU64,
    /// 背压解除次数
    end_count: AtomicU64,
    /// 事件丢弃次数
    dropped_count: AtomicU64,
    /// 事件拒绝次数
    rejected_count: AtomicU64,
    /// 最后一次背压触发时的快照
    last_start_snapshot: Mutex<Option<BackpressureStatsSnapshot>>,
    /// 最后一次背压解除时的快照
    last_end_snapshot: Mutex<Option<BackpressureStatsSnapshot>>,
}

impl CountingCallback {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_count(&self) -> u64 {
        self.start_count.load(Ordering::SeqCst)
    }

    pub fn end_count(&self) -> u64 {
        self.end_count.load(Ordering::SeqCst)
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped_count.load(Ordering::SeqCst)
    }

    pub fn rejected_count(&self) -> u64 {
        self.rejected_count.load(Ordering::SeqCst)
    }

    pub fn last_start_snapshot(&self) -> Option<BackpressureStatsSnapshot> {
        self.last_start_snapshot.lock().clone()
    }

    pub fn last_end_snapshot(&self) -> Option<BackpressureStatsSnapshot> {
        self.last_end_snapshot.lock().clone()
    }
}

impl BackpressureCallback for CountingCallback {
    fn on_backpressure_start(&self, stats: &BackpressureStatsSnapshot) {
        self.start_count.fetch_add(1, Ordering::SeqCst);
        *self.last_start_snapshot.lock() = Some(stats.clone());
    }

    fn on_backpressure_end(&self, stats: &BackpressureStatsSnapshot) {
        self.end_count.fetch_add(1, Ordering::SeqCst);
        *self.last_end_snapshot.lock() = Some(stats.clone());
    }

    fn on_event_dropped(&self, _event: &ChangeEvent) {
        self.dropped_count.fetch_add(1, Ordering::SeqCst);
    }

    fn on_event_rejected(&self, _event: &ChangeEvent) {
        self.rejected_count.fetch_add(1, Ordering::SeqCst);
    }
}

// =====================================================================
// 有界事件队列 — 背压核心
// =====================================================================

/// 有界事件队列 — 基于水位线的背压机制
///
/// **设计**：
/// - `queue: Mutex<VecDeque<ChangeEvent>>`：内部队列
/// - `not_full: Condvar`：队列非满条件变量（Block 策略 push 等待）
/// - `not_empty: Condvar`：队列非空条件变量（pop 等待）
/// - `config: BackpressureConfig`：配置
/// - `stats: BackpressureStats`：实时统计
/// - `state: Mutex<BackpressureState>`：状态机（与 stats.state 同步）
/// - `callbacks: RwLock<Vec<Arc<dyn BackpressureCallback>>>`：回调列表
/// - `closed: Mutex<bool>`：是否已关闭
///
/// **线程安全**：所有方法支持多线程并发调用
///
/// **使用示例**：
/// ```ignore
/// use szrsql_cdc::backpressure::*;
/// use szrsql_cdc::ChangeEvent;
/// use std::sync::Arc;
///
/// let config = BackpressureConfig::new(1000, 800, 200, BackpressureStrategy::Block).unwrap();
/// let queue = Arc::new(BoundedEventQueue::new(config));
///
/// // 生产者线程
/// let producer_queue = queue.clone();
/// let producer = std::thread::spawn(move || {
///     for i in 0..10000u64 {
///         let event = ChangeEvent::insert(1, i, 42, vec![i as u8], i);
///         producer_queue.push(event); // Block 策略：满时阻塞
///     }
/// });
///
/// // 消费者线程
/// let consumer_queue = queue.clone();
/// let consumer = std::thread::spawn(move || {
///     let mut count = 0u64;
///     while count < 10000 {
///         if let Some(event) = consumer_queue.pop() {
///             count += 1;
///         }
///     }
/// });
///
/// producer.join().unwrap();
/// consumer.join().unwrap();
/// ```
pub struct BoundedEventQueue {
    /// 内部队列
    queue: Mutex<VecDeque<ChangeEvent>>,
    /// 队列非满条件变量
    not_full: Condvar,
    /// 队列非空条件变量
    not_empty: Condvar,
    /// 配置
    config: BackpressureConfig,
    /// 实时统计
    stats: BackpressureStats,
    /// 状态机（与 stats.state 同步）
    state: Mutex<BackpressureState>,
    /// 回调列表
    callbacks: RwLock<Vec<Arc<dyn BackpressureCallback>>>,
    /// 是否已关闭
    closed: Mutex<bool>,
}

impl BoundedEventQueue {
    /// 创建有界事件队列
    pub fn new(config: BackpressureConfig) -> Self {
        let stats = BackpressureStats::from_config(&config);
        Self {
            queue: Mutex::new(VecDeque::with_capacity(config.capacity)),
            not_full: Condvar::new(),
            not_empty: Condvar::new(),
            config,
            stats,
            state: Mutex::new(BackpressureState::Normal),
            callbacks: RwLock::new(Vec::new()),
            closed: Mutex::new(false),
        }
    }

    /// 获取配置（不可变引用）
    pub fn config(&self) -> BackpressureConfig {
        self.config
    }

    /// 获取统计快照
    pub fn stats(&self) -> BackpressureStatsSnapshot {
        self.stats.snapshot()
    }

    /// 获取统计引用（用于实时查询）
    pub fn stats_ref(&self) -> &BackpressureStats {
        &self.stats
    }

    /// 注册回调
    pub fn register_callback(&self, callback: Arc<dyn BackpressureCallback>) {
        let mut callbacks = self.callbacks.write();
        // 去重：按 Arc 数据指针地址
        let target_addr = Arc::as_ptr(&callback) as *const () as usize;
        if !callbacks
            .iter()
            .any(|c| Arc::as_ptr(c) as *const () as usize == target_addr)
        {
            callbacks.push(callback);
        }
    }

    /// 注销回调
    pub fn unregister_callback<C: BackpressureCallback + 'static>(
        &self,
        callback: &Arc<C>,
    ) -> bool {
        let mut callbacks = self.callbacks.write();
        let target_addr = Arc::as_ptr(callback) as *const () as usize;
        let original_len = callbacks.len();
        callbacks.retain(|c| Arc::as_ptr(c) as *const () as usize != target_addr);
        callbacks.len() < original_len
    }

    /// 已注册的回调数量
    pub fn callback_count(&self) -> usize {
        self.callbacks.read().len()
    }

    /// 关闭队列（不再接受新事件，pop 仍可继续直到队列空）
    pub fn close(&self) {
        let mut closed = self.closed.lock();
        *closed = true;
        // 唤醒所有等待的线程
        self.not_full.notify_all();
        self.not_empty.notify_all();
    }

    /// 是否已关闭
    pub fn is_closed(&self) -> bool {
        *self.closed.lock()
    }

    /// 当前队列大小
    pub fn len(&self) -> usize {
        self.queue.lock().len()
    }

    /// 队列是否为空
    pub fn is_empty(&self) -> bool {
        self.queue.lock().is_empty()
    }

    /// 队列是否已满
    pub fn is_full(&self) -> bool {
        self.queue.lock().len() >= self.config.capacity
    }

    /// 容量
    pub fn capacity(&self) -> usize {
        self.config.capacity
    }

    /// 高水位
    pub fn high_watermark(&self) -> usize {
        self.config.high_watermark
    }

    /// 低水位
    pub fn low_watermark(&self) -> usize {
        self.config.low_watermark
    }

    /// 策略
    pub fn strategy(&self) -> BackpressureStrategy {
        self.config.strategy
    }

    /// 推送事件（阻塞策略）
    ///
    /// **行为**：
    /// - `Block`：满时阻塞直到有空间
    /// - `DropOldest`：满时丢弃最旧事件，新事件入队
    /// - `Reject`：满时返回错误
    /// - `Signal`：满时仍入队（可能超过 capacity，调用方应避免）
    ///
    /// **返回**：
    /// - `Ok(())`：成功入队
    /// - `Err(QueueClosed)`：队列已关闭
    /// - `Err(BufferFull)`：Reject 策略且队列已满
    pub fn push(&self, event: ChangeEvent) -> Result<(), BackpressureError> {
        if self.is_closed() {
            return Err(BackpressureError::QueueClosed);
        }

        match self.config.strategy {
            BackpressureStrategy::Block => self.push_blocking(event),
            BackpressureStrategy::DropOldest => self.push_drop_oldest(event),
            BackpressureStrategy::Reject => self.push_reject(event),
            BackpressureStrategy::Signal => self.push_signal(event),
        }
    }

    /// 非阻塞推送（按策略处理，不等待）
    ///
    /// 与 `push` 的区别：Block 策略下，满时立即返回错误而非阻塞
    pub fn try_push(&self, event: ChangeEvent) -> Result<(), BackpressureError> {
        if self.is_closed() {
            return Err(BackpressureError::QueueClosed);
        }

        let mut queue = self.queue.lock();
        if queue.len() >= self.config.capacity {
            match self.config.strategy {
                BackpressureStrategy::Block => {
                    // Block 策略在 try_push 下退化为 Reject
                    return Err(BackpressureError::BufferFull {
                        capacity: self.config.capacity,
                        current_size: queue.len(),
                    });
                }
                BackpressureStrategy::DropOldest => {
                    if let Some(dropped) = queue.pop_front() {
                        self.stats.total_dropped.fetch_add(1, Ordering::SeqCst);
                        self.notify_event_dropped(&dropped);
                    }
                }
                BackpressureStrategy::Reject => {
                    self.stats.total_rejected.fetch_add(1, Ordering::SeqCst);
                    self.notify_event_rejected(&event);
                    return Err(BackpressureError::BufferFull {
                        capacity: self.config.capacity,
                        current_size: queue.len(),
                    });
                }
                BackpressureStrategy::Signal => {
                    // Signal 策略：仍入队，仅通知
                }
            }
        }

        queue.push_back(event);
        let new_size = queue.len();
        drop(queue);

        self.update_state_and_stats_after_push(new_size);
        self.not_empty.notify_one();
        Ok(())
    }

    /// 阻塞推送（Block 策略）
    fn push_blocking(&self, event: ChangeEvent) -> Result<(), BackpressureError> {
        let mut queue = self.queue.lock();
        loop {
            if *self.closed.lock() {
                return Err(BackpressureError::QueueClosed);
            }
            if queue.len() < self.config.capacity {
                queue.push_back(event);
                let new_size = queue.len();
                drop(queue);
                self.update_state_and_stats_after_push(new_size);
                self.not_empty.notify_one();
                return Ok(());
            }
            // 满时等待
            self.not_full.wait(&mut queue);
        }
    }

    /// 丢弃最旧事件推送（DropOldest 策略）
    fn push_drop_oldest(&self, event: ChangeEvent) -> Result<(), BackpressureError> {
        let mut queue = self.queue.lock();
        if queue.len() >= self.config.capacity {
            if let Some(dropped) = queue.pop_front() {
                self.stats.total_dropped.fetch_add(1, Ordering::SeqCst);
                self.notify_event_dropped(&dropped);
            }
        }
        queue.push_back(event);
        let new_size = queue.len();
        drop(queue);
        self.update_state_and_stats_after_push(new_size);
        self.not_empty.notify_one();
        Ok(())
    }

    /// 拒绝推送（Reject 策略）
    fn push_reject(&self, event: ChangeEvent) -> Result<(), BackpressureError> {
        let mut queue = self.queue.lock();
        if queue.len() >= self.config.capacity {
            self.stats.total_rejected.fetch_add(1, Ordering::SeqCst);
            self.notify_event_rejected(&event);
            return Err(BackpressureError::BufferFull {
                capacity: self.config.capacity,
                current_size: queue.len(),
            });
        }
        queue.push_back(event);
        let new_size = queue.len();
        drop(queue);
        self.update_state_and_stats_after_push(new_size);
        self.not_empty.notify_one();
        Ok(())
    }

    /// 信号推送（Signal 策略）— 满时仍入队，仅通知
    ///
    /// **注**：此策略可能导致队列超过 capacity，调用方应自行检查
    fn push_signal(&self, event: ChangeEvent) -> Result<(), BackpressureError> {
        let mut queue = self.queue.lock();
        queue.push_back(event);
        let new_size = queue.len();
        drop(queue);
        self.update_state_and_stats_after_push(new_size);
        self.not_empty.notify_one();
        Ok(())
    }

    /// 弹出事件（阻塞）
    ///
    /// **行为**：队列空时阻塞，直到有事件或队列关闭
    ///
    /// **返回**：
    /// - `Some(event)`：成功弹出
    /// - `None`：队列已关闭且为空
    pub fn pop(&self) -> Option<ChangeEvent> {
        let mut queue = self.queue.lock();
        loop {
            if let Some(event) = queue.pop_front() {
                let new_size = queue.len();
                drop(queue);
                self.update_state_and_stats_after_pop(new_size);
                self.not_full.notify_one();
                return Some(event);
            }
            // 队列为空
            if *self.closed.lock() {
                return None;
            }
            self.not_empty.wait(&mut queue);
        }
    }

    /// 非阻塞弹出
    ///
    /// **返回**：
    /// - `Some(event)`：成功弹出
    /// - `None`：队列为空
    pub fn try_pop(&self) -> Option<ChangeEvent> {
        let mut queue = self.queue.lock();
        if let Some(event) = queue.pop_front() {
            let new_size = queue.len();
            drop(queue);
            self.update_state_and_stats_after_pop(new_size);
            self.not_full.notify_one();
            Some(event)
        } else {
            None
        }
    }

    /// 批量弹出（最多 n 个事件）
    ///
    /// **返回**：实际弹出的事件列表（可能少于 n）
    pub fn drain(&self, n: usize) -> Vec<ChangeEvent> {
        let mut queue = self.queue.lock();
        let take_n = n.min(queue.len());
        let events: Vec<ChangeEvent> = queue.drain(..take_n).collect();
        let new_size = queue.len();
        drop(queue);
        self.update_state_and_stats_after_pop(new_size);
        self.not_full.notify_all();
        events
    }

    /// 推送后更新状态和统计
    fn update_state_and_stats_after_push(&self, new_size: usize) {
        self.stats
            .current_size
            .store(new_size as u64, Ordering::SeqCst);
        self.stats.total_pushed.fetch_add(1, Ordering::SeqCst);

        // 更新峰值
        let current_peak = self.stats.peak_size.load(Ordering::SeqCst);
        if (new_size as u64) > current_peak {
            self.stats
                .peak_size
                .store(new_size as u64, Ordering::SeqCst);
        }

        // 状态转换检查：Normal → BackpressureActive
        if new_size >= self.config.high_watermark {
            let mut state = self.state.lock();
            if *state == BackpressureState::Normal {
                *state = BackpressureState::BackpressureActive;
                self.stats.state.store(1, Ordering::SeqCst);
                self.stats.backpressure_count.fetch_add(1, Ordering::SeqCst);
                let snapshot = self.stats.snapshot();
                drop(state);
                self.notify_backpressure_start(&snapshot);
            }
        }
    }

    /// 弹出后更新状态和统计
    fn update_state_and_stats_after_pop(&self, new_size: usize) {
        self.stats
            .current_size
            .store(new_size as u64, Ordering::SeqCst);
        self.stats.total_popped.fetch_add(1, Ordering::SeqCst);

        // 状态转换检查：BackpressureActive → Normal
        // 注：使用 <= 而非 <，使 low_watermark 成为"达到即解除"的水位线，
        // 与 high_watermark 的">= 触发"对称，滞回区间为 (low, high) 开区间。
        if new_size <= self.config.low_watermark {
            let mut state = self.state.lock();
            if *state == BackpressureState::BackpressureActive {
                *state = BackpressureState::Normal;
                self.stats.state.store(0, Ordering::SeqCst);
                let snapshot = self.stats.snapshot();
                drop(state);
                self.notify_backpressure_end(&snapshot);
            }
        }
    }

    /// 通知所有回调：背压触发
    fn notify_backpressure_start(&self, snapshot: &BackpressureStatsSnapshot) {
        let callbacks = self.callbacks.read();
        for callback in callbacks.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback.on_backpressure_start(snapshot);
            }));
        }
    }

    /// 通知所有回调：背压解除
    fn notify_backpressure_end(&self, snapshot: &BackpressureStatsSnapshot) {
        let callbacks = self.callbacks.read();
        for callback in callbacks.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback.on_backpressure_end(snapshot);
            }));
        }
    }

    /// 通知所有回调：事件被丢弃
    fn notify_event_dropped(&self, event: &ChangeEvent) {
        let callbacks = self.callbacks.read();
        for callback in callbacks.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback.on_event_dropped(event);
            }));
        }
    }

    /// 通知所有回调：事件被拒绝
    fn notify_event_rejected(&self, event: &ChangeEvent) {
        let callbacks = self.callbacks.read();
        for callback in callbacks.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback.on_event_rejected(event);
            }));
        }
    }
}

// =====================================================================
// 辅助构造函数
// =====================================================================

impl BoundedEventQueue {
    /// 创建默认配置的队列（capacity=10000, high=8000, low=2000, Block 策略）
    pub fn with_default_config() -> Self {
        Self::new(BackpressureConfig::default())
    }

    /// 创建 Block 策略的队列
    pub fn with_block_strategy(capacity: usize, high: usize, low: usize) -> Self {
        let config = BackpressureConfig::new(capacity, high, low, BackpressureStrategy::Block)
            .expect("invalid backpressure config");
        Self::new(config)
    }

    /// 创建 DropOldest 策略的队列
    pub fn with_drop_oldest_strategy(capacity: usize, high: usize, low: usize) -> Self {
        let config = BackpressureConfig::new(capacity, high, low, BackpressureStrategy::DropOldest)
            .expect("invalid backpressure config");
        Self::new(config)
    }

    /// 创建 Reject 策略的队列
    pub fn with_reject_strategy(capacity: usize, high: usize, low: usize) -> Self {
        let config = BackpressureConfig::new(capacity, high, low, BackpressureStrategy::Reject)
            .expect("invalid backpressure config");
        Self::new(config)
    }

    /// 创建 Signal 策略的队列
    pub fn with_signal_strategy(capacity: usize, high: usize, low: usize) -> Self {
        let config = BackpressureConfig::new(capacity, high, low, BackpressureStrategy::Signal)
            .expect("invalid backpressure config");
        Self::new(config)
    }
}

// =====================================================================
// WalBackpressureSignal — 通知 WAL 降低/恢复提交速度的信号
// =====================================================================

/// WAL 背压信号 — 通知 WAL 降低或恢复提交速度
///
/// **设计**：
/// - 内部 `Arc<AtomicU64>` 表示当前建议的提交速率（TPS）
/// - `0` 表示停止提交
/// - `u64::MAX` 表示全速提交（无限制）
/// - 中间值表示建议的 TPS 上限
///
/// **使用方式**：
/// 1. WAL 持有 `WalBackpressureSignal`，定期检查 `recommended_tps()`
/// 2. 背压触发时，回调设置 `set_recommended_tps(low_value)`
/// 3. 背压解除时，回调设置 `set_recommended_tps(u64::MAX)`
///
/// **线程安全**：内部 `AtomicU64`，支持多线程并发读写
#[derive(Debug, Clone)]
pub struct WalBackpressureSignal {
    recommended_tps: Arc<AtomicU64>,
    /// 背压触发次数（统计用）
    trigger_count: Arc<AtomicU64>,
    /// 背压解除次数（统计用）
    release_count: Arc<AtomicU64>,
}

impl Default for WalBackpressureSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl WalBackpressureSignal {
    /// 创建信号，初始推荐 TPS 为 u64::MAX（全速）
    pub fn new() -> Self {
        Self {
            recommended_tps: Arc::new(AtomicU64::new(u64::MAX)),
            trigger_count: Arc::new(AtomicU64::new(0)),
            release_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 获取当前推荐的 TPS
    pub fn recommended_tps(&self) -> u64 {
        self.recommended_tps.load(Ordering::SeqCst)
    }

    /// 设置推荐的 TPS
    pub fn set_recommended_tps(&self, tps: u64) {
        self.recommended_tps.store(tps, Ordering::SeqCst);
    }

    /// 是否处于背压状态（推荐 TPS < u64::MAX）
    pub fn is_backpressure_active(&self) -> bool {
        self.recommended_tps() < u64::MAX
    }

    /// 触发背压（设置低 TPS）
    pub fn trigger(&self, low_tps: u64) {
        self.set_recommended_tps(low_tps);
        self.trigger_count.fetch_add(1, Ordering::SeqCst);
    }

    /// 解除背压（恢复全速）
    pub fn release(&self) {
        self.set_recommended_tps(u64::MAX);
        self.release_count.fetch_add(1, Ordering::SeqCst);
    }

    /// 触发次数
    pub fn trigger_count(&self) -> u64 {
        self.trigger_count.load(Ordering::SeqCst)
    }

    /// 解除次数
    pub fn release_count(&self) -> u64 {
        self.release_count.load(Ordering::SeqCst)
    }
}

/// 基于 WalBackpressureSignal 的回调实现
///
/// **行为**：
/// - `on_backpressure_start`：调用 `signal.trigger(low_tps)`，降低 WAL 提交速度
/// - `on_backpressure_end`：调用 `signal.release()`，恢复 WAL 提交速度
pub struct WalBackpressureCallback {
    /// WAL 背压信号
    signal: WalBackpressureSignal,
    /// 背压时的低 TPS 值
    low_tps: u64,
}

impl WalBackpressureCallback {
    /// 创建回调
    ///
    /// **参数**：
    /// - `signal`：WAL 背压信号
    /// - `low_tps`：背压时的低 TPS 值（例如 100000，对应生产速度 1000000 TPS 的 10%）
    pub fn new(signal: WalBackpressureSignal, low_tps: u64) -> Self {
        Self { signal, low_tps }
    }

    /// 获取信号引用
    pub fn signal(&self) -> &WalBackpressureSignal {
        &self.signal
    }
}

impl BackpressureCallback for WalBackpressureCallback {
    fn on_backpressure_start(&self, _stats: &BackpressureStatsSnapshot) {
        self.signal.trigger(self.low_tps);
    }

    fn on_backpressure_end(&self, _stats: &BackpressureStatsSnapshot) {
        self.signal.release();
    }
}

// =====================================================================
// 模拟 WAL 提交器 — 测试用
// =====================================================================

/// 模拟 WAL 提交器 — 根据背压信号调整提交速度
///
/// **设计**：
/// - 持有 `WalBackpressureSignal`，每次提交前检查推荐 TPS
/// - 若推荐 TPS < 全速，按比例延迟提交（模拟降速）
/// - 统计：总提交数、被背压延迟的提交数、总延迟时间
#[derive(Debug)]
pub struct SimulatedWalCommitter {
    signal: WalBackpressureSignal,
    total_committed: AtomicU64,
    backpressure_delayed: AtomicU64,
    full_speed_tps: u64,
}

impl SimulatedWalCommitter {
    /// 创建模拟提交器
    ///
    /// **参数**：
    /// - `signal`：WAL 背压信号
    /// - `full_speed_tps`：全速提交时的 TPS（例如 1000000）
    pub fn new(signal: WalBackpressureSignal, full_speed_tps: u64) -> Self {
        Self {
            signal,
            total_committed: AtomicU64::new(0),
            backpressure_delayed: AtomicU64::new(0),
            full_speed_tps,
        }
    }

    /// 模拟提交一个事件（根据背压信号决定是否延迟）
    ///
    /// **返回**：实际等待的时间（毫秒）
    pub fn commit_one(&self) -> u64 {
        let recommended = self.signal.recommended_tps();
        let delay_ms = if recommended >= self.full_speed_tps {
            0
        } else if recommended == 0 {
            // 完全停止，等待 1ms 避免忙等
            self.backpressure_delayed.fetch_add(1, Ordering::SeqCst);
            1
        } else {
            // 按比例延迟：recommended_tps 越低，延迟越长
            self.backpressure_delayed.fetch_add(1, Ordering::SeqCst);
            let ratio = self.full_speed_tps as f64 / recommended as f64;
            // 模拟延迟（实际不真的 sleep，仅返回延迟值用于测试）
            ((ratio - 1.0) * 0.001) as u64 // 0.001ms 基础单位
        };

        self.total_committed.fetch_add(1, Ordering::SeqCst);
        delay_ms
    }

    /// 总提交数
    pub fn total_committed(&self) -> u64 {
        self.total_committed.load(Ordering::SeqCst)
    }

    /// 被背压延迟的提交数
    pub fn backpressure_delayed(&self) -> u64 {
        self.backpressure_delayed.load(Ordering::SeqCst)
    }

    /// 是否处于背压状态
    pub fn is_backpressure_active(&self) -> bool {
        self.signal.is_backpressure_active()
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    // 辅助：创建测试事件
    fn make_event(lsn: u64) -> ChangeEvent {
        ChangeEvent::insert(1, lsn, 42, vec![lsn as u8], lsn)
    }

    // 辅助：创建小容量配置（capacity=10, high=8, low=2）
    fn small_config(strategy: BackpressureStrategy) -> BackpressureConfig {
        BackpressureConfig::new(10, 8, 2, strategy).unwrap()
    }

    // =================================================================
    // Part 1: BackpressureConfig 基础
    // =================================================================

    #[test]
    fn phase_2_5_8_config_default_values() {
        let config = BackpressureConfig::default();
        assert_eq!(config.capacity, 10_000);
        assert_eq!(config.high_watermark, 8_000);
        assert_eq!(config.low_watermark, 2_000);
        assert_eq!(config.strategy, BackpressureStrategy::Block);
    }

    #[test]
    fn phase_2_5_8_config_new_valid() {
        let config = BackpressureConfig::new(1000, 800, 200, BackpressureStrategy::Block).unwrap();
        assert_eq!(config.capacity, 1000);
        assert_eq!(config.high_watermark, 800);
        assert_eq!(config.low_watermark, 200);
    }

    #[test]
    fn phase_2_5_8_config_new_zero_capacity_rejected() {
        let result = BackpressureConfig::new(0, 0, 0, BackpressureStrategy::Block);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, BackpressureError::InvalidConfig { .. }));
    }

    #[test]
    fn phase_2_5_8_config_new_high_exceeds_capacity_rejected() {
        let result = BackpressureConfig::new(100, 200, 50, BackpressureStrategy::Block);
        assert!(result.is_err());
    }

    #[test]
    fn phase_2_5_8_config_new_low_zero_rejected() {
        let result = BackpressureConfig::new(100, 80, 0, BackpressureStrategy::Block);
        assert!(result.is_err());
    }

    #[test]
    fn phase_2_5_8_config_new_low_geq_high_rejected() {
        // low == high
        let result = BackpressureConfig::new(100, 80, 80, BackpressureStrategy::Block);
        assert!(result.is_err());

        // low > high
        let result = BackpressureConfig::new(100, 80, 90, BackpressureStrategy::Block);
        assert!(result.is_err());
    }

    #[test]
    fn phase_2_5_8_config_strategy_default_is_block() {
        assert_eq!(BackpressureStrategy::default(), BackpressureStrategy::Block);
    }

    // =================================================================
    // Part 2: BackpressureState
    // =================================================================

    #[test]
    fn phase_2_5_8_state_default_is_normal() {
        assert_eq!(BackpressureState::default(), BackpressureState::Normal);
    }

    #[test]
    fn phase_2_5_8_state_is_active() {
        assert!(!BackpressureState::Normal.is_active());
        assert!(BackpressureState::BackpressureActive.is_active());
    }

    #[test]
    fn phase_2_5_8_state_as_str() {
        assert_eq!(BackpressureState::Normal.as_str(), "normal");
        assert_eq!(
            BackpressureState::BackpressureActive.as_str(),
            "backpressure_active"
        );
    }

    // =================================================================
    // Part 3: BackpressureStats 基础
    // =================================================================

    #[test]
    fn phase_2_5_8_stats_new_defaults_zero() {
        let stats = BackpressureStats::new();
        assert_eq!(stats.current_size(), 0);
        assert_eq!(stats.capacity(), 0);
        assert_eq!(stats.high_watermark(), 0);
        assert_eq!(stats.low_watermark(), 0);
        assert_eq!(stats.state(), BackpressureState::Normal);
        assert_eq!(stats.total_pushed(), 0);
        assert_eq!(stats.total_popped(), 0);
        assert_eq!(stats.total_dropped(), 0);
        assert_eq!(stats.total_rejected(), 0);
        assert_eq!(stats.backpressure_count(), 0);
        assert_eq!(stats.peak_size(), 0);
    }

    #[test]
    fn phase_2_5_8_stats_from_config() {
        let config = BackpressureConfig::new(1000, 800, 200, BackpressureStrategy::Block).unwrap();
        let stats = BackpressureStats::from_config(&config);
        assert_eq!(stats.capacity(), 1000);
        assert_eq!(stats.high_watermark(), 800);
        assert_eq!(stats.low_watermark(), 200);
    }

    #[test]
    fn phase_2_5_8_stats_utilization() {
        let stats = BackpressureStats::new();
        stats.capacity.store(100, Ordering::SeqCst);
        stats.current_size.store(50, Ordering::SeqCst);
        assert!((stats.utilization() - 0.5).abs() < 0.001);

        stats.current_size.store(75, Ordering::SeqCst);
        assert!((stats.utilization() - 0.75).abs() < 0.001);

        // capacity = 0 时返回 0.0
        stats.capacity.store(0, Ordering::SeqCst);
        assert_eq!(stats.utilization(), 0.0);
    }

    #[test]
    fn phase_2_5_8_stats_snapshot() {
        let stats = BackpressureStats::new();
        stats.capacity.store(100, Ordering::SeqCst);
        stats.current_size.store(50, Ordering::SeqCst);
        stats.total_pushed.store(100, Ordering::SeqCst);
        stats.total_popped.store(50, Ordering::SeqCst);

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.current_size, 50);
        assert_eq!(snapshot.capacity, 100);
        assert_eq!(snapshot.total_pushed, 100);
        assert_eq!(snapshot.total_popped, 50);
        assert_eq!(snapshot.state, BackpressureState::Normal);
        assert!((snapshot.utilization - 0.5).abs() < 0.001);
    }

    // =================================================================
    // Part 4: BoundedEventQueue 基础（Block 策略）
    // =================================================================

    #[test]
    fn phase_2_5_8_queue_new_initial_state() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
        assert!(!queue.is_full());
        assert_eq!(queue.capacity(), 10);
        assert_eq!(queue.high_watermark(), 8);
        assert_eq!(queue.low_watermark(), 2);
        assert_eq!(queue.strategy(), BackpressureStrategy::Block);
        assert!(!queue.is_closed());
        assert_eq!(queue.callback_count(), 0);
    }

    #[test]
    fn phase_2_5_8_queue_push_pop_basic() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        let event = make_event(1);
        queue.push(event).unwrap();

        assert_eq!(queue.len(), 1);
        assert!(!queue.is_empty());
        assert!(!queue.is_full());

        let popped = queue.pop();
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().lsn, 1);
        assert!(queue.is_empty());
    }

    #[test]
    fn phase_2_5_8_queue_fifo_order() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        for lsn in 1..=5 {
            queue.push(make_event(lsn)).unwrap();
        }
        for lsn in 1..=5 {
            let event = queue.pop().unwrap();
            assert_eq!(event.lsn, lsn, "FIFO order violated");
        }
    }

    #[test]
    fn phase_2_5_8_queue_try_pop_empty_returns_none() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        assert!(queue.try_pop().is_none());
    }

    #[test]
    fn phase_2_5_8_queue_try_push_below_capacity() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        let result = queue.try_push(make_event(1));
        assert!(result.is_ok());
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn phase_2_5_8_queue_stats_after_push_pop() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        queue.push(make_event(1)).unwrap();
        queue.push(make_event(2)).unwrap();
        queue.push(make_event(3)).unwrap();

        let stats = queue.stats();
        assert_eq!(stats.current_size, 3);
        assert_eq!(stats.total_pushed, 3);
        assert_eq!(stats.total_popped, 0);
        assert_eq!(stats.peak_size, 3);

        queue.pop();
        let stats = queue.stats();
        assert_eq!(stats.current_size, 2);
        assert_eq!(stats.total_popped, 1);
        // peak 不下降
        assert_eq!(stats.peak_size, 3);
    }

    // =================================================================
    // Part 5: 水位线触发与解除
    // =================================================================

    #[test]
    fn phase_2_5_8_queue_backpressure_triggers_at_high_watermark() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        // capacity=10, high=8, low=2
        // 推送 8 个事件（达到 high_watermark）应触发背压
        for lsn in 1..=7 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert!(!queue.stats().is_backpressure_active());
        assert_eq!(queue.stats().backpressure_count, 0);

        // 第 8 个触发背压
        queue.push(make_event(8)).unwrap();
        assert!(queue.stats().is_backpressure_active());
        assert_eq!(queue.stats().backpressure_count, 1);
    }

    #[test]
    fn phase_2_5_8_queue_backpressure_releases_at_low_watermark() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        // 推送到 high_watermark 触发背压
        for lsn in 1..=8 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert!(queue.stats().is_backpressure_active());

        // 弹出到 low_watermark 以下解除背压
        for _ in 0..6 {
            queue.pop();
        }
        // 8 - 6 = 2 == low_watermark，应解除
        assert!(!queue.stats().is_backpressure_active());
        assert_eq!(queue.stats().backpressure_count, 1);
    }

    #[test]
    fn phase_2_5_8_queue_backpressure_hysteresis_no_flicker() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        // 推送到 high_watermark 触发背压
        for lsn in 1..=8 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert!(queue.stats().is_backpressure_active());
        assert_eq!(queue.stats().backpressure_count, 1);

        // 弹出 1 个（7 = high - 1），不应解除（必须 < low=2）
        queue.pop();
        assert!(queue.stats().is_backpressure_active());

        // 再推 1 个（回到 8），不应再次触发（已是 BackpressureActive）
        queue.push(make_event(9)).unwrap();
        assert!(queue.stats().is_backpressure_active());
        assert_eq!(queue.stats().backpressure_count, 1); // 仍然 1 次

        // 弹出到 low_watermark 以下（8 -> 2 -> 1）
        for _ in 0..7 {
            queue.pop();
        }
        assert!(!queue.stats().is_backpressure_active());
    }

    #[test]
    fn phase_2_5_8_queue_backpressure_multiple_cycles() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        // 循环 3 次：每轮从空队列开始，推 8 触发，弹 8 清空（0 <= low 解除）
        for cycle in 0..3u64 {
            // 确保每轮开始队列为空
            assert_eq!(queue.len(), 0);
            for i in 0..8 {
                queue.push(make_event(cycle * 100 + i)).unwrap();
            }
            assert!(queue.stats().is_backpressure_active());
            // 弹出全部 8 个，size=0 <= low_watermark=2 解除
            for _ in 0..8 {
                queue.pop();
            }
            assert!(!queue.stats().is_backpressure_active());
        }
        // 3 次触发
        assert_eq!(queue.stats().backpressure_count, 3);
    }

    // =================================================================
    // Part 6: Reject 策略
    // =================================================================

    #[test]
    fn phase_2_5_8_reject_strategy_rejects_when_full() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Reject));
        // 推满 10 个
        for lsn in 1..=10 {
            assert!(queue.push(make_event(lsn)).is_ok());
        }
        // 第 11 个应被拒绝
        let result = queue.push(make_event(11));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, BackpressureError::BufferFull { .. }));
        assert_eq!(queue.len(), 10);
        assert_eq!(queue.stats().total_rejected, 1);
    }

    #[test]
    fn phase_2_5_8_reject_strategy_try_push_rejects() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Reject));
        for lsn in 1..=10 {
            queue.try_push(make_event(lsn)).unwrap();
        }
        let result = queue.try_push(make_event(11));
        assert!(result.is_err());
        assert_eq!(queue.stats().total_rejected, 1);
    }

    #[test]
    fn phase_2_5_8_reject_strategy_pop_frees_space() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Reject));
        for lsn in 1..=10 {
            queue.push(make_event(lsn)).unwrap();
        }
        // 弹出 1 个，再推 1 个应成功
        queue.pop();
        assert!(queue.push(make_event(11)).is_ok());
        assert_eq!(queue.len(), 10);
    }

    // =================================================================
    // Part 7: DropOldest 策略
    // =================================================================

    #[test]
    fn phase_2_5_8_drop_oldest_strategy_drops_oldest() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::DropOldest));
        // 推满 10 个
        for lsn in 1..=10 {
            queue.push(make_event(lsn)).unwrap();
        }
        // 第 11 个：丢弃最旧（lsn=1），入队新事件
        queue.push(make_event(11)).unwrap();
        assert_eq!(queue.len(), 10);
        assert_eq!(queue.stats().total_dropped, 1);

        // 弹出验证：最旧的应该是 lsn=2
        let first = queue.pop().unwrap();
        assert_eq!(first.lsn, 2);

        // 最新的是 lsn=11
        let events: Vec<ChangeEvent> = queue.drain(10);
        let last = events.last().unwrap();
        assert_eq!(last.lsn, 11);
    }

    #[test]
    fn phase_2_5_8_drop_oldest_strategy_try_push_drops() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::DropOldest));
        for lsn in 1..=10 {
            queue.try_push(make_event(lsn)).unwrap();
        }
        // try_push 第 11 个：丢弃最旧
        queue.try_push(make_event(11)).unwrap();
        assert_eq!(queue.stats().total_dropped, 1);
        assert_eq!(queue.len(), 10);
    }

    #[test]
    fn phase_2_5_8_drop_oldest_multiple_overflow() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::DropOldest));
        // 推 15 个（capacity=10，丢弃 5 个最旧的）
        for lsn in 1..=15 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert_eq!(queue.len(), 10);
        assert_eq!(queue.stats().total_dropped, 5);

        // 最旧的是 lsn=6
        let first = queue.pop().unwrap();
        assert_eq!(first.lsn, 6);
    }

    // =================================================================
    // Part 8: Signal 策略
    // =================================================================

    #[test]
    fn phase_2_5_8_signal_strategy_allows_overflow() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Signal));
        // 推满 10 个
        for lsn in 1..=10 {
            queue.push(make_event(lsn)).unwrap();
        }
        // Signal 策略：第 11 个仍入队（可能超过 capacity）
        queue.push(make_event(11)).unwrap();
        assert_eq!(queue.len(), 11);
        assert_eq!(queue.stats().total_dropped, 0);
        assert_eq!(queue.stats().total_rejected, 0);
    }

    #[test]
    fn phase_2_5_8_signal_strategy_triggers_backpressure() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Signal));
        // 推 8 个触发背压（high_watermark=8）
        for lsn in 1..=8 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert!(queue.stats().is_backpressure_active());
        // 但事件仍能入队
        queue.push(make_event(9)).unwrap();
        assert_eq!(queue.len(), 9);
    }

    // =================================================================
    // Part 9: 回调机制
    // =================================================================

    #[test]
    fn phase_2_5_8_callback_register_unregister() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        let callback = Arc::new(CountingCallback::new());
        assert_eq!(queue.callback_count(), 0);

        queue.register_callback(callback.clone());
        assert_eq!(queue.callback_count(), 1);

        // 重复注册相同指针的 callback 应被忽略
        queue.register_callback(callback.clone());
        assert_eq!(queue.callback_count(), 1);

        // 注销
        assert!(queue.unregister_callback(&callback));
        assert_eq!(queue.callback_count(), 0);

        // 再次注销返回 false
        assert!(!queue.unregister_callback(&callback));
    }

    #[test]
    fn phase_2_5_8_callback_backpressure_start_end() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        let callback = Arc::new(CountingCallback::new());
        queue.register_callback(callback.clone());

        // 推 8 个触发背压
        for lsn in 1..=8 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert_eq!(callback.start_count(), 1);
        assert!(callback.last_start_snapshot().is_some());

        // 弹出 7 个解除背压（剩 1 < low=2）
        for _ in 0..7 {
            queue.pop();
        }
        assert_eq!(callback.end_count(), 1);
        assert!(callback.last_end_snapshot().is_some());
    }

    #[test]
    fn phase_2_5_8_callback_event_dropped_drop_oldest() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::DropOldest));
        let callback = Arc::new(CountingCallback::new());
        queue.register_callback(callback.clone());

        for lsn in 1..=15 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert_eq!(callback.dropped_count(), 5);
    }

    #[test]
    fn phase_2_5_8_callback_event_rejected_reject() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Reject));
        let callback = Arc::new(CountingCallback::new());
        queue.register_callback(callback.clone());

        for lsn in 1..=10 {
            queue.push(make_event(lsn)).unwrap();
        }
        // 第 11 个被拒绝
        let _ = queue.push(make_event(11));
        assert_eq!(callback.rejected_count(), 1);
    }

    #[test]
    fn phase_2_5_8_callback_multiple_callbacks() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        let cb1 = Arc::new(CountingCallback::new());
        let cb2 = Arc::new(CountingCallback::new());
        queue.register_callback(cb1.clone());
        queue.register_callback(cb2.clone());

        for lsn in 1..=8 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert_eq!(cb1.start_count(), 1);
        assert_eq!(cb2.start_count(), 1);
    }

    #[test]
    fn phase_2_5_8_callback_noop_does_not_panic() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        let callback = Arc::new(NoopCallback);
        queue.register_callback(callback);

        // 不应 panic
        for lsn in 1..=8 {
            queue.push(make_event(lsn)).unwrap();
        }
        for _ in 0..7 {
            queue.pop();
        }
    }

    // =================================================================
    // Part 10: 关闭队列
    // =================================================================

    #[test]
    fn phase_2_5_8_close_queue_rejects_push() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        queue.close();
        assert!(queue.is_closed());

        let result = queue.push(make_event(1));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            BackpressureError::QueueClosed
        ));
    }

    #[test]
    fn phase_2_5_8_close_queue_pop_remaining() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        for lsn in 1..=5 {
            queue.push(make_event(lsn)).unwrap();
        }
        queue.close();

        // 仍可弹出剩余事件
        for lsn in 1..=5 {
            let event = queue.pop().unwrap();
            assert_eq!(event.lsn, lsn);
        }
        // 队列空且已关闭，pop 返回 None
        assert!(queue.pop().is_none());
    }

    #[test]
    fn phase_2_5_8_close_queue_try_push_rejected() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        queue.close();
        let result = queue.try_push(make_event(1));
        assert!(matches!(
            result.unwrap_err(),
            BackpressureError::QueueClosed
        ));
    }

    // =================================================================
    // Part 11: drain 批量弹出
    // =================================================================

    #[test]
    fn phase_2_5_8_drain_pops_up_to_n() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        for lsn in 1..=5 {
            queue.push(make_event(lsn)).unwrap();
        }

        let events = queue.drain(3);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].lsn, 1);
        assert_eq!(events[1].lsn, 2);
        assert_eq!(events[2].lsn, 3);
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn phase_2_5_8_drain_more_than_available() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        for lsn in 1..=3 {
            queue.push(make_event(lsn)).unwrap();
        }

        let events = queue.drain(10);
        assert_eq!(events.len(), 3);
        assert!(queue.is_empty());
    }

    #[test]
    fn phase_2_5_8_drain_zero_returns_empty() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        queue.push(make_event(1)).unwrap();

        let events = queue.drain(0);
        assert!(events.is_empty());
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn phase_2_5_8_drain_releases_backpressure() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        // 推 8 个触发背压
        for lsn in 1..=8 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert!(queue.stats().is_backpressure_active());

        // drain 7 个（剩 1 < low=2）解除背压
        let events = queue.drain(7);
        assert_eq!(events.len(), 7);
        assert!(!queue.stats().is_backpressure_active());
    }

    // =================================================================
    // Part 12: WalBackpressureSignal
    // =================================================================

    #[test]
    fn phase_2_5_8_signal_default_full_speed() {
        let signal = WalBackpressureSignal::new();
        assert_eq!(signal.recommended_tps(), u64::MAX);
        assert!(!signal.is_backpressure_active());
    }

    #[test]
    fn phase_2_5_8_signal_trigger_release() {
        let signal = WalBackpressureSignal::new();
        signal.trigger(100_000);
        assert_eq!(signal.recommended_tps(), 100_000);
        assert!(signal.is_backpressure_active());
        assert_eq!(signal.trigger_count(), 1);

        signal.release();
        assert_eq!(signal.recommended_tps(), u64::MAX);
        assert!(!signal.is_backpressure_active());
        assert_eq!(signal.release_count(), 1);
    }

    #[test]
    fn phase_2_5_8_signal_clone_shared_state() {
        let signal = WalBackpressureSignal::new();
        let cloned = signal.clone();

        signal.trigger(50_000);
        // clone 共享同一内部状态
        assert_eq!(cloned.recommended_tps(), 50_000);
        assert!(cloned.is_backpressure_active());
    }

    #[test]
    fn phase_2_5_8_signal_set_recommended_tps() {
        let signal = WalBackpressureSignal::new();
        signal.set_recommended_tps(500_000);
        assert_eq!(signal.recommended_tps(), 500_000);
        assert!(signal.is_backpressure_active());

        signal.set_recommended_tps(0);
        assert_eq!(signal.recommended_tps(), 0);
        assert!(signal.is_backpressure_active());
    }

    // =================================================================
    // Part 13: WalBackpressureCallback
    // =================================================================

    #[test]
    fn phase_2_5_8_wal_callback_triggers_on_backpressure() {
        let signal = WalBackpressureSignal::new();
        let callback = WalBackpressureCallback::new(signal.clone(), 100_000);
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        queue.register_callback(Arc::new(callback));

        // 推 8 个触发背压
        for lsn in 1..=8 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert_eq!(signal.recommended_tps(), 100_000);
        assert!(signal.is_backpressure_active());
        assert_eq!(signal.trigger_count(), 1);

        // 弹 7 个解除背压
        for _ in 0..7 {
            queue.pop();
        }
        assert_eq!(signal.recommended_tps(), u64::MAX);
        assert!(!signal.is_backpressure_active());
        assert_eq!(signal.release_count(), 1);
    }

    #[test]
    fn phase_2_5_8_wal_callback_multiple_cycles() {
        let signal = WalBackpressureSignal::new();
        let callback = WalBackpressureCallback::new(signal.clone(), 50_000);
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        queue.register_callback(Arc::new(callback));

        // 3 轮触发-解除：每轮从空队列开始，推 8 触发，弹 8 清空（size=0 <= low=2 解除）
        for _ in 0..3 {
            assert_eq!(queue.len(), 0);
            for lsn in 1..=8 {
                queue.push(make_event(lsn)).unwrap();
            }
            for _ in 0..8 {
                queue.pop();
            }
        }
        assert_eq!(signal.trigger_count(), 3);
        assert_eq!(signal.release_count(), 3);
    }

    // =================================================================
    // Part 14: SimulatedWalCommitter
    // =================================================================

    #[test]
    fn phase_2_5_8_simulated_committer_no_backpressure() {
        let signal = WalBackpressureSignal::new();
        let committer = SimulatedWalCommitter::new(signal, 1_000_000);

        for _ in 0..100 {
            let delay = committer.commit_one();
            assert_eq!(delay, 0); // 无背压，无延迟
        }
        assert_eq!(committer.total_committed(), 100);
        assert_eq!(committer.backpressure_delayed(), 0);
        assert!(!committer.is_backpressure_active());
    }

    #[test]
    fn phase_2_5_8_simulated_committer_with_backpressure() {
        let signal = WalBackpressureSignal::new();
        let committer = SimulatedWalCommitter::new(signal.clone(), 1_000_000);

        signal.trigger(100_000); // 推荐速度 = 10% 全速
        assert!(committer.is_backpressure_active());

        for _ in 0..100 {
            committer.commit_one();
        }
        assert_eq!(committer.total_committed(), 100);
        assert_eq!(committer.backpressure_delayed(), 100);
    }

    #[test]
    fn phase_2_5_8_simulated_committer_zero_tps() {
        let signal = WalBackpressureSignal::new();
        let committer = SimulatedWalCommitter::new(signal.clone(), 1_000_000);

        signal.set_recommended_tps(0);
        let delay = committer.commit_one();
        assert_eq!(delay, 1); // 完全停止时延迟 1ms
        assert_eq!(committer.backpressure_delayed(), 1);
    }

    // =================================================================
    // Part 15: 辅助构造函数
    // =================================================================

    #[test]
    fn phase_2_5_8_with_default_config() {
        let queue = BoundedEventQueue::with_default_config();
        assert_eq!(queue.capacity(), 10_000);
        assert_eq!(queue.strategy(), BackpressureStrategy::Block);
    }

    #[test]
    fn phase_2_5_8_with_block_strategy() {
        let queue = BoundedEventQueue::with_block_strategy(100, 80, 20);
        assert_eq!(queue.capacity(), 100);
        assert_eq!(queue.strategy(), BackpressureStrategy::Block);
    }

    #[test]
    fn phase_2_5_8_with_drop_oldest_strategy() {
        let queue = BoundedEventQueue::with_drop_oldest_strategy(100, 80, 20);
        assert_eq!(queue.strategy(), BackpressureStrategy::DropOldest);
    }

    #[test]
    fn phase_2_5_8_with_reject_strategy() {
        let queue = BoundedEventQueue::with_reject_strategy(100, 80, 20);
        assert_eq!(queue.strategy(), BackpressureStrategy::Reject);
    }

    #[test]
    fn phase_2_5_8_with_signal_strategy() {
        let queue = BoundedEventQueue::with_signal_strategy(100, 80, 20);
        assert_eq!(queue.strategy(), BackpressureStrategy::Signal);
    }

    // =================================================================
    // Part 16: 并发安全 — 多生产者多消费者
    // =================================================================

    #[test]
    fn phase_2_5_8_concurrent_single_producer_single_consumer() {
        let queue = Arc::new(BoundedEventQueue::with_block_strategy(100, 80, 20));
        let total_events = 10_000u64;

        // 生产者
        let producer_queue = queue.clone();
        let producer = thread::spawn(move || {
            for lsn in 1..=total_events {
                producer_queue.push(make_event(lsn)).unwrap();
            }
        });

        // 消费者（故意慢一点：每 50 个事件 sleep 1us，确保背压必然触发）
        let consumer_queue = queue.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0u64;
            let mut last_lsn = 0u64;
            while count < total_events {
                if let Some(event) = consumer_queue.pop() {
                    // FIFO 顺序：lsn 单调递增
                    assert!(event.lsn > last_lsn, "FIFO order violated");
                    last_lsn = event.lsn;
                    count += 1;
                    // 每 50 个事件 sleep 1us，给 producer 机会填满队列触发背压
                    if count.is_multiple_of(50) {
                        thread::sleep(Duration::from_micros(1));
                    }
                }
            }
            count
        });

        producer.join().unwrap();
        let consumed = consumer.join().unwrap();
        assert_eq!(consumed, total_events);

        let stats = queue.stats();
        assert_eq!(stats.total_pushed, total_events);
        assert_eq!(stats.total_popped, total_events);
        // 背压至少触发过 1 次（生产速度 >> 消费速度）
        assert!(stats.backpressure_count > 0, "backpressure should trigger");
        // 缓冲区始终 <= capacity
        assert!(
            stats.peak_size <= 100,
            "peak_size should not exceed capacity"
        );
    }

    #[test]
    fn phase_2_5_8_concurrent_multi_producer_single_consumer() {
        let queue = Arc::new(BoundedEventQueue::with_block_strategy(50, 40, 10));
        let producers_count = 4;
        let events_per_producer = 1_000u64;
        let total_events = producers_count * events_per_producer;

        let mut producer_handles = Vec::new();
        for producer_id in 0..producers_count {
            let queue = queue.clone();
            let handle = thread::spawn(move || {
                for i in 0..events_per_producer {
                    let lsn = producer_id * events_per_producer + i + 1;
                    queue.push(make_event(lsn)).unwrap();
                }
            });
            producer_handles.push(handle);
        }

        let consumer_queue = queue.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0u64;
            while count < total_events {
                if consumer_queue.pop().is_some() {
                    count += 1;
                }
            }
            count
        });

        for handle in producer_handles {
            handle.join().unwrap();
        }
        let consumed = consumer.join().unwrap();
        assert_eq!(consumed, total_events);

        let stats = queue.stats();
        assert_eq!(stats.total_pushed, total_events);
        assert_eq!(stats.total_popped, total_events);
        assert!(stats.peak_size <= 50);
    }

    #[test]
    fn phase_2_5_8_concurrent_single_producer_multi_consumer() {
        let queue = Arc::new(BoundedEventQueue::with_block_strategy(50, 40, 10));
        let total_events = 5_000u64;
        let consumers_count = 4;

        let producer_queue = queue.clone();
        let producer = thread::spawn(move || {
            for lsn in 1..=total_events {
                producer_queue.push(make_event(lsn)).unwrap();
            }
        });

        let mut consumer_handles = Vec::new();
        let consumed_counts = Arc::new(Mutex::new(vec![0u64; consumers_count]));
        for consumer_id in 0..consumers_count {
            let queue = queue.clone();
            let consumed_counts = consumed_counts.clone();
            let handle = thread::spawn(move || {
                let mut my_count = 0u64;
                // 持续 pop 直到队列关闭且为空（pop 返回 None）
                while let Some(event) = queue.pop() {
                    my_count += 1;
                    // 拿到最后一个事件后主动结束（其他 consumer 会通过 close 退出）
                    if event.lsn == total_events {
                        break;
                    }
                }
                consumed_counts.lock()[consumer_id] = my_count;
            });
            consumer_handles.push(handle);
        }

        producer.join().unwrap();
        // 关闭队列：唤醒所有阻塞在 pop 上的 consumer，让它们拿到 None 后退出
        queue.close();
        for handle in consumer_handles {
            handle.join().unwrap();
        }

        let total_consumed: u64 = consumed_counts.lock().iter().sum();
        assert_eq!(total_consumed, total_events);
    }

    #[test]
    fn phase_2_5_8_concurrent_multi_producer_multi_consumer() {
        let queue = Arc::new(BoundedEventQueue::with_block_strategy(100, 80, 20));
        let producers_count = 4;
        let consumers_count = 4;
        let events_per_producer = 500u64;
        let total_events = producers_count * events_per_producer;

        let mut producer_handles = Vec::new();
        for producer_id in 0..producers_count {
            let queue = queue.clone();
            let handle = thread::spawn(move || {
                for i in 0..events_per_producer {
                    let lsn = producer_id * events_per_producer + i + 1;
                    queue.push(make_event(lsn)).unwrap();
                }
            });
            producer_handles.push(handle);
        }

        let consumed_count = Arc::new(AtomicU64::new(0));
        let mut consumer_handles = Vec::new();
        for _ in 0..consumers_count {
            let queue = queue.clone();
            let consumed_count = consumed_count.clone();
            let handle = thread::spawn(move || {
                // 持续 pop 直到队列关闭且为空（pop 返回 None）
                while let Some(_event) = queue.pop() {
                    let prev = consumed_count.fetch_add(1, Ordering::SeqCst);
                    if prev + 1 >= total_events {
                        break;
                    }
                }
            });
            consumer_handles.push(handle);
        }

        for handle in producer_handles {
            handle.join().unwrap();
        }
        // 关闭队列：唤醒所有阻塞在 pop 上的 consumer，让它们拿到 None 后退出
        queue.close();
        for handle in consumer_handles {
            handle.join().unwrap();
        }

        assert_eq!(consumed_count.load(Ordering::SeqCst), total_events);
        let stats = queue.stats();
        assert!(stats.peak_size <= 100);
    }

    // =================================================================
    // Part 17: Stress 测试 — 生产速度 >> 消费速度
    // =================================================================

    #[test]
    fn phase_2_5_8_stress_production_faster_than_consumption_block() {
        // 生产速度 1000000 TPS，消费速度 100000 TPS
        // Block 策略：缓冲区满时阻塞生产者，不 OOM
        let queue = Arc::new(BoundedEventQueue::with_block_strategy(1000, 800, 200));
        let total_events = 100_000u64;

        let producer_queue = queue.clone();
        let producer = thread::spawn(move || {
            for lsn in 1..=total_events {
                producer_queue.push(make_event(lsn)).unwrap();
            }
        });

        // 消费者：每消费 100 个事件 sleep 1us（模拟慢消费）
        let consumer_queue = queue.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0u64;
            while count < total_events {
                if let Some(_event) = consumer_queue.pop() {
                    count += 1;
                    if count.is_multiple_of(100) {
                        thread::sleep(Duration::from_micros(1));
                    }
                }
            }
            count
        });

        producer.join().unwrap();
        let consumed = consumer.join().unwrap();
        assert_eq!(consumed, total_events);

        let stats = queue.stats();
        assert_eq!(stats.total_pushed, total_events);
        assert_eq!(stats.total_popped, total_events);
        assert!(stats.backpressure_count > 0, "backpressure must trigger");
        assert!(
            stats.peak_size <= 1000,
            "peak_size must not exceed capacity"
        );
        assert_eq!(stats.total_dropped, 0); // Block 策略不丢弃
        assert_eq!(stats.total_rejected, 0); // Block 策略不拒绝
    }

    #[test]
    fn phase_2_5_8_stress_production_faster_than_consumption_drop_oldest() {
        // DropOldest 策略：丢弃最旧事件，不阻塞
        let queue = Arc::new(BoundedEventQueue::with_drop_oldest_strategy(1000, 800, 200));
        let total_events = 100_000u64;

        let producer_queue = queue.clone();
        let producer = thread::spawn(move || {
            let mut pushed = 0u64;
            for lsn in 1..=total_events {
                if producer_queue.push(make_event(lsn)).is_ok() {
                    pushed += 1;
                }
            }
            pushed
        });

        // 消费者：用 try_pop + 轮询，避免阻塞 pop 导致 deadline 检查失效
        let consumer_queue = queue.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0u64;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(_event) = consumer_queue.try_pop() {
                    count += 1;
                } else {
                    // 队列空，检查 producer 是否完成且队列已空
                    if consumer_queue.stats().total_pushed >= total_events
                        && consumer_queue.is_empty()
                    {
                        break;
                    }
                    if Instant::now() > deadline {
                        break;
                    }
                    thread::yield_now();
                }
            }
            count
        });

        let pushed = producer.join().unwrap();
        let consumed = consumer.join().unwrap();

        let stats = queue.stats();
        // DropOldest 策略下，丢弃数量 = pushed - consumed
        assert_eq!(stats.total_dropped, pushed - consumed);
        assert!(stats.peak_size <= 1000);
        // 消费数 + 丢弃数 = 推送数
        assert_eq!(consumed + stats.total_dropped, pushed);
    }

    #[test]
    fn phase_2_5_8_stress_reject_strategy_no_oom() {
        // Reject 策略：满时拒绝，调用方可重试或丢弃
        let queue = Arc::new(BoundedEventQueue::with_reject_strategy(1000, 800, 200));
        let total_events = 100_000u64;

        let producer_queue = queue.clone();
        let producer = thread::spawn(move || {
            let mut pushed = 0u64;
            let mut rejected = 0u64;
            for lsn in 1..=total_events {
                loop {
                    match producer_queue.push(make_event(lsn)) {
                        Ok(()) => {
                            pushed += 1;
                            break;
                        }
                        Err(BackpressureError::BufferFull { .. }) => {
                            rejected += 1;
                            // 短暂让出 CPU，给 consumer 机会 pop
                            thread::yield_now();
                        }
                        Err(BackpressureError::QueueClosed) => break,
                        Err(BackpressureError::InvalidConfig { .. }) => break,
                    }
                }
            }
            (pushed, rejected)
        });

        // 消费者：故意慢一点（每 50 个事件 sleep 1us），确保队列会满触发拒绝
        let consumer_queue = queue.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0u64;
            while count < total_events {
                if consumer_queue.pop().is_some() {
                    count += 1;
                    if count.is_multiple_of(50) {
                        thread::sleep(Duration::from_micros(1));
                    }
                }
            }
            count
        });

        let (pushed, _rejected) = producer.join().unwrap();
        let consumed = consumer.join().unwrap();
        assert_eq!(pushed, consumed); // 全部推送的都被消费

        let stats = queue.stats();
        assert!(stats.peak_size <= 1000);
        // Reject 策略下，consumer 慢时队列会满，触发拒绝
        assert!(
            stats.total_rejected > 0,
            "some events must be rejected (total_rejected={})",
            stats.total_rejected
        );
    }

    // =================================================================
    // Part 18: Stress 测试 — 1M 事件
    // =================================================================

    #[test]
    fn phase_2_5_8_stress_1m_events_block_strategy() {
        let queue = Arc::new(BoundedEventQueue::with_block_strategy(5000, 4000, 1000));
        let total_events = 1_000_000u64;

        let producer_queue = queue.clone();
        let producer = thread::spawn(move || {
            for lsn in 1..=total_events {
                producer_queue.push(make_event(lsn)).unwrap();
            }
        });

        // 消费者：每 10 个事件 sleep 1us，确保生产速度 >> 消费速度，背压必然触发
        let consumer_queue = queue.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0u64;
            while count < total_events {
                if consumer_queue.pop().is_some() {
                    count += 1;
                    if count.is_multiple_of(10) {
                        thread::sleep(Duration::from_micros(1));
                    }
                }
            }
            count
        });

        producer.join().unwrap();
        let consumed = consumer.join().unwrap();
        assert_eq!(consumed, total_events);

        let stats = queue.stats();
        assert_eq!(stats.total_pushed, total_events);
        assert_eq!(stats.total_popped, total_events);
        assert!(stats.backpressure_count > 0);
        assert!(stats.peak_size <= 5000);
    }

    // =================================================================
    // Part 19: Stress 测试 — 模拟 1M TPS 生产 / 100K TPS 消费
    // =================================================================

    #[test]
    fn phase_2_5_8_stress_simulated_1m_tps_production_100k_tps_consumption() {
        // 模拟：1M TPS 生产，100K TPS 消费
        // 期望：背压触发，WAL 降速，不 OOM
        let queue = Arc::new(BoundedEventQueue::with_block_strategy(10_000, 8_000, 2_000));
        let signal = WalBackpressureSignal::new();
        let callback = WalBackpressureCallback::new(signal.clone(), 100_000); // 背压时降到 100K TPS
        queue.register_callback(Arc::new(callback));

        let total_events = 100_000u64; // 模拟 0.1 秒的生产

        let producer_queue = queue.clone();
        let producer = thread::spawn(move || {
            for lsn in 1..=total_events {
                producer_queue.push(make_event(lsn)).unwrap();
            }
        });

        // 消费者：每 10 个事件 sleep 1us（模拟 100K TPS）
        let consumer_queue = queue.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0u64;
            while count < total_events {
                if consumer_queue.pop().is_some() {
                    count += 1;
                    if count.is_multiple_of(10) {
                        thread::sleep(Duration::from_micros(1));
                    }
                }
            }
            count
        });

        producer.join().unwrap();
        let consumed = consumer.join().unwrap();
        assert_eq!(consumed, total_events);

        let stats = queue.stats();
        assert!(stats.backpressure_count > 0, "backpressure must trigger");
        assert!(stats.peak_size <= 10_000, "must not exceed capacity");
        assert_eq!(stats.total_dropped, 0);
        assert_eq!(stats.total_rejected, 0);
        assert!(signal.trigger_count() > 0);
        assert!(signal.release_count() > 0);
        // 背压最终应解除（队列空）
        assert!(!signal.is_backpressure_active());
    }

    // =================================================================
    // Part 20: 端到端 — ChangeEvent 流 + 背压队列 + WAL 信号
    // =================================================================

    #[test]
    fn phase_2_5_8_e2e_change_event_stream_with_backpressure() {
        // 模拟：CDC 事件流 → BoundedEventQueue → 消费者
        // 当生产速度 > 消费速度时，背压触发，WalBackpressureSignal 通知 WAL 降速
        let queue = Arc::new(BoundedEventQueue::with_block_strategy(1000, 800, 200));
        let signal = WalBackpressureSignal::new();
        let callback = WalBackpressureCallback::new(signal.clone(), 100_000); // 降到 100K TPS
        queue.register_callback(Arc::new(callback));

        let total_events = 50_000u64;

        // 生产者：模拟 CDC 引擎推送 ChangeEvent
        let producer_queue = queue.clone();
        let producer = thread::spawn(move || {
            for lsn in 1..=total_events {
                let event = ChangeEvent::insert(1, lsn, 42, vec![lsn as u8], lsn);
                producer_queue.push(event).unwrap();
            }
        });

        // 消费者：模拟下游消费者（较慢）
        let consumer_queue = queue.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0u64;
            let mut last_lsn = 0u64;
            while count < total_events {
                if let Some(event) = consumer_queue.pop() {
                    // 验证 FIFO 顺序
                    assert!(event.lsn > last_lsn, "FIFO order violated");
                    last_lsn = event.lsn;
                    count += 1;
                    if count.is_multiple_of(50) {
                        thread::sleep(Duration::from_micros(1));
                    }
                }
            }
            count
        });

        producer.join().unwrap();
        let consumed = consumer.join().unwrap();
        assert_eq!(consumed, total_events);

        let stats = queue.stats();
        assert!(stats.backpressure_count > 0, "backpressure must trigger");
        assert!(
            stats.peak_size <= 1000,
            "peak_size must not exceed capacity"
        );
        assert_eq!(stats.total_pushed, total_events);
        assert_eq!(stats.total_popped, total_events);
        assert_eq!(stats.total_dropped, 0);
        assert_eq!(stats.total_rejected, 0);
        assert!(signal.trigger_count() > 0);
        assert!(signal.release_count() > 0);
        assert!(!signal.is_backpressure_active());
    }

    #[test]
    fn phase_2_5_8_e2e_backpressure_with_drop_oldest_and_signal() {
        // DropOldest 策略 + Signal 回调：背压时丢弃旧事件 + 通知 WAL
        let queue = Arc::new(BoundedEventQueue::with_drop_oldest_strategy(500, 400, 100));
        let signal = WalBackpressureSignal::new();
        let callback = WalBackpressureCallback::new(signal.clone(), 50_000);
        queue.register_callback(Arc::new(callback));

        let total_events = 10_000u64;

        let producer_queue = queue.clone();
        let producer = thread::spawn(move || {
            for lsn in 1..=total_events {
                let event = ChangeEvent::insert(1, lsn, 42, vec![lsn as u8], lsn);
                producer_queue.push(event).unwrap();
            }
        });

        // 消费者：用 try_pop + 短 sleep 轮询，避免阻塞导致 deadline 检查失效
        let consumer_queue = queue.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0u64;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(_event) = consumer_queue.try_pop() {
                    count += 1;
                    thread::sleep(Duration::from_micros(10));
                } else {
                    // 队列空，检查 producer 是否完成（通过 total_pushed）
                    if consumer_queue.stats().total_pushed >= total_events
                        && consumer_queue.is_empty()
                    {
                        break;
                    }
                    if Instant::now() > deadline {
                        break;
                    }
                    thread::yield_now();
                }
            }
            count
        });

        producer.join().unwrap();
        let consumed = consumer.join().unwrap();

        let stats = queue.stats();
        assert!(
            stats.total_dropped > 0,
            "DropOldest should drop some events"
        );
        assert!(stats.peak_size <= 500);
        assert!(signal.trigger_count() > 0);
        // consumed + dropped = total
        assert_eq!(consumed + stats.total_dropped, total_events);
    }

    // =================================================================
    // Part 21: 不变量验证
    // =================================================================

    #[test]
    fn phase_2_5_8_invariant_size_never_exceeds_capacity_block() {
        // Block 策略下，队列大小始终 <= capacity
        let queue = Arc::new(BoundedEventQueue::with_block_strategy(100, 80, 20));
        let total_events = 10_000u64;

        let queue_clone = queue.clone();
        let producer = thread::spawn(move || {
            for lsn in 1..=total_events {
                queue_clone.push(make_event(lsn)).unwrap();
            }
        });

        let queue_clone = queue.clone();
        let stop = Arc::new(AtomicU64::new(0));
        let stop_clone = stop.clone();
        let monitor = thread::spawn(move || {
            while stop_clone.load(Ordering::SeqCst) == 0 {
                let size = queue_clone.len();
                assert!(size <= 100, "size {size} exceeded capacity 100");
                // 用 sleep 而非 yield_now，避免在 Windows 上占用 CPU 导致死锁
                thread::sleep(Duration::from_micros(100));
            }
        });

        let queue_clone = queue.clone();
        let consumer = thread::spawn(move || {
            let mut count = 0u64;
            while count < total_events {
                if queue_clone.pop().is_some() {
                    count += 1;
                }
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();
        stop.store(1, Ordering::SeqCst);
        monitor.join().unwrap();
    }

    #[test]
    fn phase_2_5_8_invariant_size_never_exceeds_capacity_drop_oldest() {
        let queue = BoundedEventQueue::with_drop_oldest_strategy(100, 80, 20);
        // 推 1000 个，DropOldest 保证 size 始终 <= 100
        for lsn in 1..=1000u64 {
            queue.push(make_event(lsn)).unwrap();
            assert!(queue.len() <= 100, "size exceeded capacity at lsn {lsn}");
        }
    }

    #[test]
    fn phase_2_5_8_invariant_size_never_exceeds_capacity_reject() {
        let queue = BoundedEventQueue::with_reject_strategy(100, 80, 20);
        for lsn in 1..=1000u64 {
            let _ = queue.push(make_event(lsn));
            assert!(queue.len() <= 100, "size exceeded capacity at lsn {lsn}");
        }
    }

    #[test]
    fn phase_2_5_8_invariant_backpressure_count_equals_start_callbacks() {
        // Block 策略：背压触发次数 == start_callback 调用次数
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        let callback = Arc::new(CountingCallback::new());
        queue.register_callback(callback.clone());

        // 5 轮触发-解除：每轮从空队列开始，推 8 触发，弹 8 清空
        for cycle in 0..5u64 {
            assert_eq!(queue.len(), 0);
            for i in 0..8 {
                queue.push(make_event(cycle * 100 + i)).unwrap();
            }
            for _ in 0..8 {
                queue.pop();
            }
        }

        assert_eq!(queue.stats().backpressure_count, 5);
        assert_eq!(callback.start_count(), 5);
        assert_eq!(callback.end_count(), 5);
    }

    #[test]
    fn phase_2_5_8_invariant_pushed_minus_popped_equals_current_size() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        for lsn in 1..=5u64 {
            queue.push(make_event(lsn)).unwrap();
        }
        for _ in 0..2 {
            queue.pop();
        }
        let stats = queue.stats();
        assert_eq!(stats.total_pushed - stats.total_popped, stats.current_size);
        assert_eq!(stats.current_size, 3);
    }

    // =================================================================
    // Part 22: 边界条件
    // =================================================================

    #[test]
    fn phase_2_5_8_edge_high_watermark_equals_capacity() {
        // high_watermark = capacity（极端配置，背压只在满时触发）
        let config = BackpressureConfig::new(10, 10, 2, BackpressureStrategy::Block).unwrap();
        let queue = BoundedEventQueue::new(config);
        for lsn in 1..=9 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert!(!queue.stats().is_backpressure_active());

        // 第 10 个（达到 high=10=capacity）触发背压
        queue.push(make_event(10)).unwrap();
        assert!(queue.stats().is_backpressure_active());
    }

    #[test]
    fn phase_2_5_8_edge_low_watermark_one() {
        // low_watermark = 1（最小有效值）
        let config = BackpressureConfig::new(10, 8, 1, BackpressureStrategy::Block).unwrap();
        let queue = BoundedEventQueue::new(config);
        for lsn in 1..=8 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert!(queue.stats().is_backpressure_active());

        // 弹出 7 个，剩 1 == low_watermark，应解除
        for _ in 0..7 {
            queue.pop();
        }
        assert!(!queue.stats().is_backpressure_active());
    }

    #[test]
    fn phase_2_5_8_edge_drain_all() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        for lsn in 1..=5 {
            queue.push(make_event(lsn)).unwrap();
        }
        let events = queue.drain(usize::MAX);
        assert_eq!(events.len(), 5);
        assert!(queue.is_empty());
    }

    #[test]
    fn phase_2_5_8_edge_close_empty_queue() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        queue.close();
        assert!(queue.pop().is_none());
    }

    #[test]
    fn phase_2_5_8_edge_pop_closed_empty_returns_none_immediately() {
        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        queue.close();
        let start = Instant::now();
        let result = queue.pop();
        assert!(result.is_none());
        // 应立即返回，不阻塞
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    // =================================================================
    // Part 23: 回调 panic 隔离
    // =================================================================

    #[test]
    fn phase_2_5_8_callback_panic_isolated_does_not_affect_queue() {
        use std::sync::Mutex as StdMutex;

        struct PanicCallback;
        impl BackpressureCallback for PanicCallback {
            fn on_backpressure_start(&self, _stats: &BackpressureStatsSnapshot) {
                panic!("intentional panic in callback");
            }
        }

        let queue = BoundedEventQueue::new(small_config(BackpressureStrategy::Block));
        let panic_cb = Arc::new(PanicCallback);
        let counting_cb = Arc::new(CountingCallback::new());
        queue.register_callback(panic_cb);
        queue.register_callback(counting_cb.clone());

        // 推 8 个触发背压 — PanicCallback panic 应被隔离
        for lsn in 1..=8 {
            queue.push(make_event(lsn)).unwrap();
        }

        // CountingCallback 仍应被调用
        assert_eq!(counting_cb.start_count(), 1);
        // 队列仍可正常使用
        assert_eq!(queue.len(), 8);

        // 验证：用一个 Mutex 保护的状态，避免 panic 传播
        let flag = Arc::new(StdMutex::new(false));
        let flag_clone = flag.clone();
        struct FlagSetter(Arc<StdMutex<bool>>);
        impl BackpressureCallback for FlagSetter {
            fn on_backpressure_start(&self, _stats: &BackpressureStatsSnapshot) {
                *self.0.lock().unwrap() = true;
            }
        }
        queue.register_callback(Arc::new(FlagSetter(flag_clone)));

        // 弹出再推回，再次触发背压
        for _ in 0..7 {
            queue.pop();
        }
        for lsn in 100..107 {
            queue.push(make_event(lsn)).unwrap();
        }
        assert!(*flag.lock().unwrap());
    }

    // =================================================================
    // Part 24: 大事件压力测试（验证内存不爆）
    // =================================================================

    #[test]
    fn phase_2_5_8_large_events_block_strategy_no_oom() {
        // 大事件（1KB each），Block 策略
        let queue = Arc::new(BoundedEventQueue::with_block_strategy(100, 80, 20));
        let total_events = 10_000u64;
        let payload_size = 1024; // 1KB

        // 使用 barrier 确保生产者先填满队列到高水位，触发背压
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let producer_barrier = barrier.clone();

        let producer_queue = queue.clone();
        let producer = thread::spawn(move || {
            for lsn in 1..=total_events {
                let payload = vec![lsn as u8; payload_size];
                let event = ChangeEvent::insert(1, lsn, 42, payload, lsn);
                producer_queue.push(event).unwrap();
            }
            // 生产者完成后释放消费者
            producer_barrier.wait();
        });

        let consumer_queue = queue.clone();
        let consumer_barrier = barrier.clone();
        let consumer = thread::spawn(move || {
            // 等待一小段时间让生产者先跑，确保队列达到高水位触发背压
            thread::sleep(std::time::Duration::from_millis(5));
            let mut count = 0u64;
            while count < total_events {
                if let Some(event) = consumer_queue.pop() {
                    // 验证事件 payload 大小
                    assert_eq!(event.new_row.as_ref().unwrap().len(), payload_size);
                    count += 1;
                }
            }
            consumer_barrier.wait();
            count
        });

        producer.join().unwrap();
        let consumed = consumer.join().unwrap();
        assert_eq!(consumed, total_events);

        let stats = queue.stats();
        assert!(stats.peak_size <= 100);
        // 背压触发依赖于生产者先于消费者填充队列，
        // 通过初始延迟确保队列达到高水位
        assert!(
            stats.backpressure_count > 0,
            "expected backpressure to trigger, peak_size={}",
            stats.peak_size
        );
    }
}
