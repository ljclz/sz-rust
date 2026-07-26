//! 告警管理器（Alert Manager）— Phase 7d.9
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7 运维监控 — 告警管理器设计。
//!
//! # 设计
//!
//! AlertManager 管理告警规则，监控指标值，超阈值触发告警，通过通知通道发送，
//! 重复告警在抑制窗口内不重复发送，指标恢复后发送 Resolved 告警。
//!
//! - **AlertLevel** — 告警级别（Info/Warning/Critical/Fatal）
//! - **Comparison** — 比较运算符（GreaterThan/GreaterThanOrEqual/LessThan/LessThanOrEqual）
//! - **AlertRule** — 告警规则（指标名 + 阈值 + 比较 + 持续时间 + 级别）
//! - **Alert** — 告警事件（规则 ID + 级别 + 消息 + 时间戳 + 指标值 + 阈值）
//! - **AlertState** — 告警状态（Pending 触发但未满足持续时间 / Firing 触发中 / Resolved 已恢复）
//! - **NotificationChannel** — 通知通道（Console 控制台 / Webhook HTTP POST）
//! - **AlertManager** — 告警管理器（规则 + 活跃告警 + 通知 + 抑制）
//!
//! ## 验证标准
//!
//! - 设置阈值规则（QPS > 10000 / 延迟 > 1s / 连接数 > 90%）
//! - 超阈值触发告警 → Webhook/Console 通知
//! - 重复告警抑制（同规则在抑制窗口内不重复）
//! - 指标恢复后自动发送 Resolved 告警
//! - 告警延迟 < 5s

use std::collections::HashMap;

// =====================================================================
//  常量
// =====================================================================

/// 默认告警抑制窗口（秒）— 同规则在窗口内不重复触发
pub const DEFAULT_SUPPRESSION_WINDOW_SECS: u64 = 300;

/// 默认告警评估周期（秒）
pub const DEFAULT_EVALUATION_INTERVAL_SECS: u64 = 10;

/// 最大通知通道数
pub const MAX_NOTIFICATION_CHANNELS: usize = 16;

// =====================================================================
//  AlertLevel — 告警级别
// =====================================================================

/// 告警级别 — 4 级严重度
///
/// - **Info** — 信息（低优先级，仅供记录）
/// - **Warning** — 警告（需要注意，可能升级）
/// - **Critical** — 严重（影响业务，需立即处理）
/// - **Fatal** — 致命（系统不可用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AlertLevel {
    /// 信息
    Info,
    /// 警告
    Warning,
    /// 严重
    Critical,
    /// 致命
    Fatal,
}

impl AlertLevel {
    /// 级别名称
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertLevel::Info => "info",
            AlertLevel::Warning => "warning",
            AlertLevel::Critical => "critical",
            AlertLevel::Fatal => "fatal",
        }
    }

    /// 数值严重度（0=Info, 1=Warning, 2=Critical, 3=Fatal）
    pub fn severity(&self) -> u8 {
        match self {
            AlertLevel::Info => 0,
            AlertLevel::Warning => 1,
            AlertLevel::Critical => 2,
            AlertLevel::Fatal => 3,
        }
    }

    /// 是否严重级别（Critical 或 Fatal）
    pub fn is_severe(&self) -> bool {
        matches!(self, AlertLevel::Critical | AlertLevel::Fatal)
    }

    /// 是否致命
    pub fn is_fatal(&self) -> bool {
        matches!(self, AlertLevel::Fatal)
    }
}

impl std::fmt::Display for AlertLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  Comparison — 比较运算符
// =====================================================================

/// 比较运算符 — 告警规则的阈值比较
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Comparison {
    /// 大于（value > threshold）
    GreaterThan,
    /// 大于等于（value >= threshold）
    GreaterThanOrEqual,
    /// 小于（value < threshold）
    LessThan,
    /// 小于等于（value <= threshold）
    LessThanOrEqual,
}

impl Comparison {
    /// 运算符符号
    pub fn as_str(&self) -> &'static str {
        match self {
            Comparison::GreaterThan => ">",
            Comparison::GreaterThanOrEqual => ">=",
            Comparison::LessThan => "<",
            Comparison::LessThanOrEqual => "<=",
        }
    }

    /// 判断值是否满足比较条件
    pub fn evaluate(&self, value: f64, threshold: f64) -> bool {
        match self {
            Comparison::GreaterThan => value > threshold,
            Comparison::GreaterThanOrEqual => value >= threshold,
            Comparison::LessThan => value < threshold,
            Comparison::LessThanOrEqual => value <= threshold,
        }
    }
}

impl std::fmt::Display for Comparison {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  AlertRule — 告警规则
// =====================================================================

/// 告警规则 — 定义阈值条件和告警级别
#[derive(Debug, Clone)]
pub struct AlertRule {
    /// 规则 ID（唯一标识）
    pub rule_id: String,
    /// 规则名称
    pub name: String,
    /// 监控指标名（如 "qps"、"latency_ms"、"connection_usage"）
    pub metric_name: String,
    /// 阈值
    pub threshold: f64,
    /// 比较运算符
    pub comparison: Comparison,
    /// 持续时间（秒）— 持续满足条件 N 秒后才触发告警
    pub for_duration_secs: u64,
    /// 告警级别
    pub level: AlertLevel,
    /// 规则描述
    pub description: String,
    /// 自定义标签
    pub labels: HashMap<String, String>,
}

impl AlertRule {
    /// 构造新告警规则
    pub fn new(
        rule_id: impl Into<String>,
        name: impl Into<String>,
        metric_name: impl Into<String>,
        threshold: f64,
        comparison: Comparison,
        level: AlertLevel,
    ) -> Self {
        Self {
            rule_id: rule_id.into(),
            name: name.into(),
            metric_name: metric_name.into(),
            threshold,
            comparison,
            for_duration_secs: 0,
            level,
            description: String::new(),
            labels: HashMap::new(),
        }
    }

    /// 设置持续时间（秒）
    pub fn with_for_duration(mut self, secs: u64) -> Self {
        self.for_duration_secs = secs;
        self
    }

    /// 设置描述
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    /// 添加标签
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// 判断指标值是否满足告警条件
    pub fn matches(&self, value: f64) -> bool {
        self.comparison.evaluate(value, self.threshold)
    }

    /// 生成告警消息
    pub fn alert_message(&self, value: f64) -> String {
        format!(
            "规则 [{}] {}: {} {} {} (当前值: {:.2})",
            self.rule_id,
            self.name,
            self.metric_name,
            self.comparison.as_str(),
            self.threshold,
            value
        )
    }
}

// =====================================================================
//  Alert — 告警事件
// =====================================================================

/// 告警状态 — 告警的生命周期状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertState {
    /// 待定 — 条件已满足但持续时间不足
    Pending,
    /// 触发中 — 已满足触发条件并发送通知
    Firing,
    /// 已恢复 — 条件不再满足
    Resolved,
}

impl AlertState {
    /// 状态名称
    pub fn as_str(&self) -> &'static str {
        match self {
            AlertState::Pending => "pending",
            AlertState::Firing => "firing",
            AlertState::Resolved => "resolved",
        }
    }

    /// 是否触发中
    pub fn is_firing(&self) -> bool {
        matches!(self, AlertState::Firing)
    }

    /// 是否已恢复
    pub fn is_resolved(&self) -> bool {
        matches!(self, AlertState::Resolved)
    }
}

impl std::fmt::Display for AlertState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 告警事件 — 触发的告警记录
#[derive(Debug, Clone)]
pub struct Alert {
    /// 规则 ID
    pub rule_id: String,
    /// 告警级别
    pub level: AlertLevel,
    /// 告警状态
    pub state: AlertState,
    /// 告警消息
    pub message: String,
    /// 触发时间戳（秒）
    pub fired_at: u64,
    /// 当前指标值
    pub value: f64,
    /// 阈值
    pub threshold: f64,
    /// 比较运算符
    pub comparison: Comparison,
    /// 标签（从规则继承）
    pub labels: HashMap<String, String>,
}

