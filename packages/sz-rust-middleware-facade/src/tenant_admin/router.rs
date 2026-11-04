// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户管理 API 路由 — TenantAdminApiState 与 build_tenant_admin_router

use std::sync::Arc;

use axum::middleware::from_fn;
use axum::routing::{get, post, put};
use axum::Extension;
use axum::Router;

use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::tenant::ext::config_loader::TenantConfigLoader;
use sz_rust_orm_facade::tenant::ext::hot_reload::TenantHotReloadManager;

use crate::tenant::platform_admin_guard::platform_admin_guard_middleware;
use crate::tenant::tenant_config_guard::{tenant_config_guard_middleware, TenantConfigGuardState};

use super::handlers::{
    activate_tenant, create_tenant, delete_tenant, delete_tenant_config, disable_tenant,
    get_tenant, list_global_configs, list_scoped_tables, list_tenant_configs, list_tenants,
    register_scoped_table, set_global_config, set_tenant_config, suspend_tenant, update_tenant,
};

#[derive(Clone)]
pub struct TenantAdminApiState {
    pub manager: Arc<TenantHotReloadManager>,
    pub config_loader: Arc<TenantConfigLoader>,
    pub metrics: Arc<DataScopeMetrics>,
    pub tenant_admin_roles: Vec<String>,
}

pub fn build_tenant_admin_router(state: TenantAdminApiState) -> Router {
    let config_guard_state = TenantConfigGuardState {
        tenant_admin_roles: state.tenant_admin_roles.clone(),
    };

    let tenant_mgmt_router = Router::new()
        .route("/api/tenants", post(create_tenant).get(list_tenants))
        .route(
            "/api/tenants/{tenant_id}",
            get(get_tenant).put(update_tenant).delete(delete_tenant),
        )
        .route("/api/tenants/{tenant_id}/suspend", put(suspend_tenant))
        .route("/api/tenants/{tenant_id}/activate", put(activate_tenant))
        .route("/api/tenants/{tenant_id}/disable", put(disable_tenant))
        .route(
            "/api/tenant-scoped-tables",
            post(register_scoped_table).get(list_scoped_tables),
        )
        .layer(from_fn(platform_admin_guard_middleware));

    let config_router = Router::new()
        .route("/api/tenant-configs", get(list_tenant_configs))
        .route(
            "/api/tenant-configs/{config_key}",
            put(set_tenant_config).delete(delete_tenant_config),
        )
        .route("/api/global-configs", get(list_global_configs))
        .route("/api/global-configs/{config_key}", put(set_global_config))
        .layer(from_fn(tenant_config_guard_middleware))
        .layer(Extension(config_guard_state));

    tenant_mgmt_router.merge(config_router).with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_orm_facade::data_scope::ext::audit::TracingAuditLogger;
    use sz_rust_orm_facade::data_scope::ext::generation::PolicyGeneration;
    use sz_rust_orm_facade::data_scope::ext::notifier::ChangeNotifier;
    use sz_rust_orm_facade::tenant::config::TenantConfigRegistry;
    use sz_rust_orm_facade::tenant::record::TenantRecordRegistry;
    use sz_rust_orm_facade::tenant::scoped_table::TenantScopedTableRegistry;

    fn make_state() -> TenantAdminApiState {
        let manager = Arc::new(TenantHotReloadManager::new(
            Arc::new(TenantRecordRegistry::new()),
            Arc::new(TenantScopedTableRegistry::new()),
            Arc::new(TenantConfigRegistry::new()),
            PolicyGeneration::new(),
            ChangeNotifier::new(64),
            Arc::new(DataScopeMetrics::new()),
            Arc::new(TracingAuditLogger),
        ));
        let path_guard = sz_rust_orm_facade::data_scope::ext::path_guard::PathGuard::new(vec![
            std::env::current_dir().unwrap(),
            std::env::temp_dir(),
        ]);
        let config_loader = Arc::new(TenantConfigLoader::new(
            manager.clone(),
            path_guard,
            TenantConfigLoader::DEFAULT_MAX_FILE_SIZE,
        ));
        TenantAdminApiState {
            manager,
            config_loader,
            metrics: Arc::new(DataScopeMetrics::new()),
            tenant_admin_roles: vec!["tenant_admin".into()],
        }
    }

    #[tokio::test]
    async fn test_build_router_succeeds() {
        let state = make_state();
        let router = build_tenant_admin_router(state);

        // 注入平台管理员上下文通过 platform_admin_guard 后：
        // 已注册路径 + 未注册方法 → 405；未注册路径 → 404。
        // 以 PATCH 探测证明 /api/tenants 路由真实挂载（不触达 handler，免注册表状态干扰）
        use crate::data_scope::DataScopeUserContext;
        use tower::ServiceExt;
        let mut request = axum::http::Request::builder()
            .method("PATCH")
            .uri("/api/tenants")
            .body(axum::body::Body::empty())
            .unwrap();
        request.extensions_mut().insert(
            DataScopeUserContext::new(1)
                .with_super(true)
                .with_platform_admin(true),
        );
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::METHOD_NOT_ALLOWED
        );
    }
}
