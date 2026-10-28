// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据权限扩展 E2E 集成测试
//!
//! 覆盖验收标准：
//! - AC-01：创建规则 + 世代号自增
//! - AC-03：upsert 覆盖
//! - AC-04：单条删除
//! - AC-07/AC-08：YAML/JSON 加载与等价
//! - AC-09：部分失败继续
//! - AC-10：加载失败保持现有
//! - AC-11：路径穿越拒绝
//! - AC-13：热重载生效
//! - AC-14：变更通知广播
//! - AC-15：世代号单调递增
//!
//! 所有临时文件在测试结束后删除。

use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use sz_rust_orm_facade::data_scope::cache::{DeptTreeCache, DeptTreeProvider};
use sz_rust_orm_facade::data_scope::context::DataScopeContext;
use sz_rust_orm_facade::data_scope::custom::CustomGeneratorRegistry;
use sz_rust_orm_facade::data_scope::error::DataScopeError;
use sz_rust_orm_facade::data_scope::evaluator::{DataScopeEvaluator, DefaultDataScopeEvaluator};
use sz_rust_orm_facade::data_scope::ext::audit::{AuditLogger, TracingAuditLogger};
use sz_rust_orm_facade::data_scope::ext::config_loader::ConfigLoader;
use sz_rust_orm_facade::data_scope::ext::generation::PolicyGeneration;
use sz_rust_orm_facade::data_scope::ext::hot_reload::HotReloadManager;
use sz_rust_orm_facade::data_scope::ext::notifier::{ChangeEvent, ChangeNotifier};
use sz_rust_orm_facade::data_scope::ext::path_guard::PathGuard;
use sz_rust_orm_facade::data_scope::field_scope::policy::FieldScopePolicy;
use sz_rust_orm_facade::data_scope::field_scope::registry::FieldScopePolicyRegistry;
use sz_rust_orm_facade::data_scope::field_scope::visibility::FieldVisibility;
use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::data_scope::registry::DataScopeRuleRegistry;
use sz_rust_orm_facade::data_scope::rule::{DataScopeMode, DataScopeRule};
use sz_rust_orm_facade::repository::WhereOp;
use sz_rust_orm_facade::Value;

// ============================================================================
// 辅助构造
// ============================================================================

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

fn make_manager() -> Arc<HotReloadManager> {
    let rule_registry = Arc::new(DataScopeRuleRegistry::new());
    let policy_registry = Arc::new(FieldScopePolicyRegistry::new());
    let custom_registry = Arc::new(CustomGeneratorRegistry::new());
    let generation = PolicyGeneration::new();
    let notifier = ChangeNotifier::new(64);
    let metrics = Arc::new(DataScopeMetrics::new());
    let audit: Arc<dyn AuditLogger> = Arc::new(TracingAuditLogger);
    Arc::new(HotReloadManager::new(
        rule_registry,
        policy_registry,
        custom_registry,
        generation,
        notifier,
        metrics,
        audit,
    ))
}

fn make_loader(manager: Arc<HotReloadManager>) -> ConfigLoader {
    let cwd = std::env::current_dir().unwrap();
    let temp = std::env::temp_dir();
    let guard = PathGuard::new(vec![cwd, temp]);
    ConfigLoader::new(manager, guard, ConfigLoader::DEFAULT_MAX_FILE_SIZE)
}

async fn write_temp_file(name: &str, content: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("sz_rust_data_perm_ext_e2e");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let path = dir.join(name);
    tokio::fs::write(&path, content).await.unwrap();
    path
}

async fn cleanup(path: &std::path::Path) {
    let _ = tokio::fs::remove_file(path).await;
}

// ============================================================================
// 原有 E2E 测试（DeptAndSub 降级 + RuleRegistry 优先级匹配）
// ============================================================================

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

// ============================================================================
// AC-01：创建规则 + 世代号自增
// ============================================================================

#[tokio::test]
async fn ac01_create_rule_generation_increment() {
    let mgr = make_manager();
    let g0 = mgr.generation();
    assert_eq!(g0, 0);

    let rule = DataScopeRule::new("order", DataScopeMode::All).with_rule_id("ac01_r1");
    let result = mgr.create_rule(rule).await.unwrap();
    assert_eq!(result.status, "created");
    assert_eq!(result.generation, 1);
    assert_eq!(mgr.generation(), 1);
    assert_eq!(mgr.rule_count(), 1);

    // 第二条规则世代号继续自增
    let rule2 = DataScopeRule::new("user", DataScopeMode::All).with_rule_id("ac01_r2");
    let result2 = mgr.create_rule(rule2).await.unwrap();
    assert_eq!(result2.generation, 2);
    assert_eq!(mgr.generation(), 2);
}

