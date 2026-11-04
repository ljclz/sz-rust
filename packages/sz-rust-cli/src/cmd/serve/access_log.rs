// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! 访问日志中间件
//!
//! 记录每个 HTTP 请求的 method、path、status、elapsed_ms。
//! 禁止记录请求体、Authorization 头值、响应体内容。

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

/// 访问日志中间件处理器
///
/// 记录请求方法、路径、响应状态码和耗时毫秒。
/// 不记录请求体、Authorization 头值或响应体内容。
pub async fn access_log_handler(req: Request, next: Next) -> Response {
    let method = req.method().as_str().to_owned();
    let path = req.uri().path().to_owned();
    let start = std::time::Instant::now();

    let response = next.run(req).await;

    let elapsed_ms = start.elapsed().as_millis();
    let status = response.status().as_u16();

    tracing::info!(
        "access_log: method={} path={} status={} elapsed_ms={}",
        method,
        path,
        status,
        elapsed_ms
    );

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use http::StatusCode;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_access_log_records_request() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(access_log_handler));

        let response = app
            .oneshot(
                http::Request::builder()
                    .method("GET")
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_access_log_preserves_response() {
        let app = Router::new()
            .route("/api/test", get(|| async { "hello" }))
            .layer(axum::middleware::from_fn(access_log_handler));

        let response = app
            .oneshot(
                http::Request::builder()
                    .method("GET")
                    .uri("/api/test")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        use http_body_util::BodyExt;
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn test_access_log_404_response() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(access_log_handler));

        let response = app
            .oneshot(
                http::Request::builder()
                    .method("GET")
                    .uri("/nonexistent")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
