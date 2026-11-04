// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! WAF 规则存储 + 热加载（spec 5.11.4）

#![forbid(unsafe_code)]

use std::sync::Arc;

use parking_lot::RwLock;

use super::WafRule;

/// WAF 规则存储
///
/// 使用 RwLock 实现热加载（spec 5.11.4）。
pub struct WafRuleStore {
    rules: RwLock<Arc<Vec<WafRule>>>,
    version: RwLock<u64>,
}

impl WafRuleStore {
    /// 创建规则存储
    pub fn new(mut rules: Vec<WafRule>) -> Self {
        // 按优先级排序（数值越小越高）
        rules.sort_by_key(|r| r.priority);
        Self {
            rules: RwLock::new(Arc::new(rules)),
            version: RwLock::new(1),
        }
    }

    /// 获取当前规则集（按优先级排序）
    pub fn get_rules(&self) -> Arc<Vec<WafRule>> {
        self.rules.read().clone()
    }

    /// 热加载新规则集（spec 5.11.4）
    pub fn reload(&self, mut rules: Vec<WafRule>) {
        rules.sort_by_key(|r| r.priority);
        let mut version = self.version.write();
        *version += 1;
        tracing::info!(version = *version, count = rules.len(), "WAF 规则集热加载");
        *self.rules.write() = Arc::new(rules);
    }

    /// 获取当前版本
    pub fn version(&self) -> u64 {
        *self.version.read()
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use regex::Regex;

    fn make_rule(id: &str, priority: i32) -> WafRule {
        WafRule {
            id: id.to_string(),
            priority,
            pattern: Regex::new("test").unwrap(),
            risk: RiskLevel::Low,
            description: "test".to_string(),
        }
    }

    #[test]
    fn test_store_sorted_by_priority() {
        let store = WafRuleStore::new(vec![
            make_rule("low", 10),
            make_rule("high", 1),
            make_rule("mid", 5),
        ]);

        let rules = store.get_rules();
        assert_eq!(rules[0].id, "high");
        assert_eq!(rules[1].id, "mid");
        assert_eq!(rules[2].id, "low");
    }

    #[test]
    fn test_store_reload() {
        let store = WafRuleStore::new(vec![make_rule("r1", 1)]);
        assert_eq!(store.get_rules().len(), 1);
        assert_eq!(store.version(), 1);

        store.reload(vec![make_rule("r2", 1), make_rule("r3", 2)]);
        assert_eq!(store.get_rules().len(), 2);
        assert_eq!(store.version(), 2);
    }
}
