// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! SSE 端点集成测试（v1.8.0 P2-2.3）
//!
//! 验证 `v18-sse` feature gate 下：
//! 1. SseService：创建/发布/订阅/历史缓存/事件 ID 递增
//! 2. Last-Event-ID 恢复：重连后补发缺失事件
//! 3. 背压配置：BackpressureConfig 默认值与自定义
//! 4. 业务广播：订单状态/设备状态
//! 5. SSE 端点：HTTP 响应头 + 事件流格式

#![cfg(feature = "v18-sse")]

use std::time::Duration;

use sz_rust_sz300::services::sse_service::{BackpressureConfig, SseEvent, SseService};

// ============================================================================
// SseService 基础测试
// ============================================================================

#[tokio::test]
async fn test_sse_service_creation() {
    let svc = SseService::new(128, 500);
    assert_eq!(svc.capacity(), 128);
    assert_eq!(svc.history_count().await, 0);
}

#[tokio::test]
async fn test_sse_with_defaults() {
    let svc = SseService::with_defaults();
    assert_eq!(svc.capacity(), 256);
    assert_eq!(svc.history_count().await, 0);
}

#[tokio::test]
async fn test_publish_increments_event_id() {
    let svc = SseService::with_defaults();
    let id1 = svc.publish("test", "data1").await;
    let id2 = svc.publish("test", "data2").await;
    let id3 = svc.publish("test", "data3").await;
    assert_eq!(id1, 1);
    assert_eq!(id2, 2);
    assert_eq!(id3, 3);
    assert_eq!(svc.history_count().await, 3);
}

