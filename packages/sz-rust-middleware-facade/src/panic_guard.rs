// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Panic 兜底中间件 — 用 `catch_unwind` 捕获下游 panic，返回 500 + 记录堆栈
//!
//! 防止单个请求处理中的 panic 导致整个 axum 服务进程崩溃。
//! 捕获到 panic 后：
//! 1. 通过 `tracing::error!` 记录 panic 信息（可选附 backtrace）；
//! 2. 返回 `500` JSON 响应 `{"error":"internal server error","request_id":"..."}`。
//!
//! # request_id 来源
//!
//! 优先从请求头 `x-request-id` 提取；缺失时生成 UUID v4。
//!
//! # 示例
//!
//! ```
//! use sz_rust_middleware_facade::panic_guard::{panic_guard_middleware, PanicGuardConfig};
//! use axum::routing::get;
//! use axum::Router;
//!
//! let app = Router::new()
//!     .route("/", get(|| async { "ok" }))
//!     .layer(axum::middleware::from_fn_with_state(
//!         PanicGuardConfig::default(),
//!         panic_guard_middleware,
//!     ));
//! ```

use futures::FutureExt;
use std::panic::AssertUnwindSafe;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

/// Panic 兜底配置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanicGuardConfig {
    /// 是否在日志中附 backtrace（默认 true）
    pub include_backtrace: bool,
}

impl Default for PanicGuardConfig {
    fn default() -> Self {
        Self {
            include_backtrace: true,
        }
    }
}

/// 提取或生成 request_id
fn resolve_request_id(req: &Request) -> String {
    if let Some(val) = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
    {
        if !val.is_empty() {
            return val.to_string();
        }
    }
    uuid::Uuid::new_v4().to_string()
}

/// 构造 500 JSON 错误响应
fn internal_error_response(request_id: &str) -> Response {
    let body = format!(
        r#"{{"error":"internal server error","request_id":"{}"}}"#,
        request_id
    );
    Response::builder()
        .status(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        .header("content-type", "application/json")
        .header("x-request-id", request_id)
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| Response::new(axum::body::Body::empty()))
}

/// 记录 panic 信息到 tracing
fn log_panic(payload: &Box<dyn std::any::Any + Send>, request_id: &str, include_backtrace: bool) {
    let msg = payload
        .downcast_ref::<&'static str>()
        .copied()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_string());

    if include_backtrace {
        let bt = std::backtrace::Backtrace::capture();
        tracing::error!(
            request_id = %request_id,
            panic = %msg,
            backtrace = %bt,
            "panic captured by panic_guard_middleware"
        );
    } else {
        tracing::error!(
            request_id = %request_id,
            panic = %msg,
            "panic captured by panic_guard_middleware"
        );
    }
}

/// Panic 兜底中间件
///
/// 用 `FutureExt::catch_unwind` 包裹下游 `next.run(req)`，捕获 panic 后返回 500。
pub async fn panic_guard_middleware(
    axum::extract::State(config): axum::extract::State<PanicGuardConfig>,
    req: Request,
    next: Next,
) -> Response {
    let request_id = resolve_request_id(&req);

    let result = AssertUnwindSafe(next.run(req)).catch_unwind().await;

    match result {
        Ok(response) => response,
        Err(payload) => {
            log_panic(&payload, &request_id, config.include_backtrace);
            internal_error_response(&request_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use tower::ServiceExt;

    /// panic handler — 显式返回 `&'static str` 避免 edition 2024 never type fallback
    async fn panic_handler() -> &'static str {
        panic!("boom");
    }

    #[test]
    fn test_default_config() {
        let cfg = PanicGuardConfig::default();
        assert!(cfg.include_backtrace);
    }

    #[tokio::test]
    async fn test_normal_request_passes_through() {
        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                PanicGuardConfig::default(),
                panic_guard_middleware,
            ));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_panic_returns_500() {
        let app = axum::Router::new().route("/", get(panic_handler)).layer(
            axum::middleware::from_fn_with_state(
                PanicGuardConfig::default(),
                panic_guard_middleware,
            ),
        );

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json"
        );
    }

    #[tokio::test]
    async fn test_panic_preserves_request_id_header() {
        let app = axum::Router::new().route("/", get(panic_handler)).layer(
            axum::middleware::from_fn_with_state(
                PanicGuardConfig::default(),
                panic_guard_middleware,
            ),
        );

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .header("x-request-id", "test-id-123")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(resp.headers().get("x-request-id").unwrap(), "test-id-123");
    }

    #[tokio::test]
    async fn test_panic_response_body_contains_error_and_request_id() {
        let app = axum::Router::new().route("/", get(panic_handler)).layer(
            axum::middleware::from_fn_with_state(
                PanicGuardConfig::default(),
                panic_guard_middleware,
            ),
        );

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .header("x-request-id", "abc-456")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert!(body_str.contains(r#""error":"internal server error""#));
        assert!(body_str.contains(r#""request_id":"abc-456""#));
    }

    #[tokio::test]
    async fn test_panic_without_request_id_generates_uuid() {
        let app = axum::Router::new().route("/", get(panic_handler)).layer(
            axum::middleware::from_fn_with_state(
                PanicGuardConfig::default(),
                panic_guard_middleware,
            ),
        );

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        let rid = resp
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            uuid::Uuid::parse_str(rid).is_ok(),
            "应为合法 UUID v4: {rid}"
        );
    }

    #[tokio::test]
    async fn test_config_include_backtrace_false() {
        let config = PanicGuardConfig {
            include_backtrace: false,
        };

        let app = axum::Router::new().route("/", get(panic_handler)).layer(
            axum::middleware::from_fn_with_state(config, panic_guard_middleware),
        );

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_normal_request_with_request_id_header_passes() {
        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                PanicGuardConfig::default(),
                panic_guard_middleware,
            ));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .header("x-request-id", "trace-789")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[test]
    fn test_resolve_request_id_from_header() {
        let req = axum::http::Request::builder()
            .header("x-request-id", "hdr-id")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(resolve_request_id(&req), "hdr-id");
    }

    #[test]
    fn test_resolve_request_id_empty_header_generates_uuid() {
        let req = axum::http::Request::builder()
            .header("x-request-id", "")
            .body(axum::body::Body::empty())
            .unwrap();
        let rid = resolve_request_id(&req);
        assert!(uuid::Uuid::parse_str(&rid).is_ok());
    }

    #[test]
    fn test_resolve_request_id_missing_header_generates_uuid() {
        let req: Request = axum::http::Request::builder()
            .body(axum::body::Body::empty())
            .unwrap();
        let rid = resolve_request_id(&req);
        assert!(uuid::Uuid::parse_str(&rid).is_ok());
    }

    #[tokio::test]
    async fn test_internal_error_response_format() {
        let resp = internal_error_response("rid-xyz");
        assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json"
        );
        assert_eq!(resp.headers().get("x-request-id").unwrap(), "rid-xyz");
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
        assert_eq!(
            body_str,
            r#"{"error":"internal server error","request_id":"rid-xyz"}"#
        );
    }
}
