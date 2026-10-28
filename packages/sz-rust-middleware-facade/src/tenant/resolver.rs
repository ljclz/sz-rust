// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户解析中间件 — 从请求按 JWT → Header → Path 优先级解析 tenant_id，
//! 校验用户归属匹配，注入 TenantContext 到 request extensions。

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::tenant::context::TenantContext;
use sz_rust_orm_facade::tenant::error::TenantError;
use sz_rust_orm_facade::tenant::record::TenantRecordRegistry;
use sz_rust_orm_facade::tenant::resolver::{TenantRequest, TenantResolver};

use crate::data_perm_admin::error_response::ApiErrorResponse;
use crate::data_scope::DataScopeUserContext;

use super::JwtTenantId;

struct ReqAdapter {
    headers: std::collections::HashMap<String, String>,
    jwt_tenant_id: Option<i64>,
}

impl ReqAdapter {
    fn from_request(req: &Request) -> Self {
        let mut headers = std::collections::HashMap::new();
        for (name, value) in req.headers().iter() {
            if let Ok(v) = value.to_str() {
                headers.insert(name.as_str().to_lowercase(), v.to_string());
            }
        }
        Self {
            headers,
            jwt_tenant_id: req.extensions().get::<JwtTenantId>().map(|t| t.0),
        }
    }
}

impl TenantRequest for ReqAdapter {
    fn get_header(&self, name: &str) -> Option<String> {
        self.headers.get(&name.to_lowercase()).cloned()
    }

    fn get_path_param(&self, _name: &str) -> Option<String> {
        None
    }

    fn get_jwt_tenant_id(&self) -> Option<i64> {
        self.jwt_tenant_id
    }
}

#[derive(Clone)]
pub struct TenantResolveMiddlewareState {
    pub resolver: Arc<TenantResolver>,
    pub tenant_registry: Arc<TenantRecordRegistry>,
    pub metrics: Arc<DataScopeMetrics>,
    pub tenant_required: bool,
}

pub async fn tenant_resolve_middleware(
    State(state): State<TenantResolveMiddlewareState>,
    req: Request,
    next: Next,
) -> Response {
    let adapter = ReqAdapter::from_request(&req);
    match state.resolver.resolve(&adapter).await {
        Ok((tenant_id, source)) => {
            let jwt_tenant = req.extensions().get::<JwtTenantId>().map(|t| t.0);
            let header_tenant = adapter
                .get_header("X-Tenant-Id")
                .and_then(|s| s.parse::<i64>().ok());
            if let (Some(jwt), Some(header)) = (jwt_tenant, header_tenant) {
                if jwt != header {
                    state.metrics.record_tenant_resolve_failed();
                    return ApiErrorResponse::from_error(
                        TenantError::TenantMismatch {
                            header_tenant: header,
                            user_tenant: jwt,
                        }
                        .into(),
                        0,
                    )
                    .into_response();
                }
            }

            let is_platform_admin = req
                .extensions()
                .get::<DataScopeUserContext>()
                .map(|u| u.is_platform_admin)
                .unwrap_or(false);

            let ctx = TenantContext::new(tenant_id, is_platform_admin, source);
            let mut req = req;
            req.extensions_mut().insert(ctx);
            state.metrics.record_tenant_request();
            next.run(req).await
        }
        Err(_) => {
            state.metrics.record_tenant_resolve_failed();
            if state.tenant_required {
                ApiErrorResponse::from_error(TenantError::TenantIdRequired.into(), 0)
                    .into_response()
            } else {
                next.run(req).await
            }
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
    use tower::ServiceExt;

    fn make_state(tenant_required: bool) -> TenantResolveMiddlewareState {
        TenantResolveMiddlewareState {
            resolver: Arc::new(TenantResolver::new(None)),
            tenant_registry: Arc::new(TenantRecordRegistry::new()),
            metrics: Arc::new(DataScopeMetrics::new()),
            tenant_required,
        }
    }

    fn make_app(state: TenantResolveMiddlewareState) -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn_with_state(state, tenant_resolve_middleware))
    }

    #[tokio::test]
    async fn test_resolve_success_injects_context() {
        let state = make_state(true);
        let app = make_app(state.clone());
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut().insert(JwtTenantId(42));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(state.metrics.tenant_request_total(), 1);
    }

    #[tokio::test]
    async fn test_missing_tenant_id_required_400() {
        let state = make_state(true);
        let app = make_app(state);
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_global_route_no_tenant_continues() {
        let state = make_state(false);
        let app = make_app(state);
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_header_resolve_success() {
        let state = make_state(true);
        let app = make_app(state);
        let mut req = Request::builder()
            .uri("/")
            .header("X-Tenant-Id", "5")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(1).with_super(true));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_tenant_mismatch_403() {
        let state = make_state(true);
        let app = make_app(state);
        let mut req = Request::builder()
            .uri("/")
            .header("X-Tenant-Id", "5")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(JwtTenantId(3));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_jwt_skips_match_check() {
        let state = make_state(true);
        let app = make_app(state);
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut().insert(JwtTenantId(7));
        req.extensions_mut()
            .insert(DataScopeUserContext::new(1).with_platform_admin(true));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
