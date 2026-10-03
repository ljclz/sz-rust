// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Server-Sent Events (SSE) 支持
//!
//! 基于 axum 0.8 的 `axum::response::sse` 模块，提供轻量级服务器推送能力。
//!
//! ## 与 WebSocket 的区别
//!
//! | 维度 | SSE | WebSocket |
//! |------|-----|-----------|
//! | 方向 | 服务器 → 客户端（单向） | 双向 |
//! | 协议 | HTTP | WS |
//! | 重连 | 自动重连 | 需手动 |
//! | 浏览器支持 | EventSource API | WebSocket API |
//! | 适用场景 | 通知/日志流/进度 | 聊天/实时交互 |
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_http_facade::sse::{SseEvent, sse_response};
//! use futures::stream::{self, StreamExt};
//!
//! async fn events_handler() -> impl IntoResponse {
//!     let stream = stream::iter(vec![
//!         SseEvent::data("hello").event("greeting"),
//!         SseEvent::data("world").event("message"),
//!     ])
//!     .map(Ok);
//!     sse_response(stream)
//! }
//! ```

use axum::response::sse::{Event as AxumEvent, KeepAlive, Sse};
use axum::response::IntoResponse;
use core::convert::Infallible;
use futures::stream::{Stream, StreamExt};

/// SSE 事件构建器
///
/// 对 `axum::response::sse::Event` 的封装，提供更简洁的 API。
#[derive(Debug, Clone)]
pub struct SseEvent {
    data: String,
    event: Option<String>,
    id: Option<String>,
    retry: Option<u64>,
}

impl SseEvent {
    /// 创建数据事件
    pub fn data(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            event: None,
            id: None,
            retry: None,
        }
    }

    /// 设置事件类型
    pub fn event(mut self, event: impl Into<String>) -> Self {
        self.event = Some(event.into());
        self
    }

    /// 设置事件 ID
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// 设置重连等待时间（毫秒）
    pub fn retry(mut self, retry_ms: u64) -> Self {
        self.retry = Some(retry_ms);
        self
    }

    /// 转换为 axum Event
    pub fn into_axum_event(self) -> Result<AxumEvent, Infallible> {
        let mut event = AxumEvent::default().data(&self.data);
        if let Some(name) = self.event {
            event = event.event(name);
        }
        if let Some(id) = self.id {
            event = event.id(id);
        }
        if let Some(retry) = self.retry {
            event = event.retry(std::time::Duration::from_millis(retry));
        }
        Ok(event)
    }
}

/// 创建 SSE 响应，带 KeepAlive
///
/// 接受 `Stream<Item = Result<SseEvent, Infallible>>`，内部转换为 axum Event 流。
pub fn sse_response<S>(stream: S) -> impl IntoResponse
where
    S: Stream<Item = Result<SseEvent, Infallible>> + Send + 'static,
{
    let axum_stream = stream.map(|item| item.and_then(|e| e.into_axum_event()));
    Sse::new(axum_stream).keep_alive(KeepAlive::default())
}

/// 创建 SSE 响应，自定义 KeepAlive 间隔
pub fn sse_response_with_interval<S>(stream: S, interval_secs: u64) -> impl IntoResponse
where
    S: Stream<Item = Result<SseEvent, Infallible>> + Send + 'static,
{
    let axum_stream = stream.map(|item| item.and_then(|e| e.into_axum_event()));
    Sse::new(axum_stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(interval_secs))
            .text("keep-alive"),
    )
}

/// 从 Vec 创建有限 SSE 流（发送完所有事件后关闭）
pub fn sse_from_events(events: Vec<SseEvent>) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    futures::stream::iter(events.into_iter().map(Ok))
}

// ============================================================================
// SSE 重连恢复 + 事件过滤 + 背压（spec §5.22，v1.7.0 新增）
// ============================================================================

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// SSE 事件 ID（单调递增，spec §5.22 规则 4）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub struct EventId(pub u64);

