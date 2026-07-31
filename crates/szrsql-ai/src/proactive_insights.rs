//! 主动洞察引擎（P6）— 无问智推。
//!
//! 对应 `TDengine启发评估与改进规划.md` §十九。
//!
//! # 设计
//!
//! 陶建辉"无问智推"理念：数据库应主动发现异常并推送，而非等用户查询。
//! 本模块实现一个**无线程、由外部调度器驱动**的主动洞察引擎：
//!
//! - **`InsightEvent`** — 推送事件（rule_id / severity / message / timestamp / context）
//! - **`InsightRule` trait** — 异常检测规则（evaluate 输入快照，输出事件）
//! - **`InsightSink` trait** — 推送目标（notify 接收事件）
//! - **`ProactiveEngine`** — 引擎主结构，注册规则 + 订阅者，`tick` 驱动一轮采集 → 检测 → 推送
//!
//! # 调度模型
//!
//! 引擎**不内置线程**，由外部调度器（tokio 任务 / 定时器 / 手动触发）调用 `tick(snapshot)`。
//! 这样设计的好处：
//! 1. 避免引入线程同步复杂度（保持单线程可测试性）
//! 2. 调度策略可灵活定制（cron / 事件驱动 / 手动）
//! 3. 测试时可直接构造快照触发规则，无需等待真实时间
//!
//! # 内置规则
//!
//! - `SlowQuerySpikeRule` — 慢查询数超过阈值
//! - `DeadlockFrequentRule` — 死锁历史超过阈值
//! - `CapacityUrgentRule` — 容量预测值超过阈值
//! - `ErrorRateHighRule` — 错误查询占比超过阈值
//! - `LockWaitHighRule` — 锁等待事件超过阈值
//!
//! # 去重与限频
//!
//! 同一 rule_id 的事件在 `cooldown_ms` 毫秒内只推送一次，避免告警风暴。

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// =====================================================================
//  事件与快照
// =====================================================================

/// 严重级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// 信息
    Info,
    /// 警告
    Warn,
    /// 严重
    Critical,
}

impl Severity {
    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Critical => "critical",
        }
    }
}

/// 推送事件
#[derive(Debug, Clone)]
pub struct InsightEvent {
    /// 触发规则的 ID
    pub rule_id: String,
    /// 严重级别
    pub severity: Severity,
    /// 事件消息
    pub message: String,
    /// 事件时间戳（UNIX 毫秒）
    pub timestamp_ms: u64,
    /// 上下文键值对（如 "slow_query_count": "15"）
    pub context: Vec<(String, String)>,
}

impl InsightEvent {
    /// 当前时间戳
    pub fn now(rule_id: &str, severity: Severity, message: String) -> Self {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            rule_id: rule_id.to_string(),
            severity,
            message,
            timestamp_ms,
            context: Vec::new(),
        }
    }

    /// 添加上下文
    pub fn with_context(mut self, key: &str, value: &str) -> Self {
        self.context.push((key.to_string(), value.to_string()));
        self
    }
}

/// 运行时快照（规则评估输入）
///
/// 由外部采集器从 `ExecutorBackend::runtime_stats()` 转换得到。
/// 引擎不依赖具体后端类型，仅依赖此快照结构。
#[derive(Debug, Clone, Default)]
pub struct RuntimeSnapshot {
    /// 慢查询数
    pub slow_query_count: usize,
    /// 错误查询数
    pub error_query_count: usize,
    /// 总查询数
    pub total_query_count: usize,
    /// 死锁历史数
    pub deadlock_count: usize,
    /// 锁等待事件总数
    pub lock_wait_events: u64,
    /// 未授予锁数
    pub pending_locks: usize,
    /// 活动事务数
    pub active_transaction_count: usize,
    /// 容量预测值（字节）
    pub predicted_storage_bytes: Option<f64>,
    /// 容量置信度
    pub capacity_confidence: Option<f64>,
}

