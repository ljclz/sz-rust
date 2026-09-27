//! 上下文注入器（v1.5.0 P2-1）
//!
//! 按需求语义检索相关代码片段注入 prompt（spec 5.5.2）。
//! 复用 P2-3 HybridSearch 进行检索。
#![forbid(unsafe_code)]

use sz_rust_rag::hybrid_search::{
    HybridSearch, HybridSearchConfig, HybridSearchResult, RawSearchResult,
};

/// 上下文注入器
pub struct ContextInjector {
    search: HybridSearch,
}

/// 注入结果
#[derive(Debug, Clone)]
pub struct InjectionResult {
    /// 增强后的 prompt
    pub enhanced_prompt: String,
    /// 检索到的上下文片段
    pub contexts: Vec<HybridSearchResult>,
    /// 注入的片段数
    pub injected_count: usize,
}

impl ContextInjector {
    /// 创建上下文注入器
    pub fn new(
        config: HybridSearchConfig,
    ) -> Result<Self, sz_rust_rag::hybrid_search::HybridSearchError> {
        let search = HybridSearch::new(config)?;
        Ok(Self { search })
    }

    /// 注入上下文（spec 5.5.2）
    ///
    /// 按需求语义检索相关代码片段，注入到 prompt 中。
    pub async fn inject<F1, Fut1, F2, Fut2>(
        &self,
        requirement: &str,
        vector_search: F1,
        keyword_search: F2,
    ) -> Result<InjectionResult, sz_rust_rag::hybrid_search::HybridSearchError>
    where
        F1: FnOnce(&str) -> Fut1,
        Fut1: std::future::Future<Output = Result<Vec<RawSearchResult>, String>>,
        F2: FnOnce(&str) -> Fut2,
        Fut2: std::future::Future<Output = Result<Vec<RawSearchResult>, String>>,
    {
        let contexts = self
            .search
            .search(requirement, vector_search, keyword_search)
            .await?;

        let mut enhanced_prompt = format!("## 需求\n{}\n\n## 相关代码上下文\n", requirement);
        for (i, ctx) in contexts.iter().enumerate() {
            enhanced_prompt.push_str(&format!(
                "### 片段 {} (score: {:.3}, source: {:?})\n```\n{}\n```\n\n",
                i + 1,
                ctx.score,
                ctx.source,
                ctx.content
            ));
        }

        let injected_count = contexts.len();

        Ok(InjectionResult {
            enhanced_prompt,
            contexts,
            injected_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_raw(doc_id: &str, score: f64) -> RawSearchResult {
        RawSearchResult {
            content: format!("fn {}() {{}}", doc_id),
            score,
            doc_id: doc_id.to_string(),
            fragment_position: 0,
            knowledge_base_id: "kb1".to_string(),
            tenant_id: "t1".to_string(),
        }
    }

    #[tokio::test]
    async fn test_inject_adds_context_to_prompt() {
        let config = HybridSearchConfig {
            rerank_enabled: false,
            cache_enabled: false,
            ..Default::default()
        };
        let injector = ContextInjector::new(config).unwrap();

        let result = injector
            .inject(
                "实现用户注册功能",
                |_| async { Ok(vec![make_raw("register", 0.9)]) },
                |_| async { Ok(vec![make_raw("user_model", 0.7)]) },
            )
            .await
            .unwrap();

        assert!(
            result.enhanced_prompt.contains("实现用户注册功能"),
            "prompt 应包含需求"
        );
        assert!(
            result.enhanced_prompt.contains("相关代码上下文"),
            "prompt 应包含上下文标记"
        );
        assert_eq!(result.injected_count, 2, "应注入 2 个片段");
        assert!(
            result.enhanced_prompt.contains("register"),
            "prompt 应包含检索到的代码"
        );
    }

    #[tokio::test]
    async fn test_inject_no_results() {
        let config = HybridSearchConfig {
            rerank_enabled: false,
            cache_enabled: false,
            ..Default::default()
        };
        let injector = ContextInjector::new(config).unwrap();

        let result = injector
            .inject(
                "无匹配需求",
                |_| async { Ok(vec![]) },
                |_| async { Ok(vec![]) },
            )
            .await
            .unwrap();

        assert_eq!(result.injected_count, 0);
        assert!(result.enhanced_prompt.contains("无匹配需求"));
    }
}
