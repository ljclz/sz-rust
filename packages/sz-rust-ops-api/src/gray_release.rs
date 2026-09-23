// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 灰度发布管理（T055）
//!
//! 支持 IP / 租户 / 百分比三种灰度策略。百分比策略采用 FNV-1a 哈希 + 阈值比较
//! （与 P5 配置中心灰度一致）。

use std::collections::HashMap;
use std::sync::RwLock;

/// 灰度策略。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GrayStrategy {
    /// 按 IP 白名单灰度。
    ByIp {
        /// 命中灰度的 IP 列表。
        ips: Vec<String>,
    },
    /// 按租户白名单灰度。
    ByTenant {
        /// 命中灰度的租户 ID 列表。
        tenants: Vec<String>,
    },
    /// 按百分比灰度（0-100）。
    ByPercentage {
        /// 灰度百分比（0-100）。
        percent: u8,
    },
}

/// 灰度规则，封装一条策略。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GrayReleaseRule {
    /// 灰度策略。
    pub strategy: GrayStrategy,
}

/// 灰度上下文，描述当前请求的关键标识。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GrayContext {
    /// 客户端 IP。
    pub ip: Option<String>,
    /// 租户 ID。
    pub tenant_id: Option<String>,
    /// 用户 ID。
    pub user_id: Option<String>,
}

/// 灰度决策结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct GrayDecision {
    /// 命中的规则名称列表。
    pub matched_rules: Vec<String>,
    /// 是否应使用新版本（命中任一规则即为 true）。
    pub should_use_new: bool,
}

impl GrayStrategy {
    /// 判断当前上下文是否命中该策略。
    pub fn matches(&self, context: &GrayContext) -> bool {
        match self {
            GrayStrategy::ByIp { ips } => context
                .ip
                .as_ref()
                .is_some_and(|ip| ips.iter().any(|target| target == ip)),
            GrayStrategy::ByTenant { tenants } => context
                .tenant_id
                .as_ref()
                .is_some_and(|tid| tenants.iter().any(|t| t == tid)),
            GrayStrategy::ByPercentage { percent } => {
                let Some(key) = context.hash_key() else {
                    return false;
                };
                let hash = fnv1a_hash(&key);
                let threshold = (*percent as f64 / 100.0 * u64::MAX as f64) as u64;
                hash < threshold
            }
        }
    }
}

impl GrayReleaseRule {
    /// 判断当前上下文是否命中该规则。
    pub fn matches(&self, context: &GrayContext) -> bool {
        self.strategy.matches(context)
    }
}

impl GrayContext {
    /// 选取百分比灰度的哈希键（优先 user_id，其次 tenant_id，最后 ip）。
    fn hash_key(&self) -> Option<String> {
        self.user_id
            .clone()
            .or_else(|| self.tenant_id.clone())
            .or_else(|| self.ip.clone())
    }
}

/// 灰度发布管理器，维护命名规则集合并评估上下文。
#[derive(Debug, Default)]
pub struct GrayReleaseManager {
    rules: RwLock<HashMap<String, GrayReleaseRule>>,
}

impl GrayReleaseManager {
    /// 创建空的灰度发布管理器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加（或覆盖同名）灰度规则。
    pub fn add_rule(&self, name: String, rule: GrayReleaseRule) {
        self.rules.write().unwrap().insert(name, rule);
    }

    /// 评估上下文，返回命中的规则名称与是否使用新版本。
    pub fn evaluate(&self, context: &GrayContext) -> GrayDecision {
        let rules = self.rules.read().unwrap();
        let matched_rules: Vec<String> = rules
            .iter()
            .filter(|(_, rule)| rule.matches(context))
            .map(|(name, _)| name.clone())
            .collect();
        let should_use_new = !matched_rules.is_empty();
        GrayDecision {
            matched_rules,
            should_use_new,
        }
    }
}