impl RuntimeSnapshot {
    /// 错误率（0.0 ~ 1.0）
    pub fn error_rate(&self) -> f64 {
        if self.total_query_count == 0 {
            return 0.0;
        }
        self.error_query_count as f64 / self.total_query_count as f64
    }
}

// =====================================================================
//  规则 trait 与内置规则
// =====================================================================

/// 异常检测规则
pub trait InsightRule: Send + Sync {
    /// 规则 ID（唯一标识）
    fn rule_id(&self) -> &str;

    /// 评估快照，返回事件（None 表示未触发）
    fn evaluate(&self, snapshot: &RuntimeSnapshot) -> Option<InsightEvent>;
}

/// 慢查询激增规则
pub struct SlowQuerySpikeRule {
    /// 阈值（慢查询数 >= threshold 触发）
    pub threshold: usize,
}

impl SlowQuerySpikeRule {
    /// 创建规则，默认阈值 10
    pub fn new(threshold: usize) -> Self {
        Self { threshold }
    }
}

impl Default for SlowQuerySpikeRule {
    fn default() -> Self {
        Self::new(10)
    }
}

impl InsightRule for SlowQuerySpikeRule {
    fn rule_id(&self) -> &str {
        "slow_query_spike"
    }

    fn evaluate(&self, snapshot: &RuntimeSnapshot) -> Option<InsightEvent> {
        if snapshot.slow_query_count < self.threshold {
            return None;
        }
        let severity = if snapshot.slow_query_count >= self.threshold * 3 {
            Severity::Critical
        } else {
            Severity::Warn
        };
        let msg = format!(
            "慢查询激增：当前 {} 条（阈值 {}）",
            snapshot.slow_query_count, self.threshold
        );
        Some(
            InsightEvent::now(self.rule_id(), severity, msg)
                .with_context("slow_query_count", &snapshot.slow_query_count.to_string())
                .with_context("threshold", &self.threshold.to_string()),
        )
    }
}

/// 死锁频发规则
pub struct DeadlockFrequentRule {
    /// 阈值（死锁数 >= threshold 触发）
    pub threshold: usize,
}

impl DeadlockFrequentRule {
    /// 创建规则，默认阈值 1
    pub fn new(threshold: usize) -> Self {
        Self { threshold }
    }
}

impl Default for DeadlockFrequentRule {
    fn default() -> Self {
        Self::new(1)
    }
}

impl InsightRule for DeadlockFrequentRule {
    fn rule_id(&self) -> &str {
        "deadlock_frequent"
    }

    fn evaluate(&self, snapshot: &RuntimeSnapshot) -> Option<InsightEvent> {
        if snapshot.deadlock_count < self.threshold {
            return None;
        }
        let severity = if snapshot.deadlock_count >= 3 {
            Severity::Critical
        } else {
            Severity::Warn
        };
        let msg = format!(
            "死锁频发：近期 {} 次（阈值 {}）",
            snapshot.deadlock_count, self.threshold
        );
        Some(
            InsightEvent::now(self.rule_id(), severity, msg)
                .with_context("deadlock_count", &snapshot.deadlock_count.to_string())
                .with_context("threshold", &self.threshold.to_string()),
        )
    }
}

/// 容量告急规则
pub struct CapacityUrgentRule {
    /// 阈值（字节）
    pub threshold_bytes: f64,
}

impl CapacityUrgentRule {
    /// 创建规则，默认 10GB
    pub fn new(threshold_bytes: f64) -> Self {
        Self { threshold_bytes }
    }
}

impl Default for CapacityUrgentRule {
    fn default() -> Self {
        Self::new(10.0 * 1024.0 * 1024.0 * 1024.0)
    }
}

impl InsightRule for CapacityUrgentRule {
    fn rule_id(&self) -> &str {
        "capacity_urgent"
    }

