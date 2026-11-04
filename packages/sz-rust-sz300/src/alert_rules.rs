// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 业务告警规则集与分级路由
//!
//! 对应 tasks.md §6.4：定义业务告警规则 + 分级路由 + 脱敏
//!
//! ## 告警分级
//!
//! | 级别 | AlertSeverity | 渠道 | 场景 |
//! |------|--------------|------|------|
//! | P0 | Critical | 全渠道+电话 | 订单失败率 > 10% |
//! | P1 | Warning | 即时渠道（钉钉/飞书） | 设备离线率 > 20% |
//! | P2 | Info | 邮件 | 库存低于阈值 |

use std::collections::HashMap;
use std::time::Duration;

use sz_rust_alert_engine::{AlertRule, AlertSeverity, NotificationChannel};
use sz_rust_data_mask::BuiltinMaskRule;

/// 业务告警规则集
pub struct BusinessAlertRules;

impl BusinessAlertRules {
    /// P0：订单失败率 > 10% 持续 5min
    ///
    /// 渠道：Webhook + 钉钉 + 飞书（全渠道）
    pub fn order_failure_rate() -> AlertRule {
        AlertRule {
            id: "order_failure_rate".to_string(),
            name: "订单失败率告警".to_string(),
            expr: "rate(order_failed_total[5m]) / rate(order_total[5m]) > 0.1".to_string(),
            severity: AlertSeverity::Critical,
            eval_interval: Duration::from_secs(30),
            channels: vec![
                NotificationChannel::Webhook {
                    url: "http://alert-gateway/api/order-failure".to_string(),
                },
                NotificationChannel::DingTalk {
                    webhook: "https://oapi.dingtalk.com/robot/send?access_token=order-alert"
                        .to_string(),
                },
                NotificationChannel::Feishu {
                    webhook: "https://open.feishu.cn/open-apis/bot/v2/hook/order-alert".to_string(),
                },
            ],
            labels: HashMap::from([
                ("service".to_string(), "sz300".to_string()),
                ("priority".to_string(), "P0".to_string()),
                ("team".to_string(), "order".to_string()),
            ]),
        }
    }

    /// P1：设备离线率 > 20% 持续 5min
    ///
    /// 渠道：钉钉 + 飞书（即时渠道）
    pub fn device_offline_rate() -> AlertRule {
        AlertRule {
            id: "device_offline_rate".to_string(),
            name: "设备离线率告警".to_string(),
            expr: "count(device_status{status=\"offline\"}) / count(device_status) > 0.2"
                .to_string(),
            severity: AlertSeverity::Warning,
            eval_interval: Duration::from_secs(60),
            channels: vec![
                NotificationChannel::DingTalk {
                    webhook: "https://oapi.dingtalk.com/robot/send?access_token=device-alert"
                        .to_string(),
                },
                NotificationChannel::Feishu {
                    webhook: "https://open.feishu.cn/open-apis/bot/v2/hook/device-alert"
                        .to_string(),
                },
            ],
            labels: HashMap::from([
                ("service".to_string(), "sz300".to_string()),
                ("priority".to_string(), "P1".to_string()),
                ("team".to_string(), "iot".to_string()),
            ]),
        }
    }

    /// P2：库存低于阈值
    ///
    /// 渠道：邮件（非紧急）
    pub fn stock_low() -> AlertRule {
        AlertRule {
            id: "stock_low".to_string(),
            name: "库存告急".to_string(),
            expr: "product_stock < 10".to_string(),
            severity: AlertSeverity::Info,
            eval_interval: Duration::from_secs(120),
            channels: vec![NotificationChannel::Email {
                to: vec!["inventory@sz300.com".to_string()],
            }],
            labels: HashMap::from([
                ("service".to_string(), "sz300".to_string()),
                ("priority".to_string(), "P2".to_string()),
                ("team".to_string(), "inventory".to_string()),
            ]),
        }
    }

    /// 获取所有业务告警规则
    pub fn all() -> Vec<AlertRule> {
        vec![
            Self::order_failure_rate(),
            Self::device_offline_rate(),
            Self::stock_low(),
        ]
    }
}

