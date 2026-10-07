// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 集成测试共享工具模块
//!
//! 提供：
//! - MySQL 连接池初始化（ensure_mysql）
//! - AppState 构造（make_state）
//! - 测试路由构造（make_router）
//! - 数据库表 setup/teardown

#![allow(dead_code)]

use std::sync::Arc;

use axum::Router;
use sz_rust_core::orm::Pool;
use sz_rust_observability::MetricsRegistry;
use sz_rust_sz300::state::AppState;
use sz_rust_sz300::{config, db};

pub mod assertions;
pub mod fixtures;

/// MySQL 测试配置（127.0.0.1:3306, root/test123, sz_orm_test）
fn mysql_test_config() -> config::AppConfig {
    config::AppConfig {
        server: config::ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8300,
        },
        database: config::DatabaseConfig {
            host: "127.0.0.1".to_string(),
            port: 3306,
            username: "root".to_string(),
            password: "test123".to_string(),
            database: "sz_orm_test".to_string(),
        },
    }
}

/// 尝试连接 MySQL，失败返回 None（测试应跳过）
pub async fn ensure_mysql() -> Option<Pool> {
    let cfg = mysql_test_config();
    match db::init_pool(&cfg).await {
        Ok(pool) => match pool.acquire().await {
            Ok(mut conn) => match conn.query("SELECT 1").await {
                Ok(_) => Some(pool),
                Err(_) => {
                    pool.close_all().await;
                    None
                }
            },
            Err(_) => {
                pool.close_all().await;
                None
            }
        },
        Err(_) => None,
    }
}

/// 构造 AppState（启用所有 v1.8.0 feature gate 字段）
pub fn make_state(pool: Pool) -> AppState {
    let metrics = Arc::new(MetricsRegistry::new());
    let db_pool = Arc::new(pool);
    #[cfg(feature = "v19-graphql-persist")]
    let graphql_schema_db = sz_rust_sz300::graphql::build_schema_with_db(db_pool.clone());
    AppState {
        db_pool,
        pg_pool: None,
        metrics_registry: metrics,
        #[cfg(feature = "v18-rbac")]
        rbac_engine: Arc::new(sz_rust_sz300::rbac::roles::init_rbac_engine()),
        #[cfg(feature = "v18-key-rotation")]
        key_manager: sz_rust_sz300::services::auth_service::default_key_manager_for_tests(),
        #[cfg(feature = "v18-audit-chain")]
        chain_auditor: Arc::new(sz_rust_middleware_facade::audit_chain::ChainHashAuditor::new()),
        #[cfg(feature = "v18-upload")]
        upload_config: sz_rust_sz300::config::upload_config(),
        #[cfg(feature = "v18-graphql")]
        graphql_schema: sz_rust_sz300::graphql::build_schema(),
        #[cfg(feature = "v19-graphql-persist")]
        graphql_schema_db,
        #[cfg(feature = "v18-websocket")]
        ws_manager: Arc::new(sz_rust_websocket::manager::ConnectionManager::with_defaults()),
        #[cfg(feature = "v18-websocket")]
        ws_rooms: Arc::new(sz_rust_websocket::room::RoomManager::new()),
        #[cfg(feature = "v18-sse")]
        sse_service: sz_rust_sz300::services::sse_service::SseService::with_defaults(),
        #[cfg(feature = "v19-plugin-flow")]
        plugin_manager: Arc::new(sz_rust_sz300::services::plugin_manager::PluginManager::new(
            Arc::new(sz_rust_sz300::services::plugin_manager::InMemoryMarketplace::new()),
            "test_pub_key".to_string(),
        )),
    }
}

/// 构造 AppState 并注入自定义插件市场（v1.9.0 插件生态 E2E 测试用）
#[cfg(feature = "v19-plugin-flow")]
pub fn make_state_with_marketplace(
    pool: Pool,
    marketplace: Arc<dyn sz_rust_sz300::services::plugin_manager::PluginMarketplace>,
) -> AppState {
    let metrics = Arc::new(MetricsRegistry::new());
    let db_pool = Arc::new(pool);
    #[cfg(feature = "v19-graphql-persist")]
    let graphql_schema_db = sz_rust_sz300::graphql::build_schema_with_db(db_pool.clone());
    AppState {
        db_pool,
        pg_pool: None,
        metrics_registry: metrics,
        #[cfg(feature = "v18-rbac")]
        rbac_engine: Arc::new(sz_rust_sz300::rbac::roles::init_rbac_engine()),
        #[cfg(feature = "v18-key-rotation")]
        key_manager: sz_rust_sz300::services::auth_service::default_key_manager_for_tests(),
        #[cfg(feature = "v18-audit-chain")]
        chain_auditor: Arc::new(sz_rust_middleware_facade::audit_chain::ChainHashAuditor::new()),
        #[cfg(feature = "v18-upload")]
        upload_config: sz_rust_sz300::config::upload_config(),
        #[cfg(feature = "v18-graphql")]
        graphql_schema: sz_rust_sz300::graphql::build_schema(),
        #[cfg(feature = "v19-graphql-persist")]
        graphql_schema_db,
        #[cfg(feature = "v18-websocket")]
        ws_manager: Arc::new(sz_rust_websocket::manager::ConnectionManager::with_defaults()),
        #[cfg(feature = "v18-websocket")]
        ws_rooms: Arc::new(sz_rust_websocket::room::RoomManager::new()),
        #[cfg(feature = "v18-sse")]
        sse_service: sz_rust_sz300::services::sse_service::SseService::with_defaults(),
        #[cfg(feature = "v19-plugin-flow")]
        plugin_manager: Arc::new(sz_rust_sz300::services::plugin_manager::PluginManager::new(
            marketplace,
            "test_pub_key".to_string(),
        )),
    }
}

