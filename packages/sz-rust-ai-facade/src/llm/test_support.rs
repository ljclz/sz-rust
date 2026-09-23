// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 测试辅助工具：提供 `MockProvider` 供 doctest 与单元测试复用。

use crate::common::AiError;
use crate::llm::provider::{ChatCompletion, ChatMessage, ChatRequest, LlmProvider, StreamDelta};
use async_trait::async_trait;
use futures::stream::BoxStream;

/// 空实现的 Mock Provider，用于测试与文档示例。
pub struct MockProvider;

#[async_trait]
impl LlmProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
        Err(AiError::Internal("mock".into()))
    }

    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        Err(AiError::Internal("mock".into()))
    }

    async fn token_count(&self, _messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(0)
    }

    fn supported_models(&self) -> &[&str] {
        &[]
    }
}
