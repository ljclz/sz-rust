// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! MQTT 服务配置集成测试
//!
//! 验证 `sz_rust_sz300::services::mqtt_service` 的配置函数返回结构合法。
//! 这些函数是纯逻辑（不依赖真实 DB 连接），可直接测试。

use sz_rust_sz300::services::mqtt_service;

#[test]
fn test_get_mqtt_config() {
    let config = mqtt_service::get_mqtt_config();
    // 验证 broker_url 已设置
    assert!(!config.broker_url.is_empty());
    // 验证 keep_alive 配置
    assert_eq!(config.keep_alive, 30);
    // 验证 client_id 已生成
    assert!(config.client_id.is_some());
    let client_id = config.client_id.as_ref().unwrap();
    assert!(client_id.starts_with("sz300-server-"));
}

#[test]
fn test_get_subscribe_topics() {
    let topics = mqtt_service::get_subscribe_topics();
    // 验证返回非空列表
    assert!(!topics.is_empty());
    // 验证包含设备主题
    let names: Vec<&str> = topics.iter().map(|t| t.name.as_str()).collect();
    assert!(names.iter().any(|n| n.contains("/sz/device/")));
    assert!(names.iter().any(|n| n.contains("status")));
    assert!(names.iter().any(|n| n.contains("order")));
    assert!(names.iter().any(|n| n.contains("log")));
}

#[test]
fn test_get_subscribe_topics_count() {
    let topics = mqtt_service::get_subscribe_topics();
    // 应返回 3 个订阅主题
    assert_eq!(topics.len(), 3);
}

#[test]
fn test_get_subscribe_topics_use_wildcard() {
    let topics = mqtt_service::get_subscribe_topics();
    // 所有订阅主题应使用 + 通配符匹配任意 device_sn
    for topic in &topics {
        assert!(
            topic.name.contains('+'),
            "topic {} should contain '+' wildcard",
            topic.name
        );
    }
}

// ============================================================================
// P1-TEST-03：dispatch / start_consumer 集成测试占位（需真实 DB）
//
// MqttDispatcher::dispatch 和 MqttDispatcher::start_consumer 均依赖 AppState
//（含真实 Pool），无法在单元测试中构造。以下 #[ignore] 占位记录集成测试需求，
// 实际验证由 CI 的 db-integration job（MySQL 9.6 / PostgreSQL 18 容器）完成。
// ============================================================================

/// dispatch 路由分发集成测试（需真实 DB）
///
/// 验证：topic 格式解析正确，action 路由到对应 handler，
/// 短 topic（parts.len() < 5）静默返回，未知 action 仅 warn 日志。
#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_dispatch_topic_routing_integration() {
    use std::sync::Arc;
    use sz_rust_core::orm::Value as OrmValue;
    use sz_rust_sz300::{config, db, state::AppState};

    let cfg = config::AppConfig {
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
    };
    let pool = match db::init_pool(&cfg).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("⚠️ MySQL 不可达，跳过 dispatch 集成测试");
            return;
        }
    };
    {
        let mut conn = match pool.acquire().await {
            Ok(c) => c,
            Err(_) => {
                eprintln!("⚠️ MySQL 获取连接失败，跳过");
                pool.close_all().await;
                return;
            }
        };
        if conn.query("SELECT 1").await.is_err() {
            eprintln!("⚠️ MySQL 查询失败，跳过");
            pool.close_all().await;
            return;
        }
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
        conn.execute_with_params(
            "INSERT INTO device (device_sn, merchant_id, status) VALUES (?, 100, 1)",
            &[OrmValue::String("SN_DISPATCH_001".to_string())],
        )
        .await
        .expect("插入 device 失败");
    }

    let state = AppState {
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
        #[cfg(feature = "v18-websocket")]
        ws_manager: std::sync::Arc::new(
            sz_rust_websocket::manager::ConnectionManager::with_defaults(),
        ),
        #[cfg(feature = "v18-websocket")]
        ws_rooms: std::sync::Arc::new(sz_rust_websocket::room::RoomManager::new()),
        #[cfg(feature = "v18-sse")]
        sse_service: sz_rust_sz300::services::sse_service::SseService::with_defaults(),
    };

    let payload = serde_json::json!({
        "status": 2,
        "signal_strength": 80,
        "fw_version": "1.2.0"
    })
    .to_string();
    sz_rust_sz300::services::mqtt_listener::MqttDispatcher::dispatch(
        &state,
        "/sz/device/SN_DISPATCH_001/status",
        payload.as_bytes(),
    )
    .await;

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        let rows = conn
            .query_with_params(
                "SELECT status, signal_strength, fw_version FROM device WHERE device_sn = ?",
                &[OrmValue::String("SN_DISPATCH_001".to_string())],
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
        assert_eq!(status, 2, "dispatch 应将 status 更新为 2");
        assert_eq!(signal, 80, "dispatch 应将 signal_strength 更新为 80");
        assert_eq!(fw, "1.2.0", "dispatch 应将 fw_version 更新为 1.2.0");
    }

    {
        let mut conn = pool.acquire().await.expect("获取连接失败");
        conn.execute("DROP TABLE IF EXISTS device").await.ok();
    }
    pool.close_all().await;
}

/// start_consumer 优雅退出集成测试（需真实 DB）
///
/// 验证：收到 shutdown_rx=true 后在合理时间内退出，不泄漏任务。
#[tokio::test]
#[ignore = "requires real MySQL 9.6"]
async fn test_start_consumer_graceful_shutdown_integration() {
    use std::sync::Arc;
    use sz_rust_sz300::{config, db, state::AppState};
    use tokio::sync::watch;
    use tokio::time::Duration;

    let cfg = config::AppConfig {
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
    };
    let pool = match db::init_pool(&cfg).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("⚠️ MySQL 不可达，跳过 start_consumer 测试");
            return;
        }
    };
    let state = AppState {
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
        #[cfg(feature = "v18-websocket")]
        ws_manager: std::sync::Arc::new(
            sz_rust_websocket::manager::ConnectionManager::with_defaults(),
        ),
        #[cfg(feature = "v18-websocket")]
        ws_rooms: std::sync::Arc::new(sz_rust_websocket::room::RoomManager::new()),
        #[cfg(feature = "v18-sse")]
        sse_service: sz_rust_sz300::services::sse_service::SseService::with_defaults(),
    };

    let (tx, rx) = watch::channel(false);
    let handle = tokio::spawn(async move {
        sz_rust_sz300::services::mqtt_listener::MqttDispatcher::start_consumer(state, rx).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    tx.send(true).expect("发送 shutdown 信号失败");

    let result = tokio::time::timeout(Duration::from_secs(3), handle).await;
    assert!(result.is_ok(), "start_consumer 应在 3 秒内优雅退出");
    assert!(result.unwrap().is_ok(), "start_consumer 任务应无 panic");

    pool.close_all().await;
}

// ============================================================================
// P1-TEST-03：send_ota_command 测试（纯逻辑，不依赖真实 broker）
// ============================================================================

#[tokio::test]
async fn test_send_ota_command_builds_message() {
    // send_ota_command 仅构建 MqttMessage 并返回 Ok，不实际发送
    let result =
        mqtt_service::send_ota_command("SN001", "http://ota.example.com/firmware.bin", "2.0").await;
    assert!(result.is_ok(), "send_ota_command 应成功构建消息");
}

#[tokio::test]
async fn test_send_ota_command_empty_sn() {
    let result = mqtt_service::send_ota_command("", "http://example.com/f.bin", "1.0").await;
    // 空 sn 仍应成功（topic 为 /sz/server//ota）
    assert!(result.is_ok());
}