impl EventId {
    /// 递增到下一个 ID
    pub fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// 带单调递增 ID 的 SSE 事件（spec §5.22 规则 4）
#[derive(Debug, Clone, serde::Serialize)]
pub struct SseEventWithId {
    /// 事件 ID（单调递增）
    pub id: EventId,
    /// 事件类型（None 表示默认 message 事件）
    pub event: Option<String>,
    /// 事件数据
    pub data: String,
    /// 创建时间
    #[serde(skip)]
    pub created_at: Instant,
}

impl SseEventWithId {
    /// 创建新事件
    pub fn new(id: EventId, data: impl Into<String>) -> Self {
        Self {
            id,
            event: None,
            data: data.into(),
            created_at: Instant::now(),
        }
    }

    /// 设置事件类型
    pub fn with_event(mut self, event: impl Into<String>) -> Self {
        self.event = Some(event.into());
        self
    }
}

/// 事件过滤器（spec §5.22 规则 2）
///
/// 客户端订阅时指定感兴趣的事件类型，仅推送匹配事件。
#[derive(Debug, Clone)]
pub struct EventFilter {
    /// 订阅的事件类型集合
    pub subscribed_types: HashSet<String>,
}

impl EventFilter {
    /// 创建空过滤器（不过滤任何事件）
    pub fn new() -> Self {
        Self {
            subscribed_types: HashSet::new(),
        }
    }

    /// 订阅指定事件类型
    pub fn subscribe(mut self, event_type: impl Into<String>) -> Self {
        self.subscribed_types.insert(event_type.into());
        self
    }

    /// 订阅多个事件类型
    pub fn subscribe_many(mut self, types: impl IntoIterator<Item = String>) -> Self {
        self.subscribed_types.extend(types);
        self
    }

    /// 检查事件是否匹配过滤器
    ///
    /// - 事件有类型 → 检查是否在订阅集合中
    /// - 事件无类型 → 总是匹配（默认 message 事件）
    pub fn matches(&self, event: &SseEventWithId) -> bool {
        match &event.event {
            Some(t) => self.subscribed_types.contains(t),
            None => true,
        }
    }
}

impl Default for EventFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// SSE 重连恢复（spec §5.22 规则 3）
///
/// 保留事件历史，客户端重连时从 Last-Event-ID 之后继续推送。
pub struct SseRecovery {
    /// 事件历史（EventId → 事件）
    history: parking_lot::RwLock<HashMap<EventId, SseEventWithId>>,
    /// 历史保留时长
    retention: Duration,
    /// 下一个事件 ID
    next_id: parking_lot::Mutex<EventId>,
}

impl SseRecovery {
    /// 创建重连恢复器
    ///
    /// # 参数
    /// - `retention`: 历史事件保留时长（超时自动清理）
    pub fn new(retention: Duration) -> Self {
        Self {
            history: parking_lot::RwLock::new(HashMap::new()),
            retention,
            next_id: parking_lot::Mutex::new(EventId(1)),
        }
    }

    /// 分配下一个事件 ID（单调递增）
    pub fn allocate_id(&self) -> EventId {
        let mut id = self.next_id.lock();
        let current = *id;
        *id = id.next();
        current
    }

    /// 记算下一个事件 ID（不分配）
    pub fn peek_next_id(&self) -> EventId {
        *self.next_id.lock()
    }

    /// 记算下一个事件 ID（从当前值递增）
    pub fn next_id_from(&self, current: EventId) -> EventId {
        current.next()
    }

    /// 存储事件到历史
    pub fn store(&self, event: SseEventWithId) {
        let mut history = self.history.write();
        history.insert(event.id, event);
    }