// ============================================================================
// AC-03：upsert 覆盖
// ============================================================================

#[tokio::test]
async fn ac03_upsert_overrides_existing() {
    let mgr = make_manager();
    let r1 = DataScopeRule::new("order", DataScopeMode::Dept)
        .with_dept_field("dept_id")
        .with_priority(5)
        .with_rule_id("ac03_r1");
    let created = mgr.create_rule(r1).await.unwrap();
    assert_eq!(created.status, "created");

    // 同 table + mode 不同 priority → upsert 覆盖
    let r2 = DataScopeRule::new("order", DataScopeMode::Dept)
        .with_dept_field("dept_id")
        .with_priority(99)
        .with_rule_id("ac03_r1");
    let updated = mgr.create_rule(r2).await.unwrap();
    assert_eq!(updated.status, "updated");
    assert_eq!(mgr.rule_count(), 1, "upsert 不应增加规则数");

    let got = mgr.rule_registry().get_rule_by_id("ac03_r1").unwrap();
    assert_eq!(got.priority, 99);
}

// ============================================================================
// AC-04：单条删除
// ============================================================================

#[tokio::test]
async fn ac04_delete_single_rule() {
    let mgr = make_manager();
    let rule = DataScopeRule::new("order", DataScopeMode::All).with_rule_id("ac04_r1");
    let created = mgr.create_rule(rule).await.unwrap();
    assert_eq!(mgr.rule_count(), 1);

    let deleted = mgr.delete_rule(&created.rule_id).await.unwrap();
    assert_eq!(deleted.status, "deleted");
    assert_eq!(mgr.rule_count(), 0);
    assert!(mgr.rule_registry().get_rule_by_id("ac04_r1").is_none());
}

// ============================================================================
// AC-07：YAML 加载
// ============================================================================

#[tokio::test]
async fn ac07_load_yaml_config() {
    let mgr = make_manager();
    let loader = make_loader(mgr.clone());
    let yaml = r#"
format_version: "1.0"
rules:
  - mode: all
    target_table: order
    priority: 5
    rule_id: ac07_r1
    enabled: true
  - mode: dept
    target_table: user
    priority: 3
    rule_id: ac07_r2
    enabled: true
    dept_field: dept_id
policies:
  - table_name: employee
    role_name: hr
    field_rules:
      - [salary, hidden]
    default_visibility: visible
    enabled: true
"#;
    let path = write_temp_file("ac07.yaml", yaml).await;
    let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
    assert_eq!(report.success_count, 3);
    assert_eq!(report.failed_count, 0);
    assert_eq!(mgr.rule_count(), 2);
    assert_eq!(mgr.policy_count(), 1);

    let policy = mgr.policy_registry().get_policy("employee", "hr").unwrap();
    assert_eq!(policy.field_visibility("salary"), FieldVisibility::Hidden);
    cleanup(&path).await;
}

// ============================================================================
// AC-08：JSON 加载与 YAML 等价
// ============================================================================

