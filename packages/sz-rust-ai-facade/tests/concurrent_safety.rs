//! T1.8 并发安全端到端测试

mod common;

use common::providers::StubProvider;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use sz_rust_ai_facade::llm::provider::{ChatMessage, ChatRequest, LlmProvider, Role};

#[tokio::test]
async fn it_concurrent_facade_thread_safety() {
    let provider = Arc::new(StubProvider::new("concurrent-test"));
    let success_count = Arc::new(AtomicU32::new(0));
    let total_calls = 100u32;

    let mut handles = Vec::new();
    for _ in 0..total_calls {
        let p = provider.clone();
        let sc = success_count.clone();
        handles.push(tokio::spawn(async move {
            let req = ChatRequest::new(
                "stub-model",
                vec![ChatMessage {
                    role: Role::User,
                    content: "concurrent".into(),
                    tool_call_id: None,
                    tool_calls: None,
                }],
            );
            let result = p.chat_completion(req).await;
            if result.is_ok() {
                sc.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(success_count.load(Ordering::SeqCst), total_calls);
}

#[tokio::test]
async fn it_concurrent_failover_thread_safety() {
    use sz_rust_ai_facade::llm::failover::ProviderFailover;

    let fo = Arc::new(ProviderFailover::new(3, 5000));
    let success_count = Arc::new(AtomicU32::new(0));
    let total_calls = 50u32;

    let mut handles = Vec::new();
    for _ in 0..total_calls {
        let fo_clone = fo.clone();
        let sc = success_count.clone();
        handles.push(tokio::spawn(async move {
            let result: Result<i32, sz_rust_ai_facade::common::AiError> = fo_clone
                .call_with_failover("openai", Some("claude"), |_| async {
                    Ok::<i32, sz_rust_ai_facade::common::AiError>(42)
                })
                .await;
            if result.is_ok() {
                sc.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(success_count.load(Ordering::SeqCst), total_calls);
}
