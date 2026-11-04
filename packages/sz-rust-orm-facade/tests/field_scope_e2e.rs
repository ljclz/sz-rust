// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 字段级数据权限 E2E 测试

use std::sync::Arc;
use sz_rust_orm_facade::data_scope::context::DataScopeContext;
use sz_rust_orm_facade::data_scope::field_scope::evaluator::{
    DefaultFieldScopeEvaluator, FieldScopeEvaluator,
};
use sz_rust_orm_facade::data_scope::field_scope::filter::FieldFilter;
use sz_rust_orm_facade::data_scope::field_scope::policy::FieldScopePolicy;
use sz_rust_orm_facade::data_scope::field_scope::registry::FieldScopePolicyRegistry;
use sz_rust_orm_facade::data_scope::field_scope::result::FieldScopeResult;
use sz_rust_orm_facade::data_scope::field_scope::visibility::FieldVisibility;

fn make_registry_with_policies() -> Arc<FieldScopePolicyRegistry> {
    let registry = Arc::new(FieldScopePolicyRegistry::new());
    registry.register(
        FieldScopePolicy::new("employee", "hr")
            .with_field_rule("salary", FieldVisibility::Hidden)
            .with_field_rule("ssn", FieldVisibility::Hidden)
            .with_field_rule("name", FieldVisibility::ReadOnly),
    );
    registry.register(
        FieldScopePolicy::new("employee", "manager")
            .with_field_rule("salary", FieldVisibility::Visible)
            .with_field_rule("ssn", FieldVisibility::Hidden),
    );
    registry.register(
        FieldScopePolicy::new("employee", "auditor")
            .with_field_rule("salary", FieldVisibility::ReadOnly)
            .with_field_rule("ssn", FieldVisibility::ReadOnly),
    );
    registry
}

#[tokio::test]
async fn it_field_scope_super_admin_all_visible() {
    let evaluator = DefaultFieldScopeEvaluator::new(make_registry_with_policies());
    let ctx = DataScopeContext::new(1, 0, true);
    let result = evaluator.evaluate(&ctx, "employee", &[]).await.unwrap();
    assert!(result.is_all_visible);
    assert!(!result.is_hidden("salary"));
}

#[tokio::test]
async fn it_field_scope_single_role_hidden_fields() {
    let evaluator = DefaultFieldScopeEvaluator::new(make_registry_with_policies());
    let ctx = DataScopeContext::new(10, 5, false);
    let result = evaluator
        .evaluate(&ctx, "employee", &["hr".into()])
        .await
        .unwrap();
    assert!(result.is_hidden("salary"));
    assert!(result.is_hidden("ssn"));
    assert!(result.is_readonly("name"));
    assert!(!result.is_hidden("name"));
}

#[tokio::test]
async fn it_field_scope_multi_role_visible_wins() {
    let evaluator = DefaultFieldScopeEvaluator::new(make_registry_with_policies());
    let ctx = DataScopeContext::new(10, 5, false);
    let result = evaluator
        .evaluate(&ctx, "employee", &["hr".into(), "manager".into()])
        .await
        .unwrap();
    assert!(
        !result.is_hidden("salary"),
        "manager Visible should win over hr Hidden"
    );
    assert!(result.is_hidden("ssn"), "both roles hide ssn");
}

#[tokio::test]
async fn it_field_scope_readonly_wins_over_hidden() {
    let evaluator = DefaultFieldScopeEvaluator::new(make_registry_with_policies());
    let ctx = DataScopeContext::new(10, 5, false);
    let result = evaluator
        .evaluate(&ctx, "employee", &["hr".into(), "auditor".into()])
        .await
        .unwrap();
    assert!(
        !result.is_hidden("salary"),
        "auditor ReadOnly should win over hr Hidden"
    );
    assert!(result.is_readonly("salary"));
    assert!(
        !result.is_hidden("ssn"),
        "auditor ReadOnly should win over hr Hidden for ssn"
    );
    assert!(result.is_readonly("ssn"));
}

#[tokio::test]
async fn it_field_scope_no_policy_all_visible() {
    let evaluator = DefaultFieldScopeEvaluator::new(Arc::new(FieldScopePolicyRegistry::new()));
    let ctx = DataScopeContext::new(10, 5, false);
    let result = evaluator
        .evaluate(&ctx, "unknown_table", &["hr".into()])
        .await
        .unwrap();
    assert!(result.is_all_visible);
}

#[tokio::test]
async fn it_field_filter_removes_hidden_from_json() {
    let mut json = serde_json::json!({
        "name": "Alice",
        "salary": 50000,
        "ssn": "123-45-6789",
        "age": 30
    });
    let mut result = FieldScopeResult::default();
    result.hidden_fields.insert("salary".into());
    result.hidden_fields.insert("ssn".into());
    FieldFilter::filter_json(&mut json, &result).unwrap();
    assert!(json.get("salary").is_none());
    assert!(json.get("ssn").is_none());
    assert_eq!(json["name"], "Alice");
    assert_eq!(json["age"], 30);
}

#[tokio::test]
async fn it_field_filter_all_visible_no_change() {
    let mut json = serde_json::json!({"name": "Alice", "salary": 50000});
    let result = FieldScopeResult::all_visible();
    FieldFilter::filter_json(&mut json, &result).unwrap();
    assert!(json.get("salary").is_some());
}

#[tokio::test]
async fn it_field_scope_policy_default_visibility() {
    let policy = FieldScopePolicy::new("order", "guest")
        .with_default_visibility(FieldVisibility::Hidden)
        .with_field_rule("id", FieldVisibility::Visible);
    assert_eq!(policy.field_visibility("id"), FieldVisibility::Visible);
    assert_eq!(policy.field_visibility("secret"), FieldVisibility::Hidden);
}

#[tokio::test]
async fn it_field_scope_registry_invalidate() {
    let registry = make_registry_with_policies();
    assert!(registry.get_policy("employee", "hr").is_some());
    registry.invalidate("employee", "hr");
    assert!(registry.get_policy("employee", "hr").is_none());
    assert!(registry.get_policy("employee", "manager").is_some());
}
