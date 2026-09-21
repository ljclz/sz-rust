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
    TenantScopedTableChanged {
        generation: u64,
        table_name: String,
        action: &'static str,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    TenantConfigChanged {
        generation: u64,
        tenant_id: i64,
        config_key: String,
        is_global: bool,
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    TenantStatusChanged {
        generation: u64,
        tenant_id: i64,
        from: String,
        to: String,
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

    #[test]
    fn test_broadcast_no_subscriber_silent() {
        let notifier = ChangeNotifier::new(16);
        // 无订阅者广播：SendError 静默忽略，不得 panic
        notifier.broadcast(make_event());
        // broadcast 无历史回放：广播后才订阅的接收者不应收到先前事件
        let mut rx = notifier.subscribe();
        assert!(rx.try_recv().is_err(), "无订阅者时的广播不应残留可接收事件");
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

    #[test]
    fn test_tenant_change_event_serialize() {
        let event1 = ChangeEvent::TenantScopedTableChanged {
            generation: 1,
            table_name: "orders".into(),
            action: "registered",
            timestamp: Utc::now(),
        };
        let json1 = serde_json::to_string(&event1).unwrap();
        assert!(json1.contains("TenantScopedTableChanged"));
        assert!(json1.contains("orders"));

        let event2 = ChangeEvent::TenantConfigChanged {
            generation: 2,
            tenant_id: 1,
            config_key: "theme".into(),
            is_global: false,
            timestamp: Utc::now(),
        };
        let json2 = serde_json::to_string(&event2).unwrap();
        assert!(json2.contains("TenantConfigChanged"));
        assert!(json2.contains("theme"));

        let event3 = ChangeEvent::TenantStatusChanged {
            generation: 3,
            tenant_id: 1,
            from: "active".into(),
            to: "suspended".into(),
            timestamp: Utc::now(),
        };
        let json3 = serde_json::to_string(&event3).unwrap();
        assert!(json3.contains("TenantStatusChanged"));
        assert!(json3.contains("suspended"));
    }
}
