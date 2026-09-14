// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! `serve` 命令 — 启动 HTTP 服务（可选加载 admin 插件）
//!
//! 对齐 PHP `php think run`，封装 `sz_rust_core::server::serve_with_graceful_shutdown`。
//!
//! ## 用法
//!
//! ```bash
//! # 基础服务（不加载 admin 插件）
//! sz-rust serve
//!
//! # 加载 admin 插件
//! sz-rust serve --with-admin --addr 0.0.0.0:8080
//! ```

use std::sync::Arc;

use axum::Router;
use sz_rust_addons_admin::AdminAddonPlugin;
use sz_rust_addons_loader::capability_hook::CapabilityHook;
use sz_rust_capability::CapabilityRegistry;
use sz_rust_core::config::AppConfig;
use sz_rust_core::container::App;
use sz_rust_orm_facade::{Connection, ConnectionFactory, DbError, Pool, PoolConfig};

use crate::error::CliError;

/// AnyPool 连接工厂包装器
///
/// 将 `sz_orm_sqlx::any_driver::AnyPool` 适配为 `sz_orm_core::ConnectionFactory`，
/// 使其可用于创建 `sz_orm_core::Pool`。
struct AnyPoolConnectionFactory(sz_orm_sqlx::any_driver::AnyPool);

#[async_trait::async_trait]
impl ConnectionFactory for AnyPoolConnectionFactory {
    async fn create(&self) -> Result<Box<dyn Connection>, DbError> {
        let conn = self
            .0
            .create()
            .await
            .map_err(|e| DbError::ConnectionError(format!("AnyPool create failed: {e}")))?;
        Ok(Box::new(conn))
    }
}

/// 构建 admin 插件路由并注册 Capability
///
/// 纯函数：不启动服务、不打印日志、不 panic。
/// 可被单元测试独立调用。
pub fn build_router_with_admin(pool: Arc<Pool>, admin_roles: Vec<String>) -> (Router, usize) {
    let plugin = AdminAddonPlugin::new(pool, admin_roles);
    let admin_router = plugin.router();
    let base_router = Router::new().route("/", axum::routing::get(|| async { "SZ-Rust" }));
    let merged_router = base_router.merge(admin_router);

    let hook = plugin.capability_hook();
    let registry = CapabilityRegistry::new();
    let registered = hook.register_capabilities(&registry).unwrap_or_default();

    (merged_router, registered.len())
}

/// 从 AppConfig 构建数据库连接池
async fn acquire_pool(config: &AppConfig) -> Result<Arc<Pool>, CliError> {
    let db_name = &config.database.default;
    let conn_config = config
        .database
        .connections
        .get(db_name)
        .ok_or_else(|| CliError::Generic(format!("数据库连接 '{db_name}' 未配置")))?;

    let db_url = build_db_url(conn_config);
    let any_pool = sz_orm_sqlx::any_driver::AnyPool::connect(&db_url)
        .await
        .map_err(|e| CliError::Generic(format!("数据库连接失败: {e}")))?;

    let factory: Arc<dyn ConnectionFactory> = Arc::new(AnyPoolConnectionFactory(any_pool));
    let pool = Pool::new(PoolConfig::default(), factory)
        .map_err(|e| CliError::Generic(format!("连接池创建失败: {e}")))?;
    Ok(Arc::new(pool))
}

/// 从 DatabaseConnection 配置构建数据库 URL
fn build_db_url(conn: &sz_rust_core::config::DatabaseConnection) -> String {
    let driver = match conn.r#type.as_str() {
        "mysql" => "mysql",
        "postgres" | "pgsql" => "postgres",
        "sqlite" => "sqlite",
        other => other,
    };
    format!(
        "{driver}://{}:{}@{}:{}/{}",
        conn.username, conn.password, conn.hostname, conn.hostport, conn.database
    )
}

/// 读取管理员角色列表
///
/// 从环境变量 `SZ_RUST_ADMIN_ROLES`（逗号分隔）读取，缺失时回退到默认值。
fn acquire_admin_roles() -> Vec<String> {
    std::env::var("SZ_RUST_ADMIN_ROLES")
        .ok()
        .and_then(|s| {
            let roles: Vec<String> = s.split(',').map(|r| r.trim().to_string()).collect();
            if roles.is_empty() {
                None
            } else {
                Some(roles)
            }
        })
        .unwrap_or_else(|| {
            tracing::warn!("SZ_RUST_ADMIN_ROLES 未设置，使用默认角色 [super_admin]");
            vec!["super_admin".to_string()]
        })
}

/// 执行 serve 命令
pub async fn execute(with_admin: bool, addr: &str) -> Result<i32, CliError> {
    let config_dir = std::env::var("SZ_RUST_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("config"));
    let config = AppConfig::load_from_dir(&config_dir)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("加载配置失败（使用默认配置）: {e}");
            AppConfig::default()
        });

    let _app = App::init(config.clone());

    let router = if with_admin {
        let pool = acquire_pool(&config).await?;
        let admin_roles = acquire_admin_roles();
        let (router, cap_count) = build_router_with_admin(pool, admin_roles);
        tracing::info!("Admin 插件已加载：{cap_count} 个 Capability 已注册");
        router
    } else {
        Router::new().route("/", axum::routing::get(|| async { "SZ-Rust" }))
    };

    tracing::info!("HTTP 服务启动于 {addr}");
    sz_rust_core::server::serve_with_graceful_shutdown(router, addr)
        .await
        .map_err(CliError::from)?;
    Ok(0)
}
