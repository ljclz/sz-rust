use crate::router::is_public_path;
use crate::services::auth_service;
use axum::{
    body::Body,
    http::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

/// JWT 鉴权中间件：校验 Authorization 头中的 Bearer 令牌，
/// 公开路径（白名单精确匹配）自动跳过鉴权
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

    let token = auth_header.strip_prefix("Bearer ").unwrap_or("");

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
