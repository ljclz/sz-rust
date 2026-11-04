// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 行级数据权限扩展 E2E — DeptAndSub降级 + RuleRegistry优先级匹配

use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use sz_rust_orm_facade::data_scope::cache::{DeptTreeCache, DeptTreeProvider};
use sz_rust_orm_facade::data_scope::context::DataScopeContext;
use sz_rust_orm_facade::data_scope::custom::CustomGeneratorRegistry;
use sz_rust_orm_facade::data_scope::error::DataScopeError;
use sz_rust_orm_facade::data_scope::evaluator::{DataScopeEvaluator, DefaultDataScopeEvaluator};
use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::data_scope::registry::DataScopeRuleRegistry;
use sz_rust_orm_facade::data_scope::rule::{DataScopeMode, DataScopeRule};
use sz_rust_orm_facade::repository::WhereOp;
use sz_rust_orm_facade::Value;

struct UnavailableDeptProvider;

#[async_trait]
impl DeptTreeProvider for UnavailableDeptProvider {
    async fn sub_depts(&self, _dept_id: i64) -> Result<Vec<i64>, DataScopeError> {
        Err(DataScopeError::DeptTreeUnavailable(
            "provider offline".into(),
        ))
    }
}

fn make_evaluator_with_unavailable_cache() -> DefaultDataScopeEvaluator {
    let cache = Arc::new(DeptTreeCache::new(
        Arc::new(UnavailableDeptProvider),
        Duration::from_secs(300),
    ));
    DefaultDataScopeEvaluator::new(
        cache,
        Arc::new(CustomGeneratorRegistry::new()),
        Arc::new(DataScopeMetrics::new()),
    )
}

#[tokio::test]
async fn it_degrade_to_dept_when_provider_unavailable() {
    let evaluator = make_evaluator_with_unavailable_cache();
    let ctx = DataScopeContext::new(10, 5, false);
    let rule = DataScopeRule::new("order", DataScopeMode::DeptAndSub).with_dept_field("dept_id");
    let conditions = evaluator.evaluate(&ctx, &rule).await.unwrap();
    assert_eq!(conditions.len(), 1);
    assert_eq!(conditions[0].op, WhereOp::Eq);
    match &conditions[0].value {
        Value::I64(v) => assert_eq!(*v, 5),
        other => panic!("expected I64, got {other:?}"),
    }
}

#[tokio::test]
async fn it_rule_registry_returns_highest_priority() {
    let registry = DataScopeRuleRegistry::new();
    registry.register(
        DataScopeRule::new("order", DataScopeMode::Self_)
            .with_creator_field("creator_id")
            .with_priority(5),
    );
    registry.register(
        DataScopeRule::new("order", DataScopeMode::Dept)
            .with_dept_field("dept_id")
            .with_priority(10),
    );
    registry.register(DataScopeRule::new("order", DataScopeMode::All).with_priority(1));
    let rule = registry.get_rule("order").unwrap();
    assert_eq!(rule.priority, 10);
    assert_eq!(rule.mode, DataScopeMode::Dept);
}

#[tokio::test]
async fn it_rule_registry_no_rule_for_table() {
    let registry = DataScopeRuleRegistry::new();
    assert!(registry.get_rule("nonexistent").is_none());
}

#[tokio::test]
async fn it_rule_registry_invalidate_and_requery() {
    let registry = DataScopeRuleRegistry::new();
    registry.register(
        DataScopeRule::new("order", DataScopeMode::Dept)
            .with_dept_field("dept_id")
            .with_priority(10),
    );
    assert!(registry.get_rule("order").is_some());
    registry.invalidate("order");
    assert!(registry.get_rule("order").is_none());
}

#[tokio::test]
async fn it_metrics_field_filter_recorded() {
    let metrics = DataScopeMetrics::new();
    metrics.record_field_filter("employee", 2);
    metrics.record_field_filter("employee", 0);
    assert_eq!(metrics.field_filter_total(), 2);
    assert_eq!(metrics.field_hidden_total(), 1);
    assert_eq!(metrics.field_all_visible_total(), 1);
}
