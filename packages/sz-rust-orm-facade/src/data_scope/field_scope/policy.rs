// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 字段权限策略配置

use super::visibility::FieldVisibility;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldScopePolicy {
    pub table_name: String,
    pub role_name: String,
    pub field_rules: Vec<(String, FieldVisibility)>,
    pub default_visibility: FieldVisibility,
    pub enabled: bool,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl FieldScopePolicy {
    pub fn new(table_name: impl Into<String>, role_name: impl Into<String>) -> Self {
        Self {
            table_name: table_name.into(),
            role_name: role_name.into(),
            field_rules: Vec::new(),
            default_visibility: FieldVisibility::Visible,
            enabled: true,
            updated_at: None,
        }
    }

    pub fn with_field_rule(
        mut self,
        field: impl Into<String>,
        visibility: FieldVisibility,
    ) -> Self {
        self.field_rules.push((field.into(), visibility));
        self
    }

    pub fn with_default_visibility(mut self, visibility: FieldVisibility) -> Self {
        self.default_visibility = visibility;
        self
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn field_visibility(&self, field_name: &str) -> FieldVisibility {
        for (field, vis) in &self.field_rules {
            if field == field_name {
                return *vis;
            }
        }
        self.default_visibility
    }

    pub fn policy_id(&self) -> String {
        format!("{}:{}", self.table_name, self.role_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_builder() {
        let policy = FieldScopePolicy::new("employee", "hr")
            .with_field_rule("salary", FieldVisibility::Hidden)
            .with_field_rule("name", FieldVisibility::ReadOnly)
            .with_default_visibility(FieldVisibility::Visible);
        assert_eq!(policy.field_visibility("salary"), FieldVisibility::Hidden);
        assert_eq!(policy.field_visibility("name"), FieldVisibility::ReadOnly);
        assert_eq!(policy.field_visibility("age"), FieldVisibility::Visible);
    }

    #[test]
    fn test_default_visibility() {
        let policy = FieldScopePolicy::new("order", "admin");
        assert_eq!(
            policy.field_visibility("any_field"),
            FieldVisibility::Visible
        );
    }

    #[test]
    fn test_policy_default_enabled() {
        let policy = FieldScopePolicy::new("employee", "hr");
        assert!(policy.enabled, "new policy should be enabled by default");
        assert!(policy.updated_at.is_none());
    }

    #[test]
    fn test_policy_id_format() {
        let policy = FieldScopePolicy::new("employee", "hr");
        assert_eq!(policy.policy_id(), "employee:hr");
    }

    #[test]
    fn test_policy_serialize_roundtrip() {
        let policy = FieldScopePolicy::new("employee", "hr")
            .with_field_rule("salary", FieldVisibility::Hidden);
        let json = serde_json::to_string(&policy).unwrap();
        let deserialized: FieldScopePolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.table_name, policy.table_name);
        assert_eq!(deserialized.role_name, policy.role_name);
        assert_eq!(deserialized.enabled, policy.enabled);
    }
}
