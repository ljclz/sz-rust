//! Capability 适配：将 RAG 检索注册为 Skill。

use crate::search::{IndustryRagSearcher, RagSearchRequest};
use async_trait::async_trait;
use std::sync::Arc;
use sz_rust_capability::error::{CapError, CapResult};
use sz_rust_capability::source::CapabilitySource;
use sz_rust_capability::Capability;

/// 行业知识检索 Capability。
pub struct IndustryKnowledgeCapability {
    searcher: Arc<IndustryRagSearcher>,
}

impl IndustryKnowledgeCapability {
    pub fn new(searcher: Arc<IndustryRagSearcher>) -> Self {
        Self { searcher }
    }
}

#[async_trait]
impl Capability for IndustryKnowledgeCapability {
    fn name(&self) -> &'static str {
        "rag.industry_knowledge_search"
    }

    fn description(&self) -> &'static str {
        "生鲜零售行业 RAG 知识检索"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "查询文本" },
                "topk": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 },
                "token_budget": { "type": "integer", "minimum": 1, "default": 4096 },
                "tenant_id": { "type": "string", "description": "租户标识" }
            },
            "required": ["query", "tenant_id"]
        })
    }

    fn tags(&self) -> &[&'static str] {
        &["rag", "search", "industry"]
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Skill
    }

    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("missing query".into()))?
            .to_string();
        let tenant_id = args
            .get("tenant_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("missing tenant_id".into()))?
            .to_string();
        let topk = args.get("topk").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let token_budget = args
            .get("token_budget")
            .and_then(|v| v.as_u64())
            .unwrap_or(4096) as u32;

        let req = RagSearchRequest {
            query,
            topk,
            token_token_budget: token_budget,
            tenant_id,
        };

        let result = self
            .searcher
            .search(req)
            .await
            .map_err(|e| CapError::ExecutionError(e.to_string()))?;

        serde_json::to_value(&result).map_err(|e| CapError::ExecutionError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::RagAuditLogger;
    use crate::config::RagConfig;
    use crate::metrics::RagMetrics;
    use crate::rule::FileRuleStore;
    use crate::template::FileTemplateStore;
    use crate::term::FileTermStore;
    use sz_rust_ai_facade::common::AiError;
    use sz_rust_ai_facade::embedding::{
        EmbeddingProvider, EmbeddingRequest, EmbeddingResult, SimilarityMetric, VectorHit,
        VectorStore,
    };

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
            Ok(vec![])
        }
        async fn delete(&self, _ids: &[&str], _tenant: &str) -> Result<(), AiError> {
            Ok(())
        }
    }

    fn make_capability() -> IndustryKnowledgeCapability {
        let searcher = Arc::new(IndustryRagSearcher::new(
            Arc::new(StubEmbedding),
            Arc::new(StubVectorStore),
            Arc::new(FileTermStore::in_memory()),
            Arc::new(FileRuleStore::in_memory()),
            Arc::new(FileTemplateStore::in_memory()),
            Arc::new(RagConfig::for_testing()),
            RagAuditLogger::noop(),
            Arc::new(RagMetrics::register()),
        ));
        IndustryKnowledgeCapability::new(searcher)
    }

    #[tokio::test]
    async fn capability_metadata() {
        let cap = make_capability();
        assert_eq!(cap.name(), "rag.industry_knowledge_search");
        assert_eq!(cap.source(), CapabilitySource::Skill);
        assert!(cap.tags().contains(&"rag"));
    }

    #[tokio::test]
    async fn capability_call_success() {
        let cap = make_capability();
        let args = serde_json::json!({"query": "称重", "tenant_id": "t"});
        let result = cap.call(args).await.unwrap();
        assert!(result.get("citations").is_some());
    }

    #[tokio::test]
    async fn capability_call_missing_query() {
        let cap = make_capability();
        let args = serde_json::json!({"tenant_id": "t"});
        let result = cap.call(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn capability_description() {
        let cap = make_capability();
        assert_eq!(cap.description(), "生鲜零售行业 RAG 知识检索");
    }

    #[tokio::test]
    async fn capability_schema_structure() {
        let cap = make_capability();
        let schema = cap.schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["properties"]["tenant_id"].is_object());
        assert!(schema["required"].is_array());
    }

    #[tokio::test]
    async fn capability_call_missing_tenant_id() {
        let cap = make_capability();
        let args = serde_json::json!({"query": "称重"});
        let result = cap.call(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn capability_call_with_topk_and_budget() {
        let cap = make_capability();
        let args =
            serde_json::json!({"query": "称重", "tenant_id": "t", "topk": 5, "token_budget": 2048});
        let result = cap.call(args).await.unwrap();
        assert!(result.get("citations").is_some());
    }
}
