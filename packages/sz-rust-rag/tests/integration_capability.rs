// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 集成测试：Capability 注册与调用。

use std::sync::Arc;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::{
    EmbeddingProvider, EmbeddingRequest, EmbeddingResult, SimilarityMetric, VectorHit, VectorStore,
};
use sz_rust_rag::audit::RagAuditLogger;
use sz_rust_rag::config::RagConfig;
use sz_rust_rag::metrics::RagMetrics;
use sz_rust_rag::rule::FileRuleStore;
use sz_rust_rag::search::IndustryRagSearcher;
use sz_rust_rag::template::FileTemplateStore;
use sz_rust_rag::term::FileTermStore;

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

#[tokio::test]
async fn capability_register_and_call() {
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

    let cap = sz_rust_rag::capability::IndustryKnowledgeCapability::new(searcher);
    let registry = sz_rust_capability::CapabilityRegistry::new();
    registry.register(Arc::new(cap));

    let found = registry.find_by_tags(&["rag", "search"], None);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name(), "rag.industry_knowledge_search");

    let args = serde_json::json!({"query": "称重计价", "tenant_id": "t1"});
    let result = registry
        .call("rag.industry_knowledge_search", args)
        .await
        .unwrap();
    assert!(result.get("content").is_some());
    assert!(result.get("citations").is_some());
    assert!(result.get("warnings").is_some());
}

#[tokio::test]
async fn capability_validation_error() {
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

    let cap = sz_rust_rag::capability::IndustryKnowledgeCapability::new(searcher);
    let registry = sz_rust_capability::CapabilityRegistry::new();
    registry.register(Arc::new(cap));

    let args = serde_json::json!({"tenant_id": "t1"});
    let result = registry.call("rag.industry_knowledge_search", args).await;
    assert!(result.is_err());
}
