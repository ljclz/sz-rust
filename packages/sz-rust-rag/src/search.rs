//! 行业 RAG 检索器：多源融合 + token 截断 + 警告注入 + 引用溯源。

use crate::audit::RagAuditLogger;
use crate::config::RagConfig;
use crate::error::{RagError, RagResult};
use crate::metrics::RagMetrics;
use crate::rule::RuleStore;
use crate::template::TemplateStore;
use crate::term::TermStore;
use crate::warning::RagWarningCode;
use std::sync::Arc;
use sz_rust_ai_facade::embedding::{EmbeddingProvider, EmbeddingRequest, VectorStore};

/// 检索请求。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RagSearchRequest {
    pub query: String,
    pub topk: usize,
    pub token_token_budget: u32,
    pub tenant_id: String,
}

impl RagSearchRequest {
    pub fn new(query: impl Into<String>, tenant_id: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            topk: 10,
            token_token_budget: 4096,
            tenant_id: tenant_id.into(),
        }
    }
}

/// 检索结果。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RagSearchResult {
    pub content: String,
    pub citations: Vec<RagCitation>,
    pub warnings: Vec<RagWarningCode>,
}

/// 引用。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RagCitation {
    pub doc_id: String,
    pub score: f32,
    pub text: String,
    pub tenant_id: String,
    pub knowledge_type: KnowledgeType,
    pub source: Option<SourceLocation>,
}

/// 知识类型。
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeType {
    Code,
    Term,
    Rule,
    Template,
}

/// 来源位置。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceLocation {
    pub crate_name: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: u32,
    pub symbol_type: String,
}

/// 行业 RAG 检索器。
pub struct IndustryRagSearcher {
    embedding: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn VectorStore>,
    term_store: Arc<dyn TermStore>,
    rule_store: Arc<dyn RuleStore>,
    template_store: Arc<dyn TemplateStore>,
    config: Arc<RagConfig>,
    audit_logger: RagAuditLogger,
    metrics: Arc<RagMetrics>,
}

