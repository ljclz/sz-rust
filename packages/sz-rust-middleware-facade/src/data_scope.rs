// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据权限中间件 — 从 request extensions 提取用户上下文，
//! 转换为 DataScopeContext 并注入，预加载字段策略。
//!
//! 执行顺序：Auth → DataScope → Guard → Handler
//! 不阻断请求：即使配置缺失也以安全降级方式继续。

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;
use sz_rust_orm_facade::data_scope::context::DataScopeContext;
use sz_rust_orm_facade::data_scope::field_scope::registry::FieldScopePolicyRegistry;

/// 数据权限用户上下文（由业务层注入到 request extensions）
///
/// 比 `AuthenticatedUser` 多了 dept_id/is_super/roles，
/// 由业务中间件在 Auth 之后注入。
#[derive(Debug, Clone)]
pub struct DataScopeUserContext {
    pub user_id: i64,
    pub dept_id: i64,
    pub is_super: bool,
    pub roles: Vec<String>,
}

impl DataScopeUserContext {
    pub fn new(user_id: i64) -> Self {
        Self {
            user_id,
            dept_id: 0,
            is_super: false,
            roles: Vec::new(),
        }
    }

    pub fn with_dept(mut self, dept_id: i64) -> Self {
        self.dept_id = dept_id;
        self
    }

    pub fn with_super(mut self, is_super: bool) -> Self {
        self.is_super = is_super;
        self
    }

    pub fn with_roles(mut self, roles: Vec<String>) -> Self {
        self.roles = roles;
        self
    }
}

/// 数据权限中间件状态
#[derive(Clone)]
pub struct DataScopeMiddlewareState {
    pub field_scope_registry: Arc<FieldScopePolicyRegistry>,
}

/// 数据权限中间件
pub async fn data_scope_middleware(
    State(_state): State<DataScopeMiddlewareState>,
    req: Request,
    next: Next,
) -> Response {
    let mut req = req;

    let ctx = req
        .extensions()
        .get::<DataScopeUserContext>()
        .map(|u| DataScopeContext::new(u.user_id, u.dept_id, u.is_super))
        .unwrap_or_else(|| {
            tracing::warn!(
                target: "data_scope_middleware",
                "no DataScopeUserContext in extensions, injecting default"
            );
            DataScopeContext::default()
        });

    req.extensions_mut().insert(ctx);
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use axum::Router;
    use sz_rust_orm_facade::data_scope::context::DataScopeContext;
    use tower::ServiceExt;

    async fn handler(req: Request) -> String {
        let ctx = req.extensions().get::<DataScopeContext>().unwrap();
        format!(
            "user_id={}, dept_id={}, is_super={}",
            ctx.user_id, ctx.dept_id, ctx.is_super
        )
    }

    fn make_app(state: DataScopeMiddlewareState) -> Router {
        Router::new()
            .route("/", get(handler))
            .layer(from_fn_with_state(state, data_scope_middleware))
    }

    #[tokio::test]
    async fn test_middleware_injects_context() {
        let state = DataScopeMiddlewareState {
            field_scope_registry: Arc::new(FieldScopePolicyRegistry::new()),
        };
        let app = make_app(state);

        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(42).with_dept(5));

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_default_context_when_no_user() {
        let state = DataScopeMiddlewareState {
            field_scope_registry: Arc::new(FieldScopePolicyRegistry::new()),
        };
        let app = make_app(state);

        let req = Request::builder().uri("/").body(Body::empty()).unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_middleware_super_admin() {
        let state = DataScopeMiddlewareState {
            field_scope_registry: Arc::new(FieldScopePolicyRegistry::new()),
        };
        let app = make_app(state);

        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(1).with_super(true).with_dept(0));

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
