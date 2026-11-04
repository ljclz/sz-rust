// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据范围规则注册表 — 按表名分组存储，优先级匹配

use crate::data_scope::rule::DataScopeRule;
use dashmap::DashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertAction {
    Created,
    Updated,
}

#[derive(Debug, Default, Clone)]
pub struct RuleListFilter {
    pub table: Option<String>,
}

pub struct DataScopeRuleRegistry {
    rules: DashMap<String, Vec<DataScopeRule>>,
}

impl DataScopeRuleRegistry {
    pub fn new() -> Self {
        Self {
            rules: DashMap::new(),
        }
    }

    pub fn register(&self, rule: DataScopeRule) {
        self.rules
            .entry(rule.target_table.clone())
            .or_default()
            .push(rule);
    }

    pub fn upsert_rule(&self, rule: DataScopeRule) -> UpsertAction {
        let mut entry = self.rules.entry(rule.target_table.clone()).or_default();
        if let Some(existing) = entry.iter_mut().find(|r| r.mode == rule.mode) {
            *existing = rule;
            UpsertAction::Updated
        } else {
            entry.push(rule);
            UpsertAction::Created
        }
    }

    pub fn get_rule(&self, table_name: &str) -> Option<DataScopeRule> {
        self.rules.get(table_name).and_then(|rules| {
            rules
                .iter()
                .filter(|r| r.enabled)
                .max_by_key(|r| r.priority)
                .cloned()
        })
    }

    pub fn get_rule_by_id(&self, rule_id: &str) -> Option<DataScopeRule> {
        for entry in self.rules.iter() {
            if let Some(rule) = entry.value().iter().find(|r| r.rule_id == rule_id) {
                return Some(rule.clone());
            }
        }
        None
    }

    pub fn delete_rule_by_id(&self, rule_id: &str) -> Option<DataScopeRule> {
        // 先在只读迭代中定位 rule_id 所属的 table 与位置，避免在 iter_mut 持有
        // shard 写锁时调用 self.rules.remove 导致死锁。
        let mut found_table: Option<String> = None;
        for entry in self.rules.iter() {
            if let Some(rule) = entry.value().iter().find(|r| r.rule_id == rule_id) {
                found_table = Some(rule.target_table.clone());
                break;
            }
        }
        let table = found_table?;
        // 对该 table 的 entry 做单键原子操作
        let mut entry = self.rules.entry(table.clone()).or_default();
        if let Some(pos) = entry.iter().position(|r| r.rule_id == rule_id) {
            let removed = entry.remove(pos);
            if entry.is_empty() {
                drop(entry);
                self.rules.remove(&table);
            }
            return Some(removed);
        }
        None
    }

    pub fn list_rules(&self, filter: &RuleListFilter) -> Vec<DataScopeRule> {
        let mut result = Vec::new();
        if let Some(table) = &filter.table {
            if let Some(rules) = self.rules.get(table) {
                result.extend(rules.iter().cloned());
            }
        } else {
            for entry in self.rules.iter() {
                result.extend(entry.value().iter().cloned());
            }
        }
        result
    }

    pub fn count_rules(&self) -> usize {
        self.rules.iter().map(|e| e.value().len()).sum()
    }

    pub fn invalidate(&self, table_name: &str) {
        self.rules.remove(table_name);
    }

    pub fn invalidate_all(&self) {
        self.rules.clear();
    }
}

