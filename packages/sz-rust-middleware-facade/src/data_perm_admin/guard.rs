// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 管理员鉴权中间件 — `admin_guard_middleware`
//!
//! 从 request extensions 提取 `DataScopeUserContext`，校验是否为管理员：
//! - 缺失上下文 → 401 AUTH_REQUIRED
//! - `is_super` 或 `roles ∩ admin_roles ≠ ∅` 通过；否则 403 ADMIN_REQUIRED

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::HashSet;

use super::error_response::ApiErrorResponse;
use super::router::AdminApiState;
use crate::data_scope::DataScopeUserContext;

/// 管理员鉴权中间件
///
/// 从 `req.extensions()` 提取 `DataScopeUserContext`：
/// - 缺失返回 401 AUTH_REQUIRED
/// - 校验 `is_super || roles ∩ admin_roles ≠ ∅`，失败返回 403 ADMIN_REQUIRED
/// - 通过则 `next.run(req)`
pub async fn admin_guard_middleware(
    State(state): State<AdminApiState>,
    req: Request,
    next: Next,
) -> Response {
    let user = req.extensions().get::<DataScopeUserContext>().cloned();

    let user = match user {
        Some(u) => u,
        None => {
            let gen = state.manager.generation();
            return ApiErrorResponse::from_error(
                sz_rust_orm_facade::data_scope::error::DataScopeError::AuthRequired,
                gen,
            )
            .into_response();
        }
    };

    if !is_admin(&user, &state.admin_roles) {
        let gen = state.manager.generation();
        return ApiErrorResponse::from_error(
            sz_rust_orm_facade::data_scope::error::DataScopeError::AdminRequired,
            gen,
        )
        .into_response();
    }

    next.run(req).await
}

/// 判定用户是否为管理员：`is_super` 或 `roles ∩ admin_roles ≠ ∅`
fn is_admin(user: &DataScopeUserContext, admin_roles: &[String]) -> bool {
    if user.is_super {
        return true;
    }
    if admin_roles.is_empty() {
        return false;
    }
    let admin_set: HashSet<&str> = admin_roles.iter().map(|s| s.as_str()).collect();
    user.roles.iter().any(|r| admin_set.contains(r.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_perm_admin::router::AdminApiState;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use axum::Router;
    use sz_rust_orm_facade::data_scope::custom::CustomGeneratorRegistry;
    use sz_rust_orm_facade::data_scope::ext::audit::TracingAuditLogger;
    use sz_rust_orm_facade::data_scope::ext::config_loader::ConfigLoader;
    use sz_rust_orm_facade::data_scope::ext::generation::PolicyGeneration;
    use sz_rust_orm_facade::data_scope::ext::hot_reload::HotReloadManager;
    use sz_rust_orm_facade::data_scope::ext::notifier::ChangeNotifier;
    use sz_rust_orm_facade::data_scope::ext::path_guard::PathGuard;
    use sz_rust_orm_facade::data_scope::field_scope::registry::FieldScopePolicyRegistry;
    use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
    use sz_rust_orm_facade::data_scope::registry::DataScopeRuleRegistry;
    use tower::ServiceExt;

    async fn ok_handler() -> &'static str {
        "ok"
    }

    fn make_state(admin_roles: Vec<String>) -> AdminApiState {
        let rule_registry = std::sync::Arc::new(DataScopeRuleRegistry::new());
        let policy_registry = std::sync::Arc::new(FieldScopePolicyRegistry::new());
        let custom_registry = std::sync::Arc::new(CustomGeneratorRegistry::new());
        let generation = PolicyGeneration::new();
        let notifier = ChangeNotifier::new(64);
        let metrics = std::sync::Arc::new(DataScopeMetrics::new());
        let audit: std::sync::Arc<dyn sz_rust_orm_facade::data_scope::ext::audit::AuditLogger> =
            std::sync::Arc::new(TracingAuditLogger);
        let manager = std::sync::Arc::new(HotReloadManager::new(
            rule_registry,
            policy_registry,
            custom_registry,
            generation,
            notifier,
            metrics.clone(),
            audit,
        ));
        let path_guard = PathGuard::new(vec![std::env::current_dir().unwrap()]);
        let config_loader = std::sync::Arc::new(ConfigLoader::new(
            manager.clone(),
            path_guard,
            ConfigLoader::DEFAULT_MAX_FILE_SIZE,
        ));
        AdminApiState {
            manager,
            config_loader,
            metrics,
            admin_roles,
        }
    }

    fn make_app(state: AdminApiState) -> Router {
        Router::new()
            .route("/", get(ok_handler))
            .layer(from_fn_with_state(state, admin_guard_middleware))
    }

    #[tokio::test]
    async fn test_missing_token_401() {
        let state = make_state(vec!["admin".into()]);
        let app = make_app(state);
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "AUTH_REQUIRED");
    }

    #[tokio::test]
    async fn test_non_admin_rejected_403() {
        let state = make_state(vec!["admin".into()]);
        let app = make_app(state);
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(10).with_roles(vec!["user".into()]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "ADMIN_REQUIRED");
    }

    #[tokio::test]
    async fn test_super_admin_pass() {
        let state = make_state(vec!["admin".into()]);
        let app = make_app(state);
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(1).with_super(true));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_role_admin_pass() {
        let state = make_state(vec!["admin".into(), "super_admin".into()]);
        let app = make_app(state);
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(5).with_roles(vec!["editor".into(), "admin".into()]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_empty_admin_roles_non_super_rejected() {
        let state = make_state(vec![]);
        let app = make_app(state);
        let mut req = Request::builder().uri("/").body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(DataScopeUserContext::new(10).with_roles(vec!["user".into()]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_is_admin_logic() {
        let user = DataScopeUserContext::new(1).with_super(true);
        assert!(is_admin(&user, &[]));
        assert!(is_admin(&user, &["admin".into()]));

        let user = DataScopeUserContext::new(2).with_roles(vec!["admin".into()]);
        assert!(is_admin(&user, &["admin".into()]));
        assert!(is_admin(&user, &["x".into(), "admin".into()]));

        let user = DataScopeUserContext::new(3).with_roles(vec!["user".into()]);
        assert!(!is_admin(&user, &["admin".into()]));
        assert!(!is_admin(&user, &[]));
    }
}
