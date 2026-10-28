// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! RBAC 权限中间件集成测试（v1.8.0 P1-1.1）
//!
//! 验证 `v18-rbac` feature gate 下：
//! 1. 角色继承链正确传播权限
//! 2. RbacEngine 权限矩阵覆盖 admin/merchant:write/merchant:read/device
//! 3. RbacGuard 中间件正确 allow/deny 请求
//! 4. 无令牌 / 无效令牌返回 401

#![cfg(feature = "v18-rbac")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::post;
use axum::Router;
use http_body_util::BodyExt;
use sz_rust_auth_facade::{PermissionDecision, RbacEngine};
use sz_rust_sz300::rbac::{guard::rbac_guard, roles};
use tower::ServiceExt;

// ============================================================================
// 测试辅助
// ============================================================================

const TEST_SECRET: &str = "test-rbac-secret-2026";

fn init_test_auth() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        sz_rust_sz300::services::auth_service::init_auth_test_only(TEST_SECRET);
    });
}

fn issue_token(username: &str, user_id: i64, roles: Vec<&str>) -> String {
    init_test_auth();
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + 3600;
    let claims = sz_rust_core::orm::jwt::JwtClaims::new(username, exp)
        .with_issuer("sz300-test")
        .with_roles(roles.into_iter().map(String::from).collect())
        .with_user_id(user_id);
    sz_rust_core::orm::jwt::JwtEncoder::new(TEST_SECRET)
        .encode(&claims)
        .unwrap()
}

fn bearer_request(token: &str) -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri("/protected")
        .header("Authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap()
}

fn no_token_request() -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri("/protected")
        .body(Body::empty())
        .unwrap()
}

async fn ok_handler() -> &'static str {
    "ok"
}

fn build_router(engine: Arc<RbacEngine>, resource: sz_rust_auth_facade::Resource) -> Router {
    Router::new()
        .route("/protected", post(ok_handler))
        .layer(middleware::from_fn_with_state(
            (engine, resource),
            rbac_guard,
        ))
}

