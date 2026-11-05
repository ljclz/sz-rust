// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 SSE 服务 — Server-Sent Events 推送
//!
//! 支持订单状态变更推送、Last-Event-ID 重连恢复、事件过滤、背压缓冲。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, Mutex};

/// SSE 事件
#[derive(Debug, Clone)]
pub struct SseEvent {
    /// 事件 ID（用于 Last-Event-ID 恢复）
    pub id: u64,
    /// 事件类型（用于过滤）
    pub event_type: String,
    /// 事件数据
    pub data: String,
}

impl SseEvent {
    /// 创建新事件
    pub fn new(id: u64, event_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self {
            id,
            event_type: event_type.into(),
            data: data.into(),
        }
    }
}

/// SSE 服务 — 事件广播 + 历史缓存 + 背压缓冲
pub struct SseService {
    /// 广播通道
    sender: broadcast::Sender<SseEvent>,
    /// 历史事件缓存（用于 Last-Event-ID 恢复）
    history: Mutex<VecDeque<SseEvent>>,
    /// 最大历史缓存数
    max_history: usize,
    /// 下一个事件 ID
    next_id: Mutex<u64>,
    /// 缓冲区容量
    buffer_capacity: usize,
}

impl SseService {
    /// 创建 SSE 服务
    pub fn new(buffer_size: usize, max_history: usize) -> Arc<Self> {
        let (sender, _) = broadcast::channel(buffer_size);
        Arc::new(Self {
            sender,
            history: Mutex::new(VecDeque::with_capacity(max_history)),
            max_history,
            next_id: Mutex::new(0),
            buffer_capacity: buffer_size,
        })
    }

    /// 使用默认配置创建（buffer=256, history=1000）
    pub fn with_defaults() -> Arc<Self> {
        Self::new(256, 1000)
    }

    /// 发布事件
    pub async fn publish(&self, event_type: &str, data: &str) -> u64 {
        let mut next_id = self.next_id.lock().await;
        *next_id += 1;
        let id = *next_id;
        drop(next_id);

        let event = SseEvent::new(id, event_type, data);

        let mut history = self.history.lock().await;
        if history.len() >= self.max_history {
            history.pop_front();
        }
        history.push_back(event.clone());
        drop(history);

        let _ = self.sender.send(event);
        id
    }

    /// 订阅事件流（从 last_event_id 之后开始）
    pub async fn subscribe(
        &self,
        last_event_id: Option<u64>,
    ) -> (broadcast::Receiver<SseEvent>, Vec<SseEvent>) {
        let receiver = self.sender.subscribe();

        let missed = if let Some(last_id) = last_event_id {
            let history = self.history.lock().await;
            history.iter().filter(|e| e.id > last_id).cloned().collect()
        } else {
            Vec::new()
        };

        (receiver, missed)
    }

    /// 获取历史事件数量
    pub async fn history_count(&self) -> usize {
        self.history.lock().await.len()
    }

    /// 广播订单状态变更
    pub async fn broadcast_order_status(&self, order_no: &str, status: i8) -> u64 {
        let data = format!(r#"{{"order_no":"{order_no}","status":{status}}}"#);
        self.publish("order_status", &data).await
    }

    /// 广播设备状态变更
    pub async fn broadcast_device_status(&self, device_sn: &str, status: i8) -> u64 {
        let data = format!(r#"{{"device_sn":"{device_sn}","status":{status}}}"#);
        self.publish("device_status", &data).await
    }

    /// 背压检查：如果订阅者消费速度跟不上，返回 true
    pub fn check_backpressure(receiver: &broadcast::Receiver<SseEvent>) -> bool {
        receiver.len() > 200
    }

    /// 获取缓冲区容量
    pub fn capacity(&self) -> usize {
        self.buffer_capacity
    }
}

/// SSE 背压缓冲配置
#[derive(Debug, Clone)]
pub struct BackpressureConfig {
    /// 最大缓冲事件数
    pub max_buffer: usize,
    /// 溢出时丢弃策略
    pub drop_oldest: bool,
    /// 检查间隔
    pub check_interval: Duration,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            max_buffer: 1000,
            drop_oldest: true,
            check_interval: Duration::from_secs(1),
        }
    }
}

