//! CORS 中间件 — 跨域请求支持
//!
//! 对齐 PHP `app\CrossDomain`：
//! - 默认 `Access-Control-Allow-Origin: *`
//! - 若配置 `cookie.domain`，则回显请求 `Origin`（前提：Origin 命中 cookie domain）
//! - `Access-Control-Allow-Credentials: true`
//! - `Access-Control-Max-Age: 1800`
//! - `Access-Control-Allow-Methods: GET, POST, PATCH, PUT, DELETE, OPTIONS`
//! - `Access-Control-Allow-Headers: Authorization, Content-Type, If-Match, If-Modified-Since,
//!    If-None-Match, If-Unmodified-Since, X-CSRF-TOKEN, X-Requested-With`
//!
//! 基于 `tower-http::cors`，提供：
//! - [`cors_layer`]：默认 CORS Layer（与 PHP 全局中间件等价）
//! - [`cors_layer_with_origin`]：回显 Origin 的 CORS Layer（与 PHP 配置 cookie.domain 等价）
//! - [`cors_layer_with_config`]：自定义完整 CORS 配置
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::middleware::cors::cors_layer;
//! use axum::Router;
//!
//! let app: Router = Router::new()
//!     .route("/", axum::routing::get(|| async { "hello" }))
//!     .layer(cors_layer());
//! ```

use axum::http::HeaderName;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};

/// 默认允许的方法（对齐 PHP `Access-Control-Allow-Methods`）
pub const DEFAULT_ALLOW_METHODS: &str = "GET, POST, PATCH, PUT, DELETE, OPTIONS";

/// 默认允许的请求头（对齐 PHP `Access-Control-Allow-Headers`）
pub const DEFAULT_ALLOW_HEADERS: &str =
    "Authorization, Content-Type, If-Match, If-Modified-Since, If-None-Match, If-Unmodified-Since, X-CSRF-TOKEN, X-Requested-With";

/// 默认预检缓存时长（秒）（对齐 PHP `Access-Control-Max-Age: 1800`）
pub const DEFAULT_MAX_AGE: u64 = 1800;

/// 默认 CORS Layer（对齐 PHP `app\CrossDomain` 默认行为）
///
/// 等价于 PHP 全局中间件配置：
/// - `Access-Control-Allow-Origin: *`
/// - `Access-Control-Allow-Credentials: true`
/// - `Access-Control-Allow-Methods: GET, POST, PATCH, PUT, DELETE, OPTIONS`
/// - `Access-Control-Allow-Headers: Authorization, Content-Type, ...`
/// - `Access-Control-Max-Age: 1800`
///
/// ## 注意
///
/// 浏览器规范要求：当 `Allow-Credentials: true` 时，`Allow-Origin` 不能为 `*`。
/// `tower-http` 在处理时会自动将 `*` 替换为请求的 Origin 回显，以满足规范。
/// 这与 PHP `think\middleware\AllowCrossDomain` 的实际行为一致。
pub fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_credentials(true)
        .allow_methods(parse_methods(DEFAULT_ALLOW_METHODS))
        .allow_headers(parse_headers(DEFAULT_ALLOW_HEADERS))
        .max_age(std::time::Duration::from_secs(DEFAULT_MAX_AGE))
}

