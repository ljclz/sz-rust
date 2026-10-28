// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 集成测试：多源融合检索。

use std::sync::Arc;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::{
    EmbeddingProvider, EmbeddingRequest, EmbeddingResult, SimilarityMetric, VectorHit, VectorStore,
};
use sz_rust_rag::audit::RagAuditLogger;
use sz_rust_rag::config::RagConfig;
use sz_rust_rag::metrics::RagMetrics;
use sz_rust_rag::rule::{FileRuleStore, RuleEntry, RuleStore};
use sz_rust_rag::search::{IndustryRagSearcher, KnowledgeType, RagSearchRequest};
use sz_rust_rag::template::{FileTemplateStore, ModelTemplate, TemplateField, TemplateStore};
use sz_rust_rag::term::{FileTermStore, TermEntry, TermStore};

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
            score: 0.92,
            metadata: serde_json::json!({"crate_name": "sz-rust-core", "file_path": "src/lib.rs", "line_start": 1, "line_end": 20, "symbol_type": "function"}),
            text: "pub fn calculate_weight_price(weight: f64, unit_price: f64) -> f64 { weight * unit_price }".into(),
        }])
    }
    async fn delete(&self, _ids: &[&str], _tenant: &str) -> Result<(), AiError> {
        Ok(())
    }
}

#[tokio::test]
async fn multi_source_fusion_search() {
    let term_store = FileTermStore::in_memory();
    term_store
        .add(
            TermEntry {
                term_name: "称重".into(),
                definition: "按重量计价的商品类型".into(),
                aliases: vec!["散称".into()],
                confusable_with: vec![],
                version: 1,
                updated_at: chrono::Utc::now(),
                updated_by: "test".into(),
            },
            "tenant-1",
        )
        .await
        .unwrap();

    term_store
        .add(
            TermEntry {
                term_name: "损耗".into(),
                definition: "商品在流转过程中的重量损失".into(),
                aliases: vec![],
                confusable_with: vec![],
                version: 1,
                updated_at: chrono::Utc::now(),
                updated_by: "test".into(),
            },
            "tenant-1",
        )
        .await
        .unwrap();

    let rule_store = FileRuleStore::in_memory();
    rule_store
        .add(
            RuleEntry {
                rule_name: "称重计价规则".into(),
                rule_text: "称重商品按实际重量 × 单价计价，损耗率 ≤ 5%".into(),
                source_crate: "sz-rust-core".into(),
                source_file_path: "src/pricing/mod.rs".into(),
                source_line_start: 10,
                source_line_end: 30,
                applicable_scene: Some("pricing".into()),
                acceptance_criteria: Some("损耗率校验".into()),
                version: 1,
                updated_at: chrono::Utc::now(),
                updated_by: "test".into(),
            },
            "tenant-1",
        )
        .await
        .unwrap();

    let template_store = FileTemplateStore::in_memory();
    template_store
        .add(
            ModelTemplate {
                object_name: "商品".into(),
                fields: vec![TemplateField {
                    field_name: "sku_code".into(),
                    business_meaning: "商品 SKU 编码".into(),
                    constraint: Some("non-empty".into()),
                }],
                version: 1,
                updated_at: chrono::Utc::now(),
                updated_by: "test".into(),
            },
            "tenant-1",
        )
        .await
        .unwrap();

    let searcher = IndustryRagSearcher::new(
        Arc::new(StubEmbedding),
        Arc::new(StubVectorStore),
        Arc::new(term_store),
        Arc::new(rule_store),
        Arc::new(template_store),
        Arc::new(RagConfig::for_testing()),
        RagAuditLogger::noop(),
        Arc::new(RagMetrics::register()),
    );

    let req = RagSearchRequest::new("生鲜称重商品如何计价", "tenant-1");
    let result = searcher.search(req).await.unwrap();

    assert!(!result.citations.is_empty(), "should have citations");

    let has_code = result
        .citations
        .iter()
        .any(|c| c.knowledge_type == KnowledgeType::Code);
    let has_term = result
        .citations
        .iter()
        .any(|c| c.knowledge_type == KnowledgeType::Term);
    let has_rule = result
        .citations
        .iter()
        .any(|c| c.knowledge_type == KnowledgeType::Rule);
    let has_template = result
        .citations
        .iter()
        .any(|c| c.knowledge_type == KnowledgeType::Template);

    assert!(has_code, "should have code hit");
    assert!(has_term, "should have term hit");
    assert!(has_rule, "should have rule hit");
    assert!(has_template, "should have template hit");

    for citation in &result.citations {
        assert_eq!(citation.tenant_id, "tenant-1");
    }

    let estimated_tokens = result.content.len() / 4;
    assert!(
        estimated_tokens <= 2048,
        "content should be within token budget"
    );
}

#[tokio::test]
async fn search_tenant_isolation() {
    let term_store = FileTermStore::in_memory();
    term_store
        .add(
            TermEntry {
                term_name: "称重".into(),
                definition: "租户A的术语".into(),
                aliases: vec![],
                confusable_with: vec![],
                version: 1,
                updated_at: chrono::Utc::now(),
                updated_by: "test".into(),
            },
            "tenant-A",
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

    let req = RagSearchRequest::new("称重", "tenant-B");
    let result = searcher.search(req).await.unwrap();

    for citation in &result.citations {
        assert_eq!(citation.tenant_id, "tenant-B");
    }
    let term_hits: Vec<_> = result
        .citations
        .iter()
        .filter(|c| c.knowledge_type == KnowledgeType::Term)
        .collect();
    assert!(
        term_hits.is_empty(),
        "tenant-B should not see tenant-A terms"
    );
}
