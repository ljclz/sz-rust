// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 降级链 + 成本优先 + 同能力负载均衡。
//!
//! - [`FallbackChain`]：按优先级降级，首选不可达时依次尝试备选。
//! - [`cost_optimal`]：在满足能力的模型中选择成本最低的。
//! - [`RoundRobinLb`]：同能力多实例轮询负载均衡。

use crate::common::AiError;
use crate::llm::model_registry::{ModelEntry, ModelRegistry};
use crate::llm::provider::ProviderRef;
use crate::llm::router::RoutingStrategy;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 轮询负载均衡器。
///
/// 在同能力多模型实例间轮询分发请求。
///
/// ```
/// use sz_rust_ai_facade::llm::fallback::RoundRobinLb;
/// let lb = RoundRobinLb::new();
/// let items = vec!["a", "b", "c"];
/// assert_eq!(lb.pick(&items), Some(&"a"));
/// assert_eq!(lb.pick(&items), Some(&"b"));
/// assert_eq!(lb.pick(&items), Some(&"c"));
/// assert_eq!(lb.pick(&items), Some(&"a"));
/// ```
pub struct RoundRobinLb {
    counter: AtomicUsize,
}

impl Default for RoundRobinLb {
    fn default() -> Self {
        Self::new()
    }
}

impl RoundRobinLb {
    /// 创建轮询负载均衡器。
    pub fn new() -> Self {
        Self {
            counter: AtomicUsize::new(0),
        }
    }

    /// 从列表中轮询选择一个元素。
    pub fn pick<'a, T>(&self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % items.len();
        Some(&items[idx])
    }
}

