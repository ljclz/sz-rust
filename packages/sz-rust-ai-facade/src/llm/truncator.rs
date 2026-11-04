// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::common::AiError;
use crate::llm::provider::{ChatMessage, LlmProvider};

pub struct ContextTruncator {
    default_budget: u32,
}

impl ContextTruncator {
    pub fn new(default_budget: u32) -> Self {
        Self { default_budget }
    }

    pub async fn truncate(
        &self,
        messages: Vec<ChatMessage>,
        budget: Option<u32>,
        counter: &dyn LlmProvider,
    ) -> Result<(Vec<ChatMessage>, u32, u32), AiError> {
        let budget = budget.unwrap_or(self.default_budget);
        let before = counter.token_count(&messages).await?;

        if before <= budget {
            return Ok((messages, before, before));
        }

        let mut truncated = messages.clone();
        while !truncated.is_empty() {
            let count = counter.token_count(&truncated).await?;
            if count <= budget {
                let after = count;
                tracing::warn!(
                    target: "ai_truncator",
                    before_tokens = before,
                    after_tokens = after,
                    "AI_CONTEXT_TRUNCATED"
                );
                return Ok((truncated, before, after));
            }
            let system_count = truncated
                .iter()
                .position(|m| !matches!(m.role, crate::llm::provider::Role::System))
                .unwrap_or(0);
            if truncated.len() > system_count + 1 {
                truncated.remove(system_count);
            } else {
                break;
            }
        }

        let after = counter.token_count(&truncated).await?;
        Ok((truncated, before, after))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{ChatCompletion, ChatRequest, LlmProvider, Role, StreamDelta};
    use async_trait::async_trait;
    use futures::stream::BoxStream;

    struct FixedTokenCounter(u32);
    #[async_trait]
    impl LlmProvider for FixedTokenCounter {
        fn name(&self) -> &str {
            "fixed"
        }
        async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
            Err(AiError::Internal("not impl".into()))
        }
        async fn stream_completion(
            &self,
            _req: ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
            Err(AiError::Internal("not impl".into()))
        }
        async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
            Ok(self.0 * messages.len() as u32)
        }
        fn supported_models(&self) -> &[&str] {
            &[]
        }
    }

    fn msg(role: Role, content: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    #[tokio::test]
    async fn no_truncation_when_under_budget() {
        let truncator = ContextTruncator::new(100);
        let counter = FixedTokenCounter(10);
        let messages = vec![msg(Role::User, "hello"), msg(Role::Assistant, "hi")];
        let (result, before, after) = truncator.truncate(messages, None, &counter).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(before, 20);
        assert_eq!(after, 20);
    }

    #[tokio::test]
    async fn truncation_removes_oldest_non_system() {
        let truncator = ContextTruncator::new(15);
        let counter = FixedTokenCounter(10);
        let messages = vec![
            msg(Role::System, "sys"),
            msg(Role::User, "msg1"),
            msg(Role::User, "msg2"),
        ];
        let (result, before, after) = truncator.truncate(messages, None, &counter).await.unwrap();
        assert_eq!(before, 30);
        assert!(after < before);
        assert!(result.len() < 3);
        assert_eq!(result[0].role, Role::System);
    }

    #[tokio::test]
    async fn truncation_with_explicit_budget() {
        let truncator = ContextTruncator::new(1000);
        let counter = FixedTokenCounter(10);
        let messages = vec![
            msg(Role::User, "a"),
            msg(Role::User, "b"),
            msg(Role::User, "c"),
        ];
        let (result, before, after) = truncator
            .truncate(messages, Some(15), &counter)
            .await
            .unwrap();
        assert_eq!(before, 30);
        assert!(after < before);
        assert!(result.len() < 3);
    }
}
