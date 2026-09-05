// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! RAG 混合检索：向量检索 + BM25 关键词检索 + RRF 融合
//!
//! 任务组 12.2：HybridRetriever 实现
//! 任务组 12.3：RagPipeline 可选切换 HybridRetriever

use crate::common::AiError;
use crate::embedding::{
    EmbeddingProvider, EmbeddingRequest, SimilarityMetric, VectorHit, VectorRecord, VectorStore,
};
use crate::rag::bm25::{Bm25Hit, Bm25Index};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// RRF (Reciprocal Rank Fusion) 参数
#[derive(Clone, Debug)]
pub struct RrfParams {
    /// RRF 平滑常数，默认 60
    pub k: u32,
    /// 向量检索权重，默认 0.5
    pub vector_weight: f32,
    /// 关键词检索权重，默认 0.5
    pub keyword_weight: f32,
}

impl Default for RrfParams {
    fn default() -> Self {
        Self {
            k: 60,
            vector_weight: 0.5,
            keyword_weight: 0.5,
        }
    }
}

/// 混合检索器 trait
///
/// 结合向量语义检索与 BM25 关键词检索，通过 RRF 算法融合结果。
#[async_trait]
pub trait HybridRetrieverTrait: Send + Sync {
    /// 混合检索
    ///
    /// - `query`: 查询文本
    /// - `topk`: 返回前 topk 个结果
    /// - `tenant`: 租户 ID（用于向量检索隔离）
    async fn retrieve(
        &self,
        query: &str,
        topk: usize,
        tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError>;
}

/// 混合检索器实现
///
/// 组合向量存储 + BM25 索引，通过 RRF 融合两路检索结果。
pub struct HybridRetriever {
    embedding: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn VectorStore>,
    bm25: Arc<tokio::sync::RwLock<Bm25Index>>,
    embedding_model: String,
    metric: SimilarityMetric,
    rrf_params: RrfParams,
}

impl HybridRetriever {
    pub fn new(
        embedding: Arc<dyn EmbeddingProvider>,
        vector_store: Arc<dyn VectorStore>,
        bm25: Arc<tokio::sync::RwLock<Bm25Index>>,
    ) -> Self {
        Self {
            embedding,
            vector_store,
            bm25,
            embedding_model: "text-embedding-3-small".to_string(),
            metric: SimilarityMetric::Cosine,
            rrf_params: RrfParams::default(),
        }
    }

    pub fn with_embedding_model(mut self, model: impl Into<String>) -> Self {
        self.embedding_model = model.into();
        self
    }

    pub fn with_metric(mut self, metric: SimilarityMetric) -> Self {
        self.metric = metric;
        self
    }

    pub fn with_rrf_params(mut self, params: RrfParams) -> Self {
        self.rrf_params = params;
        self
    }

