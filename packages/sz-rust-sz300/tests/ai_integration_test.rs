//! AI chat 端到端测试 — 验证 Ai facade 初始化 + chat 调用链
//!
//! 测试覆盖：
//! 1. Ai::init_default() + Ai::is_initialized() — 初始化路径
//! 2. Ai::chat() 正常调用 — StubProvider 返回固定响应
//! 3. Ai::chat() 未知模型 — 路由失败错误
//! 4. Ai::chat() 空消息 — 仍可调用（Provider 决定行为）
//!
//! 注意：Ai 是全局单例（OnceLock），init_default 只能调用一次。
//! 用 std::sync::Once 确保初始化只执行一次，多个测试共享同一初始化状态。

use async_trait::async_trait;
use futures::stream::BoxStream;
use std::collections::HashMap;
use std::sync::{Arc, Once};
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, ContentPart, FinishReason, LlmProvider, Role,
    StreamDelta, Usage,
};
use sz_rust_ai_facade::llm::{provider::ProviderRef, ModelRouter};

/// Stub Provider — 返回固定响应（与 ai-facade 测试中的 StubProvider 对齐）
struct StubProvider;

#[async_trait]
impl LlmProvider for StubProvider {
    fn name(&self) -> &str {
        "stub"
    }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        Ok(ChatCompletion {
            id: "chatcmpl-stub".to_string(),
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: ContentPart::Text("Stub response".into()),
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
        Err(AiError::Internal("stub does not support stream".into()))
    }

    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages
            .iter()
            .map(|m| m.content.text_or_empty().len() as u32)
            .sum())
    }

    fn supported_models(&self) -> &[&str] {
        &["gpt-4o-mini", "gpt-4o", "gpt-4.1-mini"]
    }
}

static INIT: Once = Once::new();

fn init_ai_once() {
    INIT.call_once(|| {
        let provider = Arc::new(StubProvider) as ProviderRef;
        let mut routes = HashMap::new();
        routes.insert("gpt-4o-mini".to_string(), provider.clone());
        routes.insert("gpt-4o".to_string(), provider.clone());
        routes.insert("gpt-4.1-mini".to_string(), provider.clone());

        let router = ModelRouter::new(routes, "gpt-4o-mini".to_string());
        sz_rust_ai_facade::Ai::init_default(router, None, None, None, None)
            .expect("Ai::init_default 应成功");
    });
}

#[tokio::test]
async fn ai_is_initialized_after_init() {
    init_ai_once();
    assert!(
        sz_rust_ai_facade::Ai::is_initialized(),
        "Ai::is_initialized() 应返回 true"
    );
}

#[tokio::test]
async fn ai_chat_returns_response_with_stub_provider() {
    init_ai_once();

    let req = ChatRequest::new(
        "gpt-4o-mini",
        vec![ChatMessage {
            role: Role::User,
            content: "你好".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    );

    let result = sz_rust_ai_facade::Ai::chat(req).await;
    assert!(result.is_ok(), "Ai::chat 应成功: {:?}", result.err());

    let completion = result.unwrap();
    assert_eq!(completion.model, "gpt-4o-mini");
    assert!(!completion.choices.is_empty());
    assert_eq!(
        completion.choices[0].message.content.text_or_empty(),
        "Stub response"
    );
    assert_eq!(completion.usage.total_tokens, 30);
}

#[tokio::test]
async fn ai_chat_with_unknown_model_returns_error() {
    init_ai_once();

    let req = ChatRequest::new(
        "nonexistent-model",
        vec![ChatMessage {
            role: Role::User,
            content: "test".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    );

    let result = sz_rust_ai_facade::Ai::chat(req).await;
    assert!(result.is_err(), "未知模型应返回错误");
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("not found") || err.to_string().contains("nonexistent"),
        "错误信息应包含模型名: {err}"
    );
}

#[tokio::test]
async fn ai_chat_with_gpt4o_model_routes_correctly() {
    init_ai_once();

    let req = ChatRequest::new(
        "gpt-4o",
        vec![ChatMessage {
            role: Role::User,
            content: "用 gpt-4o 回答".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    );

    let result = sz_rust_ai_facade::Ai::chat(req).await;
    assert!(result.is_ok(), "gpt-4o 应路由成功: {:?}", result.err());

    let completion = result.unwrap();
    assert_eq!(completion.model, "gpt-4o");
}

#[tokio::test]
async fn ai_chat_with_gpt41_mini_model_routes_correctly() {
    init_ai_once();

    let req = ChatRequest::new(
        "gpt-4.1-mini",
        vec![ChatMessage {
            role: Role::User,
            content: "用 gpt-4.1-mini 回答".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    );

    let result = sz_rust_ai_facade::Ai::chat(req).await;
    assert!(
        result.is_ok(),
        "gpt-4.1-mini 应路由成功: {:?}",
        result.err()
    );

    let completion = result.unwrap();
    assert_eq!(completion.model, "gpt-4.1-mini");
}

#[tokio::test]
async fn ai_default_model_returns_gpt4o_mini() {
    init_ai_once();

    let default = sz_rust_ai_facade::Ai::default_model();
    assert!(default.is_ok(), "default_model 应成功: {:?}", default.err());
    assert_eq!(default.unwrap(), "gpt-4o-mini");
}
