// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::common::AiError;
use crate::embedding::{
    EmbeddingProvider, EmbeddingRequest, SimilarityMetric, VectorHit, VectorStore,
};
use crate::llm::provider::{ChatMessage, ChatRequest, LlmProvider, Role};
use crate::rag::citation::Citation;
use crate::rag::hybrid::HybridRetrieverTrait;
use crate::rag::reranker::{NoopReranker, Reranker};
use std::sync::Arc;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RagRequest {
    pub query: String,
    pub topk: usize,
    pub token_budget: u32,
    pub tenant_id: String,
}

impl RagRequest {
    pub fn new(query: impl Into<String>, tenant_id: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            topk: 10,
            token_budget: 4096,
            tenant_id: tenant_id.into(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RagResult {
    pub content: String,
    pub citations: Vec<Citation>,
    pub warnings: Vec<WarningCode>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    ContextTruncated,
    LowRecallScore,
    RerankerSkipped,
}

pub struct RagPipeline {
    embedding: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn VectorStore>,
    llm: Arc<dyn LlmProvider>,
    reranker: Arc<dyn Reranker>,
    hybrid_retriever: Option<Arc<dyn HybridRetrieverTrait>>,
    embedding_model: String,
    llm_model: String,
    metric: SimilarityMetric,
}

impl RagPipeline {
    pub fn new(
        embedding: Arc<dyn EmbeddingProvider>,
        vector_store: Arc<dyn VectorStore>,
        llm: Arc<dyn LlmProvider>,
    ) -> Self {
        Self {
            embedding,
            vector_store,
            llm,
            reranker: Arc::new(NoopReranker::new()),
            hybrid_retriever: None,
            embedding_model: "text-embedding-3-small".to_string(),
            llm_model: "gpt-4o".to_string(),
            metric: SimilarityMetric::Cosine,
        }
    }

    pub fn with_reranker(mut self, reranker: Arc<dyn Reranker>) -> Self {
        self.reranker = reranker;
        self
    }

    /// 启用混合检索（运行时切换）
    ///
    /// 传入 HybridRetriever 实例后，retrieve 阶段将使用混合检索替代纯向量检索。
    pub fn with_hybrid_retriever(mut self, retriever: Arc<dyn HybridRetrieverTrait>) -> Self {
        self.hybrid_retriever = Some(retriever);
        self
    }

    pub fn with_embedding_model(mut self, model: impl Into<String>) -> Self {
        self.embedding_model = model.into();
        self
    }

    pub fn with_llm_model(mut self, model: impl Into<String>) -> Self {
        self.llm_model = model.into();
        self
    }

    pub fn with_metric(mut self, metric: SimilarityMetric) -> Self {
        self.metric = metric;
        self
    }

    pub async fn rag(&self, req: RagRequest) -> Result<RagResult, AiError> {
        let mut warnings = Vec::new();

        let hits = self.retrieve(&req.query, req.topk).await?;
        let final_hits = match self
            .reranker
            .rerank(&req.query, hits.clone(), req.topk)
            .await
        {
            Ok(reranked) if !reranked.is_empty() => reranked,
            Ok(_) => {
                warnings.push(WarningCode::RerankerSkipped);
                hits
            }
            Err(e) => {
                tracing::warn!(target: "ai_rag", "reranker failed, using original order: {e}");
                warnings.push(WarningCode::RerankerSkipped);
                hits
            }
        };
        let context = self.assemble(&final_hits, req.token_budget).await?;

        if context.chars().count() as u32 >= req.token_budget * 4 {
            warnings.push(WarningCode::ContextTruncated);
        }

        let mut result = self.generate(&final_hits, &context, &req.query).await?;
        result.warnings.extend(warnings);
        Ok(result)
    }

    pub async fn retrieve(&self, query: &str, topk: usize) -> Result<Vec<VectorHit>, AiError> {
        if let Some(ref hybrid) = self.hybrid_retriever {
            return hybrid.retrieve(query, topk, "").await;
        }

        let embed_req = EmbeddingRequest::new(&self.embedding_model, vec![query.to_string()]);
        let embed_result = self.embedding.embed(embed_req).await?;

        let query_vec = embed_result
            .embeddings
            .into_iter()
            .next()
            .ok_or_else(|| AiError::Internal("embedding returned no vectors".to_string()))?;

        self.vector_store
            .query(&query_vec, topk, self.metric, "")
            .await
    }

    /// 检索 + 重排序的便捷方法
    pub async fn retrieve_with_rerank(
        &self,
        query: &str,
        topk: usize,
    ) -> Result<Vec<VectorHit>, AiError> {
        let hits = self.retrieve(query, topk).await?;
        self.reranker.rerank(query, hits, topk).await
    }

    pub async fn assemble(&self, hits: &[VectorHit], budget: u32) -> Result<String, AiError> {
        let mut context = String::new();
        let mut total_chars = 0u32;
        let budget_chars = budget * 4;

        for (i, hit) in hits.iter().enumerate() {
            let chunk = format!("[{}] {}\n\n", i + 1, hit.text);
            let chunk_chars = chunk.chars().count() as u32;

            if total_chars + chunk_chars > budget_chars {
                tracing::warn!(
                    target: "ai_rag",
                    "AI_CONTEXT_TRUNCATED: budget={} chars, current={}",
                    budget_chars, total_chars
                );
                break;
            }

            context.push_str(&chunk);
            total_chars += chunk_chars;
        }

        Ok(context)
    }

    pub async fn generate(
        &self,
        hits: &[VectorHit],
        context: &str,
        query: &str,
    ) -> Result<RagResult, AiError> {
        let system_prompt = format!(
            "You are a helpful assistant. Use the following context to answer the question.\n\nContext:\n{}",
            context
        );

        let req = ChatRequest::new(
            &self.llm_model,
            vec![
                ChatMessage {
                    role: Role::System,
                    content: system_prompt.into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: query.to_string().into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
            ],
        );

        let completion = self.llm.chat_completion(req).await?;
        let content = completion
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content.to_string())
            .unwrap_or_default();

        let citations: Vec<Citation> = hits
            .iter()
            .enumerate()
            .map(|(i, hit)| Citation {
                doc_id: hit.id.clone(),
                offset: i as u32,
                length: hit.text.len() as u32,
                score: hit.score,
                text: hit.text.clone(),
            })
            .collect();

        Ok(RagResult {
            content,
            citations,
            warnings: Vec::new(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::{EmbeddingProvider, EmbeddingRequest, EmbeddingResult, VectorRecord};
    use crate::llm::provider::{
        ChatCompletion, Choice, FinishReason, LlmProvider, StreamDelta, Usage,
    };
    use async_trait::async_trait;
    use futures::stream::BoxStream;

    struct MockEmbedding {
        dim: usize,
        empty: bool,
    }

    #[async_trait]
    impl EmbeddingProvider for MockEmbedding {
        fn name(&self) -> &str {
            "mock-embedding"
        }
        async fn embed(&self, req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
            if self.empty {
                return Ok(EmbeddingResult {
                    model: req.model,
                    embeddings: vec![],
                    dimensions: self.dim,
                    usage_tokens: 0,
                });
            }
            Ok(EmbeddingResult {
                model: req.model,
                embeddings: vec![vec![0.1; self.dim]],
                dimensions: self.dim,
                usage_tokens: 1,
            })
        }
        fn dimensions(&self) -> usize {
            self.dim
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

    struct MockLlmProvider {
        response: String,
    }

    #[async_trait]
    impl LlmProvider for MockLlmProvider {
        fn name(&self) -> &str {
            "mock-llm"
        }
        async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
            Ok(ChatCompletion {
                id: "mock-id".into(),
                model: req.model,
                choices: vec![Choice {
                    index: 0,
                    message: ChatMessage {
                        role: Role::Assistant,
                        content: self.response.clone().into(),
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
            Err(AiError::Internal("not implemented".into()))
        }
        async fn token_count(&self, _messages: &[ChatMessage]) -> Result<u32, AiError> {
            Ok(0)
        }
        fn supported_models(&self) -> &[&str] {
            &["mock"]
        }
    }

    struct MockHybridRetriever {
        hits: Vec<VectorHit>,
    }

    #[async_trait]
    impl HybridRetrieverTrait for MockHybridRetriever {
        async fn retrieve(
            &self,
            _query: &str,
            topk: usize,
            _tenant: &str,
        ) -> Result<Vec<VectorHit>, AiError> {
            Ok(self.hits.iter().take(topk).cloned().collect())
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

    fn make_pipeline() -> RagPipeline {
        RagPipeline::new(
            Arc::new(MockEmbedding {
                dim: 3,
                empty: false,
            }),
            Arc::new(MockVectorStore { hits: vec![] }),
            Arc::new(MockLlmProvider {
                response: "mock answer".into(),
            }),
        )
    }

    #[test]
    fn rag_request_new_defaults() {
        let req = RagRequest::new("hello", "tenant-1");
        assert_eq!(req.query, "hello");
        assert_eq!(req.topk, 10);
        assert_eq!(req.token_budget, 4096);
        assert_eq!(req.tenant_id, "tenant-1");
    }

    #[test]
    fn warning_code_serde_snake_case() {
        let json = serde_json::to_string(&WarningCode::ContextTruncated).unwrap();
        assert_eq!(json, "\"context_truncated\"");
        let json = serde_json::to_string(&WarningCode::LowRecallScore).unwrap();
        assert_eq!(json, "\"low_recall_score\"");
        let json = serde_json::to_string(&WarningCode::RerankerSkipped).unwrap();
        assert_eq!(json, "\"reranker_skipped\"");
    }

    #[tokio::test]
    async fn assemble_within_budget() {
        let pipeline = make_pipeline();
        let hits = vec![make_hit("a", 0.9, "doc a"), make_hit("b", 0.8, "doc b")];
        let context = pipeline.assemble(&hits, 4096).await.unwrap();
        assert!(context.contains("doc a"));
        assert!(context.contains("doc b"));
        assert!(context.contains("[1]"));
        assert!(context.contains("[2]"));
    }

    #[tokio::test]
    async fn assemble_truncates_at_budget() {
        let pipeline = make_pipeline();
        let hits = vec![make_hit("a", 0.9, "very long document text")];
        let context = pipeline.assemble(&hits, 1).await.unwrap();
        assert!(
            context.is_empty(),
            "budget=1 means 4 chars, first chunk exceeds"
        );
    }

    #[tokio::test]
    async fn assemble_empty_hits() {
        let pipeline = make_pipeline();
        let context = pipeline.assemble(&[], 4096).await.unwrap();
        assert!(context.is_empty());
    }

    #[tokio::test]
    async fn assemble_partial_truncation() {
        let pipeline = make_pipeline();
        let hits = vec![
            make_hit("a", 0.9, "short"),
            make_hit("b", 0.8, "also short"),
            make_hit("c", 0.7, "this one is very long and will exceed the budget"),
        ];
        // budget=10 -> 40 chars, 前两个 chunk 应该能放入，第三个被截断
        let context = pipeline.assemble(&hits, 10).await.unwrap();
        assert!(context.contains("short"));
        assert!(!context.contains("this one is very long"));
    }

    #[tokio::test]
    async fn retrieve_via_vector_store() {
        let hits = vec![make_hit("a", 0.9, "doc a")];
        let pipeline = RagPipeline::new(
            Arc::new(MockEmbedding {
                dim: 3,
                empty: false,
            }),
            Arc::new(MockVectorStore { hits }),
            Arc::new(MockLlmProvider {
                response: "answer".into(),
            }),
        );
        let result = pipeline.retrieve("query", 5).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "a");
    }

    #[tokio::test]
    async fn retrieve_via_hybrid_retriever() {
        let hits = vec![make_hit("hybrid-1", 0.85, "hybrid doc")];
        let pipeline =
            make_pipeline().with_hybrid_retriever(Arc::new(MockHybridRetriever { hits }));
        let result = pipeline.retrieve("query", 5).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "hybrid-1");
    }

    #[tokio::test]
    async fn retrieve_embedding_returns_no_vectors_errors() {
        let pipeline = RagPipeline::new(
            Arc::new(MockEmbedding {
                dim: 3,
                empty: true,
            }),
            Arc::new(MockVectorStore { hits: vec![] }),
            Arc::new(MockLlmProvider {
                response: "answer".into(),
            }),
        );
        let result = pipeline.retrieve("query", 5).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().error_code(), "AI_INTERNAL");
    }

    #[tokio::test]
    async fn generate_produces_result_with_citations() {
        let pipeline = make_pipeline();
        let hits = vec![make_hit("a", 0.9, "doc a")];
        let result = pipeline.generate(&hits, "context", "query").await.unwrap();
        assert_eq!(result.content, "mock answer");
        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].doc_id, "a");
        assert_eq!(result.citations[0].offset, 0);
        assert!(result.warnings.is_empty());
    }

    #[tokio::test]
    async fn generate_empty_hits() {
        let pipeline = make_pipeline();
        let result = pipeline.generate(&[], "context", "query").await.unwrap();
        assert_eq!(result.content, "mock answer");
        assert!(result.citations.is_empty());
    }

    #[tokio::test]
    async fn rag_full_pipeline() {
        let hits = vec![make_hit("a", 0.9, "doc a")];
        let pipeline = RagPipeline::new(
            Arc::new(MockEmbedding {
                dim: 3,
                empty: false,
            }),
            Arc::new(MockVectorStore { hits }),
            Arc::new(MockLlmProvider {
                response: "final answer".into(),
            }),
        );
        let req = RagRequest::new("query", "tenant");
        let result = pipeline.rag(req).await.unwrap();
        assert_eq!(result.content, "final answer");
        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].doc_id, "a");
    }

    #[tokio::test]
    async fn rag_with_reranker_skipped_warning() {
        // NoopReranker 在 candidates 为空时返回空，触发 RerankerSkipped
        let pipeline = make_pipeline(); // 默认 NoopReranker
        let req = RagRequest::new("query", "tenant");
        // MockVectorStore 返回空 hits，reranker 返回空 -> RerankerSkipped
        let result = pipeline.rag(req).await.unwrap();
        assert!(result
            .warnings
            .iter()
            .any(|w| matches!(w, WarningCode::RerankerSkipped)));
    }

    #[tokio::test]
    async fn retrieve_with_rerank() {
        let hits = vec![make_hit("a", 0.9, "doc a"), make_hit("b", 0.8, "doc b")];
        let pipeline = RagPipeline::new(
            Arc::new(MockEmbedding {
                dim: 3,
                empty: false,
            }),
            Arc::new(MockVectorStore { hits }),
            Arc::new(MockLlmProvider {
                response: "answer".into(),
            }),
        );
        let result = pipeline.retrieve_with_rerank("query", 2).await.unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn rag_pipeline_builder_methods() {
        let pipeline = make_pipeline()
            .with_embedding_model("custom-embed")
            .with_llm_model("custom-llm")
            .with_metric(SimilarityMetric::Dot);
        // builder 方法返回 Self，验证链式调用不 panic
        let _pipeline2 = pipeline.with_reranker(Arc::new(NoopReranker::new()));
    }
}
