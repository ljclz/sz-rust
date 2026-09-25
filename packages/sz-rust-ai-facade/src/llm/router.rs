// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 多模型路由器：显式路由 + 策略路由 + 路由追踪。
//!
//! 核心结构：
//! - [`RoutingStrategy`]：路由策略枚举（Explicit / Capability / Cost / Fallback）
//! - [`RoutingRecord`]：路由决策追踪记录（API Key 脱敏）
//! - [`ModelRouter`]：路由器，支持显式模型名 + 策略路由 + 追踪记录

use crate::common::AiError;
use crate::llm::provider::ProviderRef;
use arc_swap::ArcSwap;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// 路由策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutingStrategy {
    /// 显式模型名路由。
    Explicit,
    /// 按能力标签路由。
    Capability,
    /// 成本优先路由。
    Cost,
    /// 降级链路由。
    Fallback,
}

impl std::fmt::Display for RoutingStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Explicit => write!(f, "explicit"),
            Self::Capability => write!(f, "capability"),
            Self::Cost => write!(f, "cost"),
            Self::Fallback => write!(f, "fallback"),
        }
    }
}

/// 路由决策追踪记录。
///
/// API Key 不会出现在记录中（脱敏）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct RoutingRecord {
    /// 时间戳。
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 请求的模型名（显式路由时有值）。
    pub input_model: Option<String>,
    /// 请求的能力标签（策略路由时有值）。
    pub input_capability: Option<String>,
    /// 选中的模型名。
    pub selected_model: String,
    /// 使用的路由策略。
    pub strategy: RoutingStrategy,
    /// 决策原因。
    pub selection_reason: String,
    /// 降级链（完整候选列表）。
    pub fallback_chain: Vec<String>,
}

/// 多模型路由器。
pub struct ModelRouter {
    routes: ArcSwap<HashMap<String, ProviderRef>>,
    default_model: ArcSwap<String>,
    records: Mutex<VecDeque<RoutingRecord>>,
    max_records: usize,
}

impl ModelRouter {
    /// 创建路由器。
    ///
    /// `max_records` 为追踪记录上限（默认 1000）。
    pub fn new(routes: HashMap<String, ProviderRef>, default_model: String) -> Self {
        Self {
            routes: ArcSwap::from_pointee(routes),
            default_model: ArcSwap::from_pointee(default_model),
            records: Mutex::new(VecDeque::new()),
            max_records: 1000,
        }
    }

    /// 设置追踪记录上限。
    pub fn with_max_records(mut self, max: usize) -> Self {
        self.max_records = max;
        self
    }

    /// 显式模型名路由。
    ///
    /// `model` 为 `None` 时使用默认模型。
    pub fn route(&self, model: Option<&str>) -> Result<ProviderRef, AiError> {
        let routes = self.routes.load();
        let default_model = self.default_model.load();
        let model_name = model.unwrap_or(default_model.as_str());
        let provider = routes.get(model_name).cloned().ok_or_else(|| {
            AiError::ConfigInvalid(format!("model '{}' not found in routing table", model_name))
        });

        if provider.is_ok() {
            self.record(RoutingRecord {
                timestamp: chrono::Utc::now(),
                input_model: model.map(|s| s.to_string()),
                input_capability: None,
                selected_model: model_name.to_string(),
                strategy: RoutingStrategy::Explicit,
                selection_reason: format!("explicit route to '{}'", model_name),
                fallback_chain: vec![],
            });
        }

        provider
    }

    /// 策略路由：按能力标签从路由表中选择模型。
    ///
    /// 返回选中的 ProviderRef，并记录路由追踪。
    pub fn route_by_capability(
        &self,
        capability: &str,
        strategy: RoutingStrategy,
    ) -> Result<ProviderRef, AiError> {
        let routes = self.routes.load();
        let candidates: Vec<(String, ProviderRef)> = routes
            .iter()
            .filter(|(name, _)| name.contains(capability) || capability.is_empty())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        if candidates.is_empty() {
            return Err(AiError::ProviderUnavailable(format!(
                "no model with capability '{}'",
                capability
            )));
        }

        let selected = match strategy {
            RoutingStrategy::Cost => candidates
                .iter()
                .min_by(|a, b| a.0.cmp(&b.0))
                .expect("candidates 已验证非空，min_by 必返回条目"),
            _ => &candidates[0],
        };

        let chain: Vec<String> = candidates.iter().map(|(k, _)| k.clone()).collect();
        self.record(RoutingRecord {
            timestamp: chrono::Utc::now(),
            input_model: None,
            input_capability: Some(capability.to_string()),
            selected_model: selected.0.clone(),
            strategy,
            selection_reason: format!(
                "strategy {:?} selected '{}' for capability '{}'",
                strategy, selected.0, capability
            ),
            fallback_chain: chain,
        });

        Ok(selected.1.clone())
    }

