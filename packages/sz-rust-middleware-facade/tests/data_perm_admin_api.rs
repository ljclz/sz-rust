// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 管理后台 API 契约测试 — 覆盖全部 13 个端点的成功与错误响应
//!
//! 覆盖验收标准：
//! - AC-02：非管理员 403
//! - AC-18：generation 端点
//! - AC-25：敏感字段脱敏（custom_generator 仅暴露名称）
//!
//! 使用 axum::Router + tower::ServiceExt 构造模拟请求，
//! 请求中注入 DataScopeUserContext（is_super=true 或 roles 含 admin role）以通过鉴权。

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

use sz_rust_middleware_facade::data_perm_admin::{build_admin_router, AdminApiState};
use sz_rust_middleware_facade::data_scope::DataScopeUserContext;
use sz_rust_orm_facade::data_scope::custom::CustomGeneratorRegistry;
use sz_rust_orm_facade::data_scope::ext::audit::TracingAuditLogger;
use sz_rust_orm_facade::data_scope::ext::config_loader::ConfigLoader;
use sz_rust_orm_facade::data_scope::ext::generation::PolicyGeneration;
use sz_rust_orm_facade::data_scope::ext::hot_reload::HotReloadManager;
use sz_rust_orm_facade::data_scope::ext::notifier::ChangeNotifier;
use sz_rust_orm_facade::data_scope::ext::path_guard::PathGuard;
use sz_rust_orm_facade::data_scope::field_scope::registry::FieldScopePolicyRegistry;
use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::data_scope::registry::DataScopeRuleRegistry;

// ============================================================================
// 辅助构造
// ============================================================================

fn make_state() -> AdminApiState {
    let rule_registry = Arc::new(DataScopeRuleRegistry::new());
    let policy_registry = Arc::new(FieldScopePolicyRegistry::new());
    let custom_registry = Arc::new(CustomGeneratorRegistry::new());
    let generation = PolicyGeneration::new();
    let notifier = ChangeNotifier::new(64);
    let metrics = Arc::new(DataScopeMetrics::new());
    let audit: Arc<dyn sz_rust_orm_facade::data_scope::ext::audit::AuditLogger> =
        Arc::new(TracingAuditLogger);
    let manager = Arc::new(HotReloadManager::new(
        rule_registry,
        policy_registry,
        custom_registry,
        generation,
        notifier,
        metrics.clone(),
        audit,
    ));
    let path_guard = PathGuard::new(vec![std::env::current_dir().unwrap(), std::env::temp_dir()]);
    let config_loader = Arc::new(ConfigLoader::new(
        manager.clone(),
        path_guard,
        ConfigLoader::DEFAULT_MAX_FILE_SIZE,
    ));
    AdminApiState {
        manager,
        config_loader,
        metrics,
        admin_roles: vec!["admin".into()],
    }
}

fn make_app() -> axum::Router {
    build_admin_router(make_state())
}

/// 构造带超级管理员上下文的请求
fn super_admin_request(method: Method, uri: &str, body: Vec<u8>) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(1).with_super(true));
    req
}

/// 构造带 admin 角色的请求
fn role_admin_request(method: Method, uri: &str, body: Vec<u8>) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(5).with_roles(vec!["admin".into()]));
    req
}

/// 构造无鉴权上下文的请求
fn no_auth_request(method: Method, uri: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

/// 构造非管理员角色的请求
fn non_admin_request(method: Method, uri: &str, body: Vec<u8>) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(10).with_roles(vec!["user".into()]));
    req
}

async fn read_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

// ============================================================================
// AC-02：非管理员 403
// ============================================================================

#[tokio::test]
async fn ac02_non_admin_rejected_403() {
    let app = make_app();
    let req = non_admin_request(Method::GET, "/api/data-perm/generation", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "ADMIN_REQUIRED");
}

#[tokio::test]
async fn ac02_missing_token_401() {
    let app = make_app();
    let req = no_auth_request(Method::GET, "/api/data-perm/generation", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "AUTH_REQUIRED");
}

// ============================================================================
// AC-18：generation 端点
// ============================================================================

#[tokio::test]
async fn ac18_generation_endpoint() {
    let app = make_app();
    let req = super_admin_request(Method::GET, "/api/data-perm/generation", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["generation"], 0);
    assert_eq!(json["rule_count"], 0);
    assert_eq!(json["policy_count"], 0);
}