/// 告警内容脱敏（客户手机号等敏感信息）
///
/// 对齐 spec 5.7.1 规则 8：告警内容经 data-mask 脱敏
///
/// 扫描文本中的 11 位手机号（1[3-9]xxxxxxxxx）并脱敏
pub fn mask_alert_content(content: &str) -> String {
    let chars: Vec<char> = content.chars().collect();
    let mut result = String::with_capacity(content.len());
    let mut i = 0;
    while i < chars.len() {
        // 检测手机号模式：1[3-9] + 9位数字
        if i + 11 <= chars.len()
            && chars[i] == '1'
            && ('3'..='9').contains(&chars[i + 1])
            && chars[i..i + 11].iter().all(|c| c.is_ascii_digit())
        {
            let phone: String = chars[i..i + 11].iter().collect();
            result.push_str(&BuiltinMaskRule::Phone.apply(&phone));
            i += 11;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

/// 根据告警级别获取通知渠道
pub fn channels_for_severity(severity: AlertSeverity) -> Vec<&'static str> {
    match severity {
        AlertSeverity::Critical => vec!["webhook", "dingtalk", "feishu", "phone"],
        AlertSeverity::Warning => vec!["dingtalk", "feishu"],
        AlertSeverity::Info => vec!["email"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_failure_rate_rule() {
        let rule = BusinessAlertRules::order_failure_rate();
        assert_eq!(rule.id, "order_failure_rate");
        assert_eq!(rule.severity, AlertSeverity::Critical);
        assert!(rule.expr.contains("0.1"), "阈值应为 10%");
        assert!(rule.channels.len() >= 3, "P0 应全渠道通知");
        assert!(rule.validate().is_ok(), "规则应验证通过");
        assert_eq!(rule.labels.get("priority"), Some(&"P0".to_string()));
    }

    #[test]
    fn test_device_offline_rate_rule() {
        let rule = BusinessAlertRules::device_offline_rate();
        assert_eq!(rule.id, "device_offline_rate");
        assert_eq!(rule.severity, AlertSeverity::Warning);
        assert!(rule.expr.contains("0.2"), "阈值应为 20%");
        assert!(rule.validate().is_ok(), "规则应验证通过");
        assert_eq!(rule.labels.get("priority"), Some(&"P1".to_string()));
    }

    #[test]
    fn test_stock_low_rule() {
        let rule = BusinessAlertRules::stock_low();
        assert_eq!(rule.id, "stock_low");
        assert_eq!(rule.severity, AlertSeverity::Info);
        assert!(rule.validate().is_ok(), "规则应验证通过");
        assert_eq!(rule.labels.get("priority"), Some(&"P2".to_string()));
    }

    #[test]
    fn test_all_rules() {
        let rules = BusinessAlertRules::all();
        assert_eq!(rules.len(), 3);
        for rule in &rules {
            assert!(rule.validate().is_ok(), "规则 {} 应验证通过", rule.id);
        }
    }

    #[test]
    fn test_mask_alert_content_phone() {
        let content = "客户手机号 13812345678 订单失败";
        let masked = mask_alert_content(content);
        assert!(masked.contains("138****5678"), "手机号应脱敏: {}", masked);
        assert!(!masked.contains("1234"), "中间 4 位不应出现");
    }

    #[test]
    fn test_channels_for_severity() {
        let p0 = channels_for_severity(AlertSeverity::Critical);
        assert!(p0.contains(&"phone"), "P0 应包含电话渠道");
        assert!(p0.contains(&"webhook"), "P0 应包含 webhook");

        let p1 = channels_for_severity(AlertSeverity::Warning);
        assert!(p1.contains(&"dingtalk"), "P1 应包含钉钉");
        assert!(!p1.contains(&"phone"), "P1 不应包含电话");

        let p2 = channels_for_severity(AlertSeverity::Info);
        assert!(p2.contains(&"email"), "P2 应包含邮件");
        assert!(!p2.contains(&"dingtalk"), "P2 不应包含钉钉");
    }
}
