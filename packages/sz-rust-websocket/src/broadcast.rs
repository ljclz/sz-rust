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
