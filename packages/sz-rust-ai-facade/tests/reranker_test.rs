//! 任务组 11.4：重排序前后顺序变化测试
//! 验证 Reranker trait 实现的重排序效果

mod common;

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
use sz_rust_ai_facade::rag::pipeline::{RagPipeline, RagRequest};
use sz_rust_ai_facade::rag::reranker::{NoopReranker, Reranker, WeightedReranker};

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
                    content: "Answer".into(),
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

fn make_hit(id: &str, score: f32, text: &str) -> VectorHit {
    VectorHit {
        id: id.into(),
        score,
        metadata: serde_json::json!({}),
        text: text.into(),
    }
}

#[tokio::test]
async fn noop_reranker_preserves_original_order() {
    let reranker = NoopReranker::new();
    let candidates = vec![
        make_hit("a", 0.9, "doc a"),
        make_hit("b", 0.8, "doc b"),
        make_hit("c", 0.7, "doc c"),
    ];

    let result = reranker
        .rerank("query", candidates.clone(), 3)
        .await
        .unwrap();

    assert_eq!(result.len(), 3);
    for (i, (orig, reranked)) in candidates.iter().zip(result.iter()).enumerate() {
        assert_eq!(orig.id, reranked.id, "order must be preserved at index {i}");
        assert!(
            (orig.score - reranked.score).abs() < 1e-6,
            "score must be preserved at index {i}"
        );
    }
}

#[tokio::test]
async fn weighted_reranker_changes_order_when_length_matters() {
    let reranker = WeightedReranker::new(0.3);
    let candidates = vec![
        make_hit("short-high-score", 0.95, "ab"),
        make_hit("long-low-score", 0.2, "abcdefghijklmnopqrstuvwxyz"),
    ];

    let original_ids: Vec<_> = candidates.iter().map(|h| h.id.clone()).collect();
    let result = reranker.rerank("query", candidates, 2).await.unwrap();
    let reranked_ids: Vec<_> = result.iter().map(|h| h.id.clone()).collect();

    assert_eq!(result.len(), 2);
    assert!(
        original_ids != reranked_ids,
        "weighted reranker with alpha=0.3 should reorder when long doc has low vec score"
    );
    assert!(
        result[0].score >= result[1].score,
        "results must be sorted by combined score descending"
    );
}

#[tokio::test]
async fn weighted_reranker_preserves_order_when_alpha_is_1() {
    let reranker = WeightedReranker::new(1.0);
    let candidates = vec![
        make_hit("a", 0.9, "short"),
        make_hit("b", 0.8, "longer text here"),
        make_hit("c", 0.7, "mid"),
    ];

    let result = reranker
        .rerank("query", candidates.clone(), 3)
        .await
        .unwrap();

    assert_eq!(result.len(), 3);
    assert_eq!(result[0].id, "a");
    assert_eq!(result[1].id, "b");
    assert_eq!(result[2].id, "c");
}

#[tokio::test]
async fn rag_pipeline_with_noop_reranker_matches_default_behavior() {
    let hits = vec![
        make_hit("doc1", 0.9, "First document"),
        make_hit("doc2", 0.8, "Second document"),
    ];

    let pipeline_default = RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: hits.clone() }),
        Arc::new(MockLlm),
    );

    let pipeline_noop = RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: hits.clone() }),
        Arc::new(MockLlm),
    )
    .with_reranker(Arc::new(NoopReranker::new()));

    let req = RagRequest::new("query", "tenant");
    let result_default = pipeline_default.rag(req.clone()).await.unwrap();
    let result_noop = pipeline_noop.rag(req).await.unwrap();

    assert_eq!(
        result_default.citations.len(),
        result_noop.citations.len(),
        "noop reranker should produce same citation count as default"
    );
    for (c_default, c_noop) in result_default
        .citations
        .iter()
        .zip(result_noop.citations.iter())
    {
        assert_eq!(c_default.doc_id, c_noop.doc_id);
    }
}

#[tokio::test]
async fn rag_pipeline_with_weighted_reranker_produces_valid_citations() {
    let hits = vec![
        make_hit("doc1", 0.9, "Short"),
        make_hit("doc2", 0.5, "A much longer document with more context"),
        make_hit("doc3", 0.7, "Medium length doc"),
    ];

    let pipeline = RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits }),
        Arc::new(MockLlm),
    )
    .with_reranker(Arc::new(WeightedReranker::new(0.5)));

    let req = RagRequest::new("query", "tenant");
    let result = pipeline.rag(req).await.unwrap();

    assert_eq!(result.citations.len(), 3);
    for i in 0..result.citations.len() - 1 {
        assert!(
            result.citations[i].score >= result.citations[i + 1].score,
            "citations must be sorted by reranked score descending"
        );
    }
}

#[tokio::test]
async fn retrieve_with_rerank_combines_retrieval_and_reranking() {
    let hits = vec![
        make_hit("doc1", 0.9, "a"),
        make_hit("doc2", 0.8, "bb"),
        make_hit("doc3", 0.7, "ccc"),
    ];

    let pipeline = RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits }),
        Arc::new(MockLlm),
    )
    .with_reranker(Arc::new(WeightedReranker::new(0.5)));

    let result = pipeline.retrieve_with_rerank("query", 3).await.unwrap();

    assert_eq!(result.len(), 3);
    for i in 0..result.len() - 1 {
        assert!(
            result[i].score >= result[i + 1].score,
            "retrieve_with_rerank results must be sorted descending"
        );
    }
}

#[tokio::test]
async fn reranker_with_empty_candidates_returns_empty() {
    let reranker = WeightedReranker::default();
    let result = reranker.rerank("query", vec![], 5).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn reranker_respects_topk_limit() {
    let reranker = NoopReranker::new();
    let candidates: Vec<VectorHit> = (0..10)
        .map(|i| make_hit(&format!("doc{i}"), 0.5, &format!("text{i}")))
        .collect();

    let result = reranker.rerank("query", candidates, 3).await.unwrap();
    assert_eq!(result.len(), 3);
}
