//! API 版本管理 — URL/Header/Query 多策略
//!
//! ## 设计目标
//!
//! 提供灵活的 API 版本协商机制，支持三种主流策略：
//!
//! 1. **URL 路径策略**：`/api/v1/users`、`/api/v2/users`
//!    - 对齐 GitHub API、Twitter API 风格
//!    - 版本号作为 URL 路径前缀
//!
//! 2. **Header 策略**：
//!    - 自定义头 `X-API-Version: 2`
//!    - 或 `Accept: application/vnd.api+json; version=2`
//!    - 对齐 Stripe API、GitHub API（Accept header）风格
//!
//! 3. **Query 参数策略**：`/api/users?api_version=2`
//!    - 对齐部分内部 API 风格
//!    - 适合调试（浏览器直接访问）
//!
//! ## 中间件集成
//!
//! [`ApiVersionExtractor`] 是 axum 中间件，从请求中提取版本号并注入到请求扩展中。
//! 后续 handler 可通过 [`Request::extensions`] 获取 [`ApiVersion`]。
//!
//! ## 路由分组
//!
//! [`VersionedRouter`] 提供按版本分组的路由构建器：
//!
//! ```ignore
//! use sz_rust_core::api_version::{VersionedRouter, ApiVersion};
//!
//! let router = VersionedRouter::new()
//!     .route("v1", "/users", axum::routing::get(get_users_v1))
//!     .route("v2", "/users", axum::routing::get(get_users_v2))
//!     .build();
//! ```
//!
//! ## 默认版本与降级
//!
//! - 未指定版本时使用 `default_version`（通常为最新稳定版）
//! - 不存在的版本返回 400 Bad Request（避免误用）

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// API 版本号
// ============================================================================

/// API 版本号
///
/// 内部以 `u32` 存储，支持 `v1`、`v2` 等数字版本。
/// 不支持语义化版本（如 `v1.2.3`），保持 API 版本协商简单。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ApiVersion(u32);

impl ApiVersion {
    /// 创建新的 API 版本号
    pub const fn new(version: u32) -> Self {
        Self(version)
    }

    /// 获取版本号数值
    pub fn as_u32(&self) -> u32 {
        self.0
    }

    /// 从字符串解析版本号
    ///
    /// 支持格式：
    /// - `"1"` / `"2"` → 纯数字
    /// - `"v1"` / `"v2"` → v 前缀
    /// - `"version=1"` → query 参数格式
    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        // 处理 "version=1" 形式（先检测，避免被 trim_start_matches('v') 误删前缀 'v'）
        let num_str = if let Some(rest) = trimmed.strip_prefix("version=") {
            rest.trim()
        } else {
            trimmed.trim_start_matches('v').trim()
        };
        num_str.parse::<u32>().ok().map(Self)
    }

    /// 转为 `vN` 字符串（如 `v1`、`v2`）
    pub fn to_vstring(&self) -> String {
        format!("v{}", self.0)
    }
}

impl std::fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}", self.0)
    }
}

impl Default for ApiVersion {
    fn default() -> Self {
        Self::new(1)
    }
}

// ============================================================================
// 版本协商策略
// ============================================================================

/// 版本协商策略
///
/// 控制从请求中提取版本号的优先级和方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VersionStrategy {
    /// URL 路径策略：`/api/v1/users`
    ///
    /// 从 URL 路径的第一段提取版本号（如 `/v1/users` → `v1`）。
    #[default]
    UrlPath,
    /// 自定义 Header 策略：`X-API-Version: 1`
    Header,
    /// Accept Header 策略：`Accept: application/vnd.api+json; version=1`
    AcceptHeader,
    /// Query 参数策略：`?api_version=1`
    Query,
}

// ============================================================================
// 版本协商器
// ============================================================================

