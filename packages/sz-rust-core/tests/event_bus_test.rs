use std::sync::Arc;
use sz_rust_core::plugin::{EventBus, EventHandler, InMemoryEventBus, PluginEvent};

struct CountHandler {
    count: parking_lot::Mutex<usize>,
}

impl CountHandler {
    fn new() -> Self {
        Self {
            count: parking_lot::Mutex::new(0),
        }
    }
    fn get(&self) -> usize {
        *self.count.lock()
    }
}

#[async_trait::async_trait]
impl EventHandler for CountHandler {
    async fn handle(&self, _event: &PluginEvent) -> Result<(), String> {
        *self.count.lock() += 1;
        Ok(())
    }
}

struct FailHandler {
    attempts: parking_lot::Mutex<usize>,
}

#[async_trait::async_trait]
impl EventHandler for FailHandler {
    async fn handle(&self, _event: &PluginEvent) -> Result<(), String> {
        *self.attempts.lock() += 1;
        Err("模拟处理失败".to_string())
    }
}

fn make_event(event_type: &str) -> PluginEvent {
    PluginEvent {
        id: 0,
        tenant_id: 1,
        event_type: event_type.to_string(),
        source_plugin: "test".to_string(),
        payload: serde_json::json!({}),
    }
}

#[tokio::test]
async fn test_publish_persists_event() {
    let bus = InMemoryEventBus::new();
    let event = make_event("order.created");
    let id = bus.publish(&event).await.expect("发布失败");
    assert!(id > 0, "事件 ID 应大于 0");
}

#[tokio::test]
async fn test_subscriber_receives_event() {
    let bus = InMemoryEventBus::new();
    let handler = Arc::new(CountHandler::new());
    let _ = bus.subscribe("test.event", handler.clone()).await;
    bus.publish(&make_event("test.event"))
        .await
        .expect("发布失败");
    assert_eq!(handler.get(), 1, "订阅者应收到 1 个事件");
}

#[tokio::test]
async fn test_multiple_subscribers() {
    let bus = InMemoryEventBus::new();
    let h1 = Arc::new(CountHandler::new());
    let h2 = Arc::new(CountHandler::new());
    let _ = bus.subscribe("multi.event", h1.clone()).await;
    let _ = bus.subscribe("multi.event", h2.clone()).await;
    bus.publish(&make_event("multi.event"))
        .await
        .expect("发布失败");
    assert_eq!(h1.get(), 1);
    assert_eq!(h2.get(), 1);
}

#[tokio::test]
async fn test_unsubscribe() {
    let bus = InMemoryEventBus::new();
    let handler = Arc::new(CountHandler::new());
    let sub_id = bus
        .subscribe("unsub.event", handler.clone())
        .await
        .expect("订阅失败");
    bus.publish(&make_event("unsub.event"))
        .await
        .expect("发布失败");
    assert_eq!(handler.get(), 1);
    bus.unsubscribe(sub_id).await.expect("取消订阅失败");
    bus.publish(&make_event("unsub.event"))
        .await
        .expect("发布失败");
    assert_eq!(handler.get(), 1, "取消订阅后不应再收到事件");
}

#[tokio::test]
async fn test_handler_failure_does_not_crash() {
    let bus = InMemoryEventBus::new();
    let fail_handler = Arc::new(FailHandler {
        attempts: parking_lot::Mutex::new(0),
    });
    let _ = bus.subscribe("fail.event", fail_handler.clone()).await;
    let result = bus.publish(&make_event("fail.event")).await;
    assert!(result.is_ok(), "发布不应因处理器失败而报错");
}

#[tokio::test]
async fn test_replay_pending() {
    let bus = InMemoryEventBus::new();
    let count = bus.replay_pending().await.expect("重放失败");
    assert_eq!(count, 0, "无待重放事件");
}

#[tokio::test]
async fn test_event_isolation_by_type() {
    let bus = InMemoryEventBus::new();
    let handler_a = Arc::new(CountHandler::new());
    let handler_b = Arc::new(CountHandler::new());
    let _ = bus.subscribe("type.a", handler_a.clone()).await;
    let _ = bus.subscribe("type.b", handler_b.clone()).await;
    bus.publish(&make_event("type.a")).await.expect("发布失败");
    assert_eq!(handler_a.get(), 1, "type.a 订阅者应收到");
    assert_eq!(handler_b.get(), 0, "type.b 订阅者不应收到");
}
