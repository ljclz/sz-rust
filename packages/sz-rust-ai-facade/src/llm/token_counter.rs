use crate::common::AiError;
use crate::llm::provider::ChatMessage;
use crate::llm::router::ModelRouter;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

pub struct TokenCounter {
    router: Arc<ModelRouter>,
    cache: Arc<RwLock<HashMap<u64, u32>>>,
}

impl TokenCounter {
    pub fn new(router: Arc<ModelRouter>) -> Self {
        Self {
            router,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn count(
        &self,
        messages: &[ChatMessage],
        model: Option<&str>,
    ) -> Result<u32, AiError> {
        let hash = Self::hash_messages(messages, model);
        {
            let cache = self.cache.read();
            if let Some(&count) = cache.get(&hash) {
                return Ok(count);
            }
        }

        let provider = self.router.route(model)?;
        let count = provider.token_count(messages).await?;

        {
            let mut cache = self.cache.write();
            cache.insert(hash, count);
        }

        Ok(count)
    }

    fn hash_messages(messages: &[ChatMessage], model: Option<&str>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for msg in messages {
            msg.content.hash(&mut hasher);
            std::mem::discriminant(&msg.role).hash(&mut hasher);
        }
        model.hash(&mut hasher);
        hasher.finish()
    }

    pub fn clear_cache(&self) {
        self.cache.write().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{
        ChatCompletion, ChatMessage, ChatRequest, LlmProvider, Role, StreamDelta,
    };
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct CountingProvider {
        call_count: Arc<AtomicU32>,
    }
    #[async_trait]
    impl LlmProvider for CountingProvider {
        fn name(&self) -> &str {
            "counting"
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
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(messages.iter().map(|m| m.content.len() as u32).sum())
        }
        fn supported_models(&self) -> &[&str] {
            &[]
        }
    }

    fn msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    fn make_counter() -> (TokenCounter, Arc<AtomicU32>) {
        let call_count = Arc::new(AtomicU32::new(0));
        let provider = CountingProvider {
            call_count: call_count.clone(),
        };
        let mut routes = std::collections::HashMap::new();
        routes.insert(
            "test-model".to_string(),
            Arc::new(provider) as crate::llm::provider::ProviderRef,
        );
        let router = ModelRouter::new(routes, "test-model".to_string());
        (TokenCounter::new(Arc::new(router)), call_count)
    }

    #[tokio::test]
    async fn count_caches_repeated_calls() {
        let (counter, call_count) = make_counter();
        let messages = vec![msg("hello"), msg("world")];
        let c1 = counter.count(&messages, Some("test-model")).await.unwrap();
        let c2 = counter.count(&messages, Some("test-model")).await.unwrap();
        assert_eq!(c1, c2);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn count_different_messages_no_cache() {
        let (counter, call_count) = make_counter();
        let m1 = vec![msg("hello")];
        let m2 = vec![msg("world")];
        counter.count(&m1, Some("test-model")).await.unwrap();
        counter.count(&m2, Some("test-model")).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn clear_cache_forces_recount() {
        let (counter, call_count) = make_counter();
        let messages = vec![msg("hello")];
        counter.count(&messages, Some("test-model")).await.unwrap();
        counter.clear_cache();
        counter.count(&messages, Some("test-model")).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }
}
