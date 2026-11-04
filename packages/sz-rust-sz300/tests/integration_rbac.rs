// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P3-3.4 RBAC 端到端集成测试
//!
//! 通过完整路由验证 RBAC 权限控制。
//! 需要真实 MySQL 连接。

#![cfg(feature = "v18-rbac")]

mod common;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use common::{ensure_mysql, make_router, make_state};

#[tokio::test]
async fn test_e2e_rbac_unauthenticated_request_rejected() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/merchant/list")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"page": 1, "page_size": 10}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    assert!(
        status == axum::http::StatusCode::UNAUTHORIZED
            || status == axum::http::StatusCode::FORBIDDEN,
        "无 JWT 的请求应返回 401/403, 实际: {status}"
    );
}

#[tokio::test]
async fn test_e2e_rbac_public_endpoint_accessible() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn test_e2e_rbac_health_ready_accessible() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn test_e2e_rbac_metrics_accessible() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn test_e2e_rbac_invalid_token_rejected() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/merchant/list")
                .method("POST")
                .header("content-type", "application/json")
                .header("authorization", "Bearer invalid.token.here")
                .body(Body::from(r#"{"page": 1, "page_size": 10}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    assert!(
        status == axum::http::StatusCode::UNAUTHORIZED
            || status == axum::http::StatusCode::FORBIDDEN,
        "无效 JWT 应返回 401/403, 实际: {status}"
    );
}

#[tokio::test]
async fn test_e2e_rbac_graphql_with_rbac_requires_auth() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/graphql")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query": "{ __typename }"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    assert!(
        status == axum::http::StatusCode::UNAUTHORIZED
            || status == axum::http::StatusCode::FORBIDDEN,
        "RBAC 模式下 GraphQL 无 JWT 应返回 401/403, 实际: {status}"
    );
}

#[tokio::test]
async fn test_e2e_rbac_api_docs_accessible() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api-docs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}