    /// 热更新路由表。
    pub fn apply_update(&self, routes: HashMap<String, ProviderRef>, default_model: String) {
        self.routes.store(Arc::new(routes));
        self.default_model.store(Arc::new(default_model));
    }

    /// 当前默认模型名。
    pub fn default_model(&self) -> String {
        self.default_model.load().as_str().to_string()
    }

    /// 记录路由追踪。
    fn record(&self, rec: RoutingRecord) {
        if self.max_records == 0 {
            return;
        }
        let mut records = self.records.lock();
        if records.len() >= self.max_records {
            records.pop_front();
        }
        records.push_back(rec);
    }

    /// 查询路由追踪记录（返回副本）。
    pub fn routing_records(&self) -> Vec<RoutingRecord> {
        self.records.lock().iter().cloned().collect()
    }

    /// 清空路由追踪记录。
    pub fn clear_records(&self) {
        self.records.lock().clear();
    }

    /// 最近一次路由记录。
    pub fn last_record(&self) -> Option<RoutingRecord> {
        self.records.lock().back().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{
        ChatCompletion, ChatMessage, ChatRequest, LlmProvider, StreamDelta,
    };
    use crate::llm::router::RoutingStrategy;
    use async_trait::async_trait;
    use futures::stream::BoxStream;

    struct MockProvider;
    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
            Err(AiError::Internal("mock".into()))
        }
        async fn stream_completion(
            &self,
            _req: ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
            Err(AiError::Internal("mock".into()))
        }
        async fn token_count(&self, _messages: &[ChatMessage]) -> Result<u32, AiError> {
            Ok(0)
        }
        fn supported_models(&self) -> &[&str] {
            &[]
        }
    }

    fn make_router() -> ModelRouter {
        let mut routes = HashMap::new();
        routes.insert("gpt-4o".to_string(), Arc::new(MockProvider) as ProviderRef);
        routes.insert(
            "claude-3".to_string(),
            Arc::new(MockProvider) as ProviderRef,
        );
        ModelRouter::new(routes, "gpt-4o".to_string())
    }

    #[test]
    fn route_by_explicit_model() {
        let r = make_router();
        r.route(Some("gpt-4o")).unwrap();
        r.route(Some("claude-3")).unwrap();
        let records = r.routing_records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].selected_model, "gpt-4o");
        assert_eq!(records[1].selected_model, "claude-3");
    }

    #[test]
    fn route_by_default_model() {
        let r = make_router();
        r.route(None).unwrap();
        let records = r.routing_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].selected_model, "gpt-4o");
    }

    #[test]
    fn route_unknown_model_errors() {
        let r = make_router();
        let result = r.route(Some("unknown-model"));
        match result {
            Err(e) => assert_eq!(e.error_code(), "AI_CONFIG_INVALID"),
            Ok(_) => panic!("expected error for unknown model"),
        }
    }

    #[test]
    fn apply_update_replaces_routes() {
        let r = make_router();
        assert!(r.route(Some("gpt-4o")).is_ok());
        let new_routes = HashMap::new();
        r.apply_update(new_routes, "new-default".to_string());
        assert_eq!(r.default_model(), "new-default");
        assert!(r.route(Some("gpt-4o")).is_err());
    }

    #[test]
    fn default_model_returns_correct_value() {
        let r = make_router();
        assert_eq!(r.default_model(), "gpt-4o");
    }

    // --- RoutingStrategy 测试 ---

    #[test]
    fn routing_strategy_display() {
        assert_eq!(RoutingStrategy::Explicit.to_string(), "explicit");
        assert_eq!(RoutingStrategy::Capability.to_string(), "capability");
        assert_eq!(RoutingStrategy::Cost.to_string(), "cost");
        assert_eq!(RoutingStrategy::Fallback.to_string(), "fallback");
    }

    #[test]
    fn routing_strategy_serde() {
        let s = serde_json::to_string(&RoutingStrategy::Cost).unwrap();
        assert_eq!(s, "\"cost\"");
        let r: RoutingStrategy = serde_json::from_str("\"fallback\"").unwrap();
        assert_eq!(r, RoutingStrategy::Fallback);
    }

    // --- 路由追踪测试 ---

    #[test]
    fn route_records_trace() {
        let r = make_router();
        r.route(Some("gpt-4o")).unwrap();
        r.route(Some("claude-3")).unwrap();

        let records = r.routing_records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].selected_model, "gpt-4o");
        assert_eq!(records[1].selected_model, "claude-3");
        assert_eq!(records[0].strategy, RoutingStrategy::Explicit);
    }

    #[test]
    fn route_trace_no_api_key_leak() {
        let r = make_router();
        r.route(Some("gpt-4o")).unwrap();
        let records = r.routing_records();
        let record_json = serde_json::to_string(&records[0]).unwrap();
        assert!(!record_json.contains("api_key"));
        assert!(!record_json.contains("apiKey"));
        assert!(!record_json.contains("secret"));
    }

    #[test]
    fn last_record_returns_most_recent() {
        let r = make_router();
        r.route(Some("gpt-4o")).unwrap();
        r.route(Some("claude-3")).unwrap();
        let last = r.last_record().unwrap();
        assert_eq!(last.selected_model, "claude-3");
    }

    #[test]
    fn clear_records() {
        let r = make_router();
        r.route(Some("gpt-4o")).unwrap();
        assert_eq!(r.routing_records().len(), 1);
        r.clear_records();
        assert_eq!(r.routing_records().len(), 0);
    }

    #[test]
    fn records_capped_at_max() {
        let r = make_router().with_max_records(3);
        for _ in 0..5 {
            r.route(Some("gpt-4o")).unwrap();
        }
        assert_eq!(r.routing_records().len(), 3);
    }

    #[test]
    fn route_by_capability_records_trace() {
        let r = make_router();
        r.route_by_capability("gpt", RoutingStrategy::Capability)
            .unwrap();
        let records = r.routing_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_capability.as_deref(), Some("gpt"));
        assert_eq!(records[0].selected_model, "gpt-4o");
    }

    #[test]
    fn route_by_capability_no_match_errors() {
        let r = make_router();
        let result = r.route_by_capability("nonexistent", RoutingStrategy::Capability);
        assert!(result.is_err());
        assert_eq!(
            result.err().unwrap().error_code(),
            "AI_PROVIDER_UNAVAILABLE"
        );
    }

    #[test]
    fn route_default_model_records_trace() {
        let r = make_router();
        r.route(None).unwrap();
        let records = r.routing_records();
        assert_eq!(records.len(), 1);
        assert!(records[0].input_model.is_none());
        assert_eq!(records[0].selected_model, "gpt-4o");
    }

    #[test]
    fn route_by_capability_cost_selects_min_name() {
        let mut routes = HashMap::new();
        routes.insert(
            "zzz-model".to_string(),
            Arc::new(MockProvider) as ProviderRef,
        );
        routes.insert(
            "aaa-model".to_string(),
            Arc::new(MockProvider) as ProviderRef,
        );
        routes.insert(
            "mmm-model".to_string(),
            Arc::new(MockProvider) as ProviderRef,
        );
        let r = ModelRouter::new(routes, "aaa-model".to_string());
        r.route_by_capability("model", RoutingStrategy::Cost)
            .unwrap();
        let records = r.routing_records();
        assert_eq!(records[0].selected_model, "aaa-model");
    }

    #[test]
    fn records_capped_at_zero() {
        let r = make_router().with_max_records(0);
        r.route(Some("gpt-4o")).unwrap();
        assert_eq!(r.routing_records().len(), 0);
    }
}