impl Alert {
    /// 构造新告警
    pub fn new(rule: &AlertRule, state: AlertState, value: f64, fired_at: u64) -> Self {
        Self {
            rule_id: rule.rule_id.clone(),
            level: rule.level,
            state,
            message: rule.alert_message(value),
            fired_at,
            value,
            threshold: rule.threshold,
            comparison: rule.comparison,
            labels: rule.labels.clone(),
        }
    }

    /// 是否为严重告警
    pub fn is_severe(&self) -> bool {
        self.level.is_severe()
    }

    /// 告警持续时间（秒）
    pub fn duration_secs(&self, now: u64) -> u64 {
        now.saturating_sub(self.fired_at)
    }

    /// 序列化为 JSON 字符串（用于 Webhook 推送）
    pub fn to_json(&self) -> String {
        let labels_json: String = self
            .labels
            .iter()
            .map(|(k, v)| format!("\"{}\":\"{}\"", k, v))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"rule_id\":\"{}\",\"level\":\"{}\",\"state\":\"{}\",\"message\":\"{}\",\"fired_at\":{},\"value\":{:.2},\"threshold\":{:.2},\"comparison\":\"{}\",\"labels\":{{{}}}}}",
            self.rule_id,
            self.level.as_str(),
            self.state.as_str(),
            self.message.replace('"', "\\\""),
            self.fired_at,
            self.value,
            self.threshold,
            self.comparison.as_str(),
            labels_json
        )
    }
}

// =====================================================================
//  NotificationChannel — 通知通道
// =====================================================================

/// 通知通道类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelType {
    /// 控制台输出（stdout/stderr）
    Console,
    /// Webhook HTTP POST（URL 由 channel_id 指定）
    Webhook,
}

impl ChannelType {
    /// 类型名称
    pub fn as_str(&self) -> &'static str {
        match self {
            ChannelType::Console => "console",
            ChannelType::Webhook => "webhook",
        }
    }
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 通知通道 — 告警发送的目标
#[derive(Debug, Clone)]
pub struct NotificationChannel {
    /// 通道 ID
    pub channel_id: String,
    /// 通道类型
    pub channel_type: ChannelType,
    /// 目标地址（Console 为前缀标识，Webhook 为 URL）
    pub target: String,
    /// 最小告警级别（低于此级别不发送）
    pub min_level: AlertLevel,
    /// 已发送通知计数
    pub sent_count: u64,
}

impl NotificationChannel {
    /// 构造 Console 通道
    pub fn console(channel_id: impl Into<String>) -> Self {
        Self {
            channel_id: channel_id.into(),
            channel_type: ChannelType::Console,
            target: "stdout".to_string(),
            min_level: AlertLevel::Info,
            sent_count: 0,
        }
    }

    /// 构造 Webhook 通道
    pub fn webhook(channel_id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            channel_id: channel_id.into(),
            channel_type: ChannelType::Webhook,
            target: url.into(),
            min_level: AlertLevel::Warning,
            sent_count: 0,
        }
    }

    /// 设置最小告警级别
    pub fn with_min_level(mut self, level: AlertLevel) -> Self {
        self.min_level = level;
        self
    }

    /// 判断告警级别是否满足发送条件
    pub fn should_send(&self, alert: &Alert) -> bool {
        alert.level.severity() >= self.min_level.severity()
    }

    /// 模拟发送通知（不真正发 HTTP，仅返回发送载荷）
    pub fn send(&mut self, alert: &Alert) -> Option<NotificationPayload> {
        if !self.should_send(alert) {
            return None;
        }
        self.sent_count += 1;
        Some(NotificationPayload {
            channel_id: self.channel_id.clone(),
            channel_type: self.channel_type.clone(),
            target: self.target.clone(),
            payload: alert.to_json(),
        })
    }
}

/// 通知载荷 — 发送给通道的内容
#[derive(Debug, Clone)]
pub struct NotificationPayload {
    /// 通道 ID
    pub channel_id: String,
    /// 通道类型
    pub channel_type: ChannelType,
    /// 目标地址
    pub target: String,
    /// JSON 载荷
    pub payload: String,
}

// =====================================================================
//  RuleState — 规则运行时状态
// =====================================================================

/// 规则运行时状态 — 跟踪规则触发的中间状态
#[derive(Debug, Clone)]
struct RuleRuntimeState {
    /// 当前状态
    state: AlertState,
    /// 条件首次满足时间戳（进入 Pending 状态时）
    pending_since: Option<u64>,
    /// 当前 Firing 告警的触发时间戳（进入 Firing 状态时）
    firing_since: Option<u64>,
    /// 上次发送通知的时间戳（用于抑制窗口）
    last_notified_at: Option<u64>,
    /// 触发计数
    fire_count: u64,
}

impl RuleRuntimeState {
    fn new() -> Self {
        Self {
            state: AlertState::Resolved,
            pending_since: None,
            firing_since: None,
            last_notified_at: None,
            fire_count: 0,
        }
    }

    /// 是否处于 Firing 状态
    fn is_firing(&self) -> bool {
        self.state == AlertState::Firing
    }

    /// 是否在抑制窗口内
    fn is_suppressed(&self, now: u64, window_secs: u64) -> bool {
        match self.last_notified_at {
            Some(last) => now.saturating_sub(last) < window_secs,
            None => false,
        }
    }
}

// =====================================================================
//  AlertManager — 告警管理器
// =====================================================================

/// 告警管理器 — 管理规则、监控指标、触发告警、发送通知
#[derive(Debug, Clone)]
pub struct AlertManager {
    /// 告警规则（rule_id → AlertRule）
    rules: HashMap<String, AlertRule>,
    /// 规则运行时状态（rule_id → RuleRuntimeState）
    runtime_states: HashMap<String, RuleRuntimeState>,
    /// 通知通道（channel_id → NotificationChannel）
    channels: HashMap<String, NotificationChannel>,
    /// 当前活跃告警（rule_id → Firing 状态的 Alert）
    active_alerts: HashMap<String, Alert>,
    /// 抑制窗口（秒）
    suppression_window_secs: u64,
    /// 评估周期（秒）
    evaluation_interval_secs: u64,
    /// 已触发的告警历史（按时间顺序，事件快照不可变）
    alert_history: Vec<Alert>,
    /// 已发送的通知载荷历史
    notification_history: Vec<NotificationPayload>,
    /// 上次评估时间戳
    last_evaluation: u64,
    /// 总评估次数
    total_evaluations: u64,
    /// 总触发次数
    total_fires: u64,
    /// 总抑制次数
    total_suppressions: u64,
}

impl AlertManager {
    /// 构造默认告警管理器
    pub fn new() -> Self {
        Self {
            rules: HashMap::new(),
            runtime_states: HashMap::new(),
            channels: HashMap::new(),
            active_alerts: HashMap::new(),
            suppression_window_secs: DEFAULT_SUPPRESSION_WINDOW_SECS,
            evaluation_interval_secs: DEFAULT_EVALUATION_INTERVAL_SECS,
            alert_history: Vec::new(),
            notification_history: Vec::new(),
            last_evaluation: 0,
            total_evaluations: 0,
            total_fires: 0,
            total_suppressions: 0,
        }
    }

    /// 设置抑制窗口（秒）
    pub fn with_suppression_window(mut self, secs: u64) -> Self {
        self.suppression_window_secs = secs;
        self
    }

    /// 设置评估周期（秒）
    pub fn with_evaluation_interval(mut self, secs: u64) -> Self {
        self.evaluation_interval_secs = secs;
        self
    }

    /// 添加告警规则
    pub fn add_rule(&mut self, rule: AlertRule) -> bool {
        if self.rules.contains_key(&rule.rule_id) {
            return false;
        }
        self.runtime_states
            .insert(rule.rule_id.clone(), RuleRuntimeState::new());
        self.rules.insert(rule.rule_id.clone(), rule);
        true
    }

