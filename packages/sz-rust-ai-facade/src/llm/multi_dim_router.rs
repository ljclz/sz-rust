//! 多维度模型路由（v1.5.0 P2-2）
//!
//! 多维度加权路由 + 多级故障转移链 + 负载均衡 + 成本统计。
//!
//! 评分公式：score = w_task * task_match + w_load * (1 - norm_load)
//!                 + w_latency * (1 - norm_latency) + w_cost * (1 - norm_cost)
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_ai_facade::llm::multi_dim_router::{MultiDimRouter, MultiDimWeights};
//!
//! let router = MultiDimRouter::new(weights, failover_chain)?;
//! let model = router.route(&metrics).await?;
//! ```
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use parking_lot::Mutex;

use crate::common::error::AiError;

/// 多维度权重（spec 6.3.2：和为 1 且均非负）
#[derive(Debug, Clone)]
pub struct MultiDimWeights {
    /// 任务匹配权重
    pub task: f64,
    /// 负载权重
    pub load: f64,
    /// 延迟权重
    pub latency: f64,
    /// 成本权重
    pub cost: f64,
}

impl Default for MultiDimWeights {
    fn default() -> Self {
        Self {
            task: 0.4,
            load: 0.2,
            latency: 0.2,
            cost: 0.2,
        }
    }
}

impl MultiDimWeights {
    /// 校验权重合法性（和为 1 且均非负，spec 6.3.2）
    pub fn validate(&self) -> Result<(), AiError> {
        if self.task < 0.0 || self.load < 0.0 || self.latency < 0.0 || self.cost < 0.0 {
            return Err(AiError::ConfigInvalid("权重不能为负数".to_string()));
        }
        let sum = self.task + self.load + self.latency + self.cost;
        if (sum - 1.0).abs() > 1e-6 {
            return Err(AiError::ConfigInvalid(format!(
                "权重和应为 1，实际为 {}",
                sum
            )));
        }
        Ok(())
    }
}

/// 模型运行时指标
#[derive(Debug, Clone, Default)]
pub struct ModelMetrics {
    /// 任务匹配度（0.0-1.0）
    pub task_match: f64,
    /// 归一化负载（0.0=空闲, 1.0=满载）
    pub norm_load: f64,
    /// 归一化延迟（0.0=快, 1.0=慢）
    pub norm_latency: f64,
    /// 归一化成本（0.0=便宜, 1.0=贵）
    pub norm_cost: f64,
}

/// 负载均衡策略
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadBalanceStrategy {
    /// 轮询
    RoundRobin,
    /// 加权
    Weighted,
    /// 最少连接
    LeastConnections,
}

/// 成本统计记录
#[derive(Debug, Clone, Default)]
pub struct CostRecord {
    /// 调用次数
    pub call_count: u64,
    /// Token 总数
    pub total_tokens: u64,
    /// 累计成本
    pub total_cost: f64,
}

/// 成本追踪器（spec 5.6.7）
pub struct CostTracker {
    records: Mutex<HashMap<String, CostRecord>>,
}

impl CostTracker {
    pub fn new() -> Self {
        Self {
            records: Mutex::new(HashMap::new()),
        }
    }

    /// 记录一次调用
    pub fn record(&self, model: &str, tokens: u64, cost: f64) {
        let mut records = self.records.lock();
        let entry = records.entry(model.to_string()).or_default();
        entry.call_count = entry.call_count.saturating_add(1);
        entry.total_tokens = entry.total_tokens.saturating_add(tokens);
        entry.total_cost += cost;
    }

    /// 获取模型的成本记录
    pub fn get(&self, model: &str) -> Option<CostRecord> {
        self.records.lock().get(model).cloned()
    }

    /// 获取所有模型的成本记录
    pub fn all(&self) -> HashMap<String, CostRecord> {
        self.records.lock().clone()
    }

    /// 重置所有记录（spec 5.6.3 异常3：溢出时自动重置）
    pub fn reset(&self) {
        self.records.lock().clear();
    }
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// 故障转移链（spec 6.3.3：无重复模型）
#[derive(Debug, Clone)]
pub struct FailoverChain {
    /// 模型列表（主 → 备1 → 备2 → ...）
    models: Vec<String>,
}

impl FailoverChain {
    pub fn new(models: Vec<String>) -> Result<Self, AiError> {
        let mut seen = std::collections::HashSet::new();
        for m in &models {
            if !seen.insert(m.clone()) {
                return Err(AiError::ConfigInvalid(format!(
                    "故障转移链中存在重复模型: {}",
                    m
                )));
            }
        }
        Ok(Self { models })
    }

