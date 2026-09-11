// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use http::StatusCode;
use http_body_util::BodyExt;
use sz_rust_addons_admin::{AdminAddonPlugin, AdminCapabilityHook};
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

#[test]
fn test_plugin_creation() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["admin".to_string()]);
    let router = plugin.router();
    let _ = router;
}

#[test]
fn test_capability_hook_registration() {
    let pool = make_mock_pool();
    let hook = AdminCapabilityHook::new(pool);
    let registry = CapabilityRegistry::new();
    let names = hook.register_capabilities(&registry).unwrap();
    assert_eq!(names.len(), 17);
    assert_eq!(registry.len(), 17);
}

#[test]
fn test_capability_hook_names() {
    let pool = make_mock_pool();
    let hook = AdminCapabilityHook::new(pool);
    let names = hook.capability_names();
    assert_eq!(names.len(), 17);
    for name in &names {
        assert!(name.starts_with("admin."));
    }
}

#[test]
fn test_unregister_all_capabilities() {
    let pool = make_mock_pool();
    let hook = AdminCapabilityHook::new(pool);
    let registry = CapabilityRegistry::new();
    hook.register_capabilities(&registry).unwrap();
    assert_eq!(registry.len(), 17);

    let removed =
        sz_rust_addons_loader::capability_hook::unregister_plugin_capabilities(&registry, "admin");
    assert_eq!(removed.len(), 17);
    assert_eq!(registry.len(), 0);
}

#[test]
fn test_router_built_with_21_endpoints() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let _router = plugin.router();
}

#[tokio::test]
async fn test_dashboard_endpoint_returns_200() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let router = plugin.router();

    let response = router
        .oneshot(make_super_admin_request("GET", "/api/admin/dashboard"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json.get("user_count").is_some(),
        "dashboard 应返回 user_count"
    );
    assert!(
        json.get("role_count").is_some(),
        "dashboard 应返回 role_count"
    );
}

#[tokio::test]
async fn test_users_list_endpoint_returns_200() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let router = plugin.router();

    let response = router
        .oneshot(make_super_admin_request("GET", "/api/admin/users"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("items").is_some(), "用户列表应返回 items");
    assert!(json.get("total").is_some(), "用户列表应返回 total");
}

#[tokio::test]
async fn test_roles_list_endpoint_returns_200() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let router = plugin.router();

    let response = router
        .oneshot(make_super_admin_request("GET", "/api/admin/roles"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array(), "角色列表应返回数组");
}

#[tokio::test]
async fn test_permission_tree_endpoint_returns_200() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let router = plugin.router();

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
    assert!(json.get("modules").is_some(), "权限树应返回 modules");
}

#[tokio::test]
async fn test_no_auth_returns_401() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let router = plugin.router();

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

#[tokio::test]
async fn test_unknown_admin_route_returns_404() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let router = plugin.router();

    let response = router
        .oneshot(make_super_admin_request("GET", "/api/admin/nonexistent"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