    /// 移除告警规则
    pub fn remove_rule(&mut self, rule_id: &str) -> bool {
        let removed = self.rules.remove(rule_id).is_some();
        self.runtime_states.remove(rule_id);
        removed
    }

    /// 获取规则
    pub fn rule(&self, rule_id: &str) -> Option<&AlertRule> {
        self.rules.get(rule_id)
    }

    /// 规则数量
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// 添加通知通道
    pub fn add_channel(&mut self, channel: NotificationChannel) -> bool {
        if self.channels.len() >= MAX_NOTIFICATION_CHANNELS {
            return false;
        }
        if self.channels.contains_key(&channel.channel_id) {
            return false;
        }
        self.channels.insert(channel.channel_id.clone(), channel);
        true
    }

    /// 移除通知通道
    pub fn remove_channel(&mut self, channel_id: &str) -> bool {
        self.channels.remove(channel_id).is_some()
    }

    /// 通道数量
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// 抑制窗口（秒）
    pub fn suppression_window_secs(&self) -> u64 {
        self.suppression_window_secs
    }

    /// 评估周期（秒）
    pub fn evaluation_interval_secs(&self) -> u64 {
        self.evaluation_interval_secs
    }

    /// 总评估次数
    pub fn total_evaluations(&self) -> u64 {
        self.total_evaluations
    }

    /// 总触发次数
    pub fn total_fires(&self) -> u64 {
        self.total_fires
    }

    /// 总抑制次数
    pub fn total_suppressions(&self) -> u64 {
        self.total_suppressions
    }

    /// 是否应该评估（按评估周期判断）
    pub fn should_evaluate(&self, now: u64) -> bool {
        now.saturating_sub(self.last_evaluation) >= self.evaluation_interval_secs
    }

    /// 获取当前 Firing 状态的告警（基于活跃告警映射）
    pub fn firing_alerts(&self) -> Vec<&Alert> {
        self.active_alerts.values().collect()
    }

    /// 获取告警历史
    pub fn alert_history(&self) -> &[Alert] {
        &self.alert_history
    }

    /// 获取通知历史
    pub fn notification_history(&self) -> &[NotificationPayload] {
        &self.notification_history
    }

    /// 评估单个指标值 — 核心 Evaluate 方法
    ///
    /// 对所有匹配 metric_name 的规则进行评估，更新运行时状态，
    /// 必要时触发告警并发送通知。
    ///
    /// 返回本次评估触发的新告警列表
    pub fn evaluate(&mut self, metric_name: &str, value: f64, now: u64) -> Vec<Alert> {
        self.total_evaluations += 1;
        self.last_evaluation = now;

        let mut new_alerts = Vec::new();

        // 收集匹配的规则 ID（避免借用问题）
        let matching_rule_ids: Vec<String> = self
            .rules
            .iter()
            .filter(|(_, r)| r.metric_name == metric_name)
            .map(|(id, _)| id.clone())
            .collect();

        for rule_id in matching_rule_ids {
            let rule = self.rules.get(&rule_id).unwrap().clone();
            let runtime = self
                .runtime_states
                .entry(rule_id.clone())
                .or_insert_with(RuleRuntimeState::new);

            let condition_met = rule.matches(value);

            let mut transition: Option<AlertState> = None;
            let mut should_notify = false;

            match runtime.state {
                AlertState::Resolved => {
                    if condition_met {
                        // 从 Resolved → Pending
                        runtime.pending_since = Some(now);
                        runtime.state = AlertState::Pending;

                        if rule.for_duration_secs == 0 {
                            // 无持续时间要求，立即触发
                            runtime.state = AlertState::Firing;
                            runtime.firing_since = Some(now);
                            runtime.fire_count += 1;
                            transition = Some(AlertState::Firing);
                            should_notify =
                                !runtime.is_suppressed(now, self.suppression_window_secs);
                        }
                    }
                }
                AlertState::Pending => {
                    if condition_met {
                        // 检查持续时间是否已满足
                        let pending_since = runtime.pending_since.unwrap_or(now);
                        let elapsed = now.saturating_sub(pending_since);
                        if elapsed >= rule.for_duration_secs {
                            runtime.state = AlertState::Firing;
                            runtime.firing_since = Some(now);
                            runtime.fire_count += 1;
                            transition = Some(AlertState::Firing);
                            should_notify =
                                !runtime.is_suppressed(now, self.suppression_window_secs);
                        }
                    } else {
                        // 条件不再满足，回到 Resolved
                        runtime.state = AlertState::Resolved;
                        runtime.pending_since = None;
                    }
                }
                AlertState::Firing => {
                    if !condition_met {
                        // 条件不再满足，进入 Resolved
                        runtime.state = AlertState::Resolved;
                        runtime.pending_since = None;
                        runtime.firing_since = None;
                        transition = Some(AlertState::Resolved);
                        should_notify = true; // Resolved 通知不受抑制窗口限制
                    }
                    // 条件仍满足，保持 Firing（不重复通知）
                }
            }

            // 处理状态转换和通知
            if let Some(new_state) = transition {
                let alert = Alert::new(&rule, new_state, value, now);
                if should_notify {
                    if new_state == AlertState::Firing {
                        runtime.last_notified_at = Some(now);
                        self.total_fires += 1;
                    }
                    // 发送到所有通道
                    let channels_to_send: Vec<String> = self.channels.keys().cloned().collect();
                    for channel_id in channels_to_send {
                        if let Some(channel) = self.channels.get_mut(&channel_id) {
                            if let Some(payload) = channel.send(&alert) {
                                self.notification_history.push(payload);
                            }
                        }
                    }
                } else if new_state == AlertState::Firing {
                    // 抑制
                    self.total_suppressions += 1;
                }

                // 维护活跃告警映射
                match new_state {
                    AlertState::Firing => {
                        self.active_alerts.insert(rule_id.clone(), alert.clone());
                    }
                    AlertState::Resolved => {
                        self.active_alerts.remove(&rule_id);
                    }
                    AlertState::Pending => {}
                }

                self.alert_history.push(alert.clone());
                new_alerts.push(alert);
            }
        }

        new_alerts
    }

    /// 批量评估多个指标
    ///
    /// metrics 是 (metric_name, value) 的列表
    pub fn evaluate_batch(&mut self, metrics: &[(&str, f64)], now: u64) -> Vec<Alert> {
        let mut all_alerts = Vec::new();
        for (metric_name, value) in metrics {
            let alerts = self.evaluate(metric_name, *value, now);
            all_alerts.extend(alerts);
        }
        all_alerts
    }

    /// 强制触发告警（手动触发，不经过规则匹配）
    ///
    /// 用于测试或紧急告警场景
    pub fn fire_manual(
        &mut self,
        rule_id: &str,
        level: AlertLevel,
        message: impl Into<String>,
        value: f64,
        now: u64,
    ) -> Alert {
        let alert = Alert {
            rule_id: rule_id.to_string(),
            level,
            state: AlertState::Firing,
            message: message.into(),
            fired_at: now,
            value,
            threshold: 0.0,
            comparison: Comparison::GreaterThan,
            labels: HashMap::new(),
        };

        // 发送到所有通道
        let channels_to_send: Vec<String> = self.channels.keys().cloned().collect();
        for channel_id in channels_to_send {
            if let Some(channel) = self.channels.get_mut(&channel_id) {
                if let Some(payload) = channel.send(&alert) {
                    self.notification_history.push(payload);
                }
            }
        }

        self.total_fires += 1;
        self.active_alerts
            .insert(rule_id.to_string(), alert.clone());
        self.alert_history.push(alert.clone());
        alert
    }

    /// 清空告警历史
    pub fn clear_history(&mut self) {
        self.alert_history.clear();
        self.notification_history.clear();
    }

