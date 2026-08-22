use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::Arc;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::{
    EmbeddingProvider, EmbeddingRequest, EmbeddingResult, SimilarityMetric, VectorHit,
    VectorRecord, VectorStore,
};
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    Usage,
};
use sz_rust_ai_facade::rag::pipeline::{RagPipeline, RagRequest, RagResult};

struct MockEmbedding;
#[async_trait]
impl EmbeddingProvider for MockEmbedding {
    fn name(&self) -> &str {
        "mock-embedding"
    }
    async fn embed(&self, req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
        let embeddings: Vec<Vec<f32>> = req
            .input
            .iter()
            .map(|text| {
                let hash = text.len() as f32;
                vec![hash / 100.0, 0.5, 0.3]
            })
            .collect();
        let usage = embeddings.len() as u32;
        Ok(EmbeddingResult {
            model: req.model,
            dimensions: 3,
            embeddings,
            usage_tokens: usage,
        })
    }
    fn dimensions(&self) -> usize {
        3
    }
    fn supported_models(&self) -> &[&str] {
        &["text-embedding-3-small"]
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
    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        let user_msg = req
            .messages
            .iter()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();
        Ok(ChatCompletion {
            id: "chatcmpl-mock".into(),
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: format!("Answer based on context: {}", user_msg).into(),
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
        Err(AiError::Internal("stream not supported in mock".into()))
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

fn make_hits() -> Vec<VectorHit> {
    vec![
        VectorHit {
            id: "doc1".into(),
            score: 0.95,
            metadata: serde_json::json!({"page": 1}),
            text: "Rust is a systems programming language".into(),
        },
        VectorHit {
            id: "doc2".into(),
            score: 0.85,
            metadata: serde_json::json!({"page": 2}),
            text: "Cargo is the Rust package manager".into(),
        },
        VectorHit {
            id: "doc3".into(),
            score: 0.75,
            metadata: serde_json::json!({"page": 3}),
            text: "Tokio is an async runtime".into(),
        },
    ]
}

fn make_pipeline(hits: Vec<VectorHit>) -> RagPipeline {
    RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits }),
        Arc::new(MockLlm),
    )
}

#[tokio::test]
async fn rag_three_stage_pipeline() {
    let pipeline = make_pipeline(make_hits());
    let req = RagRequest::new("What is Rust?", "tenant-1");
    let result = pipeline.rag(req).await.unwrap();
    assert!(!result.content.is_empty());
    assert!(result.content.contains("Answer based on context"));
}

#[tokio::test]
async fn rag_retrieve_returns_hits() {
    let pipeline = make_pipeline(make_hits());
    let hits = pipeline.retrieve("What is Rust?", 10).await.unwrap();
    assert_eq!(hits.len(), 3);
    assert!(hits[0].score >= hits[1].score);
}

#[tokio::test]
async fn rag_retrieve_respects_topk() {
    let pipeline = make_pipeline(make_hits());
    let hits = pipeline.retrieve("query", 2).await.unwrap();
    assert_eq!(hits.len(), 2);
}

#[tokio::test]
async fn rag_assemble_builds_context() {
    let pipeline = make_pipeline(make_hits());
    let hits = make_hits();
    let context = pipeline.assemble(&hits, 4096).await.unwrap();
    assert!(context.contains("[1]"));
    assert!(context.contains("Rust is a systems programming language"));
    assert!(context.contains("[2]"));
}

#[tokio::test]
async fn rag_assemble_truncates_on_budget() {
    let pipeline = make_pipeline(make_hits());
    let hits = make_hits();
    let context = pipeline.assemble(&hits, 1).await.unwrap();
    let total_chars: usize = context.chars().count();
    assert!(total_chars <= 4);
}

#[tokio::test]
async fn rag_generate_produces_answer() {
    let hits = make_hits();
    let pipeline = make_pipeline(hits.clone());
    let result = pipeline
        .generate(&hits, "some context", "What is Rust?")
        .await
        .unwrap();
    assert!(result.content.contains("Answer based on context"));
    assert_eq!(result.citations.len(), hits.len());
}

#[tokio::test]
async fn rag_empty_retrieval_still_works() {
    let pipeline = make_pipeline(vec![]);
    let req = RagRequest::new("query", "tenant-1");
    let result = pipeline.rag(req).await.unwrap();
    assert!(!result.content.is_empty());
}

#[tokio::test]
async fn rag_with_custom_models() {
    let pipeline = make_pipeline(make_hits())
        .with_embedding_model("custom-embed")
        .with_llm_model("custom-llm")
        .with_metric(SimilarityMetric::Dot);
    let req = RagRequest::new("query", "tenant-1");
    let result = pipeline.rag(req).await.unwrap();
    assert!(!result.content.is_empty());
}

#[tokio::test]
async fn rag_request_default_values() {
    let req = RagRequest::new("query", "tenant-1");
    assert_eq!(req.topk, 10);
    assert_eq!(req.token_budget, 4096);
    assert_eq!(req.tenant_id, "tenant-1");
}

#[tokio::test]
async fn rag_result_has_citations_field() {
    let pipeline = make_pipeline(make_hits());
    let req = RagRequest::new("query", "tenant-1");
    let result: RagResult = pipeline.rag(req).await.unwrap();
    assert_eq!(result.citations.len(), 3);
    assert!(result.warnings.is_empty());
}