impl Default for DataScopeRuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_scope::rule::DataScopeMode;

    #[test]
    fn test_register_and_get() {
        let registry = DataScopeRuleRegistry::new();
        registry.register(
            DataScopeRule::new("order", DataScopeMode::Dept)
                .with_dept_field("dept_id")
                .with_priority(10),
        );
        registry.register(
            DataScopeRule::new("order", DataScopeMode::Self_)
                .with_creator_field("creator_id")
                .with_priority(5),
        );
        let rule = registry.get_rule("order").unwrap();
        assert_eq!(rule.priority, 10, "should return highest priority rule");
        assert_eq!(rule.mode, DataScopeMode::Dept);
    }

    #[test]
    fn test_get_nonexistent_table() {
        let registry = DataScopeRuleRegistry::new();
        assert!(registry.get_rule("nonexistent").is_none());
    }

    #[test]
    fn test_invalidate() {
        let registry = DataScopeRuleRegistry::new();
        registry.register(DataScopeRule::new("order", DataScopeMode::All));
        assert!(registry.get_rule("order").is_some());
        registry.invalidate("order");
        assert!(registry.get_rule("order").is_none());
    }

    #[test]
    fn test_invalidate_all() {
        let registry = DataScopeRuleRegistry::new();
        registry.register(DataScopeRule::new("order", DataScopeMode::All));
        registry.register(DataScopeRule::new("user", DataScopeMode::Dept));
        registry.invalidate_all();
        assert!(registry.get_rule("order").is_none());
        assert!(registry.get_rule("user").is_none());
    }

    #[test]
    fn test_same_priority_returns_first() {
        let registry = DataScopeRuleRegistry::new();
        registry.register(
            DataScopeRule::new("order", DataScopeMode::Dept)
                .with_dept_field("dept_id")
                .with_priority(5),
        );
        registry.register(
            DataScopeRule::new("order", DataScopeMode::Self_)
                .with_creator_field("creator_id")
                .with_priority(5),
        );
        let rule = registry.get_rule("order").unwrap();
        assert_eq!(rule.priority, 5);
    }

    #[test]
    fn test_upsert_rule_replaces_existing() {
        let registry = DataScopeRuleRegistry::new();
        let r1 = DataScopeRule::new("order", DataScopeMode::Dept)
            .with_dept_field("dept_id")
            .with_priority(5);
        registry.register(r1.clone());
        let r2 = DataScopeRule::new("order", DataScopeMode::Dept)
            .with_dept_field("dept_id")
            .with_priority(10);
        let action = registry.upsert_rule(r2);
        assert_eq!(action, UpsertAction::Updated);
        assert_eq!(registry.count_rules(), 1, "upsert should replace not add");
        let got = registry.get_rule("order").unwrap();
        assert_eq!(got.priority, 10);
    }

    #[test]
    fn test_upsert_rule_creates_new() {
        let registry = DataScopeRuleRegistry::new();
        let rule = DataScopeRule::new("order", DataScopeMode::Dept).with_priority(5);
        let action = registry.upsert_rule(rule);
        assert_eq!(action, UpsertAction::Created);
    }

    #[test]
    fn test_delete_rule_by_id() {
        let registry = DataScopeRuleRegistry::new();
        let r1 = DataScopeRule::new("order", DataScopeMode::Dept).with_priority(5);
        let r2 = DataScopeRule::new("order", DataScopeMode::All).with_priority(1);
        registry.register(r1.clone());
        registry.register(r2.clone());
        let deleted = registry.delete_rule_by_id(&r1.rule_id).unwrap();
        assert_eq!(deleted.rule_id, r1.rule_id);
        assert_eq!(registry.count_rules(), 1, "only one rule should remain");
        assert!(registry.get_rule_by_id(&r1.rule_id).is_none());
        assert!(registry.get_rule_by_id(&r2.rule_id).is_some());
    }

    #[test]
    fn test_list_rules_filter_by_table() {
        let registry = DataScopeRuleRegistry::new();
        registry.register(DataScopeRule::new("order", DataScopeMode::Dept));
        registry.register(DataScopeRule::new("user", DataScopeMode::All));
        let order_rules = registry.list_rules(&RuleListFilter {
            table: Some("order".into()),
        });
        assert_eq!(order_rules.len(), 1);
        let all_rules = registry.list_rules(&RuleListFilter::default());
        assert_eq!(all_rules.len(), 2);
    }

    #[test]
    fn test_count_rules() {
        let registry = DataScopeRuleRegistry::new();
        registry.register(DataScopeRule::new("order", DataScopeMode::Dept));
        registry.register(DataScopeRule::new("order", DataScopeMode::All));
        registry.register(DataScopeRule::new("user", DataScopeMode::Self_));
        assert_eq!(registry.count_rules(), 3);
    }

    #[test]
    fn test_get_rule_skips_disabled() {
        let registry = DataScopeRuleRegistry::new();
        registry.register(
            DataScopeRule::new("order", DataScopeMode::Dept)
                .with_priority(10)
                .with_enabled(false),
        );
        registry.register(DataScopeRule::new("order", DataScopeMode::All).with_priority(5));
        let rule = registry.get_rule("order").unwrap();
        assert_eq!(
            rule.mode,
            DataScopeMode::All,
            "disabled rule should be skipped"
        );
    }
}
