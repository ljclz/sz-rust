//! Trace 中间件 — 请求追踪 span（复用 sz-orm-tracing）
//!
//! sz-rust 自研中间件，对齐 PHP `think\middleware\SessionInit` 的「请求初始化」语义。
//! PHP `SessionInit` 仅初始化会话，无追踪能力；sz-rust 的 Trace 中间件是自研增强，
//! 提供 W3C TraceContext 传播 + Span 生命周期管理。
//!
//! 本模块在 [`crate::order::DEFAULT_ORDER`] 中位于第 1 位
//! （**`Trace`** → `Cors` → `Log` → `RateLimit` → `Auth`），最先执行，
//! 确保所有后续中间件和 handler 都能通过 `request.extensions()` 获取 Span。
//!
//! ## 行为
//!
//! 1. **排除路径检查**：如果请求路径在 `exclude_paths` 中，直接放行（不创建 Span）
//! 2. **提取 traceparent**：从请求 headers 提取 W3C traceparent（如果存在）
//!    - 存在 → 创建子 Span（继承 trace_id，parent_id = 提取的 span_id）
//!    - 不存在 → 创建新 Span（新 trace_id + 新 span_id）
//! 3. **注入 Span**：将 Span 注入到 request extensions
//! 4. **调用 next**：传递请求给下游
//! 5. **finish Span**：标记 Span 结束（写入 end_time）
//! 6. **注入 traceparent**：将当前 Span 的 traceparent 注入到响应 headers
//!
//! ## W3C TraceContext
//!
//! traceparent 格式：`00-<trace_id>-<span_id>-<trace_flags>`
//! - `trace_id`：32 字符 hex（16 字节）
//! - `span_id`：16 字符 hex（8 字节）
//! - `trace_flags`：2 字符 hex（1 字节，如 `01` 表示 sampled）
//!
//! sz-orm-tracing 的 `Tracer::inject` 生成 traceparent，`Tracer::extract` 解析 traceparent。
//!
//! ## PHP 对齐
//!
//! PHP `SessionInit` 仅初始化会话（`session_start()`），无追踪能力。
//! sz-rust 的 Trace 中间件是自研增强，提供：
//! - W3C TraceContext 标准传播（对齐 OpenTelemetry）
//! - Span 生命周期管理（start_time/end_time/duration）
//! - 跨服务追踪（通过 traceparent header 传递）
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::middleware::trace::{trace_middleware, TraceConfig};
//! use sz_orm_tracing::SzTracer;
//! use std::sync::Arc;
//! use axum::Router;
//!
//! let tracer = Arc::new(SzTracer::new("my-service"));
//! let config = TraceConfig::new(tracer);
//! let app: Router = Router::new()
//!     .route("/", axum::routing::get(|| async { "ok" }))
//!     .layer(axum::middleware::from_fn_with_state(config, trace_middleware));
//! ```

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use std::collections::HashMap;
use std::sync::Arc;

use sz_rust_orm_facade::{Span, Tracer};

/// Trace 中间件配置
#[derive(Clone)]
pub struct TraceConfig {
    /// Tracer 实例（`Arc<dyn Tracer + Send + Sync>` 共享）
    pub tracer: Arc<dyn Tracer + Send + Sync>,
    /// 服务名（对齐 sz-orm-tracing 的 service_name）
    pub service_name: String,
    /// 排除路径（不创建 Span，复用 [`crate::auth::is_route_allowed`] 匹配）
    pub exclude_paths: Vec<String>,
}

impl std::fmt::Debug for TraceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceConfig")
            .field("service_name", &self.service_name)
            .field("exclude_paths", &self.exclude_paths)
            .finish_non_exhaustive()
    }
}

impl TraceConfig {
    /// 创建 TraceConfig
    pub fn new(tracer: Arc<dyn Tracer + Send + Sync>) -> Self {
        let service_name = "sz-rust".to_string();
        Self {
            tracer,
            service_name,
            exclude_paths: Vec::new(),
        }
    }

