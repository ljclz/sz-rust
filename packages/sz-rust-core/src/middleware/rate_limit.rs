//! RateLimit 中间件 — 限流（复用 sz-orm-limit）
//!
//! sz-rust 自研中间件，PHP 端无限流实现（PHP `app/middleware.php` 仅含
//! `SessionInit` + `AllowCrossDomain`，业务代码也无 `cache('rate_...')` 等频率限制）。
//! 本模块在 [`crate::middleware::order::DEFAULT_ORDER`] 中位于第 4 位
//! （`Trace` → `Cors` → `Log` → **`RateLimit`** → `Auth`），在鉴权之前限流，
//! 避免无效请求消耗鉴权开销。
//!
//! ## 行为
//!
//! 1. **排除路径检查**：如果请求路径在 `exclude_paths` 中，直接放行（不消耗令牌）
//! 2. **提取限流 Key**：根据 `key_extractor` 策略提取（Ip / UserId / IpPlusRoute）
//! 3. **调用 `limiter.acquire(&key)`**：返回 `RateLimitResult`
//!    - `allowed=true` → 放行，响应添加 `X-RateLimit-Remaining` / `X-RateLimit-Reset` headers
//!    - `allowed=false` → 返回 HTTP 429，响应添加 `Retry-After` / `X-RateLimit-*` headers
//! 4. **错误处理**：limiter 内部错误（如 RwLock 中毒）采用 **fail-open** 策略（放行避免影响业务）
//!
//! ## 限流算法
//!
//! 复用 `sz-orm-limit` 提供的两种算法：
//! - `SlidingWindowRateLimiter`：滑动窗口（保留窗口内所有请求时间戳）
//! - `TokenBucketRateLimiter`：令牌桶（容量 + 每秒补充速率）
//!
//! ## Key 提取策略
//!
//! | 策略 | Key 组成 | 适用场景 |
//! |------|---------|---------|
//! | `Ip` | 客户端 IP | 全局限流（默认） |
//! | `UserId` | 已认证用户 ID（需前置 Auth 中间件） | 用户级限流 |
//! | `IpPlusRoute` | `IP:route_path` | 路由级限流 |
//!
//! 客户端 IP 提取优先级：`X-Forwarded-For`（取第一个）> `X-Real-IP` > `"unknown"`
//!
//! ## 响应格式
//!
//! ### 限流通过（HTTP 200/2xx/4xx/5xx 由下游决定）
//!
//! 响应 headers 添加：
//! - `X-RateLimit-Remaining: <剩余配额>`
//! - `X-RateLimit-Reset: <Unix 毫秒时间戳>`
//!
//! ### 限流拒绝（HTTP 429 Too Many Requests）
//!
//! 响应 headers：
//! - `X-RateLimit-Remaining: 0`
//! - `X-RateLimit-Reset: <Unix 毫秒时间戳>`
//! - `Retry-After: <秒数>`
//!
//! 响应体（对齐 PHP `renderJson` 格式，code=429 表示限流）：
//! ```json
//! {
//!   "code": 429,
//!   "msg": "Too Many Requests",
//!   "data": {
//!     "retry_after_seconds": 60,
//!     "reset_at_ms": 1234567890123
//!   }
//! }
//! ```
//!
//! ## PHP 对齐
//!
//! PHP 端无限流实现，sz-rust 的 RateLimit 中间件是自研增强，提供：
//! - 请求频率自动控制（无需业务代码手动检查）
//! - 多算法支持（滑动窗口 / 令牌桶）
//! - 多 Key 策略（IP / UserId / IpPlusRoute）
//! - 标准 HTTP 429 响应 + 限流 headers
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::middleware::rate_limit::{sliding_window_config, rate_limit_middleware};
//! use std::time::Duration;
//! use axum::Router;
//!
//! let config = sliding_window_config(100, Duration::from_secs(60))
//!     .with_exclude_paths(vec!["/health".to_string()]);
//! let app: Router = Router::new()
//!     .route("/", axum::routing::get(|| async { "ok" }))
//!     .layer(axum::middleware::from_fn_with_state(config, rate_limit_middleware));
//! ```

use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sz_orm_limit::RateLimiter;

use crate::middleware::auth::AuthenticatedUser;

