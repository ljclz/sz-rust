// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! WebSocket 连接管理器（spec §5.21 规则 1）
//!
//! 基于 `dashmap` 并发 map 管理连接生命周期（建立/保持/关闭）。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::mpsc;

use crate::error::WebSocketError;

/// 连接 ID
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct ConnectionId(pub uuid::Uuid);

impl ConnectionId {
    /// 生成新连接 ID
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for ConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

/// 房间/频道标识
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RoomId(pub String);

impl RoomId {
    /// 从字符串创建房间 ID
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

/// 连接信息
#[derive(Debug)]
pub struct ConnectionInfo {
    /// 连接 ID
    pub id: ConnectionId,
    /// 关联的用户 ID（认证后设置）
    pub user_id: Option<String>,
    /// 最后活跃时间
    pub last_active: Instant,
    /// 消息发送通道
    pub sender: Option<mpsc::Sender<String>>,
}

/// WebSocket 配置（spec §6.21）
#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    /// 心跳间隔（默认 ≤ 30s，spec §6.21 规则 1）
    pub heartbeat_interval: Duration,
    /// 空闲超时（默认 ≤ 120s，spec §6.21 规则 2）
    pub idle_timeout: Duration,
    /// 最大并发连接（≥ 10000，spec §6.21 规则 3）
    pub max_connections: usize,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(120),
            max_connections: 10000,
        }
    }
}

impl WebSocketConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 校验配置
    pub fn validate(&self) -> Result<(), WebSocketError> {
        if self.heartbeat_interval == Duration::ZERO {
            return Err(WebSocketError::Config("heartbeat_interval 不能为零".into()));
        }
        if self.idle_timeout == Duration::ZERO {
            return Err(WebSocketError::Config("idle_timeout 不能为零".into()));
        }
        if self.max_connections == 0 {
            return Err(WebSocketError::Config("max_connections 不能为零".into()));
        }
        Ok(())
    }

    /// 设置最大连接数
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }
}

/// 连接管理器（spec §5.21 规则 1）
///
/// 基于 `dashmap` 并发 map 管理连接 + 房间分组。
pub struct ConnectionManager {
    /// 连接表：ConnectionId → ConnectionInfo
    connections: DashMap<ConnectionId, ConnectionInfo>,
    /// 房间表：RoomId → 连接集合
    rooms: DashMap<RoomId, HashSet<ConnectionId>>,
    /// 配置
    config: WebSocketConfig,
}

impl ConnectionManager {
    /// 创建连接管理器
    pub fn new(config: WebSocketConfig) -> Self {
        Self {
            connections: DashMap::new(),
            rooms: DashMap::new(),
            config,
        }
    }

    /// 使用默认配置创建
    pub fn with_defaults() -> Self {
        Self::new(WebSocketConfig::default())
    }

    /// 获取配置引用
    pub fn config(&self) -> &WebSocketConfig {
        &self.config
    }

    /// 注册新连接（spec §5.21 规则 1）
    ///
    /// # 后置条件
    /// - 连接数超限 → 返回 `ConnectionLimitExceeded`
    /// - 成功 → 返回连接 ID
    pub fn register(
        &self,
        sender: mpsc::Sender<String>,
        user_id: Option<String>,
    ) -> Result<ConnectionId, WebSocketError> {
        if self.connections.len() >= self.config.max_connections {
            return Err(WebSocketError::ConnectionLimitExceeded);
        }
        let id = ConnectionId::new();
        let info = ConnectionInfo {
            id: id.clone(),
            user_id,
            last_active: Instant::now(),
            sender: Some(sender),
        };
        self.connections.insert(id.clone(), info);
        Ok(id)
    }

    /// 注销连接
    pub fn unregister(&self, conn_id: &ConnectionId) {
        self.connections.remove(conn_id);
        let room_ids: Vec<RoomId> = self.rooms.iter().map(|entry| entry.key().clone()).collect();
        for room_id in &room_ids {
            if let Some(mut members) = self.rooms.get_mut(room_id) {
                members.remove(conn_id);
            }
        }
    }

    /// 更新连接活跃时间
    pub fn touch(&self, conn_id: &ConnectionId) -> Result<(), WebSocketError> {
        match self.connections.get_mut(conn_id) {
            Some(mut info) => {
                info.last_active = Instant::now();
                Ok(())
            }
            None => Err(WebSocketError::ConnectionNotFound(conn_id.0.to_string())),
        }
    }

