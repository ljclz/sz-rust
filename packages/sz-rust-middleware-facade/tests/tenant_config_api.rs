// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T12.3 租户配置 API 契约测试 — AC-17~AC-20/29
//!
//! 覆盖验收标准：
//! - AC-17: 配置隔离，租户2读不到租户1配置
//! - AC-18: 全局继承
//! - AC-19: 租户覆盖全局
//! - AC-20: 租户管理员改全局配置 403
//! - AC-29: 配置更新缓存失效

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
use sz_rust_orm_facade::tenant::context::{TenantContext, TenantResolveSource};
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

fn tenant_admin_request(method: Method, uri: &str, body: Vec<u8>, tenant_id: i64) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(5).with_roles(vec!["tenant_admin".into()]));
    req.extensions_mut().insert(TenantContext::new(
        tenant_id,
        false,
        TenantResolveSource::Header,
    ));
    req
}

async fn read_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

// ============================================================================
// AC-17: 配置隔离 — 租户2读不到租户1配置
// ============================================================================

#[tokio::test]
async fn ac17_tenant_config_isolation() {
    let state = make_state();
    state
        .manager
        .set_tenant_config(
            1,
            "secret",
            "tenant1-value",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();

    let app = build_tenant_admin_router(state);

    let req = tenant_admin_request(Method::GET, "/api/tenant-configs", vec![], 2);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["total"], 0, "tenant 2 should not see tenant 1 configs");
}

// ============================================================================
// AC-18: 全局继承 — 租户未设置配置时继承全局配置
// ============================================================================

#[tokio::test]
async fn ac18_global_config_inheritance() {
    let state = make_state();
    state
        .manager
        .set_global_config(
            "theme",
            "dark",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();

    let app = build_tenant_admin_router(state);

    let req = tenant_admin_request(Method::GET, "/api/tenant-configs", vec![], 1);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["total"], 1, "tenant should inherit global config");
    assert_eq!(json["configs"][0]["config_key"], "theme");
    assert_eq!(json["configs"][0]["config_value"], "dark");
    assert_eq!(json["configs"][0]["source"], "global");
}

// ============================================================================
// AC-19: 租户覆盖全局配置
// ============================================================================

#[tokio::test]
async fn ac19_tenant_overrides_global() {
    let state = make_state();
    state
        .manager
        .set_global_config(
            "theme",
            "dark",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();
    state
        .manager
        .set_tenant_config(
            1,
            "theme",
            "light",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();

    let app = build_tenant_admin_router(state);

    let req = tenant_admin_request(Method::GET, "/api/tenant-configs", vec![], 1);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert_eq!(json["total"], 1);
    assert_eq!(json["configs"][0]["config_value"], "light");
    assert_eq!(json["configs"][0]["source"], "tenant");
}

// ============================================================================
// AC-20: 租户管理员改全局配置 403
// ============================================================================

#[tokio::test]
async fn ac20_tenant_admin_cannot_set_global_config() {
    let app = make_app();
    let body = br#"{"config_value":"dark","config_type":"string"}"#.to_vec();
    let req = tenant_admin_request(Method::PUT, "/api/global-configs/theme", body, 1);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "PLATFORM_ADMIN_REQUIRED");
}

#[tokio::test]
async fn ac20_platform_admin_can_set_global_config() {
    let state = make_state();
    let app = build_tenant_admin_router(state);
    let body = br#"{"config_value":"dark","config_type":"string"}"#.to_vec();
    let req = platform_admin_request(Method::PUT, "/api/global-configs/theme", body);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ============================================================================
// AC-29: 配置更新缓存失效
// ============================================================================

#[tokio::test]
async fn ac29_config_update_invalidates_cache() {
    let state = make_state();

    state
        .manager
        .set_tenant_config(
            1,
            "theme",
            "dark",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();

    let (value, source) = state.manager.get_tenant_config(1, "theme").unwrap();
    assert_eq!(value, serde_json::json!("dark"));
    assert_eq!(
        source,
        sz_rust_orm_facade::tenant::config::ConfigSource::Tenant
    );

    state
        .manager
        .set_tenant_config(
            1,
            "theme",
            "light",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();

    let (value, _) = state.manager.get_tenant_config(1, "theme").unwrap();
    assert_eq!(
        value,
        serde_json::json!("light"),
        "cache should be invalidated after update"
    );
}

#[tokio::test]
async fn ac29_global_config_update_invalidates_all_tenant_caches() {
    let state = make_state();

    state
        .manager
        .set_global_config(
            "theme",
            "dark",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();

    let (value, _) = state.manager.get_tenant_config(1, "theme").unwrap();
    assert_eq!(value, serde_json::json!("dark"));

    state
        .manager
        .set_global_config(
            "theme",
            "light",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();

    let (value, _) = state.manager.get_tenant_config(1, "theme").unwrap();
    assert_eq!(
        value,
        serde_json::json!("light"),
        "global cache invalidation should propagate"
    );
}

// ============================================================================
// 租户配置 CRUD 端点
// ============================================================================

#[tokio::test]
async fn it_set_tenant_config_success() {
    let state = make_state();
    let app = build_tenant_admin_router(state);
    let body = br#"{"config_value":"dark","config_type":"string"}"#.to_vec();
    let req = tenant_admin_request(Method::PUT, "/api/tenant-configs/theme", body, 1);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn it_delete_tenant_config_falls_back_to_global() {
    let state = make_state();
    state
        .manager
        .set_global_config(
            "theme",
            "dark",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();
    state
        .manager
        .set_tenant_config(
            1,
            "theme",
            "light",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();

    let app = build_tenant_admin_router(state.clone());
    let req = tenant_admin_request(Method::DELETE, "/api/tenant-configs/theme", vec![], 1);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let (value, source) = state.manager.get_tenant_config(1, "theme").unwrap();
    assert_eq!(value, serde_json::json!("dark"));
    assert_eq!(
        source,
        sz_rust_orm_facade::tenant::config::ConfigSource::Global
    );
}

#[tokio::test]
async fn it_list_global_configs() {
    let state = make_state();
    state
        .manager
        .set_global_config(
            "theme",
            "dark",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();
    state
        .manager
        .set_global_config(
            "locale",
            "en",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();

    let app = build_tenant_admin_router(state);
    let req = platform_admin_request(Method::GET, "/api/global-configs", vec![]);
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = read_body(resp).await;
    assert!(json.as_array().unwrap().len() >= 2);
}

// ============================================================================
// 配置鉴权 — 非租户管理员不能访问租户配置
// ============================================================================

#[tokio::test]
async fn it_non_tenant_admin_cannot_list_configs() {
    let app = make_app();
    let mut req = Request::builder()
        .method(Method::GET)
        .uri("/api/tenant-configs")
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(10).with_roles(vec!["user".into()]));
    req.extensions_mut()
        .insert(TenantContext::new(1, false, TenantResolveSource::Header));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn it_super_can_list_tenant_configs() {
    let state = make_state();
    state
        .manager
        .set_tenant_config(
            1,
            "theme",
            "dark",
            sz_rust_orm_facade::tenant::config::TenantConfigType::String,
        )
        .await
        .unwrap();

    let app = build_tenant_admin_router(state);
    let mut req = Request::builder()
        .method(Method::GET)
        .uri("/api/tenant-configs")
        .header("content-type", "application/json")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(1).with_super(true));
    req.extensions_mut()
        .insert(TenantContext::new(1, false, TenantResolveSource::Header));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
