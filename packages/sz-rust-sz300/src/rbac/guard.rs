// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! RBAC axum 中间件适配
//!
//! 从请求提取 JWT → 验证令牌 → 查 RbacEngine → allow/deny。
//!
//! ## 使用方式
//!
//! ```ignore
//! use axum::middleware;
//! use crate::rbac::guard::rbac_guard;
//!
//! let protected = Router::new()
//!     .route("/api/v1/merchant/create", post(merchant::create))
//!     .layer(middleware::from_fn_with_state(
//!         (rbac_engine, perm::merchant("create")),
//!         rbac_guard,
//!     ));
//! ```

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sz_rust_auth_facade::{PermissionDecision, RbacEngine, Resource};

use crate::services::auth_service;

/// RBAC 守卫状态：(引擎, 需要的资源权限)
pub type RbacGuardState = (Arc<RbacEngine>, Resource);
/// RBAC 中间件：校验当前用户是否拥有指定资源权限
///
/// ## 错误响应
///
/// - 401：未提供令牌 / 令牌无效
/// - 403：令牌有效但无权限
pub async fn rbac_guard(
    State((engine, resource)): State<RbacGuardState>,
    req: Request<Body>,
    next: Next,
) -> Response {
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
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "令牌无效或已过期").into_response();
        }
    };

    let user_id = user.id.to_string();
    match engine.check(&user_id, &resource).await {
        Ok(PermissionDecision::Allow) => next.run(req).await,
        Ok(PermissionDecision::Deny) => (
            StatusCode::FORBIDDEN,
            format!(
                "用户 {} 无权访问资源 {}/{}",
                user.username, resource.resource_type, resource.operation
            ),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = ?e, "RBAC 权限校验内部错误");
            (StatusCode::INTERNAL_SERVER_ERROR, "权限校验失败").into_response()
        }
    }
}
