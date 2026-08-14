//! 角色鉴权中间件 — 基于 JWT 角色声明的路由级访问控制
//!
//! 提供 [`role_guard`] 中间件，验证请求携带的 JWT 令牌中包含指定角色。
//! 用于保护管理端点（如 `/api/admin/*`），仅允许 `admin` 角色访问。
//!
//! ## 与 [`super::auth_middleware`] 的区别
//!
//! - `auth_middleware`：全局中间件，验证令牌有效性（是否过期 / 签名是否正确）
//! - `role_guard`：路由级中间件，在令牌有效的基础上进一步校验角色
//!
//! ## 使用方式
//!
//! ```ignore
//! use crate::middleware::role_guard::admin_role_guard;
//!
//! let admin_routes = Router::new()
//!     .route("/server/info", get(admin::server_info))
//!     .route("/db/pool", get(admin::db_pool))
//!     .route("/redis/info", get(admin::redis_info))
//!     .layer(middleware::from_fn(admin_role_guard));
//! ```

use crate::services::auth_service;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// 验证 JWT 令牌中包含 `"admin"` 角色
///
/// 从 `Authorization: Bearer <token>` 头中提取令牌，调用 `auth_service::verify_token`
/// 验证令牌有效性并获取用户声明，检查 `user.roles` 是否包含 `"admin"`。
///
/// ## 错误响应
///
/// - 401：未提供令牌 / 令牌无效 / 令牌已过期
/// - 403：令牌有效但用户不具备 `admin` 角色
pub async fn admin_role_guard(req: Request<Body>, next: Next) -> Response {
    role_guard(req, next, "admin").await
}

/// 通用角色鉴权中间件
///
/// 验证请求 JWT 中包含指定角色。通过 `axum::middleware::from_fn_with_state`
/// 可传入任意角色名，此处提供固定 `"admin"` 的便捷版本 [`admin_role_guard`]。
async fn role_guard(req: Request<Body>, next: Next, required_role: &str) -> Response {
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = auth_header.strip_prefix("Bearer ").unwrap_or("");

    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "未提供认证令牌").into_response();
    }

    let user = match auth_service::verify_token(token) {
        Ok(u) => u,
        Err(_) => return (StatusCode::UNAUTHORIZED, "令牌无效或已过期").into_response(),
    };

    if !user.roles.iter().any(|r| r == required_role) {
        return (
            StatusCode::FORBIDDEN,
            format!("需要 {} 角色才能访问此资源", required_role),
        )
            .into_response();
    }

    next.run(req).await
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use axum::middleware;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use sz_rust_core::orm::jwt::{JwtClaims, JwtEncoder};
    use tower::util::ServiceExt;

    /// 测试用 JWT 密钥
    const TEST_SECRET: &str = "test-role-guard-secret-2026";

    /// 初始化测试用 JWT 认证器（仅设置 encoder，不接 DB）
    fn init_test_auth() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            crate::services::auth_service::init_auth_test_only(TEST_SECRET);
        });
    }

    /// 签发一个测试用 JWT 令牌（指定角色列表）
    fn issue_token(username: &str, roles: Vec<&str>) -> String {
        init_test_auth();
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        let claims = JwtClaims::new(username, exp)
            .with_issuer("sz300-test")
            .with_roles(roles.into_iter().map(String::from).collect())
            .with_user_id(999);
        JwtEncoder::new(TEST_SECRET).encode(&claims).unwrap()
    }

    /// 模拟 handler（仅当角色检查通过时才会执行）
    async fn ok_handler() -> &'static str {
        "ok"
    }

    fn build_router(role: &'static str) -> Router {
        Router::new()
            .route("/protected", get(ok_handler))
            .layer(middleware::from_fn(move |req, next| {
                role_guard(req, next, role)
            }))
    }

    fn bearer_request(token: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri("/protected")
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap()
    }

    async fn fetch_body_string(resp: Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn test_missing_token_returns_401() {
        let router = build_router("admin");
        let req = Request::builder()
            .method(Method::GET)
            .uri("/protected")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = fetch_body_string(resp).await;
        assert!(body.contains("未提供认证令牌"));
    }

    #[tokio::test]
    async fn test_invalid_token_returns_401() {
        let router = build_router("admin");
        let req = bearer_request("invalid-token-xyz");
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_valid_token_without_role_returns_403() {
        // 构造一个不含 admin 角色的用户令牌
        let token = issue_token("regular_user", vec!["user"]);

        let router = build_router("admin");
        let req = bearer_request(&token);
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = fetch_body_string(resp).await;
        assert!(body.contains("admin"));
    }

    #[tokio::test]
    async fn test_valid_token_with_admin_role_passes() {
        let token = issue_token("admin_user", vec!["admin", "user"]);

        let router = build_router("admin");
        let req = bearer_request(&token);
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = fetch_body_string(resp).await;
        assert_eq!(body, "ok");
    }
}
