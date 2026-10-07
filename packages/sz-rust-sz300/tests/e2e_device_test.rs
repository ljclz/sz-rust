// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P1-2 设备管理端到端测试基线（spec 5.2）
//!
//! 覆盖控制器→服务→模型→数据库全链路，使用真实 MySQL 9.6 连接池。
//! 禁止 mock 控制器/服务/模型任一层（spec 5.2.8）。
//! 运行方式：
//! ```
//! cargo test --package sz-rust-sz300 --test e2e_device_test -- --ignored
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

/// 建 device 表，列对齐 Device::columns()（11 列）
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

async fn setup_operate_log_table(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute("DROP TABLE IF EXISTS operate_log").await.ok();
    conn.execute(
        "CREATE TABLE operate_log (\
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
}

async fn setup_order_table(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute("DROP TABLE IF EXISTS `order`").await.ok();
    conn.execute(
        "CREATE TABLE `order` (\
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
}

async fn cleanup(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute("DROP TABLE IF EXISTS device").await.ok();
    conn.execute("DROP TABLE IF EXISTS operate_log").await.ok();
    conn.execute("DROP TABLE IF EXISTS `order`").await.ok();
}

/// 插入未绑定设备（merchant_id=0），返回 device_id
async fn insert_unbound_device(pool: &Pool, device_sn: &str) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO device (device_sn, merchant_id, status) VALUES (?, 0, 0)",
        &[Value::String(device_sn.to_string())],
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

/// 插入已绑定设备（merchant_id>0, bind_at=NOW()），返回 device_id
async fn insert_bound_device(pool: &Pool, device_sn: &str, merchant_id: i64) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute_with_params(
        "INSERT INTO device (device_sn, merchant_id, status, bind_at) VALUES (?, ?, 1, NOW())",
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

async fn read_json(response: axum::response::Response) -> serde_json::Value {
    let (_parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .expect("读取 body 失败");
    serde_json::from_slice(&bytes).expect("解析 JSON 失败")
}

fn json_request(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

// ============================================================================
// spec 5.2.1: 设备绑定端到端
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_device_bind_creates_binding_record() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_operate_log_table(&pool).await;
    insert_unbound_device(&pool, "SN_E2E_BIND").await;
    let state = make_state(pool.clone()).await;

    let req = json_request(
        "POST",
        "/api/v1/device/bind",
        serde_json::json!({"device_sn": "SN_E2E_BIND", "merchant_id": 200}),
    );
    let response = sz_rust_sz300::controllers::device::bind(axum::extract::State(state), req).await;
    let json = read_json(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    assert_eq!(code, 1, "绑定成功应返回 code=1, 实际: {:?}", json);

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT merchant_id, bind_at IS NOT NULL as has_bind_at FROM device WHERE device_sn = ?",
            &[Value::String("SN_E2E_BIND".to_string())],
        )
        .await
        .expect("查询失败");
    assert_eq!(rows.len(), 1);
    let merchant_id = rows[0]
        .get("merchant_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let has_bind_at = rows[0]
        .get("has_bind_at")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert_eq!(merchant_id, 200, "绑定后 merchant_id 应为 200");
    assert_eq!(has_bind_at, 1, "绑定后 bind_at 不应为 NULL");

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.2.2: 设备解绑端到端
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_device_unbind_resets_fields() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    let device_id = insert_bound_device(&pool, "SN_E2E_UNBIND", 300).await;
    let state = make_state(pool.clone()).await;

    let req = json_request(
        "POST",
        "/api/v1/device/unbind",
        serde_json::json!({"device_id": device_id}),
    );
    let response =
        sz_rust_sz300::controllers::device::unbind(axum::extract::State(state), req).await;
    let json = read_json(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    assert_eq!(code, 1, "解绑成功应返回 code=1, 实际: {:?}", json);

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT merchant_id, status, bind_at FROM device WHERE device_id = ?",
            &[Value::I64(device_id)],
        )
        .await
        .expect("查询失败");
    assert_eq!(rows.len(), 1);
    let merchant_id = rows[0]
        .get("merchant_id")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let status = rows[0].get("status").and_then(|v| v.as_i64()).unwrap_or(-1);
    assert_eq!(merchant_id, 0, "解绑后 merchant_id 应为 0");
    assert_eq!(status, 0, "解绑后 status 应为 0");
    assert!(rows[0]["bind_at"].is_null(), "解绑后 bind_at 应为 NULL");

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.2.3: 设备状态上报端到端
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_device_status_report_updates_fields() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    let device_id = insert_bound_device(&pool, "SN_E2E_STATUS", 100).await;
    let state = make_state(pool.clone()).await;

    let req = json_request(
        "POST",
        "/api/v1/device/status_report",
        serde_json::json!({
            "device_id": device_id,
            "status": 2,
            "signal_strength": 88,
            "fw_version": "3.1.0"
        }),
    );
    let response =
        sz_rust_sz300::controllers::device::status_report(axum::extract::State(state), req).await;
    let json = read_json(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    assert_eq!(code, 1, "状态上报应返回 code=1, 实际: {:?}", json);

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT status, signal_strength, fw_version FROM device WHERE device_id = ?",
            &[Value::I64(device_id)],
        )
        .await
        .expect("查询失败");
    assert_eq!(rows.len(), 1);
    let status = rows[0].get("status").and_then(|v| v.as_i64()).unwrap_or(-1);
    let signal = rows[0]
        .get("signal_strength")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let fw = rows[0]
        .get("fw_version")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(status, 2, "status 应为 2");
    assert_eq!(signal, 88, "signal_strength 应为 88");
    assert_eq!(fw, "3.1.0", "fw_version 应为 3.1.0");

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.2.4: 设备日志端到端（MQTT handler → operate_log）
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_device_log_inserts_to_operate_log() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_operate_log_table(&pool).await;
    insert_bound_device(&pool, "SN_E2E_LOG", 100).await;
    let state = make_state(pool.clone()).await;

    let payload = serde_json::json!({
        "level": "warn",
        "message": "device battery low"
    });
    let result = services::mqtt_service::MqttMessageHandler::handle_device_log(
        &state,
        "SN_E2E_LOG",
        &payload,
    )
    .await;
    assert!(result.is_ok(), "handle_device_log 应成功: {:?}", result);

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query("SELECT COUNT(*) as cnt FROM operate_log")
        .await
        .expect("查询 operate_log 失败");
    let cnt = rows[0].get("cnt").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(cnt >= 1, "operate_log 应至少有 1 条记录");

    let log_rows = conn
        .query("SELECT operator, action, detail FROM operate_log ORDER BY id DESC LIMIT 1")
        .await
        .expect("查询日志详情失败");
    assert_eq!(log_rows.len(), 1);
    let operator = log_rows[0]
        .get("operator")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        operator.contains("SN_E2E_LOG"),
        "operator 应包含设备 SN, 实际: {}",
        operator
    );

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.2.5: 设备订单端到端（MQTT handler → order）
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_device_order_inserts_to_order_table() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_order_table(&pool).await;
    insert_bound_device(&pool, "SN_E2E_ORDER", 100).await;
    let state = make_state(pool.clone()).await;

    let payload = serde_json::json!({
        "offline_seq": "E2E_SEQ_001",
        "total_fen": 9900,
        "items": [{"sku": "P001", "qty": 3}, {"sku": "P002", "qty": 1}]
    });
    let result = services::mqtt_service::MqttMessageHandler::handle_device_order(
        &state,
        "SN_E2E_ORDER",
        &payload,
    )
    .await;
    assert!(result.is_ok(), "handle_device_order 应成功: {:?}", result);

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query("SELECT order_no, merchant_id, device_id, total_fen, offline_seq, item_count, status FROM `order` ORDER BY order_id DESC LIMIT 1")
        .await
        .expect("查询 order 失败");
    assert_eq!(rows.len(), 1, "order 表应有 1 条记录");
    let total_fen = rows[0]
        .get("total_fen")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let item_count = rows[0]
        .get("item_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let offline_seq = rows[0]
        .get("offline_seq")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(total_fen, 9900, "total_fen 应为 9900");
    assert_eq!(item_count, 2, "item_count 应为 2（2 个商品）");
    assert_eq!(offline_seq, "E2E_SEQ_001", "offline_seq 应为 E2E_SEQ_001");

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.2.6: 设备不存在处理
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_device_bind_nonexistent_returns_error() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_operate_log_table(&pool).await;
    let state = make_state(pool.clone()).await;

    let req = json_request(
        "POST",
        "/api/v1/device/bind",
        serde_json::json!({"device_sn": "NONEXISTENT_SN", "merchant_id": 100}),
    );
    let response = sz_rust_sz300::controllers::device::bind(axum::extract::State(state), req).await;
    let json = read_json(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(1);
    assert_eq!(code, 0, "绑定不存在设备应返回 code=0");
    let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("不存在"),
        "错误消息应包含'不存在', 实际: {}",
        msg
    );

    cleanup(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_device_trigger_ota_nonexistent_device_returns_error() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    let state = make_state(pool.clone()).await;

    let req = json_request(
        "POST",
        "/api/v1/device/ota",
        serde_json::json!({"device_id": 999999, "ota_version": "1.0.0"}),
    );
    let response =
        sz_rust_sz300::controllers::device::trigger_ota(axum::extract::State(state), req).await;
    let json = read_json(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(1);
    assert_eq!(code, 0, "操作不存在设备应返回 code=0");
    let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("不存在"),
        "错误消息应包含'不存在', 实际: {}",
        msg
    );

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.2.7: 测试隔离 — 每个测试独立建表/清理，互不影响
// ============================================================================
// 已通过 TEST_LOCK 全局序列化 + 每测试 setup/cleanup 保证隔离
