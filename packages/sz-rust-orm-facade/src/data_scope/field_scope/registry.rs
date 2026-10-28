// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 字段权限策略注册表 — 按表+角色维度缓存

use super::policy::FieldScopePolicy;
use dashmap::DashMap;

/// 策略列表过滤条件
#[derive(Debug, Default, Clone)]
pub struct PolicyListFilter {
    pub table: Option<String>,
    pub role: Option<String>,
}

pub struct FieldScopePolicyRegistry {
    policies: DashMap<(String, String), FieldScopePolicy>,
}

impl FieldScopePolicyRegistry {
    pub fn new() -> Self {
        Self {
            policies: DashMap::new(),
        }
    }

    pub fn register(&self, policy: FieldScopePolicy) {
        let key = (policy.table_name.clone(), policy.role_name.clone());
        self.policies.insert(key, policy);
    }

    pub fn get_policy(&self, table_name: &str, role_name: &str) -> Option<FieldScopePolicy> {
        self.policies
            .get(&(table_name.to_string(), role_name.to_string()))
            .map(|p| p.clone())
    }

    pub fn get_policies_for_roles(
        &self,
        table_name: &str,
        roles: &[String],
    ) -> Vec<FieldScopePolicy> {
        roles
            .iter()
            .filter_map(|role| self.get_policy(table_name, role))
            .collect()
    }

    pub fn invalidate(&self, table_name: &str, role_name: &str) {
        self.policies
            .remove(&(table_name.to_string(), role_name.to_string()));
    }

    pub fn invalidate_all(&self) {
        self.policies.clear();
    }

    /// 按过滤条件列出策略
    pub fn list_policies(&self, filter: &PolicyListFilter) -> Vec<FieldScopePolicy> {
        self.policies
            .iter()
            .filter(|entry| {
                let (table, role) = entry.key();
                filter.table.as_ref().map_or(true, |t| t == table)
                    && filter.role.as_ref().map_or(true, |r| r == role)
            })
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// 策略总数
    pub fn count_policies(&self) -> usize {
        self.policies.len()
    }
}

impl Default for FieldScopePolicyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::visibility::FieldVisibility;
    use super::*;

    #[test]
    fn test_register_and_get() {
        let registry = FieldScopePolicyRegistry::new();
        let policy = FieldScopePolicy::new("employee", "hr")
            .with_field_rule("salary", FieldVisibility::Hidden);
        registry.register(policy);
        let got = registry.get_policy("employee", "hr").unwrap();
        assert_eq!(got.field_visibility("salary"), FieldVisibility::Hidden);
    }

    #[test]
    fn test_get_policies_for_roles() {
        let registry = FieldScopePolicyRegistry::new();
        registry.register(
            FieldScopePolicy::new("employee", "hr")
                .with_field_rule("salary", FieldVisibility::Hidden),
        );
        registry.register(
            FieldScopePolicy::new("employee", "manager")
                .with_field_rule("salary", FieldVisibility::Visible),
        );
        let policies =
            registry.get_policies_for_roles("employee", &["hr".into(), "manager".into()]);
        assert_eq!(policies.len(), 2);
    }

    #[test]
    fn test_invalidate() {
        let registry = FieldScopePolicyRegistry::new();
        registry.register(FieldScopePolicy::new("employee", "hr"));
        registry.invalidate("employee", "hr");
        assert!(registry.get_policy("employee", "hr").is_none());
    }

    #[test]
    fn test_invalidate_all() {
        let registry = FieldScopePolicyRegistry::new();
        registry.register(FieldScopePolicy::new("employee", "hr"));
        registry.register(FieldScopePolicy::new("order", "admin"));
        registry.invalidate_all();
        assert!(registry.get_policy("employee", "hr").is_none());
        assert!(registry.get_policy("order", "admin").is_none());
    }

    #[test]
    fn test_list_policies_filter() {
        let registry = FieldScopePolicyRegistry::new();
        registry.register(FieldScopePolicy::new("employee", "hr"));
        registry.register(FieldScopePolicy::new("employee", "manager"));
        registry.register(FieldScopePolicy::new("order", "admin"));

        // 无过滤：返回全部
        let all = registry.list_policies(&PolicyListFilter::default());
        assert_eq!(all.len(), 3);

        // 按 table 过滤
        let emp = registry.list_policies(&PolicyListFilter {
            table: Some("employee".into()),
            role: None,
        });
        assert_eq!(emp.len(), 2);
        for p in &emp {
            assert_eq!(p.table_name, "employee");
        }

        // 按 table + role 精确过滤
        let emp_hr = registry.list_policies(&PolicyListFilter {
            table: Some("employee".into()),
            role: Some("hr".into()),
        });
        assert_eq!(emp_hr.len(), 1);
        assert_eq!(emp_hr[0].role_name, "hr");

        // 不匹配的过滤
        let none = registry.list_policies(&PolicyListFilter {
            table: Some("nonexistent".into()),
            role: None,
        });
        assert!(none.is_empty());
    }

    #[test]
    fn test_count_policies() {
        let registry = FieldScopePolicyRegistry::new();
        assert_eq!(registry.count_policies(), 0);
        registry.register(FieldScopePolicy::new("employee", "hr"));
        registry.register(FieldScopePolicy::new("order", "admin"));
        assert_eq!(registry.count_policies(), 2);
        registry.invalidate("employee", "hr");
        assert_eq!(registry.count_policies(), 1);
    }
}
