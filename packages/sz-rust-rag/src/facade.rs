// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Facade 统一入口：全局单例 + 便捷 API。

use crate::audit::RagAuditLogger;
use crate::capability::IndustryKnowledgeCapability;
use crate::config::RagConfig;
use crate::error::RagResult;
use crate::metrics::RagMetrics;
use crate::rule::FileRuleStore;
use crate::search::{IndustryRagSearcher, RagSearchRequest, RagSearchResult};
use crate::template::FileTemplateStore;
use crate::term::FileTermStore;

use std::sync::{Arc, OnceLock};
use sz_rust_ai_facade::embedding::{EmbeddingProvider, VectorStore};

static INSTANCE: OnceLock<Arc<IndustryRagFacade>> = OnceLock::new();

/// Facade 内部持有所有组件。
pub struct IndustryRagFacade {
    searcher: Arc<IndustryRagSearcher>,
    config: Arc<RagConfig>,
}

impl IndustryRagFacade {
    fn new(
        embedding: Arc<dyn EmbeddingProvider>,
        vector_store: Arc<dyn VectorStore>,
        config: Arc<RagConfig>,
        term_store: Arc<FileTermStore>,
        rule_store: Arc<FileRuleStore>,
        template_store: Arc<FileTemplateStore>,
        metrics: Arc<RagMetrics>,
    ) -> Self {
        let audit_logger = RagAuditLogger::new(config.audit_log_path.clone().into());
        let searcher = Arc::new(IndustryRagSearcher::new(
            embedding,
            vector_store,
            term_store,
            rule_store,
            template_store,
            config.clone(),
            audit_logger,
            metrics,
        ));
        Self { searcher, config }
    }

    pub async fn search(&self, req: RagSearchRequest) -> RagResult<RagSearchResult> {
        self.searcher.search(req).await
    }

    pub fn searcher(&self) -> &Arc<IndustryRagSearcher> {
        &self.searcher
    }

    pub fn config(&self) -> &Arc<RagConfig> {
        &self.config
    }
}

/// 全局静态 API。
pub struct IndustryRag;

impl IndustryRag {
    /// 初始化（OnceLock 全局单例）。
    pub fn init(
        embedding: Arc<dyn EmbeddingProvider>,
        vector_store: Arc<dyn VectorStore>,
        config: Arc<RagConfig>,
        term_store: Arc<FileTermStore>,
        rule_store: Arc<FileRuleStore>,
        template_store: Arc<FileTemplateStore>,
        metrics: Arc<RagMetrics>,
    ) -> RagResult<()> {
        let facade = IndustryRagFacade::new(
            embedding,
            vector_store,
            config,
            term_store,
            rule_store,
            template_store,
            metrics,
        );
        INSTANCE
            .set(Arc::new(facade))
            .map_err(|_| crate::error::RagError::Internal("already initialized".into()))?;
        Ok(())
    }

    /// 获取全局实例。
    pub fn instance() -> RagResult<Arc<IndustryRagFacade>> {
        INSTANCE
            .get()
            .cloned()
            .ok_or_else(|| crate::error::RagError::Internal("not initialized".into()))
    }

    /// 行业知识检索（静态 API）。
    pub async fn search(req: RagSearchRequest) -> RagResult<RagSearchResult> {
        Self::instance()?.search(req).await
    }

    /// 将本组件注册为 CapabilityRegistry 中的 Skill。
    pub fn register_capability(registry: &sz_rust_capability::CapabilityRegistry) -> RagResult<()> {
        let facade = Self::instance()?;
        let cap = IndustryKnowledgeCapability::new(facade.searcher().clone());
        registry.register(Arc::new(cap));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_ai_facade::common::AiError;
    use sz_rust_ai_facade::embedding::{
        EmbeddingProvider, EmbeddingRequest, EmbeddingResult, SimilarityMetric, VectorHit,
    };

    #[allow(dead_code)] // 测试 stub，供单测引用
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

    #[allow(dead_code)] // 测试 stub，供单测引用
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
    async fn instance_not_initialized() {
        let result = IndustryRag::instance();
        assert!(result.is_err());
    }
}
