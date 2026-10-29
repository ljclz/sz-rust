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

/// MySQL 测试配置（127.0.0.1:3306, root/$SZ_TEST_DB_PASSWORD, sz_orm_test）
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
            password: std::env::var("SZ_TEST_DB_PASSWORD").unwrap_or_default(),
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
        #[cfg(feature = "v18-audit-chain")]
        audit_sink: {
            // 集成测试保持通道畅通但不落库（避免依赖测试 DB 的 operate_log 表）。
            let (sink, rx) = sz_rust_sz300::middleware::audit_chain::AuditSink::new(None);
            sz_rust_sz300::middleware::audit_chain::spawn_audit_drain(rx);
            sink
        },
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
        metrics_token: None,
        metrics_public: true,
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
        #[cfg(feature = "v18-audit-chain")]
        audit_sink: {
            // 集成测试保持通道畅通但不落库（避免依赖测试 DB 的 operate_log 表）。
            let (sink, rx) = sz_rust_sz300::middleware::audit_chain::AuditSink::new(None);
            sz_rust_sz300::middleware::audit_chain::spawn_audit_drain(rx);
            sink
        },
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
        metrics_token: None,
        metrics_public: true,
    }
}

/// 构造完整路由（含所有中间件：CORS + CSRF + JWT + RBAC + 脱敏 + 审计 + 安全头）
pub fn make_router(state: AppState) -> Router {
    sz_rust_sz300::router::create_router(state)
}

// ============================================================================
// Schema 初始化 —— 直接执行生产迁移文件（2026-10-28 修复「测试手写 DDL 失真」）
//
// 旧夹具用 `CREATE TABLE` 手写了一套与生产无关的 Schema（merchant_name / product /
// 瘦身版 order），导致 SQL 引用的真实列名（name / good / total_weight_g …）
// 在测试里全都不存在，测试通过也证明不了生产可用。
//
// 现在改为：DROP 掉本 crate 的表 → 按版本顺序执行 migrations/*.sql（含 ALTER）。
// SQL 内容用 include_str! 在编译期内嵌，测试与生产永远是同一份 DDL。
// ============================================================================

/// 迁移文件（顺序 = 生产执行顺序；仅 MySQL 版）
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_init", include_str!("../../migrations/001_init.sql")),
    (
        "002_order_status_check",
        include_str!("../../migrations/002_order_status_check.sql"),
    ),
    (
        "003_indexes",
        include_str!("../../migrations/003_indexes.sql"),
    ),
    (
        "004_good_stock",
        include_str!("../../migrations/004_good_stock.sql"),
    ),
    (
        "005_order_idempotency",
        include_str!("../../migrations/005_order_idempotency.sql"),
    ),
    (
        "006_outbox",
        include_str!("../../migrations/006_outbox.sql"),
    ),
    (
        "007_sz_dtx_log",
        include_str!("../../migrations/007_sz_dtx_log.sql"),
    ),
    (
        "add_ai_classification_override",
        include_str!("../../migrations/add_ai_classification_override.sql"),
    ),
    (
        "add_alert_history",
        include_str!("../../migrations/add_alert_history.sql"),
    ),
    (
        "add_config_version",
        include_str!("../../migrations/add_config_version.sql"),
    ),
    (
        "add_sz_dtx_log_manual_intervention",
        include_str!("../../migrations/add_sz_dtx_log_manual_intervention.sql"),
    ),
];

/// 本 crate 迁移创建的全部表（DROP/DELETE 用；无外键，顺序无关）
const KNOWN_TABLES: &[&str] = &[
    "market",
    "merchant",
    "category",
    "good",
    "ai_category",
    "device",
    "order",
    "order_item",
    "settlement",
    "ota_version",
    "operate_log",
    "merchant_user",
    "system_config",
    "outbox_event",
    "sz_dtx_log",
    "ai_classification_override",
    "alert_history",
    "config_version",
];

/// 按版本执行全部迁移（每个测试进程只执行一次）
pub async fn migrate_schema(pool: &Pool) {
    static INIT: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    INIT.get_or_init(|| async {
        assert_disposable_database(pool).await;
        drop_known_tables(pool).await;
        run_migrations(pool).await;
    })
    .await;
}

