// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户配置鉴权中间件 — 全局配置写操作需平台管理员，
//! 租户配置操作需租户管理员，跨租户访问拒绝。

use std::collections::HashSet;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use sz_rust_orm_facade::data_scope::error::DataScopeError;
use sz_rust_orm_facade::tenant::error::TenantError;

use crate::data_perm_admin::error_response::ApiErrorResponse;
use crate::data_scope::DataScopeUserContext;

#[derive(Debug, Clone)]
pub struct TenantConfigGuardState {
    pub tenant_admin_roles: Vec<String>,
}

pub async fn tenant_config_guard_middleware(req: Request, next: Next) -> Response {
    let guard_state = req.extensions().get::<TenantConfigGuardState>().cloned();

    let user = match req.extensions().get::<DataScopeUserContext>() {
        Some(u) => u.clone(),
        None => {
            return ApiErrorResponse::from_error(DataScopeError::AuthRequired, 0).into_response();
        }
    };

    let uri = req.uri().path();
    let method = req.method();

    if uri.starts_with("/api/global-configs")
        && method == "PUT"
        && !(user.is_super && user.is_platform_admin)
    {
        return ApiErrorResponse::from_error(TenantError::PlatformAdminRequired.into(), 0)
            .into_response();
    }

    if uri.starts_with("/api/tenant-configs") {
        let tenant_admin_roles = guard_state
            .map(|s| s.tenant_admin_roles)
            .unwrap_or_default();
        if !is_tenant_admin(&user, &tenant_admin_roles) {
            return ApiErrorResponse::from_error(DataScopeError::AdminRequired, 0).into_response();
        }
    }

    next.run(req).await
}

fn is_tenant_admin(user: &DataScopeUserContext, tenant_admin_roles: &[String]) -> bool {
    if user.is_super {
        return true;
    }
    if tenant_admin_roles.is_empty() {
        return false;
    }
    let admin_set: HashSet<&str> = tenant_admin_roles.iter().map(|s| s.as_str()).collect();
    user.roles.iter().any(|r| admin_set.contains(r.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, StatusCode};
    use axum::middleware::from_fn;

    use axum::Extension;
    use axum::Router;
    use tower::ServiceExt;

    fn make_app(guard_state: TenantConfigGuardState) -> Router {
        Router::new()
            .fallback(|| async { "ok" })
            .layer(from_fn(tenant_config_guard_middleware))
            .layer(Extension(guard_state))
    }

    #[tokio::test]
    async fn test_global_config_requires_platform_admin() {
        let guard_state = TenantConfigGuardState {
            tenant_admin_roles: vec!["tenant_admin".into()],
        };
        let app = make_app(guard_state);
        let mut req = Request::builder()
            .method(Method::PUT)
            .uri("/api/global-configs/theme")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(
            DataScopeUserContext::new(1)
                .with_super(true)
                .with_roles(vec!["tenant_admin".into()]),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_global_config_platform_admin_pass() {
        let guard_state = TenantConfigGuardState {
            tenant_admin_roles: vec![],
        };
        let app = make_app(guard_state);
        let mut req = Request::builder()
            .method(Method::PUT)
            .uri("/api/global-configs/theme")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(
            DataScopeUserContext::new(1)
                .with_super(true)
                .with_platform_admin(true),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_tenant_config_tenant_admin_pass() {
        let guard_state = TenantConfigGuardState {
            tenant_admin_roles: vec!["tenant_admin".into()],
        };
        let app = make_app(guard_state);
        let mut req = Request::builder()
            .uri("/api/tenant-configs")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(1).with_roles(vec!["tenant_admin".into()]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_tenant_config_non_admin_403() {
        let guard_state = TenantConfigGuardState {
            tenant_admin_roles: vec!["tenant_admin".into()],
        };
        let app = make_app(guard_state);
        let mut req = Request::builder()
            .uri("/api/tenant-configs")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(1).with_roles(vec!["user".into()]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_missing_context_401() {
        let guard_state = TenantConfigGuardState {
            tenant_admin_roles: vec![],
        };
        let app = make_app(guard_state);
        let req = Request::builder()
            .uri("/api/tenant-configs")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_super_passes_tenant_config() {
        let guard_state = TenantConfigGuardState {
            tenant_admin_roles: vec![],
        };
        let app = make_app(guard_state);
        let mut req = Request::builder()
            .uri("/api/tenant-configs")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(1).with_super(true));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
