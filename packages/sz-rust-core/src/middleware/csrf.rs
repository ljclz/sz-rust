//! CSRF 防护中间件 — 双提交 Cookie 模式
//!
//! 2026-07-25 新增（修复 P0 安全审计项）。
//!
//! ## 设计
//!
//! 采用「双提交 Cookie」（Double Submit Cookie）模式，适用于无状态 JWT API：
//! 1. 登录成功时，服务端设置一个 `csrf_token` Cookie（HttpOnly=false，允许 JS 读取）
//! 2. 前端发起写请求时，从 Cookie 读取 token，附加到 `X-CSRF-TOKEN` 请求头
//! 3. 中间件校验 Cookie 值与 Header 值一致
//!
//! ## 安全说明
//!
//! - 安全方法（GET/HEAD/OPTIONS）跳过校验
//! - 公开路径（如 `/health`、`/metrics`、`/api/v1/auth/login`）跳过校验
//! - token 使用 32 字节随机数 + Base64 编码，不可预测
//! - Cookie 设置 `SameSite=Strict`，阻止跨站携带
//! - 不依赖 Session，与 JWT 无状态架构兼容
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::middleware::csrf::{csrf_middleware, CsrfConfig};
//! use axum::{middleware, Router};
//!
//! let app: Router = Router::new()
//!     .route("/api/v1/data", axum::routing::post(handler))
//!     .layer(middleware::from_fn(csrf_middleware));
//! ```

use axum::body::Body;
use axum::http::{HeaderName, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rand::RngCore;

/// CSRF Cookie 名称
pub const CSRF_COOKIE_NAME: &str = "csrf_token";

/// CSRF Header 名称
pub const CSRF_HEADER_NAME: &str = "x-csrf-token";

/// 默认跳过校验的公开路径
///
/// 包含：
/// - `/health` / `/metrics`：健康检查与监控端点
/// - `/api/v1/auth/login` / `/api/v1/auth/refresh`：登录与刷新令牌（用户尚未持有 CSRF token，
///   登录成功后由服务端通过 `Set-Cookie` 下发 `csrf_token`）
pub const DEFAULT_PUBLIC_PATHS: &[&str] = &[
    "/health",
    "/metrics",
    "/api/v1/auth/login",
    "/api/v1/auth/refresh",
];

/// 安全方法（不需要 CSRF 校验）
pub fn is_safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// 判断路径是否在公开路径列表中（前缀匹配）
pub fn is_public_path(path: &str, public_paths: &[&str]) -> bool {
    public_paths.iter().any(|p| path.starts_with(p))
}

/// 生成 32 字节随机 CSRF token（Base64 编码，44 字符）
///
/// 使用 `rand::RngCore::fill_bytes` 生成密码学安全的随机字节。
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64_encode(&bytes)
}

/// Base64 编码（URL-safe，无填充）
fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// CSRF 中间件 — 校验双提交 Cookie
///
/// ## 校验流程
///
/// 1. 安全方法（GET/HEAD/OPTIONS）直接放行
/// 2. 公开路径直接放行
/// 3. 从 Cookie 提取 `csrf_token`
/// 4. 从 Header 提取 `X-CSRF-TOKEN`
/// 5. 比较两者是否一致（常量时间比较，防时序攻击）
/// 6. 不一致或缺失返回 403
#[tracing::instrument(skip(req, next))]
pub async fn csrf_middleware(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    // 1. 安全方法直接放行
    if is_safe_method(&method) {
        return next.run(req).await;
    }

    // 2. 公开路径直接放行
    if is_public_path(&path, DEFAULT_PUBLIC_PATHS) {
        return next.run(req).await;
    }

    // 3. 从 Cookie 提取 csrf_token
    let cookie_token = extract_cookie_value(req.headers().get("cookie"), CSRF_COOKIE_NAME);

    // 4. 从 Header 提取 X-CSRF-TOKEN
    let header_token = req
        .headers()
        .get(HeaderName::from_static(CSRF_HEADER_NAME))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // 5. 常量时间比较（防时序攻击）
    match (cookie_token, header_token) {
        (Some(cookie), Some(header)) if constant_time_eq(cookie.as_bytes(), header.as_bytes()) => {
            // 校验通过
            next.run(req).await
        }
        _ => {
            tracing::warn!(
                method = %method,
                path = %path,
                "CSRF 校验失败：Cookie 或 Header token 缺失/不匹配"
            );
            (StatusCode::FORBIDDEN, "CSRF token 校验失败").into_response()
        }
    }
}

