// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户状态校验中间件 — 从 TenantContext 提取 tenant_id，
//! 校验租户状态 active/suspended/disabled。

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::tenant::context::TenantContext;
use sz_rust_orm_facade::tenant::error::TenantError;
use sz_rust_orm_facade::tenant::record::TenantRecordRegistry;
use sz_rust_orm_facade::tenant::status::TenantStatus;

use crate::data_perm_admin::error_response::ApiErrorResponse;

#[derive(Clone)]
pub struct TenantStatusMiddlewareState {
    pub tenant_registry: Arc<TenantRecordRegistry>,
    pub metrics: Arc<DataScopeMetrics>,
}

pub async fn tenant_status_middleware(
    State(state): State<TenantStatusMiddlewareState>,
    req: Request,
    next: Next,
) -> Response {
    let ctx = match req.extensions().get::<TenantContext>() {
        Some(ctx) => ctx.clone(),
        None => return next.run(req).await,
    };

    let tenant = match state.tenant_registry.get(ctx.tenant_id()) {
        Some(t) => t,
        None => {
            state.metrics.record_tenant_resolve_failed();
            return ApiErrorResponse::from_error(
                TenantError::TenantNotFound(ctx.tenant_id()).into(),
                0,
            )
            .into_response();
        }
    };

    match tenant.status {
        TenantStatus::Active => next.run(req).await,
        TenantStatus::Suspended => {
            state.metrics.record_tenant_resolve_failed();
            ApiErrorResponse::from_error(TenantError::TenantSuspended(ctx.tenant_id()).into(), 0)
                .into_response()
        }
        TenantStatus::Disabled => {
            state.metrics.record_tenant_resolve_failed();
            ApiErrorResponse::from_error(TenantError::TenantDisabled(ctx.tenant_id()).into(), 0)
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use axum::Router;
    use sz_rust_orm_facade::tenant::context::{TenantContext, TenantResolveSource};
    use tower::ServiceExt;

    fn make_state(registry: Arc<TenantRecordRegistry>) -> TenantStatusMiddlewareState {
        TenantStatusMiddlewareState {
            tenant_registry: registry,
            metrics: Arc::new(DataScopeMetrics::new()),
        }
    }

    fn make_app(state: TenantStatusMiddlewareState) -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn_with_state(state, tenant_status_middleware))
    }

    #[tokio::test]
    async fn test_active_passes() {
        let registry = Arc::new(TenantRecordRegistry::new());
        registry.create("acme".into()).unwrap();
        let state = make_state(registry);
        let app = make_app(state);
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(TenantContext::new(1, false, TenantResolveSource::Header));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_suspended_403() {
        let registry = Arc::new(TenantRecordRegistry::new());
        let t = registry.create("acme".into()).unwrap();
        registry
            .update_status(t.tenant_id, TenantStatus::Suspended)
            .unwrap();
        let state = make_state(registry);
        let app = make_app(state);
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut().insert(TenantContext::new(
            t.tenant_id,
            false,
            TenantResolveSource::Header,
        ));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_disabled_403() {
        let registry = Arc::new(TenantRecordRegistry::new());
        let t = registry.create("acme".into()).unwrap();
        registry
            .update_status(t.tenant_id, TenantStatus::Disabled)
            .unwrap();
        let state = make_state(registry);
        let app = make_app(state);
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut().insert(TenantContext::new(
            t.tenant_id,
            false,
            TenantResolveSource::Header,
        ));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_not_found_403() {
        let registry = Arc::new(TenantRecordRegistry::new());
        let state = make_state(registry);
        let app = make_app(state);
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(TenantContext::new(999, false, TenantResolveSource::Header));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_no_context_passes() {
        let registry = Arc::new(TenantRecordRegistry::new());
        let state = make_state(registry);
        let app = make_app(state);
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
