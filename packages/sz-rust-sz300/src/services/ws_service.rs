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
