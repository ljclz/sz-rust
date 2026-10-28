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

fn cnt_row(n: i64) -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("cnt".into(), Value::I64(n));
    row
}

fn user_row() -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::I64(1));
    row.insert("username".into(), Value::String("alice".into()));
    row.insert("email".into(), Value::String("alice@example.com".into()));
    row.insert("phone".into(), Value::Null);
    row.insert("status".into(), Value::String("active".into()));
    row.insert("tenant_id".into(), Value::I64(0));
    row.insert(
        "created_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row.insert(
        "updated_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row.insert("last_login_at".into(), Value::Null);
    row
}

fn role_row() -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::I64(1));
    row.insert("name".into(), Value::String("管理员".into()));
    row.insert("code".into(), Value::String("admin".into()));
    row.insert("description".into(), Value::Null);
    row.insert("is_builtin".into(), Value::Bool(false));
    row.insert("tenant_id".into(), Value::I64(0));
    row.insert(
        "created_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row.insert(
        "updated_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row
}

fn menu_row() -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::I64(1));
    row.insert("name".into(), Value::String("用户管理".into()));
    row.insert("code".into(), Value::String("user_mgr".into()));
    row.insert("path".into(), Value::String("/admin/users".into()));
    row.insert("icon".into(), Value::Null);
    row.insert("parent_id".into(), Value::I64(0));
    row.insert("sort".into(), Value::I32(0));
    row.insert("is_visible".into(), Value::Bool(true));
    row.insert("permission_code".into(), Value::Null);
    row.insert("tenant_id".into(), Value::I64(0));
    row.insert(
        "created_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row.insert(
        "updated_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row
}

fn config_row() -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::I64(1));
    row.insert("key".into(), Value::String("site_name".into()));
    row.insert("value".into(), Value::String("\"MySite\"".into()));
    row.insert("value_type".into(), Value::String("string".into()));
    row.insert("group".into(), Value::Null);
    row.insert("description".into(), Value::Null);
    row.insert("tenant_id".into(), Value::I64(0));
    row.insert(
        "created_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row.insert(
        "updated_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row
}

fn log_row() -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::I64(1));
    row.insert("operator_id".into(), Value::I64(1));
    row.insert("operator_name".into(), Value::String("admin".into()));
    row.insert("operation_type".into(), Value::String("create".into()));
    row.insert("target_type".into(), Value::String("user".into()));
    row.insert("target_id".into(), Value::I64(1));
    row.insert("detail".into(), Value::Null);
    row.insert("ip".into(), Value::String("127.0.0.1".into()));
    row.insert("tenant_id".into(), Value::I64(0));
    row.insert(
        "created_at".into(),
        Value::String("2026-01-01T00:00:00+00:00".into()),
    );
    row
}

fn permission_code_row() -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("code".into(), Value::String("admin:user:list".into()));
    row
}

fn permission_tree_row() -> HashMap<String, Value> {
    let mut row = HashMap::new();
    row.insert("code".into(), Value::String("admin:user:list".into()));
    row.insert("name".into(), Value::String("用户列表".into()));
    row
}

/// 按成功路径路由 SQL（handler 集成测试用）
fn route_query(sql: &str) -> QueryRows {
    let sql = sql.to_ascii_lowercase();

    // COUNT(*) → 1
    if sql.contains("count(*)") {
        return vec![cnt_row(1)];
    }

    // SELECT id, username ... FROM users → user 行
    if sql.contains("select id, username") {
        return vec![user_row()];
    }

    // 查重检查（WHERE username/code/`key`）→ 空（不重复）
    if sql.contains("select id from") {
        if sql.contains("where username")
            || (sql.contains("where code")
                && (sql.contains("from menus") || sql.contains("from roles")))
            || sql.contains("where `key`")
        {
            return vec![];
        }
        // 子菜单检查 → 空（无子菜单）
        if sql.contains("where parent_id") {
            return vec![];
        }
        // 存在性（WHERE id）→ 命中
        return vec![HashMap::new()];
    }

    // SELECT id, is_builtin FROM roles WHERE id → {is_builtin: false}
    if sql.contains("select id, is_builtin from roles") {
        let mut row = HashMap::new();
        row.insert("is_builtin".into(), Value::Bool(false));
        return vec![row];
    }

    // SELECT code FROM permissions WHERE code → 命中
    if sql.contains("select code from permissions where code") {
        return vec![HashMap::new()];
    }

    // super_admin 检查 → 空（非 super_admin，但测试用 super_admin 直通）
    if sql.contains("from user_roles") && sql.contains("join roles") {
        return vec![];
    }

    // guard 权限检查 → 空（super_admin 直通，不会走到这里）
    if sql.contains("from user_roles") && sql.contains("join role_permissions") {
        return vec![];
    }

    // 实体行返回
    if sql.contains("from menus") {
        return vec![menu_row()];
    }
    if sql.contains("from configs") {
        return vec![config_row()];
    }
    if sql.contains("from operation_logs") {
        return vec![log_row()];
    }
    if sql.contains("from roles") {
        return vec![role_row()];
    }
    if sql.contains("from permissions") {
        // tree 查询有 name 列，list_all 只有 code 列
        if sql.contains("select code, name") {
            return vec![permission_tree_row()];
        }
        return vec![permission_code_row()];
    }
    if sql.contains("from users") {
        return vec![user_row()];
    }

    vec![]
}

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
        sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
        let rows = route_query(sql);
        Box::pin(async { Ok(rows) })
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
        sql: &'a str,
        _params: &'a [Value],
    ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
        let rows = route_query(sql);
        Box::pin(async { Ok(rows) })
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

