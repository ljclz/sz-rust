// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 平台管理员鉴权中间件 — 校验 is_super && is_platform_admin

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use sz_rust_orm_facade::data_scope::error::DataScopeError;
use sz_rust_orm_facade::tenant::error::TenantError;

use crate::data_perm_admin::error_response::ApiErrorResponse;
use crate::data_scope::DataScopeUserContext;

pub async fn platform_admin_guard_middleware(req: Request, next: Next) -> Response {
    let user = match req.extensions().get::<DataScopeUserContext>() {
        Some(u) => u.clone(),
        None => {
            return ApiErrorResponse::from_error(DataScopeError::AuthRequired, 0).into_response();
        }
    };

    if !(user.is_super && user.is_platform_admin) {
        return ApiErrorResponse::from_error(TenantError::PlatformAdminRequired.into(), 0)
            .into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::middleware::from_fn;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn make_app() -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn(platform_admin_guard_middleware))
    }

    #[tokio::test]
    async fn test_missing_context_401() {
        let app = make_app();
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_non_platform_admin_403() {
        let app = make_app();
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(1).with_super(true));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_platform_admin_pass() {
        let app = make_app();
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut().insert(
            DataScopeUserContext::new(1)
                .with_super(true)
                .with_platform_admin(true),
        );
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_super_but_not_platform_admin_403() {
        let app = make_app();
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(1).with_super(true));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_platform_admin_but_not_super_403() {
        let app = make_app();
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(1).with_platform_admin(true));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