/// 构造完整路由（含所有中间件：CORS + CSRF + JWT + RBAC + 脱敏 + 审计 + 安全头）
pub fn make_router(state: AppState) -> Router {
    sz_rust_sz300::router::create_router(state)
}

/// 建表：device（幂等，不 DROP）
pub async fn setup_device_table(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS device (\
         device_id BIGINT AUTO_INCREMENT PRIMARY KEY,\
         merchant_id BIGINT NOT NULL DEFAULT 0,\
         device_sn VARCHAR(64) NOT NULL UNIQUE,\
         device_model VARCHAR(64) NOT NULL DEFAULT '',\
         fw_version VARCHAR(32) NOT NULL DEFAULT '',\
         status INT NOT NULL DEFAULT 0,\
         signal_strength INT NOT NULL DEFAULT 0,\
         bind_at DATETIME NULL,\
         last_online_at DATETIME NULL,\
         created_at DATETIME NULL,\
         updated_at DATETIME NULL\
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建 device 表失败");
    conn.execute("TRUNCATE TABLE device").await.ok();
}

/// 建表：operate_log（幂等，不 DROP）
pub async fn setup_operate_log_table(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS operate_log (\
         id BIGINT AUTO_INCREMENT PRIMARY KEY,\
         merchant_id BIGINT NOT NULL DEFAULT 0,\
         operator VARCHAR(64) NOT NULL DEFAULT '',\
         action VARCHAR(64) NOT NULL DEFAULT '',\
         detail TEXT,\
         ip VARCHAR(45) NOT NULL DEFAULT ''\
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建 operate_log 表失败");
    conn.execute("TRUNCATE TABLE operate_log").await.ok();
}

/// 建表：merchant（幂等，不 DROP）
pub async fn setup_merchant_table(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS merchant (\
         merchant_id BIGINT AUTO_INCREMENT PRIMARY KEY,\
         merchant_name VARCHAR(128) NOT NULL DEFAULT '',\
         contact_phone VARCHAR(32) NOT NULL DEFAULT '',\
         status INT NOT NULL DEFAULT 0,\
         created_at DATETIME NULL,\
         updated_at DATETIME NULL\
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建 merchant 表失败");
    conn.execute("TRUNCATE TABLE merchant").await.ok();
}

/// 建表：product（幂等，不 DROP）
pub async fn setup_product_table(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS product (\
         product_id BIGINT AUTO_INCREMENT PRIMARY KEY,\
         merchant_id BIGINT NOT NULL DEFAULT 0,\
         product_name VARCHAR(128) NOT NULL DEFAULT '',\
         price_fen BIGINT NOT NULL DEFAULT 0,\
         stock INT NOT NULL DEFAULT 0,\
         status INT NOT NULL DEFAULT 0,\
         created_at DATETIME NULL,\
         updated_at DATETIME NULL\
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建 product 表失败");
    conn.execute("TRUNCATE TABLE product").await.ok();
}

/// 建表：order（幂等，不 DROP）
pub async fn setup_order_table(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS `order` (\
         order_id BIGINT AUTO_INCREMENT PRIMARY KEY,\
         order_no VARCHAR(64) NOT NULL DEFAULT '',\
         merchant_id BIGINT NOT NULL DEFAULT 0,\
         device_id BIGINT NOT NULL DEFAULT 0,\
         total_fen BIGINT NOT NULL DEFAULT 0,\
         offline_seq VARCHAR(64) NOT NULL DEFAULT '',\
         item_count INT NOT NULL DEFAULT 0,\
         status INT NOT NULL DEFAULT 0\
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建 order 表失败");
    conn.execute("TRUNCATE TABLE `order`").await.ok();
}

/// 清理所有测试表数据（DELETE，安全忽略不存在表）
pub async fn teardown_all(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute("DELETE FROM device").await.ok();
    conn.execute("DELETE FROM operate_log").await.ok();
    conn.execute("DELETE FROM merchant").await.ok();
    conn.execute("DELETE FROM product").await.ok();
    conn.execute("DELETE FROM `order`").await.ok();
}

/// 建表：全部
pub async fn setup_all_tables(pool: &Pool) {
    setup_merchant_table(pool).await;
    setup_product_table(pool).await;
    setup_device_table(pool).await;
    setup_order_table(pool).await;
    setup_operate_log_table(pool).await;
}