#[tokio::test]
async fn test_plugin_creation() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["admin".to_string()]);
    let router = plugin.router();

    // 未注入用户上下文的请求应被 permission guard 拒绝（AUTH_REQUIRED → 401），
    // 证明守卫已挂载且路由可达，而非仅构造成功
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

#[tokio::test]
async fn test_router_built_with_21_endpoints() {
    let pool = make_mock_pool();
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    let router = plugin.router();

    // 21 个（方法, 路径）端点 = 15 条唯一路径（与 src/router.rs 一致）
    let endpoints: Vec<(&str, &str)> = vec![
        ("GET", "/api/admin/users"),
        ("POST", "/api/admin/users"),
        ("PUT", "/api/admin/users/u1"),
        ("DELETE", "/api/admin/users/u1"),
        ("PUT", "/api/admin/users/u1/status"),
        ("PUT", "/api/admin/users/u1/roles"),
        ("GET", "/api/admin/roles"),
        ("POST", "/api/admin/roles"),
        ("PUT", "/api/admin/roles/r1"),
        ("DELETE", "/api/admin/roles/r1"),
        ("PUT", "/api/admin/roles/r1/permissions"),
        ("GET", "/api/admin/permissions/tree"),
        ("GET", "/api/admin/menus/tree"),
        ("POST", "/api/admin/menus"),
        ("PUT", "/api/admin/menus/m1"),
        ("DELETE", "/api/admin/menus/m1"),
        ("GET", "/api/admin/configs"),
        ("PUT", "/api/admin/configs/cfg1"),
        ("DELETE", "/api/admin/configs/cfg1"),
        ("GET", "/api/admin/operation-logs"),
        ("GET", "/api/admin/dashboard"),
    ];
    assert_eq!(endpoints.len(), 21, "应注册 21 个端点");

    // super_admin 直通守卫后到达 axum 路由层：已注册路径对未注册方法返回 405，
    // 未注册路径返回 404 —— 以 PATCH 探测证明路径真实挂载，不触达 handler
    //（免受 mock 池业务语义干扰），去重后 15 条路径
    let paths = [
        "/api/admin/users",
        "/api/admin/users/u1",
        "/api/admin/users/u1/status",
        "/api/admin/users/u1/roles",
        "/api/admin/roles",
        "/api/admin/roles/r1",
        "/api/admin/roles/r1/permissions",
        "/api/admin/permissions/tree",
        "/api/admin/menus/tree",
        "/api/admin/menus",
        "/api/admin/menus/m1",
        "/api/admin/configs",
        "/api/admin/configs/cfg1",
        "/api/admin/operation-logs",
        "/api/admin/dashboard",
    ];
    for path in paths {
        let response = router
            .clone()
            .oneshot(make_super_admin_request("PATCH", path))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "端点路径未注册: {path}"
        );
    }
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
// =========================================================================
// Handler 层 POST/PUT/DELETE 集成测试
// =========================================================================

fn make_super_admin_request_with_body(
    method: &str,
    uri: &str,
    body: serde_json::Value,
) -> http::Request<axum::body::Body> {
    let user = DataScopeUserContext::new(1)
        .with_super(true)
        .with_roles(vec!["super_admin".to_string()]);
    let tenant = TenantContext::new(0, true, TenantResolveSource::Header);
    let bytes = serde_json::to_vec(&body).unwrap();
    http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .extension(user)
        .extension(tenant)
        .body(axum::body::Body::from(bytes))
        .unwrap()
}

