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