impl BackpressureConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_event_new_constructs_fields() {
        let e = SseEvent::new(7, "order_status", r#"{"order_no":"X1"}"#);
        assert_eq!(e.id, 7);
        assert_eq!(e.event_type, "order_status");
        assert_eq!(e.data, r#"{"order_no":"X1"}"#);
    }

    #[test]
    fn sse_service_new_records_capacity() {
        let svc = SseService::new(128, 500);
        assert_eq!(svc.capacity(), 128);
        assert_eq!(svc.buffer_capacity, 128);
        assert_eq!(svc.max_history, 500);
    }

    #[tokio::test]
    async fn with_defaults_uses_documented_constants() {
        let svc = SseService::with_defaults();
        assert_eq!(svc.capacity(), 256);
        assert_eq!(svc.max_history, 1000);
        // 初始历史为空
        assert_eq!(svc.history_count().await, 0);
    }

    #[tokio::test]
    async fn publish_returns_monotonic_ids_and_grows_history() {
        let svc = SseService::new(64, 100);
        let id1 = svc.publish("e", "d1").await;
        let id2 = svc.publish("e", "d2").await;
        let id3 = svc.publish("e", "d3").await;
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
        assert_eq!(svc.history_count().await, 3);
    }

    #[tokio::test]
    async fn publish_evicts_oldest_when_history_exceeds_max() {
        let svc = SseService::new(64, 3);
        svc.publish("e", "d1").await;
        svc.publish("e", "d2").await;
        svc.publish("e", "d3").await;
        assert_eq!(svc.history_count().await, 3);
        // 第 4 条触发 pop_front，最旧的 d1 被驱逐
        let id4 = svc.publish("e", "d4").await;
        assert_eq!(id4, 4);
        assert_eq!(svc.history_count().await, 3);
        // subscribe 用 last_event_id=1 应只能拿到 id>1 的事件（d2/d3/d4）
        let (_rx, missed) = svc.subscribe(Some(1)).await;
        assert_eq!(missed.len(), 3);
        assert_eq!(missed[0].id, 2);
        assert_eq!(missed[2].id, 4);
    }

    #[tokio::test]
    async fn subscribe_without_last_event_id_returns_empty_missed() {
        let svc = SseService::with_defaults();
        svc.publish("e", "d1").await;
        let (_rx, missed) = svc.subscribe(None).await;
        assert!(missed.is_empty());
    }

    #[tokio::test]
    async fn subscribe_with_last_event_id_filters_history() {
        let svc = SseService::new(64, 100);
        svc.publish("e", "d1").await; // id=1
        svc.publish("e", "d2").await; // id=2
        svc.publish("e", "d3").await; // id=3
        let (_rx, missed) = svc.subscribe(Some(2)).await;
        // 只返回 id>2 的事件
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].id, 3);
    }

    #[tokio::test]
    async fn broadcast_order_status_emits_expected_payload() {
        let svc = SseService::with_defaults();
        let id = svc.broadcast_order_status("ORD-1", 2).await;
        assert!(id >= 1);
        let (_rx, missed) = svc.subscribe(Some(0)).await;
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].event_type, "order_status");
        assert!(missed[0].data.contains(r#""order_no":"ORD-1""#));
        assert!(missed[0].data.contains(r#""status":2"#));
    }

    #[tokio::test]
    async fn broadcast_device_status_emits_expected_payload() {
        let svc = SseService::with_defaults();
        let id = svc.broadcast_device_status("SN-ABC", 1).await;
        assert!(id >= 1);
        let (_rx, missed) = svc.subscribe(Some(0)).await;
        assert_eq!(missed.len(), 1);
        assert_eq!(missed[0].event_type, "device_status");
        assert!(missed[0].data.contains(r#""device_sn":"SN-ABC""#));
        assert!(missed[0].data.contains(r#""status":1"#));
    }

    #[tokio::test]
    async fn check_backpressure_true_when_receiver_buffer_exceeds_threshold() {
        let svc = SseService::new(300, 1000);
        let (rx, _missed) = svc.subscribe(None).await;
        // publish 250 条（< capacity 300，不丢），receiver 缓冲 250 > 200
        for i in 0..250 {
            svc.publish("e", &format!("d{i}")).await;
        }
        assert!(SseService::check_backpressure(&rx), "receiver.len={} 应 > 200", rx.len());
    }

    #[tokio::test]
    async fn check_backpressure_false_when_receiver_buffer_below_threshold() {
        let svc = SseService::with_defaults();
        let (rx, _missed) = svc.subscribe(None).await;
        for i in 0..10 {
            svc.publish("e", &format!("d{i}")).await;
        }
        assert!(!SseService::check_backpressure(&rx), "receiver.len={} 应 <= 200", rx.len());
    }

    #[test]
    fn backpressure_config_default_matches_documented_values() {
        let cfg = BackpressureConfig::default();
        assert_eq!(cfg.max_buffer, 1000);
        assert!(cfg.drop_oldest);
        assert_eq!(cfg.check_interval, Duration::from_secs(1));
        // new() 等价于 default()
        let cfg2 = BackpressureConfig::new();
        assert_eq!(cfg2.max_buffer, cfg.max_buffer);
        assert_eq!(cfg2.check_interval, cfg.check_interval);
    }
}
