// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 配置变更通知器 — 基于 tokio broadcast 的事件广播

use serde::Serialize;
use tokio::sync::broadcast;

/// 配置变更事件
#[derive(Debug, Clone, Serialize)]
pub enum ChangeEvent {
    RuleCreated {
        generation: u64,
        target_table: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    RuleUpdated {
        generation: u64,
        target_table: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    RuleDeleted {
        generation: u64,
        target_table: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    PolicyCreated {
        generation: u64,
        target_table: String,
        target_role: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    PolicyUpdated {
        generation: u64,
        target_table: String,
        target_role: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    PolicyDeleted {
        generation: u64,
        target_table: String,
        target_role: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    ConfigReloaded {
        generation: u64,
        target_table: String,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
}

/// 配置变更通知器
pub struct ChangeNotifier {
    sender: broadcast::Sender<ChangeEvent>,
}

impl ChangeNotifier {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// 订阅变更事件
    pub fn subscribe(&self) -> broadcast::Receiver<ChangeEvent> {
        self.sender.subscribe()
    }

    /// 广播事件（无订阅者时 SendError 静默忽略）
    pub fn broadcast(&self, event: ChangeEvent) {
        let _ = self.sender.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_event() -> ChangeEvent {
        ChangeEvent::RuleCreated {
            generation: 1,
            target_table: "order".into(),
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_broadcast_received_by_subscriber() {
        let notifier = ChangeNotifier::new(16);
        let mut rx = notifier.subscribe();
        notifier.broadcast(make_event());
        let event = rx.recv().await.unwrap();
        match event {
            ChangeEvent::RuleCreated { target_table, .. } => {
                assert_eq!(target_table, "order");
            }
            _ => panic!("expected RuleCreated"),
        }
    }

    #[tokio::test]
    async fn test_broadcast_no_subscriber_silent() {
        let notifier = ChangeNotifier::new(16);
        // 无订阅者，广播不应 panic
        notifier.broadcast(make_event());
    }

    #[test]
    fn test_change_event_serialize() {
        let event = ChangeEvent::PolicyUpdated {
            generation: 5,
            target_table: "employee".into(),
            target_role: "hr".into(),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("PolicyUpdated"));
        assert!(json.contains("employee"));
        assert!(json.contains("hr"));
    }
}
