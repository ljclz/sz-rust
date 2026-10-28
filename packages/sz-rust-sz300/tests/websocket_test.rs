// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! WebSocket 端点集成测试（v1.8.0 P2-2.2）
//!
//! 验证 `v18-websocket` feature gate 下：
//! 1. ConnectionManager：注册/注销/发送/心跳/连接数/空闲检测
//! 2. RoomManager：加入/离开/成员/广播
//! 3. WsService：设备上线/下线/心跳广播
//! 4. WebSocketConfig：配置校验

#![cfg(feature = "v18-websocket")]

use std::time::Duration;

use sz_rust_sz300::services::ws_service::WsService;
use sz_rust_websocket::manager::{ConnectionId, ConnectionManager, RoomId, WebSocketConfig};
use sz_rust_websocket::room::RoomManager;
use sz_rust_websocket::WebSocketError;
use tokio::sync::mpsc;

// ============================================================================
// ConnectionManager 测试
// ============================================================================

#[tokio::test]
async fn test_register_and_unregister() {
    let mgr = ConnectionManager::with_defaults();
    let (tx, _rx) = mpsc::channel(10);
    let conn_id = mgr.register(tx, Some("user1".into())).unwrap();
    assert_eq!(mgr.connection_count(), 1);
    assert!(mgr.contains(&conn_id));
    mgr.unregister(&conn_id);
    assert_eq!(mgr.connection_count(), 0);
    assert!(!mgr.contains(&conn_id));
}

#[tokio::test]
async fn test_connection_limit_exceeded() {
    let config = WebSocketConfig::new().with_max_connections(1);
    let mgr = ConnectionManager::new(config);
    let (tx1, _rx1) = mpsc::channel(10);
    let (tx2, _rx2) = mpsc::channel(10);
    assert!(mgr.register(tx1, None).is_ok());
    assert!(matches!(
        mgr.register(tx2, None),
        Err(WebSocketError::ConnectionLimitExceeded)
    ));
}

#[tokio::test]
async fn test_send_to_connection() {
    let mgr = ConnectionManager::with_defaults();
    let (tx, mut rx) = mpsc::channel(10);
    let conn_id = mgr.register(tx, None).unwrap();
    mgr.send_to(&conn_id, "hello").await.unwrap();
    let msg = rx.recv().await.unwrap();
    assert_eq!(msg, "hello");
}

