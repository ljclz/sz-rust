// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T1.4 多 Provider 链式故障切换端到端测试

mod common;

use common::providers::{FailingProvider, StubProvider};
use std::sync::Arc;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::llm::failover::ProviderFailover;
use sz_rust_ai_facade::llm::provider::LlmProvider;

#[tokio::test]
async fn it_failover_openai_to_claude_to_gemini() {
    let fo = ProviderFailover::new(2, 100);
    let openai = Arc::new(FailingProvider::new("openai", "down"));
    let claude = Arc::new(FailingProvider::new("claude", "down"));
    let gemini = Arc::new(StubProvider::new("gemini"));

    let result: Result<String, AiError> = fo
        .call_with_failover_chain(&["openai", "claude", "gemini"], |name| {
            let providers: Vec<Arc<dyn LlmProvider>> = vec![
                openai.clone() as Arc<dyn LlmProvider>,
                claude.clone() as Arc<dyn LlmProvider>,
                gemini.clone() as Arc<dyn LlmProvider>,
            ];
            let p = providers
                .iter()
                .find(|p| p.name() == name)
                .cloned()
                .unwrap();
            async move {
                let req = sz_rust_ai_facade::llm::provider::ChatRequest::new(
                    "model",
                    vec![sz_rust_ai_facade::llm::provider::ChatMessage {
                        role: sz_rust_ai_facade::llm::provider::Role::User,
                        content: "hi".into(),
                        tool_call_id: None,
                        tool_calls: None,
                    }],
                );
                let result = p.chat_completion(req).await?;
                Ok(result
                    .choices
                    .into_iter()
                    .next()
                    .unwrap()
                    .message
                    .content
                    .to_string())
            }
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Standardized response");
    assert_eq!(openai.fail_count(), 1);
    assert_eq!(claude.fail_count(), 1);
}

#[tokio::test]
async fn it_failover_all_providers_fail() {
    let fo = ProviderFailover::new(2, 100);
    let openai = Arc::new(FailingProvider::new("openai", "down"));
    let claude = Arc::new(FailingProvider::new("claude", "down"));

    let result: Result<String, AiError> = fo
        .call_with_failover_chain(&["openai", "claude"], |name| {
            let providers: Vec<Arc<dyn LlmProvider>> = vec![
                openai.clone() as Arc<dyn LlmProvider>,
                claude.clone() as Arc<dyn LlmProvider>,
            ];
            let p = providers
                .iter()
                .find(|p| p.name() == name)
                .cloned()
                .unwrap();
            async move {
                let req = sz_rust_ai_facade::llm::provider::ChatRequest::new(
                    "model",
                    vec![sz_rust_ai_facade::llm::provider::ChatMessage {
                        role: sz_rust_ai_facade::llm::provider::Role::User,
                        content: "hi".into(),
                        tool_call_id: None,
                        tool_calls: None,
                    }],
                );
                let result = p.chat_completion(req).await?;
                Ok(result
                    .choices
                    .into_iter()
                    .next()
                    .unwrap()
                    .message
                    .content
                    .to_string())
            }
        })
        .await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().error_code(), "AI_PROVIDER_UNAVAILABLE");
}

#[tokio::test]
async fn it_failover_state_transitions() {
    let fo = ProviderFailover::new(2, 100);
    assert_eq!(fo.state("openai"), "unknown");

    fo.record_failure("openai");
    assert_eq!(fo.state("openai"), "degraded");

    fo.record_failure("openai");
    assert_eq!(fo.state("openai"), "cooldown");

    fo.record_success("openai");
    assert_eq!(fo.state("openai"), "available");
}
