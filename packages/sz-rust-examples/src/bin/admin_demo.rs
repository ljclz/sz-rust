// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//

//! Admin 插件接线示例
//!
//! 演示如何将 `sz-rust-addons-admin` 插件接入主应用：
//! 1. 创建 mock 数据库连接池
//! 2. 实例化 `AdminAddonPlugin`
//! 3. 将插件路由合并到主 Router
//! 4. 注册 CapabilityHook
//!
//! ## 运行
//!
//! ```bash
//! cargo run -p sz-rust-examples --bin admin_demo
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use sz_rust_addons_admin::AdminAddonPlugin;
use sz_rust_addons_loader::capability_hook::CapabilityHook;
use sz_rust_capability::CapabilityRegistry;
use sz_rust_orm_facade::{Connection, ConnectionFactory, DbError, Pool, PoolConfig, Value};

// 复用 tests/admin_wiring.rs 的 mock 模式

type QueryRows = Vec<HashMap<String, Value>>;

struct MockConnection;

impl Connection for MockConnection {
    fn execute<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(0) })
    }
    fn query<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn execute_with_params<'a>(
        &'a mut self,
        _sql: &'a str,
        _params: &'a [Value],
    ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(1) })
    }
    fn query_with_params<'a>(
        &'a mut self,
        _sql: &'a str,
        _params: &'a [Value],
    ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn begin_transaction<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn commit<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn rollback<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
    fn is_connected(&self) -> bool {
        true
    }
    fn ping<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async { true })
    }
    fn close<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

struct MockConnectionFactory;

#[async_trait]
impl ConnectionFactory for MockConnectionFactory {
    async fn create(&self) -> Result<Box<dyn Connection>, DbError> {
        Ok(Box::new(MockConnection))
    }
}

fn make_mock_pool() -> Arc<Pool> {
    let config = PoolConfig::default();
    let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory);
    Arc::new(Pool::new(config, factory).expect("mock pool creation should not fail"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Step 1: 创建 mock 数据库连接池
    let pool = make_mock_pool();

    // Step 2: 实例化 AdminAddonPlugin
    let plugin = AdminAddonPlugin::new(pool, vec!["super_admin".to_string()]);

    // Step 3: 合并插件路由到主 Router
    let admin_router = plugin.router();
    let router = Router::new()
        .route("/", axum::routing::get(|| async { "SZ-Rust" }))
        .merge(admin_router);

    // Step 4: 注册 CapabilityHook
    let hook = plugin.capability_hook();
    let registry = CapabilityRegistry::new();
    let registered = hook.register_capabilities(&registry)?;

    tracing::info!("Admin 插件接线完成");
    tracing::info!("  端点数: 21 (来源: build_admin_addon_router)");
    tracing::info!(
        "  Capability 数: {} (来源: register_capabilities 返回值)",
        registered.len()
    );
    println!(
        "Admin Demo 接线完成 — {} 个 Capability 已注册",
        registered.len()
    );

    // 保持 router 不被优化掉
    drop(router);

    Ok(())
}
