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
