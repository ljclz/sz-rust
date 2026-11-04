// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! DataScopeMiddleware E2E 测试

use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use sz_rust_middleware_facade::data_scope::{
    data_scope_middleware, DataScopeMiddlewareState, DataScopeUserContext,
};
use sz_rust_orm_facade::data_scope::context::DataScopeContext;
use sz_rust_orm_facade::data_scope::field_scope::registry::FieldScopePolicyRegistry;
use tower::ServiceExt;

async fn handler(req: Request) -> String {
    let ctx = req.extensions().get::<DataScopeContext>().unwrap();
    format!(
        "user_id={}, dept_id={}, is_super={}",
        ctx.user_id, ctx.dept_id, ctx.is_super
    )
}

fn make_app() -> Router {
    let state = DataScopeMiddlewareState {
        field_scope_registry: Arc::new(FieldScopePolicyRegistry::new()),
    };
    Router::new()
        .route("/", get(handler))
        .layer(from_fn_with_state(state, data_scope_middleware))
}

#[tokio::test]
async fn it_middleware_injects_data_scope_context() {
    let app = make_app();
    let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(42).with_dept(5));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("user_id=42"));
    assert!(text.contains("dept_id=5"));
    assert!(text.contains("is_super=false"));
}

#[tokio::test]
async fn it_middleware_default_context_when_no_user() {
    let app = make_app();
    let req = Request::builder().uri("/").body(Body::empty()).unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("user_id=0"));
    assert!(text.contains("dept_id=0"));
    assert!(text.contains("is_super=false"));
}

#[tokio::test]
async fn it_middleware_super_admin_context() {
    let app = make_app();
    let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
    req.extensions_mut()
        .insert(DataScopeUserContext::new(1).with_super(true).with_dept(0));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("is_super=true"));
}

#[tokio::test]
async fn it_middleware_with_roles() {
    let app = make_app();
    let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
    req.extensions_mut().insert(
        DataScopeUserContext::new(10)
            .with_dept(3)
            .with_roles(vec!["hr".into(), "manager".into()]),
    );

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("user_id=10"));
    assert!(text.contains("dept_id=3"));
}
