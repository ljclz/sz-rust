//! T1.7 流式 SSE 透传端到端测试

mod common;

use common::providers::StreamingProvider;
use futures::StreamExt;
use sz_rust_ai_facade::llm::provider::{ChatMessage, ChatRequest, LlmProvider};

#[tokio::test]
async fn it_stream_sse_token_order_preserved() {
    let provider = StreamingProvider::new("test", vec!["Hello".into(), " ".into(), "World".into()]);
    let req = ChatRequest::new(
        "stream-model",
        vec![ChatMessage {
            role: sz_rust_ai_facade::llm::provider::Role::User,
            content: "say hello".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    );

    let mut stream = provider.stream_completion(req).await.unwrap();
    let mut collected = String::new();
    let mut event_count = 0u32;

    while let Some(delta) = stream.next().await {
        let delta = delta.unwrap();
        if !delta.content_delta.is_empty() {
            collected.push_str(&delta.content_delta);
            event_count += 1;
        }
    }

    assert_eq!(event_count, 3);
    assert_eq!(collected, "Hello World");
}
