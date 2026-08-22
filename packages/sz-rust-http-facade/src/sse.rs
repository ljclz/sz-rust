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
}
