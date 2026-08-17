use crate::router::is_public_path;
use crate::services::auth_service;
use axum::{
    body::Body,
    http::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

/// 从 Authorization header 提取 Bearer token（安全修复 L-3：大小写不敏感）
///
/// 对齐核心库 `strip_bearer_prefix` 语义：`Bearer`/`bearer`/`BEARER` 均识别，
/// 无前缀时保持原样（由 verify_token 校验失败兜底）。
pub(crate) fn extract_bearer_token(auth_header: &str) -> &str {
    let trimmed = auth_header.trim();
    if trimmed.len() >= 6 {
        let prefix = &trimmed[..6];
        if prefix.eq_ignore_ascii_case("bearer") {
            return trimmed[6..].trim_start();
        }
    }
    trimmed
}

/// JWT 鉴权中间件：校验 Authorization 头中的 Bearer 令牌，
/// 公开路径（白名单精确匹配）自动跳过鉴权。
///
/// 安全说明（2026-07-26 P1 修复）：
/// - 旧版使用 `path.starts_with("/api/v1/auth/")` 前缀匹配，会绕过 `/api/v1/auth/me`、
///   `/api/v1/auth/logout` 等需要鉴权的接口
/// - 新版调用 `crate::router::is_public_path`（精确匹配）共用同一份白名单，
///   避免白名单散落多处导致策略不一致
///
/// # Deprecated 说明
///
/// `sz_rust_middleware_facade::auth::auth_middleware` 签名不兼容
/// （需 `State<AuthConfig>` + `from_fn_with_state`，本仓使用 `from_fn` + 全局 `auth_service`）。
/// 保留自研版以维持 JWT 验证逻辑（`auth_service::verify_token`）与错误响应格式不变。
/// 迁移至 middleware-facade 版需同步改造 `auth_service` 初始化流程。
///
/// 安全修复 H-1（2026-08-14）：验证通过后将用户身份注入 `Request.extensions()`
/// （`Arc<User>`），业务层通过 `auth_service::current_user` 获取，禁止信任请求体身份字段。
#[deprecated(
    since = "1.2.0",
    note = "请使用 sz_rust_middleware_facade::auth::auth_middleware（需同步迁移 AuthConfig 初始化）"
)]
pub async fn auth_middleware(mut req: Request<Body>, next: Next) -> Response {
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = extract_bearer_token(auth_header);

    // 公开路径白名单（精确匹配，避免前缀绕过）
    let path = req.uri().path();
    if is_public_path(path) {
        return next.run(req).await;
    }

    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "未授权").into_response();
    }

    match auth_service::verify_token(token) {
        Ok(user) => {
            // 2026-08-14 安全修复 H-1：将 JWT 身份注入请求扩展（Arc 避免 clone 开销），
            // 业务层通过 extensions 获取当前用户，禁止信任请求体中的 user_id/merchant_id。
            req.extensions_mut().insert(std::sync::Arc::new(user));
            next.run(req).await
        }
        Err(_) => (StatusCode::UNAUTHORIZED, "令牌无效或已过期").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_bearer_token_standard_prefix() {
        assert_eq!(extract_bearer_token("Bearer abc123"), "abc123");
    }

    #[test]
    fn extract_bearer_token_case_insensitive() {
        assert_eq!(extract_bearer_token("bearer xyz"), "xyz");
        assert_eq!(extract_bearer_token("BEARER token"), "token");
        assert_eq!(extract_bearer_token("BeArEr mixed"), "mixed");
    }

    #[test]
    fn extract_bearer_token_with_extra_spaces() {
        assert_eq!(extract_bearer_token("Bearer   spaced"), "spaced");
        assert_eq!(extract_bearer_token("  Bearer tok  "), "tok");
    }

    #[test]
    fn extract_bearer_token_no_prefix_returns_original() {
        assert_eq!(extract_bearer_token("abc123"), "abc123");
        assert_eq!(extract_bearer_token("just-a-token"), "just-a-token");
    }

    #[test]
    fn extract_bearer_token_empty_returns_empty() {
        assert_eq!(extract_bearer_token(""), "");
        assert_eq!(extract_bearer_token("   "), "");
    }

    #[test]
    fn extract_bearer_token_short_string_returns_trimmed() {
        // 长度 < 6，不可能是 "Bearer" 前缀
        assert_eq!(extract_bearer_token("abc"), "abc");
        assert_eq!(extract_bearer_token("Bear"), "Bear");
    }

    #[test]
    fn extract_bearer_token_exact_six_chars_no_prefix() {
        // 恰好 6 字符但不是 "Bearer"
        assert_eq!(extract_bearer_token("Bearer"), "");
        // "Bearer" 本身没有 token 部分，trim_start() 后为空
    }

    // ---- auth_middleware 路由级测试 ----

    use axum::middleware;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    #[allow(deprecated)]
    fn build_test_router() -> Router {
        Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/api/v1/protected", get(|| async { StatusCode::OK }))
            .layer(middleware::from_fn(auth_middleware))
    }

    #[tokio::test]
    async fn auth_middleware_public_path_passes_through() {
        let router = build_test_router();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_middleware_protected_path_no_token_returns_401() {
        let router = build_test_router();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_middleware_protected_path_invalid_token_returns_401() {
        crate::services::auth_service::init_auth_test_only("test-secret");
        let router = build_test_router();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/protected")
                    .header("authorization", "Bearer invalid.token.here")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_middleware_protected_path_empty_bearer_returns_401() {
        let router = build_test_router();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/protected")
                    .header("authorization", "Bearer ")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
