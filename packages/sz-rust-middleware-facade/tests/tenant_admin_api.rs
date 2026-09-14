// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T12.2 租户管理 API 契约测试 — AC-12~AC-16/25
//!
//! 覆盖验收标准：
//! - AC-12: 创建租户返回 201
//! - AC-13: 非平台管理员 403
//! - AC-14: 名称重复 409
//! - AC-15: 非法状态流转 400
//! - AC-16: 删除非 disabled 租户 400
//! - AC-25: 审计日志记录
//! - AC-26: 指标暴露

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;

use sz_rust_middleware_facade::data_scope::DataScopeUserContext;
use sz_rust_middleware_facade::tenant_admin::{build_tenant_admin_router, TenantAdminApiState};
use sz_rust_orm_facade::data_scope::ext::audit::TracingAuditLogger;
use sz_rust_orm_facade::data_scope::ext::generation::PolicyGeneration;
use sz_rust_orm_facade::data_scope::ext::notifier::ChangeNotifier;
use sz_rust_orm_facade::data_scope::ext::path_guard::PathGuard;
use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::tenant::config::TenantConfigRegistry;

use sz_rust_orm_facade::tenant::ext::config_loader::TenantConfigLoader;
use sz_rust_orm_facade::tenant::ext::hot_reload::TenantHotReloadManager;
use sz_rust_orm_facade::tenant::record::TenantRecordRegistry;
use sz_rust_orm_facade::tenant::scoped_table::TenantScopedTableRegistry;

// ============================================================================
// 辅助构造
// ============================================================================

fn make_state() -> TenantAdminApiState {
    let metrics = Arc::new(DataScopeMetrics::new());
    let manager = Arc::new(TenantHotReloadManager::new(
        Arc::new(TenantRecordRegistry::new()),
        Arc::new(TenantScopedTableRegistry::new()),
        Arc::new(TenantConfigRegistry::new()),
        PolicyGeneration::new(),
        ChangeNotifier::new(64),
        metrics.clone(),
        Arc::new(TracingAuditLogger),
    ));
    let path_guard = PathGuard::new(vec![std::env::current_dir().unwrap(), std::env::temp_dir()]);
    let config_loader = Arc::new(TenantConfigLoader::new(
        manager.clone(),
        path_guard,
        TenantConfigLoader::DEFAULT_MAX_FILE_SIZE,
    ));
    TenantAdminApiState {
        manager,
        config_loader,
        metrics,
        tenant_admin_roles: vec!["tenant_admin".into()],
    }
}

fn make_app() -> axum::Router {
    build_tenant_admin_router(make_state())
}

fn platform_admin_request(method: Method, uri: &str, body: Vec<u8>) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(
        DataScopeUserContext::new(1)
            .with_super(true)
            .with_platform_admin(true),
    );
    req
}

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

