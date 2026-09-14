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
//!
//! # 生产级配置
//! sz-rust serve --with-admin --workers 4 --grace-timeout 30 --health --access-log
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use sz_rust_addons_admin::AdminAddonPlugin;
use sz_rust_addons_loader::capability_hook::CapabilityHook;
use sz_rust_capability::CapabilityRegistry;
use sz_rust_core::config::AppConfig;
use sz_rust_core::container::App;
use sz_rust_orm_facade::{Connection, ConnectionFactory, DbError, Pool, PoolConfig};

use crate::error::CliError;

mod access_log;
mod runtime;
mod signal;
mod watcher;

pub use runtime::{build_runtime, resolve_workers, validate_workers};

/// serve 命令参数集合
///
/// 由 clap `Command::Serve` 变体字段映射构造，用于解耦 CLI 解析与业务逻辑。
#[derive(Debug, Clone)]
pub struct ServeArgs {
    /// 启用 admin 插件（加载 /api/admin/* 路由 + Capability 注册）
    pub with_admin: bool,
    /// 监听地址（默认 0.0.0.0:8080）
    pub addr: String,
    /// 启用配置热重载（监听 config/ 目录文件变更）
    pub watch_config: bool,
    /// worker 线程数（None 时用配置文件值或 CPU 核心数）
    pub workers: Option<u16>,
    /// 优雅关闭超时秒数（None 时用配置文件值或默认 30）
    pub grace_timeout: Option<u16>,
    /// TLS 证书文件路径
    pub tls_cert: Option<PathBuf>,
    /// TLS 私钥文件路径
    pub tls_key: Option<PathBuf>,
    /// 启用访问日志中间件
    pub access_log: bool,
    /// 启用健康检查端点（默认 true）
    pub health: bool,
}

impl ServeArgs {
    /// 校验参数合法性
    pub fn validate(&self) -> Result<(), CliError> {
        if let Some(w) = self.workers {
            if w == 0 {
                return Err(CliError::Generic("worker 数量必须 >= 1".to_string()));
            }
            if w > 1024 {
                return Err(CliError::Generic("worker 数量超过上限 1024".to_string()));
            }
        }
        if let Some(t) = self.grace_timeout {
            if t > 300 {
                return Err(CliError::Generic("优雅关闭超时超过上限 300 秒".to_string()));
            }
        }
        if self.tls_cert.is_some() != self.tls_key.is_some() {
            return Err(CliError::Generic(
                "--tls-cert 和 --tls-key 必须同时提供或同时缺失".to_string(),
            ));
        }
        Ok(())
    }
}

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

/// 执行 serve 命令（同步入口）
///
/// 构建指定 worker 数的 multi-thread runtime，在 runtime 上 block_on 执行 async 逻辑。
/// 调用方应在 `spawn_blocking` 线程上调用此函数，避免 runtime 嵌套。
pub fn execute(args: ServeArgs) -> Result<i32, CliError> {
    args.validate()?;

    let config_dir = std::env::var("SZ_RUST_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("config"));

    let config = {
        let tmp_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| CliError::Generic(format!("临时 runtime 构建失败: {e}")))?;
        tmp_rt.block_on(async {
            AppConfig::load_from_dir(&config_dir)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("加载配置失败（使用默认配置）: {e}");
                    AppConfig::default()
                })
        })
    };

    let workers = resolve_workers(args.workers, config.server.workers);
    tracing::info!("使用 {workers} 个 worker 线程");

    let runtime = build_runtime(workers)?;
    runtime.block_on(execute_async(args, config, config_dir))
}

/// serve 命令的 async 内部逻辑
async fn execute_async(
    args: ServeArgs,
    config: AppConfig,
    config_dir: std::path::PathBuf,
) -> Result<i32, CliError> {
    if args.watch_config {
        let (reload_tx, reload_rx) = tokio::sync::mpsc::channel::<std::path::PathBuf>(16);
        match watcher::ConfigWatcher::start(&config_dir, reload_tx) {
            Ok(_) => {
                tracing::info!("配置热重载已启用，监听目录: {}", config_dir.display());
                watcher::spawn_reload_coordinator(reload_rx, config_dir.clone());
            }
            Err(e) => {
                tracing::warn!("配置热重载启动失败，降级为不启用: {e}");
            }
        }
    }

    let (reload_signal_tx, mut reload_signal_rx) = tokio::sync::mpsc::channel::<()>(1);
    let (loglevel_tx, mut loglevel_rx) = tokio::sync::mpsc::channel::<()>(1);
    signal::install_runtime_signals(reload_signal_tx, loglevel_tx);
    let signal_config_dir = config_dir.clone();
    tokio::spawn(async move {
        while reload_signal_rx.recv().await.is_some() {
            match watcher::reload_config(&signal_config_dir).await {
                Ok(_) => tracing::info!("信号触发配置重载成功（数据库/路由变更需重启生效）"),
                Err(e) => tracing::error!("信号触发配置重载失败，保留旧配置: {e}"),
            }
        }
    });
    tokio::spawn(async move {
        let mut current_level = tracing::Level::INFO;
        while loglevel_rx.recv().await.is_some() {
            current_level = signal::log_level_cycle(current_level);
            tracing::info!("日志级别切换为 {current_level}");
        }
    });

    let _app = App::init(config.clone());

    let router = if args.with_admin {
        let pool = acquire_pool(&config).await?;
        let admin_roles = acquire_admin_roles();
        let (router, cap_count) = build_router_with_admin(pool, admin_roles);
        tracing::info!("Admin 插件已加载：{cap_count} 个 Capability 已注册");
        router
    } else {
        Router::new().route("/", axum::routing::get(|| async { "SZ-Rust" }))
    };

    let router = if args.health {
        tracing::info!(
            "健康检查端点已启用：GET /health/ (liveness) + GET /health/ready (readiness)"
        );
        router.merge(sz_rust_core::health::default_health_router())
    } else {
        router
    };

    let router = if args.access_log {
        tracing::info!("访问日志中间件已启用");
        router.layer(axum::middleware::from_fn(access_log::access_log_handler))
    } else {
        router
    };

    let grace_timeout = args.grace_timeout.unwrap_or(config.server.grace_timeout);
    let timeout = std::time::Duration::from_secs(grace_timeout as u64);

    if let (Some(cert), Some(key)) = (&args.tls_cert, &args.tls_key) {
        tracing::info!(
            "HTTPS 服务启动于 {}（TLS 证书: {}，优雅关闭超时 {}s）",
            args.addr,
            cert.display(),
            grace_timeout
        );
        let serve_tls =
            sz_rust_core::h2::serve_h2_with_graceful_shutdown(router, &args.addr, cert, key);
        match tokio::time::timeout(timeout, serve_tls).await {
            Ok(result) => {
                result.map_err(|e| CliError::Generic(format!("TLS 服务错误: {e}")))?;
            }
            Err(_) => {
                tracing::warn!("TLS 优雅关闭超时，强制中断剩余连接");
            }
        }
    } else {
        tracing::info!(
            "HTTP 服务启动于 {}（优雅关闭超时 {}s）",
            args.addr,
            grace_timeout
        );
        sz_rust_core::server::serve_with_graceful_shutdown_timeout(router, &args.addr, timeout)
            .await
            .map_err(CliError::from)?;
    }
    Ok(0)
}