    /// 从 Last-Event-ID 之后恢复（spec §5.22 规则 3）
    ///
    /// # 后置条件
    /// - 有 Last-Event-ID → 返回该 ID 之后的所有未过期事件（按 ID 排序）
    /// - 无 Last-Event-ID → 返回空 Vec（从最新事件开始）
    pub fn recover_from(&self, last_event_id: Option<EventId>) -> Vec<SseEventWithId> {
        let history = self.history.read();
        let now = Instant::now();
        let threshold = match last_event_id {
            Some(id) => id,
            None => return Vec::new(),
        };
        let mut events: Vec<SseEventWithId> = history
            .iter()
            .filter(|(id, e)| **id > threshold && now.duration_since(e.created_at) < self.retention)
            .map(|(_, e)| e.clone())
            .collect();
        events.sort_by_key(|e| e.id);
        events
    }

    /// 清理过期历史事件
    pub fn purge_expired(&self) -> usize {
        let mut history = self.history.write();
        let now = Instant::now();
        let before = history.len();
        history.retain(|_, e| now.duration_since(e.created_at) < self.retention);
        before - history.len()
    }

    /// 历史事件数量
    pub fn history_count(&self) -> usize {
        self.history.read().len()
    }
}

/// 背压缓冲（spec §5.22 规则 4，§6.22 规则 3）
///
/// 客户端消费慢时背压限速，缓冲有上限不无限缓冲。
/// 缓冲满时 `try_send` 返回错误，生产者应降级或丢弃。
pub struct BackpressureBuffer {
    sender: mpsc::Sender<SseEventWithId>,
    receiver: mpsc::Receiver<SseEventWithId>,
    /// 缓冲上限
    max_buffer: usize,
}

impl BackpressureBuffer {
    /// 创建背压缓冲
    ///
    /// # 参数
    /// - `max_buffer`: 缓冲上限（spec §6.22 规则 3）
    pub fn new(max_buffer: usize) -> Self {
        let (sender, receiver) = mpsc::channel(max_buffer);
        Self {
            sender,
            receiver,
            max_buffer,
        }
    }

    /// 尝试发送事件（不阻塞）
    ///
    /// # 返回
    /// - `Ok(())` — 发送成功
    /// - `Err(event)` — 缓冲已满，返回事件供调用者降级处理
    pub fn try_send(&self, event: SseEventWithId) -> Result<(), SseEventWithId> {
        self.sender.try_send(event).map_err(|e| e.into_inner())
    }

    /// 异步发送事件（阻塞等待缓冲有空间）
    pub async fn send(
        &self,
        event: SseEventWithId,
    ) -> Result<(), mpsc::error::SendError<SseEventWithId>> {
        self.sender.send(event).await
    }

    /// 接收下一个事件
    pub async fn recv(&mut self) -> Option<SseEventWithId> {
        self.receiver.recv().await
    }

    /// 缓冲上限
    pub fn capacity(&self) -> usize {
        self.max_buffer
    }

    /// 当前缓冲使用量
    pub fn len(&self) -> usize {
        self.max_buffer - self.sender.capacity()
    }

    /// 缓冲是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_event_data() {
        let event = SseEvent::data("hello");
        assert_eq!(event.data, "hello");
        assert!(event.event.is_none());
        assert!(event.id.is_none());
        assert!(event.retry.is_none());
    }

    #[test]
    fn test_sse_event_builder() {
        let event = SseEvent::data("payload")
            .event("update")
            .id("123")
            .retry(5000);
        assert_eq!(event.data, "payload");
        assert_eq!(event.event.as_deref(), Some("update"));
        assert_eq!(event.id.as_deref(), Some("123"));
        assert_eq!(event.retry, Some(5000));
    }

    #[test]
    fn test_sse_event_to_axum() {
        let event = SseEvent::data("test").event("ping");
        let axum_event = event.into_axum_event();
        assert!(axum_event.is_ok());
    }

    #[test]
    fn test_sse_from_events() {
        let events = vec![SseEvent::data("first"), SseEvent::data("second")];
        let stream = sse_from_events(events);
        let collected: Vec<_> = futures::executor::block_on(stream.collect());
        assert_eq!(collected.len(), 2);
    }