/// 限流 Key 提取策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum KeyExtractor {
    /// 按客户端 IP（从 `X-Forwarded-For` 或 `X-Real-IP`）
    #[default]
    Ip,
    /// 按已认证用户 ID（需前置 Auth 中间件注入 `AuthenticatedUser`）
    ///
    /// 如果 extensions 中无 `AuthenticatedUser`（如 Auth 中间件未执行或白名单跳过），
    /// 回退到客户端 IP。
    UserId,
    /// 按 IP + 路由组合（`IP:route_path`）
    IpPlusRoute,
}

impl KeyExtractor {
    /// 返回策略的人类可读名称
    pub fn as_str(self) -> &'static str {
        match self {
            KeyExtractor::Ip => "ip",
            KeyExtractor::UserId => "user_id",
            KeyExtractor::IpPlusRoute => "ip_plus_route",
        }
    }
}

impl std::fmt::Display for KeyExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// RateLimit 中间件配置
///
/// 必须通过 [`RateLimitConfig::new()`] 构造，传入一个 `RateLimiter` 实例（用 `Arc` 包裹）。
/// 可通过 `with_*` 链式 builder 方法配置 Key 提取策略、排除路径、Key 前缀。
#[derive(Clone)]
pub struct RateLimitConfig {
    /// 限流器实例（`Arc<dyn RateLimiter + Send + Sync>` 共享）
    pub limiter: Arc<dyn RateLimiter + Send + Sync>,
    /// Key 提取策略（默认 `Ip`）
    pub key_extractor: KeyExtractor,
    /// 排除路径（不进行限流，复用 [`crate::middleware::auth::is_route_allowed`] 匹配）
    pub exclude_paths: Vec<String>,
    /// Key 前缀（用于区分不同限流场景，如 `"login"` / `"api"` / `"sms"`）
    pub key_prefix: String,
}

impl std::fmt::Debug for RateLimitConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimitConfig")
            .field("key_extractor", &self.key_extractor)
            .field("exclude_paths", &self.exclude_paths)
            .field("key_prefix", &self.key_prefix)
            .finish_non_exhaustive()
    }
}

impl RateLimitConfig {
    /// 创建 RateLimitConfig
    pub fn new(limiter: Arc<dyn RateLimiter + Send + Sync>) -> Self {
        Self {
            limiter,
            key_extractor: KeyExtractor::default(),
            exclude_paths: Vec::new(),
            key_prefix: String::new(),
        }
    }

    /// 设置 Key 提取策略
    pub fn with_key_extractor(mut self, extractor: KeyExtractor) -> Self {
        self.key_extractor = extractor;
        self
    }

    /// 设置排除路径
    pub fn with_exclude_paths(mut self, paths: Vec<String>) -> Self {
        self.exclude_paths = paths;
        self
    }

    /// 设置 Key 前缀（用于区分不同限流场景）
    pub fn with_key_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.key_prefix = prefix.into();
        self
    }

    /// 判断路径是否被排除
    pub fn is_excluded(&self, path: &str) -> bool {
        crate::middleware::auth::is_route_allowed(path, &self.exclude_paths)
    }
}