/// 判断请求 Origin 是否命中 cookie_domain
///
/// ## 匹配规则（精确后缀匹配，避免子串绕过）
///
/// - `origin = "https://example.com"` + `domain = "example.com"` → 匹配
/// - `origin = "https://app.example.com"` + `domain = "example.com"` → 匹配（子域名）
/// - `origin = "https://evil-example.com"` + `domain = "example.com"` → **不匹配**（避免子串绕过）
/// - `origin = "https://example.com.evil.com"` + `domain = "example.com"` → **不匹配**
///
/// ## 实现细节
///
/// 1. 剥离 scheme（`http://` / `https://`）
/// 2. 剥离端口号（注意 IPv6 `[::1]:8080` 用 `[]` 包裹）
/// 3. 精确匹配或 `.domain` 后缀匹配
pub fn origin_matches_domain(origin: &str, domain: &str) -> bool {
    // 剥离 scheme
    let host = origin.split("://").nth(1).unwrap_or(origin);
    // 剥离端口号（区分 IPv6 与 IPv4/host）
    let host = if let Some(stripped) = host.strip_prefix('[') {
        // IPv6: [::1]:8080 → 返回 ::1（不含端口）
        stripped.split(']').next().unwrap_or(stripped)
    } else {
        // IPv4/host: example.com:8080 → example.com（rsplit_once 避免错误切分 IPv6）
        host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host)
    };
    // 精确匹配或 .domain 后缀匹配（阻止 evil-example.com / example.com.evil.com 等绕过）
    host == domain || host.ends_with(&format!(".{domain}"))
}

/// 回显请求 Origin 的 CORS Layer
///
/// 当配置了 `cookie.domain` 时使用此 Layer。若请求 `Origin` 命中 `cookie_domain`
/// 则回显 Origin，否则不设置 `Allow-Origin`（拒绝跨域）。
///
/// ## 安全实现
///
/// 使用 [`origin_matches_domain`] 做精确后缀匹配，避免 PHP 原版 `strpos` 子串匹配
/// 被 `evil-example.com` 等恶意域名绕过。
pub fn cors_layer_with_origin(cookie_domain: &str) -> CorsLayer {
    let cookie_domain = cookie_domain.to_string();
    let allow_origin = AllowOrigin::predicate(move |origin, _| {
        if cookie_domain.is_empty() {
            return true; // 空字符串等价于通配
        }
        match origin.to_str() {
            Ok(origin_str) => origin_matches_domain(origin_str, &cookie_domain),
            Err(_) => false,
        }
    });

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_credentials(true)
        .allow_methods(parse_methods(DEFAULT_ALLOW_METHODS))
        .allow_headers(parse_headers(DEFAULT_ALLOW_HEADERS))
        .max_age(std::time::Duration::from_secs(DEFAULT_MAX_AGE))
}

/// 自定义完整 CORS 配置
///
/// 提供 `Allow-Origin: *` + 不带 credentials 的简化版本，用于不需要 cookie 的纯 API 场景。
pub fn cors_layer_with_config(
    allow_origin: AllowOrigin,
    allow_credentials: bool,
    allow_methods: &str,
    allow_headers: &str,
    max_age_secs: u64,
) -> CorsLayer {
    let mut layer = CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods(parse_methods(allow_methods))
        .allow_headers(parse_headers(allow_headers))
        .max_age(std::time::Duration::from_secs(max_age_secs));
    if allow_credentials {
        layer = layer.allow_credentials(true);
    }
    layer
}

/// 解析方法字符串为 `AllowMethods`
fn parse_methods(methods: &str) -> AllowMethods {
    let mut list = Vec::new();
    for m in methods.split(',') {
        let m = m.trim();
        if let Ok(method) = m.parse::<axum::http::Method>() {
            list.push(method);
        }
    }
    AllowMethods::list(list)
}

