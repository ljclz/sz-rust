// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::common::AiError;
use crate::llm::provider::{FinishReason, StreamDelta};
use futures::stream::{Stream, StreamExt};
use sz_rust_http_facade::sse::SseEvent;

/// SSE 错误类型（任务组 18：SSE 错误传播）
///
/// 包装 `AiError`，使 `SseAdapter::adapt` 能够传播错误而非静默丢弃。
#[derive(Debug, thiserror::Error)]
pub enum SseError {
    /// AI 层错误
    #[error("SSE stream error: {0}")]
    Ai(#[from] AiError),
}

pub struct SseAdapter;

impl SseAdapter {
    /// 将 LLM 流适配为 SSE 事件流
    ///
    /// 错误传播：`Err(AiError)` 转换为 `Err(SseError::Ai)`，不再静默丢弃。
    pub fn adapt<S>(stream: S) -> impl Stream<Item = Result<SseEvent, SseError>> + Send + 'static
    where
        S: Stream<Item = Result<StreamDelta, AiError>> + Send + 'static,
    {
        stream.map(|item| match item {
            Ok(delta) => Ok(Self::delta_to_event(delta)),
            Err(e) => Err(SseError::from(e)),
        })
    }

    fn delta_to_event(delta: StreamDelta) -> SseEvent {
        let mut event = SseEvent::data(&delta.content_delta);
        if let Some(reason) = delta.finish_reason {
            event = event.event(Self::finish_reason_to_str(reason));
        }
        event
    }

    fn finish_reason_to_str(reason: FinishReason) -> &'static str {
        match reason {
            FinishReason::Stop => "stop",
            FinishReason::Length => "length",
            FinishReason::ToolCalls => "tool_calls",
            FinishReason::ContentFilter => "content_filter",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::StreamDelta;
    use futures::stream;

    fn make_delta(content: &str) -> StreamDelta {
        StreamDelta {
            content_delta: content.to_string(),
            finish_reason: None,
            tool_call_delta: None,
        }
    }

    #[tokio::test]
    async fn sse_adapt_propagates_errors() {
        let error_stream = stream::iter(vec![Err(AiError::Internal("test error".to_string()))]);

        let mut adapted = SseAdapter::adapt(error_stream);
        let item = adapted.next().await;

        assert!(item.is_some(), "error should be propagated, not dropped");
        let result = item.unwrap();
        assert!(result.is_err(), "result should be Err");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("test error"),
            "error message should be preserved: {err}"
        );
    }

    #[tokio::test]
    async fn sse_adapt_preserves_ok_events() {
        let ok_stream = stream::iter(vec![Ok(make_delta("hello"))]);

        let mut adapted = SseAdapter::adapt(ok_stream);
        let item = adapted.next().await;

        assert!(item.is_some());
        let result = item.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn sse_adapt_mixed_ok_and_error() {
        let mixed_stream = stream::iter(vec![
            Ok(make_delta("data")),
            Err(AiError::Internal("mid-stream error".to_string())),
        ]);

        let mut adapted = SseAdapter::adapt(mixed_stream);
        let first = adapted.next().await.unwrap();
        let second = adapted.next().await.unwrap();

        assert!(first.is_ok(), "first item should be Ok");
        assert!(second.is_err(), "second item should be Err");
    }
}
