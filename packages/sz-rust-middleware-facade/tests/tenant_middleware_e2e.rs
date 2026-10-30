// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T12.4 中间件 E2E 测试 — AC-05~AC-09/26
//!
//! 覆盖验收标准：
//! - AC-05: JWT 优先 Header
//! - AC-06: TENANT_MISMATCH 403
//! - AC-07: TENANT_ID_REQUIRED 400
//! - AC-08: TENANT_SUSPENDED 403
//! - AC-09: 平台管理员绕过
//! - AC-26: 指标暴露 tenant_request_total/tenant_resolve_failed_total
//! - 中间件执行顺序：Auth → TenantResolve → TenantStatus → Handler

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use tower::ServiceExt;

use sz_rust_middleware_facade::data_scope::DataScopeUserContext;
use sz_rust_middleware_facade::tenant::resolver::{
    tenant_resolve_middleware, TenantResolveMiddlewareState,
};
use sz_rust_middleware_facade::tenant::status_guard::{
    tenant_status_middleware, TenantStatusMiddlewareState,
};
use sz_rust_middleware_facade::tenant::JwtTenantId;
use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::tenant::context::{TenantContext, TenantResolveSource};
use sz_rust_orm_facade::tenant::record::TenantRecordRegistry;
use sz_rust_orm_facade::tenant::resolver::TenantResolver;
use sz_rust_orm_facade::tenant::status::TenantStatus;

// ============================================================================
// 辅助构造 — 构建完整中间件链
// ============================================================================

struct MiddlewareChain {
    resolve_state: TenantResolveMiddlewareState,
    status_state: TenantStatusMiddlewareState,
    metrics: Arc<DataScopeMetrics>,
    tenant_registry: Arc<TenantRecordRegistry>,
}

impl MiddlewareChain {
    fn new(tenant_required: bool) -> Self {
        let metrics = Arc::new(DataScopeMetrics::new());
        let tenant_registry = Arc::new(TenantRecordRegistry::new());
        let resolver = Arc::new(TenantResolver::new(None));

        let resolve_state = TenantResolveMiddlewareState {
            resolver,
            tenant_registry: tenant_registry.clone(),
            metrics: metrics.clone(),
            tenant_required,
        };

        let status_state = TenantStatusMiddlewareState {
            tenant_registry: tenant_registry.clone(),
            metrics: metrics.clone(),
        };

        Self {
            resolve_state,
            status_state,
            metrics,
            tenant_registry,
        }
    }

    fn build_router(self) -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(from_fn_with_state(
                self.status_state,
                tenant_status_middleware,
            ))
            .layer(from_fn_with_state(
                self.resolve_state,
                tenant_resolve_middleware,
            ))
    }
}

