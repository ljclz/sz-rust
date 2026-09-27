//! 混合检索（v1.5.0 P2-3）
//!
//! 向量检索 + 关键词检索加权融合 → 截取 top-k（spec 5.7.1）。
//! 支持重排序、缓存、来源引用、知识库隔离。
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_rag::hybrid_search::{HybridSearch, HybridSearchConfig};
//! use std::time::Duration;
//!
//! let config = HybridSearchConfig::default();
//! let search = HybridSearch::new(config);
//! let results = search.search("query", vector_search_fn, keyword_search_fn).await?;
//! ```
#![forbid(unsafe_code)]

use std::time::Duration;

use crate::reranker::Reranker;
use crate::search_cache::SearchCache;
use crate::source_citation::SourceCitation;

/// 检索来源
#[derive(Debug, Clone, PartialEq)]
pub enum SearchSource {
    /// 向量检索
    Vector,
    /// 关键词检索
    Keyword,
    /// 混合融合
    Hybrid,
}

/// 混合检索结果项
#[derive(Debug, Clone, PartialEq)]
pub struct HybridSearchResult {
    /// 内容
    pub content: String,
    /// 融合后的相关性分数（0.0-1.0）
    pub score: f64,
    /// 来源引用
    pub citation: SourceCitation,
    /// 检索来源
    pub source: SearchSource,
    /// 文档 ID（用于知识库隔离过滤）
    pub doc_id: String,
    /// 知识库 ID
    pub knowledge_base_id: String,
    /// 租户 ID
    pub tenant_id: String,
}

/// 混合检索配置
#[derive(Debug, Clone)]
pub struct HybridSearchConfig {
    /// 向量检索权重（spec 6.4.2：权重和为 1 且均非负）
    pub vector_weight: f64,
    /// 关键词检索权重
    pub keyword_weight: f64,
    /// 返回 top-k 结果
    pub topk: usize,
    /// 是否启用重排序
    pub rerank_enabled: bool,
    /// 是否启用缓存
    pub cache_enabled: bool,
    /// 缓存 TTL（0 不缓存，spec 5.7.8）
    pub cache_ttl: Duration,
    /// 重排序超时
    pub rerank_timeout: Duration,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            vector_weight: 0.7,
            keyword_weight: 0.3,
            topk: 10,
            rerank_enabled: true,
            cache_enabled: true,
            cache_ttl: Duration::from_secs(300),
            rerank_timeout: Duration::from_secs(2),
        }
    }
}

impl HybridSearchConfig {
    /// 校验权重合法性（和为 1 且均非负，spec 6.4.2）
    pub fn validate(&self) -> Result<(), HybridSearchError> {
        if self.vector_weight < 0.0 || self.keyword_weight < 0.0 {
            return Err(HybridSearchError::InvalidConfig(
                "权重不能为负数".to_string(),
            ));
        }
        let sum = self.vector_weight + self.keyword_weight;
        if (sum - 1.0).abs() > 1e-6 {
            return Err(HybridSearchError::InvalidConfig(format!(
                "权重和应为 1，实际为 {}",
                sum
            )));
        }
        Ok(())
    }
}

/// 混合检索错误
#[derive(Debug, thiserror::Error)]
pub enum HybridSearchError {
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    #[error("vector search failed: {0}")]
    VectorSearchFailed(String),
    #[error("keyword search failed: {0}")]
    KeywordSearchFailed(String),
}

/// 混合检索器
pub struct HybridSearch {
    config: HybridSearchConfig,
    cache: SearchCache<Vec<HybridSearchResult>>,
    reranker: Reranker,
}

/// 检索结果（原始中间表示，供闭包返回）
#[derive(Debug, Clone)]
pub struct RawSearchResult {
    /// 内容文本
    pub content: String,
    /// 原始相关性分数（0.0-1.0）
    pub score: f64,
    /// 文档 ID
    pub doc_id: String,
    /// 片段位置
    pub fragment_position: usize,
    /// 知识库 ID
    pub knowledge_base_id: String,
    /// 租户 ID
    pub tenant_id: String,
}

impl HybridSearch {
    /// 创建混合检索器
    pub fn new(config: HybridSearchConfig) -> Result<Self, HybridSearchError> {
        config.validate()?;
        let cache = SearchCache::new(if config.cache_enabled {
            config.cache_ttl
        } else {
            Duration::ZERO
        });
        let reranker = Reranker::new(config.rerank_timeout);
        Ok(Self {
            config,
            cache,
            reranker,
        })
    }

    /// 执行混合检索（spec 5.7.1）
    ///
    /// `vector_search` 和 `keyword_search` 为检索函数，返回原始结果。
    /// 向量 DB 不可用 → 降级仅关键词检索 + 告警（spec 5.7.3 异常1）。
    pub async fn search<F1, Fut1, F2, Fut2>(
        &self,
        query: &str,
        vector_search: F1,
        keyword_search: F2,
    ) -> Result<Vec<HybridSearchResult>, HybridSearchError>
    where
        F1: FnOnce(&str) -> Fut1,
        Fut1: std::future::Future<Output = Result<Vec<RawSearchResult>, String>>,
        F2: FnOnce(&str) -> Fut2,
        Fut2: std::future::Future<Output = Result<Vec<RawSearchResult>, String>>,
    {
        if let Some(cached) = self.cache.get(query) {
            return Ok((*cached).clone());
        }

        let vector_results = match vector_search(query).await {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("向量检索失败，降级仅关键词检索: {}", e);
                Vec::new()
            }
        };