    fn evaluate(&self, snapshot: &RuntimeSnapshot) -> Option<InsightEvent> {
        let predicted = snapshot.predicted_storage_bytes?;
        if predicted < self.threshold_bytes {
            return None;
        }
        let severity = if predicted >= self.threshold_bytes * 2.0 {
            Severity::Critical
        } else {
            Severity::Warn
        };
        let msg = format!(
            "容量告急：预测存储 {:.2} GB（阈值 {:.2} GB）",
            predicted / 1024.0 / 1024.0 / 1024.0,
            self.threshold_bytes / 1024.0 / 1024.0 / 1024.0
        );
        Some(
            InsightEvent::now(self.rule_id(), severity, msg)
                .with_context("predicted_bytes", &predicted.to_string())
                .with_context("threshold_bytes", &self.threshold_bytes.to_string())
                .with_context(
                    "confidence",
                    &format!(
                        "{:.2}",
                        snapshot.capacity_confidence.unwrap_or(0.0)
                    ),
                ),
        )
    }
}

/// 错误率过高规则
pub struct ErrorRateHighRule {
    /// 阈值（0.0 ~ 1.0）
    pub threshold: f64,
    /// 最小总查询数（避免低样本误报）
    pub min_total_queries: usize,
}

impl ErrorRateHighRule {
    /// 创建规则，默认阈值 0.1（10%），最小样本 100
    pub fn new(threshold: f64, min_total_queries: usize) -> Self {
        Self {
            threshold,
            min_total_queries,
        }
    }
}

impl Default for ErrorRateHighRule {
    fn default() -> Self {
        Self::new(0.1, 100)
    }
}

impl InsightRule for ErrorRateHighRule {
    fn rule_id(&self) -> &str {
        "error_rate_high"
    }

    fn evaluate(&self, snapshot: &RuntimeSnapshot) -> Option<InsightEvent> {
        if snapshot.total_query_count < self.min_total_queries {
            return None;
        }
        let rate = snapshot.error_rate();
        if rate < self.threshold {
            return None;
        }
        let severity = if rate >= self.threshold * 2.0 {
            Severity::Critical
        } else {
            Severity::Warn
        };
        let msg = format!(
            "错误率过高：{:.2}%（阈值 {:.2}%，总查询 {}）",
            rate * 100.0,
            self.threshold * 100.0,
            snapshot.total_query_count
        );
        Some(
            InsightEvent::now(self.rule_id(), severity, msg)
                .with_context("error_rate", &format!("{:.4}", rate))
                .with_context("threshold", &format!("{:.4}", self.threshold))
                .with_context(
                    "total_query_count",
                    &snapshot.total_query_count.to_string(),
                ),
        )
    }
}

/// 锁等待过多规则
pub struct LockWaitHighRule {
    /// 阈值
    pub threshold: u64,
}

impl LockWaitHighRule {
    /// 创建规则，默认阈值 50
    pub fn new(threshold: u64) -> Self {
        Self { threshold }
    }
}

impl Default for LockWaitHighRule {
    fn default() -> Self {
        Self::new(50)
    }
}

impl InsightRule for LockWaitHighRule {
    fn rule_id(&self) -> &str {
        "lock_wait_high"
    }

    fn evaluate(&self, snapshot: &RuntimeSnapshot) -> Option<InsightEvent> {
        if snapshot.lock_wait_events < self.threshold {
            return None;
        }
        let severity = if snapshot.lock_wait_events >= self.threshold * 3 {
            Severity::Critical
        } else {
            Severity::Warn
        };
        let msg = format!(
            "锁等待过多：{} 次（阈值 {}）",
            snapshot.lock_wait_events, self.threshold
        );
        Some(
            InsightEvent::now(self.rule_id(), severity, msg)
                .with_context(
                    "lock_wait_events",
                    &snapshot.lock_wait_events.to_string(),
                )
                .with_context("threshold", &self.threshold.to_string())
                .with_context("pending_locks", &snapshot.pending_locks.to_string()),
        )
    }
}

// =====================================================================
//  推送目标 trait
// =====================================================================

/// 推送目标
pub trait InsightSink: Send + Sync {
    /// 接收事件
    fn notify(&self, event: &InsightEvent);
}