#[tokio::test]
async fn test_send_to_nonexistent() {
    let mgr = ConnectionManager::with_defaults();
    let fake_id = ConnectionId::new();
    let result = mgr.send_to(&fake_id, "hello").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_touch_updates_active_time() {
    let mgr = ConnectionManager::with_defaults();
    let (tx, _rx) = mpsc::channel(10);
    let conn_id = mgr.register(tx, None).unwrap();
    std::thread::sleep(Duration::from_millis(10));
    mgr.touch(&conn_id).unwrap();
    let idle = mgr.idle_connections();
    assert!(idle.is_empty());
}

#[tokio::test]
async fn test_idle_connections_detection() {
    let config = WebSocketConfig {
        idle_timeout: Duration::from_millis(1),
        ..Default::default()
    };
    let mgr = ConnectionManager::new(config);
    let (tx, _rx) = mpsc::channel(10);
    let conn_id = mgr.register(tx, None).unwrap();
    std::thread::sleep(Duration::from_millis(10));
    let idle = mgr.idle_connections();
    assert!(idle.contains(&conn_id));
}

#[tokio::test]
async fn test_multiple_connections() {
    let mgr = ConnectionManager::with_defaults();
    let (tx1, _rx1) = mpsc::channel(10);
    let (tx2, _rx2) = mpsc::channel(10);
    let conn1 = mgr.register(tx1, None).unwrap();
    let conn2 = mgr.register(tx2, None).unwrap();
    assert_eq!(mgr.connection_count(), 2);
    mgr.unregister(&conn1);
    assert_eq!(mgr.connection_count(), 1);
    mgr.unregister(&conn2);
    assert_eq!(mgr.connection_count(), 0);
}

// ============================================================================
// RoomManager 测试
// ============================================================================

#[tokio::test]
async fn test_join_and_leave_room() {
    let rooms = RoomManager::new();
    let mgr = ConnectionManager::with_defaults();
    let (tx, _rx) = mpsc::channel(10);
    let conn_id = mgr.register(tx, None).unwrap();
    let room = RoomId::new("test_room");

    rooms.join_room(&conn_id, &room).unwrap();
    assert_eq!(rooms.room_size(&room), 1);

    rooms.leave_room(&conn_id, &room).unwrap();
    assert_eq!(rooms.room_size(&room), 0);
}

#[tokio::test]
async fn test_room_members() {
    let rooms = RoomManager::new();
    let mgr = ConnectionManager::with_defaults();
    let (tx1, _rx1) = mpsc::channel(10);
    let (tx2, _rx2) = mpsc::channel(10);
    let conn1 = mgr.register(tx1, None).unwrap();
    let conn2 = mgr.register(tx2, None).unwrap();
    let room = RoomId::new("admin");

    rooms.join_room(&conn1, &room).unwrap();
    rooms.join_room(&conn2, &room).unwrap();

    let members = rooms.room_members(&room);
    assert_eq!(members.len(), 2);
    assert!(members.contains(&conn1));
    assert!(members.contains(&conn2));
}

#[tokio::test]
async fn test_broadcast_room() {
    let rooms = RoomManager::new();
    let mgr = ConnectionManager::with_defaults();
    let (tx1, mut rx1) = mpsc::channel(10);
    let (tx2, mut rx2) = mpsc::channel(10);
    let conn1 = mgr.register(tx1, None).unwrap();
    let conn2 = mgr.register(tx2, None).unwrap();
    let room = RoomId::new("admin");

    rooms.join_room(&conn1, &room).unwrap();
    rooms.join_room(&conn2, &room).unwrap();

    let count = rooms.broadcast_room(&mgr, &room, "hello").await.unwrap();
    assert_eq!(count, 2, "应广播到 2 个连接");

    let msg1 = rx1.recv().await.unwrap();
    let msg2 = rx2.recv().await.unwrap();
    assert_eq!(msg1, "hello");
    assert_eq!(msg2, "hello");
}

#[tokio::test]
async fn test_broadcast_empty_room() {
    let rooms = RoomManager::new();
    let mgr = ConnectionManager::with_defaults();
    let room = RoomId::new("empty");
    let count = rooms.broadcast_room(&mgr, &room, "hello").await.unwrap();
    assert_eq!(count, 0, "空房间广播应返回 0");
}

#[tokio::test]
async fn test_all_rooms() {
    let rooms = RoomManager::new();
    let mgr = ConnectionManager::with_defaults();
    let (tx, _rx) = mpsc::channel(10);
    let conn_id = mgr.register(tx, None).unwrap();

    rooms.join_room(&conn_id, &RoomId::new("room1")).unwrap();
    rooms.join_room(&conn_id, &RoomId::new("room2")).unwrap();

    let all = rooms.all_rooms();
    assert_eq!(all.len(), 2);
    assert_eq!(rooms.room_count(), 2);
}

// ============================================================================
// WsService 测试
// ============================================================================

#[tokio::test]
async fn test_ws_service_broadcast_device_online() {
    let (mgr, rooms) = WsService::default_managers();
    let (tx, mut rx) = mpsc::channel(100);
    let conn_id = mgr.register(tx, None).unwrap();
    WsService::join_admin_room(&rooms, &conn_id).unwrap();

    let count = WsService::broadcast_device_online(&mgr, &rooms, "SN001").await;
    assert_eq!(count, 1, "应广播到 1 个连接");

    let msg = rx.recv().await.unwrap();
    assert!(
        msg.contains("device_online"),
        "消息应包含 device_online: {msg}"
    );
    assert!(msg.contains("SN001"), "消息应包含设备号: {msg}");
}

#[tokio::test]
async fn test_ws_service_broadcast_device_offline() {
    let (mgr, rooms) = WsService::default_managers();
    let (tx, mut rx) = mpsc::channel(100);
    let conn_id = mgr.register(tx, None).unwrap();
    WsService::join_admin_room(&rooms, &conn_id).unwrap();

    let count = WsService::broadcast_device_offline(&mgr, &rooms, "SN002").await;
    assert_eq!(count, 1);

    let msg = rx.recv().await.unwrap();
    assert!(
        msg.contains("device_offline"),
        "消息应包含 device_offline: {msg}"
    );
}

#[tokio::test]
async fn test_ws_service_broadcast_device_heartbeat() {
    let (mgr, rooms) = WsService::default_managers();
    let (tx, mut rx) = mpsc::channel(100);
    let conn_id = mgr.register(tx, None).unwrap();
    WsService::join_admin_room(&rooms, &conn_id).unwrap();

    let count = WsService::broadcast_device_heartbeat(&mgr, &rooms, "SN003").await;
    assert_eq!(count, 1);

    let msg = rx.recv().await.unwrap();
    assert!(
        msg.contains("device_heartbeat"),
        "消息应包含 device_heartbeat: {msg}"
    );
}

#[tokio::test]
async fn test_ws_service_broadcast_no_connections() {
    let (mgr, rooms) = WsService::default_managers();
    let count = WsService::broadcast_device_online(&mgr, &rooms, "SN004").await;
    assert_eq!(count, 0, "无连接时广播应返回 0");
}

#[tokio::test]
async fn test_ws_service_join_admin_room() {
    let (mgr, rooms) = WsService::default_managers();
    let (tx, _rx) = mpsc::channel(10);
    let conn_id = mgr.register(tx, None).unwrap();

    WsService::join_admin_room(&rooms, &conn_id).unwrap();
    let admin_room = RoomId::new("admin");
    assert_eq!(rooms.room_size(&admin_room), 1);
}

// ============================================================================
// WebSocketConfig 测试
// ============================================================================

#[test]
fn test_config_validate_ok() {
    let config = WebSocketConfig::default();
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_validate_heartbeat_zero() {
    let config = WebSocketConfig {
        heartbeat_interval: Duration::ZERO,
        ..Default::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn test_config_with_max_connections() {
    let config = WebSocketConfig::new().with_max_connections(50000);
    assert_eq!(config.max_connections, 50000);
    assert!(config.validate().is_ok());
}
