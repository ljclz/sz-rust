use crate::common::AiError;
use crate::llm::provider::{FinishReason, StreamDelta};
use futures::stream::{Stream, StreamExt};
use sz_rust_http_facade::sse::SseEvent;

pub struct SseAdapter;

impl SseAdapter {
    pub fn adapt<S>(
        stream: S,
    ) -> impl Stream<Item = Result<SseEvent, std::convert::Infallible>> + Send + 'static
    where
        S: Stream<Item = Result<StreamDelta, AiError>> + Send + 'static,
    {
        stream.filter_map(|item| async move {
            match item {
                Ok(delta) => Some(Ok(Self::delta_to_event(delta))),
                Err(_) => None,
            }
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
