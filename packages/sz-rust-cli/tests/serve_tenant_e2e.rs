// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

//! serve 命令 tenant_middleware 接线验证测试

use axum::Router;
use http::StatusCode;
use sz_rust_cli::cmd::serve::build_router_with_tenant;
use tower::ServiceExt;

fn base_router() -> Router {
    Router::new().route("/", axum::routing::get(|| async { "OK" }))
}

#[tokio::test]
async fn serve_with_tenant_routes_request_through_tenant_middleware() {
    let router = build_router_with_tenant(base_router(), true);
    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/")
                .header("X-Tenant-Id", "42")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn serve_without_tenant_header_returns_400() {
    let router = build_router_with_tenant(base_router(), true);
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
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn serve_with_invalid_tenant_header_returns_400() {
    let router = build_router_with_tenant(base_router(), true);
    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/")
                .header("X-Tenant-Id", "abc")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn serve_without_with_tenant_flag_behavior_unchanged() {
    let router = build_router_with_tenant(base_router(), false);
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
