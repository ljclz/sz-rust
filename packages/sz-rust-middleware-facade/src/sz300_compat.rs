// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! sz300 兼容层 — 对齐 sz300 自研 auth_middleware/log_middleware/trace_middleware 签名
//!
//! 提供 axum 0.8 中间件适配函数，便于 sz300 渐进替换自研中间件：
//!
//! - [`auth_middleware_compat`]：校验 Bearer Token，注入到 request extension
//! - [`log_middleware_compat`]：记录请求方法 + 路径 + 状态码 + 耗时
//! - [`trace_middleware_compat`]：注入 trace_id（从 `x-request-id` header 或生成 UUID）
//!
//! ## 设计
//!
//! 每个适配函数返回 [`axum::middleware::FromFnLayer`]，可直接通过 `.layer(...)` 挂载到
//! `Router`。中间件内部逻辑对齐 sz300 自研实现，但复用 axum 0.8 / `tracing` / `uuid`
//! 生态，避免重复造轮子。
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_middleware_facade::sz300_compat::{
//!     auth_middleware_compat, log_middleware_compat, trace_middleware_compat,
//! };
//! use axum::Router;
//!
//! let app: Router = Router::new()
//!     .route("/api", axum::routing::get(|| async { "ok" }))
//!     .layer(trace_middleware_compat())
//!     .layer(log_middleware_compat())
//!     .layer(auth_middleware_compat());
//! ```

use axum::body::Body;
use axum::extract::Request;
use axum::middleware::{from_fn, FromFnLayer, Next};
use axum::response::Response;

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;
use uuid::Uuid;

/// 已认证的 Bearer Token（注入到 request extension，供下游 handler 使用）
#[derive(Debug, Clone)]
pub struct CompatAuthToken {
    /// bearer token 原始值
    pub token: String,
}

/// 请求追踪 ID（注入到 request extension，供下游 handler 使用）
#[derive(Debug, Clone)]
pub struct CompatTraceId {
    /// trace_id 字符串
    pub trace_id: String,
}

/// 中间件 handler 返回的 boxed future 类型
type BoxedResponseFuture = Pin<Box<dyn Future<Output = Response> + Send + 'static>>;

/// sz300 兼容 auth 中间件 handler 的函数指针类型
type AuthCompatHandler = fn(Request, Next) -> BoxedResponseFuture;

/// sz300 兼容 log 中间件 handler 的函数指针类型
type LogCompatHandler = fn(Request, Next) -> BoxedResponseFuture;

/// sz300 兼容 trace 中间件 handler 的函数指针类型
type TraceCompatHandler = fn(Request, Next) -> BoxedResponseFuture;

/// sz300 兼容 auth 中间件 — 校验 Bearer Token
///
/// 对齐 sz300 自研 `auth_middleware` 签名，返回 axum 0.8 [`FromFnLayer`]。
///
/// ## 行为
///
/// 1. 从 `Authorization` header 提取 `Bearer <token>`（大小写不敏感）
/// 2. 验证 token 非空
/// 3. 注入 [`CompatAuthToken`] 到 request extension
/// 4. 校验失败返回 `401 Unauthorized`
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_middleware_facade::sz300_compat::auth_middleware_compat;
/// use axum::Router;
///
/// let app: Router = Router::new()
///     .route("/api", axum::routing::get(|| async { "ok" }))
///     .layer(auth_middleware_compat());
/// ```
pub fn auth_middleware_compat() -> FromFnLayer<AuthCompatHandler, (), (Request,)> {
    from_fn(auth_middleware_compat_inner)
}

/// sz300 兼容 log 中间件 — 记录请求方法 + 路径 + 状态码 + 耗时
///
/// 对齐 sz300 自研 `log_middleware` 签名，返回 axum 0.8 [`FromFnLayer`]。
///
/// ## 行为
///
/// 1. 提取请求 method + path
/// 2. 记录起始时间
/// 3. 调用下游 `next`
/// 4. 用 `tracing::info!` 输出结构化日志（method/path/status/elapsed_ms）
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_middleware_facade::sz300_compat::log_middleware_compat;
/// use axum::Router;
///
/// let app: Router = Router::new()
///     .route("/api", axum::routing::get(|| async { "ok" }))
///     .layer(log_middleware_compat());
/// ```
pub fn log_middleware_compat() -> FromFnLayer<LogCompatHandler, (), (Request,)> {
    from_fn(log_middleware_compat_inner)
}

/// sz300 兼容 trace 中间件 — 注入 trace_id
///
/// 对齐 sz300 自研 `trace_middleware` 签名，返回 axum 0.8 [`FromFnLayer`]。
///
/// ## 行为
///
/// 1. 从 `x-request-id` header 提取 trace_id（如果存在）
/// 2. 否则生成新的 UUID v4 作为 trace_id
/// 3. 注入 [`CompatTraceId`] 到 request extension
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_middleware_facade::sz300_compat::trace_middleware_compat;
/// use axum::Router;
///
/// let app: Router = Router::new()
///     .route("/api", axum::routing::get(|| async { "ok" }))
///     .layer(trace_middleware_compat());
/// ```
pub fn trace_middleware_compat() -> FromFnLayer<TraceCompatHandler, (), (Request,)> {
    from_fn(trace_middleware_compat_inner)
}

/// auth 兼容中间件内部实现
fn auth_middleware_compat_inner(req: Request, next: Next) -> BoxedResponseFuture {
    Box::pin(async move {
        let auth_header = req.headers().get(axum::http::header::AUTHORIZATION);
        let token = auth_header
            .and_then(|v| v.to_str().ok())
            .and_then(extract_bearer_token)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        match token {
            Some(t) => {
                let mut req = req;
                req.extensions_mut().insert(CompatAuthToken { token: t });
                next.run(req).await
            }
            None => {
                let mut resp = Response::new(Body::empty());
                *resp.status_mut() = axum::http::StatusCode::UNAUTHORIZED;
                resp
            }
        }
    })
}

/// log 兼容中间件内部实现
fn log_middleware_compat_inner(req: Request, next: Next) -> BoxedResponseFuture {
    Box::pin(async move {
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        let start = Instant::now();

        let response = next.run(req).await;
        let status = response.status().as_u16();
        let elapsed_ms = start.elapsed().as_millis();

        tracing::info!(
            method = %method,
            path = %path,
            status = status,
            elapsed_ms = elapsed_ms,
            "request completed"
        );

        response
    })
}

/// trace 兼容中间件内部实现
fn trace_middleware_compat_inner(req: Request, next: Next) -> BoxedResponseFuture {
    Box::pin(async move {
        let trace_id = req
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        let mut req = req;
        req.extensions_mut().insert(CompatTraceId { trace_id });
        next.run(req).await
    })
}

/// 从 Authorization header 值提取 Bearer token
///
/// 支持大小写不敏感的 `Bearer` 前缀，返回去除前缀后的 token（trim 空白）。
/// 不合法的输入返回 `None`。
fn extract_bearer_token(header_value: &str) -> Option<&str> {
    let trimmed = header_value.trim();
    let prefix = trimmed.get(..7)?;
    if prefix.eq_ignore_ascii_case("bearer ") {
        Some(trimmed[7..].trim())
    } else {
        None
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// 读取响应 body 为字符串
    async fn read_body(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    // ========================================================================
    // extract_bearer_token 单元测试
    // ========================================================================

    #[test]
    fn test_extract_bearer_token_valid() {
        assert_eq!(extract_bearer_token("Bearer abc123"), Some("abc123"));
    }

    #[test]
    fn test_extract_bearer_token_lowercase() {
        assert_eq!(extract_bearer_token("bearer abc123"), Some("abc123"));
    }

    #[test]
    fn test_extract_bearer_token_mixed_case() {
        assert_eq!(extract_bearer_token("BeArEr abc123"), Some("abc123"));
    }

    #[test]
    fn test_extract_bearer_token_no_prefix() {
        assert_eq!(extract_bearer_token("abc123"), None);
    }

    #[test]
    fn test_extract_bearer_token_empty() {
        assert_eq!(extract_bearer_token(""), None);
    }

    #[test]
    fn test_extract_bearer_token_with_trailing_whitespace() {
        assert_eq!(extract_bearer_token("Bearer abc123   "), Some("abc123"));
    }

    #[test]
    fn test_extract_bearer_token_only_prefix() {
        // "Bearer " trim 后为 "Bearer"（6 字符），不满足前缀要求
        assert_eq!(extract_bearer_token("Bearer "), None);
    }

    #[test]
    fn test_extract_bearer_token_too_short() {
        assert_eq!(extract_bearer_token("Bear"), None);
    }

    // ========================================================================
    // auth_middleware_compat 集成测试
    // ========================================================================

    #[tokio::test]
    async fn test_auth_compat_valid_bearer_token() {
        let app = Router::new()
            .route("/api", axum::routing::get(|| async { "ok" }))
            .layer(auth_middleware_compat());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api")
                    .header("authorization", "Bearer abc123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_compat_lowercase_bearer() {
        let app = Router::new()
            .route("/api", axum::routing::get(|| async { "ok" }))
            .layer(auth_middleware_compat());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api")
                    .header("authorization", "bearer abc123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_compat_missing_authorization_header() {
        let app = Router::new()
            .route("/api", axum::routing::get(|| async { "ok" }))
            .layer(auth_middleware_compat());

        let resp = app
            .oneshot(Request::builder().uri("/api").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_compat_empty_token() {
        let app = Router::new()
            .route("/api", axum::routing::get(|| async { "ok" }))
            .layer(auth_middleware_compat());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api")
                    .header("authorization", "Bearer ")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_compat_no_bearer_prefix() {
        let app = Router::new()
            .route("/api", axum::routing::get(|| async { "ok" }))
            .layer(auth_middleware_compat());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api")
                    .header("authorization", "abc123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_compat_injects_token_to_extension() {
        let app = Router::new()
            .route(
                "/api",
                axum::routing::get(|req: Request| async move {
                    let token = req.extensions().get::<CompatAuthToken>().unwrap();
                    token.token.clone()
                }),
            )
            .layer(auth_middleware_compat());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api")
                    .header("authorization", "Bearer my-token-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "my-token-123");
    }

    // ========================================================================
    // log_middleware_compat 集成测试
    // ========================================================================

    #[tokio::test]
    async fn test_log_compat_returns_response_unchanged() {
        let app = Router::new()
            .route("/api", axum::routing::get(|| async { "ok" }))
            .layer(log_middleware_compat());

        let resp = app
            .oneshot(Request::builder().uri("/api").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn test_log_compat_records_status_code() {
        let app = Router::new()
            .route("/api", axum::routing::get(|| async { "ok" }))
            .layer(log_middleware_compat());

        let resp = app
            .oneshot(Request::builder().uri("/api").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_log_compat_handles_post_request() {
        let app = Router::new()
            .route("/api", axum::routing::post(|| async { "created" }))
            .layer(log_middleware_compat());

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "created");
    }

    #[tokio::test]
    async fn test_log_compat_preserves_404_status() {
        let app = Router::new()
            .route("/api", axum::routing::get(|| async { "ok" }))
            .layer(log_middleware_compat());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }

    // ========================================================================
    // trace_middleware_compat 集成测试
    // ========================================================================

    #[tokio::test]
    async fn test_trace_compat_generates_uuid_when_no_header() {
        let app = Router::new()
            .route(
                "/api",
                axum::routing::get(|req: Request| async move {
                    let trace = req.extensions().get::<CompatTraceId>().unwrap();
                    trace.trace_id.clone()
                }),
            )
            .layer(trace_middleware_compat());

        let resp = app
            .oneshot(Request::builder().uri("/api").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = read_body(resp).await;
        assert!(
            Uuid::parse_str(&body).is_ok(),
            "应生成有效 UUID，实际: {body}"
        );
    }

    #[tokio::test]
    async fn test_trace_compat_uses_x_request_id_header() {
        let app = Router::new()
            .route(
                "/api",
                axum::routing::get(|req: Request| async move {
                    let trace = req.extensions().get::<CompatTraceId>().unwrap();
                    trace.trace_id.clone()
                }),
            )
            .layer(trace_middleware_compat());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api")
                    .header("x-request-id", "custom-trace-id-123")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "custom-trace-id-123");
    }

    #[tokio::test]
    async fn test_trace_compat_empty_x_request_id_generates_uuid() {
        let app = Router::new()
            .route(
                "/api",
                axum::routing::get(|req: Request| async move {
                    let trace = req.extensions().get::<CompatTraceId>().unwrap();
                    trace.trace_id.clone()
                }),
            )
            .layer(trace_middleware_compat());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api")
                    .header("x-request-id", "")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = read_body(resp).await;
        assert!(
            Uuid::parse_str(&body).is_ok(),
            "空 header 应生成 UUID，实际: {body}"
        );
    }

    #[tokio::test]
    async fn test_trace_compat_injects_trace_id_to_extension() {
        let app = Router::new()
            .route(
                "/api",
                axum::routing::get(|req: Request| async move {
                    let trace = req.extensions().get::<CompatTraceId>();
                    assert!(trace.is_some(), "trace_id 应注入到 extension");
                    "ok"
                }),
            )
            .layer(trace_middleware_compat());

        let resp = app
            .oneshot(Request::builder().uri("/api").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    // ========================================================================
    // 中间件链组合测试
    // ========================================================================

    #[tokio::test]
    async fn test_compat_middlewares_chain() {
        let app = Router::new()
            .route(
                "/api",
                axum::routing::get(|req: Request| async move {
                    let token = req.extensions().get::<CompatAuthToken>().unwrap();
                    let trace = req.extensions().get::<CompatTraceId>().unwrap();
                    format!("{}|{}", token.token, trace.trace_id)
                }),
            )
            .layer(auth_middleware_compat())
            .layer(trace_middleware_compat());

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api")
                    .header("authorization", "Bearer tok")
                    .header("x-request-id", "tid-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "tok|tid-1");
    }
}