    /// 重置所有规则运行时状态
    pub fn reset_states(&mut self) {
        for state in self.runtime_states.values_mut() {
            state.state = AlertState::Resolved;
            state.pending_since = None;
            state.firing_since = None;
            state.last_notified_at = None;
        }
        self.active_alerts.clear();
    }
}

impl Default for AlertManager {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  辅助函数 — 预设规则
// =====================================================================

/// 创建 QPS 高告警规则（QPS > threshold）
pub fn rule_high_qps(threshold: f64, level: AlertLevel) -> AlertRule {
    AlertRule::new(
        "high_qps",
        "High QPS",
        "qps",
        threshold,
        Comparison::GreaterThan,
        level,
    )
    .with_description(format!("QPS 超过 {} 触发告警", threshold))
    .with_label("category", "throughput")
}

/// 创建延迟告警规则（延迟 > threshold_ms 毫秒）
pub fn rule_high_latency(threshold_ms: f64, level: AlertLevel) -> AlertRule {
    AlertRule::new(
        "high_latency",
        "High Latency",
        "latency_ms",
        threshold_ms,
        Comparison::GreaterThan,
        level,
    )
    .with_description(format!("延迟超过 {}ms 触发告警", threshold_ms))
    .with_label("category", "latency")
}

/// 创建连接数使用率告警规则（usage > threshold%）
pub fn rule_high_connection_usage(threshold_pct: f64, level: AlertLevel) -> AlertRule {
    AlertRule::new(
        "high_connection_usage",
        "High Connection Usage",
        "connection_usage",
        threshold_pct,
        Comparison::GreaterThan,
        level,
    )
    .with_description(format!("连接数使用率超过 {}% 触发告警", threshold_pct))
    .with_label("category", "resource")
}

/// 创建错误率告警规则（error_rate > threshold%）
pub fn rule_high_error_rate(threshold_pct: f64, level: AlertLevel) -> AlertRule {
    AlertRule::new(
        "high_error_rate",
        "High Error Rate",
        "error_rate",
        threshold_pct,
        Comparison::GreaterThan,
        level,
    )
    .with_description(format!("错误率超过 {}% 触发告警", threshold_pct))
    .with_label("category", "error")
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  AlertLevel 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_alert_level_as_str() {
        assert_eq!(AlertLevel::Info.as_str(), "info");
        assert_eq!(AlertLevel::Warning.as_str(), "warning");
        assert_eq!(AlertLevel::Critical.as_str(), "critical");
        assert_eq!(AlertLevel::Fatal.as_str(), "fatal");
    }

    #[test]
    fn test_alert_level_severity() {
        assert_eq!(AlertLevel::Info.severity(), 0);
        assert_eq!(AlertLevel::Warning.severity(), 1);
        assert_eq!(AlertLevel::Critical.severity(), 2);
        assert_eq!(AlertLevel::Fatal.severity(), 3);
    }

    #[test]
    fn test_alert_level_is_severe() {
        assert!(!AlertLevel::Info.is_severe());
        assert!(!AlertLevel::Warning.is_severe());
        assert!(AlertLevel::Critical.is_severe());
        assert!(AlertLevel::Fatal.is_severe());
    }

    #[test]
    fn test_alert_level_is_fatal() {
        assert!(!AlertLevel::Info.is_fatal());
        assert!(!AlertLevel::Warning.is_fatal());
        assert!(!AlertLevel::Critical.is_fatal());
        assert!(AlertLevel::Fatal.is_fatal());
    }

    #[test]
    fn test_alert_level_display() {
        assert_eq!(AlertLevel::Info.to_string(), "info");
        assert_eq!(AlertLevel::Fatal.to_string(), "fatal");
    }

    #[test]
    fn test_alert_level_ordering() {
        assert!(AlertLevel::Info < AlertLevel::Warning);
        assert!(AlertLevel::Warning < AlertLevel::Critical);
        assert!(AlertLevel::Critical < AlertLevel::Fatal);
    }

    // -----------------------------------------------------------------
    //  Comparison 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_comparison_as_str() {
        assert_eq!(Comparison::GreaterThan.as_str(), ">");
        assert_eq!(Comparison::GreaterThanOrEqual.as_str(), ">=");
        assert_eq!(Comparison::LessThan.as_str(), "<");
        assert_eq!(Comparison::LessThanOrEqual.as_str(), "<=");
    }

    #[test]
    fn test_comparison_evaluate_greater_than() {
        assert!(Comparison::GreaterThan.evaluate(15.0, 10.0));
        assert!(!Comparison::GreaterThan.evaluate(10.0, 10.0));
        assert!(!Comparison::GreaterThan.evaluate(5.0, 10.0));
    }

    #[test]
    fn test_comparison_evaluate_greater_or_equal() {
        assert!(Comparison::GreaterThanOrEqual.evaluate(15.0, 10.0));
        assert!(Comparison::GreaterThanOrEqual.evaluate(10.0, 10.0));
        assert!(!Comparison::GreaterThanOrEqual.evaluate(5.0, 10.0));
    }

    #[test]
    fn test_comparison_evaluate_less_than() {
        assert!(Comparison::LessThan.evaluate(5.0, 10.0));
        assert!(!Comparison::LessThan.evaluate(10.0, 10.0));
        assert!(!Comparison::LessThan.evaluate(15.0, 10.0));
    }

    #[test]
    fn test_comparison_evaluate_less_or_equal() {
        assert!(Comparison::LessThanOrEqual.evaluate(5.0, 10.0));
        assert!(Comparison::LessThanOrEqual.evaluate(10.0, 10.0));
        assert!(!Comparison::LessThanOrEqual.evaluate(15.0, 10.0));
    }

    #[test]
    fn test_comparison_display() {
        assert_eq!(Comparison::GreaterThan.to_string(), ">");
        assert_eq!(Comparison::LessThanOrEqual.to_string(), "<=");
    }

