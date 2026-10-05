// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! P1-TEST-03 服务层 & 控制器层覆盖集成测试
//!
//! 使用真实 MySQL 9.6 连接池验证服务层和控制器层。
//! 运行方式：
//! ```
//! cargo test --package sz-rust-sz300 --test service_coverage_test -- --ignored
//! ```

#![forbid(unsafe_code)]

use axum::response::IntoResponse;
use std::sync::Arc;
use std::sync::LazyLock;
use sz_rust_core::orm::{Pool, Value};
use sz_rust_sz300::state::AppState;
use sz_rust_sz300::{config, db, services};
use tokio::sync::Mutex;

/// 全局序列化锁：所有 ignored 集成测试共享同一真实 MySQL，
/// 并行 DROP/CREATE 同名表会冲突，故串行执行。
static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

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

async fn ensure_mysql() -> Option<Pool> {
    let cfg = mysql_test_config();
    match db::init_pool(&cfg).await {
        Ok(pool) => match pool.acquire().await {
            Ok(mut conn) => match conn.query("SELECT 1").await {
                Ok(_) => Some(pool),
                Err(_) => {
                    eprintln!("⚠️ MySQL 查询失败，跳过测试");
                    pool.close_all().await;
                    None
                }
            },
            Err(_) => {
                eprintln!("⚠️ MySQL 不可达，跳过测试");
                pool.close_all().await;
                None
            }
        },
        Err(_) => {
            eprintln!("⚠️ MySQL 连接池初始化失败，跳过测试");
            None
        }
    }
}

