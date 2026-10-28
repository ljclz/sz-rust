// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! serve 命令健康检查接线验证测试
//!
//! 验证：
//! - CLI 参数 `--health` / `--no-health` 正确解析到 ServeArgs
//! - `default_health_router()` merge 后 /health/ 和 /health/ready 端点可达
//! - 禁用健康检查时不挂载端点

use clap::Parser;
use http::StatusCode;
use http_body_util::BodyExt;
use sz_rust_cli::{Cli, CliCommand};
use tower::ServiceExt;

/// 默认 `serve` 解析后 health=true
#[test]
fn test_parse_serve_health_default_enabled() {
    let cli = Cli::parse_from(["sz-rust", "serve"]);
    match cli.command {
        Some(CliCommand::Serve {
            health, no_health, ..
        }) => {
            assert!(health, "health 默认应为 true");
            assert!(!no_health, "no_health 默认应为 false");
        }
        _ => panic!("expected Serve command"),
    }
}

/// `serve --no-health` 解析后 no_health=true
#[test]
fn test_parse_serve_no_health_flag() {
    let cli = Cli::parse_from(["sz-rust", "serve", "--no-health"]);
    match cli.command {
        Some(CliCommand::Serve { no_health, .. }) => {
            assert!(no_health, "no_health 应为 true");
        }
        _ => panic!("expected Serve command"),
    }
}

/// `serve --health` 显式启用
#[test]
fn test_parse_serve_health_explicit() {
    let cli = Cli::parse_from(["sz-rust", "serve", "--health"]);
    match cli.command {
        Some(CliCommand::Serve {
            health, no_health, ..
        }) => {
            assert!(health, "health 应为 true");
            assert!(!no_health, "no_health 应为 false");
        }
        _ => panic!("expected Serve command"),
    }
}

/// `default_health_router()` merge 后 GET /health/ 返回 200（liveness）
#[tokio::test]
async fn test_health_liveness_endpoint_reachable() {
    let base = axum::Router::new().route("/", axum::routing::get(|| async { "SZ-Rust" }));
    let app = base.merge(sz_rust_core::health::default_health_router());

    let response = app
        .oneshot(
            http::Request::builder()
                .method("GET")
                .uri("/health/")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 1, "liveness 应返回 code=1");
    assert_eq!(json["msg"], "ok");
}

/// `default_health_router()` merge 后 GET /health/ready 返回 200（readiness，无子检查时通过）
#[tokio::test]
async fn test_health_readiness_endpoint_reachable() {
    let base = axum::Router::new().route("/", axum::routing::get(|| async { "SZ-Rust" }));
    let app = base.merge(sz_rust_core::health::default_health_router());

    let response = app
        .oneshot(
            http::Request::builder()
                .method("GET")
                .uri("/health/ready")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 1, "readiness 无子检查时应返回 code=1");
}

/// 禁用健康检查时（不 merge），/health/ 返回 404
#[tokio::test]
async fn test_health_disabled_endpoint_not_found() {
    let app: axum::Router =
        axum::Router::new().route("/", axum::routing::get(|| async { "SZ-Rust" }));

    let response = app
        .oneshot(
            http::Request::builder()
                .method("GET")
                .uri("/health/")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
