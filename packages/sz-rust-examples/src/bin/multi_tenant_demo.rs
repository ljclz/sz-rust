// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T13.4 多租户 SaaS 生产接线示例
//!
//! 演示完整链路：
//! 1. 启动时构造 TenantHotReloadManager + TenantConfigLoader
//! 2. 挂载 build_tenant_admin_router 到主 Router
//! 3. 挂载 tenant_resolve_middleware + tenant_status_middleware 到中间件链
//! 4. 追加 core 层 tenant_middleware（X-Tenant-Id Header 提取 → TenantContext thread-local）
//! 5. 业务 handler 使用 TenantContext 实现数据隔离
//! 6. 平台管理员跨租户查询验证绕过

use std::sync::Arc;

use axum::extract::Extension;
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use sz_rust_middleware_facade::tenant::resolver::{
    tenant_resolve_middleware, TenantResolveMiddlewareState,
};
use sz_rust_middleware_facade::tenant::status_guard::{
    tenant_status_middleware, TenantStatusMiddlewareState,
};

use sz_rust_middleware_facade::tenant_admin::{build_tenant_admin_router, TenantAdminApiState};
use sz_rust_orm_facade::data_scope::ext::audit::TracingAuditLogger;
use sz_rust_orm_facade::data_scope::ext::generation::PolicyGeneration;
use sz_rust_orm_facade::data_scope::ext::notifier::ChangeNotifier;
use sz_rust_orm_facade::data_scope::ext::path_guard::PathGuard;
use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
use sz_rust_orm_facade::tenant::config::TenantConfigRegistry;
use sz_rust_orm_facade::tenant::context::TenantContext;
use sz_rust_orm_facade::tenant::ext::config_loader::TenantConfigLoader;
use sz_rust_orm_facade::tenant::ext::hot_reload::TenantHotReloadManager;
use sz_rust_orm_facade::tenant::record::TenantRecordRegistry;
use sz_rust_orm_facade::tenant::resolver::TenantResolver;
use sz_rust_orm_facade::tenant::scoped_table::TenantScopedTableRegistry;

async fn business_handler(Extension(ctx): Extension<TenantContext>) -> String {
    if ctx.is_platform_admin() {
        format!(
            "平台管理员访问，绕过租户隔离（tenant_id={}）",
            ctx.tenant_id()
        )
    } else {
        format!("租户 {} 的业务数据（自动隔离）", ctx.tenant_id())
    }
}

async fn health_handler() -> &'static str {
    "ok"
}

fn build_app() -> Router {
    let metrics = Arc::new(DataScopeMetrics::new());
    let tenant_registry = Arc::new(TenantRecordRegistry::new());
    let scoped_table_registry = Arc::new(TenantScopedTableRegistry::new());
    let config_registry = Arc::new(TenantConfigRegistry::new());
    let generation = PolicyGeneration::new();
    let notifier = ChangeNotifier::new(64);
    let audit: Arc<dyn sz_rust_orm_facade::data_scope::ext::audit::AuditLogger> =
        Arc::new(TracingAuditLogger);

    let manager = Arc::new(TenantHotReloadManager::new(
        tenant_registry.clone(),
        scoped_table_registry.clone(),
        config_registry.clone(),
        generation,
        notifier,
        metrics.clone(),
        audit,
    ));

    let cwd = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
    let path_guard = PathGuard::new(vec![cwd, std::env::temp_dir()]);
    let config_loader = Arc::new(TenantConfigLoader::new(
        manager.clone(),
        path_guard,
        TenantConfigLoader::DEFAULT_MAX_FILE_SIZE,
    ));

    let admin_state = TenantAdminApiState {
        manager,
        config_loader,
        metrics: metrics.clone(),
        tenant_admin_roles: vec!["tenant_admin".into()],
    };

    let admin_router = build_tenant_admin_router(admin_state);

    let resolve_state = TenantResolveMiddlewareState {
        resolver: Arc::new(TenantResolver::new(None)),
        tenant_registry: tenant_registry.clone(),
        metrics: metrics.clone(),
        tenant_required: true,
    };

    let status_state = TenantStatusMiddlewareState {
        tenant_registry,
        metrics,
    };

    let business_router = Router::new()
        .route("/api/orders", get(business_handler))
        .layer(axum::middleware::from_fn(
            sz_rust_core::multi_tenant::tenant_middleware,
        ))
        .layer(from_fn_with_state(status_state, tenant_status_middleware))
        .layer(from_fn_with_state(resolve_state, tenant_resolve_middleware));

    Router::new()
        .route("/health", get(health_handler))
        .merge(admin_router)
        .merge(business_router)
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt::init();

    let app = build_app();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    tracing::info!("多租户 SaaS 示例服务启动: http://localhost:3000");
    tracing::info!("端点:");
    tracing::info!("  GET  /health                    — 健康检查");
    tracing::info!("  POST /api/tenants               — 创建租户（需平台管理员）");
    tracing::info!("  GET  /api/tenants               — 列出租户");
    tracing::info!("  PUT  /api/tenants/{{id}}/suspend   — 挂起租户");
    tracing::info!("  POST /api/tenant-scoped-tables  — 注册隔离表");
    tracing::info!("  GET  /api/orders                — 业务查询（自动租户隔离）");

    axum::serve(listener, app).await
}
