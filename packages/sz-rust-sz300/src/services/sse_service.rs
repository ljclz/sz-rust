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
