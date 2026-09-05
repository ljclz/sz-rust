// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! RAG 重排序：Reranker trait + NoopReranker + CrossEncoderReranker
//!
//! 任务组 11：AIM-6 RAG 重排序
//! - 11.1 Reranker trait + NoopReranker 兜底实现
//! - 11.2 CrossEncoderReranker（Cohere Rerank API），feature gate `reranker`

use crate::common::AiError;
use crate::embedding::VectorHit;
use async_trait::async_trait;
use std::sync::Arc;

/// 重排序器 trait
///
/// 在向量检索（retrieve）之后、上下文组装（assemble）之前对候选文档重排序，
/// 提升最终引用质量。实现可以是 Noop（直接透传）、CrossEncoder（HTTP API）等。
#[async_trait]
pub trait Reranker: Send + Sync {
    /// 重排序器名称（用于日志/审计）
    fn name(&self) -> &str;

    /// 对候选文档重排序
    ///
    /// - `query`: 用户查询文本
    /// - `candidates`: 向量检索返回的候选文档（已按相似度降序）
    /// - `topk`: 返回前 topk 个文档
    ///
    /// 返回重排序后的文档列表（长度 <= topk），score 字段更新为重排序分数
    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<VectorHit>,
        topk: usize,
    ) -> Result<Vec<VectorHit>, AiError>;
}

/// 空操作重排序器：直接返回原序前 topk 个
///
/// 作为兜底实现，当未配置真实 reranker 时使用，不改变候选顺序。
pub struct NoopReranker;

impl NoopReranker {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoopReranker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Reranker for NoopReranker {
    fn name(&self) -> &str {
        "noop-reranker"
    }

    async fn rerank(
        &self,
        _query: &str,
        candidates: Vec<VectorHit>,
        topk: usize,
    ) -> Result<Vec<VectorHit>, AiError> {
        Ok(candidates.into_iter().take(topk).collect())
    }
}

/// 基于分数加权重排序器（无外部依赖，用于测试/演示）
///
/// 结合原始向量分数与文本长度归一化分数，按 `alpha * vec_score + (1 - alpha) * len_score` 重排序。
/// 不调用外部 API，适合作为 feature gate 关闭时的轻量替代。
pub struct WeightedReranker {
    /// 向量分数权重（0.0 ~ 1.0），默认 0.7
    pub alpha: f32,
}

impl WeightedReranker {
    pub fn new(alpha: f32) -> Self {
        Self {
            alpha: alpha.clamp(0.0, 1.0),
        }
    }
}

impl Default for WeightedReranker {
    fn default() -> Self {
        Self::new(0.7)
    }
}

#[async_trait]
impl Reranker for WeightedReranker {
    fn name(&self) -> &str {
        "weighted-reranker"
    }

    async fn rerank(
        &self,
        _query: &str,
        mut candidates: Vec<VectorHit>,
        topk: usize,
    ) -> Result<Vec<VectorHit>, AiError> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let max_len = candidates.iter().map(|h| h.text.len()).max().unwrap_or(1) as f32;
        let min_len = candidates.iter().map(|h| h.text.len()).min().unwrap_or(0) as f32;
        let len_range = (max_len - min_len).max(1.0);

        for hit in candidates.iter_mut() {
            let len_score = (hit.text.len() as f32 - min_len) / len_range;
            hit.score = self.alpha * hit.score + (1.0 - self.alpha) * len_score;
        }

        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(candidates.into_iter().take(topk).collect())
    }
}

/// CrossEncoder 重排序器（Cohere Rerank API）
///
/// 调用 Cohere Rerank API 对候选文档重排序。需要 feature gate `reranker` 启用。
///
/// # 环境变量
/// - `COHERE_API_KEY`: Cohere API Key（必填）
///
/// # 示例
/// ```no_run
/// use sz_rust_ai_facade::rag::reranker::CrossEncoderReranker;
/// let reranker = CrossEncoderReranker::new("https://api.cohere.ai/v1/rerank", "your-api-key");
/// ```
#[cfg(feature = "reranker")]
pub struct CrossEncoderReranker {
    endpoint: String,
    api_key: String,
    model: String,
    http: reqwest::Client,
}