    /// 向指定连接发送消息
    pub async fn send_to(
        &self,
        conn_id: &ConnectionId,
        message: &str,
    ) -> Result<(), WebSocketError> {
        let sender = match self.connections.get(conn_id) {
            Some(info) => info.sender.clone(),
            None => return Err(WebSocketError::ConnectionNotFound(conn_id.0.to_string())),
        };
        match sender {
            Some(s) => s
                .send(message.to_string())
                .await
                .map_err(|_| WebSocketError::ConnectionNotFound(conn_id.0.to_string())),
            None => Err(WebSocketError::ConnectionNotFound(conn_id.0.to_string())),
        }
    }

    /// 当前连接数
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// 检查连接是否存在
    pub fn contains(&self, conn_id: &ConnectionId) -> bool {
        self.connections.contains_key(conn_id)
    }

    /// 获取所有空闲超时的连接（spec §5.21 规则 2）
    pub fn idle_connections(&self) -> Vec<ConnectionId> {
        let now = Instant::now();
        self.connections
            .iter()
            .filter(|entry| now.duration_since(entry.last_active) > self.config.idle_timeout)
            .map(|entry| entry.id.clone())
            .collect()
    }

    /// 获取所有连接 ID
    pub fn connections_iter(&self) -> impl Iterator<Item = ConnectionId> + '_ {
        self.connections.iter().map(|entry| entry.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn test_connection_limit() {
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

    #[test]
    fn test_config_validate() {
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
    fn test_config_validate_idle_timeout_zero() {
        let config = WebSocketConfig {
            idle_timeout: Duration::ZERO,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_max_connections_zero() {
        let config = WebSocketConfig {
            max_connections: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[tokio::test]
    async fn test_touch_nonexistent() {
        let mgr = ConnectionManager::with_defaults();
        let fake_id = ConnectionId::new();
        let result = mgr.touch(&fake_id);
        assert!(matches!(result, Err(WebSocketError::ConnectionNotFound(_))));
    }

    #[tokio::test]
    async fn test_unregister_removes_connection() {
        let mgr = ConnectionManager::with_defaults();
        let (tx1, _rx1) = mpsc::channel(10);
        let (tx2, _rx2) = mpsc::channel(10);
        let conn1 = mgr.register(tx1, None).unwrap();
        let conn2 = mgr.register(tx2, None).unwrap();
        assert_eq!(mgr.connection_count(), 2);
        mgr.unregister(&conn1);
        assert_eq!(mgr.connection_count(), 1);
        assert!(!mgr.contains(&conn1));
        assert!(mgr.contains(&conn2));
        mgr.unregister(&conn2);
        assert_eq!(mgr.connection_count(), 0);
    }

    #[tokio::test]
    async fn test_idle_connections_multiple() {
        let config = WebSocketConfig {
            idle_timeout: Duration::from_millis(1),
            ..Default::default()
        };
        let mgr = ConnectionManager::new(config);
        let (tx1, _rx1) = mpsc::channel(10);
        let (tx2, _rx2) = mpsc::channel(10);
        let conn1 = mgr.register(tx1, None).unwrap();
        let conn2 = mgr.register(tx2, None).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        let idle = mgr.idle_connections();
        assert_eq!(idle.len(), 2);
        assert!(idle.contains(&conn1));
        assert!(idle.contains(&conn2));
    }

    #[tokio::test]
    async fn test_connections_iter_multiple() {
        let mgr = ConnectionManager::with_defaults();
        let (tx1, _rx1) = mpsc::channel(10);
        let (tx2, _rx2) = mpsc::channel(10);
        let conn1 = mgr.register(tx1, None).unwrap();
        let conn2 = mgr.register(tx2, None).unwrap();
        let ids: Vec<_> = mgr.connections_iter().collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&conn1));
        assert!(ids.contains(&conn2));
    }

    #[test]
    fn test_config_new_equals_default() {
        let c1 = WebSocketConfig::new();
        let c2 = WebSocketConfig::default();
        assert_eq!(c1.heartbeat_interval, c2.heartbeat_interval);
        assert_eq!(c1.idle_timeout, c2.idle_timeout);
        assert_eq!(c1.max_connections, c2.max_connections);
    }

    #[test]
    fn test_config_with_max_connections() {
        let config = WebSocketConfig::new().with_max_connections(50000);
        assert_eq!(config.max_connections, 50000);
        assert!(config.validate().is_ok());
    }
}
