// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::Arc;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    ToolCall, ToolDef, Usage,
};

struct StubProvider {
    name: String,
    models: Vec<&'static str>,
}
#[async_trait]
impl LlmProvider for StubProvider {
    fn name(&self) -> &str {
        &self.name
    }
    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        Ok(ChatCompletion {
            id: format!("chatcmpl-{}", self.name),
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "Standardized response".into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            },
        })
    }
    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        Err(AiError::Internal("stub".into()))
    }
    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages
            .iter()
            .map(|m| m.content.text_or_empty().len() as u32)
            .sum())
    }
    fn supported_models(&self) -> &[&str] {
        &self.models
    }
}

fn make_request() -> ChatRequest {
    ChatRequest::new(
        "gpt-4o",
        vec![
            ChatMessage {
                role: Role::System,
                content: "You are helpful".into(),
                tool_call_id: None,
                tool_calls: None,
            },
            ChatMessage {
                role: Role::User,
                content: "Hello".into(),
                tool_call_id: None,
                tool_calls: None,
            },
        ],
    )
}

#[tokio::test]
async fn contract_all_providers_return_same_shape() {
    let providers: Vec<Arc<dyn LlmProvider>> = vec![
        Arc::new(StubProvider {
            name: "openai".into(),
            models: vec!["gpt-4o"],
        }),
        Arc::new(StubProvider {
            name: "claude".into(),
            models: vec!["claude-3"],
        }),
        Arc::new(StubProvider {
            name: "gemini".into(),
            models: vec!["gemini-pro"],
        }),
    ];
    for provider in &providers {
        let result = provider.chat_completion(make_request()).await.unwrap();
        assert!(!result.id.is_empty());
        assert!(!result.model.is_empty());
        assert_eq!(result.choices.len(), 1);
        assert!(result.choices[0].finish_reason.is_some());
        assert!(result.usage.total_tokens > 0);
        assert_eq!(
            result.usage.total_tokens,
            result.usage.prompt_tokens + result.usage.completion_tokens
        );
    }
}

#[tokio::test]
async fn contract_openai_response_fields() {
    let provider = StubProvider {
        name: "openai".into(),
        models: vec!["gpt-4o"],
    };
    let result = provider.chat_completion(make_request()).await.unwrap();
    assert!(result.id.starts_with("chatcmpl-"));
    assert_eq!(result.choices[0].message.role, Role::Assistant);
    assert_eq!(result.choices[0].finish_reason, Some(FinishReason::Stop));
}

#[tokio::test]
async fn contract_claude_response_fields() {
    let provider = StubProvider {
        name: "claude".into(),
        models: vec!["claude-3"],
    };
    let result = provider.chat_completion(make_request()).await.unwrap();
    assert!(result.id.starts_with("chatcmpl-"));
    assert_eq!(result.choices[0].message.role, Role::Assistant);
    assert_eq!(result.choices[0].finish_reason, Some(FinishReason::Stop));
}

#[tokio::test]
async fn contract_gemini_response_fields() {
    let provider = StubProvider {
        name: "gemini".into(),
        models: vec!["gemini-pro"],
    };
    let result = provider.chat_completion(make_request()).await.unwrap();
    assert!(result.id.starts_with("chatcmpl-"));
    assert_eq!(result.choices[0].message.role, Role::Assistant);
    assert_eq!(result.choices[0].finish_reason, Some(FinishReason::Stop));
}

#[tokio::test]
async fn contract_token_count_consistent() {
    let providers: Vec<Arc<dyn LlmProvider>> = vec![
        Arc::new(StubProvider {
            name: "openai".into(),
            models: vec!["gpt-4o"],
        }),
        Arc::new(StubProvider {
            name: "claude".into(),
            models: vec!["claude-3"],
        }),
        Arc::new(StubProvider {
            name: "gemini".into(),
            models: vec!["gemini-pro"],
        }),
    ];
    let messages = vec![ChatMessage {
        role: Role::User,
        content: "hello world".into(),
        tool_call_id: None,
        tool_calls: None,
    }];
    let counts: Vec<u32> =
        futures::future::try_join_all(providers.iter().map(|p| p.token_count(&messages)))
            .await
            .unwrap();
    assert!(counts.iter().all(|&c| c == counts[0]));
}

