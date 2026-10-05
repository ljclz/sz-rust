// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! axum Layer 适配 — 将安全头注入到每个 HTTP 响应
//!
//! v1.8.0 新增：提供 `security_headers_layer` 函数，返回 tower Layer，
//! 可直接挂载到 `Router::layer()`。

use std::sync::Arc;

use axum::response::Response;
use tower::util::MapResponseLayer;

use crate::error::SecurityHeaderError;
use crate::headers::SecurityHeadersConfig;

/// 创建安全头 axum 中间件 layer
///
/// 返回一个 `MapResponseLayer`，可直接用于 `Router::layer()`。
/// 每个响应自动注入 CSP / HSTS / X-Frame-Options / X-Content-Type-Options / Referrer-Policy 头。
///
/// # 错误
///
/// 配置无效（HSTS max-age < 31536000 或 CSP 为空）时返回 `SecurityHeaderError`。
pub fn security_headers_layer(
    config: SecurityHeadersConfig,
) -> Result<
    MapResponseLayer<impl FnMut(Response) -> Response + Clone + Send + Sync + 'static>,
    SecurityHeaderError,
> {
    config.validate()?;
    let config = Arc::new(config);
    let f = move |mut response: Response| -> Response {
        for (key, value) in config.headers() {
            if let (Ok(name), Ok(val)) = (
                http::HeaderName::from_bytes(key.as_bytes()),
                http::HeaderValue::from_str(&value),
            ) {
                response.headers_mut().insert(name, val);
            }
        }
        response
    };
    Ok(MapResponseLayer::new(f))
}

/// 使用默认配置创建安全头中间件 layer
pub fn security_headers_layer_with_defaults() -> Result<
    MapResponseLayer<impl FnMut(Response) -> Response + Clone + Send + Sync + 'static>,
    SecurityHeaderError,
> {
    security_headers_layer(SecurityHeadersConfig::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn handler() -> &'static str {
        "ok"
    }

    fn make_app() -> Router {
        let layer = security_headers_layer_with_defaults().unwrap();
        Router::new().route("/test", get(handler)).layer(layer)
    }

    fn make_request() -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .method("GET")
            .uri("/test")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn test_layer_injects_csp() {
        let app = make_app();
        let res = app.oneshot(make_request()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let csp = res.headers().get("content-security-policy").unwrap();
        assert!(csp.to_str().unwrap().contains("default-src 'self'"));
    }

    #[tokio::test]
    async fn test_layer_injects_hsts() {
        let app = make_app();
        let res = app.oneshot(make_request()).await.unwrap();
        let hsts = res.headers().get("strict-transport-security").unwrap();
        assert!(hsts.to_str().unwrap().contains("max-age=31536000"));
    }

    #[tokio::test]
    async fn test_layer_injects_x_frame_options() {
        let app = make_app();
        let res = app.oneshot(make_request()).await.unwrap();
        let xfo = res.headers().get("x-frame-options").unwrap();
        assert_eq!(xfo.to_str().unwrap(), "DENY");
    }

    #[tokio::test]
    async fn test_layer_injects_x_content_type_options() {
        let app = make_app();
        let res = app.oneshot(make_request()).await.unwrap();
        let xcto = res.headers().get("x-content-type-options").unwrap();
        assert_eq!(xcto.to_str().unwrap(), "nosniff");
    }

    #[tokio::test]
    async fn test_layer_injects_referrer_policy() {
        let app = make_app();
        let res = app.oneshot(make_request()).await.unwrap();
        let rp = res.headers().get("referrer-policy").unwrap();
        assert_eq!(rp.to_str().unwrap(), "strict-origin-when-cross-origin");
    }

    #[tokio::test]
    async fn test_layer_invalid_config_rejected() {
        let bad_config = SecurityHeadersConfig {
            csp: "".to_string(),
            ..Default::default()
        };
        assert!(security_headers_layer(bad_config).is_err());
    }

    #[tokio::test]
    async fn test_layer_custom_config() {
        let custom = SecurityHeadersConfig {
            csp: "default-src 'none'".to_string(),
            hsts_max_age: 63072000,
            hsts_preload: false,
            ..Default::default()
        };
        let layer = security_headers_layer(custom).unwrap();
        let app = Router::new().route("/test", get(handler)).layer(layer);
        let res = app.oneshot(make_request()).await.unwrap();
        let csp = res.headers().get("content-security-policy").unwrap();
        assert_eq!(csp.to_str().unwrap(), "default-src 'none'");
        let hsts = res.headers().get("strict-transport-security").unwrap();
        assert!(hsts.to_str().unwrap().contains("max-age=63072000"));
        assert!(!hsts.to_str().unwrap().contains("preload"));
    }

    #[tokio::test]
    async fn test_layer_preserves_response_body() {
        let app = make_app();
        let res = app.oneshot(make_request()).await.unwrap();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), b"ok");
    }
}
