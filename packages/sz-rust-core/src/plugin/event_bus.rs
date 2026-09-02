//! 事件总线 trait + EventHandler trait 抽象。
//!
//! 对应 design.md §2.2.2 接口 7-8。
//! 所有 trait 必须 `Send + Sync + 'static`。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::schema::SysEvent;

/// 事件 ID 类型。
pub type EventId = i64;

/// 订阅 ID 类型。
pub type SubscriptionId = u64;

/// 插件事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEvent {
    /// 事件 ID（发布后分配）
    pub id: EventId,
    /// 租户 ID（多租户隔离）
    pub tenant_id: i64,
    /// 事件类型（如 order.created）
    pub event_type: String,
    /// 来源插件名
    pub source_plugin: String,
    /// 事件负载（JSON）
    pub payload: serde_json::Value,
}

impl From<SysEvent> for PluginEvent {
    fn from(e: SysEvent) -> Self {
        Self {
            id: e.id,
            tenant_id: e.tenant_id,
            event_type: e.event_type,
            source_plugin: e.source_plugin,
            payload: e.payload,
        }
    }
}

/// 事件处理器 trait。
#[async_trait]
pub trait EventHandler: Send + Sync + 'static {
    /// 处理事件，返回 `Ok(())` 表示处理成功。
    async fn handle(&self, event: &PluginEvent) -> Result<(), String>;
}

/// 事件总线 trait。
#[async_trait]
pub trait EventBus: Send + Sync + 'static {
    /// 发布事件，返回事件 ID。
    async fn publish(&self, event: &PluginEvent) -> Result<EventId, String>;

    /// 订阅事件，返回订阅 ID。
    async fn subscribe(
        &self,
        event_type: &str,
        handler: Arc<dyn EventHandler>,
    ) -> Result<SubscriptionId, String>;

    /// 取消订阅。
    async fn unsubscribe(&self, sub_id: SubscriptionId) -> Result<(), String>;

    /// 重放未投递事件（至少一次投递保障）。
    async fn replay_pending(&self) -> Result<usize, String>;
}

/// 事件总线中的订阅表条目：订阅 ID + 处理器
pub type SubscriptionEntry = (SubscriptionId, Arc<dyn EventHandler>);

/// 事件总线中的订阅表：event_type → 订阅条目列表
pub type SubscriptionMap = std::collections::HashMap<String, Vec<SubscriptionEntry>>;

/// 内存事件总线实现（用于测试和轻量场景）。
pub struct InMemoryEventBus {
    /// 已发布事件记录（按序追加）
    events: parking_lot::RwLock<Vec<PluginEvent>>,
    /// 下一个事件 ID 计数器
    next_id: parking_lot::Mutex<EventId>,
    /// 订阅表：event_type → [(sub_id, handler)]
    subscribers: parking_lot::RwLock<SubscriptionMap>,
    /// 下一个订阅 ID 计数器
    next_sub_id: parking_lot::Mutex<SubscriptionId>,
}

impl InMemoryEventBus {
    /// 创建空的内存事件总线
    pub fn new() -> Self {
        Self {
            events: parking_lot::RwLock::new(Vec::new()),
            next_id: parking_lot::Mutex::new(1),
            subscribers: parking_lot::RwLock::new(std::collections::HashMap::new()),
            next_sub_id: parking_lot::Mutex::new(1),
        }
    }
}

impl Default for InMemoryEventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventBus for InMemoryEventBus {
    async fn publish(&self, event: &PluginEvent) -> Result<EventId, String> {
        let id = {
            let mut next = self.next_id.lock();
            let id = *next;
            *next += 1;
            id
        };
        let mut event = event.clone();
        event.id = id;
        let event_type = event.event_type.clone();
        self.events.write().push(event.clone());
        let handlers: Vec<Arc<dyn EventHandler>> = {
            let subs = self.subscribers.read();
            subs.get(&event_type)
                .map(|v| v.iter().map(|(_, h)| h.clone()).collect())
                .unwrap_or_default()
        };
        for handler in handlers {
            let _ = handler.handle(&event).await;
        }
        Ok(id)
    }

    async fn subscribe(
        &self,
        event_type: &str,
        handler: Arc<dyn EventHandler>,
    ) -> Result<SubscriptionId, String> {
        let sub_id = {
            let mut next = self.next_sub_id.lock();
            let id = *next;
            *next += 1;
            id
        };
        let mut subs = self.subscribers.write();
        subs.entry(event_type.to_string())
            .or_default()
            .push((sub_id, handler));
        Ok(sub_id)
    }

    async fn unsubscribe(&self, sub_id: SubscriptionId) -> Result<(), String> {
        let mut subs = self.subscribers.write();
        for handlers in subs.values_mut() {
            handlers.retain(|(id, _)| *id != sub_id);
        }
        Ok(())
    }

    async fn replay_pending(&self) -> Result<usize, String> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CountHandler {
        count: parking_lot::Mutex<usize>,
    }

    #[async_trait]
    impl EventHandler for CountHandler {
        async fn handle(&self, _event: &PluginEvent) -> Result<(), String> {
            *self.count.lock() += 1;
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_publish_and_subscribe() {
        let bus = InMemoryEventBus::new();
        let handler = Arc::new(CountHandler {
            count: parking_lot::Mutex::new(0),
        });
        let _ = bus.subscribe("test.event", handler.clone()).await;
        let event = PluginEvent {
            id: 0,
            tenant_id: 1,
            event_type: "test.event".to_string(),
            source_plugin: "test".to_string(),
            payload: serde_json::json!({}),
        };
        let id = bus.publish(&event).await.expect("发布失败");
        assert!(id > 0);
        assert_eq!(*handler.count.lock(), 1);
    }

    #[tokio::test]
    async fn test_publish_id_increments_monotonically() {
        let bus = InMemoryEventBus::new();
        let event1 = PluginEvent {
            id: 0,
            tenant_id: 1,
            event_type: "a".to_string(),
            source_plugin: "t".to_string(),
            payload: serde_json::json!({}),
        };
        let event2 = PluginEvent {
            id: 0,
            tenant_id: 1,
            event_type: "b".to_string(),
            source_plugin: "t".to_string(),
            payload: serde_json::json!({}),
        };
        let id1 = bus.publish(&event1).await.unwrap();
        let id2 = bus.publish(&event2).await.unwrap();
        assert_eq!(id2, id1 + 1);
    }

    #[tokio::test]
    async fn test_subscribe_id_increments_monotonically() {
        let bus = InMemoryEventBus::new();
        let h1: Arc<dyn EventHandler> = Arc::new(CountHandler {
            count: parking_lot::Mutex::new(0),
        });
        let h2: Arc<dyn EventHandler> = Arc::new(CountHandler {
            count: parking_lot::Mutex::new(0),
        });
        let sub1 = bus.subscribe("type1", h1).await.unwrap();
        let sub2 = bus.subscribe("type2", h2).await.unwrap();
        assert_eq!(sub2, sub1 + 1);
    }
}
