// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 告警展示（spec §5.26 规则 4）
//!
//! 当前告警列表展示（级别/状态/时间），对接 alert-engine。

use chrono::{DateTime, Utc};

/// 告警级别
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum AlertSeverity {
    /// 信息
    Info,
    /// 警告
    Warning,
    /// 严重
    Critical,
}

impl AlertSeverity {
    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }

    /// 从字符串解析
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "info" => Some(Self::Info),
            "warning" => Some(Self::Warning),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }
}

/// 告警状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AlertState {
    /// 已触发
    Fired,
    /// 已恢复
    Resolved,
    /// 已静默
    Silenced,
    /// 已抑制
    Inhibited,
}

impl AlertState {
    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fired => "fired",
            Self::Resolved => "resolved",
            Self::Silenced => "silenced",
            Self::Inhibited => "inhibited",
        }
    }
}

/// 告警摘要（spec §5.26 规则 4）
#[derive(Debug, Clone, serde::Serialize)]
pub struct AlertSummary {
    /// 规则 ID
    pub rule_id: String,
    /// 告警名称
    pub name: String,
    /// 级别
    pub severity: AlertSeverity,
    /// 状态
    pub state: AlertState,
    /// 触发时间
    pub time: DateTime<Utc>,
    /// 消息
    pub message: String,
}

impl AlertSummary {
    /// 创建告警摘要
    pub fn new(
        rule_id: impl Into<String>,
        name: impl Into<String>,
        severity: AlertSeverity,
        state: AlertState,
    ) -> Self {
        Self {
            rule_id: rule_id.into(),
            name: name.into(),
            severity,
            state,
            time: Utc::now(),
            message: String::new(),
        }
    }

    /// 设置消息
    pub fn with_message(mut self, msg: impl Into<String>) -> Self {
        self.message = msg.into();
        self
    }
}

/// 告警展示服务（spec §5.26 规则 4）
pub struct AlertDisplayService;

impl AlertDisplayService {
    /// 过滤活跃告警（Fired 状态）
    pub fn filter_active(alerts: &[AlertSummary]) -> Vec<AlertSummary> {
        alerts
            .iter()
            .filter(|a| a.state == AlertState::Fired)
            .cloned()
            .collect()
    }

    /// 按级别排序（Critical 优先）
    pub fn sort_by_severity(alerts: &mut [AlertSummary]) {
        alerts.sort_by_key(|a| std::cmp::Reverse(a.severity));
    }

    /// 按级别过滤
    pub fn filter_by_severity(
        alerts: &[AlertSummary],
        min_severity: AlertSeverity,
    ) -> Vec<AlertSummary> {
        alerts
            .iter()
            .filter(|a| a.severity >= min_severity)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_severity_ordering() {
        assert!(AlertSeverity::Critical > AlertSeverity::Warning);
        assert!(AlertSeverity::Warning > AlertSeverity::Info);
    }

    #[test]
    fn test_alert_severity_parse() {
        assert_eq!(
            AlertSeverity::parse("critical"),
            Some(AlertSeverity::Critical)
        );
        assert_eq!(AlertSeverity::parse("invalid"), None);
    }

    #[test]
    fn test_alert_summary() {
        let alert = AlertSummary::new(
            "r1",
            "CPU 高负载",
            AlertSeverity::Critical,
            AlertState::Fired,
        )
        .with_message("CPU > 90%");
        assert_eq!(alert.rule_id, "r1");
        assert_eq!(alert.severity, AlertSeverity::Critical);
        assert_eq!(alert.message, "CPU > 90%");
    }

    #[test]
    fn test_filter_active() {
        let alerts = vec![
            AlertSummary::new("r1", "a1", AlertSeverity::Critical, AlertState::Fired),
            AlertSummary::new("r2", "a2", AlertSeverity::Warning, AlertState::Resolved),
            AlertSummary::new("r3", "a3", AlertSeverity::Info, AlertState::Fired),
        ];
        let active = AlertDisplayService::filter_active(&alerts);
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn test_sort_by_severity() {
        let mut alerts = vec![
            AlertSummary::new("r1", "a1", AlertSeverity::Info, AlertState::Fired),
            AlertSummary::new("r2", "a2", AlertSeverity::Critical, AlertState::Fired),
            AlertSummary::new("r3", "a3", AlertSeverity::Warning, AlertState::Fired),
        ];
        AlertDisplayService::sort_by_severity(&mut alerts);
        assert_eq!(alerts[0].severity, AlertSeverity::Critical);
        assert_eq!(alerts[1].severity, AlertSeverity::Warning);
        assert_eq!(alerts[2].severity, AlertSeverity::Info);
    }

    #[test]
    fn test_filter_by_severity() {
        let alerts = vec![
            AlertSummary::new("r1", "a1", AlertSeverity::Info, AlertState::Fired),
            AlertSummary::new("r2", "a2", AlertSeverity::Critical, AlertState::Fired),
            AlertSummary::new("r3", "a3", AlertSeverity::Warning, AlertState::Fired),
        ];
        let filtered = AlertDisplayService::filter_by_severity(&alerts, AlertSeverity::Warning);
        assert_eq!(filtered.len(), 2);
    }
}
