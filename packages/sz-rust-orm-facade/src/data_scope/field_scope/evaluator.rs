// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 字段级评估器

use super::error::FieldScopeError;

use super::registry::FieldScopePolicyRegistry;
use super::result::FieldScopeResult;
use super::visibility::FieldVisibility;
use crate::data_scope::context::DataScopeContext;
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;

#[async_trait]
pub trait FieldScopeEvaluator: Send + Sync {
    async fn evaluate(
        &self,
        ctx: &DataScopeContext,
        table_name: &str,
        roles: &[String],
    ) -> Result<FieldScopeResult, FieldScopeError>;
}

pub struct DefaultFieldScopeEvaluator {
    registry: Arc<FieldScopePolicyRegistry>,
}

impl DefaultFieldScopeEvaluator {
    pub fn new(registry: Arc<FieldScopePolicyRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl FieldScopeEvaluator for DefaultFieldScopeEvaluator {
    async fn evaluate(
        &self,
        ctx: &DataScopeContext,
        table_name: &str,
        roles: &[String],
    ) -> Result<FieldScopeResult, FieldScopeError> {
        if ctx.is_super {
            return Ok(FieldScopeResult::all_visible());
        }

        let policies = self.registry.get_policies_for_roles(table_name, roles);
        if policies.is_empty() {
            return Ok(FieldScopeResult::all_visible());
        }

        let mut hidden_fields: HashSet<String> = HashSet::new();
        let mut readonly_fields: HashSet<String> = HashSet::new();
        let mut all_field_names: HashSet<String> = HashSet::new();

        for policy in &policies {
            for (field, _vis) in &policy.field_rules {
                all_field_names.insert(field.clone());
            }
        }

        for field in &all_field_names {
            let mut best = FieldVisibility::Hidden;
            for policy in &policies {
                let vis = policy.field_visibility(field);
                if vis == FieldVisibility::Visible {
                    best = FieldVisibility::Visible;
                    break;
                } else if vis == FieldVisibility::ReadOnly && best == FieldVisibility::Hidden {
                    best = FieldVisibility::ReadOnly;
                }
            }
            match best {
                FieldVisibility::Hidden => {
                    hidden_fields.insert(field.clone());
                }
                FieldVisibility::ReadOnly => {
                    readonly_fields.insert(field.clone());
                }
                FieldVisibility::Visible => {}
            }
        }

        Ok(FieldScopeResult {
            hidden_fields,
            readonly_fields,
            is_all_visible: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::policy::FieldScopePolicy;
    use super::*;

    fn make_registry() -> Arc<FieldScopePolicyRegistry> {
        Arc::new(FieldScopePolicyRegistry::new())
    }

    #[tokio::test]
    async fn test_super_admin_all_visible() {
        let evaluator = DefaultFieldScopeEvaluator::new(make_registry());
        let ctx = DataScopeContext::new(1, 0, true);
        let result = evaluator.evaluate(&ctx, "employee", &[]).await.unwrap();
        assert!(result.is_all_visible);
    }

    #[tokio::test]
    async fn test_no_policy_all_visible() {
        let evaluator = DefaultFieldScopeEvaluator::new(make_registry());
        let ctx = DataScopeContext::new(10, 5, false);
        let result = evaluator
            .evaluate(&ctx, "employee", &["hr".into()])
            .await
            .unwrap();
        assert!(result.is_all_visible);
    }

    #[tokio::test]
    async fn test_single_role_hidden() {
        let registry = make_registry();
        registry.register(
            FieldScopePolicy::new("employee", "hr")
                .with_field_rule("salary", FieldVisibility::Hidden),
        );
        let evaluator = DefaultFieldScopeEvaluator::new(registry);
        let ctx = DataScopeContext::new(10, 5, false);
        let result = evaluator
            .evaluate(&ctx, "employee", &["hr".into()])
            .await
            .unwrap();
        assert!(result.is_hidden("salary"));
    }

    #[tokio::test]
    async fn test_multi_role_union_visible_wins() {
        let registry = make_registry();
        registry.register(
            FieldScopePolicy::new("employee", "hr")
                .with_field_rule("salary", FieldVisibility::Hidden),
        );
        registry.register(
            FieldScopePolicy::new("employee", "manager")
                .with_field_rule("salary", FieldVisibility::Visible),
        );
        let evaluator = DefaultFieldScopeEvaluator::new(registry);
        let ctx = DataScopeContext::new(10, 5, false);
        let result = evaluator
            .evaluate(&ctx, "employee", &["hr".into(), "manager".into()])
            .await
            .unwrap();
        assert!(
            !result.is_hidden("salary"),
            "Visible should win over Hidden"
        );
    }

    #[tokio::test]
    async fn test_multi_role_readonly_wins_over_hidden() {
        let registry = make_registry();
        registry.register(
            FieldScopePolicy::new("employee", "hr")
                .with_field_rule("salary", FieldVisibility::Hidden),
        );
        registry.register(
            FieldScopePolicy::new("employee", "auditor")
                .with_field_rule("salary", FieldVisibility::ReadOnly),
        );
        let evaluator = DefaultFieldScopeEvaluator::new(registry);
        let ctx = DataScopeContext::new(10, 5, false);
        let result = evaluator
            .evaluate(&ctx, "employee", &["hr".into(), "auditor".into()])
            .await
            .unwrap();
        assert!(
            !result.is_hidden("salary"),
            "ReadOnly should win over Hidden"
        );
        assert!(result.is_readonly("salary"));
    }
}