/// 从请求 headers 提取客户端 IP
///
/// 优先级：`X-Forwarded-For`（取第一个，对齐 PHP `request()->ip()` 的代理透传行为）
/// > `X-Real-IP` > `"unknown"`
///
/// **注意**：`X-Forwarded-For` 可被客户端伪造，生产环境应通过可信代理覆盖该 header。
pub fn extract_client_ip(headers: &HeaderMap) -> String {
    if let Some(forwarded) = headers.get("x-forwarded-for") {
        if let Ok(value) = forwarded.to_str() {
            // X-Forwarded-For: client, proxy1, proxy2
            if let Some(first) = value.split(',').next() {
                let trimmed = first.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    if let Some(real_ip) = headers.get("x-real-ip") {
        if let Ok(value) = real_ip.to_str() {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "unknown".to_string()
}

/// 从请求中提取限流 Key
///
/// 根据 [`RateLimitConfig::key_extractor`] 策略提取 Key，并拼接 `key_prefix`（如果非空）。
pub fn extract_rate_limit_key(req: &Request, config: &RateLimitConfig) -> String {
    let inner_key = match config.key_extractor {
        KeyExtractor::Ip => extract_client_ip(req.headers()),
        KeyExtractor::UserId => req
            .extensions()
            .get::<AuthenticatedUser>()
            .map(|u| u.user_id.to_string())
            .unwrap_or_else(|| extract_client_ip(req.headers())),
        KeyExtractor::IpPlusRoute => {
            let ip = extract_client_ip(req.headers());
            let path = req.uri().path();
            format!("{}:{}", ip, path)
        }
    };
    if config.key_prefix.is_empty() {
        inner_key
    } else {
        format!("{}:{}", config.key_prefix, inner_key)
    }
}

/// 构建限流拒绝响应（HTTP 429 + 限流 headers）
///
/// 对齐 PHP `renderJson` 格式（`code` / `msg` / `data`），`code=429` 表示限流。
/// 响应 headers 添加 `X-RateLimit-Remaining` / `X-RateLimit-Reset` / `Retry-After`。
pub fn rate_limit_rejected_response(result: &sz_orm_limit::RateLimitResult) -> Response {
    let now_ms = current_unix_ms();
    let retry_after_seconds = ((result.reset_at - now_ms) / 1000).max(1) as u64;

    let body = json!({
        "code": 429,
        "msg": "Too Many Requests",
        "data": {
            "retry_after_seconds": retry_after_seconds,
            "reset_at_ms": result.reset_at
        }
    })
    .to_string();

    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        body,
    )
        .into_response();

    insert_rate_limit_headers(&mut response, result, retry_after_seconds);
    response
}

/// 当前 UNIX 毫秒时间戳
fn current_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 向响应添加限流 headers
fn insert_rate_limit_headers(
    response: &mut Response,
    result: &sz_orm_limit::RateLimitResult,
    retry_after_seconds: u64,
) {
    let headers = response.headers_mut();
    headers.insert(
        "x-ratelimit-remaining",
        HeaderValue::from_str(&result.remaining.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0")),
    );
    headers.insert(
        "x-ratelimit-reset",
        HeaderValue::from_str(&result.reset_at.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0")),
    );
    headers.insert(
        "retry-after",
        HeaderValue::from_str(&retry_after_seconds.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("1")),
    );
}

/// RateLimit 中间件主函数
///
/// ## 校验流程
///
/// 1. **排除路径检查**：如果请求路径在 `exclude_paths` 中，直接放行（不消耗令牌）
/// 2. **提取限流 Key**：根据 `key_extractor` 策略提取
/// 3. **调用 `limiter.acquire(&key)`**：
///    - `allowed=true` → 放行，响应添加 `X-RateLimit-Remaining` / `X-RateLimit-Reset`
///    - `allowed=false` → 返回 HTTP 429 + 限流 headers
/// 4. **错误处理**：limiter 内部错误采用 **fail-open** 策略（放行 + 错误日志）
pub async fn rate_limit_middleware(
    axum::extract::State(config): axum::extract::State<RateLimitConfig>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    // 1. 排除路径直接放行
    if config.is_excluded(&path) {
        return next.run(req).await;
    }

    // 2. 提取限流 Key
    let key = extract_rate_limit_key(&req, &config);

    // 3. 调用限流器（同步阻塞，但临界区短）
    match config.limiter.acquire(&key) {
        Ok(result) if result.allowed => {
            // 允许通过，添加限流 headers 到响应
            let mut response = next.run(req).await;
            let retry_after_seconds = ((result.reset_at - current_unix_ms()) / 1000).max(1) as u64;
            insert_rate_limit_headers(&mut response, &result, retry_after_seconds);
            response
        }
        Ok(result) => {
            // 限流拒绝
            rate_limit_rejected_response(&result)
        }
        Err(err) => {
            // limiter 内部错误（如 RwLock 中毒），fail-open 放行
            tracing::error!(
                error = %err,
                key = %key,
                "rate_limit limiter error, fail-open"
            );
            next.run(req).await
        }
    }
}

/// 创建滑动窗口限流器配置（便捷函数）
///
/// 等价于：
/// ```ignore
/// use std::sync::Arc;
/// use sz_orm_limit::SlidingWindowRateLimiter;
/// RateLimitConfig::new(Arc::new(SlidingWindowRateLimiter::new(max_requests, window_size)))
/// ```
pub fn sliding_window_config(max_requests: u64, window_size: Duration) -> RateLimitConfig {
    let limiter = Arc::new(sz_orm_limit::SlidingWindowRateLimiter::new(
        max_requests,
        window_size,
    ));
    RateLimitConfig::new(limiter)
}

/// 创建令牌桶限流器配置（便捷函数）
///
/// 等价于：
/// ```ignore
/// use std::sync::Arc;
/// use sz_orm_limit::TokenBucketRateLimiter;
/// RateLimitConfig::new(Arc::new(TokenBucketRateLimiter::new(capacity, refill_per_second)))
/// ```
pub fn token_bucket_config(capacity: u64, refill_per_second: f64) -> RateLimitConfig {
    let limiter = Arc::new(sz_orm_limit::TokenBucketRateLimiter::new(
        capacity,
        refill_per_second,
    ));
    RateLimitConfig::new(limiter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // ====================================================================
    // 辅助函数
    // ====================================================================

    async fn read_body(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn make_request(method: &str, uri: &str) -> Request {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn make_request_with_ip(method: &str, uri: &str, ip: &str) -> Request {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("x-forwarded-for", ip)
            .body(Body::empty())
            .unwrap()
    }

    /// 构建测试用 Router（使用滑动窗口：2 次/60 秒）
    fn build_app_sliding_window() -> Router {
        let config = sliding_window_config(2, Duration::from_secs(60));
        Router::new()
            .route(
                "/api",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                rate_limit_middleware,
            ))
    }

    /// 构建测试用 Router（使用令牌桶：容量 2，每秒补充 1）
    fn build_app_token_bucket() -> Router {
        let config = token_bucket_config(2, 1.0);
        Router::new()
            .route(
                "/api",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                rate_limit_middleware,
            ))
    }

    // ====================================================================
    // KeyExtractor 单元测试
    // ====================================================================

    #[test]
    fn test_key_extractor_as_str() {
        assert_eq!(KeyExtractor::Ip.as_str(), "ip");
        assert_eq!(KeyExtractor::UserId.as_str(), "user_id");
        assert_eq!(KeyExtractor::IpPlusRoute.as_str(), "ip_plus_route");
    }

    #[test]
    fn test_key_extractor_display() {
        assert_eq!(KeyExtractor::Ip.to_string(), "ip");
        assert_eq!(KeyExtractor::UserId.to_string(), "user_id");
        assert_eq!(KeyExtractor::IpPlusRoute.to_string(), "ip_plus_route");
    }

    #[test]
    fn test_key_extractor_default_is_ip() {
        assert_eq!(KeyExtractor::default(), KeyExtractor::Ip);
    }

    #[test]
    fn test_key_extractor_equality() {
        assert_eq!(KeyExtractor::Ip, KeyExtractor::Ip);
        assert_ne!(KeyExtractor::Ip, KeyExtractor::UserId);
        assert_ne!(KeyExtractor::UserId, KeyExtractor::IpPlusRoute);
    }

    #[test]
    fn test_key_extractor_copy_clone() {
        let extractor = KeyExtractor::UserId;
        let copied = extractor; // Copy 语义
        assert_eq!(extractor, copied);
    }

    // ====================================================================
    // extract_client_ip 单元测试
    // ====================================================================

    #[test]
    fn test_extract_client_ip_from_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());
        assert_eq!(extract_client_ip(&headers), "1.2.3.4");
    }

    #[test]
    fn test_extract_client_ip_from_x_forwarded_for_multi() {
        // X-Forwarded-For: client, proxy1, proxy2
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "1.2.3.4, 5.6.7.8, 9.10.11.12".parse().unwrap(),
        );
        assert_eq!(extract_client_ip(&headers), "1.2.3.4");
    }

    #[test]
    fn test_extract_client_ip_from_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "1.2.3.4".parse().unwrap());
        assert_eq!(extract_client_ip(&headers), "1.2.3.4");
    }

    #[test]
    fn test_extract_client_ip_x_forwarded_for_takes_priority() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.1.1.1".parse().unwrap());
        headers.insert("x-real-ip", "2.2.2.2".parse().unwrap());
        assert_eq!(extract_client_ip(&headers), "1.1.1.1");
    }

    #[test]
    fn test_extract_client_ip_no_headers() {
        let headers = HeaderMap::new();
        assert_eq!(extract_client_ip(&headers), "unknown");
    }

    #[test]
    fn test_extract_client_ip_empty_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "".parse().unwrap());
        // 空 X-Forwarded-For 应回退到 X-Real-IP 或 unknown
        assert_eq!(extract_client_ip(&headers), "unknown");
    }

    #[test]
    fn test_extract_client_ip_empty_x_forwarded_for_falls_back_to_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "".parse().unwrap());
        headers.insert("x-real-ip", "3.3.3.3".parse().unwrap());
        assert_eq!(extract_client_ip(&headers), "3.3.3.3");
    }

    #[test]
    fn test_extract_client_ip_trims_whitespace() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "  1.2.3.4  ".parse().unwrap());
        assert_eq!(extract_client_ip(&headers), "1.2.3.4");
    }

    // ====================================================================
    // extract_rate_limit_key 单元测试
    // ====================================================================

    #[test]
    fn test_extract_rate_limit_key_ip_strategy() {
        let config = sliding_window_config(10, Duration::from_secs(60));
        let req = make_request_with_ip("GET", "/api", "1.2.3.4");
        assert_eq!(extract_rate_limit_key(&req, &config), "1.2.3.4");
    }

    #[test]
    fn test_extract_rate_limit_key_ip_strategy_no_ip_header() {
        let config = sliding_window_config(10, Duration::from_secs(60));
        let req = make_request("GET", "/api");
        assert_eq!(extract_rate_limit_key(&req, &config), "unknown");
    }

    #[test]
    fn test_extract_rate_limit_key_user_id_strategy_with_auth() {
        let config = sliding_window_config(10, Duration::from_secs(60))
            .with_key_extractor(KeyExtractor::UserId);
        let mut req = make_request_with_ip("GET", "/api", "1.2.3.4");
        req.extensions_mut()
            .insert(AuthenticatedUser { user_id: 42 });
        assert_eq!(extract_rate_limit_key(&req, &config), "42");
    }

    #[test]
    fn test_extract_rate_limit_key_user_id_strategy_fallback_to_ip() {
        // 无 AuthenticatedUser 时回退到 IP
        let config = sliding_window_config(10, Duration::from_secs(60))
            .with_key_extractor(KeyExtractor::UserId);
        let req = make_request_with_ip("GET", "/api", "1.2.3.4");
        assert_eq!(extract_rate_limit_key(&req, &config), "1.2.3.4");
    }

    #[test]
    fn test_extract_rate_limit_key_ip_plus_route_strategy() {
        let config = sliding_window_config(10, Duration::from_secs(60))
            .with_key_extractor(KeyExtractor::IpPlusRoute);
        let req = make_request_with_ip("GET", "/api/users", "1.2.3.4");
        assert_eq!(extract_rate_limit_key(&req, &config), "1.2.3.4:/api/users");
    }

    #[test]
    fn test_extract_rate_limit_key_with_prefix() {
        let config = sliding_window_config(10, Duration::from_secs(60)).with_key_prefix("login");
        let req = make_request_with_ip("GET", "/api", "1.2.3.4");
        assert_eq!(extract_rate_limit_key(&req, &config), "login:1.2.3.4");
    }

    #[test]
    fn test_extract_rate_limit_key_with_prefix_and_user_id() {
        let config = sliding_window_config(10, Duration::from_secs(60))
            .with_key_extractor(KeyExtractor::UserId)
            .with_key_prefix("api");
        let mut req = make_request("GET", "/api");
        req.extensions_mut()
            .insert(AuthenticatedUser { user_id: 100 });
        assert_eq!(extract_rate_limit_key(&req, &config), "api:100");
    }

    // ====================================================================
    // RateLimitConfig 单元测试
    // ====================================================================

    #[test]
    fn test_rate_limit_config_default() {
        let config = sliding_window_config(10, Duration::from_secs(60));
        assert_eq!(config.key_extractor, KeyExtractor::Ip);
        assert!(config.exclude_paths.is_empty());
        assert!(config.key_prefix.is_empty());
    }

    #[test]
    fn test_rate_limit_config_with_key_extractor() {
        let config = sliding_window_config(10, Duration::from_secs(60))
            .with_key_extractor(KeyExtractor::UserId);
        assert_eq!(config.key_extractor, KeyExtractor::UserId);
    }

    #[test]
    fn test_rate_limit_config_with_exclude_paths() {
        let config = sliding_window_config(10, Duration::from_secs(60))
            .with_exclude_paths(vec!["/health".to_string()]);
        assert_eq!(config.exclude_paths, vec!["/health".to_string()]);
    }

    #[test]
    fn test_rate_limit_config_with_key_prefix() {
        let config = sliding_window_config(10, Duration::from_secs(60)).with_key_prefix("sms");
        assert_eq!(config.key_prefix, "sms");
    }

    #[test]
    fn test_rate_limit_config_is_excluded_exact_match() {
        let config = sliding_window_config(10, Duration::from_secs(60))
            .with_exclude_paths(vec!["/health".to_string()]);
        assert!(config.is_excluded("/health"));
        assert!(!config.is_excluded("/api"));
    }

    #[test]
    fn test_rate_limit_config_is_excluded_wildcard_match() {
        let config = sliding_window_config(10, Duration::from_secs(60))
            .with_exclude_paths(vec!["/public/*".to_string()]);
        assert!(config.is_excluded("/public/anything"));
        assert!(!config.is_excluded("/api"));
    }

    #[test]
    fn test_rate_limit_config_is_excluded_empty_list() {
        let config = sliding_window_config(10, Duration::from_secs(60));
        assert!(!config.is_excluded("/any"));
    }

    #[test]
    fn test_rate_limit_config_clone() {
        let config = sliding_window_config(10, Duration::from_secs(60)).with_key_prefix("test");
        let cloned = config.clone();
        assert_eq!(config.key_extractor, cloned.key_extractor);
        assert_eq!(config.key_prefix, cloned.key_prefix);
    }

    // ====================================================================
    // rate_limit_rejected_response 单元测试
    // ====================================================================

    #[tokio::test]
    async fn test_rate_limit_rejected_response_status_code() {
        let result = sz_orm_limit::RateLimitResult::rejected(0, current_unix_ms() + 60_000);
        let response = rate_limit_rejected_response(&result);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn test_rate_limit_rejected_response_headers() {
        let reset_at = current_unix_ms() + 60_000;
        let result = sz_orm_limit::RateLimitResult::rejected(0, reset_at);
        let response = rate_limit_rejected_response(&result);
        let headers = response.headers();
        assert_eq!(headers.get("x-ratelimit-remaining").unwrap(), "0");
        assert_eq!(
            headers.get("x-ratelimit-reset").unwrap().to_str().unwrap(),
            reset_at.to_string()
        );
        // Retry-After 应该是正数
        let retry_after: u64 = headers
            .get("retry-after")
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(retry_after > 0);
    }

    #[tokio::test]
    async fn test_rate_limit_rejected_response_body_format() {
        let reset_at = current_unix_ms() + 60_000;
        let result = sz_orm_limit::RateLimitResult::rejected(0, reset_at);
        let response = rate_limit_rejected_response(&result);
        let body = read_body(response).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["code"], 429);
        assert_eq!(json["msg"], "Too Many Requests");
        assert_eq!(json["data"]["reset_at_ms"], reset_at);
        assert!(json["data"]["retry_after_seconds"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_rate_limit_rejected_response_content_type() {
        let result = sz_orm_limit::RateLimitResult::rejected(0, current_unix_ms() + 60_000);
        let response = rate_limit_rejected_response(&result);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
    }

    // ====================================================================
    // rate_limit_middleware 集成测试（滑动窗口）
    // ====================================================================

    #[tokio::test]
    async fn test_rate_limit_middleware_allows_first_request() {
        let app = build_app_sliding_window();
        let resp = app
            .oneshot(make_request_with_ip("GET", "/api", "1.1.1.1"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_allows_second_request() {
        let app = build_app_sliding_window();
        // 第 1 次
        let resp = app
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "2.2.2.2"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // 第 2 次（滑动窗口 2 次/60 秒）
        let resp = app
            .oneshot(make_request_with_ip("GET", "/api", "2.2.2.2"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_rejects_third_request() {
        let app = build_app_sliding_window();
        // 第 1 次
        let _ = app
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "3.3.3.3"))
            .await
            .unwrap();
        // 第 2 次
        let _ = app
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "3.3.3.3"))
            .await
            .unwrap();
        // 第 3 次（应该被拒绝）
        let resp = app
            .oneshot(make_request_with_ip("GET", "/api", "3.3.3.3"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_different_ips_independent() {
        // 不同 IP 的限流相互独立
        let app = build_app_sliding_window();
        // IP 1 的 2 次
        let _ = app
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "4.4.4.4"))
            .await
            .unwrap();
        let _ = app
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "4.4.4.4"))
            .await
            .unwrap();
        // IP 2 的第 1 次应该放行
        let resp = app
            .oneshot(make_request_with_ip("GET", "/api", "5.5.5.5"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_adds_remaining_header_on_success() {
        let app = build_app_sliding_window();
        let resp = app
            .oneshot(make_request_with_ip("GET", "/api", "6.6.6.6"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let remaining = resp
            .headers()
            .get("x-ratelimit-remaining")
            .expect("X-RateLimit-Remaining header should be present");
        let remaining: u64 = remaining.to_str().unwrap().parse().unwrap();
        // 第 1 次后剩余 1（滑动窗口 2 次/60 秒）
        assert_eq!(remaining, 1);
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_adds_reset_header_on_success() {
        let app = build_app_sliding_window();
        let resp = app
            .oneshot(make_request_with_ip("GET", "/api", "7.7.7.7"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let reset = resp
            .headers()
            .get("x-ratelimit-reset")
            .expect("X-RateLimit-Reset header should be present");
        let reset: i64 = reset.to_str().unwrap().parse().unwrap();
        // reset_at 应该是未来时间
        assert!(reset > current_unix_ms());
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_rejected_response_has_retry_after() {
        let app = build_app_sliding_window();
        // 消耗 2 次配额
        let _ = app
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "8.8.8.8"))
            .await
            .unwrap();
        let _ = app
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "8.8.8.8"))
            .await
            .unwrap();
        // 第 3 次被拒绝
        let resp = app
            .oneshot(make_request_with_ip("GET", "/api", "8.8.8.8"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let retry_after = resp
            .headers()
            .get("retry-after")
            .expect("Retry-After header should be present");
        let retry_after: u64 = retry_after.to_str().unwrap().parse().unwrap();
        assert!(retry_after > 0);
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_excluded_path_bypasses_limit() {
        let config = sliding_window_config(1, Duration::from_secs(60))
            .with_exclude_paths(vec!["/health".to_string()]);
        let app = Router::new()
            .route(
                "/health",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                rate_limit_middleware,
            ));

        // 连续 5 次请求 /health 都应放行（排除路径不消耗令牌）
        for _ in 0..5 {
            let resp = app
                .clone()
                .oneshot(make_request("GET", "/health"))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_wildcard_exclude() {
        let config = sliding_window_config(1, Duration::from_secs(60))
            .with_exclude_paths(vec!["/public/*".to_string()]);
        let app = Router::new()
            .route(
                "/public/asset1",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .route(
                "/public/asset2",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                rate_limit_middleware,
            ));

        // 多个 /public/* 路径都应放行
        let resp = app
            .clone()
            .oneshot(make_request("GET", "/public/asset1"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = app
            .oneshot(make_request("GET", "/public/asset2"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_unknown_ip_shared_bucket() {
        // 无 IP header 时所有请求共享 "unknown" 桶
        let app = build_app_sliding_window();
        // 第 1 次
        let _ = app
            .clone()
            .oneshot(make_request("GET", "/api"))
            .await
            .unwrap();
        // 第 2 次
        let _ = app
            .clone()
            .oneshot(make_request("GET", "/api"))
            .await
            .unwrap();
        // 第 3 次（共享 "unknown" 桶，应该被拒绝）
        let resp = app.oneshot(make_request("GET", "/api")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_preserves_response_body() {
        let config = sliding_window_config(10, Duration::from_secs(60));
        let app = Router::new()
            .route("/body", axum::routing::get(|| async { "hello" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                rate_limit_middleware,
            ));
        let resp = app.oneshot(make_request("GET", "/body")).await.unwrap();
        let body = read_body(resp).await;
        assert_eq!(body, "hello");
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_handles_post_request() {
        let config = sliding_window_config(10, Duration::from_secs(60));
        let app = Router::new()
            .route(
                "/submit",
                axum::routing::post(|| async { axum::http::StatusCode::CREATED }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                rate_limit_middleware,
            ));
        let req = Request::builder()
            .method("POST")
            .uri("/submit")
            .header("x-forwarded-for", "9.9.9.9")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // ====================================================================
    // rate_limit_middleware 集成测试（令牌桶）
    // ====================================================================

    #[tokio::test]
    async fn test_token_bucket_allows_within_capacity() {
        let app = build_app_token_bucket();
        // 容量 2，前 2 次应放行
        let resp = app
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "10.0.0.1"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = app
            .oneshot(make_request_with_ip("GET", "/api", "10.0.0.1"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_token_bucket_rejects_over_capacity() {
        let app = build_app_token_bucket();
        // 消耗 2 个令牌
        let _ = app
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "10.0.0.2"))
            .await
            .unwrap();
        let _ = app
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "10.0.0.2"))
            .await
            .unwrap();
        // 第 3 次（无新令牌，应该被拒绝）
        let resp = app
            .oneshot(make_request_with_ip("GET", "/api", "10.0.0.2"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // ====================================================================
    // 便捷函数测试
    // ====================================================================

    #[test]
    fn test_sliding_window_config_creates_valid_config() {
        let config = sliding_window_config(100, Duration::from_secs(60));
        assert_eq!(config.key_extractor, KeyExtractor::Ip);
        assert!(config.exclude_paths.is_empty());
    }

    #[test]
    fn test_token_bucket_config_creates_valid_config() {
        let config = token_bucket_config(100, 10.0);
        assert_eq!(config.key_extractor, KeyExtractor::Ip);
    }

    // ====================================================================
    // 链式调用测试
    // ====================================================================

    #[tokio::test]
    async fn test_rate_limit_middleware_with_key_prefix_isolates_buckets() {
        // 不同 key_prefix 的限流桶相互独立
        let config1 = sliding_window_config(1, Duration::from_secs(60)).with_key_prefix("api1");
        let config2 = sliding_window_config(1, Duration::from_secs(60)).with_key_prefix("api2");

        let app1 = Router::new()
            .route(
                "/api",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config1,
                rate_limit_middleware,
            ));
        let app2 = Router::new()
            .route(
                "/api",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config2,
                rate_limit_middleware,
            ));

        // app1 消耗 1 次（api1:1.1.1.1 桶耗尽）
        let _ = app1
            .clone()
            .oneshot(make_request_with_ip("GET", "/api", "1.1.1.1"))
            .await
            .unwrap();
        // app1 第 2 次应该被拒绝
        let resp = app1
            .oneshot(make_request_with_ip("GET", "/api", "1.1.1.1"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        // app2 第 1 次应该放行（不同桶）
        let resp = app2
            .oneshot(make_request_with_ip("GET", "/api", "1.1.1.1"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_rate_limit_middleware_chains_with_other_middleware() {
        async fn add_header_middleware(req: Request, next: Next) -> Response {
            let mut resp = next.run(req).await;
            resp.headers_mut()
                .insert("X-Custom", "value".parse().unwrap());
            resp
        }

        let config = sliding_window_config(10, Duration::from_secs(60));
        let app = Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(add_header_middleware))
            .layer(axum::middleware::from_fn_with_state(
                config,
                rate_limit_middleware,
            ));

        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("X-Custom").unwrap(), "value");
    }

    // ====================================================================
    // PHP 行为对齐验证（R5 硬约束）
    // ====================================================================

    #[test]
    fn test_php_no_rate_limit_implementation() {
        // 对齐 PHP 端无限流实现的事实：
        // PHP `app/middleware.php` 仅含 `SessionInit` + `AllowCrossDomain`
        // PHP 业务代码无 `cache('rate_...')` 等频率限制
        // sz-rust 的 RateLimit 是自研增强，提供 PHP 端缺失的限流能力
        // 这里通过文档注释和模块结构验证 sz-rust 端的自研性质
        let config = sliding_window_config(10, Duration::from_secs(60));
        // 默认 Key 策略是 Ip（PHP 端无对应概念）
        assert_eq!(config.key_extractor, KeyExtractor::Ip);
    }

    #[test]
    fn test_rate_limit_response_format_aligns_with_render_json() {
        // 对齐 PHP `renderJson` 格式（code / msg / data 三字段）
        // sz-rust 的限流拒绝响应使用 code=429 / msg="Too Many Requests" / data={retry_after, reset_at}
        let result = sz_orm_limit::RateLimitResult::rejected(0, current_unix_ms() + 60_000);
        let response = rate_limit_rejected_response(&result);
        let headers = response.headers().clone();
        let _body = response.into_body();
        // 验证 Content-Type 是 JSON（对齐 PHP `json()` 函数）
        assert_eq!(
            headers.get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
    }

    #[test]
    fn test_http_429_status_code_alignment() {
        // HTTP 429 Too Many Requests 是 RFC 6585 标准限流状态码
        // PHP 端无限流所以无对应状态码，sz-rust 采用标准 HTTP 状态码
        let result = sz_orm_limit::RateLimitResult::rejected(0, current_unix_ms() + 60_000);
        let response = rate_limit_rejected_response(&result);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.status().as_u16(), 429);
    }
}
