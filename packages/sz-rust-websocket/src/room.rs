// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 房间/频道管理（spec §5.21 规则 4，§6.21 规则 4）
//!
//! 连接按房间/频道分组，组内广播与隔离。

use std::collections::HashSet;

use dashmap::DashMap;

use crate::error::WebSocketError;
use crate::manager::{ConnectionId, ConnectionManager, RoomId};

/// 房间管理器（spec §5.21 规则 4）
pub struct RoomManager {
    /// 房间表：RoomId → 连接集合
    rooms: DashMap<RoomId, HashSet<ConnectionId>>,
}

impl RoomManager {
    /// 创建房间管理器
    pub fn new() -> Self {
        Self {
            rooms: DashMap::new(),
        }
    }

    /// 加入房间（spec §5.21 规则 4）
    pub fn join_room(&self, conn_id: &ConnectionId, room: &RoomId) -> Result<(), WebSocketError> {
        let mut entry = self.rooms.entry(room.clone()).or_default();
        entry.insert(conn_id.clone());
        Ok(())
    }

    /// 离开房间
    pub fn leave_room(&self, conn_id: &ConnectionId, room: &RoomId) -> Result<(), WebSocketError> {
        let should_remove = match self.rooms.get_mut(room) {
            Some(mut members) => {
                members.remove(conn_id);
                members.is_empty()
            }
            None => return Err(WebSocketError::RoomNotFound(room.0.clone())),
        };
        if should_remove {
            self.rooms.remove(room);
        }
        Ok(())
    }

    /// 获取房间成员
    pub fn room_members(&self, room: &RoomId) -> Vec<ConnectionId> {
        match self.rooms.get(room) {
            Some(members) => members.iter().cloned().collect(),
            None => Vec::new(),
        }
    }

    /// 房间成员数
    pub fn room_size(&self, room: &RoomId) -> usize {
        self.rooms.get(room).map(|m| m.len()).unwrap_or(0)
    }

    /// 获取所有房间
    pub fn all_rooms(&self) -> Vec<RoomId> {
        self.rooms.iter().map(|entry| entry.key().clone()).collect()
    }

    /// 房间数量
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// 广播到房间（spec §5.21 规则 3，延迟 ≤ 1ms）
    ///
    /// 返回成功发送的消息数。
    pub async fn broadcast_room(
        &self,
        mgr: &ConnectionManager,
        room: &RoomId,
        message: &str,
    ) -> Result<usize, WebSocketError> {
        let members = self.room_members(room);
        if members.is_empty() {
            return Ok(0);
        }
        let mut sent = 0;
        for conn_id in &members {
            if mgr.send_to(conn_id, message).await.is_ok() {
                sent += 1;
            }
        }
        Ok(sent)
    }

    /// 从连接管理器注销时清理所有房间中的该连接
    pub fn cleanup_connection(&self, conn_id: &ConnectionId) {
        let rooms_to_clean: Vec<RoomId> = self
            .rooms
            .iter()
            .filter(|entry| entry.value().contains(conn_id))
            .map(|entry| entry.key().clone())
            .collect();
        let mut empty_rooms: Vec<RoomId> = Vec::new();
        for room_id in &rooms_to_clean {
            if let Some(mut members) = self.rooms.get_mut(room_id) {
                members.remove(conn_id);
                if members.is_empty() {
                    empty_rooms.push(room_id.clone());
                }
            }
        }
        for room in empty_rooms {
            self.rooms.remove(&room);
        }
    }
}

impl Default for RoomManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_join_and_leave_room() {
        let rooms = RoomManager::new();
        let conn_id = ConnectionId::new();
        let room = RoomId::new("general");
        rooms.join_room(&conn_id, &room).unwrap();
        assert_eq!(rooms.room_size(&room), 1);
        rooms.leave_room(&conn_id, &room).unwrap();
        assert_eq!(rooms.room_size(&room), 0);
    }

    #[tokio::test]
    async fn test_room_members() {
        let rooms = RoomManager::new();
        let c1 = ConnectionId::new();
        let c2 = ConnectionId::new();
        let room = RoomId::new("test");
        rooms.join_room(&c1, &room).unwrap();
        rooms.join_room(&c2, &room).unwrap();
        assert_eq!(rooms.room_size(&room), 2);
        let members = rooms.room_members(&room);
        assert!(members.contains(&c1));
        assert!(members.contains(&c2));
    }

    #[tokio::test]
    async fn test_broadcast_room() {
        let mgr = ConnectionManager::with_defaults();
        let rooms = RoomManager::new();
        let (tx1, mut rx1) = mpsc::channel(10);
        let (tx2, mut rx2) = mpsc::channel(10);
        let c1 = mgr.register(tx1, None).unwrap();
        let c2 = mgr.register(tx2, None).unwrap();
        let room = RoomId::new("broadcast");
        rooms.join_room(&c1, &room).unwrap();
        rooms.join_room(&c2, &room).unwrap();
        let sent = rooms.broadcast_room(&mgr, &room, "hello").await.unwrap();
        assert_eq!(sent, 2);
        assert_eq!(rx1.recv().await.unwrap(), "hello");
        assert_eq!(rx2.recv().await.unwrap(), "hello");
    }

    #[tokio::test]
    async fn test_broadcast_empty_room() {
        let mgr = ConnectionManager::with_defaults();
        let rooms = RoomManager::new();
        let room = RoomId::new("empty");
        let sent = rooms.broadcast_room(&mgr, &room, "hello").await.unwrap();
        assert_eq!(sent, 0);
    }

    #[tokio::test]
    async fn test_cleanup_connection() {
        let rooms = RoomManager::new();
        let c1 = ConnectionId::new();
        let c2 = ConnectionId::new();
        let room = RoomId::new("test");
        rooms.join_room(&c1, &room).unwrap();
        rooms.join_room(&c2, &room).unwrap();
        rooms.cleanup_connection(&c1);
        assert_eq!(rooms.room_size(&room), 1);
    }

    #[tokio::test]
    async fn test_all_rooms() {
        let rooms = RoomManager::new();
        let c = ConnectionId::new();
        rooms.join_room(&c, &RoomId::new("a")).unwrap();
        rooms.join_room(&c, &RoomId::new("b")).unwrap();
        assert_eq!(rooms.room_count(), 2);
        let all = rooms.all_rooms();
        assert_eq!(all.len(), 2);
    }
}