/// 解析请求头字符串为 `AllowHeaders`
fn parse_headers(headers: &str) -> AllowHeaders {
    let mut list = Vec::new();
    for h in headers.split(',') {
        let h = h.trim();
        if let Ok(name) = HeaderName::from_bytes(h.as_bytes()) {
            list.push(name);
        }
    }
    AllowHeaders::list(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{HeaderName, Method, Request};
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn make_router(layer: CorsLayer) -> Router {
        Router::new()
            .route(
                "/api",
                axum::routing::get(|| async { "hello" }).post(|| async { "created" }),
            )
            .layer(layer)
    }

    async fn send_request(
        router: Router,
        method: &str,
        uri: &str,
        origin: Option<&str>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(o) = origin {
            builder = builder.header("origin", o);
        }
        let req = builder.body(Body::empty()).unwrap();
        router.oneshot(req).await.unwrap()
    }

    /// 发送 OPTIONS 预检请求，携带 `Access-Control-Request-Method` 和
    /// `Access-Control-Request-Headers`（对齐真实浏览器预检行为）
    async fn send_preflight(
        router: Router,
        uri: &str,
        origin: &str,
        request_method: &str,
        request_headers: &str,
    ) -> axum::response::Response {
        let req = Request::builder()
            .method("OPTIONS")
            .uri(uri)
            .header("origin", origin)
            .header("access-control-request-method", request_method)
            .header("access-control-request-headers", request_headers)
            .body(Body::empty())
            .unwrap();
        router.oneshot(req).await.unwrap()
    }

    // ====================================================================
    // cors_layer() 默认行为
    // ====================================================================

    #[tokio::test]
    async fn test_cors_layer_sets_allow_origin_mirror() {
        let router = make_router(cors_layer());
        let resp = send_request(router, "GET", "/api", Some("https://example.com")).await;

        // mirror_request 会回显 Origin
        let allow_origin = resp
            .headers()
            .get("access-control-allow-origin")
            .expect("missing Access-Control-Allow-Origin");
        assert_eq!(allow_origin, "https://example.com");
    }

    #[tokio::test]
    async fn test_cors_layer_sets_allow_credentials() {
        let router = make_router(cors_layer());
        let resp = send_request(router, "GET", "/api", Some("https://example.com")).await;

        let creds = resp
            .headers()
            .get("access-control-allow-credentials")
            .expect("missing Access-Control-Allow-Credentials");
        assert_eq!(creds, "true");
    }

    #[tokio::test]
    async fn test_cors_layer_preflight_sets_methods() {
        let router = make_router(cors_layer());
        let resp = send_request(router, "OPTIONS", "/api", Some("https://example.com")).await;

        let methods = resp
            .headers()
            .get("access-control-allow-methods")
            .expect("missing Access-Control-Allow-Methods");
        let methods_str = methods.to_str().unwrap();
        assert!(methods_str.contains("GET"));
        assert!(methods_str.contains("POST"));
        assert!(methods_str.contains("PATCH"));
        assert!(methods_str.contains("PUT"));
        assert!(methods_str.contains("DELETE"));
        assert!(methods_str.contains("OPTIONS"));
    }

    #[tokio::test]
    async fn test_cors_layer_preflight_sets_headers() {
        let router = make_router(cors_layer());
        // 真实浏览器预检会带上 Access-Control-Request-Headers
        let resp = send_preflight(
            router,
            "/api",
            "https://example.com",
            "POST",
            "Authorization, Content-Type, X-Requested-With, X-CSRF-TOKEN",
        )
        .await;

        let headers = resp
            .headers()
            .get("access-control-allow-headers")
            .expect("missing Access-Control-Allow-Headers");
        // HTTP headers 大小写不敏感，统一转小写比较
        let headers_str = headers.to_str().unwrap().to_lowercase();
        assert!(headers_str.contains("authorization"));
        assert!(headers_str.contains("content-type"));
        assert!(headers_str.contains("x-requested-with"));
        assert!(headers_str.contains("x-csrf-token"));
    }

    #[tokio::test]
    async fn test_cors_layer_preflight_sets_max_age() {
        let router = make_router(cors_layer());
        let resp = send_request(router, "OPTIONS", "/api", Some("https://example.com")).await;

        let max_age = resp
            .headers()
            .get("access-control-max-age")
            .expect("missing Access-Control-Max-Age");
        assert_eq!(max_age, "1800");
    }

    #[tokio::test]
    async fn test_cors_layer_normal_request_passes_through() {
        let router = make_router(cors_layer());
        let resp = send_request(router, "GET", "/api", Some("https://example.com")).await;

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"hello");
    }

    // ====================================================================
    // cors_layer_with_origin()
    // ====================================================================

    #[tokio::test]
    async fn test_cors_with_origin_empty_domain_allows_all() {
        // 空字符串 cookie_domain 等价于 cors_layer()
        let router = make_router(cors_layer_with_origin(""));
        let resp = send_request(router, "GET", "/api", Some("https://anything.com")).await;

        let allow_origin = resp
            .headers()
            .get("access-control-allow-origin")
            .expect("missing Access-Control-Allow-Origin");
        assert_eq!(allow_origin, "https://anything.com");
    }

    #[tokio::test]
    async fn test_cors_with_origin_matching_domain_allows() {
        let router = make_router(cors_layer_with_origin("example.com"));
        let resp = send_request(router, "GET", "/api", Some("https://app.example.com")).await;

        let allow_origin = resp
            .headers()
            .get("access-control-allow-origin")
            .expect("missing Access-Control-Allow-Origin");
        assert_eq!(allow_origin, "https://app.example.com");
    }

    #[tokio::test]
    async fn test_cors_with_origin_non_matching_domain_blocks() {
        let router = make_router(cors_layer_with_origin("example.com"));
        let resp = send_request(router, "GET", "/api", Some("https://evil.com")).await;

        // 不匹配时不应设置 Allow-Origin
        assert!(resp.headers().get("access-control-allow-origin").is_none());
    }

    // ====================================================================
    // S-2 回归测试：origin 精确后缀匹配，阻止子串绕过
    // ====================================================================

    #[test]
    fn test_origin_matches_domain_exact() {
        assert!(origin_matches_domain("https://example.com", "example.com"));
        assert!(origin_matches_domain("http://example.com", "example.com"));
        assert!(origin_matches_domain("example.com", "example.com"));
    }

    #[test]
    fn test_origin_matches_domain_subdomain() {
        assert!(origin_matches_domain(
            "https://app.example.com",
            "example.com"
        ));
        assert!(origin_matches_domain(
            "https://a.b.example.com",
            "example.com"
        ));
    }

    #[test]
    fn test_origin_matches_domain_evil_substring_blocked() {
        // evil-example.com 不应匹配 example.com（原 PHP strpos 子串匹配会错误接受）
        assert!(!origin_matches_domain(
            "https://evil-example.com",
            "example.com"
        ));
        // example.com.evil.com 不应匹配 example.com
        assert!(!origin_matches_domain(
            "https://example.com.evil.com",
            "example.com"
        ));
        // notexample.com 不应匹配 example.com
        assert!(!origin_matches_domain(
            "https://notexample.com",
            "example.com"
        ));
    }

    #[test]
    fn test_origin_matches_domain_with_port() {
        assert!(origin_matches_domain(
            "https://example.com:8443",
            "example.com"
        ));
        assert!(origin_matches_domain(
            "https://app.example.com:8443",
            "example.com"
        ));
        // 端口不改变 host 后缀匹配规则
        assert!(!origin_matches_domain(
            "https://evil-example.com:8443",
            "example.com"
        ));
    }

    #[test]
    fn test_origin_matches_domain_ipv6() {
        // IPv6 地址用 [] 包裹，端口在 ] 之后
        assert!(origin_matches_domain("http://[::1]:8080", "::1"));
        assert!(!origin_matches_domain("http://[::2]:8080", "::1"));
    }

    #[test]
    fn test_origin_matches_domain_scheme_less() {
        // 无 scheme 的 Origin（罕见但应处理）
        assert!(origin_matches_domain("example.com", "example.com"));
        assert!(origin_matches_domain("app.example.com", "example.com"));
        assert!(!origin_matches_domain("evil-example.com", "example.com"));
    }

    #[tokio::test]
    async fn test_cors_with_origin_evil_substring_blocked() {
        // 端到端回归：evil-example.com 不应被 cors_layer_with_origin("example.com") 接受
        let router = make_router(cors_layer_with_origin("example.com"));
        let resp = send_request(router, "GET", "/api", Some("https://evil-example.com")).await;

        // 不匹配时不应设置 Allow-Origin
        assert!(
            resp.headers().get("access-control-allow-origin").is_none(),
            "evil-example.com must NOT match cookie_domain=example.com"
        );
    }

    #[tokio::test]
    async fn test_cors_with_origin_subdomain_allowed() {
        // 子域名应被允许
        let router = make_router(cors_layer_with_origin("example.com"));
        let resp = send_request(router, "GET", "/api", Some("https://app.example.com")).await;

        let allow_origin = resp
            .headers()
            .get("access-control-allow-origin")
            .expect("subdomain app.example.com should match cookie_domain=example.com");
        assert_eq!(allow_origin, "https://app.example.com");
    }

    // ====================================================================
    // cors_layer_with_config()
    // ====================================================================

    #[tokio::test]
    async fn test_cors_with_config_wildcard_no_credentials() {
        let layer =
            cors_layer_with_config(AllowOrigin::any(), false, "GET, POST", "Content-Type", 600);
        let router = make_router(layer);
        let resp = send_preflight(
            router,
            "/api",
            "https://example.com",
            "POST",
            "Content-Type",
        )
        .await;

        let allow_origin = resp
            .headers()
            .get("access-control-allow-origin")
            .expect("missing Access-Control-Allow-Origin");
        assert_eq!(allow_origin, "*");

        // 不带 credentials
        assert!(resp
            .headers()
            .get("access-control-allow-credentials")
            .is_none());

        let max_age = resp
            .headers()
            .get("access-control-max-age")
            .expect("missing Access-Control-Max-Age");
        assert_eq!(max_age, "600");
    }

    #[tokio::test]
    async fn test_cors_with_config_custom_methods_headers() {
        let layer = cors_layer_with_config(
            AllowOrigin::any(),
            false,
            "GET, POST, OPTIONS",
            "Authorization, Content-Type, X-Custom",
            3600,
        );
        let router = make_router(layer);
        let resp = send_preflight(
            router,
            "/api",
            "https://example.com",
            "POST",
            "Authorization, Content-Type, X-Custom",
        )
        .await;

        let methods = resp
            .headers()
            .get("access-control-allow-methods")
            .expect("missing methods");
        let methods_str = methods.to_str().unwrap();
        assert!(methods_str.contains("GET"));
        assert!(methods_str.contains("POST"));
        assert!(methods_str.contains("OPTIONS"));

        let headers = resp
            .headers()
            .get("access-control-allow-headers")
            .expect("missing headers");
        let headers_str = headers.to_str().unwrap().to_lowercase();
        assert!(headers_str.contains("authorization"));
        assert!(headers_str.contains("x-custom"));
    }

    // ====================================================================
    // 辅助函数测试
    // ====================================================================

    #[test]
    fn test_parse_methods_default() {
        let methods = parse_methods(DEFAULT_ALLOW_METHODS);
        // AllowMethods::list 不直接暴露内部，但通过 CorsLayer 应用到响应来验证
        // 这里仅验证不 panic
        let _ = methods;
    }

    #[test]
    fn test_parse_methods_empty() {
        let methods = parse_methods("");
        let _ = methods;
    }

    #[test]
    fn test_parse_methods_with_whitespace() {
        let methods = parse_methods("GET,  POST  , PATCH");
        let _ = methods;
    }

    #[test]
    fn test_parse_headers_default() {
        let headers = parse_headers(DEFAULT_ALLOW_HEADERS);
        let _ = headers;
    }

    #[test]
    fn test_parse_headers_empty() {
        let headers = parse_headers("");
        let _ = headers;
    }

    #[test]
    fn test_parse_headers_with_whitespace() {
        let headers = parse_headers("Authorization,  Content-Type  , X-Requested-With");
        let _ = headers;
    }

    #[test]
    fn test_default_allow_methods_constant() {
        assert!(DEFAULT_ALLOW_METHODS.contains("GET"));
        assert!(DEFAULT_ALLOW_METHODS.contains("POST"));
        assert!(DEFAULT_ALLOW_METHODS.contains("PATCH"));
        assert!(DEFAULT_ALLOW_METHODS.contains("PUT"));
        assert!(DEFAULT_ALLOW_METHODS.contains("DELETE"));
        assert!(DEFAULT_ALLOW_METHODS.contains("OPTIONS"));
    }

    #[test]
    fn test_default_allow_headers_constant() {
        assert!(DEFAULT_ALLOW_HEADERS.contains("Authorization"));
        assert!(DEFAULT_ALLOW_HEADERS.contains("Content-Type"));
        assert!(DEFAULT_ALLOW_HEADERS.contains("If-Match"));
        assert!(DEFAULT_ALLOW_HEADERS.contains("If-Modified-Since"));
        assert!(DEFAULT_ALLOW_HEADERS.contains("If-None-Match"));
        assert!(DEFAULT_ALLOW_HEADERS.contains("If-Unmodified-Since"));
        assert!(DEFAULT_ALLOW_HEADERS.contains("X-CSRF-TOKEN"));
        assert!(DEFAULT_ALLOW_HEADERS.contains("X-Requested-With"));
    }

    #[test]
    fn test_default_max_age_constant() {
        assert_eq!(DEFAULT_MAX_AGE, 1800);
    }

    // ====================================================================
    // 集成测试：与 PHP 行为对齐
    // ====================================================================

    #[tokio::test]
    async fn test_php_aligned_default_cors_headers() {
        // 对齐 PHP `app\CrossDomain` 默认 header 集合
        let router = make_router(cors_layer());
        let resp = send_request(router, "OPTIONS", "/api", Some("https://example.com")).await;

        let headers = resp.headers();
        // 必须存在的所有 CORS 响应头
        assert!(headers.contains_key("access-control-allow-origin"));
        assert!(headers.contains_key("access-control-allow-credentials"));
        assert!(headers.contains_key("access-control-allow-methods"));
        assert!(headers.contains_key("access-control-allow-headers"));
        assert!(headers.contains_key("access-control-max-age"));
    }

    #[tokio::test]
    async fn test_cors_layer_clonable() {
        // CorsLayer 必须 Clone + Send + Sync + 'static 才能用作 axum Layer
        let layer = cors_layer();
        let _cloned = layer.clone();
        fn assert_send_sync<T: Send + Sync + Clone + 'static>(_: T) {}
        assert_send_sync(layer);
    }

    #[tokio::test]
    async fn test_cors_no_origin_header_still_works() {
        // 无 Origin 头的请求也应正常处理
        let router = make_router(cors_layer());
        let resp = send_request(router, "GET", "/api", None).await;

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn test_cors_post_request_allowed() {
        let router = make_router(cors_layer());
        let resp = send_request(router, "POST", "/api", Some("https://example.com")).await;

        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"created");
    }

    #[tokio::test]
    async fn test_cors_unknown_method_returns_405() {
        let router = make_router(cors_layer());

        // DELETE 未注册
        let builder = Request::builder()
            .method(Method::DELETE)
            .uri("/api")
            .header("origin", "https://example.com");
        let req = builder.body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), axum::http::StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn test_header_name_constants_match_php() {
        // 验证 PHP header 名称都能解析
        let names = [
            "Access-Control-Allow-Origin",
            "Access-Control-Allow-Credentials",
            "Access-Control-Allow-Methods",
            "Access-Control-Allow-Headers",
            "Access-Control-Max-Age",
        ];
        for name in &names {
            assert!(
                HeaderName::from_bytes(name.as_bytes()).is_ok(),
                "invalid header name: {name}"
            );
        }
    }
}