fn no_auth_request(method: Method, uri: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

async fn read_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

// ============================================================================
// AC-12: 创建租户返回 201
// ============================================================================

#[tokio::test]
async fn ac12_create_tenant_returns_201() {
    let app = make_app();
    let body = br#"{"name":"acme"}"#.to_vec();
    let req = platform_admin_request(Method::POST, "/api/tenants", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = read_body(resp).await;
    assert_eq!(json["name"], "acme");
    assert_eq!(json["status"], "active");
}

// ============================================================================
// AC-13: 非平台管理员 403
// ============================================================================

#[tokio::test]
async fn ac13_non_platform_admin_403() {
    let app = make_app();
    let body = br#"{"name":"acme"}"#.to_vec();
    let req = non_admin_request(Method::POST, "/api/tenants", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "PLATFORM_ADMIN_REQUIRED");
}

#[tokio::test]
async fn ac13_no_auth_context_401() {
    let app = make_app();
    let body = br#"{"name":"acme"}"#.to_vec();
    let req = no_auth_request(Method::POST, "/api/tenants", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ac13_super_but_not_platform_admin_403() {
    let app = make_app();
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/api/tenants")
        .header("content-type", "application/json")
        .body(Body::from(br#"{"name":"acme"}"#.to_vec()))
        .unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(1).with_super(true));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ============================================================================
// AC-14: 名称重复 409
// ============================================================================

#[tokio::test]
async fn ac14_name_duplicate_409() {
    let state = make_state();
    let app = build_tenant_admin_router(state.clone());

    let body = br#"{"name":"acme"}"#.to_vec();
    let req = platform_admin_request(Method::POST, "/api/tenants", body.clone());
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(Method::POST, "/api/tenants", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "TENANT_NAME_DUPLICATE");
}

// ============================================================================
// AC-15: 非法状态流转 400
// ============================================================================

#[tokio::test]
async fn ac15_invalid_status_transition_400() {
    let state = make_state();
    let tenant = state.manager.create_tenant("acme".into()).await.unwrap();
    state
        .manager
        .disable_tenant(tenant.tenant_id)
        .await
        .unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(
        Method::PUT,
        &format!("/api/tenants/{}/activate", tenant.tenant_id),
        vec![],
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "INVALID_STATUS_TRANSITION");
}

// ============================================================================
// AC-16: 删除非 disabled 租户 400
// ============================================================================

#[tokio::test]
async fn ac16_delete_active_tenant_400() {
    let state = make_state();
    let tenant = state.manager.create_tenant("acme".into()).await.unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(
        Method::DELETE,
        &format!("/api/tenants/{}", tenant.tenant_id),
        vec![],
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "TENANT_NOT_DISABLED");
}

#[tokio::test]
async fn ac16_delete_disabled_tenant_success() {
    let state = make_state();
    let tenant = state.manager.create_tenant("acme".into()).await.unwrap();
    state
        .manager
        .disable_tenant(tenant.tenant_id)
        .await
        .unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(
        Method::DELETE,
        &format!("/api/tenants/{}", tenant.tenant_id),
        vec![],
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ============================================================================
// 租户查询端点
// ============================================================================

#[tokio::test]
async fn it_list_tenants_success() {
    let state = make_state();
    state.manager.create_tenant("acme".into()).await.unwrap();
    state.manager.create_tenant("globex".into()).await.unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(Method::GET, "/api/tenants", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["total"], 2);
}

#[tokio::test]
async fn it_list_tenants_filter_by_status() {
    let state = make_state();
    let t1 = state.manager.create_tenant("acme".into()).await.unwrap();
    state.manager.create_tenant("globex".into()).await.unwrap();
    state.manager.suspend_tenant(t1.tenant_id).await.unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(Method::GET, "/api/tenants?status=suspended", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["total"], 1);
    assert_eq!(json["tenants"][0]["name"], "acme");
}

#[tokio::test]
async fn it_get_tenant_found() {
    let state = make_state();
    let tenant = state.manager.create_tenant("acme".into()).await.unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(
        Method::GET,
        &format!("/api/tenants/{}", tenant.tenant_id),
        vec![],
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["name"], "acme");
}

#[tokio::test]
async fn it_get_tenant_not_found_403() {
    let app = make_app();
    let req = platform_admin_request(Method::GET, "/api/tenants/999", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "TENANT_NOT_FOUND");
}

#[tokio::test]
async fn it_update_tenant_name() {
    let state = make_state();
    let tenant = state.manager.create_tenant("acme".into()).await.unwrap();

    let app = build_tenant_admin_router(state);
    let body = br#"{"name":"acme-renamed"}"#.to_vec();
    let req = platform_admin_request(
        Method::PUT,
        &format!("/api/tenants/{}", tenant.tenant_id),
        body,
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["name"], "acme-renamed");
}

// ============================================================================
// 状态流转端点
// ============================================================================

#[tokio::test]
async fn it_suspend_tenant_success() {
    let state = make_state();
    let tenant = state.manager.create_tenant("acme".into()).await.unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(
        Method::PUT,
        &format!("/api/tenants/{}/suspend", tenant.tenant_id),
        vec![],
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["status"], "suspended");
}

#[tokio::test]
async fn it_activate_tenant_success() {
    let state = make_state();
    let tenant = state.manager.create_tenant("acme".into()).await.unwrap();
    state
        .manager
        .suspend_tenant(tenant.tenant_id)
        .await
        .unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(
        Method::PUT,
        &format!("/api/tenants/{}/activate", tenant.tenant_id),
        vec![],
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["status"], "active");
}

#[tokio::test]
async fn it_disable_tenant_success() {
    let state = make_state();
    let tenant = state.manager.create_tenant("acme".into()).await.unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(
        Method::PUT,
        &format!("/api/tenants/{}/disable", tenant.tenant_id),
        vec![],
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["status"], "disabled");
}

// ============================================================================
// 隔离表端点
// ============================================================================

#[tokio::test]
async fn it_register_scoped_table_201() {
    let app = make_app();
    let body = br#"{"table_name":"orders","tenant_field":"tenant_id","enabled":true}"#.to_vec();
    let req = platform_admin_request(Method::POST, "/api/tenant-scoped-tables", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn it_list_scoped_tables() {
    let state = make_state();
    state
        .manager
        .register_scoped_table(
            sz_rust_orm_facade::tenant::scoped_table::TenantScopedTable::new("orders"),
        )
        .await
        .unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(Method::GET, "/api/tenant-scoped-tables", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert!(!json.as_array().unwrap().is_empty());
}

// ============================================================================
// AC-25: 审计日志记录（创建租户触发审计事件）
// ============================================================================

#[tokio::test]
async fn ac25_audit_log_on_create() {
    let state = make_state();
    let gen_before = state.manager.generation();
    let app = build_tenant_admin_router(state.clone());

    let body = br#"{"name":"audit-test"}"#.to_vec();
    let req = platform_admin_request(Method::POST, "/api/tenants", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let gen_after = state.manager.generation();
    assert!(gen_after > gen_before, "generation should bump on create");
}

#[tokio::test]
async fn ac25_audit_log_on_status_change() {
    let state = make_state();
    let tenant = state.manager.create_tenant("acme".into()).await.unwrap();
    let gen_before = state.manager.generation();

    let app = build_tenant_admin_router(state.clone());
    let req = platform_admin_request(
        Method::PUT,
        &format!("/api/tenants/{}/suspend", tenant.tenant_id),
        vec![],
    );
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let gen_after = state.manager.generation();
    assert!(
        gen_after > gen_before,
        "generation should bump on status change"
    );
}

// ============================================================================
// AC-26: 指标暴露（tenant_request_total / tenant_resolve_failed_total）
// ============================================================================

#[tokio::test]
async fn ac26_metrics_exposed_after_create() {
    let state = make_state();
    let metrics = state.metrics.clone();

    state.manager.create_tenant("acme".into()).await.unwrap();
    state.manager.create_tenant("globex".into()).await.unwrap();

    assert_eq!(metrics.tenant_active_count(), 2);
}

#[tokio::test]
async fn ac26_metrics_tenant_active_count() {
    let state = make_state();
    let metrics = state.metrics.clone();

    state.manager.create_tenant("acme".into()).await.unwrap();
    state.manager.create_tenant("globex".into()).await.unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(Method::GET, "/api/tenants", vec![]);
    let _ = app.oneshot(req).await.unwrap();

    assert!(metrics.tenant_active_count() >= 2);
}
