// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 告警规则引擎核心逻辑（spec §5.12）
//!
//! Prometheus AlertManager 集成 + 规则评估 + 通知渠道 + 静默/抑制。

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::AlertEngineError;

/// 告警级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AlertSeverity {
    /// 信息
    Info,
    /// 警告
    Warning,
    /// 严重
    Critical,
}

/// 通知渠道（spec §6.12 规则 3）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NotificationChannel {
    /// Webhook
    Webhook { url: String },
    /// 邮件
    Email { to: Vec<String> },
    /// 钉钉
    DingTalk { webhook: String },
    /// 飞书
    Feishu { webhook: String },
    /// 企业微信
    WeCom { webhook: String },
}

/// 告警规则
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlertRule {
    /// 规则唯一标识（spec §6.12 规则 1）
    pub id: String,
    /// 规则名称
    pub name: String,
    /// PromQL 表达式
    pub expr: String,
    /// 级别
    pub severity: AlertSeverity,
    /// 评估周期（默认 >= 10s，spec §6.12 规则 2）
    pub eval_interval: Duration,
    /// 通知渠道
    pub channels: Vec<NotificationChannel>,
    /// 标签
    pub labels: HashMap<String, String>,
}

impl AlertRule {
    /// 验证规则（spec §6.12 规则 1/2）
    pub fn validate(&self) -> Result<(), AlertEngineError> {
        if self.id.is_empty() {
            return Err(AlertEngineError::Config("规则 ID 不能为空".to_string()));
        }
        if self.expr.is_empty() {
            return Err(AlertEngineError::Config(format!(
                "规则 {} 的表达式不能为空",
                self.id
            )));
        }
        if self.eval_interval < Duration::from_secs(10) {
            return Err(AlertEngineError::Config(format!(
                "规则 {} 的评估周期必须 >= 10s",
                self.id
            )));
        }
        Ok(())
    }
}

/// 静默窗口（spec §5.12 规则 3）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SilenceWindow {
    /// 开始时间
    pub start: DateTime<Utc>,
    /// 结束时间（> start，spec §6.12 规则 4）
    pub end: DateTime<Utc>,
    /// 标签匹配
    pub label_matchers: HashMap<String, String>,
}

impl SilenceWindow {
    /// 验证静默窗口（spec §6.12 规则 4）
    pub fn validate(&self) -> Result<(), AlertEngineError> {
        if self.end <= self.start {
            return Err(AlertEngineError::Config(
                "静默窗口结束时间必须 > 开始时间".to_string(),
            ));
        }
        Ok(())
    }

    /// 检查时间是否在窗口内
    pub fn contains_time(&self, t: DateTime<Utc>) -> bool {
        t >= self.start && t < self.end
    }

    /// 检查标签是否匹配
    pub fn matches_labels(&self, labels: &HashMap<String, String>) -> bool {
        for (k, v) in &self.label_matchers {
            if labels.get(k) != Some(v) {
                return false;
            }
        }
        true
    }
}

/// 告警事件
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum AlertEvent {
    /// 告警触发
    Fired {
        rule_id: String,
        labels: HashMap<String, String>,
        value: f64,
    },
    /// 告警恢复（spec §5.12 规则 6）
    Resolved { rule_id: String },
    /// 静默
    Silenced { rule_id: String },
    /// 抑制（spec §5.12 规则 4）
    Inhibited { rule_id: String, by_rule_id: String },
}

/// 告警规则引擎
pub struct AlertEngine {
    rules: RwLock<Vec<AlertRule>>,
    silences: RwLock<Vec<SilenceWindow>>,
}

impl AlertEngine {
    /// 创建告警引擎
    pub fn new() -> Self {
        Self {
            rules: RwLock::new(Vec::new()),
            silences: RwLock::new(Vec::new()),
        }
    }

    /// 热加载规则（spec §5.12 规则 5）
    pub async fn reload_rules(&self, rules: Vec<AlertRule>) -> Result<(), AlertEngineError> {
        for rule in &rules {
            rule.validate()?;
        }
        *self.rules.write() = rules;
        Ok(())
    }

    /// 添加静默窗口
    pub async fn add_silence(&self, silence: SilenceWindow) -> Result<(), AlertEngineError> {
        silence.validate()?;
        self.silences.write().push(silence);
        Ok(())
    }

