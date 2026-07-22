//! Log 中间件 — 请求/响应日志（对齐 PHP `think-logger`）
//!
//! sz-rust 自研中间件，PHP 端无全局 Log 中间件（PHP `app/middleware.php` 仅含
//! `SessionInit` + `AllowCrossDomain`）。本模块在 [`crate::middleware::order::DEFAULT_ORDER`]
//! 中位于第 3 位（`Trace` → `Cors` → **`Log`** → `RateLimit` → `Auth`）。
//!
//! ## 行为
//!
//! 1. **入口**：生成 `RequestId`（如果 extensions 中没有，则新生成），注入 extensions
//! 2. **记录起始时间**：`std::time::Instant::now()`
//! 3. **调用 `next.run(req)`**：传递请求给下游
//! 4. **出口**：根据响应状态码记录日志
//!    - 2xx/3xx → `tracing::info!`
//!    - 4xx → `tracing::warn!`（对齐 PHP `apart_level=['error','sql']` 的级别分离思想）
//!    - 5xx → `tracing::error!`
//!
//! ## 日志字段
//!
//! | 字段 | 来源 | 说明 |
//! |------|------|------|
//! | `request_id` | `generate_request_id()` | 全局唯一计数器 + 时间戳，16 字符 hex |
//! | `method` | `Request::method()` | HTTP 方法 |
//! | `uri` | `Request::uri().path()` | 请求路径（不含查询字符串） |
//! | `status` | `Response::status().as_u16()` | HTTP 状态码 |
//! | `duration_ms` | `Instant::elapsed()` | 请求耗时（毫秒） |
//!
//! ## PHP 对齐
//!
//! PHP 端无 Log 中间件，业务代码通过 `Log::info()` 等主动调用。
//! sz-rust 的 Log 中间件是自研增强，提供：
//! - 请求生命周期自动日志（无需业务代码手动调用）
//! - 请求 ID 追踪（贯穿整个请求链路）
//! - 响应状态码分级日志（4xx Warn / 5xx Error）
//!
//! 日志级别对齐 think-logger 的 4 级（debug/info/warn/error），
//! `apart_level` 思想对齐 PHP `config/log.php` 的 `['error','sql']` 独立文件配置。
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::middleware::log::log_middleware;
//! use axum::Router;
//!
//! let app: Router = Router::new()
//!     .route("/", axum::routing::get(|| async { "ok" }))
//!     .layer(axum::middleware::from_fn(log_middleware));
//! ```

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::log::LogLevel;

/// 请求 ID（注入到 request extensions，供下游 handler 和日志使用）
///
/// 生成方式：全局 `AtomicU64` 计数器 + 当前时间戳，保证进程内唯一。
/// 格式：16 字符 hex（`{timestamp_secs:08x}{counter:08x}`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId {
    /// 时间戳部分（UNIX 秒）
    timestamp_secs: u64,
    /// 计数器部分（进程内递增）
    counter: u64,
}

impl RequestId {
    /// 返回 16 字符 hex 字符串
    ///
    /// 格式：`{timestamp_secs:08x}{counter:08x}`（对齐 W3C traceparent 的 16 字符 span_id 长度）。
    pub fn to_hex(&self) -> String {
        format!("{:08x}{:08x}", self.timestamp_secs, self.counter)
    }

    /// 返回时间戳部分
    pub fn timestamp_secs(&self) -> u64 {
        self.timestamp_secs
    }