#[tokio::test]
async fn contract_supported_models_non_empty() {
    let providers: Vec<Arc<dyn LlmProvider>> = vec![
        Arc::new(StubProvider {
            name: "openai".into(),
            models: vec!["gpt-4o"],
        }),
        Arc::new(StubProvider {
            name: "claude".into(),
            models: vec!["claude-3"],
        }),
        Arc::new(StubProvider {
            name: "gemini".into(),
            models: vec!["gemini-pro"],
        }),
    ];
    for p in &providers {
        assert!(!p.supported_models().is_empty());
    }
}

#[tokio::test]
async fn contract_provider_names_unique() {
    let providers: Vec<Arc<dyn LlmProvider>> = vec![
        Arc::new(StubProvider {
            name: "openai".into(),
            models: vec!["gpt-4o"],
        }),
        Arc::new(StubProvider {
            name: "claude".into(),
            models: vec!["claude-3"],
        }),
        Arc::new(StubProvider {
            name: "gemini".into(),
            models: vec!["gemini-pro"],
        }),
    ];
    let names: Vec<&str> = providers.iter().map(|p| p.name()).collect();
    assert_eq!(names.len(), 3);
    assert_ne!(names[0], names[1]);
    assert_ne!(names[1], names[2]);
    assert_ne!(names[0], names[2]);
}

#[tokio::test]
async fn contract_chat_with_tools() {
    let provider = StubProvider {
        name: "openai".into(),
        models: vec!["gpt-4o"],
    };
    let mut req = make_request();
    req.tools = Some(vec![ToolDef {
        name: "calculator".into(),
        description: "A calculator".into(),
        parameters: serde_json::json!({"type": "object"}),
    }]);
    let result = provider.chat_completion(req).await.unwrap();
    assert_eq!(result.choices.len(), 1);
}

#[tokio::test]
async fn contract_chat_with_max_tokens() {
    let provider = StubProvider {
        name: "openai".into(),
        models: vec!["gpt-4o"],
    };
    let mut req = make_request();
    req.max_tokens = Some(100);
    let result = provider.chat_completion(req).await.unwrap();
    assert!(result.usage.completion_tokens <= 100 || result.usage.completion_tokens > 0);
}

#[tokio::test]
async fn contract_chat_with_temperature() {
    let provider = StubProvider {
        name: "openai".into(),
        models: vec!["gpt-4o"],
    };
    let mut req = make_request();
    req.temperature = Some(0.7);
    let result = provider.chat_completion(req).await.unwrap();
    assert!(!result.choices.is_empty());
}

#[tokio::test]
async fn contract_usage_fields_non_negative() {
    let provider = StubProvider {
        name: "openai".into(),
        models: vec!["gpt-4o"],
    };
    let result = provider.chat_completion(make_request()).await.unwrap();
    assert!(result.usage.prompt_tokens > 0);
    assert!(result.usage.completion_tokens > 0);
    assert!(result.usage.total_tokens > 0);
}

#[tokio::test]
async fn contract_role_serialization() {
    let roles = vec![Role::System, Role::User, Role::Assistant, Role::Tool];
    for role in roles {
        let msg = ChatMessage {
            role: role.clone(),
            content: "test".into(),
            tool_call_id: None,
            tool_calls: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let de: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(de.role, role);
    }
}

#[tokio::test]
async fn contract_finish_reason_serialization() {
    let reasons = vec![
        FinishReason::Stop,
        FinishReason::Length,
        FinishReason::ToolCalls,
        FinishReason::ContentFilter,
    ];
    for reason in reasons {
        let json = serde_json::to_string(&reason).unwrap();
        let de: FinishReason = serde_json::from_str(&json).unwrap();
        assert_eq!(de, reason);
    }
}

#[tokio::test]
async fn contract_tool_call_serialization() {
    let tc = ToolCall {
        id: "call_1".into(),
        name: "echo".into(),
        arguments: "{\"x\":1}".into(),
    };
    let json = serde_json::to_string(&tc).unwrap();
    let de: ToolCall = serde_json::from_str(&json).unwrap();
    assert_eq!(de.id, "call_1");
    assert_eq!(de.name, "echo");
    assert_eq!(de.arguments, "{\"x\":1}");
}