async fn make_app_state(pool: Pool) -> AppState {
    AppState {
        db_pool: Arc::new(pool),
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
         device_sn VARCHAR(64) NOT NULL UNIQUE,\
         merchant_id BIGINT NOT NULL DEFAULT 0,\
         status INT NOT NULL DEFAULT 0,\
         signal_strength INT NOT NULL DEFAULT 0,\
         fw_version VARCHAR(32) NOT NULL DEFAULT '',\
         bind_at DATETIME NULL,\
         last_online_at DATETIME NULL\
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

async fn cleanup_tables(pool: &Pool) {
    let mut conn = pool.acquire().await.expect("获取连接失败");
    conn.execute("DROP TABLE IF EXISTS device").await.ok();
    conn.execute("DROP TABLE IF EXISTS ota_version").await.ok();
    conn.execute("DROP TABLE IF EXISTS operate_log").await.ok();
    conn.execute("DROP TABLE IF EXISTS `order`").await.ok();
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

/// 读取 axum Response body 为 serde_json::Value，用于断言 ThinkPHP 风格 JSON。
async fn read_json_body(response: axum::response::Response) -> serde_json::Value {
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .expect("读取 body 失败");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("解析 JSON 失败");
    let _ = parts;
    json
}

// ============================================================================
// health_service
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_ping_db_returns_true_when_db_available() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let result = services::health_service::ping_db(&Arc::new(pool.clone())).await;
    assert!(result, "DB 可达时 ping_db 应返回 true");
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_ping_db_returns_false_when_db_unavailable() {
    let _guard = TEST_LOCK.lock().await;
    let bad_cfg = config::AppConfig {
        server: config::ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 8300,
        },
        database: config::DatabaseConfig {
            host: "127.0.0.1".to_string(),
            port: 3306,
            username: "root".to_string(),
            password: "wrong_password".to_string(),
            database: "nonexistent_db".to_string(),
        },
    };
    let result = match db::init_pool(&bad_cfg).await {
        Ok(pool) => {
            let r = services::health_service::ping_db(&Arc::new(pool.clone())).await;
            pool.close_all().await;
            r
        }
        Err(_) => false,
    };
    assert!(!result, "DB 不可达时 ping_db 应返回 false");
}

// ============================================================================
// device_service::DeviceService
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_device_service_unbind_resets_fields() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    let device_id = insert_device(&pool, "SN_UNBIND_001", 100).await;

    services::device_service::DeviceService::unbind(&pool, device_id)
        .await
        .expect("unbind 失败");

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
    assert_eq!(merchant_id, 0, "unbind 后 merchant_id 应为 0");
    assert_eq!(status, 0, "unbind 后 status 应为 0");
    let bind_at = &rows[0]["bind_at"];
    assert!(bind_at.is_null(), "unbind 后 bind_at 应为 NULL");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_device_service_get_ota_version_returns_enabled() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_ota_table(&pool).await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute_with_params(
            "INSERT INTO ota_version (version, model, url, md5, changelog, size, force_update, status) \
             VALUES (?, 'M100', 'http://ota.example.com/v1.bin', '', '', 0, 0, 1)",
            &[Value::String("1.0.0".to_string())],
        )
        .await
        .expect("插入 ota_version 失败");
    }

    let result = services::device_service::DeviceService::get_ota_version(&pool, "1.0.0")
        .await
        .expect("get_ota_version 失败");
    assert!(result.is_some(), "status=1 的 OTA 版本应返回 Some");
    let row = result.unwrap();
    let version = row.get("version").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(version, "1.0.0");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_device_service_get_ota_version_disabled_returns_none() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_ota_table(&pool).await;
    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute_with_params(
            "INSERT INTO ota_version (version, model, url, md5, changelog, size, force_update, status) \
             VALUES (?, 'M100', 'http://ota.example.com/v2.bin', '', '', 0, 0, 0)",
            &[Value::String("2.0.0".to_string())],
        )
        .await
        .expect("插入 ota_version 失败");
    }

    let result = services::device_service::DeviceService::get_ota_version(&pool, "2.0.0")
        .await
        .expect("get_ota_version 失败");
    assert!(result.is_none(), "status=0 的 OTA 版本应返回 None");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_device_service_update_status_updates_fields() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    let device_id = insert_device(&pool, "SN_STATUS_001", 100).await;

    services::device_service::DeviceService::update_status(&pool, device_id, 2, 85, "1.5.0")
        .await
        .expect("update_status 失败");

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
    assert_eq!(status, 2, "status 应更新为 2");
    assert_eq!(signal, 85, "signal_strength 应更新为 85");
    assert_eq!(fw, "1.5.0", "fw_version 应更新为 1.5.0");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// mqtt_service::MqttMessageHandler
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_handle_device_status_updates_device_record() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    insert_device(&pool, "SN_MQTT_STATUS", 100).await;
    let state = make_app_state(pool.clone()).await;

    let payload = serde_json::json!({
        "status": 3,
        "signal_strength": 90,
        "fw_version": "2.0.0"
    });
    services::mqtt_service::MqttMessageHandler::handle_device_status(
        &state,
        "SN_MQTT_STATUS",
        &payload,
    )
    .await
    .expect("handle_device_status 失败");

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT status, signal_strength, fw_version FROM device WHERE device_sn = ?",
            &[Value::String("SN_MQTT_STATUS".to_string())],
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
    assert_eq!(status, 3, "handle_device_status 应更新 status 为 3");
    assert_eq!(
        signal, 90,
        "handle_device_status 应更新 signal_strength 为 90"
    );
    assert_eq!(
        fw, "2.0.0",
        "handle_device_status 应更新 fw_version 为 2.0.0"
    );

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_handle_device_order_inserts_order() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_order_table(&pool).await;
    insert_device(&pool, "SN_MQTT_ORDER", 100).await;
    let state = make_app_state(pool.clone()).await;

    let payload = serde_json::json!({
        "offline_seq": "SEQ_001",
        "total_fen": 5000,
        "items": [{"sku": "A001", "qty": 2}]
    });
    let result = services::mqtt_service::MqttMessageHandler::handle_device_order(
        &state,
        "SN_MQTT_ORDER",
        &payload,
    )
    .await;
    assert!(result.is_ok(), "handle_device_order 应成功: {:?}", result);

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query("SELECT COUNT(*) as cnt FROM `order`")
        .await
        .expect("查询 order 失败");
    let cnt = rows[0].get("cnt").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(
        cnt >= 1,
        "handle_device_order 应在 order 表插入至少 1 条记录"
    );

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_handle_device_log_inserts_log() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_operate_log_table(&pool).await;
    insert_device(&pool, "SN_MQTT_LOG", 100).await;
    let state = make_app_state(pool.clone()).await;

    let payload = serde_json::json!({
        "level": "info",
        "message": "test log message"
    });
    let result = services::mqtt_service::MqttMessageHandler::handle_device_log(
        &state,
        "SN_MQTT_LOG",
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
    assert!(
        cnt >= 1,
        "handle_device_log 应在 operate_log 表插入至少 1 条记录"
    );

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

// ============================================================================
// health controller
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_health_startup_returns_ok_when_metrics_ready() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let registry = sz_rust_observability::MetricsRegistry::new();
    registry.register_counter("sz300_requests_total", "Total requests");
    let state = AppState {
        db_pool: Arc::new(pool.clone()),
        pg_pool: None,
        metrics_registry: Arc::new(registry),
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
        #[cfg(feature = "v18-websocket")]
        ws_manager: std::sync::Arc::new(
            sz_rust_websocket::manager::ConnectionManager::with_defaults(),
        ),
        #[cfg(feature = "v18-websocket")]
        ws_rooms: std::sync::Arc::new(sz_rust_websocket::room::RoomManager::new()),
        #[cfg(feature = "v18-sse")]
        sse_service: sz_rust_sz300::services::sse_service::SseService::with_defaults(),
    };

    let response = sz_rust_sz300::controllers::health::startup(axum::extract::State(state)).await;
    let (parts, _body) = response.into_response().into_parts();
    assert_eq!(
        parts.status,
        axum::http::StatusCode::OK,
        "metrics 就绪时 startup 应返回 200"
    );

    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_health_metrics_returns_prometheus_format() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let registry = sz_rust_observability::MetricsRegistry::new();
    registry.register_counter("sz300_test_counter", "test counter");
    let state = AppState {
        db_pool: Arc::new(pool.clone()),
        pg_pool: None,
        metrics_registry: Arc::new(registry),
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
        #[cfg(feature = "v18-websocket")]
        ws_manager: std::sync::Arc::new(
            sz_rust_websocket::manager::ConnectionManager::with_defaults(),
        ),
        #[cfg(feature = "v18-websocket")]
        ws_rooms: std::sync::Arc::new(sz_rust_websocket::room::RoomManager::new()),
        #[cfg(feature = "v18-sse")]
        sse_service: sz_rust_sz300::services::sse_service::SseService::with_defaults(),
    };

    let response = sz_rust_sz300::controllers::health::metrics(axum::extract::State(state)).await;
    let (parts, _body) = response.into_response().into_parts();
    let content_type = parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/plain"),
        "metrics 端点 content-type 应为 text/plain, 实际: {}",
        content_type
    );

    pool.close_all().await;
}