/// 内存推送目标（用于测试和采集历史）
#[derive(Debug, Default)]
pub struct InMemorySink {
    /// 已接收的事件列表（用 Mutex 保证线程安全）
    events: std::sync::Mutex<Vec<InsightEvent>>,
}

impl InMemorySink {
    /// 创建空 sink
    pub fn new() -> Self {
        Self::default()
    }

    /// 已接收的事件数
    pub fn count(&self) -> usize {
        self.events.lock().map(|e| e.len()).unwrap_or(0)
    }

    /// 获取所有事件副本
    pub fn events(&self) -> Vec<InsightEvent> {
        self.events.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// 清空
    pub fn clear(&self) {
        if let Ok(mut e) = self.events.lock() {
            e.clear();
        }
    }
}

impl InsightSink for InMemorySink {
    fn notify(&self, event: &InsightEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event.clone());
        }
    }
}

/// 日志推送目标（输出到 stderr）
pub struct LogSink;

impl InsightSink for LogSink {
    fn notify(&self, event: &InsightEvent) {
        eprintln!(
            "[insight][{}][{}] {} (context: {:?})",
            event.severity.as_str(),
            event.rule_id,
            event.message,
            event.context
        );
    }
}

// =====================================================================
//  主动洞察引擎
// =====================================================================

/// 主动洞察引擎
///
/// 注册规则 + 订阅者，由外部调度器调用 `tick(snapshot)` 驱动一轮采集 → 检测 → 推送。
pub struct ProactiveEngine {
    /// 已注册的规则
    rules: Vec<Box<dyn InsightRule>>,
    /// 已注册的订阅者
    sinks: Vec<Box<dyn InsightSink>>,
    /// 去重冷却时间
    cooldown: Duration,
    /// rule_id → 上次触发时间
    last_fired: HashMap<String, Instant>,
    /// 历史事件（保留最近 N 条）
    history: Vec<InsightEvent>,
    /// 历史上限
    history_capacity: usize,
}