#[tokio::test]
async fn ac08_load_json_equivalent_to_yaml() {
    // YAML 加载
    let mgr_yaml = make_manager();
    let loader_yaml = make_loader(mgr_yaml.clone());
    let yaml = r#"
format_version: "1.0"
rules:
  - mode: all
    target_table: order
    priority: 5
    rule_id: eq_r1
    enabled: true
policies:
  - table_name: employee
    role_name: hr
    field_rules: []
    default_visibility: visible
    enabled: true
"#;
    let path_yaml = write_temp_file("ac08.yaml", yaml).await;
    let report_yaml = loader_yaml
        .load_from_file(path_yaml.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(report_yaml.success_count, 2);

    // JSON 加载（等价内容）
    let mgr_json = make_manager();
    let loader_json = make_loader(mgr_json.clone());
    let json = r#"{
  "format_version": "1.0",
  "rules": [
    {"mode": "all", "target_table": "order", "priority": 5, "rule_id": "eq_r1", "enabled": true}
  ],
  "policies": [
    {"table_name": "employee", "role_name": "hr", "field_rules": [], "default_visibility": "visible", "enabled": true}
  ]
}"#;
    let path_json = write_temp_file("ac08.json", json).await;
    let report_json = loader_json
        .load_from_file(path_json.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(report_json.success_count, 2);

    // 两者结果等价
    assert_eq!(mgr_yaml.rule_count(), mgr_json.rule_count());
    assert_eq!(mgr_yaml.policy_count(), mgr_json.policy_count());
    assert_eq!(
        mgr_yaml
            .rule_registry()
            .get_rule_by_id("eq_r1")
            .unwrap()
            .priority,
        mgr_json
            .rule_registry()
            .get_rule_by_id("eq_r1")
            .unwrap()
            .priority
    );

    cleanup(&path_yaml).await;
    cleanup(&path_json).await;
}

// ============================================================================
// AC-09：部分失败继续
// ============================================================================

#[tokio::test]
async fn ac09_partial_failure_continues() {
    let mgr = make_manager();
    let loader = make_loader(mgr.clone());
    // 第二条规则 mode=dept 缺 dept_field → 失败；第一条应成功
    let yaml = r#"
format_version: "1.0"
rules:
  - mode: all
    target_table: ok_table
    priority: 1
    rule_id: ac09_ok
    enabled: true
  - mode: dept
    target_table: bad_table
    priority: 1
    rule_id: ac09_bad
    enabled: true
policies: []
"#;
    let path = write_temp_file("ac09.yaml", yaml).await;
    let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
    assert_eq!(report.success_count, 1);
    assert_eq!(report.failed_count, 1);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].code, "RULE_FIELD_MISSING");
    // 成功的规则仍被加载
    assert!(mgr.rule_registry().get_rule_by_id("ac09_ok").is_some());
    cleanup(&path).await;
}

// ============================================================================
// AC-10：加载失败保持现有
// ============================================================================

#[tokio::test]
async fn ac10_load_failure_keeps_existing() {
    let mgr = make_manager();
    // 预置一条已有规则
    mgr.create_rule(
        DataScopeRule::new("existing", DataScopeMode::All).with_rule_id("ac10_existing"),
    )
    .await
    .unwrap();
    assert_eq!(mgr.rule_count(), 1);

    let loader = make_loader(mgr.clone());
    // 加载含错误项的文件，已有规则应保留
    let yaml = r#"
format_version: "1.0"
rules:
  - mode: dept
    target_table: bad
    priority: 1
    rule_id: ac10_bad
    enabled: true
policies: []
"#;
    let path = write_temp_file("ac10.yaml", yaml).await;
    let report = loader.load_from_file(path.to_str().unwrap()).await.unwrap();
    assert_eq!(report.failed_count, 1);
    // 已有规则仍存在
    assert!(mgr
        .rule_registry()
        .get_rule_by_id("ac10_existing")
        .is_some());
    assert_eq!(mgr.rule_count(), 1, "已有规则应保留");
    cleanup(&path).await;
}

// ============================================================================
// AC-11：路径穿越拒绝
// ============================================================================

#[tokio::test]
async fn ac11_path_traversal_rejected() {
    let mgr = make_manager();
    let loader = make_loader(mgr);
    // 路径含 ".." 应被拒绝
    let err = loader.load_from_file("../../etc/passwd").await.unwrap_err();
    assert_eq!(err.error_code(), "PATH_NOT_ALLOWED");
}

// ============================================================================
// AC-13：热重载生效
// ============================================================================

#[tokio::test]
async fn ac13_hot_reload_takes_effect() {
    let mgr = make_manager();
    // 预置旧规则
    mgr.create_rule(DataScopeRule::new("old_table", DataScopeMode::All).with_rule_id("ac13_old"))
        .await
        .unwrap();
    assert_eq!(mgr.rule_count(), 1);

    let loader = make_loader(mgr.clone());
    let yaml = r#"
format_version: "1.0"
rules:
  - mode: all
    target_table: new_table
    priority: 1
    rule_id: ac13_new
    enabled: true
policies: []
"#;
    let path = write_temp_file("ac13.yaml", yaml).await;
    // reload with confirm_clear=true → 清空旧规则后加载新规则
    let report = loader
        .reload_from_file(path.to_str().unwrap(), true)
        .await
        .unwrap();
    assert_eq!(report.success_count, 1);
    assert_eq!(mgr.rule_count(), 1);
    assert!(mgr.rule_registry().get_rule_by_id("ac13_new").is_some());
    assert!(mgr.rule_registry().get_rule_by_id("ac13_old").is_none());
    cleanup(&path).await;
}

