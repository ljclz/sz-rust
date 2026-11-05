// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 WebSocket 服务 — 设备状态推送
//!
//! 设备上线/下线/心跳 → 广播到 admin 房间。

use std::sync::Arc;

use sz_rust_websocket::manager::{ConnectionId, ConnectionManager, RoomId};
use sz_rust_websocket::room::RoomManager;

/// WebSocket 服务
pub struct WsService;

impl WsService {
    /// 广播设备上线消息到 admin 房间
    pub async fn broadcast_device_online(
        mgr: &ConnectionManager,
        rooms: &RoomManager,
        device_sn: &str,
    ) -> usize {
        let room = RoomId::new("admin");
        let msg = format!(r#"{{"event":"device_online","device_sn":"{device_sn}"}}"#);
        rooms.broadcast_room(mgr, &room, &msg).await.unwrap_or(0)
    }

    /// 广播设备下线消息到 admin 房间
    pub async fn broadcast_device_offline(
        mgr: &ConnectionManager,
        rooms: &RoomManager,
        device_sn: &str,
    ) -> usize {
        let room = RoomId::new("admin");
        let msg = format!(r#"{{"event":"device_offline","device_sn":"{device_sn}"}}"#);
        rooms.broadcast_room(mgr, &room, &msg).await.unwrap_or(0)
    }

    /// 广播设备心跳消息到 admin 房间
    pub async fn broadcast_device_heartbeat(
        mgr: &ConnectionManager,
        rooms: &RoomManager,
        device_sn: &str,
    ) -> usize {
        let room = RoomId::new("admin");
        let msg = format!(r#"{{"event":"device_heartbeat","device_sn":"{device_sn}"}}"#);
        rooms.broadcast_room(mgr, &room, &msg).await.unwrap_or(0)
    }

    /// 加入 admin 房间
    pub fn join_admin_room(
        rooms: &RoomManager,
        conn_id: &ConnectionId,
    ) -> Result<(), sz_rust_websocket::WebSocketError> {
        let room = RoomId::new("admin");
        rooms.join_room(conn_id, &room)
    }

    /// 创建默认的 ConnectionManager + RoomManager
    pub fn default_managers() -> (Arc<ConnectionManager>, Arc<RoomManager>) {
        (
            Arc::new(ConnectionManager::with_defaults()),
            Arc::new(RoomManager::new()),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn default_managers_returns_fresh_instances() {
        let (mgr, rooms) = WsService::default_managers();
        // 默认配置 max_connections=10000，空连接表
        assert_eq!(mgr.config().max_connections, 10000);
        // 空房间
        assert_eq!(rooms.room_count(), 0);
    }

    #[test]
    fn join_admin_room_succeeds_for_new_connection() {
        let rooms = RoomManager::new();
        let conn_id = ConnectionId::new();
        let result = WsService::join_admin_room(&rooms, &conn_id);
        assert!(result.is_ok(), "join_admin_room 应成功");
        // admin 房间应有 1 个成员
        let admin = RoomId::new("admin");
        assert_eq!(rooms.room_size(&admin), 1);
    }

    #[tokio::test]
    async fn broadcast_device_online_returns_zero_for_empty_room() {
        let mgr = ConnectionManager::with_defaults();
        let rooms = RoomManager::new();
        // admin 房间无成员 → 广播返回 0
        let sent = WsService::broadcast_device_online(&mgr, &rooms, "SN-001").await;
        assert_eq!(sent, 0);
    }

    #[tokio::test]
    async fn broadcast_device_offline_returns_zero_for_empty_room() {
        let mgr = ConnectionManager::with_defaults();
        let rooms = RoomManager::new();
        let sent = WsService::broadcast_device_offline(&mgr, &rooms, "SN-002").await;
        assert_eq!(sent, 0);
    }

    #[tokio::test]
    async fn broadcast_device_heartbeat_delivers_to_admin_members() {
        let mgr = ConnectionManager::with_defaults();
        let rooms = RoomManager::new();
        // 注册一个连接并加入 admin 房间
        let (tx, mut rx) = mpsc::channel(10);
        let conn_id = mgr.register(tx, None).expect("register");
        rooms.join_room(&conn_id, &RoomId::new("admin")).expect("join");
        let sent = WsService::broadcast_device_heartbeat(&mgr, &rooms, "SN-003").await;
        assert_eq!(sent, 1, "应成功投递给 1 个 admin 成员");
        let msg = rx.recv().await.expect("应收到消息");
        assert!(msg.contains(r#""event":"device_heartbeat""#), "msg={msg}");
        assert!(msg.contains(r#""device_sn":"SN-003""#), "msg={msg}");
    }
}