/// 从 Cookie header 中提取指定名称的值
///
/// 支持标准 Cookie 格式：`name1=value1; name2=value2`
pub fn extract_cookie_value(cookie_header: Option<&axum::http::HeaderValue>, name: &str) -> Option<String> {
    let header = cookie_header?;
    let header_str = header.to_str().ok()?;
    for pair in header_str.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// 常量时间比较（防止时序攻击）
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{HeaderValue, Method, Request};
    use axum::routing::post;
    use axum::Router;
    use tower::ServiceExt;

    fn make_router() -> Router {
        Router::new()
            .route("/api/data", post(|| async { "ok" }))
            .layer(axum::middleware::from_fn(csrf_middleware))
    }

    async fn send(method: &str, path: &str, cookie: Option<&str>, header: Option<&str>) -> Response {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(c) = cookie {
            builder = builder.header("cookie", c);
        }
        if let Some(h) = header {
            builder = builder.header(CSRF_HEADER_NAME, h);
        }
        let req = builder.body(Body::empty()).unwrap();
        make_router().oneshot(req).await.unwrap()
    }

    #[test]
    fn test_generate_token_length() {
        let token = generate_token();
        // 32 字节 Base64 编码 = 43 字符（URL-safe no pad）
        assert!(token.len() >= 40, "token too short: {}", token.len());
    }

    #[test]
    fn test_generate_token_uniqueness() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2, "tokens must be unique");
    }

    #[test]
    fn test_is_safe_method() {
        assert!(is_safe_method(&Method::GET));
        assert!(is_safe_method(&Method::HEAD));
        assert!(is_safe_method(&Method::OPTIONS));
        assert!(!is_safe_method(&Method::POST));
        assert!(!is_safe_method(&Method::PUT));
        assert!(!is_safe_method(&Method::DELETE));
    }

    #[test]
    fn test_is_public_path() {
        assert!(is_public_path("/health", DEFAULT_PUBLIC_PATHS));
        assert!(is_public_path("/health/ready", DEFAULT_PUBLIC_PATHS));
        assert!(is_public_path("/metrics", DEFAULT_PUBLIC_PATHS));
        assert!(!is_public_path("/api/v1/data", DEFAULT_PUBLIC_PATHS));
    }

    #[test]
    fn test_extract_cookie_value() {
        let header = HeaderValue::from_static("csrf_token=abc123; other=value");
        assert_eq!(
            extract_cookie_value(Some(&header), "csrf_token"),
            Some("abc123".to_string())
        );
        assert_eq!(
            extract_cookie_value(Some(&header), "other"),
            Some("value".to_string())
        );
        assert_eq!(extract_cookie_value(Some(&header), "missing"), None);
    }

    #[test]
    fn test_constant_time_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[tokio::test]
    async fn test_get_method_bypasses_csrf() {
        let resp = send("GET", "/api/data", None, None).await;
        // GET 到 POST-only 路由会返回 405，但不是 403
        assert_ne!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_post_without_csrf_returns_403() {
        let resp = send("POST", "/api/data", None, None).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_post_with_mismatched_tokens_returns_403() {
        let resp = send(
            "POST",
            "/api/data",
            Some("csrf_token=abc"),
            Some("xyz"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_post_with_matching_tokens_passes() {
        let token = "valid_token_123";
        let resp = send(
            "POST",
            "/api/data",
            Some(&format!("csrf_token={}", token)),
            Some(token),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_post_with_only_cookie_returns_403() {
        let resp = send("POST", "/api/data", Some("csrf_token=abc"), None).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_post_with_only_header_returns_403() {
        let resp = send("POST", "/api/data", None, Some("abc")).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_public_path_bypasses_csrf() {
        // /health 是公开路径，POST 也应放行（虽然 /health 路由可能不存在）
        let resp = send("POST", "/health", None, None).await;
        // 不会返回 403（CSRF 放行），可能返回 404（路由不存在）
        assert_ne!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_options_method_bypasses_csrf() {
        let resp = send("OPTIONS", "/api/data", None, None).await;
        assert_ne!(resp.status(), StatusCode::FORBIDDEN);
    }
}