/// 版本协商器
///
/// 从请求中提取 API 版本号，按配置的策略顺序尝试。
#[derive(Debug, Clone)]
pub struct VersionNegotiator {
    /// 支持的版本列表（如 `[v1, v2, v3]`）
    supported_versions: Vec<ApiVersion>,
    /// 默认版本（未指定时使用）
    default_version: ApiVersion,
    /// 策略优先级顺序（前者优先）
    strategies: Vec<VersionStrategy>,
    /// URL 路径中版本前缀的识别前缀（如 `/api/v1/users` 中的 `api`）
    url_prefix: Option<String>,
    /// 版本 query 参数名（默认 `api_version`）
    query_param_name: String,
    /// 版本自定义 header 名（默认 `x-api-version`）
    header_name: String,
}

impl Default for VersionNegotiator {
    fn default() -> Self {
        Self {
            supported_versions: vec![ApiVersion::new(1)],
            default_version: ApiVersion::new(1),
            strategies: vec![
                VersionStrategy::UrlPath,
                VersionStrategy::Header,
                VersionStrategy::AcceptHeader,
                VersionStrategy::Query,
            ],
            url_prefix: Some("api".to_string()),
            query_param_name: "api_version".to_string(),
            header_name: "x-api-version".to_string(),
        }
    }
}

impl VersionNegotiator {
    /// 创建新的版本协商器
    pub fn new(default_version: ApiVersion) -> Self {
        Self {
            supported_versions: vec![default_version],
            default_version,
            ..Default::default()
        }
    }

    /// 设置支持的版本列表
    pub fn with_supported_versions(mut self, versions: Vec<ApiVersion>) -> Self {
        self.supported_versions = versions;
        self
    }

    /// 设置策略优先级顺序
    pub fn with_strategies(mut self, strategies: Vec<VersionStrategy>) -> Self {
        self.strategies = strategies;
        self
    }

    /// 设置 URL 路径前缀（如 `api`、`v` 等）
    pub fn with_url_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.url_prefix = Some(prefix.into());
        self
    }

    /// 设置 query 参数名
    pub fn with_query_param(mut self, name: impl Into<String>) -> Self {
        self.query_param_name = name.into();
        self
    }

    /// 设置自定义 header 名
    pub fn with_header_name(mut self, name: impl Into<String>) -> Self {
        self.header_name = name.into().to_lowercase();
        self
    }

    /// 从请求中协商版本号
    ///
    /// 按策略顺序尝试，首个成功的版本号即为协商结果。
    /// 若所有策略都未匹配，返回默认版本。
    ///
    /// # 返回
    ///
    /// - `Ok(ApiVersion)`：协商成功（可能是匹配的版本或默认版本）
    /// - `Err(VersionError)`：客户端指定了不支持的版本
    pub fn negotiate(&self, uri: &Uri, headers: &HeaderMap) -> Result<ApiVersion, VersionError> {
        for strategy in &self.strategies {
            let extracted = match strategy {
                VersionStrategy::UrlPath => self.extract_from_url_path(uri),
                VersionStrategy::Header => self.extract_from_header(headers),
                VersionStrategy::AcceptHeader => self.extract_from_accept_header(headers),
                VersionStrategy::Query => self.extract_from_query(uri),
            };

            if let Some(version) = extracted {
                // 客户端指定了版本，但不在支持列表中
                if !self.supported_versions.contains(&version) {
                    return Err(VersionError::UnsupportedVersion(version));
                }
                return Ok(version);
            }
        }

        // 所有策略都未匹配，使用默认版本
        Ok(self.default_version)
    }

    /// 从 URL 路径提取版本号
    ///
    /// 形如 `/api/v1/users` → `v1`，要求 `url_prefix` 后紧跟版本段。
    fn extract_from_url_path(&self, uri: &Uri) -> Option<ApiVersion> {
        let path = uri.path();
        let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();

        // 查找 url_prefix 后的版本段
        let start_idx = if let Some(ref prefix) = self.url_prefix {
            segments.iter().position(|s| *s == prefix.as_str())? + 1
        } else {
            0
        };

        if start_idx >= segments.len() {
            return None;
        }

        ApiVersion::parse(segments[start_idx])
    }

    /// 从自定义 header 提取版本号
    fn extract_from_header(&self, headers: &HeaderMap) -> Option<ApiVersion> {
        headers
            .get(&self.header_name)
            .and_then(|v| v.to_str().ok())
            .and_then(ApiVersion::parse)
    }

    /// 从 Accept header 提取版本号
    ///
    /// 形如 `application/vnd.api+json; version=1`
    fn extract_from_accept_header(&self, headers: &HeaderMap) -> Option<ApiVersion> {
        let accept = headers.get(axum::http::header::ACCEPT)?.to_str().ok()?;
        // 查找 `version=` 参数
        for part in accept.split(';') {
            let part = part.trim();
            if let Some(rest) = part.strip_prefix("version=") {
                return ApiVersion::parse(rest.trim_matches('"'));
            }
        }
        None
    }

    /// 从 query 参数提取版本号
    fn extract_from_query(&self, uri: &Uri) -> Option<ApiVersion> {
        let query = uri.query()?;
        for pair in query.split('&') {
            let mut parts = pair.splitn(2, '=');
            if parts.next()? == self.query_param_name {
                return ApiVersion::parse(parts.next()?);
            }
        }
        None
    }
}