// ============================================================================
// AC-14：变更通知广播
// ============================================================================

#[tokio::test]
async fn ac14_change_event_broadcast() {
    let mgr = make_manager();
    let mut rx = mgr.subscribe();

    // 创建规则 → RuleCreated 事件
    let rule = DataScopeRule::new("order", DataScopeMode::All).with_rule_id("ac14_r1");
    let result = mgr.create_rule(rule).await.unwrap();
    let event = rx.recv().await.unwrap();
    match event {
        ChangeEvent::RuleCreated {
            generation,
            target_table,
            ..
        } => {
            assert_eq!(generation, result.generation);
            assert_eq!(target_table, "order");
        }
        _ => panic!("期望 RuleCreated 事件"),
    }

    // 创建策略 → PolicyCreated 事件
    let policy = FieldScopePolicy::new("employee", "hr");
    let result_p = mgr.create_policy(policy).await.unwrap();
    let event_p = rx.recv().await.unwrap();
    match event_p {
        ChangeEvent::PolicyCreated {
            generation,
            target_table,
            target_role,
            ..
        } => {
            assert_eq!(generation, result_p.generation);
            assert_eq!(target_table, "employee");
            assert_eq!(target_role, "hr");
        }
        _ => panic!("期望 PolicyCreated 事件"),
    }

    // 删除规则 → RuleDeleted 事件
    mgr.delete_rule("ac14_r1").await.unwrap();
    let event_d = rx.recv().await.unwrap();
    match event_d {
        ChangeEvent::RuleDeleted { target_table, .. } => {
            assert_eq!(target_table, "order");
        }
        _ => panic!("期望 RuleDeleted 事件"),
    }
}

// ============================================================================
// AC-15：世代号单调递增
// ============================================================================

#[tokio::test]
async fn ac15_generation_strictly_monotonic() {
    let mgr = make_manager();
    let mut prev = mgr.generation();

    for i in 0..10 {
        let rule = DataScopeRule::new(format!("t{i}"), DataScopeMode::All)
            .with_rule_id(format!("ac15_r{i}"));
        let result = mgr.create_rule(rule).await.unwrap();
        assert!(
            result.generation > prev,
            "世代号必须严格单调递增: gen={} prev={}",
            result.generation,
            prev
        );
        prev = result.generation;
    }
    assert_eq!(mgr.generation(), 10);
}

// ============================================================================
// 补充：策略 CRUD E2E
// ============================================================================

#[tokio::test]
async fn e2e_policy_crud_full_cycle() {
    let mgr = make_manager();

    // 创建
    let policy = FieldScopePolicy::new("employee", "hr")
        .with_field_rule("salary", FieldVisibility::Hidden)
        .with_field_rule("name", FieldVisibility::ReadOnly);
    let created = mgr.create_policy(policy).await.unwrap();
    assert_eq!(created.status, "created");
    assert_eq!(mgr.policy_count(), 1);

    // 查询
    let got = mgr.policy_registry().get_policy("employee", "hr").unwrap();
    assert_eq!(got.field_visibility("salary"), FieldVisibility::Hidden);
    assert_eq!(got.field_visibility("name"), FieldVisibility::ReadOnly);

    // 更新
    use sz_rust_orm_facade::data_scope::ext::hot_reload::PolicyPatch;
    let patch = PolicyPatch {
        enabled: Some(false),
        ..Default::default()
    };
    let updated = mgr.update_policy("employee", "hr", patch).await.unwrap();
    assert_eq!(updated.status, "updated");
    let got2 = mgr.policy_registry().get_policy("employee", "hr").unwrap();
    assert!(!got2.enabled);

    // 删除
    let deleted = mgr.delete_policy("employee", "hr").await.unwrap();
    assert_eq!(deleted.status, "deleted");
    assert_eq!(mgr.policy_count(), 0);
}

// ============================================================================
// 补充：reload 拒绝空配置（无 confirm_clear）
// ============================================================================

#[tokio::test]
async fn e2e_reload_reject_empty_without_confirm() {
    let mgr = make_manager();
    let loader = make_loader(mgr);
    let yaml = r#"
format_version: "1.0"
rules: []
policies: []
"#;
    let path = write_temp_file("reject_empty.yaml", yaml).await;
    let err = loader
        .reload_from_file(path.to_str().unwrap(), false)
        .await
        .unwrap_err();
    assert_eq!(err.error_code(), "DATA_SCOPE_INVALID_RULE");
    cleanup(&path).await;
}
