// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 安全头中间件集成测试（v1.8.0 P1-1.3）
//!
//! 验证 `v18-security-headers` feature gate 下：
//! 1. 安全头配置从环境变量正确加载
//! 2. SecurityHeadersLayer 注入所有安全响应头
//! 3. 自定义配置正确覆盖默认值
//!
//! 仅在 `v18-security-headers` feature 启用时编译运行。

#![cfg(feature = "v18-security-headers")]

use axum::body::Body;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use http_body_util::BodyExt;
use sz_rust_security_headers::{security_headers_layer, SecurityHeadersConfig, XFrameOptions};
use sz_rust_sz300::config;
use tower::ServiceExt;

/// env 测试互斥锁 — 确保所有修改环境变量的测试串行运行
static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

async fn ok_handler() -> &'static str {
    "ok"
}

fn make_request(uri: &str) -> axum::http::Request<Body> {
    axum::http::Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

#[test]
fn test_security_headers_config_defaults() {
    let _guard = ENV_TEST_LOCK.lock().unwrap();
    std::env::remove_var("SZ300_CSP");
    std::env::remove_var("SZ300_HSTS_MAX_AGE");
    std::env::remove_var("SZ300_HSTS_PRELOAD");
    std::env::remove_var("SZ300_X_FRAME_OPTIONS");

    let cfg = config::security_headers_config();
    assert!(cfg.csp.contains("default-src 'self'"));
    assert_eq!(cfg.hsts_max_age, 31536000);
    assert!(cfg.hsts_preload);
    assert_eq!(cfg.x_frame_options, XFrameOptions::Deny);
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_security_headers_config_custom_csp() {
    let _guard = ENV_TEST_LOCK.lock().unwrap();
    std::env::set_var("SZ300_CSP", "default-src 'none'");

    let cfg = config::security_headers_config();
    assert_eq!(cfg.csp, "default-src 'none'");
    assert!(cfg.validate().is_ok());

    std::env::remove_var("SZ300_CSP");
}

#[test]
fn test_security_headers_config_custom_hsts() {
    let _guard = ENV_TEST_LOCK.lock().unwrap();
    std::env::set_var("SZ300_HSTS_MAX_AGE", "63072000");
    std::env::set_var("SZ300_HSTS_PRELOAD", "false");

    let cfg = config::security_headers_config();
    assert_eq!(cfg.hsts_max_age, 63072000);
    assert!(!cfg.hsts_preload);
    assert!(cfg.validate().is_ok());

    std::env::remove_var("SZ300_HSTS_MAX_AGE");
    std::env::remove_var("SZ300_HSTS_PRELOAD");
}

#[test]
fn test_security_headers_config_x_frame_sameorigin() {
    let _guard = ENV_TEST_LOCK.lock().unwrap();
    std::env::set_var("SZ300_X_FRAME_OPTIONS", "SAMEORIGIN");

    let cfg = config::security_headers_config();
    assert_eq!(cfg.x_frame_options, XFrameOptions::SameOrigin);

    std::env::remove_var("SZ300_X_FRAME_OPTIONS");
}

#[tokio::test]
async fn test_layer_injects_all_security_headers() {
    let layer = security_headers_layer(SecurityHeadersConfig::default()).unwrap();
    let app: Router = Router::new().route("/test", get(ok_handler)).layer(layer);

    let res = app.oneshot(make_request("/test")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // CSP
    let csp = res.headers().get("content-security-policy").unwrap();
    assert!(csp.to_str().unwrap().contains("default-src 'self'"));

    // HSTS
    let hsts = res.headers().get("strict-transport-security").unwrap();
    assert!(hsts.to_str().unwrap().contains("max-age=31536000"));
    assert!(hsts.to_str().unwrap().contains("preload"));

    // X-Frame-Options
    let xfo = res.headers().get("x-frame-options").unwrap();
    assert_eq!(xfo.to_str().unwrap(), "DENY");

    // X-Content-Type-Options
    let xcto = res.headers().get("x-content-type-options").unwrap();
    assert_eq!(xcto.to_str().unwrap(), "nosniff");

    // Referrer-Policy
    let rp = res.headers().get("referrer-policy").unwrap();
    assert_eq!(rp.to_str().unwrap(), "strict-origin-when-cross-origin");
}

#[tokio::test]
async fn test_layer_custom_config_overrides_defaults() {
    let custom = SecurityHeadersConfig {
        csp: "default-src 'none'".to_string(),
        hsts_max_age: 63072000,
        hsts_preload: false,
        x_frame_options: XFrameOptions::SameOrigin,
        ..Default::default()
    };
    let layer = security_headers_layer(custom).unwrap();
    let app: Router = Router::new().route("/test", get(ok_handler)).layer(layer);

    let res = app.oneshot(make_request("/test")).await.unwrap();

    let csp = res.headers().get("content-security-policy").unwrap();
    assert_eq!(csp.to_str().unwrap(), "default-src 'none'");

    let hsts = res.headers().get("strict-transport-security").unwrap();
    assert!(hsts.to_str().unwrap().contains("max-age=63072000"));
    assert!(!hsts.to_str().unwrap().contains("preload"));

    let xfo = res.headers().get("x-frame-options").unwrap();
    assert_eq!(xfo.to_str().unwrap(), "SAMEORIGIN");
}

#[tokio::test]
async fn test_layer_preserves_response_body_and_status() {
    let layer = security_headers_layer(SecurityHeadersConfig::default()).unwrap();
    let app: Router = Router::new().route("/test", get(ok_handler)).layer(layer);

    let res = app.oneshot(make_request("/test")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"ok");
}

#[test]
fn test_invalid_config_rejected_by_layer() {
    let bad_config = SecurityHeadersConfig {
        csp: "".to_string(),
        ..Default::default()
    };
    assert!(security_headers_layer(bad_config).is_err());

    let bad_hsts = SecurityHeadersConfig {
        hsts_max_age: 100,
        ..Default::default()
    };
    assert!(security_headers_layer(bad_hsts).is_err());
}