#[cfg(feature = "reranker")]
impl CrossEncoderReranker {
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            model: "rerank-english-v3.0".to_string(),
            http: reqwest::Client::new(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

#[cfg(feature = "reranker")]
#[async_trait]
impl Reranker for CrossEncoderReranker {
    fn name(&self) -> &str {
        "cross-encoder-cohere"
    }

    async fn rerank(
        &self,
        query: &str,
        candidates: Vec<VectorHit>,
        topk: usize,
    ) -> Result<Vec<VectorHit>, AiError> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let documents: Vec<&str> = candidates.iter().map(|h| h.text.as_str()).collect();

        let body = serde_json::json!({
            "model": self.model,
            "query": query,
            "documents": documents,
            "top_n": topk,
            "return_documents": false,
        });

        let resp = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Internal(format!("reranker HTTP request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AiError::Internal(format!(
                "reranker API returned {status}: {text}"
            )));
        }

        #[derive(serde::Deserialize)]
        struct RerankResponse {
            results: Vec<RerankResult>,
        }

        #[derive(serde::Deserialize)]
        struct RerankResult {
            index: usize,
            relevance_score: f32,
        }

        let rerank_resp: RerankResponse = resp
            .json()
            .await
            .map_err(|e| AiError::Internal(format!("reranker response parse failed: {e}")))?;

        let mut reranked: Vec<VectorHit> = Vec::with_capacity(rerank_resp.results.len());
        for result in rerank_resp.results {
            if result.index < candidates.len() {
                let mut hit = candidates[result.index].clone();
                hit.score = result.relevance_score;
                reranked.push(hit);
            }
        }

        Ok(reranked)
    }
}

/// 构建默认 reranker 的便捷函数
///
/// - feature `reranker` 启用且环境变量 `COHERE_API_KEY` 存在时返回 CrossEncoderReranker
/// - 否则返回 NoopReranker
pub fn default_reranker() -> Arc<dyn Reranker> {
    #[cfg(feature = "reranker")]
    {
        if let Ok(api_key) = std::env::var("COHERE_API_KEY") {
            if !api_key.is_empty() {
                return Arc::new(CrossEncoderReranker::new(
                    "https://api.cohere.ai/v1/rerank",
                    api_key,
                ));
            }
        }
    }
    Arc::new(NoopReranker::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hit(id: &str, score: f32, text: &str) -> VectorHit {
        VectorHit {
            id: id.into(),
            score,
            metadata: serde_json::json!({}),
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn noop_reranker_preserves_order() {
        let reranker = NoopReranker::new();
        let candidates = vec![
            make_hit("a", 0.9, "doc a"),
            make_hit("b", 0.8, "doc b"),
            make_hit("c", 0.7, "doc c"),
        ];

        let result = reranker.rerank("query", candidates, 2).await.unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].id, "a");
        assert_eq!(result[1].id, "b");
    }

    #[tokio::test]
    async fn noop_reranker_empty_candidates() {
        let reranker = NoopReranker::new();
        let result = reranker.rerank("query", vec![], 5).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn weighted_reranker_reorders_by_combined_score() {
        let reranker = WeightedReranker::new(0.5);
        let candidates = vec![
            make_hit("short-high", 0.9, "ab"),
            make_hit("long-low", 0.3, "abcdefghij"),
            make_hit("mid-mid", 0.6, "abcde"),
        ];

        let result = reranker.rerank("query", candidates, 3).await.unwrap();
        assert_eq!(result.len(), 3);
        for i in 0..result.len() - 1 {
            assert!(
                result[i].score >= result[i + 1].score,
                "results must be sorted by score descending"
            );
        }
    }

    #[tokio::test]
    async fn weighted_reranker_respects_topk() {
        let reranker = WeightedReranker::default();
        let candidates: Vec<VectorHit> = (0..10)
            .map(|i| make_hit(&format!("doc{i}"), 0.5, &format!("text{i}")))
            .collect();

        let result = reranker.rerank("query", candidates, 3).await.unwrap();
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn weighted_reranker_empty() {
        let reranker = WeightedReranker::default();
        let result = reranker.rerank("query", vec![], 5).await.unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn default_reranker_returns_noop_without_env() {
        let r = default_reranker();
        assert_eq!(r.name(), "noop-reranker");
    }
}