async fn fetch_body(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

// ============================================================================
// RbacEngine 权限矩阵测试
// ============================================================================

#[tokio::test]
async fn test_admin_has_all_permissions() {
    let engine = roles::init_rbac_engine();
    engine.assign_role("1", roles::role::admin());

    for op in ["create", "read", "update", "delete"] {
        assert_eq!(
            engine.check("1", &roles::perm::merchant(op)).await.unwrap(),
            PermissionDecision::Allow
        );
        assert_eq!(
            engine.check("1", &roles::perm::product(op)).await.unwrap(),
            PermissionDecision::Allow
        );
    }
    assert_eq!(
        engine
            .check("1", &roles::perm::device("bind"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
    assert_eq!(
        engine
            .check("1", &roles::perm::order("create"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
    assert_eq!(
        engine
            .check("1", &roles::perm::file("upload"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
}

#[tokio::test]
async fn test_merchant_write_has_crud() {
    let engine = roles::init_rbac_engine();
    engine.assign_role("2", roles::role::merchant_write());

    for op in ["create", "read", "update", "delete"] {
        assert_eq!(
            engine.check("2", &roles::perm::merchant(op)).await.unwrap(),
            PermissionDecision::Allow
        );
        assert_eq!(
            engine.check("2", &roles::perm::product(op)).await.unwrap(),
            PermissionDecision::Allow
        );
    }
    assert_eq!(
        engine
            .check("2", &roles::perm::order("create"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
    assert_eq!(
        engine
            .check("2", &roles::perm::file("upload"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
}

#[tokio::test]
async fn test_merchant_read_only_has_read() {
    let engine = roles::init_rbac_engine();
    engine.assign_role("3", roles::role::merchant_read());

    assert_eq!(
        engine
            .check("3", &roles::perm::merchant("read"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
    assert_eq!(
        engine
            .check("3", &roles::perm::product("read"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
    assert_eq!(
        engine
            .check("3", &roles::perm::merchant("create"))
            .await
            .unwrap(),
        PermissionDecision::Deny
    );
    assert_eq!(
        engine
            .check("3", &roles::perm::product("delete"))
            .await
            .unwrap(),
        PermissionDecision::Deny
    );
}

#[tokio::test]
async fn test_device_role_has_device_permissions() {
    let engine = roles::init_rbac_engine();
    engine.assign_role("4", roles::role::device());

    for op in ["bind", "unbind", "ota", "status_report"] {
        assert_eq!(
            engine.check("4", &roles::perm::device(op)).await.unwrap(),
            PermissionDecision::Allow
        );
    }
    assert_eq!(
        engine
            .check("4", &roles::perm::merchant("read"))
            .await
            .unwrap(),
        PermissionDecision::Deny
    );
}

#[tokio::test]
async fn test_role_inheritance_admin_inherits_all() {
    let engine = roles::init_rbac_engine();
    engine.assign_role("5", roles::role::admin());

    // admin 继承 merchant:write → merchant:read，应能 read
    assert_eq!(
        engine
            .check("5", &roles::perm::merchant("read"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
    // admin 继承 device，应能 bind
    assert_eq!(
        engine
            .check("5", &roles::perm::device("bind"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
}

#[tokio::test]
async fn test_no_role_denied() {
    let engine = roles::init_rbac_engine();
    // 未分配任何角色的用户
    assert_eq!(
        engine
            .check("999", &roles::perm::merchant("read"))
            .await
            .unwrap(),
        PermissionDecision::Deny
    );
}

#[tokio::test]
async fn test_multi_role_user() {
    let engine = roles::init_rbac_engine();
    engine.assign_role("10", roles::role::merchant_read());
    engine.assign_role("10", roles::role::device());

    // merchant:read 权限
    assert_eq!(
        engine
            .check("10", &roles::perm::merchant("read"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
    // device 权限
    assert_eq!(
        engine
            .check("10", &roles::perm::device("bind"))
            .await
            .unwrap(),
        PermissionDecision::Allow
    );
    // 无 merchant:write 权限
    assert_eq!(
        engine
            .check("10", &roles::perm::merchant("create"))
            .await
            .unwrap(),
        PermissionDecision::Deny
    );
}

// ============================================================================
// RbacGuard 中间件测试
// ============================================================================

#[tokio::test]
async fn test_guard_no_token_returns_401() {
    let engine = Arc::new(roles::init_rbac_engine());
    let router = build_router(engine, roles::perm::merchant("read"));

    let resp = router.oneshot(no_token_request()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = fetch_body(resp).await;
    assert!(body.contains("未提供认证令牌"));
}

#[tokio::test]
async fn test_guard_invalid_token_returns_401() {
    let engine = Arc::new(roles::init_rbac_engine());
    let router = build_router(engine, roles::perm::merchant("read"));

    let resp = router
        .oneshot(bearer_request("invalid-token-xyz"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_guard_allow_with_permission() {
    let engine = Arc::new(roles::init_rbac_engine());
    engine.assign_role("100", roles::role::merchant_read());

    let token = issue_token("testuser", 100, vec!["merchant:read"]);
    let router = build_router(engine, roles::perm::merchant("read"));

    let resp = router.oneshot(bearer_request(&token)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = fetch_body(resp).await;
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn test_guard_deny_without_permission() {
    let engine = Arc::new(roles::init_rbac_engine());
    engine.assign_role("101", roles::role::merchant_read());

    let token = issue_token("testuser", 101, vec!["merchant:read"]);
    let router = build_router(engine, roles::perm::merchant("create"));

    let resp = router.oneshot(bearer_request(&token)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = fetch_body(resp).await;
    assert!(body.contains("无权访问"));
}

#[tokio::test]
async fn test_guard_admin_allow_all() {
    let engine = Arc::new(roles::init_rbac_engine());
    engine.assign_role("102", roles::role::admin());

    let token = issue_token("admin_user", 102, vec!["admin"]);
    let router = build_router(engine, roles::perm::device("bind"));

    let resp = router.oneshot(bearer_request(&token)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
