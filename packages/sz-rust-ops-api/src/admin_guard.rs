// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Admin 鉴权中间件（T054）
//!
//! 校验请求头 `x-admin-token`，匹配则放行，不匹配返回 403。
//! 响应不携带 token 等敏感信息；token 比较采用常量时间算法以规避时序攻击。

use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Admin 鉴权守卫，持有期望的 token。
///
/// `expected_token` 为私有字段，避免被意外序列化或日志泄露。
#[derive(Clone)]
pub struct AdminGuard {
    expected_token: String,
}

impl AdminGuard {
    /// 创建 AdminGuard，传入期望的 admin token。
    pub fn new(expected_token: String) -> Self {
        Self { expected_token }
    }
}

/// 403 错误响应体（不含敏感信息）。
#[derive(Debug, Clone, Serialize)]
pub struct AdminErrorResponse {
    /// 错误描述。
    pub error: String,
    /// HTTP 状态码。
    pub code: u16,
}

/// axum 0.8 中间件：校验 `x-admin-token` 请求头。
///
/// 匹配放行；缺失或不匹配返回 403。
pub async fn admin_guard_middleware(
    State(guard): State<AdminGuard>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let token_valid = req
        .headers()
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| constant_time_eq(v, &guard.expected_token));

    if token_valid {
        next.run(req).await
    } else {
        (
            axum::http::StatusCode::FORBIDDEN,
            axum::Json(AdminErrorResponse {
                error: "forbidden".to_string(),
                code: 403,
            }),
        )
            .into_response()
    }
}

/// 常量时间字符串比较，规避时序侧信道。
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower::ServiceExt;

    fn make_router(token: &str) -> axum::Router {
        axum::Router::new()
            .route("/ops/ping", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                AdminGuard::new(token.to_string()),
                admin_guard_middleware,
            ))
    }

    #[tokio::test]
    async fn test_valid_token_passes() {
        let app = make_router("secret");
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ops/ping")
                    .header("x-admin-token", "secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_invalid_token_forbidden() {
        let app = make_router("secret");
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ops/ping")
                    .header("x-admin-token", "wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_missing_token_forbidden() {
        let app = make_router("secret");
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ops/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_forbidden_body_excludes_token() {
        let app = make_router("top-secret-token");
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/ops/ping")
                    .header("x-admin-token", "bad")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), 8192).await.unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(!text.contains("top-secret-token"), "响应体不得泄露 token");
        assert!(text.contains("forbidden"));
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("", "a"));
        assert!(constant_time_eq("", ""));
    }
}