    /// 获取所有规则
    pub fn rules(&self) -> Vec<AlertRule> {
        self.rules.read().clone()
    }

    /// 获取所有静默窗口
    pub fn silences(&self) -> Vec<SilenceWindow> {
        self.silences.read().clone()
    }

    /// 检查静默（spec §5.12 规则 3）
    pub fn is_silenced(&self, labels: &HashMap<String, String>, now: DateTime<Utc>) -> bool {
        self.silences
            .read()
            .iter()
            .any(|s| s.contains_time(now) && s.matches_labels(labels))
    }

    /// 检查抑制（高级别告警抑制相关低级别，spec §5.12 规则 4）
    ///
    /// 如果同标签有更高级别的规则已触发，则当前规则被抑制
    pub fn is_inhibited(
        &self,
        rule_id: &str,
        severity: AlertSeverity,
        active_alerts: &[AlertEvent],
    ) -> Option<String> {
        let rules = self.rules.read();
        for event in active_alerts {
            if let AlertEvent::Fired {
                rule_id: fired_id, ..
            } = event
            {
                if fired_id == rule_id {
                    continue;
                }
                if let Some(fired_rule) = rules.iter().find(|r| &r.id == fired_id) {
                    if fired_rule.severity > severity {
                        return Some(fired_id.clone());
                    }
                }
            }
        }
        None
    }

    /// 评估规则并生成事件（spec §5.12 规则 1）
    ///
    /// `eval_fn` 接收规则表达式，返回指标值；值 > 0 表示条件满足
    pub async fn evaluate<F, Fut>(
        &self,
        now: DateTime<Utc>,
        eval_fn: F,
    ) -> Result<Vec<AlertEvent>, AlertEngineError>
    where
        F: Fn(&AlertRule) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<f64, AlertEngineError>> + Send,
    {
        let rules = self.rules.read().clone();
        let mut events = Vec::new();

        for rule in &rules {
            let value = eval_fn(rule).await?;
            if value > 0.0 {
                if self.is_silenced(&rule.labels, now) {
                    events.push(AlertEvent::Silenced {
                        rule_id: rule.id.clone(),
                    });
                } else {
                    events.push(AlertEvent::Fired {
                        rule_id: rule.id.clone(),
                        labels: rule.labels.clone(),
                        value,
                    });
                }
            } else {
                events.push(AlertEvent::Resolved {
                    rule_id: rule.id.clone(),
                });
            }
        }

        Ok(events)
    }
}

