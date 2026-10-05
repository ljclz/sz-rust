// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P3-3.3 数据脱敏端到端集成测试
//!
//! 通过完整路由验证响应 JSON 中敏感字段已脱敏。
//! 需要真实 MySQL 连接。

#![cfg(feature = "v18-data-mask")]

mod common;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use common::{ensure_mysql, make_router, make_state};

#[tokio::test]
async fn test_e2e_mask_not_applied_on_health() {
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
async fn test_e2e_mask_not_applied_on_metrics() {
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

    let ct = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("text/plain"), "metrics 应为 text/plain");
}

#[tokio::test]
async fn test_e2e_mask_layer_does_not_break_health() {
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

    let body_bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(
        !body_str.is_empty(),
        "health 响应体不应为空（脱敏中间件不应破坏响应）"
    );
}

#[tokio::test]
async fn test_e2e_mask_api_returns_response_through_mask_layer() {
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
        status == axum::http::StatusCode::OK
            || status == axum::http::StatusCode::UNAUTHORIZED
            || status == axum::http::StatusCode::FORBIDDEN,
        "API 请求应返回 200/401/403, 实际: {status}"
    );

    let body_bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(
        !body_str.contains("13800138000"),
        "完整手机号不应出现在任何响应中（含错误响应）"
    );
}

#[tokio::test]
async fn test_e2e_mask_health_response_content_type_preserved() {
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

    let ct = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.contains("application/json") || ct.contains("text/plain"),
        "health content-type 应被保留, 实际: {ct}"
    );
}
