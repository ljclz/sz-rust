// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

//! data_scope_middleware 生产接线示例
//!
//! 演示完整链路：
//! 1. 构造 DataScopeMiddlewareState（FieldScopePolicyRegistry）
//! 2. 挂载 data_scope_middleware 到中间件链
//! 3. 业务 handler 从 extensions 读取 DataScopeContext
//! 4. 未注入 DataScopeUserContext 时自动降级为默认值

use std::sync::Arc;

use axum::extract::Request;
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use sz_rust_middleware_facade::data_scope::{
    data_scope_middleware, DataScopeMiddlewareState, DataScopeUserContext,
};
use sz_rust_orm_facade::data_scope::context::DataScopeContext;
use sz_rust_orm_facade::data_scope::field_scope::registry::FieldScopePolicyRegistry;

async fn business_handler(req: Request) -> String {
    let ctx = req
        .extensions()
        .get::<DataScopeContext>()
        .cloned()
        .unwrap_or_default();
    format!(
        "user_id={}, dept_id={}, is_super={}",
        ctx.user_id, ctx.dept_id, ctx.is_super
    )
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn inject_user_context_handler() -> impl axum::response::IntoResponse {
    use axum::response::Response;
    let mut resp: Response = Response::new(axum::body::Body::from("DataScopeUserContext injected"));
    resp.extensions_mut()
        .insert(DataScopeUserContext::new(10).with_dept(5).with_super(false));
    resp
}

fn build_app() -> Router {
    let state = DataScopeMiddlewareState {
        field_scope_registry: Arc::new(FieldScopePolicyRegistry::new()),
    };

    let business_router = Router::new()
        .route("/api/data", get(business_handler))
        .layer(from_fn_with_state(state, data_scope_middleware));

    Router::new()
        .route("/health", get(health_handler))
        .route("/api/inject", get(inject_user_context_handler))
        .merge(business_router)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt::init();

    let app = build_app();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001").await?;
    tracing::info!("data_scope_middleware 示例服务启动: http://localhost:3001");
    tracing::info!("端点:");
    tracing::info!("  GET /health     — 健康检查");
    tracing::info!("  GET /api/data   — 业务查询（自动注入 DataScopeContext）");

    axum::serve(listener, app).await
}