impl Default for ProactiveEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ProactiveEngine {
    /// 创建空引擎
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            sinks: Vec::new(),
            cooldown: Duration::from_secs(60),
            last_fired: HashMap::new(),
            history: Vec::new(),
            history_capacity: 1000,
        }
    }

    /// 设置去重冷却时间
    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown = cooldown;
        self
    }

    /// 设置历史容量
    pub fn with_history_capacity(mut self, capacity: usize) -> Self {
        self.history_capacity = capacity;
        self
    }

    /// 注册规则
    pub fn register_rule(&mut self, rule: Box<dyn InsightRule>) {
        self.rules.push(rule);
    }

    /// 注册订阅者
    pub fn register_sink(&mut self, sink: Box<dyn InsightSink>) {
        self.sinks.push(sink);
    }

    /// 注册全部内置规则
    pub fn register_default_rules(&mut self) {
        self.register_rule(Box::new(SlowQuerySpikeRule::default()));
        self.register_rule(Box::new(DeadlockFrequentRule::default()));
        self.register_rule(Box::new(CapacityUrgentRule::default()));
        self.register_rule(Box::new(ErrorRateHighRule::default()));
        self.register_rule(Box::new(LockWaitHighRule::default()));
    }

    /// 已注册规则数
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// 已注册订阅者数
    pub fn sink_count(&self) -> usize {
        self.sinks.len()
    }

    /// 触发一轮采集 → 检测 → 推送
    ///
    /// 返回本轮触发的事件数（去重后）。
    /// 由外部调度器调用（如 tokio 定时任务每 60s 调用一次）。
    pub fn tick(&mut self, snapshot: &RuntimeSnapshot) -> usize {
        let now = Instant::now();
        let mut fired = 0;
        for rule in &self.rules {
            if let Some(mut event) = rule.evaluate(snapshot) {
                let rule_id = rule.rule_id().to_string();
                // 去重检查
                if let Some(&last) = self.last_fired.get(&rule_id) {
                    if now.duration_since(last) < self.cooldown {
                        continue;
                    }
                }
                // 更新时间戳为引擎统一时间（避免规则内部时间不一致）
                event.timestamp_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(event.timestamp_ms);
                // 推送到所有订阅者
                for sink in &self.sinks {
                    sink.notify(&event);
                }
                // 记录历史
                self.history.push(event.clone());
                if self.history.len() > self.history_capacity {
                    self.history.remove(0);
                }
                // 更新 last_fired
                self.last_fired.insert(rule_id, now);
                fired += 1;
            }
        }
        fired
    }

    /// 历史事件
    pub fn history(&self) -> &[InsightEvent] {
        &self.history
    }

    /// 清空历史
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// 重置去重状态（强制下次 tick 触发）
    pub fn reset_cooldown(&mut self) {
        self.last_fired.clear();
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot() -> RuntimeSnapshot {
        RuntimeSnapshot::default()
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Critical > Severity::Warn);
        assert!(Severity::Warn > Severity::Info);
        assert_eq!(Severity::Critical.as_str(), "critical");
        assert_eq!(Severity::Warn.as_str(), "warn");
        assert_eq!(Severity::Info.as_str(), "info");
    }

    #[test]
    fn test_event_with_context() {
        let event = InsightEvent::now("rule1", Severity::Warn, "test message".to_string())
            .with_context("key1", "value1")
            .with_context("key2", "value2");
        assert_eq!(event.rule_id, "rule1");
        assert_eq!(event.severity, Severity::Warn);
        assert_eq!(event.message, "test message");
        assert_eq!(event.context.len(), 2);
        assert_eq!(event.context[0], ("key1".to_string(), "value1".to_string()));
        assert!(event.timestamp_ms > 0);
    }

    #[test]
    fn test_snapshot_error_rate() {
        let mut s = make_snapshot();
        assert_eq!(s.error_rate(), 0.0);
        s.total_query_count = 100;
        s.error_query_count = 5;
        assert!((s.error_rate() - 0.05).abs() < 1e-9);
    }

    #[test]
    fn test_slow_query_spike_rule_below_threshold() {
        let rule = SlowQuerySpikeRule::new(10);
        let mut s = make_snapshot();
        s.slow_query_count = 5;
        assert!(rule.evaluate(&s).is_none());
    }

    #[test]
    fn test_slow_query_spike_rule_at_threshold_warn() {
        let rule = SlowQuerySpikeRule::new(10);
        let mut s = make_snapshot();
        s.slow_query_count = 10;
        let event = rule.evaluate(&s).unwrap();
        assert_eq!(event.severity, Severity::Warn);
        assert!(event.message.contains("慢查询激增"));
    }

    #[test]
    fn test_slow_query_spike_rule_critical() {
        let rule = SlowQuerySpikeRule::new(10);
        let mut s = make_snapshot();
        s.slow_query_count = 30; // >= threshold * 3
        let event = rule.evaluate(&s).unwrap();
        assert_eq!(event.severity, Severity::Critical);
    }

    #[test]
    fn test_deadlock_frequent_rule_default() {
        let rule = DeadlockFrequentRule::default();
        let mut s = make_snapshot();
        s.deadlock_count = 0;
        assert!(rule.evaluate(&s).is_none());
        s.deadlock_count = 1;
        let event = rule.evaluate(&s).unwrap();
        assert_eq!(event.severity, Severity::Warn);
        s.deadlock_count = 3;
        let event = rule.evaluate(&s).unwrap();
        assert_eq!(event.severity, Severity::Critical);
    }

    #[test]
    fn test_capacity_urgent_rule_none_when_no_prediction() {
        let rule = CapacityUrgentRule::default();
        let s = make_snapshot();
        assert!(rule.evaluate(&s).is_none());
    }

    #[test]
    fn test_capacity_urgent_rule_warn() {
        let rule = CapacityUrgentRule::new(100.0);
        let mut s = make_snapshot();
        s.predicted_storage_bytes = Some(150.0);
        s.capacity_confidence = Some(0.8);
        let event = rule.evaluate(&s).unwrap();
        assert_eq!(event.severity, Severity::Warn);
    }

    #[test]
    fn test_capacity_urgent_rule_critical() {
        let rule = CapacityUrgentRule::new(100.0);
        let mut s = make_snapshot();
        s.predicted_storage_bytes = Some(250.0); // >= threshold * 2
        let event = rule.evaluate(&s).unwrap();
        assert_eq!(event.severity, Severity::Critical);
    }

    #[test]
    fn test_error_rate_high_rule_low_sample_skipped() {
        let rule = ErrorRateHighRule::new(0.1, 100);
        let mut s = make_snapshot();
        s.total_query_count = 50; // < min_total_queries
        s.error_query_count = 25;
        assert!(rule.evaluate(&s).is_none());
    }

    #[test]
    fn test_error_rate_high_rule_triggers() {
        let rule = ErrorRateHighRule::new(0.1, 100);
        let mut s = make_snapshot();
        s.total_query_count = 200;
        s.error_query_count = 30; // 15% > 10%
        let event = rule.evaluate(&s).unwrap();
        assert_eq!(event.severity, Severity::Warn);
        s.error_query_count = 60; // 30% > 20% = threshold * 2
        let event = rule.evaluate(&s).unwrap();
        assert_eq!(event.severity, Severity::Critical);
    }

    #[test]
    fn test_lock_wait_high_rule() {
        let rule = LockWaitHighRule::new(50);
        let mut s = make_snapshot();
        s.lock_wait_events = 30;
        assert!(rule.evaluate(&s).is_none());
        s.lock_wait_events = 50;
        let event = rule.evaluate(&s).unwrap();
        assert_eq!(event.severity, Severity::Warn);
        s.lock_wait_events = 150; // >= threshold * 3
        let event = rule.evaluate(&s).unwrap();
        assert_eq!(event.severity, Severity::Critical);
    }

    #[test]
    fn test_in_memory_sink_records_events() {
        let sink = InMemorySink::new();
        assert_eq!(sink.count(), 0);
        let event = InsightEvent::now("r1", Severity::Info, "msg".to_string());
        sink.notify(&event);
        assert_eq!(sink.count(), 1);
        let events = sink.events();
        assert_eq!(events[0].rule_id, "r1");
    }

    #[test]
    fn test_engine_new_empty() {
        let engine = ProactiveEngine::new();
        assert_eq!(engine.rule_count(), 0);
        assert_eq!(engine.sink_count(), 0);
        assert!(engine.history().is_empty());
    }

    #[test]
    fn test_engine_register_default_rules() {
        let mut engine = ProactiveEngine::new();
        engine.register_default_rules();
        assert_eq!(engine.rule_count(), 5);
    }

    #[test]
    fn test_engine_tick_no_rules() {
        let mut engine = ProactiveEngine::new();
        let s = make_snapshot();
        assert_eq!(engine.tick(&s), 0);
    }

    #[test]
    fn test_engine_tick_fires_events() {
        let mut engine = ProactiveEngine::new();
        engine.register_default_rules();
        let sink = std::sync::Arc::new(InMemorySink::new());
        engine.register_sink(Box::new(InMemorySinkWrapper(sink.clone())));
        let mut s = make_snapshot();
        s.slow_query_count = 15; // 触发 slow_query_spike
        s.deadlock_count = 2; // 触发 deadlock_frequent
        let fired = engine.tick(&s);
        assert!(fired >= 2, "should fire at least 2 events, got {}", fired);
        assert!(sink.count() >= 2);
        assert!(engine.history().len() >= 2);
    }

    #[test]
    fn test_engine_cooldown_dedup() {
        let mut engine = ProactiveEngine::new()
            .with_cooldown(Duration::from_millis(100));
        engine.register_rule(Box::new(SlowQuerySpikeRule::new(10)));
        let sink = std::sync::Arc::new(InMemorySink::new());
        engine.register_sink(Box::new(InMemorySinkWrapper(sink.clone())));
        let mut s = make_snapshot();
        s.slow_query_count = 15;
        // 第一次 tick 触发
        assert_eq!(engine.tick(&s), 1);
        assert_eq!(sink.count(), 1);
        // 立即第二次 tick 应被去重
        assert_eq!(engine.tick(&s), 0);
        assert_eq!(sink.count(), 1);
        // 等待冷却后再次触发
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(engine.tick(&s), 1);
        assert_eq!(sink.count(), 2);
    }

    #[test]
    fn test_engine_reset_cooldown_forces_fire() {
        let mut engine = ProactiveEngine::new()
            .with_cooldown(Duration::from_secs(60));
        engine.register_rule(Box::new(SlowQuerySpikeRule::new(10)));
        let sink = std::sync::Arc::new(InMemorySink::new());
        engine.register_sink(Box::new(InMemorySinkWrapper(sink.clone())));
        let mut s = make_snapshot();
        s.slow_query_count = 15;
        assert_eq!(engine.tick(&s), 1);
        // reset 后立即触发
        engine.reset_cooldown();
        assert_eq!(engine.tick(&s), 1);
        assert_eq!(sink.count(), 2);
    }

    #[test]
    fn test_engine_history_capacity() {
        let mut engine = ProactiveEngine::new()
            .with_cooldown(Duration::from_millis(0))
            .with_history_capacity(3);
        engine.register_rule(Box::new(SlowQuerySpikeRule::new(10)));
        let mut s = make_snapshot();
        s.slow_query_count = 15;
        for _ in 0..5 {
            engine.tick(&s);
        }
        assert_eq!(engine.history().len(), 3, "history should be capped at 3");
    }

    #[test]
    fn test_engine_clear_history() {
        let mut engine = ProactiveEngine::new()
            .with_cooldown(Duration::from_millis(0));
        engine.register_rule(Box::new(SlowQuerySpikeRule::new(10)));
        let mut s = make_snapshot();
        s.slow_query_count = 15;
        engine.tick(&s);
        assert!(!engine.history().is_empty());
        engine.clear_history();
        assert!(engine.history().is_empty());
    }

    #[test]
    fn test_engine_full_workflow() {
        // 端到端：注册规则 + 订阅 + 多轮 tick + 历史查询
        let mut engine = ProactiveEngine::new()
            .with_cooldown(Duration::from_millis(0));
        engine.register_default_rules();
        let sink = std::sync::Arc::new(InMemorySink::new());
        engine.register_sink(Box::new(InMemorySinkWrapper(sink.clone())));

        // 第一轮：无异常
        let s1 = make_snapshot();
        assert_eq!(engine.tick(&s1), 0);
        assert_eq!(sink.count(), 0);

        // 第二轮：慢查询激增 + 死锁
        let mut s2 = make_snapshot();
        s2.slow_query_count = 20;
        s2.deadlock_count = 2;
        let fired = engine.tick(&s2);
        assert!(fired >= 2);
        assert!(sink.count() >= 2);

        // 第三轮：容量告急 + 错误率高 + 锁等待
        let mut s3 = make_snapshot();
        s3.predicted_storage_bytes = Some(20.0 * 1024.0 * 1024.0 * 1024.0);
        s3.capacity_confidence = Some(0.85);
        s3.total_query_count = 200;
        s3.error_query_count = 30;
        s3.lock_wait_events = 60;
        let fired = engine.tick(&s3);
        assert!(fired >= 3, "should fire at least 3 events, got {}", fired);

        // 验证历史
        assert!(engine.history().len() >= 5);
        // 验证严重级别分布
        let critical_count = engine
            .history()
            .iter()
            .filter(|e| e.severity == Severity::Critical)
            .count();
        assert!(critical_count >= 1, "should have at least 1 critical event");
    }

    /// 辅助 wrapper：让 Arc<InMemorySink> 实现 InsightSink
    struct InMemorySinkWrapper(std::sync::Arc<InMemorySink>);

    impl InsightSink for InMemorySinkWrapper {
        fn notify(&self, event: &InsightEvent) {
            self.0.notify(event);
        }
    }
}
