//! 重排序器（v1.5.0 P2-3）
//!
//! 相关性二次排序；重排序超时 → 跳过返回初步结果 + 告警（spec 5.7.3 异常2）。
#![forbid(unsafe_code)]

use std::time::Duration;

use tokio::time::Instant;

/// 重排序结果项
#[derive(Debug, Clone)]
pub struct RerankItem<T> {
    /// 原始结果
    pub item: T,
    /// 重排序后的相关性分数（越高越相关）
    pub score: f64,
}

/// 重排序器
pub struct Reranker {
    /// 超时时间
    timeout: Duration,
}

/// 重排序结果
#[derive(Debug)]
pub struct RerankResult<T> {
    /// 重排序后的结果（按 score 降序）
    pub items: Vec<RerankItem<T>>,
    /// 是否因超时跳过重排序
    pub timed_out: bool,
}

impl Reranker {
    /// 创建重排序器，指定超时时间
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// 对结果进行重排序（spec 5.7.3）
    ///
    /// `scorer` 为每个结果计算相关性分数。
    /// 超时则返回原始顺序 + `timed_out = true`。
    pub async fn rerank<T, F>(&self, query: &str, items: Vec<T>, scorer: F) -> RerankResult<T>
    where
        F: Fn(&str, &T) -> f64,
    {
        let deadline = Instant::now() + self.timeout;

        let mut scored: Vec<RerankItem<T>> = items
            .into_iter()
            .map(|item| {
                let score = scorer(query, &item);
                RerankItem { item, score }
            })
            .collect();

        if Instant::now() > deadline {
            tracing::warn!("重排序超时，返回初步结果");
            return RerankResult {
                items: scored,
                timed_out: true,
            };
        }

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        RerankResult {
            items: scored,
            timed_out: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rerank_sorts_by_score() {
        let reranker = Reranker::new(Duration::from_secs(5));
        let items = vec!["low", "high", "medium"];

        let result = reranker
            .rerank("query", items, |query, item| {
                let _ = query;
                match *item {
                    "high" => 0.9,
                    "medium" => 0.5,
                    "low" => 0.1,
                    _ => 0.0,
                }
            })
            .await;

        assert!(!result.timed_out);
        assert_eq!(result.items[0].item, "high");
        assert_eq!(result.items[1].item, "medium");
        assert_eq!(result.items[2].item, "low");
    }

    #[tokio::test]
    async fn test_rerank_preserves_all_items() {
        let reranker = Reranker::new(Duration::from_secs(5));
        let items = vec![1, 2, 3, 4, 5];

        let result = reranker.rerank("q", items, |_, &i| i as f64).await;

        assert_eq!(result.items.len(), 5);
        assert_eq!(result.items[0].item, 5);
        assert_eq!(result.items[4].item, 1);
    }

    #[tokio::test]
    async fn test_rerank_timeout() {
        let reranker = Reranker::new(Duration::ZERO);
        let items = vec!["a", "b", "c"];

        let result = reranker
            .rerank("q", items, |_, item| item.len() as f64)
            .await;

        assert!(result.timed_out, "应因超时跳过");
    }
}