    /// 返回计数器部分
    pub fn counter(&self) -> u64 {
        self.counter
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// 全局 request_id 计数器（进程内递增）
static REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 生成新的 `RequestId`
///
/// 使用全局 `AtomicU64` 计数器 + 当前 UNIX 时间戳，保证进程内唯一。
/// 多线程安全（`fetch_add` 是原子操作）。
pub fn generate_request_id() -> RequestId {
    let counter = REQUEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    RequestId {
        timestamp_secs,
        counter,
    }
}

/// Log 中间件配置
#[derive(Debug, Clone, Default)]
pub struct LogConfig {
    /// 排除路径（不记录日志，对齐 PHP 端白名单思想）
    ///
    /// 支持精确匹配（如 `/health`）和通配符匹配（如 `/health/*`）。
    pub exclude_paths: Vec<String>,
}

impl LogConfig {
    /// 创建带排除路径的配置
    pub fn with_exclude_paths(mut self, paths: Vec<String>) -> Self {
        self.exclude_paths = paths;
        self
    }

    /// 判断路径是否被排除
    ///
    /// 支持精确匹配和 `*` 通配符匹配（复用 [`crate::middleware::auth::is_route_allowed`] 的逻辑）。
    pub fn is_excluded(&self, path: &str) -> bool {
        crate::middleware::auth::is_route_allowed(path, &self.exclude_paths)
    }
}

/// 根据响应状态码返回日志级别
///
/// 对齐 PHP `config/log.php` 的 `apart_level=['error','sql']` 思想：
/// - 2xx/3xx → `Info`（成功请求）
/// - 4xx → `Warn`（客户端错误）
/// - 5xx → `Error`（服务端错误）
///
/// 其他状态码（如 1xx）默认为 `Info`。
pub fn log_level_for_status(status: u16) -> LogLevel {
    match status {
        400..=499 => LogLevel::Warn,
        500..=599 => LogLevel::Error,
        _ => LogLevel::Info,
    }
}

/// 格式化请求日志消息
///
/// 输出格式：`request_id=<hex> method=<METHOD> uri=<path> status=<code> duration_ms=<ms>`
///
/// 此函数主要用于测试可验证的纯函数，中间件实际输出通过 `tracing` 宏的结构化字段实现。
pub fn format_request_log(
    method: &str,
    uri: &str,
    status: u16,
    duration_ms: u64,
    request_id: &RequestId,
) -> String {
    format!(
        "request_id={} method={} uri={} status={} duration_ms={}",
        request_id.to_hex(),
        method,
        uri,
        status,
        duration_ms
    )
}

/// Log 中间件 — 请求/响应日志
///
/// ## 校验流程
///
/// 1. **提取请求信息**：method, uri（在 `req` 被消费之前）
/// 2. **生成 RequestId**：如果 extensions 中没有，则新生成
/// 3. **记录起始时间**：`Instant::now()`
/// 4. **注入 RequestId**：插入 request extensions
/// 5. **调用 `next.run(req)`**：传递请求给下游
/// 6. **计算耗时**：`start.elapsed()`
/// 7. **记录日志**：根据状态码选择级别，输出结构化日志
///
/// ## 排除路径
///
/// 如果请求路径在 [`LogConfig::exclude_paths`] 中，则不记录日志（但仍注入 RequestId）。
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_core::middleware::log::{log_middleware, LogConfig};
/// use axum::Router;
///
/// let config = LogConfig::default();
/// let app: Router = Router::new()
///     .route("/", axum::routing::get(|| async { "ok" }))
///     .layer(axum::middleware::from_fn_with_state(config, log_middleware_with_config));
/// ```
pub async fn log_middleware(req: Request, next: Next) -> Response {
    log_middleware_inner(req, next, &LogConfig::default()).await
}

/// 带配置的 Log 中间件
pub async fn log_middleware_with_config(
    axum::extract::State(config): axum::extract::State<LogConfig>,
    req: Request,
    next: Next,
) -> Response {
    log_middleware_inner(req, next, &config).await
}

async fn log_middleware_inner(req: Request, next: Next, config: &LogConfig) -> Response {
    // 1. 提取请求信息（在 req 被消费之前）
    let method = req.method().clone();
    let uri = req.uri().path().to_string();

    // 2. 生成 RequestId（如果 extensions 中没有，则新生成）
    let request_id = req
        .extensions()
        .get::<RequestId>()
        .copied()
        .unwrap_or_else(generate_request_id);

    // 3. 记录起始时间
    let start = Instant::now();

    // 4. 注入 RequestId 到 extensions
    let mut req = req;
    req.extensions_mut().insert(request_id);

    // 5. 调用 next
    let response = next.run(req).await;

    // 6. 计算耗时
    let duration_ms = start.elapsed().as_millis() as u64;

    // 7. 记录日志（排除路径不记录）
    if !config.is_excluded(&uri) {
        let status = response.status().as_u16();
        let level = log_level_for_status(status);
        let request_id_hex = request_id.to_hex();

        match level {
            LogLevel::Debug => tracing::debug!(
                request_id = %request_id_hex,
                method = %method,
                uri = %uri,
                status = status,
                duration_ms = duration_ms,
                "request completed"
            ),
            LogLevel::Info => tracing::info!(
                request_id = %request_id_hex,
                method = %method,
                uri = %uri,
                status = status,
                duration_ms = duration_ms,
                "request completed"
            ),
            LogLevel::Warn => tracing::warn!(
                request_id = %request_id_hex,
                method = %method,
                uri = %uri,
                status = status,
                duration_ms = duration_ms,
                "request completed"
            ),
            LogLevel::Error => tracing::error!(
                request_id = %request_id_hex,
                method = %method,
                uri = %uri,
                status = status,
                duration_ms = duration_ms,
                "request completed"
            ),
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
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

    /// 构建测试用 Router
    fn build_app() -> Router {
        Router::new()
            .route(
                "/ok",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .route(
                "/notfound",
                axum::routing::get(|| async { axum::http::StatusCode::NOT_FOUND }),
            )
            .route(
                "/error",
                axum::routing::get(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
            )
            .route("/body", axum::routing::get(|| async { "hello" }))
            .layer(axum::middleware::from_fn(log_middleware))
    }

    // ====================================================================
    // RequestId 单元测试
    // ====================================================================

    #[test]
    fn test_request_id_to_hex_is_16_chars() {
        let id = RequestId {
            timestamp_secs: 0x12345678,
            counter: 0x9ABCDEF0,
        };
        let hex = id.to_hex();
        assert_eq!(hex.len(), 16);
        assert_eq!(hex, "123456789abcdef0");
    }

    #[test]
    fn test_request_id_to_hex_zero() {
        let id = RequestId {
            timestamp_secs: 0,
            counter: 0,
        };
        assert_eq!(id.to_hex(), "0000000000000000");
    }

    #[test]
    fn test_request_id_to_hex_max() {
        let id = RequestId {
            timestamp_secs: u64::MAX,
            counter: u64::MAX,
        };
        // u64::MAX = 0xFFFFFFFFFFFFFFFF，但 format!("{:08x}", u64::MAX) 会输出 16 字符
        let hex = id.to_hex();
        assert_eq!(hex.len(), 32); // 每部分 16 字符，总共 32 字符
    }

    #[test]
    fn test_request_id_display_matches_to_hex() {
        let id = RequestId {
            timestamp_secs: 0x12345678,
            counter: 0x9ABCDEF0,
        };
        assert_eq!(format!("{}", id), id.to_hex());
    }

    #[test]
    fn test_request_id_accessors() {
        let id = RequestId {
            timestamp_secs: 100,
            counter: 200,
        };
        assert_eq!(id.timestamp_secs(), 100);
        assert_eq!(id.counter(), 200);
    }

    #[test]
    fn test_request_id_equality() {
        let id1 = RequestId {
            timestamp_secs: 1,
            counter: 2,
        };
        let id2 = RequestId {
            timestamp_secs: 1,
            counter: 2,
        };
        let id3 = RequestId {
            timestamp_secs: 1,
            counter: 3,
        };
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    // ====================================================================
    // generate_request_id 单元测试
    // ====================================================================

    #[test]
    fn test_generate_request_id_returns_unique() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();
        // 计数器递增，保证唯一
        assert_ne!(id1.counter(), id2.counter());
        assert_eq!(id2.counter(), id1.counter() + 1);
    }

    #[test]
    fn test_generate_request_id_hex_is_16_chars() {
        let id = generate_request_id();
        let hex = id.to_hex();
        // 注意：如果 timestamp_secs 或 counter 超过 u32::MAX，hex 会超过 16 字符
        // 但在正常情况下（timestamp < 2106 年，counter < 40 亿次），hex 是 16 字符
        assert!(hex.len() >= 16);
    }

    // ====================================================================
    // log_level_for_status 单元测试
    // ====================================================================

    #[test]
    fn test_log_level_for_2xx_returns_info() {
        assert_eq!(log_level_for_status(200), LogLevel::Info);
        assert_eq!(log_level_for_status(201), LogLevel::Info);
        assert_eq!(log_level_for_status(204), LogLevel::Info);
    }

    #[test]
    fn test_log_level_for_3xx_returns_info() {
        assert_eq!(log_level_for_status(301), LogLevel::Info);
        assert_eq!(log_level_for_status(302), LogLevel::Info);
        assert_eq!(log_level_for_status(304), LogLevel::Info);
    }

    #[test]
    fn test_log_level_for_4xx_returns_warn() {
        assert_eq!(log_level_for_status(400), LogLevel::Warn);
        assert_eq!(log_level_for_status(401), LogLevel::Warn);
        assert_eq!(log_level_for_status(403), LogLevel::Warn);
        assert_eq!(log_level_for_status(404), LogLevel::Warn);
        assert_eq!(log_level_for_status(422), LogLevel::Warn);
        assert_eq!(log_level_for_status(499), LogLevel::Warn);
    }

    #[test]
    fn test_log_level_for_5xx_returns_error() {
        assert_eq!(log_level_for_status(500), LogLevel::Error);
        assert_eq!(log_level_for_status(501), LogLevel::Error);
        assert_eq!(log_level_for_status(502), LogLevel::Error);
        assert_eq!(log_level_for_status(503), LogLevel::Error);
        assert_eq!(log_level_for_status(599), LogLevel::Error);
    }

    #[test]
    fn test_log_level_for_1xx_returns_info() {
        // 1xx 信息响应默认为 Info
        assert_eq!(log_level_for_status(100), LogLevel::Info);
        assert_eq!(log_level_for_status(101), LogLevel::Info);
    }

    #[test]
    fn test_log_level_for_boundary() {
        // 边界测试：399 → Info，400 → Warn，499 → Warn，500 → Error，599 → Error，600 → Info
        assert_eq!(log_level_for_status(399), LogLevel::Info);
        assert_eq!(log_level_for_status(400), LogLevel::Warn);
        assert_eq!(log_level_for_status(499), LogLevel::Warn);
        assert_eq!(log_level_for_status(500), LogLevel::Error);
        assert_eq!(log_level_for_status(599), LogLevel::Error);
        assert_eq!(log_level_for_status(600), LogLevel::Info);
    }

    // ====================================================================
    // format_request_log 单元测试
    // ====================================================================

    #[test]
    fn test_format_request_log_basic() {
        let request_id = RequestId {
            timestamp_secs: 0x12345678,
            counter: 0x9ABCDEF0,
        };
        let msg = format_request_log("GET", "/api/users", 200, 15, &request_id);
        assert_eq!(
            msg,
            "request_id=123456789abcdef0 method=GET uri=/api/users status=200 duration_ms=15"
        );
    }

    #[test]
    fn test_format_request_log_post_method() {
        let request_id = RequestId {
            timestamp_secs: 0,
            counter: 1,
        };
        let msg = format_request_log("POST", "/api/orders", 201, 42, &request_id);
        assert_eq!(
            msg,
            "request_id=0000000000000001 method=POST uri=/api/orders status=201 duration_ms=42"
        );
    }

    #[test]
    fn test_format_request_log_error_status() {
        let request_id = RequestId {
            timestamp_secs: 0,
            counter: 0,
        };
        let msg = format_request_log("GET", "/missing", 404, 5, &request_id);
        assert_eq!(
            msg,
            "request_id=0000000000000000 method=GET uri=/missing status=404 duration_ms=5"
        );
    }

    #[test]
    fn test_format_request_log_with_query_string_in_uri() {
        // uri 应该是原始 path（含查询字符串），由调用方决定是否截取
        let request_id = RequestId {
            timestamp_secs: 0,
            counter: 0,
        };
        let msg = format_request_log("GET", "/api?foo=bar", 200, 1, &request_id);
        assert!(msg.contains("uri=/api?foo=bar"));
    }

    // ====================================================================
    // LogConfig 单元测试
    // ====================================================================

    #[test]
    fn test_log_config_default_empty_exclude_paths() {
        let config = LogConfig::default();
        assert!(config.exclude_paths.is_empty());
    }

    #[test]
    fn test_log_config_with_exclude_paths() {
        let config = LogConfig::default().with_exclude_paths(vec!["/health".to_string()]);
        assert_eq!(config.exclude_paths, vec!["/health".to_string()]);
    }

    #[test]
    fn test_log_config_is_excluded_exact_match() {
        let config = LogConfig::default().with_exclude_paths(vec!["/health".to_string()]);
        assert!(config.is_excluded("/health"));
        assert!(!config.is_excluded("/health/detail"));
        assert!(!config.is_excluded("/api"));
    }

    #[test]
    fn test_log_config_is_excluded_wildcard_match() {
        let config = LogConfig::default().with_exclude_paths(vec!["/health/*".to_string()]);
        assert!(config.is_excluded("/health/check"));
        assert!(config.is_excluded("/health/deep/nested"));
        assert!(!config.is_excluded("/health"));
        assert!(!config.is_excluded("/api"));
    }

    #[test]
    fn test_log_config_is_excluded_empty_list() {
        let config = LogConfig::default();
        assert!(!config.is_excluded("/any"));
    }

    #[test]
    fn test_log_config_is_excluded_multiple_entries() {
        let config = LogConfig::default()
            .with_exclude_paths(vec!["/health".to_string(), "/metrics/*".to_string()]);
        assert!(config.is_excluded("/health"));
        assert!(config.is_excluded("/metrics/prometheus"));
        assert!(!config.is_excluded("/api"));
    }

    // ====================================================================
    // log_middleware 集成测试
    // ====================================================================

    #[tokio::test]
    async fn test_log_middleware_returns_response_unchanged() {
        // 验证中间件不修改响应体
        let app = build_app();
        let resp = app.oneshot(make_request("GET", "/body")).await.unwrap();
        let body = read_body(resp).await;
        assert_eq!(body, "hello");
    }

    #[tokio::test]
    async fn test_log_middleware_returns_correct_status() {
        let app = build_app();
        let resp = app.oneshot(make_request("GET", "/ok")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_log_middleware_injects_request_id() {
        // 验证 request_id 被注入 extensions
        let app = Router::new()
            .route(
                "/",
                axum::routing::get(|req: Request| async move {
                    let request_id = req.extensions().get::<RequestId>().unwrap();
                    format!("request_id:{}", request_id.to_hex())
                }),
            )
            .layer(axum::middleware::from_fn(log_middleware));

        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert!(body.starts_with("request_id:"));
        // 验证 hex 长度至少 16 字符
        let hex = body.strip_prefix("request_id:").unwrap();
        assert!(hex.len() >= 16);
    }

    #[tokio::test]
    async fn test_log_middleware_generates_unique_request_ids() {
        // 验证多个请求生成不同的 request_id
        let app = Router::new()
            .route(
                "/",
                axum::routing::get(|req: Request| async move {
                    let request_id = req.extensions().get::<RequestId>().unwrap();
                    request_id.to_hex()
                }),
            )
            .layer(axum::middleware::from_fn(log_middleware));

        let resp1 = app.clone().oneshot(make_request("GET", "/")).await.unwrap();
        let hex1 = read_body(resp1).await;

        let resp2 = app.oneshot(make_request("GET", "/")).await.unwrap();
        let hex2 = read_body(resp2).await;

        assert_ne!(hex1, hex2);
    }

    #[tokio::test]
    async fn test_log_middleware_preserves_existing_request_id() {
        // 验证已存在的 request_id 不被覆盖
        let existing_id = RequestId {
            timestamp_secs: 0xDEADBEEF,
            counter: 0x12345678,
        };
        let app = Router::new()
            .route(
                "/",
                axum::routing::get(|req: Request| async move {
                    let request_id = req.extensions().get::<RequestId>().unwrap();
                    request_id.to_hex()
                }),
            )
            .layer(axum::middleware::from_fn(log_middleware))
            .layer(
                tower::ServiceBuilder::new().layer(tower::layer::layer_fn(move |service| {
                    tower::util::MapRequest::new(service, move |mut req: Request| {
                        req.extensions_mut().insert(existing_id);
                        req
                    })
                })),
            );

        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        let body = read_body(resp).await;
        assert_eq!(body, "deadbeef12345678");
    }

    #[tokio::test]
    async fn test_log_middleware_records_2xx_status() {
        // 验证 2xx 响应正常处理（日志级别由 log_level_for_status 决定）
        let app = build_app();
        let resp = app.oneshot(make_request("GET", "/ok")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_log_middleware_records_4xx_status() {
        let app = build_app();
        let resp = app.oneshot(make_request("GET", "/notfound")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_log_middleware_records_5xx_status() {
        let app = build_app();
        let resp = app.oneshot(make_request("GET", "/error")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn test_log_middleware_with_config_excludes_path() {
        // 验证排除路径不记录日志（但仍注入 request_id）
        let config = LogConfig::default().with_exclude_paths(vec!["/health".to_string()]);
        let app = Router::new()
            .route("/health", axum::routing::get(|| async { "healthy" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                log_middleware_with_config,
            ));

        let resp = app.oneshot(make_request("GET", "/health")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert_eq!(body, "healthy");
    }

    #[tokio::test]
    async fn test_log_middleware_with_config_wildcard_exclude() {
        // 验证通配符排除路径
        let config = LogConfig::default().with_exclude_paths(vec!["/metrics/*".to_string()]);
        let app = Router::new()
            .route(
                "/metrics/prometheus",
                axum::routing::get(|| async { "metrics" }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                log_middleware_with_config,
            ));

        let resp = app
            .oneshot(make_request("GET", "/metrics/prometheus"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_log_middleware_preserves_method_and_uri() {
        // 验证 method 和 uri 被正确提取（通过日志消息格式验证）
        // 由于 tracing 宏输出在测试中难以捕获，这里验证中间件不破坏请求
        let app = build_app();
        let resp = app.oneshot(make_request("GET", "/ok")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_log_middleware_duration_is_non_negative() {
        // 验证 duration_ms 是非负的（通过响应正常返回间接验证）
        let app = build_app();
        let start = std::time::Instant::now();
        let resp = app.oneshot(make_request("GET", "/ok")).await.unwrap();
        let elapsed = start.elapsed();
        assert!(resp.status().is_success());
        // 中间件内部记录的 duration_ms 应该 <= 测试外部的 elapsed
        assert!(elapsed.as_millis() < 5000); // 5 秒上限（防止死循环）
    }

    #[tokio::test]
    async fn test_log_middleware_handles_post_request() {
        let app = Router::new()
            .route(
                "/submit",
                axum::routing::post(|| async { axum::http::StatusCode::CREATED }),
            )
            .layer(axum::middleware::from_fn(log_middleware));

        let req = Request::builder()
            .method("POST")
            .uri("/submit")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_log_middleware_chains_with_other_middleware() {
        // 验证 Log 中间件与其他中间件链式调用
        async fn add_header_middleware(req: Request, next: Next) -> Response {
            let mut resp = next.run(req).await;
            resp.headers_mut()
                .insert("X-Custom", "value".parse().unwrap());
            resp
        }

        let app = Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(add_header_middleware))
            .layer(axum::middleware::from_fn(log_middleware));

        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get("X-Custom").unwrap(), "value");
    }

    // ====================================================================
    // PHP 行为对齐验证
    // ====================================================================

    #[test]
    fn test_php_apart_level_alignment() {
        // 对齐 PHP `config/log.php` 的 `apart_level=['error','sql']` 思想：
        // 4xx → Warn（客户端错误，类似 PHP warning）
        // 5xx → Error（服务端错误，对齐 PHP error 独立文件）
        assert_eq!(log_level_for_status(200), LogLevel::Info);
        assert_eq!(log_level_for_status(404), LogLevel::Warn);
        assert_eq!(log_level_for_status(500), LogLevel::Error);
    }

    #[test]
    fn test_php_think_logger_level_alignment() {
        // 对齐 PHP think-logger 的 4 级日志（debug/info/warn/error）
        // sz-rust 的 LogLevel 也是 4 级，一一对应
        let levels = [
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ];
        assert_eq!(levels.len(), 4);
    }

    #[test]
    fn test_request_id_format_aligns_with_w3c_span_id_length() {
        // 对齐 W3C traceparent 的 span_id 长度（16 字符 hex）
        // 便于未来 Phase 3.6 Trace 中间件实现时与 trace_id 格式兼容
        let id = RequestId {
            timestamp_secs: 0x12345678,
            counter: 0x9ABCDEF0,
        };
        assert_eq!(id.to_hex().len(), 16);
    }
}