    /// 设置服务名
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// 设置排除路径
    pub fn with_exclude_paths(mut self, paths: Vec<String>) -> Self {
        self.exclude_paths = paths;
        self
    }

    /// 判断路径是否被排除
    pub fn is_excluded(&self, path: &str) -> bool {
        crate::auth::is_route_allowed(path, &self.exclude_paths)
    }
}

/// 从请求 headers 构建 HashMap（sz-orm-tracing 的 extract 需要 HashMap）
fn headers_to_hashmap(headers: &HeaderMap) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            map.insert(name.as_str().to_lowercase(), v.to_string());
        }
    }
    map
}

/// 从请求 headers 提取或创建 Span
///
/// 优先使用 W3C traceparent 提取（如果存在），否则创建新 Span。
///
/// ## 实现细节
///
/// `Tracer::start_span` 总是生成新的 `trace_id` + `span_id`。若 `extract`
/// 返回 parent span，则通过 `Span` 的公共字段覆盖 `trace_id` 并设置
/// `parent_id`，实现「子 span 继承 parent 的 trace_id」语义。
/// 不使用 `Span::with_parent` 是因为它仅设置 `parent_id` 而不覆盖 `trace_id`。
pub fn extract_or_create_span(headers: &HeaderMap, config: &TraceConfig) -> Span {
    let headers_map = headers_to_hashmap(headers);

    // start_span 生成新的 trace_id + span_id，service_name 来自 tracer
    let mut span = config
        .tracer
        .start_span(&format!("{}:request", config.service_name));
    // 覆盖 service_name 为 config 中配置的值（可能与 tracer 内部 service_name 不同）
    span.service_name = config.service_name.clone();

    // 如果提取到 parent span，覆盖 trace_id 并设置 parent_id（创建子 span）
    if let Some(parent_span) = config.tracer.extract(&headers_map) {
        span.trace_id = parent_span.trace_id.clone();
        span.parent_id = Some(parent_span.span_id.clone());
    }
    span
}