#[tokio::test]
async fn ac18_generation_after_create_rule() {
    let app = make_app();
    // 创建一条规则
    let body = serde_json::json!({
        "mode": "all",
        "target_table": "order",
        "priority": 1,
        "rule_id": "ac18_r1",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/rules",
        serde_json::to_vec(&body).unwrap(),
    );
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // 查询世代号
    let req = super_admin_request(Method::GET, "/api/data-perm/generation", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["generation"], 1);
    assert_eq!(json["rule_count"], 1);
}

// ============================================================================
// 规则 CRUD 端点测试
// ============================================================================

#[tokio::test]
async fn api_create_rule_success() {
    let app = make_app();
    let body = serde_json::json!({
        "mode": "all",
        "target_table": "order",
        "priority": 5,
        "rule_id": "api_r1",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/rules",
        serde_json::to_vec(&body).unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = read_body(resp).await;
    assert_eq!(json["status"], "created");
    assert_eq!(json["rule_id"], "api_r1");
}

#[tokio::test]
async fn api_create_rule_invalid_body_500() {
    let app = make_app();
    // Dept 模式缺 dept_field → RULE_FIELD_MISSING → 500
    let body = serde_json::json!({
        "mode": "dept",
        "target_table": "order",
        "priority": 1,
        "rule_id": "bad_r1",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/rules",
        serde_json::to_vec(&body).unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "RULE_FIELD_MISSING");
}

#[tokio::test]
async fn api_list_rules_empty() {
    let app = make_app();
    let req = super_admin_request(Method::GET, "/api/data-perm/rules", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["total"], 0);
    assert!(json["rules"].is_array());
}

#[tokio::test]
async fn api_list_rules_with_pagination() {
    let app = make_app();
    // 创建 3 条规则（不同 table 避免 upsert 去重）
    for i in 0..3 {
        let body = serde_json::json!({
            "mode": "all",
            "target_table": format!("t{i}"),
            "priority": i,
            "rule_id": format!("p_r{i}"),
            "enabled": true
        });
        let req = super_admin_request(
            Method::POST,
            "/api/data-perm/rules",
            serde_json::to_vec(&body).unwrap(),
        );
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }
    // 分页查询 size=2
    let req = super_admin_request(Method::GET, "/api/data-perm/rules?page=1&size=2", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["total"], 3);
    assert_eq!(json["size"], 2);
    assert_eq!(json["rules"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn api_get_rule_found() {
    let app = make_app();
    // 先创建
    let body = serde_json::json!({
        "mode": "all",
        "target_table": "order",
        "priority": 1,
        "rule_id": "get_r1",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/rules",
        serde_json::to_vec(&body).unwrap(),
    );
    let _ = app.clone().oneshot(req).await.unwrap();

    // 查询
    let req = super_admin_request(Method::GET, "/api/data-perm/rules/get_r1", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["rule_id"], "get_r1");
}

#[tokio::test]
async fn api_get_rule_not_found_404() {
    let app = make_app();
    let req = super_admin_request(Method::GET, "/api/data-perm/rules/nonexistent", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "RULE_NOT_FOUND");
}

#[tokio::test]
async fn api_update_rule_success() {
    let app = make_app();
    // 先创建
    let body = serde_json::json!({
        "mode": "all",
        "target_table": "order",
        "priority": 1,
        "rule_id": "upd_r1",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/rules",
        serde_json::to_vec(&body).unwrap(),
    );
    let _ = app.clone().oneshot(req).await.unwrap();

    // 更新
    let patch = serde_json::json!({ "priority": 99 });
    let req = super_admin_request(
        Method::PUT,
        "/api/data-perm/rules/upd_r1",
        serde_json::to_vec(&patch).unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["status"], "updated");
}

#[tokio::test]
async fn api_delete_rule_success() {
    let app = make_app();
    // 先创建
    let body = serde_json::json!({
        "mode": "all",
        "target_table": "order",
        "priority": 1,
        "rule_id": "del_r1",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/rules",
        serde_json::to_vec(&body).unwrap(),
    );
    let _ = app.clone().oneshot(req).await.unwrap();

    // 删除
    let req = super_admin_request(Method::DELETE, "/api/data-perm/rules/del_r1", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["status"], "deleted");
}

#[tokio::test]
async fn api_delete_rule_not_found_404() {
    let app = make_app();
    let req = super_admin_request(Method::DELETE, "/api/data-perm/rules/nope", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ============================================================================
// 策略 CRUD 端点测试
// ============================================================================

#[tokio::test]
async fn api_create_policy_success() {
    let app = make_app();
    let body = serde_json::json!({
        "table_name": "employee",
        "role_name": "hr",
        "field_rules": [["salary", "hidden"]],
        "default_visibility": "visible",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/policies",
        serde_json::to_vec(&body).unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = read_body(resp).await;
    assert_eq!(json["policy_id"], "employee:hr");
}

#[tokio::test]
async fn api_list_policies() {
    let app = make_app();
    // 创建策略
    let body = serde_json::json!({
        "table_name": "employee",
        "role_name": "hr",
        "field_rules": [],
        "default_visibility": "visible",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/policies",
        serde_json::to_vec(&body).unwrap(),
    );
    let _ = app.clone().oneshot(req).await.unwrap();

    // 列表
    let req = super_admin_request(Method::GET, "/api/data-perm/policies", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["total"], 1);
}

#[tokio::test]
async fn api_get_policy_found() {
    let app = make_app();
    let body = serde_json::json!({
        "table_name": "employee",
        "role_name": "hr",
        "field_rules": [["salary", "hidden"]],
        "default_visibility": "visible",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/policies",
        serde_json::to_vec(&body).unwrap(),
    );
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = super_admin_request(Method::GET, "/api/data-perm/policies/employee/hr", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["table_name"], "employee");
    assert_eq!(json["role_name"], "hr");
}

#[tokio::test]
async fn api_get_policy_not_found_404() {
    let app = make_app();
    let req = super_admin_request(Method::GET, "/api/data-perm/policies/no/no", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_update_policy_success() {
    let app = make_app();
    let body = serde_json::json!({
        "table_name": "t",
        "role_name": "r",
        "field_rules": [],
        "default_visibility": "visible",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/policies",
        serde_json::to_vec(&body).unwrap(),
    );
    let _ = app.clone().oneshot(req).await.unwrap();

    let patch = serde_json::json!({ "enabled": false });
    let req = super_admin_request(
        Method::PUT,
        "/api/data-perm/policies/t/r",
        serde_json::to_vec(&patch).unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["status"], "updated");
}

#[tokio::test]
async fn api_delete_policy_success() {
    let app = make_app();
    let body = serde_json::json!({
        "table_name": "t",
        "role_name": "r",
        "field_rules": [],
        "default_visibility": "visible",
        "enabled": true
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/policies",
        serde_json::to_vec(&body).unwrap(),
    );
    let _ = app.clone().oneshot(req).await.unwrap();

    let req = super_admin_request(Method::DELETE, "/api/data-perm/policies/t/r", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["status"], "deleted");
}

// ============================================================================
// 配置端点测试
// ============================================================================

#[tokio::test]
async fn api_load_config_file_not_found() {
    let app = make_app();
    let body = serde_json::json!({ "path": "/nonexistent/path/to/file.yaml" });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/load",
        serde_json::to_vec(&body).unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::INTERNAL_SERVER_ERROR,
        "unexpected status: {status}"
    );
}

#[tokio::test]
async fn api_reload_config_file_not_found() {
    let app = make_app();
    let body =
        serde_json::json!({ "path": "/nonexistent/path/to/file.yaml", "confirm_clear": false });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/reload",
        serde_json::to_vec(&body).unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::INTERNAL_SERVER_ERROR,
        "unexpected status: {status}"
    );
}

#[tokio::test]
async fn api_load_config_success() {
    let app = make_app();
    // 写入临时 YAML 文件
    let dir = std::env::temp_dir().join("sz_rust_admin_api_contract");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let path = dir.join("api_load.yaml");
    let yaml = r#"
format_version: "1.0"
rules:
  - mode: all
    target_table: order
    priority: 1
    rule_id: api_load_r1
    enabled: true
policies: []
"#;
    tokio::fs::write(&path, yaml).await.unwrap();

    let body = serde_json::json!({ "path": path.to_str().unwrap() });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/load",
        serde_json::to_vec(&body).unwrap(),
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["success_count"], 1);
    assert_eq!(json["failed_count"], 0);

    // 清理
    tokio::fs::remove_file(&path).await.unwrap();
}

// ============================================================================
// AC-25：敏感字段脱敏（custom_generator 仅暴露名称）
// ============================================================================

#[tokio::test]
async fn ac25_custom_generator_only_name_exposed() {
    let app = make_app();
    // 创建带 custom_generator 的规则
    let body = serde_json::json!({
        "mode": "all",
        "target_table": "order",
        "priority": 1,
        "rule_id": "ac25_r1",
        "enabled": true,
        "custom_generator": "my_custom_gen"
    });
    let req = super_admin_request(
        Method::POST,
        "/api/data-perm/rules",
        serde_json::to_vec(&body).unwrap(),
    );
    let _ = app.clone().oneshot(req).await.unwrap();

    // 查询规则，custom_generator 应为字符串名称
    let req = super_admin_request(Method::GET, "/api/data-perm/rules/ac25_r1", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["custom_generator"], "my_custom_gen");
    assert!(
        json["custom_generator"].is_string(),
        "custom_generator 应为字符串名称，不暴露内部实现"
    );
}

// ============================================================================
// 角色管理员鉴权通过
// ============================================================================

#[tokio::test]
async fn api_role_admin_pass() {
    let app = make_app();
    let req = role_admin_request(Method::GET, "/api/data-perm/generation", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ============================================================================
// 错误响应结构一致性
// ============================================================================

#[tokio::test]
async fn api_error_response_structure() {
    let app = make_app();
    let req = super_admin_request(Method::GET, "/api/data-perm/rules/nope", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let json = read_body(resp).await;
    // 验证 ApiErrorResponse 四个字段都存在
    assert!(json["code"].is_string());
    assert!(json["message"].is_string());
    assert!(json["generation"].is_number());
    assert!(json["details"].is_object() || json["details"].is_null());
}
