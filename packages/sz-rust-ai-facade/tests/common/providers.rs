//! Mock Provider — 4 个测试用 LLM Provider 实现
//!
//! - `StubProvider`：返回固定响应
//! - `ScriptedProvider`：按序返回预设响应
//! - `FailingProvider`：返回指定错误，记录失败次数
//! - `StreamingProvider`：流式产出 token 序列

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, StreamDelta, Usage,
};

/// Stub Provider — 返回固定响应
pub struct StubProvider {
    name: String,
    models: Vec<&'static str>,
}

impl StubProvider {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            models: vec!["stub-model"],
        }
    }
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
                    role: sz_rust_ai_facade::llm::provider::Role::Assistant,
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
        Err(AiError::Internal("stub does not support stream".into()))
    }

    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages.iter().map(|m| m.content.len() as u32).sum())
    }

    fn supported_models(&self) -> &[&str] {
        &self.models
    }
}

/// Scripted Provider — 按序返回预设响应
pub struct ScriptedProvider {
    name: String,
    responses: Vec<ChatCompletion>,
    call_index: Arc<AtomicU32>,
}

impl ScriptedProvider {
    pub fn new(name: impl Into<String>, responses: Vec<ChatCompletion>) -> Self {
        Self {
            name: name.into(),
            responses,
            call_index: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn call_count(&self) -> u32 {
        self.call_index.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
        let idx = self.call_index.fetch_add(1, Ordering::SeqCst) as usize;
        if idx < self.responses.len() {
            Ok(self.responses[idx].clone())
        } else {
            Ok(ChatCompletion {
                id: "default".into(),
                model: "gpt-4o".into(),
                choices: vec![Choice {
                    index: 0,
                    message: ChatMessage {
                        role: sz_rust_ai_facade::llm::provider::Role::Assistant,
                        content: "Done".into(),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    finish_reason: Some(FinishReason::Stop),
                }],
                usage: Usage {
                    prompt_tokens: 5,
                    completion_tokens: 5,
                    total_tokens: 10,
                },
            })
        }
    }

    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        Err(AiError::Internal("scripted does not support stream".into()))
    }

    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages.iter().map(|m| m.content.len() as u32).sum())
    }

    fn supported_models(&self) -> &[&str] {
        &["scripted-model"]
    }
}

/// Failing Provider — 返回指定错误码，记录失败次数
pub struct FailingProvider {
    name: String,
    error_code: String,
    fail_count: Arc<AtomicU32>,
}

impl FailingProvider {
    pub fn new(name: impl Into<String>, error_code: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            error_code: error_code.into(),
            fail_count: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn fail_count(&self) -> u32 {
        self.fail_count.load(Ordering::SeqCst)
    }

    fn make_error(&self) -> AiError {
        AiError::ProviderUnavailable(self.error_code.clone())
    }
}

#[async_trait]
impl LlmProvider for FailingProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
        self.fail_count.fetch_add(1, Ordering::SeqCst);
        Err(self.make_error())
    }

    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        self.fail_count.fetch_add(1, Ordering::SeqCst);
        Err(self.make_error())
    }

    async fn token_count(&self, _messages: &[ChatMessage]) -> Result<u32, AiError> {
        self.fail_count.fetch_add(1, Ordering::SeqCst);
        Err(self.make_error())
    }

    fn supported_models(&self) -> &[&str] {
        &["failing-model"]
    }
}

/// Streaming Provider — 流式产出 token 序列
pub struct StreamingProvider {
    name: String,
    tokens: Vec<String>,
}

impl StreamingProvider {
    pub fn new(name: impl Into<String>, tokens: Vec<String>) -> Self {
        Self {
            name: name.into(),
            tokens,
        }
    }
}

#[async_trait]
impl LlmProvider for StreamingProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
        let content = self.tokens.join("");
        Ok(ChatCompletion {
            id: "stream-chatcmpl".into(),
            model: "stream-model".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: sz_rust_ai_facade::llm::provider::Role::Assistant,
                    content,
                    tool_call_id: None,
                    tool_calls: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: self.tokens.len() as u32,
                total_tokens: 10 + self.tokens.len() as u32,
            },
        })
    }

    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        let tokens = self.tokens.clone();
        let stream = futures::stream::iter(tokens.into_iter().map(|token| {
            Ok(StreamDelta {
                content_delta: token,
                finish_reason: None,
                tool_call_delta: None,
            })
        }))
        .chain(futures::stream::once(async {
            Ok(StreamDelta {
                content_delta: String::new(),
                finish_reason: Some(FinishReason::Stop),
                tool_call_delta: None,
            })
        }));
        Ok(stream.boxed())
    }

    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages.iter().map(|m| m.content.len() as u32).sum())
    }

    fn supported_models(&self) -> &[&str] {
        &["stream-model"]
    }
}