    pub fn models(&self) -> &[String] {
        &self.models
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

/// 负载均衡器
pub struct LoadBalancer {
    strategy: LoadBalanceStrategy,
    counter: Mutex<HashMap<String, u64>>,
}

impl LoadBalancer {
    pub fn new(strategy: LoadBalanceStrategy) -> Self {
        Self {
            strategy,
            counter: Mutex::new(HashMap::new()),
        }
    }

    /// 从候选模型中选择一个（spec 5.6.5）
    pub fn select(&self, candidates: &[String]) -> Option<String> {
        if candidates.is_empty() {
            return None;
        }

        match self.strategy {
            LoadBalanceStrategy::RoundRobin => {
                let mut counter = self.counter.lock();
                let min_count = candidates
                    .iter()
                    .map(|m| *counter.entry(m.clone()).or_insert(0))
                    .min()
                    .unwrap_or(0);
                let selected = candidates
                    .iter()
                    .find(|m| *counter.entry(m.to_string()).or_insert(0) == min_count)
                    .cloned()?;
                *counter.entry(selected.clone()).or_insert(0) += 1;
                Some(selected)
            }
            LoadBalanceStrategy::LeastConnections => {
                let counter = self.counter.lock();
                candidates
                    .iter()
                    .min_by_key(|m| *counter.get(*m).unwrap_or(&0))
                    .cloned()
            }
            LoadBalanceStrategy::Weighted => {
                let mut counter = self.counter.lock();
                let selected = candidates
                    .iter()
                    .min_by_key(|m| *counter.get(*m).unwrap_or(&0))
                    .cloned()?;
                *counter.entry(selected.clone()).or_insert(0) += 1;
                Some(selected)
            }
        }
    }
}

/// 路由决策记录（脱敏，不含 API Key，spec 5.6.10）
#[derive(Debug, Clone, serde::Serialize)]
pub struct MultiDimRoutingRecord {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub candidates: Vec<String>,
    pub selected_model: String,
    pub scores: HashMap<String, f64>,
    pub selection_reason: String,
    pub failover_attempted: bool,
}

/// 多维度路由器
pub struct MultiDimRouter {
    weights: ArcSwap<MultiDimWeights>,
    failover_chain: FailoverChain,
    load_balancer: LoadBalancer,
    cost_tracker: CostTracker,
    records: Mutex<Vec<MultiDimRoutingRecord>>,
    max_records: usize,
}

impl MultiDimRouter {
    /// 创建多维度路由器
    pub fn new(weights: MultiDimWeights, failover_chain: FailoverChain) -> Result<Self, AiError> {
        weights.validate()?;
        if failover_chain.is_empty() {
            return Err(AiError::ConfigInvalid("故障转移链不能为空".to_string()));
        }
        Ok(Self {
            weights: ArcSwap::from_pointee(weights),
            failover_chain,
            load_balancer: LoadBalancer::new(LoadBalanceStrategy::RoundRobin),
            cost_tracker: CostTracker::new(),
            records: Mutex::new(Vec::new()),
            max_records: 1000,
        })
    }

    /// 设置负载均衡策略
    pub fn with_load_balance(mut self, strategy: LoadBalanceStrategy) -> Self {
        self.load_balancer = LoadBalancer::new(strategy);
        self
    }

    /// 设置最大记录数
    pub fn with_max_records(mut self, max: usize) -> Self {
        self.max_records = max;
        self
    }

    /// 路由选择（spec 5.6.1）
    ///
    /// 按多维度加权评分选择最优模型。
    /// 主模型失败 → 自动切换备用（spec 5.6.2）。
    pub async fn route(&self, metrics: &HashMap<String, ModelMetrics>) -> Result<String, AiError> {
        let weights = self.weights.load();
        let candidates = self.failover_chain.models();

        let mut scores: HashMap<String, f64> = HashMap::new();
        for model in candidates {
            let m = metrics.get(model).cloned().unwrap_or_default();
            let score = weights.task * m.task_match
                + weights.load * (1.0 - m.norm_load)
                + weights.latency * (1.0 - m.norm_latency)
                + weights.cost * (1.0 - m.norm_cost);
            scores.insert(model.clone(), score);
        }

        let mut sorted_candidates: Vec<String> = candidates.to_vec();
        sorted_candidates.sort_by(|a, b| {
            scores
                .get(b)
                .unwrap_or(&0.0)
                .partial_cmp(scores.get(a).unwrap_or(&0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let selected = self
            .load_balancer
            .select(&sorted_candidates)
            .ok_or_else(|| AiError::ProviderUnavailable("无可用模型".to_string()))?;

        self.record_routing(
            candidates.to_vec(),
            selected.clone(),
            scores.clone(),
            "多维度加权评分".to_string(),
            false,
        );

        Ok(selected)
    }

    /// 故障转移路由（spec 5.6.2 + 5.6.3）
    ///
    /// `failed_models` 为已失败的模型列表，按转移链跳过。
    /// 全链失败 → `AiError::ProviderUnavailable`（spec 5.6.3 异常1）
    pub async fn route_with_failover(
        &self,
        metrics: &HashMap<String, ModelMetrics>,
        failed_models: &[String],
    ) -> Result<String, AiError> {
        let candidates: Vec<String> = self
            .failover_chain
            .models()
            .iter()
            .filter(|m| !failed_models.contains(m))
            .cloned()
            .collect();

        if candidates.is_empty() {
            return Err(AiError::ProviderUnavailable(
                "所有模型均失败，故障转移链耗尽".to_string(),
            ));
        }

        let weights = self.weights.load();
        let mut scores: HashMap<String, f64> = HashMap::new();
        for model in &candidates {
            let m = metrics.get(model).cloned().unwrap_or_default();
            let score = weights.task * m.task_match
                + weights.load * (1.0 - m.norm_load)
                + weights.latency * (1.0 - m.norm_latency)
                + weights.cost * (1.0 - m.norm_cost);
            scores.insert(model.clone(), score);
        }

        let mut sorted_candidates = candidates.clone();
        sorted_candidates.sort_by(|a, b| {
            scores
                .get(b)
                .unwrap_or(&0.0)
                .partial_cmp(scores.get(a).unwrap_or(&0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let selected = self
            .load_balancer
            .select(&sorted_candidates)
            .ok_or_else(|| AiError::ProviderUnavailable("无可用模型".to_string()))?;

        self.record_routing(
            candidates,
            selected.clone(),
            scores,
            "故障转移路由".to_string(),
            !failed_models.is_empty(),
        );

        Ok(selected)
    }

    /// 热更新权重（spec 5.6.8）
    pub fn update_weights(&self, weights: MultiDimWeights) -> Result<(), AiError> {
        weights.validate()?;
        self.weights.store(Arc::new(weights));
        Ok(())
    }

    /// 获取当前权重
    pub fn weights(&self) -> MultiDimWeights {
        (**self.weights.load()).clone()
    }

    /// 记录调用成本（spec 5.6.7）
    pub fn record_cost(&self, model: &str, tokens: u64, cost: f64) {
        self.cost_tracker.record(model, tokens, cost);
    }

    /// 获取成本统计
    pub fn cost_records(&self) -> HashMap<String, CostRecord> {
        self.cost_tracker.all()
    }

    /// 获取路由记录
    pub fn routing_records(&self) -> Vec<MultiDimRoutingRecord> {
        self.records.lock().clone()
    }

    fn record_routing(
        &self,
        candidates: Vec<String>,
        selected: String,
        scores: HashMap<String, f64>,
        reason: String,
        failover: bool,
    ) {
        let rec = MultiDimRoutingRecord {
            timestamp: chrono::Utc::now(),
            candidates,
            selected_model: selected,
            scores,
            selection_reason: reason,
            failover_attempted: failover,
        };
        let mut records = self.records.lock();
        records.push(rec);
        if records.len() > self.max_records {
            records.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weights_validate_valid() {
        let w = MultiDimWeights::default();
        assert!(w.validate().is_ok());
    }

    #[test]
    fn test_weights_validate_negative() {
        let w = MultiDimWeights {
            task: -0.1,
            load: 0.3,
            latency: 0.4,
            cost: 0.4,
        };
        assert!(w.validate().is_err());
    }

    #[test]
    fn test_weights_validate_sum_not_one() {
        let w = MultiDimWeights {
            task: 0.3,
            load: 0.3,
            latency: 0.3,
            cost: 0.3,
        };
        assert!(w.validate().is_err());
    }

    #[test]
    fn test_failover_chain_no_duplicates() {
        let chain = FailoverChain::new(vec!["model_a".to_string(), "model_b".to_string()]);
        assert!(chain.is_ok());
    }

    #[test]
    fn test_failover_chain_rejects_duplicates() {
        let chain = FailoverChain::new(vec!["model_a".to_string(), "model_a".to_string()]);
        assert!(chain.is_err());
    }

    #[tokio::test]
    async fn test_route_selects_best_model() {
        let chain = FailoverChain::new(vec!["model_a".to_string(), "model_b".to_string()]).unwrap();
        let router = MultiDimRouter::new(MultiDimWeights::default(), chain).unwrap();

        let mut metrics = HashMap::new();
        metrics.insert(
            "model_a".to_string(),
            ModelMetrics {
                task_match: 0.9,
                norm_load: 0.1,
                norm_latency: 0.1,
                norm_cost: 0.1,
            },
        );
        metrics.insert(
            "model_b".to_string(),
            ModelMetrics {
                task_match: 0.5,
                norm_load: 0.5,
                norm_latency: 0.5,
                norm_cost: 0.5,
            },
        );

        let selected = router.route(&metrics).await.unwrap();
        assert_eq!(selected, "model_a", "应选择分数更高的模型");
    }

    #[tokio::test]
    async fn test_failover_skips_failed_models() {
        let chain = FailoverChain::new(vec![
            "primary".to_string(),
            "backup1".to_string(),
            "backup2".to_string(),
        ])
        .unwrap();
        let router = MultiDimRouter::new(MultiDimWeights::default(), chain).unwrap();

        let metrics = HashMap::new();
        let selected = router
            .route_with_failover(&metrics, &["primary".to_string(), "backup1".to_string()])
            .await
            .unwrap();
        assert_eq!(selected, "backup2", "应跳过失败模型选择 backup2");
    }

    #[tokio::test]
    async fn test_all_models_failed_returns_error() {
        let chain = FailoverChain::new(vec!["primary".to_string(), "backup".to_string()]).unwrap();
        let router = MultiDimRouter::new(MultiDimWeights::default(), chain).unwrap();

        let metrics = HashMap::new();
        let result = router
            .route_with_failover(&metrics, &["primary".to_string(), "backup".to_string()])
            .await;
        assert!(result.is_err(), "全链失败应返回错误");
    }

    #[tokio::test]
    async fn test_hot_update_weights() {
        let chain = FailoverChain::new(vec!["a".to_string(), "b".to_string()]).unwrap();
        let router = MultiDimRouter::new(MultiDimWeights::default(), chain).unwrap();

        let new_weights = MultiDimWeights {
            task: 0.7,
            load: 0.1,
            latency: 0.1,
            cost: 0.1,
        };
        router.update_weights(new_weights).unwrap();
        let current = router.weights();
        assert!((current.task - 0.7).abs() < 1e-9);
    }

    #[test]
    fn test_cost_tracker() {
        let tracker = CostTracker::new();
        tracker.record("model_a", 100, 0.05);
        tracker.record("model_a", 200, 0.10);
        tracker.record("model_b", 50, 0.02);

        let a = tracker.get("model_a").unwrap();
        assert_eq!(a.call_count, 2);
        assert_eq!(a.total_tokens, 300);
        assert!((a.total_cost - 0.15).abs() < 1e-9);

        let b = tracker.get("model_b").unwrap();
        assert_eq!(b.call_count, 1);
    }

    #[tokio::test]
    async fn test_routing_record_no_api_key() {
        let chain = FailoverChain::new(vec!["model_a".to_string()]).unwrap();
        let router = MultiDimRouter::new(MultiDimWeights::default(), chain).unwrap();

        let metrics = HashMap::new();
        router.route(&metrics).await.unwrap();

        let records = router.routing_records();
        assert_eq!(records.len(), 1);
        let json = serde_json::to_string(&records[0]).unwrap();
        assert!(!json.contains("api_key"), "路由记录不应包含 API Key");
        assert!(!json.contains("secret"), "路由记录不应包含 secret");
    }

    #[test]
    fn test_load_balancer_round_robin() {
        let lb = LoadBalancer::new(LoadBalanceStrategy::RoundRobin);
        let candidates = vec!["a".to_string(), "b".to_string(), "c".to_string()];

        let s1 = lb.select(&candidates).unwrap();
        let s2 = lb.select(&candidates).unwrap();
        let s3 = lb.select(&candidates).unwrap();

        assert_eq!(s1, "a");
        assert_eq!(s2, "b");
        assert_eq!(s3, "c");
    }
}
