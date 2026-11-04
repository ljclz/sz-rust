// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::collections::HashMap;
use std::sync::Arc;
use sz_rust_ai_facade::agent::tool::ToolRegistry;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::{
    EmbeddingProvider, EmbeddingRequest, EmbeddingResult, SimilarityMetric, VectorHit,
    VectorRecord, VectorStore,
};
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    Usage,
};
use sz_rust_ai_facade::llm::router::ModelRouter;
use sz_rust_ai_facade::rag::pipeline::{RagPipeline, RagRequest};
use sz_rust_ai_facade::Ai;

struct MockLlm;
#[async_trait]
impl LlmProvider for MockLlm {
    fn name(&self) -> &str {
        "mock"
    }
    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        Ok(ChatCompletion {
            id: "chatcmpl-e2e".into(),
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "E2E response".into(),
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
    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        Err(AiError::Internal("stream not supported in e2e mock".into()))
    }
    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages
            .iter()
            .map(|m| m.content.text_or_empty().len() as u32)
            .sum())
    }
    fn supported_models(&self) -> &[&str] {
        &["gpt-4o"]
    }
}

struct MockEmbedding;
#[async_trait]
impl EmbeddingProvider for MockEmbedding {
    fn name(&self) -> &str {
        "mock-embed"
    }
    async fn embed(&self, req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
        let n = req.input.len();
        Ok(EmbeddingResult {
            model: req.model,
            embeddings: vec![vec![0.1, 0.2, 0.3]; n],
            dimensions: 3,
            usage_tokens: n as u32,
        })
    }
    fn dimensions(&self) -> usize {
        3
    }
    fn supported_models(&self) -> &[&str] {
        &["text-embedding-3-small"]
    }
}

struct MockVectorStore;
#[async_trait]
impl VectorStore for MockVectorStore {
    async fn upsert(&self, _records: &[VectorRecord]) -> Result<(), AiError> {
        Ok(())
    }
    async fn query(
        &self,
        _vec: &[f32],
        _topk: usize,
        _metric: SimilarityMetric,
        _tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError> {
        Ok(vec![VectorHit {
            id: "doc1".into(),
            score: 0.9,
            metadata: serde_json::Value::Null,
            text: "mock context".into(),
        }])
    }
    async fn delete(&self, _ids: &[&str], _tenant: &str) -> Result<(), AiError> {
        Ok(())
    }
}

fn init_facade() {
    if Ai::is_initialized() {
        return;
    }
    let mut routes = HashMap::new();
    routes.insert(
        "gpt-4o".to_string(),
        Arc::new(MockLlm) as sz_rust_ai_facade::llm::provider::ProviderRef,
    );
    let router = ModelRouter::new(routes, "gpt-4o".to_string());
    let embedding = Arc::new(MockEmbedding) as Arc<dyn EmbeddingProvider>;
    let vector_store = Arc::new(MockVectorStore) as Arc<dyn VectorStore>;
    let rag = Arc::new(RagPipeline::new(
        embedding.clone(),
        vector_store.clone(),
        Arc::new(MockLlm) as Arc<dyn LlmProvider>,
    ));
    let tools = Arc::new(ToolRegistry::new());
    let _ = Ai::init_default(
        router,
        Some(embedding),
        Some(vector_store),
        Some(rag),
        Some(tools),
    );
}

#[tokio::test]
async fn facade_chat_e2e() {
    init_facade();
    let req = ChatRequest::new(
        "gpt-4o",
        vec![ChatMessage {
            role: Role::User,
            content: "Hello".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    );
    let result = Ai::chat(req).await.unwrap();
    assert_eq!(
        result.choices[0].message.content.as_text(),
        Some("E2E response")
    );
}

#[tokio::test]
async fn facade_embed_e2e() {
    init_facade();
    let result = Ai::embed(
        vec!["hello".into(), "world".into()],
        "text-embedding-3-small",
    )
    .await
    .unwrap();
    assert_eq!(result.embeddings.len(), 2);
    assert_eq!(result.dimensions, 3);
}

#[tokio::test]
async fn facade_rag_e2e() {
    init_facade();
    let req = RagRequest::new("What is Rust?", "tenant-1");
    let result = Ai::rag(req).await.unwrap();
    assert!(!result.content.is_empty());
}

#[tokio::test]
async fn facade_default_model_e2e() {
    init_facade();
    let model = Ai::default_model().unwrap();
    assert_eq!(model, "gpt-4o");
}

#[tokio::test]
async fn facade_is_initialized_e2e() {
    init_facade();
    assert!(Ai::is_initialized());
}

#[tokio::test]
async fn facade_agent_e2e() {
    init_facade();
    use sz_rust_ai_facade::agent::engine::{AgentOptions, AgentTask};
    let task = AgentTask::new("Say hello");
    let opts = AgentOptions::new("tenant-1");
    let result = Ai::agent(task, opts).await.unwrap();
    assert!(!result.final_answer.is_empty());
}
