// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 消息广播（spec §5.21 规则 3）
//!
//! 全局广播 + 房间广播（单消息广播延迟 ≤ 1ms）。

use crate::error::WebSocketError;
use crate::manager::{ConnectionId, ConnectionManager};

/// 广播器（spec §5.21 规则 3）
pub struct Broadcaster;

impl Broadcaster {
    /// 全局广播（spec §5.21 规则 3）
    ///
    /// 向所有连接发送消息，返回成功发送数。
    pub async fn broadcast_all(
        mgr: &ConnectionManager,
        message: &str,
    ) -> Result<usize, WebSocketError> {
        let conn_ids: Vec<ConnectionId> = mgr.connections_iter().collect();
        let mut sent = 0;
        for conn_id in &conn_ids {
            if mgr.send_to(conn_id, message).await.is_ok() {
                sent += 1;
            }
        }
        Ok(sent)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::ConnectionManager;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_broadcast_all_empty() {
        let mgr = ConnectionManager::with_defaults();
        let sent = Broadcaster::broadcast_all(&mgr, "hello").await.unwrap();
        assert_eq!(sent, 0);
    }

    #[tokio::test]
    async fn test_broadcast_all_single() {
        let mgr = ConnectionManager::with_defaults();
        let (tx, mut rx) = mpsc::channel(10);
        mgr.register(tx, None).unwrap();
        let sent = Broadcaster::broadcast_all(&mgr, "ping").await.unwrap();
        assert_eq!(sent, 1);
        assert_eq!(rx.recv().await.unwrap(), "ping");
    }

    #[tokio::test]
    async fn test_broadcast_all_multiple() {
        let mgr = ConnectionManager::with_defaults();
        let (tx1, mut rx1) = mpsc::channel(10);
        let (tx2, mut rx2) = mpsc::channel(10);
        let (tx3, mut rx3) = mpsc::channel(10);
        mgr.register(tx1, Some("u1".into())).unwrap();
        mgr.register(tx2, Some("u2".into())).unwrap();
        mgr.register(tx3, None).unwrap();
        let sent = Broadcaster::broadcast_all(&mgr, "broadcast").await.unwrap();
        assert_eq!(sent, 3);
        assert_eq!(rx1.recv().await.unwrap(), "broadcast");
        assert_eq!(rx2.recv().await.unwrap(), "broadcast");
        assert_eq!(rx3.recv().await.unwrap(), "broadcast");
    }

    #[tokio::test]
    async fn test_broadcast_all_after_unregister() {
        let mgr = ConnectionManager::with_defaults();
        let (tx1, _rx1) = mpsc::channel(10);
        let (tx2, mut rx2) = mpsc::channel(10);
        let conn1 = mgr.register(tx1, None).unwrap();
        mgr.register(tx2, None).unwrap();
        mgr.unregister(&conn1);
        let sent = Broadcaster::broadcast_all(&mgr, "msg").await.unwrap();
        assert_eq!(sent, 1);
        assert_eq!(rx2.recv().await.unwrap(), "msg");
    }

    #[tokio::test]
    async fn test_broadcast_all_dropped_receiver() {
        let mgr = ConnectionManager::with_defaults();
        let (tx1, rx1) = mpsc::channel(10);
        let (tx2, mut rx2) = mpsc::channel(10);
        mgr.register(tx1, None).unwrap();
        mgr.register(tx2, None).unwrap();
        drop(rx1);
        let sent = Broadcaster::broadcast_all(&mgr, "msg").await.unwrap();
        assert_eq!(sent, 1);
        assert_eq!(rx2.recv().await.unwrap(), "msg");
    }
}