#[tokio::test]
async fn test_publish_stores_in_history() {
    let svc = SseService::with_defaults();
    svc.publish("order_status", r#"{"order_no":"A001"}"#).await;
    svc.publish("device_status", r#"{"device_sn":"D001"}"#)
        .await;
    assert_eq!(svc.history_count().await, 2);
}

// ============================================================================
// Last-Event-ID 恢复测试
// ============================================================================

#[tokio::test]
async fn test_subscribe_without_last_event_id() {
    let svc = SseService::with_defaults();
    svc.publish("test", "data1").await;
    svc.publish("test", "data2").await;

    let (_rx, missed) = svc.subscribe(None).await;
    assert_eq!(missed.len(), 0, "无 Last-Event-ID 时不应返回历史事件");
}

#[tokio::test]
async fn test_subscribe_with_last_event_id_returns_missed() {
    let svc = SseService::with_defaults();
    let id1 = svc.publish("test", "data1").await;
    let id2 = svc.publish("test", "data2").await;
    let id3 = svc.publish("test", "data3").await;

    let (_rx, missed) = svc.subscribe(Some(id1)).await;
    assert_eq!(missed.len(), 2, "应返回 id > {id1} 的 2 个事件");
    assert_eq!(missed[0].id, id2);
    assert_eq!(missed[1].id, id3);
}

#[tokio::test]
async fn test_subscribe_with_last_event_id_beyond_history() {
    let svc = SseService::with_defaults();
    svc.publish("test", "data1").await;
    svc.publish("test", "data2").await;

    let (_rx, missed) = svc.subscribe(Some(999)).await;
    assert_eq!(missed.len(), 0, "Last-Event-ID 超出历史范围时应返回空");
}

// ============================================================================
// 历史缓存溢出测试
// ============================================================================

#[tokio::test]
async fn test_history_overflow_drops_oldest() {
    let svc = SseService::new(256, 3);
    let id1 = svc.publish("test", "d1").await;
    let id2 = svc.publish("test", "d2").await;
    let _id3 = svc.publish("test", "d3").await;
    let id4 = svc.publish("test", "d4").await;

    assert_eq!(svc.history_count().await, 3, "历史缓存应限制为 3");

    let (_rx, missed) = svc.subscribe(Some(id1)).await;
    assert_eq!(missed.len(), 3, "id1 已被淘汰，应返回全部 3 个历史事件");
    assert_eq!(missed[0].id, id2);
    assert_eq!(missed[2].id, id4);
}

// ============================================================================
// 业务广播测试
// ============================================================================

#[tokio::test]
async fn test_broadcast_order_status() {
    let svc = SseService::with_defaults();
    let id = svc.broadcast_order_status("ORD-2026-001", 2).await;
    assert!(id > 0);

    let (_rx, missed) = svc.subscribe(Some(0)).await;
    assert_eq!(missed.len(), 1);
    assert_eq!(missed[0].event_type, "order_status");
    assert!(missed[0].data.contains("ORD-2026-001"));
    assert!(missed[0].data.contains("\"status\":2"));
}

#[tokio::test]
async fn test_broadcast_device_status() {
    let svc = SseService::with_defaults();
    let id = svc.broadcast_device_status("SN-DEVICE-001", 1).await;
    assert!(id > 0);

    let (_rx, missed) = svc.subscribe(Some(0)).await;
    assert_eq!(missed.len(), 1);
    assert_eq!(missed[0].event_type, "device_status");
    assert!(missed[0].data.contains("SN-DEVICE-001"));
    assert!(missed[0].data.contains("\"status\":1"));
}

// ============================================================================
// 多订阅者测试
// ============================================================================

#[tokio::test]
async fn test_multiple_subscribers_receive_events() {
    let svc = SseService::with_defaults();
    let (rx1, _) = svc.subscribe(None).await;
    let (rx2, _) = svc.subscribe(None).await;

    let _id = svc.publish("test", "broadcast").await;

    let mut rx1 = rx1;
    let mut rx2 = rx2;
    let ev1 = rx1.recv().await.unwrap();
    let ev2 = rx2.recv().await.unwrap();
    assert_eq!(ev1.data, "broadcast");
    assert_eq!(ev2.data, "broadcast");
    assert_eq!(ev1.id, ev2.id);
}

// ============================================================================
// BackpressureConfig 测试
// ============================================================================

#[tokio::test]
async fn test_backpressure_config_default() {
    let config = BackpressureConfig::default();
    assert_eq!(config.max_buffer, 1000);
    assert!(config.drop_oldest);
    assert_eq!(config.check_interval, Duration::from_secs(1));
}

#[tokio::test]
async fn test_backpressure_config_custom() {
    let config = BackpressureConfig {
        max_buffer: 500,
        drop_oldest: false,
        check_interval: Duration::from_millis(500),
    };
    assert_eq!(config.max_buffer, 500);
    assert!(!config.drop_oldest);
    assert_eq!(config.check_interval, Duration::from_millis(500));
}

// ============================================================================
// SseEvent 测试
// ============================================================================

#[tokio::test]
async fn test_sse_event_creation() {
    let event = SseEvent::new(42, "order_status", r#"{"order_no":"X"}"#);
    assert_eq!(event.id, 42);
    assert_eq!(event.event_type, "order_status");
    assert!(event.data.contains("order_no"));
}

// ============================================================================
// SSE HTTP 端点测试
// ============================================================================

mod http_endpoint {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::Arc;
    use sz_rust_core::orm::Pool;
    use sz_rust_observability::MetricsRegistry;
    use sz_rust_sz300::router::create_router;
    use sz_rust_sz300::state::AppState;
    use sz_rust_sz300::{config, db};
    use tower::ServiceExt;

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

    fn make_state(pool: Pool) -> AppState {
        let metrics = Arc::new(MetricsRegistry::new());
        AppState {
            db_pool: Arc::new(pool.clone()),
            pg_pool: None,
            metrics_registry: metrics,
            #[cfg(feature = "v18-rbac")]
            rbac_engine: Arc::new(sz_rust_sz300::rbac::roles::init_rbac_engine()),
            #[cfg(feature = "v18-key-rotation")]
            key_manager: sz_rust_sz300::services::auth_service::default_key_manager_for_tests(),
            #[cfg(feature = "v18-audit-chain")]
            chain_auditor: Arc::new(
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
            ws_manager: Arc::new(sz_rust_websocket::manager::ConnectionManager::with_defaults()),
            #[cfg(feature = "v18-websocket")]
            ws_rooms: Arc::new(sz_rust_websocket::room::RoomManager::new()),
            #[cfg(feature = "v18-sse")]
            sse_service: SseService::with_defaults(),
        }
    }

    #[tokio::test]
    async fn test_sse_endpoint_returns_event_stream() {
        let pool = match ensure_mysql().await {
            Some(p) => p,
            None => {
                eprintln!("⚠️ MySQL 不可达，跳过 SSE 端点测试");
                return;
            }
        };
        let state = make_state(pool);
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("text/event-stream"),
            "content-type 应为 text/event-stream, 实际: {content_type}"
        );
    }

    #[tokio::test]
    async fn test_sse_endpoint_with_last_event_id_header() {
        let pool = match ensure_mysql().await {
            Some(p) => p,
            None => {
                eprintln!("⚠️ MySQL 不可达，跳过 SSE Last-Event-ID 测试");
                return;
            }
        };
        let state = make_state(pool);
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .header("Last-Event-ID", "5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