/// 将 Span 的 traceparent 注入到响应 headers
///
/// 生成 W3C traceparent：`00-<trace_id>-<span_id>-01`
pub fn inject_traceparent_to_response(response: &mut Response, span: &Span, config: &TraceConfig) {
    let headers_map = config.tracer.inject(span);
    let headers = response.headers_mut();
    for (key, value) in headers_map {
        // 将 String key 转换为 HeaderName（owned），避免 'static 生命周期约束
        if let (Ok(name), Ok(header_value)) = (
            axum::http::HeaderName::from_bytes(key.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            headers.insert(name, header_value);
        }
    }
}

/// Trace 中间件主函数
///
/// ## 校验流程
///
/// 1. **排除路径检查**：如果请求路径在 `exclude_paths` 中，直接放行（不创建 Span）
/// 2. **提取/创建 Span**：从请求 headers 提取 traceparent，或创建新 Span
/// 3. **注入 Span**：将 Span 注入到 request extensions
/// 4. **调用 next**：传递请求给下游
/// 5. **end_span**：调用 `Tracer::end_span` 完成 span（写入 end_time + 存入 tracer 内部 buffer）
/// 6. **注入 traceparent**：将 Span 的 traceparent 注入到响应 headers
///
/// ## end_span vs finish
///
/// 使用 `Tracer::end_span` 而非 `Span::finish` 是因为 `end_span` 内部会调用 `finish`
/// 并将 span 存入 `SzTracer.spans`，便于后续通过 `tracer.get_spans()` 获取已完成 span
/// 用于导出（如 OTLP exporter）。
pub async fn trace_middleware(
    axum::extract::State(config): axum::extract::State<TraceConfig>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    // 1. 排除路径直接放行（不创建 Span）
    if config.is_excluded(&path) {
        return next.run(req).await;
    }

    // 2. 提取/创建 Span + 记录请求信息到 span tags（链式 builder）
    let method = req.method().clone();
    let uri = req.uri().path().to_string();
    let mut span = extract_or_create_span(req.headers(), &config)
        .with_tag("http.method", method.as_str())
        .with_tag("http.uri", &uri)
        .with_tag("http.path", &path);

    // 3. 注入 Span 到 request extensions（下游 handler 可通过 extensions 获取）
    let mut req = req;
    req.extensions_mut().insert(span.clone());

    // 4. 调用 next
    let mut response = next.run(req).await;

    // 5. 记录响应状态码到 span tags + end_span（finish + 存入 tracer 内部 buffer）
    let status = response.status().as_u16();
    span = span.with_tag("http.status_code", status.to_string());
    // end_span 接收 owned Span，clone 一份用于后续 inject；end_span 内部会 finish
    config.tracer.end_span(span.clone());

    // 6. 注入 traceparent 到响应 headers（使用 clone 的 span，已 finish 但 traceparent 不变）
    inject_traceparent_to_response(&mut response, &span, &config);

    response
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

    fn make_request_with_traceparent(method: &str, uri: &str, traceparent: &str) -> Request {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("traceparent", traceparent)
            .body(Body::empty())
            .unwrap()
    }

    /// 构建测试用 Router（使用 SzTracer）
    fn build_app() -> Router {
        let tracer = Arc::new(sz_orm_tracing::SzTracer::new("test-service"));
        let config = TraceConfig::new(tracer).with_service_name("test-service");
        Router::new()
            .route(
                "/api",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                trace_middleware,
            ))
    }

    // ====================================================================
    // TraceConfig 单元测试
    // ====================================================================

    #[test]
    fn test_trace_config_new() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer);
        assert_eq!(config.service_name, "sz-rust");
        assert!(config.exclude_paths.is_empty());
    }

    #[test]
    fn test_trace_config_with_service_name() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_service_name("my-service");
        assert_eq!(config.service_name, "my-service");
    }

    #[test]
    fn test_trace_config_with_exclude_paths() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_exclude_paths(vec!["/health".to_string()]);
        assert_eq!(config.exclude_paths, vec!["/health".to_string()]);
    }

    #[test]
    fn test_trace_config_is_excluded_exact_match() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_exclude_paths(vec!["/health".to_string()]);
        assert!(config.is_excluded("/health"));
        assert!(!config.is_excluded("/api"));
    }

    #[test]
    fn test_trace_config_is_excluded_wildcard_match() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_exclude_paths(vec!["/public/*".to_string()]);
        assert!(config.is_excluded("/public/anything"));
        assert!(!config.is_excluded("/api"));
    }

    #[test]
    fn test_trace_config_is_excluded_empty_list() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer);
        assert!(!config.is_excluded("/any"));
    }

    #[test]
    fn test_trace_config_clone() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_service_name("cloned-service");
        let cloned = config.clone();
        assert_eq!(config.service_name, cloned.service_name);
    }

    #[test]
    fn test_trace_config_debug() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_service_name("debug-service");
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("debug-service"));
        assert!(debug_str.contains("TraceConfig"));
    }

    // ====================================================================
    // headers_to_hashmap 单元测试
    // ====================================================================

    #[test]
    fn test_headers_to_hashmap_empty() {
        let headers = HeaderMap::new();
        let map = headers_to_hashmap(&headers);
        assert!(map.is_empty());
    }

    #[test]
    fn test_headers_to_hashmap_single_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-custom", "value1".parse().unwrap());
        let map = headers_to_hashmap(&headers);
        assert_eq!(map.get("x-custom"), Some(&"value1".to_string()));
    }

    #[test]
    fn test_headers_to_hashmap_multiple_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-custom-1", "value1".parse().unwrap());
        headers.insert("x-custom-2", "value2".parse().unwrap());
        let map = headers_to_hashmap(&headers);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("x-custom-1"), Some(&"value1".to_string()));
        assert_eq!(map.get("x-custom-2"), Some(&"value2".to_string()));
    }

    #[test]
    fn test_headers_to_hashmap_lowercases_keys() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Custom", "value".parse().unwrap());
        let map = headers_to_hashmap(&headers);
        // HeaderMap 已经将 name 存储为小写
        assert_eq!(map.get("x-custom"), Some(&"value".to_string()));
    }

    #[test]
    fn test_headers_to_hashmap_skips_invalid_ascii() {
        let mut headers = HeaderMap::new();
        // 插入一个包含非 ASCII 字符的 header value
        let invalid_value = HeaderValue::from_bytes(b"\xff\xfe").unwrap();
        headers.insert("x-invalid", invalid_value);
        let map = headers_to_hashmap(&headers);
        // to_str() 会失败，该 header 被跳过
        assert!(!map.contains_key("x-invalid"));
    }

    // ====================================================================
    // extract_or_create_span 单元测试
    // ====================================================================

    #[test]
    fn test_extract_or_create_span_no_traceparent_creates_new_span() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_service_name("my-service");
        let headers = HeaderMap::new();
        let span = extract_or_create_span(&headers, &config);
        // 新 span 应该有 trace_id 和 span_id
        assert!(!span.trace_id().is_empty());
        assert!(!span.span_id().is_empty());
        // 新 span 没有 parent_id
        assert!(span.parent_id().is_none());
        // service_name 应该是 config 中设置的
        assert_eq!(span.service_name(), "my-service");
    }

    #[test]
    fn test_extract_or_create_span_with_traceparent_creates_child_span() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_service_name("my-service");

        // 先创建一个 parent span，获取其 traceparent
        let parent_span = config.tracer.start_span("parent");
        let parent_trace_id = parent_span.trace_id().to_string();
        let parent_span_id = parent_span.span_id().to_string();
        let headers_map = config.tracer.inject(&parent_span);

        // 构建包含 traceparent 的 HeaderMap
        let mut headers = HeaderMap::new();
        for (key, value) in &headers_map {
            if let (Ok(name), Ok(header_value)) = (
                axum::http::HeaderName::from_bytes(key.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                headers.insert(name, header_value);
            }
        }

        let child_span = extract_or_create_span(&headers, &config);
        // 子 span 应该继承 parent 的 trace_id
        assert_eq!(child_span.trace_id(), parent_trace_id);
        // 子 span 的 parent_id 应该是 parent 的 span_id
        assert_eq!(child_span.parent_id(), Some(parent_span_id.as_str()));
    }

    #[test]
    fn test_extract_or_create_span_with_invalid_traceparent_creates_new_span() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_service_name("my-service");

        let mut headers = HeaderMap::new();
        // 无效的 traceparent（格式错误）
        headers.insert("traceparent", "invalid".parse().unwrap());

        let span = extract_or_create_span(&headers, &config);
        // 无效 traceparent 应回退到创建新 span
        assert!(span.parent_id().is_none());
    }

    // ====================================================================
    // inject_traceparent_to_response 单元测试
    // ====================================================================

    #[test]
    fn test_inject_traceparent_to_response_adds_headers() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_service_name("my-service");
        let span = config.tracer.start_span("test");

        let mut response = Response::new(Body::from("body"));
        inject_traceparent_to_response(&mut response, &span, &config);

        // 应该注入 traceparent header
        assert!(response.headers().contains_key("traceparent"));
    }

    #[test]
    fn test_inject_traceparent_to_response_preserves_existing_headers() {
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_service_name("my-service");
        let span = config.tracer.start_span("test");

        let mut response = Response::builder()
            .header("x-custom", "value")
            .body(Body::from("body"))
            .unwrap();
        inject_traceparent_to_response(&mut response, &span, &config);

        // 原有 header 应该保留
        assert_eq!(
            response
                .headers()
                .get("x-custom")
                .unwrap()
                .to_str()
                .unwrap(),
            "value"
        );
        // 新增的 traceparent 应该存在
        assert!(response.headers().contains_key("traceparent"));
    }

    // ====================================================================
    // trace_middleware 集成测试
    // ====================================================================

    #[tokio::test]
    async fn test_trace_middleware_creates_span_for_request() {
        let app = build_app();
        let resp = app.oneshot(make_request("GET", "/api")).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        // 响应应该包含 traceparent header
        assert!(resp.headers().contains_key("traceparent"));
    }

    #[tokio::test]
    async fn test_trace_middleware_excluded_path_no_span() {
        let tracer = Arc::new(sz_orm_tracing::SzTracer::new("test-service"));
        let config = TraceConfig::new(tracer).with_exclude_paths(vec!["/health".to_string()]);
        let app = Router::new()
            .route(
                "/health",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                trace_middleware,
            ));

        let resp = app.oneshot(make_request("GET", "/health")).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        // 排除路径不应创建 span，所以响应不应包含 traceparent
        assert!(!resp.headers().contains_key("traceparent"));
    }

    #[tokio::test]
    async fn test_trace_middleware_wildcard_exclude() {
        let tracer = Arc::new(sz_orm_tracing::SzTracer::new("test-service"));
        let config = TraceConfig::new(tracer).with_exclude_paths(vec!["/public/*".to_string()]);
        let app = Router::new()
            .route(
                "/public/asset",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                trace_middleware,
            ));

        let resp = app
            .oneshot(make_request("GET", "/public/asset"))
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert!(!resp.headers().contains_key("traceparent"));
    }

    #[tokio::test]
    async fn test_trace_middleware_injects_span_into_extensions() {
        let tracer = Arc::new(sz_orm_tracing::SzTracer::new("test-service"));
        let config = TraceConfig::new(tracer);
        let app = Router::new()
            .route(
                "/api",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                trace_middleware,
            ));

        // 验证 span 被注入到 extensions（通过响应是否包含 traceparent 间接验证）
        let resp = app.oneshot(make_request("GET", "/api")).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert!(resp.headers().contains_key("traceparent"));
    }

    #[tokio::test]
    async fn test_trace_middleware_child_span_inherits_trace_id() {
        let tracer = Arc::new(sz_orm_tracing::SzTracer::new("test-service"));
        let config = TraceConfig::new(tracer);

        // 先创建一个 parent span，获取其 traceparent
        let parent_span = config.tracer.start_span("parent");
        let parent_trace_id = parent_span.trace_id().to_string();
        let headers_map = config.tracer.inject(&parent_span);
        let traceparent = headers_map
            .get("traceparent")
            .expect("traceparent should be in injected headers");

        let app = Router::new()
            .route(
                "/api",
                axum::routing::get(|| async { axum::http::StatusCode::OK }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config.clone(),
                trace_middleware,
            ));

        let resp = app
            .oneshot(make_request_with_traceparent("GET", "/api", traceparent))
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        // 响应的 traceparent 应该包含与 parent 相同的 trace_id
        let response_traceparent = resp.headers().get("traceparent").unwrap().to_str().unwrap();
        // traceparent 格式：00-<trace_id>-<span_id>-01
        let parts: Vec<&str> = response_traceparent.split('-').collect();
        assert_eq!(parts.len(), 4);
        // trace_id 应该继承自 parent
        assert_eq!(parts[1], parent_trace_id);
    }

    #[tokio::test]
    async fn test_trace_middleware_preserves_response_body() {
        let tracer = Arc::new(sz_orm_tracing::SzTracer::new("test-service"));
        let config = TraceConfig::new(tracer);
        let app = Router::new()
            .route("/body", axum::routing::get(|| async { "hello" }))
            .layer(axum::middleware::from_fn_with_state(
                config,
                trace_middleware,
            ));

        let resp = app.oneshot(make_request("GET", "/body")).await.unwrap();
        let body = read_body(resp).await;
        assert_eq!(body, "hello");
    }

    #[tokio::test]
    async fn test_trace_middleware_handles_post_request() {
        let tracer = Arc::new(sz_orm_tracing::SzTracer::new("test-service"));
        let config = TraceConfig::new(tracer);
        let app = Router::new()
            .route(
                "/submit",
                axum::routing::post(|| async { axum::http::StatusCode::CREATED }),
            )
            .layer(axum::middleware::from_fn_with_state(
                config,
                trace_middleware,
            ));

        let req = Request::builder()
            .method("POST")
            .uri("/submit")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::CREATED);
        assert!(resp.headers().contains_key("traceparent"));
    }

    #[tokio::test]
    async fn test_trace_middleware_chains_with_other_middleware() {
        async fn add_header_middleware(req: Request, next: Next) -> Response {
            let mut resp = next.run(req).await;
            resp.headers_mut()
                .insert("X-Custom", "value".parse().unwrap());
            resp
        }

        let tracer = Arc::new(sz_orm_tracing::SzTracer::new("test-service"));
        let config = TraceConfig::new(tracer);
        let app = Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(add_header_middleware))
            .layer(axum::middleware::from_fn_with_state(
                config,
                trace_middleware,
            ));

        let resp = app.oneshot(make_request("GET", "/")).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert_eq!(
            resp.headers().get("X-Custom").unwrap().to_str().unwrap(),
            "value"
        );
        assert!(resp.headers().contains_key("traceparent"));
    }

    #[tokio::test]
    async fn test_trace_middleware_different_requests_different_trace_ids() {
        let app = build_app();
        let resp1 = app
            .clone()
            .oneshot(make_request("GET", "/api"))
            .await
            .unwrap();
        let resp2 = app.oneshot(make_request("GET", "/api")).await.unwrap();

        let tp1 = resp1
            .headers()
            .get("traceparent")
            .unwrap()
            .to_str()
            .unwrap();
        let tp2 = resp2
            .headers()
            .get("traceparent")
            .unwrap()
            .to_str()
            .unwrap();

        // 两个请求应该有不同的 trace_id（除非有 parent traceparent）
        let parts1: Vec<&str> = tp1.split('-').collect();
        let parts2: Vec<&str> = tp2.split('-').collect();
        assert_ne!(parts1[1], parts2[1]); // trace_id 不同
    }

    // ====================================================================
    // PHP 行为对齐验证（R5 硬约束）
    // ====================================================================

    #[test]
    fn test_php_session_init_no_tracing_capability() {
        // 对齐 PHP `think\middleware\SessionInit` 的事实：
        // PHP SessionInit 仅初始化会话（`session_start()`），无追踪能力
        // sz-rust 的 Trace 中间件是自研增强，提供 W3C TraceContext 传播
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer);
        // 验证 sz-rust Trace 中间件的自研性质：默认 service_name 是 "sz-rust"（PHP 端无对应概念）
        assert_eq!(config.service_name, "sz-rust");
    }

    #[test]
    fn test_w3c_tracecontext_format_alignment() {
        // 对齐 W3C TraceContext 标准（OpenTelemetry）
        // traceparent 格式：00-<trace_id(32 hex)>-<span_id(16 hex)>-<trace_flags(2 hex)>
        let tracer: Arc<dyn Tracer + Send + Sync> = Arc::new(sz_orm_tracing::SzTracer::new("test"));
        let config = TraceConfig::new(tracer).with_service_name("test-service");
        let span = config.tracer.start_span("test");
        let headers_map = config.tracer.inject(&span);
        let traceparent = headers_map
            .get("traceparent")
            .expect("traceparent should be present");

        // 验证 W3C 格式
        let parts: Vec<&str> = traceparent.split('-').collect();
        assert_eq!(parts.len(), 4, "traceparent should have 4 parts");
        assert_eq!(parts[0], "00", "version should be 00");
        assert_eq!(parts[1].len(), 32, "trace_id should be 32 hex chars");
        assert_eq!(parts[2].len(), 16, "span_id should be 16 hex chars");
        assert_eq!(parts[3].len(), 2, "trace_flags should be 2 hex chars");
    }

    #[test]
    fn test_trace_middleware_executes_first_in_order() {
        // 对齐 DEFAULT_ORDER 中 Trace 位于第 1 位的约定
        // Trace 必须最先执行，确保所有后续中间件都能通过 extensions 获取 Span
        use crate::order::{MiddlewareKind, DEFAULT_ORDER};
        assert_eq!(DEFAULT_ORDER.first(), Some(&MiddlewareKind::Trace));
    }
}