/// 拒绝在非测试库上重建 schema（本函数会 DROP 表）
async fn assert_disposable_database(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query("SELECT DATABASE() AS db")
        .await
        .expect("查询当前数据库名失败");
    let db = rows
        .first()
        .and_then(|r| r.get("db"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    assert!(
        db.to_ascii_lowercase().contains("test"),
        "拒绝在非测试库 `{db}` 上重建 Schema（夹具会 DROP 表）；\
         请把测试库名配置为包含 test 的独立实例"
    );
}

async fn drop_known_tables(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    for table in KNOWN_TABLES {
        conn.execute(&format!("DROP TABLE IF EXISTS `{table}`"))
            .await
            .unwrap_or_else(|e| panic!("DROP TABLE {table} 失败: {e}"));
    }
}

async fn run_migrations(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    for (version, sql) in MIGRATIONS {
        for stmt in split_sql_statements(sql) {
            conn.execute(&stmt).await.unwrap_or_else(|e| {
                panic!("迁移 {version} 执行失败: {e}\nSQL: {stmt}");
            });
        }
    }
}

/// 按分号切分 SQL 语句（跳过 `--` 行注释、`/* */` 块注释与引号内内容）
fn split_sql_statements(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut in_single, mut in_double) = (false, false);
    let (mut line_comment, mut block_comment) = (false, false);
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();

        if line_comment {
            if c == '\n' {
                line_comment = false;
                cur.push(c);
            }
            i += 1;
            continue;
        }
        if block_comment {
            if c == '*' && next == Some('/') {
                block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if in_single || in_double {
            cur.push(c);
            if c == '\\' {
                if let Some(escaped) = next {
                    cur.push(escaped);
                    i += 2;
                    continue;
                }
            } else if (in_single && c == '\'') || (in_double && c == '"') {
                in_single = false;
                in_double = false;
            }
            i += 1;
            continue;
        }
        match (c, next) {
            ('-', Some('-')) => {
                line_comment = true;
                i += 2;
                continue;
            }
            ('/', Some('*')) => {
                block_comment = true;
                i += 2;
                continue;
            }
            _ => {}
        }
        match c {
            '\'' => {
                in_single = true;
                cur.push(c);
            }
            '"' => {
                in_double = true;
                cur.push(c);
            }
            ';' => {
                let stmt = cur.trim();
                if !stmt.is_empty() {
                    out.push(stmt.to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
        i += 1;
    }

    let tail = cur.trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// 清空全部表数据（DELETE，安全忽略不存在的表）
pub async fn teardown_all(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    for table in KNOWN_TABLES {
        conn.execute(&format!("DELETE FROM `{table}`")).await.ok();
    }
}

async fn truncate_table(pool: &Pool, table: &str) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute(&format!("TRUNCATE TABLE `{table}`"))
        .await
        .unwrap_or_else(|e| panic!("TRUNCATE {table} 失败: {e}"));
}

/// 初始化 Schema 并清空 device
pub async fn setup_device_table(pool: &Pool) {
    migrate_schema(pool).await;
    truncate_table(pool, "device").await;
}

/// 初始化 Schema 并清空 operate_log
pub async fn setup_operate_log_table(pool: &Pool) {
    migrate_schema(pool).await;
    truncate_table(pool, "operate_log").await;
}

/// 初始化 Schema 并清空 merchant
pub async fn setup_merchant_table(pool: &Pool) {
    migrate_schema(pool).await;
    truncate_table(pool, "merchant").await;
}

/// 初始化 Schema 并清空 good（保留旧函数名兼容既有调用：生产表名是 `good`）
pub async fn setup_product_table(pool: &Pool) {
    migrate_schema(pool).await;
    truncate_table(pool, "good").await;
}

/// 初始化 Schema 并清空 order
pub async fn setup_order_table(pool: &Pool) {
    migrate_schema(pool).await;
    truncate_table(pool, "order").await;
}

/// 初始化全部表（执行生产迁移）
pub async fn setup_all_tables(pool: &Pool) {
    migrate_schema(pool).await;
    teardown_all(pool).await;
}

/// 构造测试用审计缓冲（通道保持畅通但不落库），避免集成测试中的丢弃噪音。
#[cfg(feature = "v18-audit-chain")]
pub fn test_audit_sink() -> Arc<sz_rust_sz300::middleware::audit_chain::AuditSink> {
    let (sink, rx) = sz_rust_sz300::middleware::audit_chain::AuditSink::new(None);
    sz_rust_sz300::middleware::audit_chain::spawn_audit_drain(rx);
    sink
}