async fn read_body(resp: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

// ============================================================================
// AC-05: JWT 优先 Header
// ============================================================================

#[tokio::test]
async fn ac05_jwt_takes_priority_over_header() {
    let chain = MiddlewareChain::new(true);
    let tenant = chain.tenant_registry.create("acme".into()).unwrap();
    let app = chain.build_router();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", &tenant.tenant_id.to_string())
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(JwtTenantId(tenant.tenant_id));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn ac05_jwt_only_no_header() {
    let chain = MiddlewareChain::new(true);
    let t7 = chain.tenant_registry.create("t7".into()).unwrap();
    let app = chain.build_router();

    let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
    req.extensions_mut().insert(JwtTenantId(t7.tenant_id));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn ac05_header_only_no_jwt() {
    let chain = MiddlewareChain::new(true);
    let t5 = chain.tenant_registry.create("t5".into()).unwrap();
    let app = chain.build_router();

    let req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", &t5.tenant_id.to_string())
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ============================================================================
// AC-06: TENANT_MISMATCH 403
// ============================================================================

#[tokio::test]
async fn ac06_tenant_mismatch_403() {
    let chain = MiddlewareChain::new(true);
    let app = chain.build_router();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", "5")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(JwtTenantId(3));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "TENANT_MISMATCH");
}

// ============================================================================
// AC-07: TENANT_ID_REQUIRED 400
// ============================================================================

#[tokio::test]
async fn ac07_tenant_id_required_400() {
    let chain = MiddlewareChain::new(true);
    let app = chain.build_router();

    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "TENANT_ID_REQUIRED");
}

#[tokio::test]
async fn ac07_no_tenant_required_continues() {
    let chain = MiddlewareChain::new(false);
    let app = chain.build_router();

    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ============================================================================
// AC-08: TENANT_SUSPENDED 403
// ============================================================================

#[tokio::test]
async fn ac08_tenant_suspended_403() {
    let chain = MiddlewareChain::new(true);
    let tenant = chain.tenant_registry.create("acme".into()).unwrap();
    chain
        .tenant_registry
        .update_status(tenant.tenant_id, TenantStatus::Suspended)
        .unwrap();

    let app = chain.build_router();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", &tenant.tenant_id.to_string())
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(JwtTenantId(tenant.tenant_id));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "TENANT_SUSPENDED");
}

#[tokio::test]
async fn ac08_tenant_disabled_403() {
    let chain = MiddlewareChain::new(true);
    let tenant = chain.tenant_registry.create("acme".into()).unwrap();
    chain
        .tenant_registry
        .update_status(tenant.tenant_id, TenantStatus::Disabled)
        .unwrap();

    let app = chain.build_router();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", &tenant.tenant_id.to_string())
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(JwtTenantId(tenant.tenant_id));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "TENANT_DISABLED");
}

#[tokio::test]
async fn ac08_tenant_not_found_403() {
    let chain = MiddlewareChain::new(true);
    let app = chain.build_router();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", "999")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(JwtTenantId(999));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let json = read_body(resp).await;
    assert_eq!(json["code"], "TENANT_NOT_FOUND");
}

#[tokio::test]
async fn ac08_tenant_active_passes() {
    let chain = MiddlewareChain::new(true);
    let tenant = chain.tenant_registry.create("acme".into()).unwrap();

    let app = chain.build_router();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", &tenant.tenant_id.to_string())
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(JwtTenantId(tenant.tenant_id));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ============================================================================
// AC-09: 平台管理员绕过
// ============================================================================

#[tokio::test]
async fn ac09_platform_admin_context_propagated() {
    let chain = MiddlewareChain::new(true);
    let tenant = chain.tenant_registry.create("acme".into()).unwrap();

    let app = chain.build_router();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", &tenant.tenant_id.to_string())
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(JwtTenantId(tenant.tenant_id));
    req.extensions_mut().insert(
        DataScopeUserContext::new(1)
            .with_super(true)
            .with_platform_admin(true),
    );

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn ac09_platform_admin_active_tenant_passes() {
    let chain = MiddlewareChain::new(true);
    let tenant = chain.tenant_registry.create("acme".into()).unwrap();

    let metrics = chain.metrics.clone();
    let app = chain.build_router();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", &tenant.tenant_id.to_string())
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(JwtTenantId(tenant.tenant_id));
    req.extensions_mut().insert(
        DataScopeUserContext::new(1)
            .with_super(true)
            .with_platform_admin(true),
    );

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(metrics.tenant_isolation_bypass_total(), 0);
}

// ============================================================================
// AC-26: 指标暴露 tenant_request_total / tenant_resolve_failed_total
// ============================================================================

#[tokio::test]
async fn ac26_metrics_tenant_request_total() {
    let chain = MiddlewareChain::new(true);
    let metrics = chain.metrics.clone();
    let app = chain.build_router();

    let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
    req.extensions_mut().insert(JwtTenantId(1));

    let _ = app.oneshot(req).await.unwrap();
    assert_eq!(metrics.tenant_request_total(), 1);
}

#[tokio::test]
async fn ac26_metrics_tenant_resolve_failed_total() {
    let chain = MiddlewareChain::new(true);
    let metrics = chain.metrics.clone();
    let app = chain.build_router();

    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let _ = app.oneshot(req).await.unwrap();

    assert!(metrics.tenant_resolve_failed_total() > 0);
}

#[tokio::test]
async fn ac26_metrics_tenant_resolve_failed_on_mismatch() {
    let chain = MiddlewareChain::new(true);
    let metrics = chain.metrics.clone();
    let app = chain.build_router();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", "5")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(JwtTenantId(3));

    let _ = app.oneshot(req).await.unwrap();
    assert!(metrics.tenant_resolve_failed_total() > 0);
}

// ============================================================================
// 中间件执行顺序验证：TenantResolve → TenantStatus → Handler
// ============================================================================

#[tokio::test]
async fn it_middleware_order_resolve_before_status() {
    let chain = MiddlewareChain::new(true);
    let tenant = chain.tenant_registry.create("acme".into()).unwrap();

    let app = chain.build_router();

    let mut req = Request::builder()
        .uri("/")
        .header("X-Tenant-Id", &tenant.tenant_id.to_string())
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(JwtTenantId(tenant.tenant_id));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn it_no_tenant_context_status_middleware_passes() {
    let chain = MiddlewareChain::new(false);
    let app = chain.build_router();

    let req = Request::builder().uri("/").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ============================================================================
// TenantContext 注入验证
// ============================================================================

#[tokio::test]
async fn it_tenant_context_injected_by_resolve_middleware() {
    let metrics = Arc::new(DataScopeMetrics::new());
    let tenant_registry = Arc::new(TenantRecordRegistry::new());
    let resolver = Arc::new(TenantResolver::new(None));

    let resolve_state = TenantResolveMiddlewareState {
        resolver,
        tenant_registry: tenant_registry.clone(),
        metrics: metrics.clone(),
        tenant_required: true,
    };

    let app = Router::new()
        .route(
            "/",
            get(|req: Request<Body>| async move {
                let ctx = req.extensions().get::<TenantContext>().unwrap();
                assert_eq!(ctx.tenant_id(), 42);
                assert_eq!(ctx.resolve_source(), TenantResolveSource::Jwt);
                "ok"
            }),
        )
        .layer(from_fn_with_state(resolve_state, tenant_resolve_middleware));

    let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
    req.extensions_mut().insert(JwtTenantId(42));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ============================================================================
// DataScopeUserContext + TenantContext 联合验证
// ============================================================================

#[tokio::test]
async fn it_platform_admin_context_propagates_to_tenant_context() {
    let metrics = Arc::new(DataScopeMetrics::new());
    let tenant_registry = Arc::new(TenantRecordRegistry::new());
    let resolver = Arc::new(TenantResolver::new(None));

    let resolve_state = TenantResolveMiddlewareState {
        resolver,
        tenant_registry: tenant_registry.clone(),
        metrics: metrics.clone(),
        tenant_required: true,
    };

    let app = Router::new()
        .route(
            "/",
            get(|req: Request<Body>| async move {
                let ctx = req.extensions().get::<TenantContext>().unwrap();
                assert!(ctx.is_platform_admin());
                "ok"
            }),
        )
        .layer(from_fn_with_state(resolve_state, tenant_resolve_middleware));

    let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
    req.extensions_mut().insert(JwtTenantId(1));
    req.extensions_mut().insert(
        DataScopeUserContext::new(1)
            .with_super(true)
            .with_platform_admin(true),
    );

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