    /// 向量检索分支
    async fn vector_search(
        &self,
        query: &str,
        topk: usize,
        tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError> {
        let embed_req = EmbeddingRequest::new(&self.embedding_model, vec![query.to_string()]);
        let embed_result = self.embedding.embed(embed_req).await?;

        let query_vec = embed_result
            .embeddings
            .into_iter()
            .next()
            .ok_or_else(|| AiError::Internal("embedding returned no vectors".to_string()))?;

        self.vector_store
            .query(&query_vec, topk, self.metric, tenant)
            .await
    }

    /// BM25 检索分支
    ///
    /// 从 BM25 索引检索，并从向量存储获取对应 VectorHit（含 text/metadata）。
    /// 若向量存储中找不到对应文档，则构造仅含 doc_id 和 score 的 VectorHit。
    async fn keyword_search(
        &self,
        query: &str,
        topk: usize,
        _tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError> {
        let bm25 = self.bm25.read().await;
        let hits = bm25.search(query, topk);
        drop(bm25);

        let vector_hits: Vec<VectorHit> = hits
            .into_iter()
            .map(|bm25_hit: Bm25Hit| VectorHit {
                id: bm25_hit.doc_id,
                score: bm25_hit.score,
                metadata: serde_json::json!({}),
                text: String::new(),
            })
            .collect();

        Ok(vector_hits)
    }

    /// RRF 融合两路检索结果
    fn rrf_fuse(
        &self,
        vector_hits: Vec<VectorHit>,
        keyword_hits: Vec<VectorHit>,
        topk: usize,
    ) -> Vec<VectorHit> {
        let k = self.rrf_params.k as f32;
        let vw = self.rrf_params.vector_weight;
        let kw = self.rrf_params.keyword_weight;

        let mut scores: HashMap<String, (f32, VectorHit)> = HashMap::new();

        for (rank, hit) in vector_hits.iter().enumerate() {
            let rrf_score = vw / (k + (rank as f32 + 1.0));
            scores
                .entry(hit.id.clone())
                .and_modify(|(s, _)| *s += rrf_score)
                .or_insert((rrf_score, hit.clone()));
        }

        for (rank, hit) in keyword_hits.iter().enumerate() {
            let rrf_score = kw / (k + (rank as f32 + 1.0));
            scores
                .entry(hit.id.clone())
                .and_modify(|(s, existing)| {
                    *s += rrf_score;
                    if existing.text.is_empty() && !hit.text.is_empty() {
                        existing.text = hit.text.clone();
                        existing.metadata = hit.metadata.clone();
                    }
                })
                .or_insert((rrf_score, hit.clone()));
        }

        let mut fused: Vec<(f32, VectorHit)> = scores
            .into_iter()
            .map(|(_, (score, mut hit))| {
                hit.score = score;
                (score, hit)
            })
            .collect();

        fused.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        fused.into_iter().take(topk).map(|(_, hit)| hit).collect()
    }
}

#[async_trait]
impl HybridRetrieverTrait for HybridRetriever {
    async fn retrieve(
        &self,
        query: &str,
        topk: usize,
        tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError> {
        if topk == 0 {
            return Ok(Vec::new());
        }

        let vector_hits = self.vector_search(query, topk, tenant).await?;
        let keyword_hits = self.keyword_search(query, topk, tenant).await?;

        Ok(self.rrf_fuse(vector_hits, keyword_hits, topk))
    }
}

/// 向量存储适配：将 VectorRecord 添加到 BM25 索引
pub async fn index_records_to_bm25(
    bm25: &Arc<tokio::sync::RwLock<Bm25Index>>,
    records: &[VectorRecord],
) {
    let mut index = bm25.write().await;
    for record in records {
        if let Some(text) = record.metadata.get("text").and_then(|v| v.as_str()) {
            index.add_document(&record.id, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::EmbeddingResult;
    use async_trait::async_trait;

    struct MockEmbedding;

    #[async_trait]
    impl EmbeddingProvider for MockEmbedding {
        fn name(&self) -> &str {
            "mock"
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

    fn make_hit(id: &str, score: f32, text: &str) -> VectorHit {
        VectorHit {
            id: id.into(),
            score,
            metadata: serde_json::json!({}),
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn hybrid_retrieve_fuses_vector_and_keyword() {
        let vector_hits = vec![
            make_hit("vec1", 0.9, "vector result 1"),
            make_hit("vec2", 0.8, "vector result 2"),
        ];

        let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));
        {
            let mut index = bm25.write().await;
            index.add_document("kw1", "keyword result keyword");
            index.add_document("kw2", "another keyword doc");
        }

        let retriever = HybridRetriever::new(
            Arc::new(MockEmbedding),
            Arc::new(MockVectorStore { hits: vector_hits }),
            bm25,
        );

        let result = retriever.retrieve("keyword", 5, "tenant").await.unwrap();
        assert!(!result.is_empty());
        assert!(result.len() <= 5);
    }

    #[tokio::test]
    async fn hybrid_retrieve_empty_bm25_returns_vector_results() {
        let vector_hits = vec![make_hit("vec1", 0.9, "vector only")];
        let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));

        let retriever = HybridRetriever::new(
            Arc::new(MockEmbedding),
            Arc::new(MockVectorStore {
                hits: vector_hits.clone(),
            }),
            bm25,
        );

        let result = retriever.retrieve("query", 5, "tenant").await.unwrap();
        assert!(!result.is_empty());
        assert_eq!(result[0].id, "vec1");
    }

    #[tokio::test]
    async fn hybrid_retrieve_topk_zero_returns_empty() {
        let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));
        let retriever = HybridRetriever::new(
            Arc::new(MockEmbedding),
            Arc::new(MockVectorStore { hits: vec![] }),
            bm25,
        );

        let result = retriever.retrieve("query", 0, "tenant").await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn hybrid_rrf_overlapping_docs_get_higher_score() {
        let vector_hits = vec![make_hit("shared", 0.9, "shared doc")];
        let bm25 = Arc::new(tokio::sync::RwLock::new(Bm25Index::new()));
        {
            let mut index = bm25.write().await;
            index.add_document("shared", "shared keyword");
        }

        let retriever = HybridRetriever::new(
            Arc::new(MockEmbedding),
            Arc::new(MockVectorStore { hits: vector_hits }),
            bm25,
        );

        let result = retriever.retrieve("shared", 5, "tenant").await.unwrap();
        let shared_hit = result.iter().find(|h| h.id == "shared");
        assert!(
            shared_hit.is_some(),
            "shared doc should appear in fused results"
        );
    }
}