    // ===== v1.7.0 SSE 重连恢复测试 =====

    #[test]
    fn test_event_id_monotonic() {
        let id = EventId(1);
        assert_eq!(id.next(), EventId(2));
        assert_eq!(id.next().next(), EventId(3));
    }

    #[test]
    fn test_sse_event_with_id() {
        let event = SseEventWithId::new(EventId(1), "hello").with_event("greeting");
        assert_eq!(event.id, EventId(1));
        assert_eq!(event.event.as_deref(), Some("greeting"));
        assert_eq!(event.data, "hello");
    }

    #[test]
    fn test_event_filter_match() {
        let filter = EventFilter::new().subscribe("update").subscribe("delete");
        let e1 = SseEventWithId::new(EventId(1), "d1").with_event("update");
        let e2 = SseEventWithId::new(EventId(2), "d2").with_event("create");
        let e3 = SseEventWithId::new(EventId(3), "d3");
        assert!(filter.matches(&e1));
        assert!(!filter.matches(&e2));
        assert!(filter.matches(&e3));
    }

    #[test]
    fn test_event_filter_many() {
        let filter = EventFilter::new().subscribe_many(vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
        ]);
        let e = SseEventWithId::new(EventId(1), "x").with_event("b");
        assert!(filter.matches(&e));
    }

    #[test]
    fn test_sse_recovery_recover_from() {
        let recovery = SseRecovery::new(Duration::from_secs(60));
        for i in 1..=5 {
            let id = recovery.allocate_id();
            assert_eq!(id, EventId(i as u64));
            let event = SseEventWithId::new(id, format!("data{i}"));
            recovery.store(event);
        }
        assert_eq!(recovery.history_count(), 5);
        let recovered = recovery.recover_from(Some(EventId(2)));
        assert_eq!(recovered.len(), 3);
        assert_eq!(recovered[0].id, EventId(3));
        assert_eq!(recovered[1].id, EventId(4));
        assert_eq!(recovered[2].id, EventId(5));
    }

    #[test]
    fn test_sse_recovery_no_last_event_id() {
        let recovery = SseRecovery::new(Duration::from_secs(60));
        let id = recovery.allocate_id();
        recovery.store(SseEventWithId::new(id, "data"));
        let recovered = recovery.recover_from(None);
        assert!(recovered.is_empty());
    }

    #[test]
    fn test_sse_recovery_purge_expired() {
        let recovery = SseRecovery::new(Duration::from_millis(1));
        let id = recovery.allocate_id();
        recovery.store(SseEventWithId::new(id, "data"));
        std::thread::sleep(Duration::from_millis(10));
        let purged = recovery.purge_expired();
        assert_eq!(purged, 1);
        assert_eq!(recovery.history_count(), 0);
    }

    #[tokio::test]
    async fn test_backpressure_buffer_send_recv() {
        let mut buf = BackpressureBuffer::new(2);
        let e1 = SseEventWithId::new(EventId(1), "d1");
        let e2 = SseEventWithId::new(EventId(2), "d2");
        buf.send(e1).await.unwrap();
        buf.send(e2).await.unwrap();
        let r1 = buf.recv().await.unwrap();
        let r2 = buf.recv().await.unwrap();
        assert_eq!(r1.id, EventId(1));
        assert_eq!(r2.id, EventId(2));
    }

    #[test]
    fn test_backpressure_buffer_full() {
        let buf = BackpressureBuffer::new(1);
        let e1 = SseEventWithId::new(EventId(1), "d1");
        let e2 = SseEventWithId::new(EventId(2), "d2");
        assert!(buf.try_send(e1).is_ok());
        assert!(buf.try_send(e2).is_err());
    }

    #[test]
    fn test_backpressure_capacity() {
        let buf = BackpressureBuffer::new(8);
        assert_eq!(buf.capacity(), 8);
    }
}
