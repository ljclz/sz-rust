// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P1-3 OTA 升级端到端测试基线（spec 5.3）
//!
//! 覆盖 OTA 版本查询/升级触发/状态跟踪全链路，使用真实 MySQL 9.6。
//! 运行方式：
//! ```
//! cargo test --package sz-rust-sz300 --test e2e_ota_test -- --ignored
//! ```

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::LazyLock;

use axum::body::Body;
use axum::http::Request;
use sz_rust_core::orm::{Pool, Value};
use sz_rust_sz300::state::AppState;
use sz_rust_sz300::{config, db, services};
use tokio::sync::Mutex;

static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn mysql_config() -> config::AppConfig {
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

async fn ensure_mysql() -> Option<Pool> {
    let cfg = mysql_config();
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

async fn make_state(pool: Pool) -> AppState {
    AppState {
        db_pool: Arc::new(pool.clone()),
        pg_pool: None,
        metrics_registry: Arc::new(sz_rust_observability::MetricsRegistry::new()),
        #[cfg(feature = "v18-rbac")]
        rbac_engine: std::sync::Arc::new(sz_rust_sz300::rbac::roles::init_rbac_engine()),
        #[cfg(feature = "v18-key-rotation")]
        key_manager: sz_rust_sz300::services::auth_service::default_key_manager_for_tests(),
        #[cfg(feature = "v18-audit-chain")]
        chain_auditor: std::sync::Arc::new(
            sz_rust_middleware_facade::audit_chain::ChainHashAuditor::new(),
        ),
        #[cfg(feature = "v18-upload")]
        upload_config: sz_rust_sz300::config::upload_config(),
        #[cfg(feature = "v18-graphql")]
        graphql_schema: sz_rust_sz300::graphql::build_schema(),
        #[cfg(feature = "v19-graphql-persist")]
        graphql_schema_db: sz_rust_sz300::graphql::build_schema_with_db(std::sync::Arc::new(
            pool.clone(),
        )),
        #[cfg(feature = "v18-websocket")]
        ws_manager: std::sync::Arc::new(
            sz_rust_websocket::manager::ConnectionManager::with_defaults(),
        ),
        #[cfg(feature = "v18-websocket")]
        ws_rooms: std::sync::Arc::new(sz_rust_websocket::room::RoomManager::new()),
        #[cfg(feature = "v18-sse")]
        sse_service: sz_rust_sz300::services::sse_service::SseService::with_defaults(),
    }
}

async fn setup_device_table(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute("DROP TABLE IF EXISTS device").await.ok();
    conn.execute(
        "CREATE TABLE device (\
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
}

async fn setup_ota_table(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute("DROP TABLE IF EXISTS ota_version").await.ok();
    conn.execute(
        "CREATE TABLE ota_version (\
         ota_id BIGINT AUTO_INCREMENT PRIMARY KEY,\
         version VARCHAR(32) NOT NULL,\
         model VARCHAR(64) NOT NULL DEFAULT '',\
         url VARCHAR(255) NOT NULL DEFAULT '',\
         md5 VARCHAR(64) NOT NULL DEFAULT '',\
         changelog TEXT,\
         size BIGINT NOT NULL DEFAULT 0,\
         force_update TINYINT NOT NULL DEFAULT 0,\
         status TINYINT NOT NULL DEFAULT 0,\
         created_at DATETIME NULL,\
         updated_at DATETIME NULL\
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await
    .expect("建 ota_version 表失败");
}

async fn cleanup(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute("DROP TABLE IF EXISTS device").await.ok();
    conn.execute("DROP TABLE IF EXISTS ota_version").await.ok();
}

async fn insert_device(pool: &Pool, device_sn: &str, merchant_id: i64) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO device (device_sn, merchant_id, status) VALUES (?, ?, 1)",
        &[
            Value::String(device_sn.to_string()),
            Value::I64(merchant_id),
        ],
    )
    .await
    .expect("插入 device 失败");
    let rows = conn
        .query_with_params(
            "SELECT device_id FROM device WHERE device_sn = ?",
            &[Value::String(device_sn.to_string())],
        )
        .await
        .expect("查询 device_id 失败");
    rows[0]
        .get("device_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}

/// 插入 OTA 版本记录
async fn insert_ota_version(pool: &Pool, version: &str, status: i8, url: &str) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO ota_version (version, model, url, md5, changelog, size, force_update, status) \
         VALUES (?, 'M100', ?, '', 'test changelog', 1024, 0, ?)",
        &[
            Value::String(version.to_string()),
            Value::String(url.to_string()),
            Value::I8(status),
        ],
    )
    .await
    .expect("插入 ota_version 失败");
}

async fn read_json(response: axum::response::Response) -> serde_json::Value {
    let (_parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .expect("读取 body 失败");
    serde_json::from_slice(&bytes).expect("解析 JSON 失败")
}

fn json_request(body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/device/ota")
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

// ============================================================================
// spec 5.3.1: OTA 版本查询端到端
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_ota_query_enabled_version_returns_some() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_ota_table(&pool).await;
    insert_ota_version(&pool, "1.2.0", 1, "http://ota.example.com/v1.2.0.bin").await;

    let result = services::device_service::DeviceService::get_ota_version(&pool, "1.2.0")
        .await
        .expect("get_ota_version 失败");
    assert!(result.is_some(), "status=1 的版本应返回 Some");
    let row = result.unwrap();
    let version = row.get("version").and_then(|v| v.as_str()).unwrap_or("");
    let url = row.get("url").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(version, "1.2.0", "版本号应匹配");
    assert!(
        url.contains("v1.2.0.bin"),
        "URL 应包含固件文件名, 实际: {}",
        url
    );

    cleanup(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_ota_query_disabled_version_returns_none() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_ota_table(&pool).await;
    insert_ota_version(&pool, "2.0.0", 0, "http://ota.example.com/v2.0.0.bin").await;

    let result = services::device_service::DeviceService::get_ota_version(&pool, "2.0.0")
        .await
        .expect("get_ota_version 失败");
    assert!(result.is_none(), "status=0 的版本应返回 None");

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.3.2: OTA 升级触发端到端
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_ota_trigger_valid_version_returns_success() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_ota_table(&pool).await;
    let device_id = insert_device(&pool, "SN_OTA_TRIGGER", 100).await;
    insert_ota_version(&pool, "1.5.0", 1, "http://ota.example.com/v1.5.0.bin").await;
    let state = make_state(pool.clone()).await;

    let req = json_request(serde_json::json!({
        "device_id": device_id,
        "ota_version": "1.5.0"
    }));
    let response =
        sz_rust_sz300::controllers::device::trigger_ota(axum::extract::State(state), req).await;
    let json = read_json(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    assert_eq!(code, 1, "有效 OTA 触发应返回 code=1, 实际: {:?}", json);

    let data = json.get("data").unwrap_or(&serde_json::Value::Null);
    let ota_info = data.get("ota_info").unwrap_or(&serde_json::Value::Null);
    let returned_version = ota_info
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(returned_version, "1.5.0", "响应应包含 OTA 版本信息");

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.3.3: OTA 版本不存在处理
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_ota_trigger_unknown_version_returns_error() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_ota_table(&pool).await;
    let device_id = insert_device(&pool, "SN_OTA_UNKNOWN", 100).await;
    let state = make_state(pool.clone()).await;

    let req = json_request(serde_json::json!({
        "device_id": device_id,
        "ota_version": "99.99.99"
    }));
    let response =
        sz_rust_sz300::controllers::device::trigger_ota(axum::extract::State(state), req).await;
    let json = read_json(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(1);
    assert_eq!(code, 0, "不存在的 OTA 版本应返回 code=0");
    let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("OTA 版本不存在") || msg.contains("不存在"),
        "错误消息应包含'不存在', 实际: {}",
        msg
    );

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.3.4: OTA 缺少设备标识处理
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_ota_trigger_missing_device_id_returns_error() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_ota_table(&pool).await;
    let state = make_state(pool.clone()).await;

    let req = json_request(serde_json::json!({
        "ota_version": "1.0.0"
    }));
    let response =
        sz_rust_sz300::controllers::device::trigger_ota(axum::extract::State(state), req).await;
    let json = read_json(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(1);
    assert_eq!(code, 0, "缺少 device_id 应返回 code=0");
    let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("device_id"),
        "错误消息应包含 device_id, 实际: {}",
        msg
    );

    cleanup(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_ota_trigger_missing_ota_version_returns_error() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_ota_table(&pool).await;
    let device_id = insert_device(&pool, "SN_OTA_NO_VER", 100).await;
    let state = make_state(pool.clone()).await;

    let req = json_request(serde_json::json!({
        "device_id": device_id
    }));
    let response =
        sz_rust_sz300::controllers::device::trigger_ota(axum::extract::State(state), req).await;
    let json = read_json(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(1);
    assert_eq!(code, 0, "缺少 ota_version 应返回 code=0");
    let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("ota_version"),
        "错误消息应包含 ota_version, 实际: {}",
        msg
    );

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.3.5: OTA 升级状态跟踪（状态流转：禁用→启用）
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_ota_status_tracking_disabled_to_enabled() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_ota_table(&pool).await;
    insert_ota_version(&pool, "3.0.0", 0, "http://ota.example.com/v3.0.0.bin").await;

    // 初始状态 status=0（禁用），查询应返回 None
    let result_initial = services::device_service::DeviceService::get_ota_version(&pool, "3.0.0")
        .await
        .expect("初始查询失败");
    assert!(
        result_initial.is_none(),
        "status=0 时应返回 None（禁用状态）"
    );

    // 状态流转：禁用 → 启用（UPDATE status = 1）
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute_with_params(
            "UPDATE ota_version SET status = 1 WHERE version = ?",
            &[Value::String("3.0.0".to_string())],
        )
        .await
        .expect("更新 OTA 状态失败");
    }

    // 启用后查询应返回 Some
    let result_enabled = services::device_service::DeviceService::get_ota_version(&pool, "3.0.0")
        .await
        .expect("启用后查询失败");
    assert!(
        result_enabled.is_some(),
        "status=1 时应返回 Some（启用状态）"
    );
    let row = result_enabled.unwrap();
    let status = row.get("status").and_then(|v| v.as_i64()).unwrap_or(-1);
    assert_eq!(status, 1, "启用后 status 应为 1");

    cleanup(&pool).await;
    pool.close_all().await;
}
