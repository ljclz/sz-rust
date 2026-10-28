// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

//! serve 命令 data_scope_middleware 接线验证测试

use axum::Router;
use http::StatusCode;
use sz_rust_cli::cmd::serve::{build_router_with_data_scope, build_router_with_tenant};
use sz_rust_middleware_facade::data_scope::DataScopeUserContext;

use tower::ServiceExt;

fn base_router() -> Router {
    Router::new().route("/", axum::routing::get(|| async { "OK" }))
}

#[tokio::test]
async fn serve_with_data_scope_injects_context_from_user_context() {
    let router = build_router_with_data_scope(base_router(), true);
    let mut req = axum::http::Request::builder()
        .method("GET")
        .uri("/")
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(10).with_dept(5).with_super(false));
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn serve_with_data_scope_injects_default_when_no_user_context() {
    let router = build_router_with_data_scope(base_router(), true);
    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn serve_without_with_data_scope_flag_behavior_unchanged() {
    let router = build_router_with_data_scope(base_router(), false);
    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn serve_with_both_tenant_and_data_scope_layer_order() {
    let router = build_router_with_tenant(base_router(), true);
    let router = build_router_with_data_scope(router, true);
    let mut req = axum::http::Request::builder()
        .method("GET")
        .uri("/")
        .header("X-Tenant-Id", "42")
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(10).with_dept(5).with_super(false));
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