// ============================================================================
// 版本错误
// ============================================================================

/// 版本协商错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionError {
    /// 客户端指定了不支持的版本
    UnsupportedVersion(ApiVersion),
}

impl std::fmt::Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionError::UnsupportedVersion(v) => {
                write!(f, "Unsupported API version: {}", v)
            }
        }
    }
}

impl std::error::Error for VersionError {}

impl IntoResponse for VersionError {
    fn into_response(self) -> Response {
        let body = match self {
            VersionError::UnsupportedVersion(v) => {
                format!("{{\"code\":0,\"msg\":\"Unsupported API version: {}\",\"data\":{{}}}}", v)
            }
        };
        (
            StatusCode::BAD_REQUEST,
            [(
                axum::http::header::CONTENT_TYPE,
                "application/json; charset=utf-8",
            )],
            body,
        )
            .into_response()
    }
}

// ============================================================================
// 中间件
// ============================================================================

/// 版本协商中间件状态
///
/// 通过 [`State`] 注入到中间件，避免每次请求重建协商器。
#[derive(Clone)]
pub struct ApiVersionExtractor {
    negotiator: Arc<VersionNegotiator>,
}

impl ApiVersionExtractor {
    /// 创建新的版本提取器
    pub fn new(negotiator: VersionNegotiator) -> Self {
        Self {
            negotiator: Arc::new(negotiator),
        }
    }

    /// 获取协商器引用
    pub fn negotiator(&self) -> &VersionNegotiator {
        &self.negotiator
    }
}

/// 版本协商中间件
///
/// 从请求中提取版本号并注入到请求扩展中。
/// 后续 handler 可通过 `req.extensions().get::<ApiVersion>()` 获取。
///
/// # 错误处理
///
/// 若客户端指定了不支持的版本，直接返回 400 Bad Request。
pub async fn version_negotiation_middleware(
    State(extractor): State<ApiVersionExtractor>,
    req: Request,
    next: Next,
) -> Response {
    let (parts, body) = req.into_parts();
    let uri = parts.uri.clone();
    let headers = parts.headers.clone();

    match extractor.negotiator.negotiate(&uri, &headers) {
        Ok(version) => {
            let mut req = Request::from_parts(parts, body);
            req.extensions_mut().insert(version);
            next.run(req).await
        }
        Err(err) => err.into_response(),
    }
}

// ============================================================================
// 版本化路由
// ============================================================================

/// 版本化路由构建器
///
/// 按 API 版本分组注册路由，自动添加版本前缀。
///
/// # 用法
///
/// ```ignore
/// use sz_rust_core::api_version::{VersionedRouter, ApiVersion};
///
/// let router = VersionedRouter::new()
///     .route("v1", "/users", axum::routing::get(get_users_v1))
///     .route("v2", "/users", axum::routing::get(get_users_v2))
///     .build();
/// ```
#[derive(Default)]
pub struct VersionedRouter {
    /// 版本 → (路径, MethodRouter) 列表
    routes: HashMap<String, Vec<(String, axum::routing::MethodRouter)>>,
    /// URL 前缀（如 `api`）
    url_prefix: Option<String>,
}