        let keyword_results = keyword_search(query)
            .await
            .map_err(HybridSearchError::KeywordSearchFailed)?;

        let mut fused = self.fuse_results(vector_results, keyword_results);

        if self.config.rerank_enabled && !fused.is_empty() {
            let rerank_result = self
                .reranker
                .rerank(query, fused, |q, r| {
                    let _ = q;
                    r.score
                })
                .await;
            fused = rerank_result.items.into_iter().map(|ri| ri.item).collect();
        }

        fused.truncate(self.config.topk);

        self.cache.set(query, fused.clone());

        Ok(fused)
    }

    fn fuse_results(
        &self,
        vector_results: Vec<RawSearchResult>,
        keyword_results: Vec<RawSearchResult>,
    ) -> Vec<HybridSearchResult> {
        let mut fused: Vec<HybridSearchResult> = Vec::new();

        for r in vector_results {
            fused.push(HybridSearchResult {
                content: r.content,
                score: r.score * self.config.vector_weight,
                citation: SourceCitation::new(&r.doc_id, r.fragment_position, r.score),
                source: SearchSource::Vector,
                doc_id: r.doc_id,
                knowledge_base_id: r.knowledge_base_id,
                tenant_id: r.tenant_id,
            });
        }

        for r in keyword_results {
            fused.push(HybridSearchResult {
                content: r.content,
                score: r.score * self.config.keyword_weight,
                citation: SourceCitation::new(&r.doc_id, r.fragment_position, r.score),
                source: SearchSource::Keyword,
                doc_id: r.doc_id,
                knowledge_base_id: r.knowledge_base_id,
                tenant_id: r.tenant_id,
            });
        }

        fused.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        fused
    }

    /// 清空缓存
    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_raw(doc_id: &str, score: f64) -> RawSearchResult {
        RawSearchResult {
            content: format!("content_{}", doc_id),
            score,
            doc_id: doc_id.to_string(),
            fragment_position: 0,
            knowledge_base_id: "kb1".to_string(),
            tenant_id: "t1".to_string(),
        }
    }

    #[tokio::test]
    async fn test_hybrid_search_fuses_vector_and_keyword() {
        let config = HybridSearchConfig {
            rerank_enabled: false,
            cache_enabled: false,
            ..Default::default()
        };
        let search = HybridSearch::new(config).unwrap();

        let results = search
            .search(
                "query",
                |_| async { Ok(vec![make_raw("v1", 0.9), make_raw("v2", 0.7)]) },
                |_| async { Ok(vec![make_raw("k1", 0.8)]) },
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 3, "应融合向量+关键词结果");
        assert!(results[0].score >= results[1].score, "应按分数降序");
    }

    #[tokio::test]
    async fn test_vector_degradation_on_failure() {
        let config = HybridSearchConfig {
            rerank_enabled: false,
            cache_enabled: false,
            ..Default::default()
        };
        let search = HybridSearch::new(config).unwrap();

        let results = search
            .search(
                "query",
                |_| async { Err("vector DB unavailable".to_string()) },
                |_| async { Ok(vec![make_raw("k1", 0.8)]) },
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1, "向量失败应降级仅关键词");
        assert_eq!(results[0].source, SearchSource::Keyword);
    }

    #[tokio::test]
    async fn test_cache_hit() {
        let config = HybridSearchConfig {
            rerank_enabled: false,
            cache_enabled: true,
            cache_ttl: Duration::from_secs(60),
            ..Default::default()
        };
        let search = HybridSearch::new(config).unwrap();

        let r1 = search
            .search(
                "query",
                |_| async { Ok(vec![]) },
                |_| async { Ok(vec![make_raw("k1", 0.8)]) },
            )
            .await
            .unwrap();
        let r2 = search
            .search(
                "query",
                |_| async { Ok(vec![make_raw("v1", 0.9)]) },
                |_| async { Ok(vec![make_raw("k2", 0.9)]) },
            )
            .await
            .unwrap();

        assert_eq!(r1, r2, "第二次应命中缓存");
    }

    #[test]
    fn test_config_validate_valid() {
        let config = HybridSearchConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_negative_weight() {
        let config = HybridSearchConfig {
            vector_weight: -0.1,
            keyword_weight: 1.1,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_sum_not_one() {
        let config = HybridSearchConfig {
            vector_weight: 0.5,
            keyword_weight: 0.4,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[tokio::test]
    async fn test_topk_truncation() {
        let config = HybridSearchConfig {
            topk: 2,
            rerank_enabled: false,
            cache_enabled: false,
            ..Default::default()
        };
        let search = HybridSearch::new(config).unwrap();

        let results = search
            .search(
                "query",
                |_| async {
                    Ok(vec![
                        make_raw("v1", 0.9),
                        make_raw("v2", 0.8),
                        make_raw("v3", 0.7),
                    ])
                },
                |_| async { Ok(vec![make_raw("k1", 0.6), make_raw("k2", 0.5)]) },
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 2, "应截取 top-2");
    }

    #[tokio::test]
    async fn test_result_has_citation() {
        let config = HybridSearchConfig {
            rerank_enabled: false,
            cache_enabled: false,
            ..Default::default()
        };
        let search = HybridSearch::new(config).unwrap();

        let results = search
            .search(
                "query",
                |_| async { Ok(vec![]) },
                |_| async { Ok(vec![make_raw("doc1", 0.8)]) },
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].citation.doc_id, "doc1");
        assert!((results[0].citation.confidence - 0.8).abs() < 1e-9);
    }
}
