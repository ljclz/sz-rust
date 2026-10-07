// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P1-4 MQTT 消息分发端到端测试基线（spec 5.4）
//!
//! 覆盖 MQTT→服务→数据库全链路，使用真实 MySQL 9.6。
//! 禁止 mock 消息分发逻辑（spec 5.4.6）。
//! 运行方式：
//! ```
//! cargo test --package sz-rust-sz300 --test e2e_mqtt_test -- --ignored
//! ```

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::LazyLock;

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
        #[cfg(feature = "v19-plugin-flow")]
        plugin_manager: std::sync::Arc::new(
            sz_rust_sz300::services::plugin_manager::PluginManager::new(
                std::sync::Arc::new(
                    sz_rust_sz300::services::plugin_manager::InMemoryMarketplace::new(),
                ),
                "test_pub_key".to_string(),
            ),
        ),
    }
}

async fn setup_tables(pool: &Pool) {
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

async fn insert_device(pool: &Pool, device_sn: &str, merchant_id: i64) {
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
}

async fn get_device_fields(pool: &Pool, device_sn: &str) -> (i64, i64, String) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT status, signal_strength, fw_version FROM device WHERE device_sn = ?",
            &[Value::String(device_sn.to_string())],
        )
        .await
        .expect("查询失败");
    assert_eq!(rows.len(), 1, "设备应存在: {}", device_sn);
    let status = rows[0].get("status").and_then(|v| v.as_i64()).unwrap_or(-1);
    let signal = rows[0]
        .get("signal_strength")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    let fw = rows[0]
        .get("fw_version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (status, signal, fw)
}

async fn count_rows(pool: &Pool, table: &str) -> i64 {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let sql = format!("SELECT COUNT(*) as cnt FROM {}", table);
    let rows = conn.query(&sql).await.expect("查询失败");
    rows[0].get("cnt").and_then(|v| v.as_i64()).unwrap_or(0)
}

