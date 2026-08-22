//! 请求体大小限制中间件 — 拒绝超过限制的请求体
//!
//! 对齐 spec §5.4.1（7 条业务规则）+ §6.4（BodySizeLimitConfig）。

use serde::Deserialize;
use std::collections::HashMap;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

/// 请求体大小限制配置（spec §6.4）
#[derive(Debug, Clone, Deserialize)]
pub struct BodySizeLimitConfig {
    /// 是否启用大小限制（默认 false，向后兼容）
    #[serde(default)]
    pub enabled: bool,
    /// 全局默认上限（字节），必填且 > 0
    #[serde(default = "default_max_body_size")]
    pub max_body_size: u64,
    /// 路由级覆盖（键为路由路径，值为该路由的大小上限）
    #[serde(default)]
    pub route_overrides: HashMap<String, u64>,
    /// 排除路径列表（精确匹配）
    #[serde(default)]
    pub exclude_paths: Vec<String>,
    /// 跳过校验的 HTTP 方法
    #[serde(default = "default_safe_methods")]
    pub safe_methods: Vec<String>,
}

fn default_max_body_size() -> u64 {
    2 * 1024 * 1024
}

fn default_safe_methods() -> Vec<String> {
    vec![
        "GET".to_string(),
        "HEAD".to_string(),
        "OPTIONS".to_string(),
        "DELETE".to_string(),
    ]
}

impl Default for BodySizeLimitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_body_size: default_max_body_size(),
            route_overrides: HashMap::new(),
            exclude_paths: Vec::new(),
            safe_methods: default_safe_methods(),
        }
    }
}

impl BodySizeLimitConfig {
    /// 获取指定路径的有效大小上限
    ///
    /// 路由级覆盖优先于全局默认（spec §6.4 第 3 条）。
    pub fn get_effective_limit(&self, path: &str) -> u64 {
        self.route_overrides
            .get(path)
            .copied()
            .unwrap_or(self.max_body_size)
    }

    /// 判断指定方法是否为安全方法（跳过校验）
    pub fn is_safe_method(&self, method: &str) -> bool {
        self.safe_methods
            .iter()
            .any(|m| m.eq_ignore_ascii_case(method))
    }

    /// 判断指定路径是否被排除
    pub fn is_excluded(&self, path: &str) -> bool {
        self.exclude_paths.contains(&path.to_string())
    }
}

/// 构造 413 请求体过大响应
fn body_too_large_response(limit: u64) -> Response {
    let exception = sz_rust_http_facade::BaseException::payload_too_large(format!(
        "请求体超过限制: {limit} 字节"
    ));
    let json = exception.to_json();
    let body = serde_json::to_string(&json).unwrap_or_else(|_| "{}".to_string());
    Response::builder()
        .status(axum::http::StatusCode::PAYLOAD_TOO_LARGE)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| Response::new(axum::body::Body::empty()))
}

/// 请求体大小限制中间件
///
/// 若 `config.enabled == false` 直接放行（spec §4.5.1）。
/// 安全方法跳过校验（spec §6.4 第 5 条）。
/// 排除路径跳过校验（spec §6.4 第 4 条）。
/// Content-Length 超过限制则返回 413（spec §5.4.1 规则 1）。
pub async fn body_size_limit_middleware(
    axum::extract::State(config): axum::extract::State<BodySizeLimitConfig>,
    req: Request,
    next: Next,
) -> Response {
    if !config.enabled {
        return next.run(req).await;
    }

    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    if config.is_safe_method(&method) || config.is_excluded(&path) {
        return next.run(req).await;
    }

    let effective_limit = config.get_effective_limit(&path);

    if let Some(content_length) = req
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
    {
        if content_length > effective_limit {
            return body_too_large_response(effective_limit);
        }
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_disabled() {
        let cfg = BodySizeLimitConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.max_body_size, 2 * 1024 * 1024);
        assert!(cfg.safe_methods.contains(&"GET".to_string()));
    }

    #[test]
    fn test_get_effective_limit_default() {
        let cfg = BodySizeLimitConfig::default();
        assert_eq!(cfg.get_effective_limit("/api/data"), 2 * 1024 * 1024);
    }

    #[test]
    fn test_get_effective_limit_route_override() {
        let mut cfg = BodySizeLimitConfig::default();
        cfg.route_overrides
            .insert("/api/upload".to_string(), 10 * 1024 * 1024);
        assert_eq!(cfg.get_effective_limit("/api/upload"), 10 * 1024 * 1024);
        assert_eq!(cfg.get_effective_limit("/api/data"), 2 * 1024 * 1024);
    }

    #[test]
    fn test_is_safe_method() {
        let cfg = BodySizeLimitConfig::default();
        assert!(cfg.is_safe_method("GET"));
        assert!(cfg.is_safe_method("get"));
        assert!(cfg.is_safe_method("HEAD"));
        assert!(cfg.is_safe_method("OPTIONS"));
        assert!(cfg.is_safe_method("DELETE"));
        assert!(!cfg.is_safe_method("POST"));
        assert!(!cfg.is_safe_method("PUT"));
    }

    #[test]
    fn test_is_excluded() {
        let cfg = BodySizeLimitConfig {
            exclude_paths: vec!["/health".to_string()],
            ..BodySizeLimitConfig::default()
        };
        assert!(cfg.is_excluded("/health"));
        assert!(!cfg.is_excluded("/api/data"));
    }

    #[tokio::test]
    async fn test_middleware_disabled_passes_through() {
        use axum::routing::get;
        use tower::ServiceExt;

        let config = BodySizeLimitConfig::default();

        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                body_size_limit_middleware,
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
    async fn test_middleware_rejects_oversized_post() {
        use axum::routing::post;
        use tower::ServiceExt;

        let config = BodySizeLimitConfig {
            enabled: true,
            max_body_size: 100,
            ..Default::default()
        };

        let app = axum::Router::new()
            .route("/", post(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                body_size_limit_middleware,
            ));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("content-length", "200")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_middleware_allows_safe_method() {
        use axum::routing::get;
        use tower::ServiceExt;

        let config = BodySizeLimitConfig {
            enabled: true,
            max_body_size: 100,
            ..Default::default()
        };

        let app = axum::Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                body_size_limit_middleware,
            ));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .header("content-length", "999999")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }
}