impl VersionedRouter {
    /// 创建新的版本化路由构建器
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 URL 前缀（如 `api`，最终路径为 `/api/v1/users`）
    pub fn with_url_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.url_prefix = Some(prefix.into());
        self
    }

    /// 注册版本化路由
    ///
    /// # 参数
    ///
    /// - `version`：版本字符串（如 `"v1"`、`"v2"`）
    /// - `path`：路由路径（如 `"/users"`，不含版本前缀）
    /// - `method_router`：方法路由器
    pub fn route(
        mut self,
        version: impl Into<String>,
        path: impl Into<String>,
        method_router: axum::routing::MethodRouter,
    ) -> Self {
        self.routes
            .entry(version.into())
            .or_default()
            .push((path.into(), method_router));
        self
    }

    /// 构建最终的 axum Router
    pub fn build(self) -> axum::Router {
        let mut router = axum::Router::new();
        let prefix = self.url_prefix.unwrap_or_default();

        for (version, routes) in self.routes {
            for (path, method_router) in routes {
                let full_path = if prefix.is_empty() {
                    format!("/{}/{}", version, path.trim_start_matches('/'))
                } else {
                    format!("/{}/{}/{}", prefix, version, path.trim_start_matches('/'))
                };
                router = router.route(&full_path, method_router);
            }
        }

        router
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{HeaderValue, Method};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // --------------------------------------------------------------------
    // ApiVersion
    // --------------------------------------------------------------------

    #[test]
    fn test_api_version_new() {
        let v = ApiVersion::new(2);
        assert_eq!(v.as_u32(), 2);
    }

    #[test]
    fn test_api_version_default() {
        let v = ApiVersion::default();
        assert_eq!(v.as_u32(), 1);
    }

    #[test]
    fn test_api_version_parse_pure_number() {
        assert_eq!(ApiVersion::parse("1"), Some(ApiVersion::new(1)));
        assert_eq!(ApiVersion::parse("42"), Some(ApiVersion::new(42)));
    }

    #[test]
    fn test_api_version_parse_with_v_prefix() {
        assert_eq!(ApiVersion::parse("v1"), Some(ApiVersion::new(1)));
        assert_eq!(ApiVersion::parse("v2"), Some(ApiVersion::new(2)));
    }

    #[test]
    fn test_api_version_parse_with_spaces() {
        assert_eq!(ApiVersion::parse("  v1  "), Some(ApiVersion::new(1)));
    }

    #[test]
    fn test_api_version_parse_version_equals() {
        assert_eq!(ApiVersion::parse("version=2"), Some(ApiVersion::new(2)));
    }

    #[test]
    fn test_api_version_parse_invalid() {
        assert_eq!(ApiVersion::parse("abc"), None);
        assert_eq!(ApiVersion::parse(""), None);
        assert_eq!(ApiVersion::parse("v"), None);
        assert_eq!(ApiVersion::parse("vabc"), None);
    }

    #[test]
    fn test_api_version_to_vstring() {
        assert_eq!(ApiVersion::new(1).to_vstring(), "v1");
        assert_eq!(ApiVersion::new(10).to_vstring(), "v10");
    }

    #[test]
    fn test_api_version_display() {
        assert_eq!(format!("{}", ApiVersion::new(1)), "v1");
    }

    #[test]
    fn test_api_version_equality() {
        assert_eq!(ApiVersion::new(1), ApiVersion::new(1));
        assert_ne!(ApiVersion::new(1), ApiVersion::new(2));
    }

    #[test]
    fn test_api_version_ordering() {
        assert!(ApiVersion::new(1) < ApiVersion::new(2));
        assert!(ApiVersion::new(3) > ApiVersion::new(2));
    }

    // --------------------------------------------------------------------
    // VersionNegotiator
    // --------------------------------------------------------------------

    #[test]
    fn test_negotiator_default() {
        let n = VersionNegotiator::default();
        assert_eq!(n.default_version, ApiVersion::new(1));
        assert_eq!(n.supported_versions, vec![ApiVersion::new(1)]);
    }

    #[test]
    fn test_negotiate_url_path_with_prefix() {
        let n = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)]);
        let uri = Uri::from_static("/api/v2/users");
        let headers = HeaderMap::new();

        let version = n.negotiate(&uri, &headers).unwrap();
        assert_eq!(version, ApiVersion::new(2));
    }

    #[test]
    fn test_negotiate_url_path_without_prefix() {
        // 没配置 url_prefix 时，从第一段提取版本
        let n = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)])
            .with_url_prefix("");
        let uri = Uri::from_static("/v1/users");
        let headers = HeaderMap::new();

        let version = n.negotiate(&uri, &headers).unwrap();
        assert_eq!(version, ApiVersion::new(1));
    }

    #[test]
    fn test_negotiate_header_custom() {
        let n = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)])
            .with_strategies(vec![VersionStrategy::Header]);
        let uri = Uri::from_static("/users");
        let mut headers = HeaderMap::new();
        headers.insert("x-api-version", HeaderValue::from_static("2"));

        let version = n.negotiate(&uri, &headers).unwrap();
        assert_eq!(version, ApiVersion::new(2));
    }

    #[test]
    fn test_negotiate_header_v_prefix() {
        let n = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)])
            .with_strategies(vec![VersionStrategy::Header]);
        let uri = Uri::from_static("/users");
        let mut headers = HeaderMap::new();
        headers.insert("x-api-version", HeaderValue::from_static("v2"));

        let version = n.negotiate(&uri, &headers).unwrap();
        assert_eq!(version, ApiVersion::new(2));
    }

    #[test]
    fn test_negotiate_accept_header() {
        let n = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)])
            .with_strategies(vec![VersionStrategy::AcceptHeader]);
        let uri = Uri::from_static("/users");
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ACCEPT,
            HeaderValue::from_static("application/vnd.api+json; version=2"),
        );

        let version = n.negotiate(&uri, &headers).unwrap();
        assert_eq!(version, ApiVersion::new(2));
    }

    #[test]
    fn test_negotiate_accept_header_quoted() {
        let n = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(3)])
            .with_strategies(vec![VersionStrategy::AcceptHeader]);
        let uri = Uri::from_static("/users");
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::ACCEPT,
            HeaderValue::from_static("application/json; version=\"3\""),
        );

        let version = n.negotiate(&uri, &headers).unwrap();
        assert_eq!(version, ApiVersion::new(3));
    }

    #[test]
    fn test_negotiate_query_param() {
        let n = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)])
            .with_strategies(vec![VersionStrategy::Query]);
        let uri = Uri::from_static("/users?api_version=2");
        let headers = HeaderMap::new();

        let version = n.negotiate(&uri, &headers).unwrap();
        assert_eq!(version, ApiVersion::new(2));
    }

    #[test]
    fn test_negotiate_custom_query_param_name() {
        let n = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)])
            .with_strategies(vec![VersionStrategy::Query])
            .with_query_param("ver");
        let uri = Uri::from_static("/users?ver=2");
        let headers = HeaderMap::new();

        let version = n.negotiate(&uri, &headers).unwrap();
        assert_eq!(version, ApiVersion::new(2));
    }

    #[test]
    fn test_negotiate_default_when_no_match() {
        let n = VersionNegotiator::new(ApiVersion::new(2))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)]);
        let uri = Uri::from_static("/users");
        let headers = HeaderMap::new();

        let version = n.negotiate(&uri, &headers).unwrap();
        assert_eq!(version, ApiVersion::new(2));
    }

    #[test]
    fn test_negotiate_strategy_priority() {
        // URL 路径优先于 Header
        let n = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)])
            .with_strategies(vec![VersionStrategy::UrlPath, VersionStrategy::Header]);

        let uri = Uri::from_static("/api/v1/users");
        let mut headers = HeaderMap::new();
        headers.insert("x-api-version", HeaderValue::from_static("2"));

        let version = n.negotiate(&uri, &headers).unwrap();
        assert_eq!(version, ApiVersion::new(1)); // URL 路径优先
    }

    #[test]
    fn test_negotiate_unsupported_version() {
        let n = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)]);
        let uri = Uri::from_static("/api/v3/users");
        let headers = HeaderMap::new();

        let result = n.negotiate(&uri, &headers);
        assert_eq!(
            result,
            Err(VersionError::UnsupportedVersion(ApiVersion::new(3)))
        );
    }

    // --------------------------------------------------------------------
    // VersionError
    // --------------------------------------------------------------------

    #[test]
    fn test_version_error_display() {
        let err = VersionError::UnsupportedVersion(ApiVersion::new(3));
        assert_eq!(err.to_string(), "Unsupported API version: v3");
    }

    #[tokio::test]
    async fn test_version_error_into_response() {
        let err = VersionError::UnsupportedVersion(ApiVersion::new(99));
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );

        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(body.contains("Unsupported API version"));
        assert!(body.contains("v99"));
    }

    // --------------------------------------------------------------------
    // 中间件集成测试
    // --------------------------------------------------------------------

    #[tokio::test]
    async fn test_middleware_injects_version() {
        let negotiator = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)]);
        let extractor = ApiVersionExtractor::new(negotiator);

        async fn handler(req: Request) -> String {
            let version = req.extensions().get::<ApiVersion>().unwrap();
            format!("version={}", version.as_u32())
        }

        let app = axum::Router::new()
            .route("/api/{*path}", axum::routing::get(handler))
            .layer(axum::middleware::from_fn_with_state(
                extractor,
                version_negotiation_middleware,
            ));

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v2/users")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "version=2");
    }

    #[tokio::test]
    async fn test_middleware_unsupported_version_returns_400() {
        let negotiator = VersionNegotiator::new(ApiVersion::new(1))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)]);
        let extractor = ApiVersionExtractor::new(negotiator);

        async fn handler(_: Request) -> &'static str {
            "should not reach"
        }

        let app = axum::Router::new()
            .route("/api/{*path}", axum::routing::get(handler))
            .layer(axum::middleware::from_fn_with_state(
                extractor,
                version_negotiation_middleware,
            ));

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v99/users")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_middleware_default_version_when_unspecified() {
        let negotiator = VersionNegotiator::new(ApiVersion::new(2))
            .with_supported_versions(vec![ApiVersion::new(1), ApiVersion::new(2)]);
        let extractor = ApiVersionExtractor::new(negotiator);

        async fn handler(req: Request) -> String {
            let version = req.extensions().get::<ApiVersion>().unwrap();
            format!("version={}", version.as_u32())
        }

        let app = axum::Router::new()
            .route("/users", axum::routing::get(handler))
            .layer(axum::middleware::from_fn_with_state(
                extractor,
                version_negotiation_middleware,
            ));

        // 未指定版本 → 使用默认 v2
        let req = Request::builder()
            .method(Method::GET)
            .uri("/users")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(body, "version=2");
    }

    // --------------------------------------------------------------------
    // VersionedRouter
    // --------------------------------------------------------------------

    #[tokio::test]
    async fn test_versioned_router_routes_to_correct_version() {
        async fn v1_handler() -> &'static str {
            "v1 response"
        }
        async fn v2_handler() -> &'static str {
            "v2 response"
        }

        let router = VersionedRouter::new()
            .with_url_prefix("api")
            .route("v1", "/users", axum::routing::get(v1_handler))
            .route("v2", "/users", axum::routing::get(v2_handler))
            .build();

        // v1 路由
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/users")
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(req).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(String::from_utf8_lossy(&bytes), "v1 response");

        // v2 路由
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v2/users")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(req).await.unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(String::from_utf8_lossy(&bytes), "v2 response");
    }

    #[tokio::test]
    async fn test_versioned_router_without_prefix() {
        async fn handler() -> &'static str {
            "ok"
        }

        let router = VersionedRouter::new()
            .route("v1", "/posts", axum::routing::get(handler))
            .build();

        let req = Request::builder()
            .method(Method::GET)
            .uri("/v1/posts")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_versioned_router_unregistered_path_returns_404() {
        async fn handler() -> &'static str {
            "ok"
        }

        let router = VersionedRouter::new()
            .with_url_prefix("api")
            .route("v1", "/users", axum::routing::get(handler))
            .build();

        // v3 路径未注册 → 404
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v3/users")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