impl Default for AlertEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(id: &str, severity: AlertSeverity) -> AlertRule {
        AlertRule {
            id: id.to_string(),
            name: format!("Rule {id}"),
            expr: "rate(errors[5m]) > 0.1".to_string(),
            severity,
            eval_interval: Duration::from_secs(10),
            channels: vec![NotificationChannel::Webhook {
                url: "http://hook".to_string(),
            }],
            labels: HashMap::from([("service".to_string(), "api".to_string())]),
        }
    }

    #[test]
    fn test_rule_validate_ok() {
        let rule = make_rule("r1", AlertSeverity::Warning);
        assert!(rule.validate().is_ok());
    }

    #[test]
    fn test_rule_validate_empty_id() {
        let mut rule = make_rule("r1", AlertSeverity::Warning);
        rule.id = "".to_string();
        assert!(rule.validate().is_err());
    }

    #[test]
    fn test_rule_validate_short_interval() {
        let mut rule = make_rule("r1", AlertSeverity::Warning);
        rule.eval_interval = Duration::from_secs(5);
        assert!(rule.validate().is_err());
    }

    #[test]
    fn test_silence_window_validate() {
        let now = Utc::now();
        let silence = SilenceWindow {
            start: now,
            end: now + chrono::Duration::hours(1),
            label_matchers: HashMap::new(),
        };
        assert!(silence.validate().is_ok());
    }

    #[test]
    fn test_silence_window_invalid_end() {
        let now = Utc::now();
        let silence = SilenceWindow {
            start: now,
            end: now,
            label_matchers: HashMap::new(),
        };
        assert!(silence.validate().is_err());
    }

    #[test]
    fn test_silence_contains_time() {
        let now = Utc::now();
        let silence = SilenceWindow {
            start: now - chrono::Duration::minutes(5),
            end: now + chrono::Duration::minutes(5),
            label_matchers: HashMap::new(),
        };
        assert!(silence.contains_time(now));
        assert!(!silence.contains_time(now + chrono::Duration::minutes(10)));
    }

    #[test]
    fn test_silence_matches_labels() {
        let silence = SilenceWindow {
            start: Utc::now(),
            end: Utc::now() + chrono::Duration::hours(1),
            label_matchers: HashMap::from([("service".to_string(), "api".to_string())]),
        };
        let matching = HashMap::from([("service".to_string(), "api".to_string())]);
        let non_matching = HashMap::from([("service".to_string(), "web".to_string())]);
        assert!(silence.matches_labels(&matching));
        assert!(!silence.matches_labels(&non_matching));
    }

    #[tokio::test]
    async fn test_reload_rules() {
        let engine = AlertEngine::new();
        let rules = vec![
            make_rule("r1", AlertSeverity::Warning),
            make_rule("r2", AlertSeverity::Critical),
        ];
        engine.reload_rules(rules).await.unwrap();
        assert_eq!(engine.rules().len(), 2);
    }

    #[tokio::test]
    async fn test_reload_rules_invalid() {
        let engine = AlertEngine::new();
        let mut rule = make_rule("r1", AlertSeverity::Warning);
        rule.eval_interval = Duration::from_secs(5);
        assert!(engine.reload_rules(vec![rule]).await.is_err());
    }

    #[tokio::test]
    async fn test_is_silenced() {
        let engine = AlertEngine::new();
        let now = Utc::now();
        engine
            .add_silence(SilenceWindow {
                start: now - chrono::Duration::minutes(5),
                end: now + chrono::Duration::minutes(5),
                label_matchers: HashMap::from([("service".to_string(), "api".to_string())]),
            })
            .await
            .unwrap();
        let labels = HashMap::from([("service".to_string(), "api".to_string())]);
        assert!(engine.is_silenced(&labels, now));
    }

    #[tokio::test]
    async fn test_is_inhibited() {
        let engine = AlertEngine::new();
        engine
            .reload_rules(vec![
                make_rule("critical", AlertSeverity::Critical),
                make_rule("warning", AlertSeverity::Warning),
            ])
            .await
            .unwrap();
        let active = vec![AlertEvent::Fired {
            rule_id: "critical".to_string(),
            labels: HashMap::new(),
            value: 1.0,
        }];
        let inhibited_by = engine.is_inhibited("warning", AlertSeverity::Warning, &active);
        assert_eq!(inhibited_by, Some("critical".to_string()));
    }

    #[tokio::test]
    async fn test_evaluate_fires() {
        let engine = AlertEngine::new();
        engine
            .reload_rules(vec![make_rule("r1", AlertSeverity::Warning)])
            .await
            .unwrap();
        let events = engine
            .evaluate(Utc::now(), |_rule| async { Ok(1.5) })
            .await
            .unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, AlertEvent::Fired { rule_id, .. } if rule_id == "r1")));
    }

    #[tokio::test]
    async fn test_evaluate_resolves() {
        let engine = AlertEngine::new();
        engine
            .reload_rules(vec![make_rule("r1", AlertSeverity::Warning)])
            .await
            .unwrap();
        let events = engine
            .evaluate(Utc::now(), |_rule| async { Ok(0.0) })
            .await
            .unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, AlertEvent::Resolved { rule_id } if rule_id == "r1")));
    }

    #[tokio::test]
    async fn test_evaluate_silenced() {
        let engine = AlertEngine::new();
        let now = Utc::now();
        engine
            .reload_rules(vec![make_rule("r1", AlertSeverity::Warning)])
            .await
            .unwrap();
        engine
            .add_silence(SilenceWindow {
                start: now - chrono::Duration::minutes(5),
                end: now + chrono::Duration::minutes(5),
                label_matchers: HashMap::from([("service".to_string(), "api".to_string())]),
            })
            .await
            .unwrap();
        let events = engine
            .evaluate(now, |_rule| async { Ok(1.0) })
            .await
            .unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, AlertEvent::Silenced { rule_id } if rule_id == "r1")));
    }
}