// ============================================================================
// auth controller
// ============================================================================

// 注：空凭据登录校验（P0）不依赖 DB，已在 src/controllers/auth.rs 的
// test_credentials_non_empty_rejects_empty 单元测试中覆盖，此处不再重复。

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_auth_me_missing_token_returns_error() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_app_state(pool.clone()).await;

    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/auth/me")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = sz_rust_sz300::controllers::auth::me(axum::extract::State(state), request).await;
    // ThinkPHP 风格：业务错误 HTTP 200 + JSON {"code": 0, "msg": "..."}
    let json = read_json_body(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(1);
    assert_eq!(code, 0, "无 Authorization header 应返回 code=0 的业务错误");

    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_auth_logout_clears_csrf_cookie() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_app_state(pool.clone()).await;

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/auth/logout")
        .body(axum::body::Body::empty())
        .unwrap();
    let response =
        sz_rust_sz300::controllers::auth::logout(axum::extract::State(state), request).await;
    let (parts, _body) = response.into_parts();
    let set_cookie = parts
        .headers
        .get(axum::http::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        set_cookie.contains("Max-Age=0") || set_cookie.contains("max-age=0"),
        "logout 响应应包含 Max-Age=0 的 set-cookie, 实际: {}",
        set_cookie
    );

    pool.close_all().await;
}

// ============================================================================
// device controller
// ============================================================================

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_device_unbind_missing_id_returns_error() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    let state = make_app_state(pool.clone()).await;

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/device/unbind")
        .body(axum::body::Body::empty())
        .unwrap();
    let response =
        sz_rust_sz300::controllers::device::unbind(axum::extract::State(state), request).await;
    // ThinkPHP 风格：缺少 device_id 返回 HTTP 200 + JSON {"code": 0, "msg": "..."}
    let json = read_json_body(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(1);
    assert_eq!(code, 0, "缺少 device_id 应返回 code=0 的业务错误");

    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_device_trigger_ota_unknown_version_returns_error() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    setup_ota_table(&pool).await;
    let device_id = insert_device(&pool, "SN_OTA_TEST", 100).await;
    let state = make_app_state(pool.clone()).await;

    let body = serde_json::json!({
        "device_id": device_id,
        "ota_version": "99.99.99"
    })
    .to_string();
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/device/trigger_ota")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    let response =
        sz_rust_sz300::controllers::device::trigger_ota(axum::extract::State(state), request).await;
    // ThinkPHP 风格：OTA 版本不存在返回 HTTP 200 + JSON {"code": 0, "msg": "OTA 版本不存在或未启用"}
    let json = read_json_body(response).await;
    let code = json.get("code").and_then(|v| v.as_i64()).unwrap_or(1);
    assert_eq!(code, 0, "不存在的 OTA 版本应返回 code=0 的业务错误");
    let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        msg.contains("OTA 版本不存在") || msg.contains("不存在"),
        "错误消息应包含'不存在', 实际: {}",
        msg
    );

    cleanup_tables(&pool).await;
    pool.close_all().await;
}

#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_device_status_report_updates_device() {
    let _guard = TEST_LOCK.lock().await;
    let pool = match ensure_mysql().await {
        Some(p) => p,
        None => return,
    };
    setup_device_table(&pool).await;
    let device_id = insert_device(&pool, "SN_REPORT_001", 100).await;
    let state = make_app_state(pool.clone()).await;

    let body = serde_json::json!({
        "device_id": device_id,
        "status": 2,
        "signal_strength": 75,
        "fw_version": "1.8.0"
    })
    .to_string();
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/device/status_report")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    let _response =
        sz_rust_sz300::controllers::device::status_report(axum::extract::State(state), request)
            .await;

    let mut conn = pool.acquire().await.expect("获取连接失败");
    let rows = conn
        .query_with_params(
            "SELECT status, signal_strength, fw_version FROM device WHERE device_sn = ?",
            &[Value::String("SN_REPORT_001".to_string())],
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
    assert_eq!(status, 2, "status_report 应更新 status 为 2");
    assert_eq!(signal, 75, "status_report 应更新 signal_strength 为 75");
    assert_eq!(fw, "1.8.0", "status_report 应更新 fw_version 为 1.8.0");

    cleanup_tables(&pool).await;
    pool.close_all().await;
}