// ============================================================================
// spec 5.4.1: MQTT 消息分发端到端
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_mqtt_dispatch_status_updates_device() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_tables(&pool).await;
    insert_device(&pool, "SN_MQTT_E2E_001", 100).await;
    let state = make_state(pool.clone()).await;

    let topic = "/sz/device/SN_MQTT_E2E_001/status";
    let payload = serde_json::json!({
        "status": 2,
        "signal_strength": 75,
        "fw_version": "2.1.0"
    });
    services::mqtt_listener::MqttDispatcher::dispatch(
        &state,
        topic,
        payload.to_string().as_bytes(),
    )
    .await;

    let (status, signal, fw) = get_device_fields(&pool, "SN_MQTT_E2E_001").await;
    assert_eq!(status, 2, "dispatch 应更新 status 为 2");
    assert_eq!(signal, 75, "dispatch 应更新 signal_strength 为 75");
    assert_eq!(fw, "2.1.0", "dispatch 应更新 fw_version 为 2.1.0");

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.4.2: MQTT 状态同步端到端
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_mqtt_status_sync_updates_all_fields() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_tables(&pool).await;
    insert_device(&pool, "SN_MQTT_SYNC", 200).await;
    let state = make_state(pool.clone()).await;

    // 初始状态
    let (init_status, init_signal, init_fw) = get_device_fields(&pool, "SN_MQTT_SYNC").await;
    assert_eq!(init_status, 1, "初始 status 应为 1");
    assert_eq!(init_signal, 0, "初始 signal_strength 应为 0");
    assert_eq!(init_fw, "", "初始 fw_version 应为空");

    // 通过 dispatch 发送状态同步消息
    let payload = serde_json::json!({
        "status": 3,
        "signal_strength": 95,
        "fw_version": "3.0.0"
    });
    services::mqtt_listener::MqttDispatcher::dispatch(
        &state,
        "/sz/device/SN_MQTT_SYNC/status",
        payload.to_string().as_bytes(),
    )
    .await;

    // 验证所有字段已同步
    let (status, signal, fw) = get_device_fields(&pool, "SN_MQTT_SYNC").await;
    assert_eq!(status, 3, "同步后 status 应为 3");
    assert_eq!(signal, 95, "同步后 signal_strength 应为 95");
    assert_eq!(fw, "3.0.0", "同步后 fw_version 应为 3.0.0");

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.4.3: MQTT 未知主题处理
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_mqtt_unknown_topic_does_not_crash() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_tables(&pool).await;
    insert_device(&pool, "SN_MQTT_UNKNOWN", 100).await;
    let state = make_state(pool.clone()).await;

    // 未知 action — 应记录告警但不崩溃
    let payload = serde_json::json!({"data": "test"});
    services::mqtt_listener::MqttDispatcher::dispatch(
        &state,
        "/sz/device/SN_MQTT_UNKNOWN/unknown_action",
        payload.to_string().as_bytes(),
    )
    .await;

    // 设备状态不应改变
    let (status, _, _) = get_device_fields(&pool, "SN_MQTT_UNKNOWN").await;
    assert_eq!(status, 1, "未知 action 不应改变设备状态");

    // 过短 topic — 应静默返回不崩溃
    services::mqtt_listener::MqttDispatcher::dispatch(
        &state,
        "/short/topic",
        payload.to_string().as_bytes(),
    )
    .await;

    // 无效 JSON — 应静默返回不崩溃
    services::mqtt_listener::MqttDispatcher::dispatch(
        &state,
        "/sz/device/SN_MQTT_UNKNOWN/status",
        b"not valid json",
    )
    .await;

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.4.4: MQTT 设备不存在处理
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_mqtt_nonexistent_device_does_not_crash() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_tables(&pool).await;
    let state = make_state(pool.clone()).await;

    // 对不存在的设备发送状态消息 — UPDATE 影响 0 行但不报错
    let payload = serde_json::json!({
        "status": 5,
        "signal_strength": 50,
        "fw_version": "9.9.9"
    });
    services::mqtt_listener::MqttDispatcher::dispatch(
        &state,
        "/sz/device/NONEXISTENT_SN/status",
        payload.to_string().as_bytes(),
    )
    .await;

    // 验证不崩溃：查询 device 表应为空
    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query("SELECT COUNT(*) as cnt FROM device")
        .await
        .expect("查询失败");
    let cnt = rows[0].get("cnt").and_then(|v| v.as_i64()).unwrap_or(-1);
    assert_eq!(cnt, 0, "不存在的设备不应创建记录");

    // 对不存在的设备发送日志消息 — 应不崩溃
    let log_payload = serde_json::json!({"level": "error", "message": "test"});
    services::mqtt_listener::MqttDispatcher::dispatch(
        &state,
        "/sz/device/NONEXISTENT_SN/log",
        log_payload.to_string().as_bytes(),
    )
    .await;

    // operate_log 应有记录（handle_device_log 用 device_id=0 插入）
    let log_cnt = count_rows(&pool, "operate_log").await;
    assert!(log_cnt >= 1, "日志消息应插入 operate_log（即使设备不存在）");

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.4.5: MQTT 消息顺序保证
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_mqtt_message_ordering_sequential() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_tables(&pool).await;
    insert_device(&pool, "SN_MQTT_ORDER", 100).await;
    let state = make_state(pool.clone()).await;

    // 依次发送 3 条状态消息，每条 status 递增
    for i in 1..=3i64 {
        let payload = serde_json::json!({
            "status": i,
            "signal_strength": i * 10,
            "fw_version": format!("{}.0.0", i)
        });
        services::mqtt_listener::MqttDispatcher::dispatch(
            &state,
            "/sz/device/SN_MQTT_ORDER/status",
            payload.to_string().as_bytes(),
        )
        .await;
    }

    // 最终状态应为最后一条消息的值（顺序处理）
    let (status, signal, fw) = get_device_fields(&pool, "SN_MQTT_ORDER").await;
    assert_eq!(status, 3, "最终 status 应为最后一条消息的值 3");
    assert_eq!(signal, 30, "最终 signal_strength 应为 30");
    assert_eq!(fw, "3.0.0", "最终 fw_version 应为 3.0.0");

    cleanup(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// spec 5.4.1 补充: MQTT order/log 分发端到端
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_mqtt_dispatch_order_inserts_record() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_tables(&pool).await;
    insert_device(&pool, "SN_MQTT_ORD_DISP", 100).await;
    let state = make_state(pool.clone()).await;

    let payload = serde_json::json!({
        "offline_seq": "ORD_SEQ_001",
        "total_fen": 6800,
        "items": [{"sku": "X001", "qty": 1}]
    });
    services::mqtt_listener::MqttDispatcher::dispatch(
        &state,
        "/sz/device/SN_MQTT_ORD_DISP/order",
        payload.to_string().as_bytes(),
    )
    .await;

    let cnt = count_rows(&pool, "`order`").await;
    assert!(cnt >= 1, "dispatch order 应在 order 表插入记录");

    cleanup(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn e2e_mqtt_dispatch_log_inserts_record() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_tables(&pool).await;
    insert_device(&pool, "SN_MQTT_LOG_DISP", 100).await;
    let state = make_state(pool.clone()).await;

    let payload = serde_json::json!({
        "level": "info",
        "message": "dispatch log test"
    });
    services::mqtt_listener::MqttDispatcher::dispatch(
        &state,
        "/sz/device/SN_MQTT_LOG_DISP/log",
        payload.to_string().as_bytes(),
    )
    .await;

    let cnt = count_rows(&pool, "operate_log").await;
    assert!(cnt >= 1, "dispatch log 应在 operate_log 表插入记录");

    cleanup(&pool).await;
    pool.close_all().await;
}
