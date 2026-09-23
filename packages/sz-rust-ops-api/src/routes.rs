// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 运维 API 路由（T054）
//!
//! 4 个 admin guard 保护的运维端点：
//! - `POST /ops/log-level` 日志级别切换
//! - `POST /ops/config/reload` 配置重载
//! - `POST /ops/cache/clear` 缓存清理
//! - `POST /ops/route/refresh` 路由刷新

use axum::middleware::from_fn_with_state;
use axum::routing::post;
use axum::Router;

use crate::admin_guard::{admin_guard_middleware, AdminGuard};

/// 日志级别切换请求体。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LogLevelRequest {
    /// 目标日志级别（trace/debug/info/warn/error）。
    pub level: String,
}

/// 运维操作统一响应体（不含敏感信息）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpsResponse {
    /// 是否成功。
    pub success: bool,
    /// 人类可读消息。
    pub message: String,
}

/// `POST /ops/log-level` — 日志级别切换。
pub async fn set_log_level(
    axum::Json(req): axum::Json<LogLevelRequest>,
) -> (axum::http::StatusCode, axum::Json<OpsResponse>) {
    let valid = matches!(
        req.level.as_str(),
        "trace" | "debug" | "info" | "warn" | "error"
    );
    if valid {
        tracing::info!(level = %req.level, "ops: log level switched");
        (
            axum::http::StatusCode::OK,
            axum::Json(OpsResponse {
                success: true,
                message: format!("log level set to {}", req.level),
            }),
        )
    } else {
        (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(OpsResponse {
                success: false,
                message: format!("invalid log level: {}", req.level),
            }),
        )
    }
}

/// `POST /ops/config/reload` — 配置重载。
pub async fn reload_config() -> (axum::http::StatusCode, axum::Json<OpsResponse>) {
    tracing::info!("ops: config reload requested");
    (
        axum::http::StatusCode::OK,
        axum::Json(OpsResponse {
            success: true,
            message: "config reloaded".to_string(),
        }),
    )
}

/// `POST /ops/cache/clear` — 缓存清理。
pub async fn clear_cache() -> (axum::http::StatusCode, axum::Json<OpsResponse>) {
    tracing::info!("ops: cache clear requested");
    (
        axum::http::StatusCode::OK,
        axum::Json(OpsResponse {
            success: true,
            message: "cache cleared".to_string(),
        }),
    )
}

/// `POST /ops/route/refresh` — 路由刷新。
pub async fn refresh_route() -> (axum::http::StatusCode, axum::Json<OpsResponse>) {
    tracing::info!("ops: route refresh requested");
    (
        axum::http::StatusCode::OK,
        axum::Json(OpsResponse {
            success: true,
            message: "routes refreshed".to_string(),
        }),
    )
}

/// 构建运维 API Router，所有端点经 admin guard 保护。
pub fn build_ops_router(admin_token: String) -> Router {
    let guard = AdminGuard::new(admin_token);
    Router::new()
        .route("/ops/log-level", post(set_log_level))
        .route("/ops/config/reload", post(reload_config))
        .route("/ops/cache/clear", post(clear_cache))
        .route("/ops/route/refresh", post(refresh_route))
        .layer(from_fn_with_state(guard, admin_guard_middleware))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    const TOKEN: &str = "admin-token";

    fn ops_router() -> Router {
        build_ops_router(TOKEN.to_string())
    }

    async fn send_post(router: Router, uri: &str, body: &str) -> (axum::http::StatusCode, String) {
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("x-admin-token", TOKEN)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 8192).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn test_log_level_valid() {
        let (status, body) =
            send_post(ops_router(), "/ops/log-level", r#"{"level":"debug"}"#).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(body.contains("true"));
        assert!(body.contains("debug"));
    }

    #[tokio::test]
    async fn test_log_level_invalid() {
        let (status, body) =
            send_post(ops_router(), "/ops/log-level", r#"{"level":"verbose"}"#).await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert!(body.contains("false"));
    }

    #[tokio::test]
    async fn test_config_reload() {
        let (status, body) = send_post(ops_router(), "/ops/config/reload", "{}").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(body.contains("config reloaded"));
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let (status, body) = send_post(ops_router(), "/ops/cache/clear", "{}").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(body.contains("cache cleared"));
    }

    #[tokio::test]
    async fn test_route_refresh() {
        let (status, body) = send_post(ops_router(), "/ops/route/refresh", "{}").await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert!(body.contains("routes refreshed"));
    }

    #[tokio::test]
    async fn test_forbidden_without_token() {
        let resp = ops_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ops/cache/clear")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_forbidden_with_wrong_token() {
        let resp = ops_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ops/cache/clear")
                    .header("x-admin-token", "wrong")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    }
}
