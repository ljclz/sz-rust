//! T1.5 RAG 全链路引用溯源端到端测试

mod common;

use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::Arc;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::{
    EmbeddingProvider, EmbeddingRequest, EmbeddingResult, SimilarityMetric, VectorHit,
    VectorRecord, VectorStore,
};
use sz_rust_ai_facade::llm::provider::StreamDelta;
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, Usage,
};
use sz_rust_ai_facade::rag::pipeline::{RagPipeline, RagRequest};

struct MockEmbedding;

#[async_trait]
impl EmbeddingProvider for MockEmbedding {
    fn name(&self) -> &str {
        "mock-embedding"
    }
    async fn embed(&self, _req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
        Ok(EmbeddingResult {
            model: "mock".into(),
            embeddings: vec![vec![0.1, 0.2, 0.3]],
            dimensions: 3,
            usage_tokens: 1,
        })
    }
    fn dimensions(&self) -> usize {
        3
    }
    fn supported_models(&self) -> &[&str] {
        &["mock"]
    }
}

struct MockVectorStore {
    hits: Vec<VectorHit>,
}

#[async_trait]
impl VectorStore for MockVectorStore {
    async fn upsert(&self, _records: &[VectorRecord]) -> Result<(), AiError> {
        Ok(())
    }
    async fn query(
        &self,
        _vec: &[f32],
        topk: usize,
        _metric: SimilarityMetric,
        _tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError> {
        Ok(self.hits.iter().take(topk).cloned().collect())
    }
    async fn delete(&self, _ids: &[&str], _tenant: &str) -> Result<(), AiError> {
        Ok(())
    }
}

struct MockLlm;

#[async_trait]
impl LlmProvider for MockLlm {
    fn name(&self) -> &str {
        "mock-llm"
    }
    async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
        Ok(ChatCompletion {
            id: "mock".into(),
            model: "mock".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "Answer based on context".into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        })
    }
    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        Err(AiError::Internal("not supported".into()))
    }
    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages
            .iter()
            .map(|m| m.content.text_or_empty().len() as u32)
            .sum())
    }
    fn supported_models(&self) -> &[&str] {
        &["mock"]
    }
}

#[tokio::test]
async fn it_rag_full_chain_with_citations() {
    let hits = vec![
        VectorHit {
            id: "doc1".into(),
            score: 0.95,
            metadata: serde_json::json!({}),
            text: "Rust is safe".into(),
        },
        VectorHit {
            id: "doc2".into(),
            score: 0.85,
            metadata: serde_json::json!({}),
            text: "Cargo is fast".into(),
        },
        VectorHit {
            id: "doc3".into(),
            score: 0.75,
            metadata: serde_json::json!({}),
            text: "Tokio is async".into(),
        },
    ];

    let pipeline = RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: hits.clone() }),
        Arc::new(MockLlm),
    );

    let req = RagRequest::new("What is Rust?", "tenant-1");
    let result = pipeline.rag(req).await.unwrap();

    assert!(!result.content.is_empty());
    assert_eq!(result.citations.len(), 3);
    for (i, citation) in result.citations.iter().enumerate() {
        assert_eq!(citation.doc_id, hits[i].id);
        assert_eq!(citation.offset, i as u32);
        assert_eq!(citation.text, hits[i].text);
        assert!((citation.score - hits[i].score).abs() < 0.001);
    }
}
