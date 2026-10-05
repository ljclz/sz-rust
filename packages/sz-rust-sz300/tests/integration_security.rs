// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P3-3.2 安全头端到端集成测试
//!
//! 通过完整路由（含所有中间件）验证安全头注入。
//! 需要真实 MySQL 连接。

#![cfg(feature = "v18-security-headers")]

mod common;

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

use common::assertions;
use common::{ensure_mysql, make_router, make_state};

#[tokio::test]
async fn test_e2e_health_endpoint_has_security_headers() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => {
            eprintln!("⚠️ MySQL 不可达，跳过");
            return;
        }
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
    assertions::assert_all_security_headers(response.headers());
}

#[tokio::test]
async fn test_e2e_metrics_endpoint_has_security_headers() {
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

    assertions::assert_all_security_headers(response.headers());
}

#[tokio::test]
async fn test_e2e_csp_header_contains_directives() {
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

    let csp = response
        .headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(csp.contains("default-src"), "CSP 应含 default-src");
}

#[tokio::test]
async fn test_e2e_hsts_header_max_age() {
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

    let hsts = response
        .headers()
        .get("strict-transport-security")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(hsts.contains("31536000"), "HSTS max-age 应为 31536000");
}

#[tokio::test]
async fn test_e2e_x_frame_options_is_deny() {
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

    let x_frame = response
        .headers()
        .get("x-frame-options")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        x_frame.eq_ignore_ascii_case("deny") || x_frame.eq_ignore_ascii_case("sameorigin"),
        "X-Frame-Options 应为 DENY/SAMEORIGIN"
    );
}

#[tokio::test]
async fn test_e2e_security_headers_on_error_response() {
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_state(pool);
    let app = make_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assertions::assert_all_security_headers(response.headers());
}
