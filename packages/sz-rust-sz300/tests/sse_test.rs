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
    use futures::StreamExt;
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
                // 本机可用 SZ_TEST_DB_PORT 覆盖（默认 3306，CI 不受影响）
                port: std::env::var("SZ_TEST_DB_PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(3306),
                username: "root".to_string(),
                password: std::env::var("SZ_TEST_DB_PASSWORD").unwrap_or_default(),
                database: "sz_orm_test".to_string(),
            },
        }
    }

    // 2026-10-28 安全修复后契约：/events 订阅前强制 JWT 校验（router.rs sse_handler）。
    // 测试令牌铸造模式与 rbac_test.rs 一致（init_auth_test_only + JwtEncoder）。
    const TEST_SECRET: &str = "test-sse-secret-2026";

    fn init_test_auth() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            sz_rust_sz300::services::auth_service::init_auth_test_only(TEST_SECRET);
        });
    }

    fn issue_token(username: &str, user_id: i64) -> String {
        init_test_auth();
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        let claims = sz_rust_core::orm::jwt::JwtClaims::new(username, exp)
            .with_issuer("sz300-test")
            .with_user_id(user_id);
        sz_rust_core::orm::jwt::JwtEncoder::new(TEST_SECRET)
            .encode(&claims)
            .unwrap()
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
            query_cache: std::sync::Arc::new(
                sz_rust_sz300::query_cache::QueryCache::with_defaults(),
            ),
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
            #[cfg(feature = "v18-audit-chain")]
            audit_sink: {
                let (sink, rx) = sz_rust_sz300::middleware::audit_chain::AuditSink::new(None);
                sz_rust_sz300::middleware::audit_chain::spawn_audit_drain(rx);
                sink
            },
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
            metrics_token: None,
            metrics_public: true,
        }
    }

    /// 在总限时内读取 SSE 响应的前 `min_events` 个事件帧（以空行 `\n\n` 分隔）
    async fn read_sse_frames(body: Body, min_events: usize, total: Duration) -> String {
        let mut buf = String::new();
        let deadline = tokio::time::Instant::now() + total;
        let mut stream = body.into_data_stream();
        while buf.matches("\n\n").count() < min_events {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.next()).await {
                Ok(Some(Ok(chunk))) => buf.push_str(&String::from_utf8_lossy(&chunk)),
                _ => break,
            }
        }
        buf
    }

    async fn content_type_of(response: &axum::response::Response) -> String {
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    /// 2026-10-28 安全契约：未携带令牌不得订阅事件流（旧断言期望 200 是安全修复前的失真）
    #[tokio::test]
    async fn test_sse_endpoint_unauthorized_without_token() {
        let pool = match ensure_mysql().await {
            Some(p) => p,
            None => {
                eprintln!("⚠️ MySQL 不可达，跳过 SSE 端点鉴权测试");
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

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// 伪造/畸形令牌同样 401（fail-closed）
    #[tokio::test]
    async fn test_sse_endpoint_rejects_invalid_token() {
        let pool = match ensure_mysql().await {
            Some(p) => p,
            None => {
                eprintln!("⚠️ MySQL 不可达，跳过 SSE 畸形令牌测试");
                return;
            }
        };
        let state = make_state(pool);
        let app = create_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .header("Authorization", "Bearer not-a-real-jwt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// 有效令牌：200 + text/event-stream，且流打开后发布的事件经 live 路径推送
    #[tokio::test]
    async fn test_sse_endpoint_live_push_with_valid_token() {
        let pool = match ensure_mysql().await {
            Some(p) => p,
            None => {
                eprintln!("⚠️ MySQL 不可达，跳过 SSE live 推送测试");
                return;
            }
        };
        let state = make_state(pool);
        let sse = state.sse_service.clone();
        let app = create_router(state);
        let token = issue_token("sse-live", 1);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Accept", "text/event-stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = content_type_of(&response).await;
        assert!(
            content_type.contains("text/event-stream"),
            "content-type 应为 text/event-stream, 实际: {content_type}"
        );

        // 流已打开，再发布事件 → live 推送路径
        let _ = sse
            .publish("order_status", r#"{"order_no":"LIVE-1"}"#)
            .await;
        let body = read_sse_frames(response.into_body(), 1, Duration::from_secs(10)).await;
        assert!(
            body.contains("event: order_status"),
            "live 事件未推送到流，实际: {body}"
        );
        assert!(
            body.contains(r#"data:{"order_no":"LIVE-1"}"#)
                || body.contains(r#"data: {"order_no":"LIVE-1"}"#),
            "data 行缺失或内容不符, 实际: {body}"
        );
        assert!(
            body.contains("id: "),
            "SSE 事件应带自增 id 行, 实际: {body}"
        );
    }

    /// Last-Event-ID 断线重连：补发 id 大于游标的历史事件，且不重复游标本身
    #[tokio::test]
    async fn test_sse_endpoint_last_event_id_replay() {
        let pool = match ensure_mysql().await {
            Some(p) => p,
            None => {
                eprintln!("⚠️ MySQL 不可达，跳过 SSE Last-Event-ID 重放测试");
                return;
            }
        };
        let state = make_state(pool);
        let sse = state.sse_service.clone();
        let id1 = sse.publish("replay", "e1").await;
        let id2 = sse.publish("replay", "e2").await;
        let id3 = sse.publish("replay", "e3").await;
        let app = create_router(state);
        let token = issue_token("sse-replay", 2);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/events")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Last-Event-ID", id1.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_sse_frames(response.into_body(), 2, Duration::from_secs(10)).await;
        assert!(
            body.contains(&format!("id: {id2}")) && body.contains(&format!("id: {id3}")),
            "应补发 id {id2}/{id3} 两个历史事件, 实际: {body}"
        );
        assert!(
            !body.contains(&format!("id: {id1}")),
            "游标事件本身不应重放, 实际: {body}"
        );
    }
}
