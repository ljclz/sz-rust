// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Admin 插件生产接线验证测试
//!
//! 验证 `AdminAddonPlugin` 可以被主应用正确加载：
//! - 插件路由合并到主 Router 后，admin 端点可达
//! - CapabilityHook 注册 17 个 admin.* 能力
//! - 注销后 Capability 全部移除
//! - 未认证请求返回 401
//! - 超级管理员请求通过权限校验

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use http::StatusCode;
use http_body_util::BodyExt;
use sz_rust_addons_admin::AdminAddonPlugin;
use sz_rust_addons_loader::capability_hook::CapabilityHook;
use sz_rust_capability::CapabilityRegistry;
use sz_rust_middleware_facade::data_scope::DataScopeUserContext;
use sz_rust_orm_facade::tenant::context::{TenantContext, TenantResolveSource};
use sz_rust_orm_facade::{Connection, ConnectionFactory, DbError, Pool, PoolConfig, Value};
use tower::ServiceExt;

type QueryRows = Vec<HashMap<String, Value>>;

struct MockConnection;

impl Connection for MockConnection {
    fn execute<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(0) })
    }
    fn query<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn execute_with_params<'a>(
        &'a mut self,
        _sql: &'a str,
        _params: &'a [Value],
    ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(1) })
    }
    fn query_with_params<'a>(
        &'a mut self,
        _sql: &'a str,
        _params: &'a [Value],
    ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn begin_transaction<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn commit<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn rollback<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn is_connected(&self) -> bool {
        true
    }
    fn ping<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async { true })
    }
    fn close<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

struct MockConnectionFactory;

#[async_trait]
impl ConnectionFactory for MockConnectionFactory {
    async fn create(&self) -> Result<Box<dyn Connection>, DbError> {
        Ok(Box::new(MockConnection))
    }
}

fn make_mock_pool() -> Arc<Pool> {
    let config = PoolConfig::default();
    let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory);
    Arc::new(Pool::new(config, factory).expect("mock pool creation should not fail"))
}

fn make_super_admin_request(method: &str, uri: &str) -> http::Request<axum::body::Body> {
    let user = DataScopeUserContext::new(1)
        .with_super(true)
        .with_roles(vec!["super_admin".to_string()]);
    let tenant = TenantContext::new(0, true, TenantResolveSource::Header);
    http::Request::builder()
        .method(method)
        .uri(uri)
        .extension(user)
        .extension(tenant)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn build_wired_router() -> Router {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let admin_router = plugin.router();
    Router::new()
        .route("/", axum::routing::get(|| async { "ok" }))
        .merge(admin_router)
}

/// 主应用合并 admin 插件路由后，dashboard 端点应返回 200
#[tokio::test]
async fn test_wired_dashboard_endpoint() {
    let router = build_wired_router();
    let response = router
        .oneshot(make_super_admin_request("GET", "/api/admin/dashboard"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("user_count").is_some());
}

/// 主应用合并 admin 插件路由后，users 端点应返回 200
#[tokio::test]
async fn test_wired_users_endpoint() {
    let router = build_wired_router();
    let response = router
        .oneshot(make_super_admin_request("GET", "/api/admin/users"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("items").is_some());
}

/// 主应用合并 admin 插件路由后，roles 端点应返回 200
#[tokio::test]
async fn test_wired_roles_endpoint() {
    let router = build_wired_router();
    let response = router
        .oneshot(make_super_admin_request("GET", "/api/admin/roles"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array());
}

/// 主应用合并 admin 插件路由后，permissions/tree 端点应返回 200
#[tokio::test]
async fn test_wired_permission_tree_endpoint() {
    let router = build_wired_router();
    let response = router
        .oneshot(make_super_admin_request(
            "GET",
            "/api/admin/permissions/tree",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("modules").is_some());
}

/// 主应用合并 admin 插件路由后，未认证请求应返回 401
#[tokio::test]
async fn test_wired_no_auth_returns_401() {
    let router = build_wired_router();
    let response = router
        .oneshot(
            http::Request::builder()
                .method("GET")
                .uri("/api/admin/dashboard")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// 主应用合并 admin 插件路由后，未知 admin 路由应返回 404
#[tokio::test]
async fn test_wired_unknown_admin_route_returns_404() {
    let router = build_wired_router();
    let response = router
        .oneshot(make_super_admin_request("GET", "/api/admin/nonexistent"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// 主应用根路由不受 admin 插件影响
#[tokio::test]
async fn test_wired_root_route_still_works() {
    let router = build_wired_router();
    let response = router
        .oneshot(
            http::Request::builder()
                .method("GET")
                .uri("/")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// CapabilityHook 注册 17 个 admin.* 能力到 Registry
#[tokio::test]
async fn test_wired_capability_registration() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let hook = plugin.capability_hook();
    let registry = CapabilityRegistry::new();
    let names = hook.register_capabilities(&registry).unwrap();
    assert_eq!(names.len(), 17);
    assert_eq!(registry.len(), 17);
    for name in &names {
        assert!(name.starts_with("admin."));
    }
}

/// 注销后 Capability 全部移除
#[tokio::test]
async fn test_wired_capability_unregistration() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let hook = plugin.capability_hook();
    let registry = CapabilityRegistry::new();
    hook.register_capabilities(&registry).unwrap();
    assert_eq!(registry.len(), 17);

    let removed =
        sz_rust_addons_loader::capability_hook::unregister_plugin_capabilities(&registry, "admin");
    assert_eq!(removed.len(), 17);
    assert_eq!(registry.len(), 0);
}
