// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! Admin 插件接线示例
//!
//! 演示如何将 `sz-rust-addons-admin` 插件接入主应用：
//! 1. 创建数据库连接池（通过 sz-orm-sqlx 或自定义 ConnectionFactory）
//! 2. 实例化 `AdminAddonPlugin`
//! 3. 将插件路由合并到主 Router
//! 4. 注册 CapabilityHook
//!
//! ## 运行
//!
//! ```bash
//! cargo run -p sz-rust-examples --bin admin_demo
//! ```

use sz_rust_core::config::AppConfig;
use sz_rust_core::container::App;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config_dir = std::env::var("SZ_RUST_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("config"));
    let config = AppConfig::load_from_dir(&config_dir)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("加载配置失败（使用默认配置）: {}", e);
            AppConfig::default()
        });

    let _app = App::init(config);
    tracing::info!("App 容器初始化完成");

    // === Admin 插件接线模式 ===
    //
    // Step 1: 创建数据库连接池
    //   let pool: Arc<Pool> = Arc::new(Pool::new(
    //       PoolConfig::default(),
    //       Arc::new(SqlxConnectionFactory::new(db_url)),
    //   )?);
    //
    // Step 2: 实例化 AdminAddonPlugin
    //   let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);
    //
    // Step 3: 合并插件路由到主 Router
    //   let admin_router = plugin.router();
    //   let router = Router::new()
    //       .route("/", axum::routing::get(|| async { "SZ-Rust" }))
    //       .merge(admin_router);
    //
    // Step 4: 注册 CapabilityHook
    //   let hook = plugin.capability_hook();
    //   let registry = CapabilityRegistry::new();
    //   let registered = hook.register_capabilities(&registry)?;
    //   tracing::info!("已注册 {} 个 admin Capability", registered.len());
    //
    // Step 5: 启动 HTTP 服务
    //   axum::serve(listener, router).await?;

    tracing::info!("Admin Demo — 接线模式请参考源码注释和 tests/admin_wiring.rs");
    Ok(())
}
