// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 上传大小限制中间件
//!
//! 检查请求 `Content-Length` 头，超过上限直接返回 413 Payload Too Large。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

/// 上传大小限制中间件状态
pub type UploadLimitState = Arc<u64>;

/// 检查 `Content-Length`，超限返回 413
pub async fn upload_limit_middleware(
    axum::extract::State(max_size): axum::extract::State<UploadLimitState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if let Some(content_length) = req
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
    {
        if content_length > *max_size {
            return (StatusCode::PAYLOAD_TOO_LARGE, "文件大小超过限制").into_response();
        }
    }
    next.run(req).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::middleware;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// 构造挂载 upload_limit_middleware 的测试 Router（下游 handler 固定返回 "ok"）
    fn build_router(max_size: u64) -> Router {
        let state: UploadLimitState = Arc::new(max_size);
        Router::new()
            .route("/any", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(state, upload_limit_middleware))
    }

    /// 收集响应体为 UTF-8 字符串
    async fn fetch_body_string(resp: Response) -> String {
        let bytes = resp
            .into_body()
            .collect()
            .await
            .expect("响应体读取失败")
            .to_bytes();
        String::from_utf8(bytes.to_vec()).expect("响应体应为 UTF-8")
    }

    /// Content-Length 超过 max_size → 413 Payload Too Large（验收 R4-AC1）
    #[tokio::test]
    async fn content_length_exceeds_limit_returns_413() {
        let app = build_router(1024);
        let req = Request::builder()
            .method("GET")
            .uri("/any")
            .header("content-length", "2048")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = fetch_body_string(resp).await;
        assert!(
            body.contains("文件大小超过限制"),
            "413 响应体应包含超限提示，实际: {body}"
        );
    }

    /// Content-Length 不超过 max_size → 放行到 next（200）（验收 R4-AC2）
    #[tokio::test]
    async fn content_length_within_limit_passes_through() {
        let app = build_router(1024);
        let req = Request::builder()
            .method("GET")
            .uri("/any")
            .header("content-length", "512")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = fetch_body_string(resp).await;
        assert_eq!(body, "ok", "不超限应放行到 next handler");
    }

    /// 无 Content-Length 头 → 放行到 next（200）（验收 R4-AC3）
    #[tokio::test]
    async fn missing_content_length_passes_through() {
        let app = build_router(1024);
        let req = Request::builder()
            .method("GET")
            .uri("/any")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = fetch_body_string(resp).await;
        assert_eq!(body, "ok", "无 Content-Length 应放行到 next handler");
    }

    /// Content-Length 非数字 → parse 失败走 None 分支，放行到 next（200）
    #[tokio::test]
    async fn non_numeric_content_length_passes_through() {
        let app = build_router(1024);
        let req = Request::builder()
            .method("GET")
            .uri("/any")
            .header("content-length", "abc")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = fetch_body_string(resp).await;
        assert_eq!(body, "ok", "非数字 Content-Length 应放行到 next handler");
    }
}