#[tokio::test]
async fn test_create_user_endpoint_returns_201() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "POST",
        "/api/admin/users",
        serde_json::json!({"username":"newuser","password":"password123","email":"u@example.com"}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_create_user_short_password_returns_400() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "POST",
        "/api/admin/users",
        serde_json::json!({"username":"newuser","password":"short"}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_update_user_endpoint_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "PUT",
        "/api/admin/users/1",
        serde_json::json!({"email":"updated@example.com"}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_update_user_status_endpoint_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "PUT",
        "/api/admin/users/1/status",
        serde_json::json!({"status":"disabled"}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_delete_user_endpoint_returns_204() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request("DELETE", "/api/admin/users/1");
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_assign_user_roles_endpoint_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "PUT",
        "/api/admin/users/1/roles",
        serde_json::json!({"role_ids":[1,2]}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_create_role_endpoint_returns_201() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "POST",
        "/api/admin/roles",
        serde_json::json!({"name":"编辑者","code":"editor","description":"编辑角色"}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_create_role_empty_name_returns_400() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "POST",
        "/api/admin/roles",
        serde_json::json!({"name":"  ","code":"bad"}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_update_role_endpoint_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "PUT",
        "/api/admin/roles/1",
        serde_json::json!({"name":"改名角色"}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_delete_role_endpoint_returns_204() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request("DELETE", "/api/admin/roles/1");
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_assign_role_permissions_endpoint_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "PUT",
        "/api/admin/roles/1/permissions",
        serde_json::json!({"permission_codes":["admin:user:list"]}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_menu_tree_endpoint_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request("GET", "/api/admin/menus/tree");
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array(), "菜单树应返回数组");
}

#[tokio::test]
async fn test_create_menu_endpoint_returns_201() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "POST",
        "/api/admin/menus",
        serde_json::json!({"name":"用户管理","code":"user_mgr","path":"/admin/users","parent_id":0,"sort":1,"is_visible":true}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_update_menu_endpoint_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "PUT",
        "/api/admin/menus/1",
        serde_json::json!({"name":"改名菜单"}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_delete_menu_endpoint_returns_204() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request("DELETE", "/api/admin/menus/1");
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_list_configs_endpoint_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request("GET", "/api/admin/configs");
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array(), "配置列表应返回数组");
}

#[tokio::test]
async fn test_upsert_config_endpoint_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "PUT",
        "/api/admin/configs/site_name",
        serde_json::json!({"value":"MySite","value_type":"string","group":"system"}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_upsert_config_type_mismatch_returns_400() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request_with_body(
        "PUT",
        "/api/admin/configs/bad",
        serde_json::json!({"value":"not_bool","value_type":"boolean"}),
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_delete_config_endpoint_returns_204() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request("DELETE", "/api/admin/configs/site_name");
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn test_list_operation_logs_endpoint_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request("GET", "/api/admin/operation-logs");
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("items").is_some(), "日志列表应返回 items");
    assert!(json.get("total").is_some(), "日志列表应返回 total");
}

#[tokio::test]
async fn test_list_operation_logs_with_query_params_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request(
        "GET",
        "/api/admin/operation-logs?page=1&size=10&operation_type=create",
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_list_users_with_filter_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request(
        "GET",
        "/api/admin/users?username=admin&status=active&page=1&size=10",
    );
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_list_configs_with_group_returns_200() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_super_admin_request("GET", "/api/admin/configs?group=system");
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
// =========================================================================
// Guard middleware 非 super_admin 路径测试
// =========================================================================

fn make_non_super_admin_request(method: &str, uri: &str) -> http::Request<axum::body::Body> {
    let user = DataScopeUserContext::new(2).with_roles(vec!["editor".to_string()]);
    let tenant = TenantContext::new(0, false, TenantResolveSource::Header);
    http::Request::builder()
        .method(method)
        .uri(uri)
        .extension(user)
        .extension(tenant)
        .body(axum::body::Body::empty())
        .unwrap()
}

#[tokio::test]
async fn test_non_super_admin_without_permission_returns_403() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    // mock 对权限检查返回空行 → 无权限 → 403
    let req = make_non_super_admin_request("GET", "/api/admin/dashboard");
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_non_super_admin_users_list_returns_403() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    let req = make_non_super_admin_request("GET", "/api/admin/users");
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_guard_no_user_context_returns_401() {
    let pool = make_mock_pool();
    let router = AdminAddonPlugin::new(pool, vec!["super_admin".into()]).router();
    // 仅注入 TenantContext，不注入 DataScopeUserContext
    let tenant = TenantContext::new(0, true, TenantResolveSource::Header);
    let req = http::Request::builder()
        .method("GET")
        .uri("/api/admin/dashboard")
        .extension(tenant)
        .body(axum::body::Body::empty())
        .unwrap();
    let response = router.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