impl IndustryRagSearcher {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        embedding: Arc<dyn EmbeddingProvider>,
        vector_store: Arc<dyn VectorStore>,
        term_store: Arc<dyn TermStore>,
        rule_store: Arc<dyn RuleStore>,
        template_store: Arc<dyn TemplateStore>,
        config: Arc<RagConfig>,
        audit_logger: RagAuditLogger,
        metrics: Arc<RagMetrics>,
    ) -> Self {
        Self {
            embedding,
            vector_store,
            term_store,
            rule_store,
            template_store,
            config,
            audit_logger,
            metrics,
        }
    }

    /// 行业知识检索：查询向量化 → 向量检索 → 多源融合 → token 截断 → 引用溯源。
    pub async fn search(&self, req: RagSearchRequest) -> RagResult<RagSearchResult> {
        let start = std::time::Instant::now();
        if req.query.trim().is_empty() {
            return Err(RagError::Internal("query is empty".into()));
        }
        if req.tenant_id.trim().is_empty() {
            return Err(RagError::Internal("tenant_id is empty".into()));
        }

        let topk = req.topk.min(self.config.max_topk).max(1);
        let mut warnings = Vec::new();
        let mut citations = Vec::new();
        let mut content_parts = Vec::new();

        let embed_req = EmbeddingRequest {
            model: self.config.embedding_model.clone(),
            input: vec![req.query.clone()],
        };

        match self.embedding.embed(embed_req).await {
            Ok(result) => {
                self.metrics.record_embedding_call();
                if let Some(query_vector) = result.embeddings.into_iter().next() {
                    match self
                        .vector_store
                        .query(
                            &query_vector,
                            topk,
                            self.config.similarity_metric,
                            &req.tenant_id,
                        )
                        .await
                    {
                        Ok(hits) => {
                            for hit in hits {
                                let source =
                                    hit.metadata.get("crate_name").and_then(|v| v.as_str()).map(
                                        |crate_name| SourceLocation {
                                            crate_name: crate_name.to_string(),
                                            file_path: hit
                                                .metadata
                                                .get("file_path")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            line_start: hit
                                                .metadata
                                                .get("line_start")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0)
                                                as u32,
                                            line_end: hit
                                                .metadata
                                                .get("line_end")
                                                .and_then(|v| v.as_u64())
                                                .unwrap_or(0)
                                                as u32,
                                            symbol_type: hit
                                                .metadata
                                                .get("symbol_type")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("other")
                                                .to_string(),
                                        },
                                    );

                                citations.push(RagCitation {
                                    doc_id: hit.id.clone(),
                                    score: hit.score,
                                    text: hit.text.clone(),
                                    tenant_id: req.tenant_id.clone(),
                                    knowledge_type: KnowledgeType::Code,
                                    source,
                                });
                                content_parts.push(hit.text);
                            }
                        }
                        Err(_) => {
                            self.metrics.record_vector_store_error();
                            warnings.push(RagWarningCode::StoreVersionMismatch);
                        }
                    }
                }
            }
            Err(_) => {
                warnings.push(RagWarningCode::EmbeddingPartialFailure);
            }
        }

        let term_results = self
            .term_store
            .search(&req.query, &req.tenant_id)
            .await
            .unwrap_or_default();
        for term in term_results {
            citations.push(RagCitation {
                doc_id: format!("term:{}", term.term_name),
                score: 1.0,
                text: format!("术语 {}: {}", term.term_name, term.definition),
                tenant_id: req.tenant_id.clone(),
                knowledge_type: KnowledgeType::Term,
                source: None,
            });
            content_parts.push(format!("术语 {}: {}", term.term_name, term.definition));
        }

        let rule_results = self
            .rule_store
            .search(&req.query, &req.tenant_id)
            .await
            .unwrap_or_default();
        for rule in rule_results {
            citations.push(RagCitation {
                doc_id: format!("rule:{}", rule.rule_name),
                score: 1.0,
                text: format!("规则 {}: {}", rule.rule_name, rule.rule_text),
                tenant_id: req.tenant_id.clone(),
                knowledge_type: KnowledgeType::Rule,
                source: Some(SourceLocation {
                    crate_name: rule.source_crate,
                    file_path: rule.source_file_path,
                    line_start: rule.source_line_start,
                    line_end: rule.source_line_end,
                    symbol_type: "rule".into(),
                }),
            });
            content_parts.push(format!("规则 {}: {}", rule.rule_name, rule.rule_text));
        }

        let template_results = self
            .template_store
            .search(&req.query, &req.tenant_id)
            .await
            .unwrap_or_default();
        for tmpl in template_results {
            let text = format!("模板 {}: {} 字段", tmpl.object_name, tmpl.fields.len());
            citations.push(RagCitation {
                doc_id: format!("template:{}", tmpl.object_name),
                score: 1.0,
                text: text.clone(),
                tenant_id: req.tenant_id.clone(),
                knowledge_type: KnowledgeType::Template,
                source: None,
            });
            content_parts.push(text);
        }

        let top_score = citations.iter().map(|c| c.score).fold(0.0f32, f32::max);
        if top_score < self.config.low_recall_threshold && !citations.is_empty() {
            warnings.push(RagWarningCode::LowRecallScore);
        }

        let mut content = content_parts.join("\n\n");
        let estimated_tokens = content.len() as u32 / 4;
        if estimated_tokens > req.token_token_budget {
            let max_chars = (req.token_token_budget as usize) * 4;
            if max_chars < content.len() {
                content.truncate(max_chars);
                warnings.push(RagWarningCode::TokenBudgetExceeded);
            }
        }

        let duration = start.elapsed();
        self.metrics.record_retrieve(duration, top_score);

        let audit_entry = crate::audit::RagAuditLog {
            trace_id: uuid::Uuid::new_v4().to_string(),
            tenant_id: req.tenant_id.clone(),
            query_redacted: req.query.clone(),
            hit_count: citations.len() as u32,
            top_score,
            duration_ms: duration.as_millis() as u64,
            warnings: warnings.clone(),
            timestamp: chrono::Utc::now(),
        };
        let _ = self.audit_logger.log(audit_entry).await;

        Ok(RagSearchResult {
            content,
            citations,
            warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::FileRuleStore;
    use crate::template::FileTemplateStore;
    use crate::term::FileTermStore;
    use sz_rust_ai_facade::common::AiError;
    use sz_rust_ai_facade::embedding::{EmbeddingResult, SimilarityMetric, VectorHit};

    struct StubEmbedding;
    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedding {
        fn name(&self) -> &str {
            "stub"
        }
        async fn embed(&self, _req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
            Ok(EmbeddingResult {
                model: "stub".into(),
                embeddings: vec![vec![1.0; 8]],
                dimensions: 8,
                usage_tokens: 1,
            })
        }
        fn dimensions(&self) -> usize {
            8
        }
        fn supported_models(&self) -> &[&str] {
            &["stub"]
        }
    }

    struct StubVectorStore;
    #[async_trait::async_trait]
    impl VectorStore for StubVectorStore {
        async fn upsert(
            &self,
            _records: &[sz_rust_ai_facade::embedding::VectorRecord],
        ) -> Result<(), AiError> {
            Ok(())
        }
        async fn query(
            &self,
            _vector: &[f32],
            _topk: usize,
            _metric: SimilarityMetric,
            _tenant: &str,
        ) -> Result<Vec<VectorHit>, AiError> {
            Ok(vec![VectorHit {
                id: "code:1".into(),
                score: 0.9,
                metadata: serde_json::json!({"crate_name": "test", "file_path": "src/lib.rs", "line_start": 1, "line_end": 10, "symbol_type": "function"}),
                text: "fn foo() {}".into(),
            }])
        }
        async fn delete(&self, _ids: &[&str], _tenant: &str) -> Result<(), AiError> {
            Ok(())
        }
    }

    fn make_searcher() -> IndustryRagSearcher {
        IndustryRagSearcher::new(
            Arc::new(StubEmbedding),
            Arc::new(StubVectorStore),
            Arc::new(FileTermStore::in_memory()),
            Arc::new(FileRuleStore::in_memory()),
            Arc::new(FileTemplateStore::in_memory()),
            Arc::new(RagConfig::for_testing()),
            RagAuditLogger::noop(),
            Arc::new(RagMetrics::register()),
        )
    }

    #[tokio::test]
    async fn search_empty_query() {
        let searcher = make_searcher();
        let req = RagSearchRequest::new("", "t");
        assert!(searcher.search(req).await.is_err());
    }

    #[tokio::test]
    async fn search_empty_tenant() {
        let searcher = make_searcher();
        let req = RagSearchRequest::new("query", "");
        assert!(searcher.search(req).await.is_err());
    }

    #[tokio::test]
    async fn search_basic() {
        let searcher = make_searcher();
        let req = RagSearchRequest::new("如何计价", "tenant-1");
        let result = searcher.search(req).await.unwrap();
        assert!(!result.citations.is_empty());
        assert_eq!(result.citations[0].tenant_id, "tenant-1");
    }

    #[tokio::test]
    async fn search_with_term() {
        let term_store = FileTermStore::in_memory();
        term_store
            .add(
                crate::term::TermEntry {
                    term_name: "称重".into(),
                    definition: "按重量计价的商品".into(),
                    aliases: vec![],
                    confusable_with: vec![],
                    version: 1,
                    updated_at: chrono::Utc::now(),
                    updated_by: "test".into(),
                },
                "t",
            )
            .await
            .unwrap();

        let searcher = IndustryRagSearcher::new(
            Arc::new(StubEmbedding),
            Arc::new(StubVectorStore),
            Arc::new(term_store),
            Arc::new(FileRuleStore::in_memory()),
            Arc::new(FileTemplateStore::in_memory()),
            Arc::new(RagConfig::for_testing()),
            RagAuditLogger::noop(),
            Arc::new(RagMetrics::register()),
        );
        let req = RagSearchRequest::new("称重", "t");
        let result = searcher.search(req).await.unwrap();
        assert!(result
            .citations
            .iter()
            .any(|c| c.knowledge_type == KnowledgeType::Term));
    }
}
