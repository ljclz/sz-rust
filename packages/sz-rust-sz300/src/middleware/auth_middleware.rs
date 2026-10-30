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

/// 当前登录用户上下文（2026-10-28 IDOR 修复新增）
///
/// 由 [`auth_middleware`] 在 JWT 校验通过后注入请求扩展，
/// 供业务控制器做数据归属过滤（防止水平越权读取其他商户数据）。
#[derive(Debug, Clone)]
pub struct CurrentUser {
    /// 用户 ID（merchant_user.user_id）
    pub user_id: i64,
    /// 用户名
    pub username: String,
    /// 所属商户 ID（DB 查询；测试/DB 不可用时为 None）
    pub merchant_id: Option<i64>,
    /// 是否管理员（merchant_user.role == 1）
    pub is_admin: bool,
}

impl CurrentUser {
    /// 从请求扩展读取当前用户（未认证请求返回 None）
    pub fn from_request(req: &axum::http::Request<axum::body::Body>) -> Option<Self> {
        req.extensions().get::<CurrentUser>().cloned()
    }
}

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
/// - 无 aud 旧令牌在 grace period 内放行 + WARN（每 60 s 采样一条，附被抑制条数），超出后按 deny_no_audience 策略处理
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
                Ok(user) => run_with_user_context(req, next, user).await,
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
                    Ok(user) => run_with_user_context(req, next, user).await,
                    Err(_) => (StatusCode::UNAUTHORIZED, "令牌无效或已过期").into_response(),
                }
            }
        }
    }

    #[cfg(not(feature = "v19-jwt-audience"))]
    {
        match auth_service::verify_token(token) {
            Ok(user) => run_with_user_context(req, next, user).await,
            Err(_) => (StatusCode::UNAUTHORIZED, "令牌无效或已过期").into_response(),
        }
    }
}

/// 校验通过后将当前用户上下文注入请求扩展，再放行到业务 handler
///
/// IDOR 修复（2026-10-28）：业务控制器通过 [`CurrentUser::from_request`]
/// 获取当前用户的 `merchant_id` 与管理员标记，对查询做数据归属过滤。
/// DB 不可用/未初始化时降级为仅 JWT 声明（`merchant_id=None`），不阻断请求。
///
/// ADR-049（2026-10-30）：授权上下文经 LRU+TTL 缓存，命中时该请求不再查库；
/// 外部直接改 `role`/`merchant_id` 后最长 60 s 生效。
async fn run_with_user_context(
    req: Request<Body>,
    next: Next,
    user: sz_rust_core::orm::auth::User,
) -> Response {
    let ctx = auth_service::auth_context(user.id).await;
    let current = CurrentUser {
        user_id: user.id,
        username: user.username.clone(),
        merchant_id: ctx.and_then(|c| c.merchant_id),
        is_admin: ctx.map(|c| c.is_admin).unwrap_or(false),
    };
    let mut req = req;
    req.extensions_mut().insert(current);
    next.run(req).await
}
