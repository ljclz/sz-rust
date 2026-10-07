// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use crate::router::is_public_path;
use crate::services::auth_service;
use axum::{
    body::Body,
    http::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

/// v1.9.0 JWT Audience 校验配置（全局静态，启动时初始化）
#[cfg(feature = "v19-jwt-audience")]
static AUDIENCE_CONFIG: std::sync::OnceLock<auth_service::AudienceConfig> =
    std::sync::OnceLock::new();

/// v1.9.0 初始化 Audience 校验配置（启动时调用）
#[cfg(feature = "v19-jwt-audience")]
pub fn init_audience_config(config: auth_service::AudienceConfig) {
    let _ = AUDIENCE_CONFIG.set(config);
}

/// JWT 鉴权中间件：校验 Authorization 头中的 Bearer 令牌，
/// 公开路径（白名单精确匹配）自动跳过鉴权
///
/// 安全说明（2026-07-26 P1 修复）：
/// - 旧版使用 `path.starts_with("/api/v1/auth/")` 前缀匹配，会绕过 `/api/v1/auth/me`、
///   `/api/v1/auth/logout` 等需要鉴权的接口
/// - 新版调用 `crate::router::is_public_path`（精确匹配）共用同一份白名单，
///   避免白名单散落多处导致策略不一致
///
/// v1.9.0 增强（feature `v19-jwt-audience`）：
/// - 启用后额外校验 JWT `aud` 声明，aud 不匹配返回 401
/// - 无 aud 旧令牌在 grace period 内放行 + WARN，超出后按 deny_no_audience 策略处理
/// - 未调用 `init_audience_config` 时回退为不校验 audience
pub async fn auth_middleware(req: Request<Body>, next: Next) -> Response {
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

    // v1.9.0: feature gate 切换 audience 校验
    #[cfg(feature = "v19-jwt-audience")]
    {
        match AUDIENCE_CONFIG.get() {
            Some(config) => match auth_service::verify_token_with_audience(token, config) {
                Ok(_user) => next.run(req).await,
                Err(auth_service::AudienceError::InvalidToken) => {
                    (StatusCode::UNAUTHORIZED, "令牌无效或已过期").into_response()
                }
                Err(auth_service::AudienceError::AudienceMismatch) => {
                    (StatusCode::UNAUTHORIZED, "audience 不匹配").into_response()
                }
                Err(auth_service::AudienceError::NoAudienceDenied) => {
                    (StatusCode::UNAUTHORIZED, "缺少 audience，请重新登录").into_response()
                }
            },
            None => {
                // 未初始化 audience 配置 — 回退为仅校验签名/过期
                match auth_service::verify_token(token) {
                    Ok(_user) => next.run(req).await,
                    Err(_) => (StatusCode::UNAUTHORIZED, "令牌无效或已过期").into_response(),
                }
            }
        }
    }

    #[cfg(not(feature = "v19-jwt-audience"))]
    {
        match auth_service::verify_token(token) {
            Ok(_user) => next.run(req).await,
            Err(_) => (StatusCode::UNAUTHORIZED, "令牌无效或已过期").into_response(),
        }
    }
}
