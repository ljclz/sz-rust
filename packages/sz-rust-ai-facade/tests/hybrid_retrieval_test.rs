// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 任务组 12.4：纯向量 vs 混合检索召回率对比测试
//! 验证混合检索召回率 >= 纯向量检索

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
use sz_rust_ai_facade::rag::bm25::Bm25Index;
use sz_rust_ai_facade::rag::hybrid::{HybridRetriever, RrfParams};
use sz_rust_ai_facade::rag::pipeline::{RagPipeline, RagRequest};
use sz_rust_ai_facade::rag::HybridRetrieverTrait;

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

fn compute_recall(retrieved: &[String], relevant: &[String]) -> f32 {
    if relevant.is_empty() {
        return 1.0;
    }
    let retrieved_set: std::collections::HashSet<&String> = retrieved.iter().collect();
    let hits = relevant
        .iter()
        .filter(|r| retrieved_set.contains(r))
        .count();
    hits as f32 / relevant.len() as f32
}

#[tokio::test]
async fn bm25_index_basic_search() {
    let mut index = Bm25Index::new();
    index.add_document("doc1", "Rust programming language memory safety");
    index.add_document("doc2", "Python scripting language");
    index.add_document("doc3", "Rust Cargo package manager");

    let hits = index.search("Rust", 3);
    assert!(!hits.is_empty());
    let hit_ids: Vec<&str> = hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(
        hit_ids.contains(&"doc1"),
        "doc1 contains Rust and should be retrieved"
    );
    assert!(
        hit_ids.contains(&"doc3"),
        "doc3 contains Rust and should be retrieved"
    );
    assert!(!hit_ids.contains(&"doc2"), "doc2 does not contain Rust");
}

#[tokio::test]
async fn hybrid_retriever_fuses_vector_and_keyword_results() {
    let vector_hits = vec![
        make_hit("vec1", 0.9, "vector semantic result"),
        make_hit("vec2", 0.8, "another vector result"),
    ];

    let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));
    {
        let mut index = bm25.write().await;
        index.add_document("kw1", "keyword exact match");
        index.add_document("kw2", "another keyword document");
    }

    let hybrid = Arc::new(HybridRetriever::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: vector_hits }),
        bm25,
    ));

    let result = hybrid.retrieve("keyword", 10, "tenant").await.unwrap();
    let ids: Vec<String> = result.iter().map(|h| h.id.clone()).collect();

    assert!(ids.contains(&"vec1".to_string()));
    assert!(ids.contains(&"vec2".to_string()));
    assert!(ids.contains(&"kw1".to_string()));
    assert!(ids.contains(&"kw2".to_string()));
}

#[tokio::test]
async fn hybrid_recall_gte_vector_recall() {
    let vector_hits = vec![
        make_hit("doc1", 0.9, "Rust memory safety"),
        make_hit("doc2", 0.8, "Python scripting"),
    ];

    let relevant: Vec<String> = vec![
        "doc1".into(),
        "doc2".into(),
        "kw_rust".into(),
        "kw_cargo".into(),
    ];

    let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));
    {
        let mut index = bm25.write().await;
        index.add_document("kw_rust", "Rust programming language");
        index.add_document("kw_cargo", "Cargo build system for Rust");
        index.add_document("kw_other", "unrelated content");
    }

    let vector_store = Arc::new(MockVectorStore {
        hits: vector_hits.clone(),
    });

    let pipeline_vector = RagPipeline::new(
        Arc::new(MockEmbedding),
        vector_store.clone(),
        Arc::new(MockLlm),
    );

    let hybrid = Arc::new(HybridRetriever::new(
        Arc::new(MockEmbedding),
        vector_store,
        bm25,
    ));
    let pipeline_hybrid = RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: vector_hits }),
        Arc::new(MockLlm),
    )
    .with_hybrid_retriever(hybrid);

    let vector_hits_result = pipeline_vector.retrieve("Rust", 10).await.unwrap();
    let vector_ids: Vec<String> = vector_hits_result.iter().map(|h| h.id.clone()).collect();
    let vector_recall = compute_recall(&vector_ids, &relevant);

    let hybrid_hits_result = pipeline_hybrid.retrieve("Rust", 10).await.unwrap();
    let hybrid_ids: Vec<String> = hybrid_hits_result.iter().map(|h| h.id.clone()).collect();
    let hybrid_recall = compute_recall(&hybrid_ids, &relevant);

    assert!(
        hybrid_recall >= vector_recall,
        "hybrid recall ({hybrid_recall}) should be >= vector recall ({vector_recall})"
    );
    assert!(hybrid_recall > 0.0, "hybrid recall should be positive");
}

#[tokio::test]
async fn hybrid_retriever_with_custom_rrf_params() {
    let vector_hits = vec![make_hit("vec1", 0.9, "vector result")];

    let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));
    {
        let mut index = bm25.write().await;
        index.add_document("kw1", "keyword result");
    }

    let hybrid = HybridRetriever::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: vector_hits }),
        bm25,
    )
    .with_rrf_params(RrfParams {
        k: 30,
        vector_weight: 0.7,
        keyword_weight: 0.3,
    });

    let result = hybrid.retrieve("keyword", 5, "tenant").await.unwrap();
    assert!(!result.is_empty());
}

#[tokio::test]
async fn rag_pipeline_with_hybrid_retriever_produces_citations() {
    let vector_hits = vec![make_hit("doc1", 0.9, "Rust is safe")];

    let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));
    {
        let mut index = bm25.write().await;
        index.add_document("doc2", "Cargo is the Rust package manager");
    }

    let hybrid = Arc::new(HybridRetriever::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore { hits: vector_hits }),
        bm25,
    ));

    let pipeline = RagPipeline::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore {
            hits: vec![make_hit("doc1", 0.9, "Rust is safe")],
        }),
        Arc::new(MockLlm),
    )
    .with_hybrid_retriever(hybrid);

    let req = RagRequest::new("Rust", "tenant");
    let result = pipeline.rag(req).await.unwrap();

    assert!(!result.citations.is_empty());
}

#[tokio::test]
async fn hybrid_retriever_empty_bm25_falls_back_to_vector() {
    let vector_hits = vec![make_hit("vec1", 0.9, "only vector result")];
    let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));

    let hybrid = Arc::new(HybridRetriever::new(
        Arc::new(MockEmbedding),
        Arc::new(MockVectorStore {
            hits: vector_hits.clone(),
        }),
        bm25,
    ));

    let result = hybrid.retrieve("query", 5, "tenant").await.unwrap();
    assert!(!result.is_empty());
    assert_eq!(result[0].id, "vec1");
}

#[tokio::test]
async fn bm25_index_incremental_update() {
    let mut index = Bm25Index::new();
    index.add_document("doc1", "first document about Rust");
    assert_eq!(index.len(), 1);

    let hits1 = index.search("Rust", 5);
    assert!(!hits1.is_empty());

    index.add_document("doc2", "second document about Cargo");
    assert_eq!(index.len(), 2);

    let hits2 = index.search("Cargo", 5);
    assert!(!hits2.is_empty());
    assert_eq!(hits2[0].doc_id, "doc2");
}