/// FNV-1a 64 位哈希（与 P5 配置中心灰度一致）。
fn fnv1a_hash(s: &str) -> u64 {
    let mut h: u64 = 14695981039346656037;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(ip: &str) -> GrayContext {
        GrayContext {
            ip: Some(ip.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_by_ip_match() {
        let rule = GrayReleaseRule {
            strategy: GrayStrategy::ByIp {
                ips: vec!["10.0.0.1".to_string()],
            },
        };
        assert!(rule.matches(&ctx("10.0.0.1")));
    }

    #[test]
    fn test_by_ip_no_match() {
        let rule = GrayReleaseRule {
            strategy: GrayStrategy::ByIp {
                ips: vec!["10.0.0.1".to_string()],
            },
        };
        assert!(!rule.matches(&ctx("10.0.0.2")));
    }

    #[test]
    fn test_by_ip_none() {
        let rule = GrayReleaseRule {
            strategy: GrayStrategy::ByIp {
                ips: vec!["10.0.0.1".to_string()],
            },
        };
        assert!(!rule.matches(&GrayContext::default()));
    }

    #[test]
    fn test_by_tenant_match() {
        let rule = GrayReleaseRule {
            strategy: GrayStrategy::ByTenant {
                tenants: vec!["tenant_a".to_string()],
            },
        };
        let context = GrayContext {
            tenant_id: Some("tenant_a".to_string()),
            ..Default::default()
        };
        assert!(rule.matches(&context));
    }

    #[test]
    fn test_by_tenant_no_match() {
        let rule = GrayReleaseRule {
            strategy: GrayStrategy::ByTenant {
                tenants: vec!["tenant_a".to_string()],
            },
        };
        let context = GrayContext {
            tenant_id: Some("tenant_b".to_string()),
            ..Default::default()
        };
        assert!(!rule.matches(&context));
    }

    #[test]
    fn test_by_percentage_zero_never_matches() {
        let rule = GrayReleaseRule {
            strategy: GrayStrategy::ByPercentage { percent: 0 },
        };
        let context = GrayContext {
            user_id: Some("any-user".to_string()),
            ..Default::default()
        };
        assert!(!rule.matches(&context));
    }

    #[test]
    fn test_by_percentage_full_always_matches() {
        let rule = GrayReleaseRule {
            strategy: GrayStrategy::ByPercentage { percent: 100 },
        };
        let context = GrayContext {
            user_id: Some("any-user".to_string()),
            ..Default::default()
        };
        assert!(rule.matches(&context));
    }

    #[test]
    fn test_by_percentage_no_key_no_match() {
        let rule = GrayReleaseRule {
            strategy: GrayStrategy::ByPercentage { percent: 50 },
        };
        assert!(!rule.matches(&GrayContext::default()));
    }

    #[test]
    fn test_by_percentage_threshold_boundary() {
        let key = "deterministic-key";
        let hash = fnv1a_hash(key);
        let threshold_50 = (50.0_f64 / 100.0 * u64::MAX as f64) as u64;
        let rule = GrayReleaseRule {
            strategy: GrayStrategy::ByPercentage { percent: 50 },
        };
        let context = GrayContext {
            user_id: Some(key.to_string()),
            ..Default::default()
        };
        assert_eq!(rule.matches(&context), hash < threshold_50);
    }

    #[test]
    fn test_fnv1a_deterministic() {
        assert_eq!(fnv1a_hash("test"), fnv1a_hash("test"));
        assert_ne!(fnv1a_hash("test"), fnv1a_hash("other"));
    }

    #[test]
    fn test_manager_evaluate_empty() {
        let manager = GrayReleaseManager::new();
        let decision = manager.evaluate(&GrayContext::default());
        assert!(decision.matched_rules.is_empty());
        assert!(!decision.should_use_new);
    }

    #[test]
    fn test_manager_evaluate_matched() {
        let manager = GrayReleaseManager::new();
        manager.add_rule(
            "vip-users".to_string(),
            GrayReleaseRule {
                strategy: GrayStrategy::ByIp {
                    ips: vec!["10.0.0.1".to_string()],
                },
            },
        );
        manager.add_rule(
            "tenant-beta".to_string(),
            GrayReleaseRule {
                strategy: GrayStrategy::ByTenant {
                    tenants: vec!["beta".to_string()],
                },
            },
        );
        let decision = manager.evaluate(&ctx("10.0.0.1"));
        assert_eq!(decision.matched_rules, vec!["vip-users".to_string()]);
        assert!(decision.should_use_new);
    }

    #[test]
    fn test_manager_evaluate_multiple_match() {
        let manager = GrayReleaseManager::new();
        let context = GrayContext {
            ip: Some("10.0.0.1".to_string()),
            tenant_id: Some("beta".to_string()),
            ..Default::default()
        };
        manager.add_rule(
            "by-ip".to_string(),
            GrayReleaseRule {
                strategy: GrayStrategy::ByIp {
                    ips: vec!["10.0.0.1".to_string()],
                },
            },
        );
        manager.add_rule(
            "by-tenant".to_string(),
            GrayReleaseRule {
                strategy: GrayStrategy::ByTenant {
                    tenants: vec!["beta".to_string()],
                },
            },
        );
        let decision = manager.evaluate(&context);
        assert_eq!(decision.matched_rules.len(), 2);
        assert!(decision.should_use_new);
    }

    #[test]
    fn test_manager_add_rule_overwrites() {
        let manager = GrayReleaseManager::new();
        manager.add_rule(
            "rule".to_string(),
            GrayReleaseRule {
                strategy: GrayStrategy::ByPercentage { percent: 0 },
            },
        );
        manager.add_rule(
            "rule".to_string(),
            GrayReleaseRule {
                strategy: GrayStrategy::ByPercentage { percent: 100 },
            },
        );
        let context = GrayContext {
            user_id: Some("u".to_string()),
            ..Default::default()
        };
        let decision = manager.evaluate(&context);
        assert!(decision.should_use_new);
    }

    #[test]
    fn test_gray_context_hash_key_priority() {
        let context = GrayContext {
            ip: Some("ip".to_string()),
            tenant_id: Some("tenant".to_string()),
            user_id: Some("user".to_string()),
        };
        assert_eq!(context.hash_key(), Some("user".to_string()));

        let context = GrayContext {
            ip: Some("ip".to_string()),
            tenant_id: Some("tenant".to_string()),
            ..Default::default()
        };
        assert_eq!(context.hash_key(), Some("tenant".to_string()));

        let context = GrayContext {
            ip: Some("ip".to_string()),
            ..Default::default()
        };
        assert_eq!(context.hash_key(), Some("ip".to_string()));
    }
}