/// 在满足能力的模型中选择成本最低的。
///
/// 成本比较基于 `input_per_1k + output_per_1k` 之和。
pub fn cost_optimal<'a>(candidates: &'a [&'a ModelEntry]) -> Option<&'a ModelEntry> {
    candidates.iter().copied().min_by(|a, b| {
        let cost_a = a.cost.input_per_1k + a.cost.output_per_1k;
        let cost_b = b.cost.input_per_1k + b.cost.output_per_1k;
        cost_a
            .partial_cmp(&cost_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// 降级链路由结果。
#[derive(Clone)]
pub struct FallbackResult {
    pub provider: ProviderRef,
    pub model_name: String,
    pub used_fallback: bool,
    pub fallback_chain: Vec<String>,
}

/// 降级链路由器。
///
/// 按优先级降级：首选模型不可达 → 按优先级尝试备选 + 告警。
pub struct FallbackChain {
    registry: Arc<ModelRegistry>,
    lb: Arc<RoundRobinLb>,
    unavailable: Mutex<std::collections::HashSet<String>>,
}

impl FallbackChain {
    /// 创建降级链路由器。
    pub fn new(registry: Arc<ModelRegistry>) -> Self {
        Self {
            registry,
            lb: Arc::new(RoundRobinLb::new()),
            unavailable: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// 标记模型不可用（模拟故障降级）。
    pub fn mark_unavailable(&self, name: &str) {
        self.unavailable.lock().insert(name.to_string());
    }

    /// 标记模型恢复可用。
    pub fn mark_available(&self, name: &str) {
        self.unavailable.lock().remove(name);
    }

    /// 按策略路由到具备指定能力的模型。
    ///
    /// - `Capability`：按优先级选择第一个可用模型。
    /// - `Cost`：在可用模型中选择成本最低的。
    /// - `Fallback`：按优先级降级链依次尝试。
    /// - `Explicit`：不应通过此方法调用。
    pub fn route(
        &self,
        capability: &str,
        strategy: RoutingStrategy,
    ) -> Result<FallbackResult, AiError> {
        let candidates = self.registry.query_by_capability(capability);
        if candidates.is_empty() {
            return Err(AiError::ProviderUnavailable(format!(
                "no model with capability '{}'",
                capability
            )));
        }

        let available: Vec<&ModelEntry> = candidates
            .iter()
            .filter(|e| !self.unavailable.lock().contains(&e.name))
            .collect();

        if available.is_empty() {
            let chain: Vec<String> = candidates.iter().map(|e| e.name.clone()).collect();
            return Err(AiError::ProviderUnavailable(format!(
                "all models with capability '{}' unavailable, chain: {:?}",
                capability, chain
            )));
        }

        let full_chain: Vec<String> = candidates.iter().map(|e| e.name.clone()).collect();

        let selected = match strategy {
            RoutingStrategy::Cost => {
                cost_optimal(&available).expect("available 已验证非空，cost_optimal 必返回条目")
            }
            RoutingStrategy::Capability | RoutingStrategy::Fallback => {
                let names: Vec<&str> = available.iter().map(|e| e.name.as_str()).collect();
                let idx = self
                    .lb
                    .pick(&names)
                    .expect("names 来自非空 available，pick 必返回条目");
                available
                    .iter()
                    .find(|e| e.name == *idx)
                    .expect("pick 返回的名字必来自 names，find 必命中")
            }
            RoutingStrategy::Explicit => available
                .first()
                .expect("available 已验证非空，first 必返回条目"),
        };

        let used_fallback = selected.name != full_chain[0];
        Ok(FallbackResult {
            provider: selected.provider.clone(),
            model_name: selected.name.clone(),
            used_fallback,
            fallback_chain: full_chain,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::model_registry::{ModelCost, ModelEntry};
    use crate::llm::test_support::MockProvider;

    fn make_entry(name: &str, caps: &[&str], priority: i32, cost: f64) -> ModelEntry {
        ModelEntry {
            name: name.to_string(),
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            priority,
            cost: ModelCost {
                input_per_1k: cost,
                output_per_1k: cost,
            },
            provider: Arc::new(MockProvider) as ProviderRef,
        }
    }

    fn make_registry() -> ModelRegistry {
        let reg = ModelRegistry::new();
        reg.register(make_entry("kat-coder", &["codegen"], 100, 0.05));
        reg.register(make_entry("deepseek", &["codegen", "chat"], 50, 0.01));
        reg.register(make_entry("openai", &["chat"], 80, 0.03));
        reg
    }

    #[test]
    fn round_robin_basic() {
        let lb = RoundRobinLb::new();
        let items = vec!["a", "b", "c"];
        assert_eq!(lb.pick(&items), Some(&"a"));
        assert_eq!(lb.pick(&items), Some(&"b"));
        assert_eq!(lb.pick(&items), Some(&"c"));
        assert_eq!(lb.pick(&items), Some(&"a"));
    }

    #[test]
    fn round_robin_empty() {
        let lb = RoundRobinLb::new();
        let items: Vec<&str> = vec![];
        assert_eq!(lb.pick(&items), None);
    }

    #[test]
    fn cost_optimal_picks_cheapest() {
        let entries = [
            make_entry("expensive", &["chat"], 10, 0.10),
            make_entry("cheap", &["chat"], 5, 0.01),
            make_entry("mid", &["chat"], 8, 0.05),
        ];
        let refs: Vec<&ModelEntry> = entries.iter().collect();
        let result = cost_optimal(&refs).unwrap();
        assert_eq!(result.name, "cheap");
    }

    #[test]
    fn cost_optimal_empty() {
        let refs: Vec<&ModelEntry> = vec![];
        assert!(cost_optimal(&refs).is_none());
    }

    #[test]
    fn fallback_route_capability() {
        let reg = Arc::new(make_registry());
        let chain = FallbackChain::new(reg);
        let result = chain.route("codegen", RoutingStrategy::Capability).unwrap();
        assert!(result.fallback_chain.contains(&"kat-coder".to_string()));
        assert!(result.fallback_chain.contains(&"deepseek".to_string()));
    }

    #[test]
    fn fallback_route_cost() {
        let reg = Arc::new(make_registry());
        let chain = FallbackChain::new(reg);
        let result = chain.route("codegen", RoutingStrategy::Cost).unwrap();
        assert_eq!(result.model_name, "deepseek");
    }

    #[test]
    fn fallback_degrades_when_primary_unavailable() {
        let reg = Arc::new(make_registry());
        let chain = FallbackChain::new(reg);
        chain.mark_unavailable("kat-coder");
        let result = chain.route("codegen", RoutingStrategy::Capability).unwrap();
        assert_eq!(result.model_name, "deepseek");
        assert!(result.used_fallback);
    }

    #[test]
    fn fallback_all_unavailable_errors() {
        let reg = Arc::new(make_registry());
        let chain = FallbackChain::new(reg);
        chain.mark_unavailable("kat-coder");
        chain.mark_unavailable("deepseek");
        let result = chain.route("codegen", RoutingStrategy::Capability);
        assert!(result.is_err());
        assert_eq!(
            result.err().unwrap().error_code(),
            "AI_PROVIDER_UNAVAILABLE"
        );
    }

    #[test]
    fn fallback_no_capability_errors() {
        let reg = Arc::new(make_registry());
        let chain = FallbackChain::new(reg);
        let result = chain.route("embedding", RoutingStrategy::Capability);
        assert!(result.is_err());
        assert_eq!(
            result.err().unwrap().error_code(),
            "AI_PROVIDER_UNAVAILABLE"
        );
    }

    #[test]
    fn fallback_mark_available_restores() {
        let reg = Arc::new(make_registry());
        let chain = FallbackChain::new(reg);
        chain.mark_unavailable("kat-coder");
        assert!(chain.route("codegen", RoutingStrategy::Cost).is_ok());
        chain.mark_available("kat-coder");
        let result = chain.route("codegen", RoutingStrategy::Cost).unwrap();
        assert_eq!(result.model_name, "deepseek");
    }

    #[test]
    fn fallback_mark_available_actually_restores() {
        let reg = Arc::new(make_registry());
        let chain = FallbackChain::new(reg);
        chain.mark_unavailable("kat-coder");
        chain.mark_unavailable("deepseek");
        assert!(chain.route("codegen", RoutingStrategy::Cost).is_err());
        chain.mark_available("kat-coder");
        let result = chain.route("codegen", RoutingStrategy::Cost).unwrap();
        assert_eq!(result.model_name, "kat-coder");
    }

    #[test]
    fn lb_distributes_across_instances() {
        let reg = Arc::new(make_registry());
        let chain = FallbackChain::new(reg);
        let mut selections = std::collections::HashSet::new();
        for _ in 0..10 {
            let result = chain.route("chat", RoutingStrategy::Capability).unwrap();
            selections.insert(result.model_name);
        }
        assert!(
            selections.len() >= 2,
            "LB should distribute to multiple instances"
        );
    }
}