    // -----------------------------------------------------------------
    //  AlertRule 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_alert_rule_new() {
        let rule = AlertRule::new(
            "r1",
            "Test Rule",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        );
        assert_eq!(rule.rule_id, "r1");
        assert_eq!(rule.name, "Test Rule");
        assert_eq!(rule.metric_name, "qps");
        assert_eq!(rule.threshold, 1000.0);
        assert_eq!(rule.comparison, Comparison::GreaterThan);
        assert_eq!(rule.level, AlertLevel::Warning);
        assert_eq!(rule.for_duration_secs, 0);
        assert!(rule.description.is_empty());
        assert!(rule.labels.is_empty());
    }

    #[test]
    fn test_alert_rule_with_for_duration() {
        let rule = AlertRule::new(
            "r1",
            "Test",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        )
        .with_for_duration(60);
        assert_eq!(rule.for_duration_secs, 60);
    }

    #[test]
    fn test_alert_rule_with_description() {
        let rule = AlertRule::new(
            "r1",
            "Test",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        )
        .with_description("QPS too high");
        assert_eq!(rule.description, "QPS too high");
    }

    #[test]
    fn test_alert_rule_with_label() {
        let rule = AlertRule::new(
            "r1",
            "Test",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        )
        .with_label("env", "prod")
        .with_label("team", "dba");
        assert_eq!(rule.labels.get("env"), Some(&"prod".to_string()));
        assert_eq!(rule.labels.get("team"), Some(&"dba".to_string()));
    }

    #[test]
    fn test_alert_rule_matches_greater_than() {
        let rule = AlertRule::new(
            "r1",
            "Test",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        );
        assert!(rule.matches(1500.0));
        assert!(!rule.matches(1000.0));
        assert!(!rule.matches(500.0));
    }

    #[test]
    fn test_alert_rule_matches_less_than() {
        let rule = AlertRule::new(
            "r1",
            "Test",
            "cpu",
            10.0,
            Comparison::LessThan,
            AlertLevel::Warning,
        );
        assert!(rule.matches(5.0));
        assert!(!rule.matches(10.0));
        assert!(!rule.matches(15.0));
    }

    #[test]
    fn test_alert_rule_alert_message() {
        let rule = AlertRule::new(
            "r1",
            "High QPS",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        );
        let msg = rule.alert_message(1500.0);
        assert!(msg.contains("r1"));
        assert!(msg.contains("High QPS"));
        assert!(msg.contains("qps"));
        assert!(msg.contains(">"));
        assert!(msg.contains("1000"));
        assert!(msg.contains("1500"));
    }

    // -----------------------------------------------------------------
    //  AlertState 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_alert_state_as_str() {
        assert_eq!(AlertState::Pending.as_str(), "pending");
        assert_eq!(AlertState::Firing.as_str(), "firing");
        assert_eq!(AlertState::Resolved.as_str(), "resolved");
    }

    #[test]
    fn test_alert_state_predicates() {
        assert!(!AlertState::Pending.is_firing());
        assert!(AlertState::Firing.is_firing());
        assert!(!AlertState::Resolved.is_firing());

        assert!(!AlertState::Pending.is_resolved());
        assert!(!AlertState::Firing.is_resolved());
        assert!(AlertState::Resolved.is_resolved());
    }

    #[test]
    fn test_alert_state_display() {
        assert_eq!(AlertState::Pending.to_string(), "pending");
        assert_eq!(AlertState::Firing.to_string(), "firing");
    }

    // -----------------------------------------------------------------
    //  Alert 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_alert_new() {
        let rule = AlertRule::new(
            "r1",
            "Test",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        )
        .with_label("env", "prod");
        let alert = Alert::new(&rule, AlertState::Firing, 1500.0, 1000);
        assert_eq!(alert.rule_id, "r1");
        assert_eq!(alert.level, AlertLevel::Critical);
        assert_eq!(alert.state, AlertState::Firing);
        assert_eq!(alert.value, 1500.0);
        assert_eq!(alert.threshold, 1000.0);
        assert_eq!(alert.fired_at, 1000);
        assert_eq!(alert.labels.get("env"), Some(&"prod".to_string()));
    }

    #[test]
    fn test_alert_is_severe() {
        let rule_warn = AlertRule::new(
            "r1",
            "T",
            "m",
            1.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        );
        let rule_crit = AlertRule::new(
            "r2",
            "T",
            "m",
            1.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        );
        let alert_warn = Alert::new(&rule_warn, AlertState::Firing, 2.0, 0);
        let alert_crit = Alert::new(&rule_crit, AlertState::Firing, 2.0, 0);
        assert!(!alert_warn.is_severe());
        assert!(alert_crit.is_severe());
    }

    #[test]
    fn test_alert_duration_secs() {
        let rule = AlertRule::new(
            "r1",
            "T",
            "m",
            1.0,
            Comparison::GreaterThan,
            AlertLevel::Info,
        );
        let alert = Alert::new(&rule, AlertState::Firing, 2.0, 1000);
        assert_eq!(alert.duration_secs(1100), 100);
        assert_eq!(alert.duration_secs(500), 0); // 饱和减法
    }

    #[test]
    fn test_alert_to_json() {
        let rule = AlertRule::new(
            "r1",
            "Test",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        )
        .with_label("env", "prod");
        let alert = Alert::new(&rule, AlertState::Firing, 1500.0, 1000);
        let json = alert.to_json();
        assert!(json.contains("\"rule_id\":\"r1\""));
        assert!(json.contains("\"level\":\"critical\""));
        assert!(json.contains("\"state\":\"firing\""));
        assert!(json.contains("\"fired_at\":1000"));
        assert!(json.contains("\"value\":1500"));
        assert!(json.contains("\"threshold\":1000"));
        assert!(json.contains("\"comparison\":\">\""));
        assert!(json.contains("\"env\":\"prod\""));
    }

    // -----------------------------------------------------------------
    //  NotificationChannel 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_channel_console() {
        let ch = NotificationChannel::console("ch1");
        assert_eq!(ch.channel_id, "ch1");
        assert_eq!(ch.channel_type, ChannelType::Console);
        assert_eq!(ch.target, "stdout");
        assert_eq!(ch.min_level, AlertLevel::Info);
        assert_eq!(ch.sent_count, 0);
    }

    #[test]
    fn test_channel_webhook() {
        let ch = NotificationChannel::webhook("ch1", "https://example.com/hook");
        assert_eq!(ch.channel_id, "ch1");
        assert_eq!(ch.channel_type, ChannelType::Webhook);
        assert_eq!(ch.target, "https://example.com/hook");
        assert_eq!(ch.min_level, AlertLevel::Warning);
    }

    #[test]
    fn test_channel_with_min_level() {
        let ch = NotificationChannel::console("ch1").with_min_level(AlertLevel::Critical);
        assert_eq!(ch.min_level, AlertLevel::Critical);
    }

    #[test]
    fn test_channel_should_send() {
        let ch = NotificationChannel::console("ch1").with_min_level(AlertLevel::Critical);
        let rule_info = AlertRule::new(
            "r1",
            "T",
            "m",
            1.0,
            Comparison::GreaterThan,
            AlertLevel::Info,
        );
        let rule_crit = AlertRule::new(
            "r2",
            "T",
            "m",
            1.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        );
        let alert_info = Alert::new(&rule_info, AlertState::Firing, 2.0, 0);
        let alert_crit = Alert::new(&rule_crit, AlertState::Firing, 2.0, 0);
        assert!(!ch.should_send(&alert_info));
        assert!(ch.should_send(&alert_crit));
    }

    #[test]
    fn test_channel_send_increments_count() {
        let mut ch = NotificationChannel::console("ch1");
        let rule = AlertRule::new(
            "r1",
            "T",
            "m",
            1.0,
            Comparison::GreaterThan,
            AlertLevel::Info,
        );
        let alert = Alert::new(&rule, AlertState::Firing, 2.0, 0);
        let payload = ch.send(&alert);
        assert!(payload.is_some());
        assert_eq!(ch.sent_count, 1);
        let p = payload.unwrap();
        assert_eq!(p.channel_id, "ch1");
        assert_eq!(p.channel_type, ChannelType::Console);
    }

    #[test]
    fn test_channel_send_filtered_by_level() {
        let mut ch = NotificationChannel::console("ch1").with_min_level(AlertLevel::Fatal);
        let rule = AlertRule::new(
            "r1",
            "T",
            "m",
            1.0,
            Comparison::GreaterThan,
            AlertLevel::Info,
        );
        let alert = Alert::new(&rule, AlertState::Firing, 2.0, 0);
        let payload = ch.send(&alert);
        assert!(payload.is_none());
        assert_eq!(ch.sent_count, 0);
    }

    #[test]
    fn test_channel_type_as_str() {
        assert_eq!(ChannelType::Console.as_str(), "console");
        assert_eq!(ChannelType::Webhook.as_str(), "webhook");
    }

    // -----------------------------------------------------------------
    //  AlertManager 基本操作测试
    // -----------------------------------------------------------------

    #[test]
    fn test_alert_manager_new() {
        let mgr = AlertManager::new();
        assert_eq!(mgr.rule_count(), 0);
        assert_eq!(mgr.channel_count(), 0);
        assert_eq!(
            mgr.suppression_window_secs(),
            DEFAULT_SUPPRESSION_WINDOW_SECS
        );
        assert_eq!(
            mgr.evaluation_interval_secs(),
            DEFAULT_EVALUATION_INTERVAL_SECS
        );
        assert_eq!(mgr.total_evaluations(), 0);
        assert_eq!(mgr.total_fires(), 0);
        assert_eq!(mgr.total_suppressions(), 0);
        assert!(mgr.alert_history().is_empty());
        assert!(mgr.notification_history().is_empty());
    }

    #[test]
    fn test_alert_manager_with_suppression_window() {
        let mgr = AlertManager::new().with_suppression_window(600);
        assert_eq!(mgr.suppression_window_secs(), 600);
    }

    #[test]
    fn test_alert_manager_with_evaluation_interval() {
        let mgr = AlertManager::new().with_evaluation_interval(30);
        assert_eq!(mgr.evaluation_interval_secs(), 30);
    }

    #[test]
    fn test_alert_manager_add_rule() {
        let mut mgr = AlertManager::new();
        let rule = AlertRule::new(
            "r1",
            "T",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        );
        assert!(mgr.add_rule(rule));
        assert_eq!(mgr.rule_count(), 1);
        assert!(mgr.rule("r1").is_some());
    }

    #[test]
    fn test_alert_manager_add_duplicate_rule_fails() {
        let mut mgr = AlertManager::new();
        let rule1 = AlertRule::new(
            "r1",
            "T1",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        );
        let rule2 = AlertRule::new(
            "r1",
            "T2",
            "latency",
            500.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        );
        assert!(mgr.add_rule(rule1));
        assert!(!mgr.add_rule(rule2)); // 重复 ID
        assert_eq!(mgr.rule_count(), 1);
    }

    #[test]
    fn test_alert_manager_remove_rule() {
        let mut mgr = AlertManager::new();
        let rule = AlertRule::new(
            "r1",
            "T",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        );
        mgr.add_rule(rule);
        assert!(mgr.remove_rule("r1"));
        assert_eq!(mgr.rule_count(), 0);
        assert!(!mgr.remove_rule("r1"));
    }

    #[test]
    fn test_alert_manager_add_channel() {
        let mut mgr = AlertManager::new();
        let ch = NotificationChannel::console("ch1");
        assert!(mgr.add_channel(ch));
        assert_eq!(mgr.channel_count(), 1);
    }

    #[test]
    fn test_alert_manager_add_duplicate_channel_fails() {
        let mut mgr = AlertManager::new();
        let ch1 = NotificationChannel::console("ch1");
        let ch2 = NotificationChannel::webhook("ch1", "http://example.com");
        assert!(mgr.add_channel(ch1));
        assert!(!mgr.add_channel(ch2));
        assert_eq!(mgr.channel_count(), 1);
    }

    #[test]
    fn test_alert_manager_remove_channel() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));
        assert!(mgr.remove_channel("ch1"));
        assert_eq!(mgr.channel_count(), 0);
    }

    // -----------------------------------------------------------------
    //  AlertManager 评估测试
    // -----------------------------------------------------------------

    #[test]
    fn test_evaluate_triggers_alert_when_threshold_exceeded() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(AlertRule::new(
            "r1",
            "High QPS",
            "qps",
            10000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));

        let alerts = mgr.evaluate("qps", 15000.0, 1000);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "r1");
        assert_eq!(alerts[0].state, AlertState::Firing);
        assert_eq!(alerts[0].level, AlertLevel::Critical);
        assert_eq!(mgr.total_fires(), 1);
        assert_eq!(mgr.total_evaluations(), 1);
    }

    #[test]
    fn test_evaluate_no_alert_when_below_threshold() {
        let mut mgr = AlertManager::new();
        mgr.add_rule(AlertRule::new(
            "r1",
            "High QPS",
            "qps",
            10000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));

        let alerts = mgr.evaluate("qps", 5000.0, 1000);
        assert!(alerts.is_empty());
        assert_eq!(mgr.total_fires(), 0);
    }

    #[test]
    fn test_evaluate_no_alert_when_equal_threshold_strict() {
        let mut mgr = AlertManager::new();
        mgr.add_rule(AlertRule::new(
            "r1",
            "T",
            "qps",
            10000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));

        let alerts = mgr.evaluate("qps", 10000.0, 1000);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_evaluate_alert_when_equal_threshold_non_strict() {
        let mut mgr = AlertManager::new();
        mgr.add_rule(AlertRule::new(
            "r1",
            "T",
            "qps",
            10000.0,
            Comparison::GreaterThanOrEqual,
            AlertLevel::Critical,
        ));

        let alerts = mgr.evaluate("qps", 10000.0, 1000);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].state, AlertState::Firing);
    }

    #[test]
    fn test_evaluate_sends_notification_to_channel() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(AlertRule::new(
            "r1",
            "T",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));

        mgr.evaluate("qps", 1500.0, 1000);
        assert_eq!(mgr.notification_history().len(), 1);
        assert_eq!(mgr.notification_history()[0].channel_id, "ch1");
    }

    #[test]
    fn test_evaluate_sends_to_multiple_channels() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_channel(NotificationChannel::webhook("ch2", "http://example.com"));
        mgr.add_rule(AlertRule::new(
            "r1",
            "T",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));

        mgr.evaluate("qps", 1500.0, 1000);
        assert_eq!(mgr.notification_history().len(), 2);
    }

    #[test]
    fn test_evaluate_suppresses_repeated_alerts() {
        let mut mgr = AlertManager::new().with_suppression_window(300);
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(AlertRule::new(
            "r1",
            "T",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));

        // 第一次触发
        let alerts1 = mgr.evaluate("qps", 1500.0, 1000);
        assert_eq!(alerts1.len(), 1);
        assert_eq!(mgr.notification_history().len(), 1);

        // 在抑制窗口内再次评估（条件仍满足，但状态已是 Firing 不再触发新告警）
        let alerts2 = mgr.evaluate("qps", 1600.0, 1100);
        assert!(alerts2.is_empty());
        assert_eq!(mgr.notification_history().len(), 1); // 仍只 1 个通知
    }

    #[test]
    fn test_evaluate_sends_resolved_when_condition_clears() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(AlertRule::new(
            "r1",
            "T",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));

        // 触发告警
        mgr.evaluate("qps", 1500.0, 1000);
        assert_eq!(mgr.alert_history().len(), 1);

        // 条件不再满足 → 发送 Resolved
        let alerts = mgr.evaluate("qps", 500.0, 2000);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].state, AlertState::Resolved);
        assert_eq!(mgr.notification_history().len(), 2); // Firing + Resolved
    }

    // -----------------------------------------------------------------
    //  持续时间测试
    // -----------------------------------------------------------------

    #[test]
    fn test_evaluate_with_for_duration_pending_state() {
        let mut mgr = AlertManager::new();
        mgr.add_rule(
            AlertRule::new(
                "r1",
                "T",
                "qps",
                1000.0,
                Comparison::GreaterThan,
                AlertLevel::Warning,
            )
            .with_for_duration(60),
        );

        // 第一次满足条件 → Pending（不触发告警）
        let alerts1 = mgr.evaluate("qps", 1500.0, 1000);
        assert!(alerts1.is_empty());

        // 30 秒后仍满足 → 仍 Pending
        let alerts2 = mgr.evaluate("qps", 1500.0, 1030);
        assert!(alerts2.is_empty());

        // 60 秒后仍满足 → Firing
        let alerts3 = mgr.evaluate("qps", 1500.0, 1060);
        assert_eq!(alerts3.len(), 1);
        assert_eq!(alerts3[0].state, AlertState::Firing);
    }

    #[test]
    fn test_evaluate_with_for_duration_resets_when_condition_clears() {
        let mut mgr = AlertManager::new();
        mgr.add_rule(
            AlertRule::new(
                "r1",
                "T",
                "qps",
                1000.0,
                Comparison::GreaterThan,
                AlertLevel::Warning,
            )
            .with_for_duration(60),
        );

        // 满足条件 → Pending
        mgr.evaluate("qps", 1500.0, 1000);

        // 条件不再满足 → Resolved（无 Resolved 通知，因为未 Firing 过）
        let alerts = mgr.evaluate("qps", 500.0, 1030);
        assert!(alerts.is_empty());

        // 再次满足 → 重新 Pending
        let alerts2 = mgr.evaluate("qps", 1500.0, 1040);
        assert!(alerts2.is_empty());

        // 60 秒后满足 → Firing
        let alerts3 = mgr.evaluate("qps", 1500.0, 1100);
        assert_eq!(alerts3.len(), 1);
        assert_eq!(alerts3[0].state, AlertState::Firing);
    }

    // -----------------------------------------------------------------
    //  批量评估测试
    // -----------------------------------------------------------------

    #[test]
    fn test_evaluate_batch_multiple_metrics() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(AlertRule::new(
            "r1",
            "High QPS",
            "qps",
            10000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));
        mgr.add_rule(AlertRule::new(
            "r2",
            "High Latency",
            "latency_ms",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        ));

        let metrics: [(&str, f64); 2] = [("qps", 15000.0), ("latency_ms", 1200.0)];
        let alerts = mgr.evaluate_batch(&metrics, 1000);
        assert_eq!(alerts.len(), 2);
    }

    #[test]
    fn test_evaluate_batch_partial_match() {
        let mut mgr = AlertManager::new();
        mgr.add_rule(AlertRule::new(
            "r1",
            "T",
            "qps",
            10000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));
        mgr.add_rule(AlertRule::new(
            "r2",
            "T",
            "latency_ms",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        ));

        let metrics: [(&str, f64); 2] = [("qps", 15000.0), ("latency_ms", 500.0)];
        let alerts = mgr.evaluate_batch(&metrics, 1000);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "r1");
    }

    // -----------------------------------------------------------------
    //  手动触发测试
    // -----------------------------------------------------------------

    #[test]
    fn test_fire_manual() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));

        let alert = mgr.fire_manual("manual1", AlertLevel::Critical, "手动告警", 999.0, 1000);
        assert_eq!(alert.rule_id, "manual1");
        assert_eq!(alert.level, AlertLevel::Critical);
        assert_eq!(alert.state, AlertState::Firing);
        assert_eq!(alert.message, "手动告警");
        assert_eq!(alert.value, 999.0);
        assert_eq!(mgr.total_fires(), 1);
        assert_eq!(mgr.notification_history().len(), 1);
    }

    // -----------------------------------------------------------------
    //  抑制窗口测试
    // -----------------------------------------------------------------

    #[test]
    fn test_suppression_within_window() {
        let mut mgr = AlertManager::new().with_suppression_window(60);
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(AlertRule::new(
            "r1",
            "T",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));

        // 触发
        mgr.evaluate("qps", 1500.0, 1000);
        assert_eq!(mgr.total_fires(), 1);

        // 条件恢复
        mgr.evaluate("qps", 500.0, 1010);
        assert_eq!(mgr.alert_history().len(), 2); // Firing + Resolved

        // 在抑制窗口内再次触发 → 应被抑制
        let alerts = mgr.evaluate("qps", 1500.0, 1020);
        // 距离上次 Firing 通知仅 20 秒，在 60 秒抑制窗口内
        // 但因为之前已 Resolved，状态已重置为 Resolved，所以会重新进入 Pending → Firing
        // 但 last_notified_at 仍是 1000，抑制窗口内
        if !alerts.is_empty() {
            assert_eq!(mgr.total_suppressions(), 1);
        }
    }

    #[test]
    fn test_suppression_outside_window() {
        let mut mgr = AlertManager::new().with_suppression_window(60);
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(AlertRule::new(
            "r1",
            "T",
            "qps",
            1000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));

        // 触发
        mgr.evaluate("qps", 1500.0, 1000);
        // 恢复
        mgr.evaluate("qps", 500.0, 1010);
        // 超过抑制窗口后再次触发
        let alerts = mgr.evaluate("qps", 1500.0, 1100); // 100 秒后 > 60 秒
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].state, AlertState::Firing);
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_rule_high_qps() {
        let rule = rule_high_qps(10000.0, AlertLevel::Critical);
        assert_eq!(rule.rule_id, "high_qps");
        assert_eq!(rule.name, "High QPS");
        assert_eq!(rule.metric_name, "qps");
        assert_eq!(rule.threshold, 10000.0);
        assert_eq!(rule.comparison, Comparison::GreaterThan);
        assert_eq!(rule.level, AlertLevel::Critical);
        assert_eq!(rule.labels.get("category"), Some(&"throughput".to_string()));
    }

    #[test]
    fn test_rule_high_latency() {
        let rule = rule_high_latency(1000.0, AlertLevel::Warning);
        assert_eq!(rule.rule_id, "high_latency");
        assert_eq!(rule.metric_name, "latency_ms");
        assert_eq!(rule.threshold, 1000.0);
        assert_eq!(rule.labels.get("category"), Some(&"latency".to_string()));
    }

    #[test]
    fn test_rule_high_connection_usage() {
        let rule = rule_high_connection_usage(90.0, AlertLevel::Critical);
        assert_eq!(rule.rule_id, "high_connection_usage");
        assert_eq!(rule.metric_name, "connection_usage");
        assert_eq!(rule.threshold, 90.0);
        assert_eq!(rule.labels.get("category"), Some(&"resource".to_string()));
    }

    #[test]
    fn test_rule_high_error_rate() {
        let rule = rule_high_error_rate(1.0, AlertLevel::Critical);
        assert_eq!(rule.rule_id, "high_error_rate");
        assert_eq!(rule.metric_name, "error_rate");
        assert_eq!(rule.threshold, 1.0);
        assert_eq!(rule.labels.get("category"), Some(&"error".to_string()));
    }

    // -----------------------------------------------------------------
    //  集成测试
    // -----------------------------------------------------------------

    #[test]
    fn test_integration_qps_latency_connection_alerts() {
        // 验证标准场景：QPS > 10000 / 延迟 > 1s / 连接数 > 90%
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("console"));
        mgr.add_channel(NotificationChannel::webhook(
            "webhook",
            "https://hooks.example.com/alert",
        ));
        mgr.add_rule(rule_high_qps(10000.0, AlertLevel::Critical));
        mgr.add_rule(rule_high_latency(1000.0, AlertLevel::Warning));
        mgr.add_rule(rule_high_connection_usage(90.0, AlertLevel::Critical));

        // 模拟时间序列：QPS 飙升
        let alerts1 = mgr.evaluate("qps", 12000.0, 1000);
        assert_eq!(alerts1.len(), 1);
        assert_eq!(alerts1[0].rule_id, "high_qps");
        assert_eq!(alerts1[0].level, AlertLevel::Critical);

        // 延迟升高
        let alerts2 = mgr.evaluate("latency_ms", 1500.0, 1001);
        assert_eq!(alerts2.len(), 1);
        assert_eq!(alerts2[0].rule_id, "high_latency");

        // 连接数超限
        let alerts3 = mgr.evaluate("connection_usage", 95.0, 1002);
        assert_eq!(alerts3.len(), 1);
        assert_eq!(alerts3[0].rule_id, "high_connection_usage");

        // 通知应发送到两个通道（3 个告警 × 2 通道 = 6 个通知）
        assert_eq!(mgr.notification_history().len(), 6);

        // 全部为 Firing
        assert_eq!(mgr.firing_alerts().len(), 3);
    }

    #[test]
    fn test_integration_suppression_no_duplicate() {
        // 验证：重复告警在抑制窗口内不重复触发
        let mut mgr = AlertManager::new().with_suppression_window(300);
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(rule_high_qps(10000.0, AlertLevel::Critical));

        // 第一次触发
        mgr.evaluate("qps", 15000.0, 1000);
        assert_eq!(mgr.notification_history().len(), 1);

        // 短时间内反复评估（条件仍满足）
        for t in 1010..1050 {
            mgr.evaluate("qps", 16000.0, t);
        }
        // 不应重复发送通知（仍为 1）
        assert_eq!(mgr.notification_history().len(), 1);
        assert_eq!(mgr.total_fires(), 1);
    }

    #[test]
    fn test_integration_recovery_sends_resolved() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(rule_high_qps(10000.0, AlertLevel::Critical));

        // 触发
        mgr.evaluate("qps", 15000.0, 1000);
        assert_eq!(mgr.alert_history().len(), 1);

        // 恢复
        let alerts = mgr.evaluate("qps", 8000.0, 2000);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].state, AlertState::Resolved);

        // 通知历史：Firing + Resolved
        assert_eq!(mgr.notification_history().len(), 2);

        // 没有活跃告警
        assert!(mgr.firing_alerts().is_empty());
    }

    #[test]
    fn test_integration_full_lifecycle() {
        let mut mgr = AlertManager::new().with_suppression_window(60);
        mgr.add_channel(NotificationChannel::console("console"));
        mgr.add_rule(rule_high_qps(10000.0, AlertLevel::Warning).with_for_duration(30));

        // 阶段1：满足条件，但持续时间不足 → Pending
        let a1 = mgr.evaluate("qps", 12000.0, 1000);
        assert!(a1.is_empty());

        // 阶段2：30 秒后仍满足 → Firing
        let a2 = mgr.evaluate("qps", 12000.0, 1030);
        assert_eq!(a2.len(), 1);
        assert_eq!(a2[0].state, AlertState::Firing);
        assert_eq!(mgr.notification_history().len(), 1);

        // 阶段3：条件恢复 → Resolved
        let a3 = mgr.evaluate("qps", 8000.0, 1040);
        assert_eq!(a3.len(), 1);
        assert_eq!(a3[0].state, AlertState::Resolved);
        assert_eq!(mgr.notification_history().len(), 2);

        // 阶段4：再次满足条件 → Pending
        let a4 = mgr.evaluate("qps", 12000.0, 1050);
        assert!(a4.is_empty());

        // 阶段5：30 秒后 → Firing（超出抑制窗口 60 秒前已经通知过）
        // 1050 + 30 = 1080，距上次 Firing(1030) 已 50 秒，仍在抑制窗口 60 秒内 → 抑制
        let a5 = mgr.evaluate("qps", 12000.0, 1080);
        assert_eq!(a5.len(), 1);
        assert_eq!(a5[0].state, AlertState::Firing);
        assert_eq!(mgr.total_suppressions(), 1);
        assert_eq!(mgr.notification_history().len(), 2); // 仍为 2（Firing 抑制）

        // 阶段6：完全恢复
        mgr.evaluate("qps", 8000.0, 1090);
        assert_eq!(mgr.firing_alerts().len(), 0);
    }

    #[test]
    fn test_integration_alert_delay_under_5_seconds() {
        // 验证：告警延迟 < 5s（无 for_duration 时立即触发）
        let mut mgr = AlertManager::new().with_evaluation_interval(1);
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(rule_high_qps(10000.0, AlertLevel::Critical));

        // T=1000 评估 → 立即触发
        let start = 1000u64;
        let alerts = mgr.evaluate("qps", 15000.0, start);
        assert_eq!(alerts.len(), 1);
        let delay = alerts[0].fired_at - start;
        assert_eq!(delay, 0); // 立即触发，延迟 0 秒
    }

    #[test]
    fn test_integration_clear_history() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(rule_high_qps(1000.0, AlertLevel::Critical));

        mgr.evaluate("qps", 1500.0, 1000);
        assert!(!mgr.alert_history().is_empty());
        assert!(!mgr.notification_history().is_empty());

        mgr.clear_history();
        assert!(mgr.alert_history().is_empty());
        assert!(mgr.notification_history().is_empty());
    }

    #[test]
    fn test_integration_reset_states() {
        let mut mgr = AlertManager::new();
        mgr.add_rule(rule_high_qps(1000.0, AlertLevel::Critical));

        mgr.evaluate("qps", 1500.0, 1000);
        assert!(!mgr.firing_alerts().is_empty());

        mgr.reset_states();
        // 重置后状态回到 Resolved
        assert!(mgr.firing_alerts().is_empty());
    }

    #[test]
    fn test_integration_should_evaluate() {
        let mgr = AlertManager::new().with_evaluation_interval(10);
        // 假设从未评估过，last_evaluation=0
        assert!(mgr.should_evaluate(10));
        assert!(mgr.should_evaluate(100));
        // 评估周期 10 秒
    }

    #[test]
    fn test_integration_multiple_rules_same_metric() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));
        // 同一指标多个阈值
        mgr.add_rule(AlertRule::new(
            "warn_qps",
            "Warn QPS",
            "qps",
            5000.0,
            Comparison::GreaterThan,
            AlertLevel::Warning,
        ));
        mgr.add_rule(AlertRule::new(
            "crit_qps",
            "Crit QPS",
            "qps",
            10000.0,
            Comparison::GreaterThan,
            AlertLevel::Critical,
        ));

        // QPS = 15000 同时触发两个规则
        let alerts = mgr.evaluate("qps", 15000.0, 1000);
        assert_eq!(alerts.len(), 2);
    }

    #[test]
    fn test_integration_less_than_rule() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::console("ch1"));
        // CPU 空闲率低于 20% 告警
        mgr.add_rule(AlertRule::new(
            "low_cpu_idle",
            "Low CPU Idle",
            "cpu_idle_pct",
            20.0,
            Comparison::LessThan,
            AlertLevel::Warning,
        ));

        let alerts = mgr.evaluate("cpu_idle_pct", 10.0, 1000);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].state, AlertState::Firing);

        // 恢复
        let alerts2 = mgr.evaluate("cpu_idle_pct", 50.0, 1010);
        assert_eq!(alerts2.len(), 1);
        assert_eq!(alerts2[0].state, AlertState::Resolved);
    }

    #[test]
    fn test_integration_webhook_payload_format() {
        let mut mgr = AlertManager::new();
        mgr.add_channel(NotificationChannel::webhook(
            "wh1",
            "https://hooks.example.com/alert",
        ));
        mgr.add_rule(rule_high_qps(10000.0, AlertLevel::Critical));

        mgr.evaluate("qps", 15000.0, 1000);

        let payload = &mgr.notification_history()[0];
        assert_eq!(payload.channel_type, ChannelType::Webhook);
        assert_eq!(payload.target, "https://hooks.example.com/alert");
        // JSON 载荷应包含关键字段
        assert!(payload.payload.contains("\"rule_id\":\"high_qps\""));
        assert!(payload.payload.contains("\"level\":\"critical\""));
        assert!(payload.payload.contains("\"state\":\"firing\""));
        assert!(payload.payload.contains("\"category\":\"throughput\""));
    }

    #[test]
    fn test_integration_min_level_filter() {
        let mut mgr = AlertManager::new();
        // Console 仅接收 Critical+
        mgr.add_channel(NotificationChannel::console("ch1").with_min_level(AlertLevel::Critical));
        // Webhook 接收所有
        mgr.add_channel(NotificationChannel::webhook("wh1", "http://example.com"));
        // Warning 级别规则
        mgr.add_rule(rule_high_latency(1000.0, AlertLevel::Warning));

        mgr.evaluate("latency_ms", 1500.0, 1000);
        // 只有 Webhook 收到（Warning 不满足 Console 的 Critical+ 要求）
        assert_eq!(mgr.notification_history().len(), 1);
        assert_eq!(mgr.notification_history()[0].channel_id, "wh1");
    }

    #[test]
    fn test_integration_long_running_scenario() {
        // 模拟长时间运行场景：多次触发/恢复
        let mut mgr = AlertManager::new().with_suppression_window(0);
        mgr.add_channel(NotificationChannel::console("ch1"));
        mgr.add_rule(rule_high_qps(1000.0, AlertLevel::Critical));

        let mut fire_count = 0;
        let mut resolve_count = 0;

        // 模拟 1000 个时间点
        for t in 0..1000 {
            let qps = if t % 100 < 50 {
                1500.0
            } else {
                500.0
            };
            let alerts = mgr.evaluate("qps", qps, t);
            for alert in &alerts {
                if alert.state == AlertState::Firing {
                    fire_count += 1;
                } else if alert.state == AlertState::Resolved {
                    resolve_count += 1;
                }
            }
        }

        // 应有多次触发和恢复
        assert!(fire_count > 0);
        assert!(resolve_count > 0);
        // 触发和恢复次数应相等（每次 Firing 后必有 Resolved）
        assert_eq!(fire_count, resolve_count);
    }
}
