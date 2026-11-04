// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 模型注册表：支持能力标签注册与查询。
//!
//! 每个模型声明能力标签（如 `["codegen"]` / `["chat", "codegen"]`），
//! 调用方可按能力标签查询满足条件的模型列表（按优先级降序排列）。

use crate::llm::provider::ProviderRef;
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;

/// 模型成本信息（每 1K token 美元）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ModelCost {
    pub input_per_1k: f64,
    pub output_per_1k: f64,
}

impl ModelCost {
    /// 计算给定 token 用量的总成本。
    ///
    /// ```
    /// use sz_rust_ai_facade::llm::model_registry::ModelCost;
    /// let c = ModelCost { input_per_1k: 0.01, output_per_1k: 0.03 };
    /// assert_eq!(c.total(1000, 1000), 0.04);
    /// ```
    pub fn total(&self, input_tokens: u32, output_tokens: u32) -> f64 {
        self.input_per_1k * (input_tokens as f64 / 1000.0)
            + self.output_per_1k * (output_tokens as f64 / 1000.0)
    }
}

/// 注册表中的模型条目。
#[derive(Clone)]
pub struct ModelEntry {
    /// 模型名称（唯一键）。
    pub name: String,
    /// 能力标签列表（如 `["codegen", "chat"]`）。
    pub capabilities: Vec<String>,
    /// 优先级（数值越大优先级越高）。
    pub priority: i32,
    /// 成本信息。
    pub cost: ModelCost,
    /// 底层 Provider 引用。
    pub provider: ProviderRef,
}

impl ModelEntry {
    /// 判断模型是否具备指定能力。
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }
}

/// 模型注册表，支持按能力标签查询。
///
/// 使用 `ArcSwap` 实现无锁热更新。
pub struct ModelRegistry {
    entries: ArcSwap<HashMap<String, ModelEntry>>,
}

impl Default for ModelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelRegistry {
    /// 创建空注册表。
    pub fn new() -> Self {
        Self {
            entries: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    /// 注册或覆盖一个模型条目。
    pub fn register(&self, entry: ModelEntry) {
        let mut map: HashMap<String, ModelEntry> = (**self.entries.load()).clone();
        map.insert(entry.name.clone(), entry);
        self.entries.store(Arc::new(map));
    }

    /// 注销模型。
    pub fn deregister(&self, name: &str) {
        let mut map: HashMap<String, ModelEntry> = (**self.entries.load()).clone();
        map.remove(name);
        self.entries.store(Arc::new(map));
    }

    /// 按名称查询模型。
    pub fn get(&self, name: &str) -> Option<ModelEntry> {
        self.entries.load().get(name).cloned()
    }

    /// 按能力标签查询，返回具备该能力的所有模型，按优先级降序排列。
    ///
    /// ```
    /// use sz_rust_ai_facade::llm::model_registry::{ModelEntry, ModelRegistry, ModelCost};
    /// use sz_rust_ai_facade::llm::provider::ProviderRef;
    /// use sz_rust_ai_facade::llm::test_support::MockProvider;
    /// use std::sync::Arc;
    /// let reg = ModelRegistry::new();
    /// reg.register(ModelEntry {
    ///     name: "a".into(), capabilities: vec!["codegen".into()],
    ///     priority: 1, cost: ModelCost::default(),
    ///     provider: Arc::new(MockProvider) as ProviderRef,
    /// });
    /// reg.register(ModelEntry {
    ///     name: "b".into(), capabilities: vec!["codegen".into()],
    ///     priority: 5, cost: ModelCost::default(),
    ///     provider: Arc::new(MockProvider) as ProviderRef,
    /// });
    /// let result = reg.query_by_capability("codegen");
    /// assert_eq!(result[0].name, "b");
    /// assert_eq!(result[1].name, "a");
    /// ```
    pub fn query_by_capability(&self, cap: &str) -> Vec<ModelEntry> {
        let entries = self.entries.load();
        let mut matched: Vec<ModelEntry> = entries
            .values()
            .filter(|e| e.has_capability(cap))
            .cloned()
            .collect();
        matched.sort_by_key(|e| std::cmp::Reverse(e.priority));
        matched
    }

    /// 列出所有已注册模型。
    pub fn list(&self) -> Vec<ModelEntry> {
        self.entries.load().values().cloned().collect()
    }

    /// 当前注册模型数量。
    pub fn len(&self) -> usize {
        self.entries.load().len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::test_support::MockProvider;

    fn make_entry(name: &str, caps: &[&str], priority: i32) -> ModelEntry {
        ModelEntry {
            name: name.to_string(),
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            priority,
            cost: ModelCost::default(),
            provider: Arc::new(MockProvider) as ProviderRef,
        }
    }

    #[test]
    fn register_and_get() {
        let reg = ModelRegistry::new();
        reg.register(make_entry("gpt-4o", &["chat", "codegen"], 10));
        assert!(reg.get("gpt-4o").is_some());
        assert!(reg.get("unknown").is_none());
    }

    #[test]
    fn deregister_removes_entry() {
        let reg = ModelRegistry::new();
        reg.register(make_entry("gpt-4o", &["chat"], 10));
        assert_eq!(reg.len(), 1);
        reg.deregister("gpt-4o");
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn query_by_capability_sorted_by_priority() {
        let reg = ModelRegistry::new();
        reg.register(make_entry("low", &["codegen"], 1));
        reg.register(make_entry("high", &["codegen"], 100));
        reg.register(make_entry("mid", &["codegen"], 50));
        reg.register(make_entry("chat_only", &["chat"], 200));

        let result = reg.query_by_capability("codegen");
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "high");
        assert_eq!(result[1].name, "mid");
        assert_eq!(result[2].name, "low");
    }

    #[test]
    fn query_by_capability_empty() {
        let reg = ModelRegistry::new();
        reg.register(make_entry("a", &["chat"], 1));
        assert!(reg.query_by_capability("codegen").is_empty());
    }

    #[test]
    fn cost_calculation() {
        let cost = ModelCost {
            input_per_1k: 0.01,
            output_per_1k: 0.03,
        };
        assert_eq!(cost.total(2000, 4000), 0.02 + 0.12);
    }

    #[test]
    fn list_all_entries() {
        let reg = ModelRegistry::new();
        reg.register(make_entry("a", &["chat"], 1));
        reg.register(make_entry("b", &["codegen"], 2));
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn register_overwrites() {
        let reg = ModelRegistry::new();
        reg.register(make_entry("a", &["chat"], 1));
        reg.register(make_entry("a", &["codegen"], 5));
        let entry = reg.get("a").unwrap();
        assert_eq!(entry.priority, 5);
        assert!(entry.has_capability("codegen"));
        assert!(!entry.has_capability("chat"));
    }
}
