// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use sz_rust_observability::MetricsRegistry;

struct MetricHandles {
    llm_request_total: Arc<sz_rust_observability::Counter>,
    llm_tokens_total: Arc<sz_rust_observability::Counter>,
    llm_request_seconds: Arc<sz_rust_observability::Histogram>,
    rag_recall_seconds: Arc<sz_rust_observability::Histogram>,
    agent_steps_total: Arc<sz_rust_observability::Counter>,
    embedding_total: Arc<sz_rust_observability::Counter>,
    cache_hit_total: Arc<sz_rust_observability::Counter>,
}

pub struct AiMetrics {
    handles: Mutex<Option<MetricHandles>>,
    llm_request_count: Mutex<u64>,
    llm_tokens_count: Mutex<u64>,
    rag_recall_total: Mutex<f64>,
    agent_steps_count: Mutex<u64>,
    embedding_count: Mutex<u64>,
    cache_hit_count: Mutex<u64>,
}

static INSTANCE: OnceLock<AiMetrics> = OnceLock::new();

impl AiMetrics {
    pub fn global() -> &'static AiMetrics {
        INSTANCE.get_or_init(|| AiMetrics {
            handles: Mutex::new(None),
            llm_request_count: Mutex::new(0),
            llm_tokens_count: Mutex::new(0),
            rag_recall_total: Mutex::new(0.0),
            agent_steps_count: Mutex::new(0),
            embedding_count: Mutex::new(0),
            cache_hit_count: Mutex::new(0),
        })
    }

    pub fn register(&self, registry: &MetricsRegistry) {
        let mut handles = self.handles.lock();
        if handles.is_some() {
            return;
        }
        *handles = Some(MetricHandles {
            llm_request_total: registry.register_counter_with_labels(
                "ai_llm_request_total",
                "Total LLM requests",
                HashMap::from([
                    ("provider".to_string(), "".to_string()),
                    ("model".to_string(), "".to_string()),
                    ("status".to_string(), "".to_string()),
                ]),
            ),
            llm_tokens_total: registry.register_counter_with_labels(
                "ai_llm_tokens_total",
                "Total LLM tokens consumed",
                HashMap::from([
                    ("provider".to_string(), "".to_string()),
                    ("model".to_string(), "".to_string()),
                    ("direction".to_string(), "".to_string()),
                ]),
            ),
            llm_request_seconds: registry.register_histogram(
                "ai_llm_request_seconds",
                "LLM request duration in seconds",
                vec![0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0],
            ),
            rag_recall_seconds: registry.register_histogram(
                "ai_rag_recall_seconds",
                "RAG retrieval duration in seconds",
                vec![0.005, 0.01, 0.05, 0.1, 0.5, 1.0],
            ),
            agent_steps_total: registry.register_counter_with_labels(
                "ai_agent_steps_total",
                "Total Agent steps executed",
                HashMap::from([
                    ("agent_id".to_string(), "".to_string()),
                    ("terminated_by".to_string(), "".to_string()),
                ]),
            ),
            embedding_total: registry.register_counter_with_labels(
                "ai_embedding_total",
                "Total embedding requests",
                HashMap::from([
                    ("provider".to_string(), "".to_string()),
                    ("model".to_string(), "".to_string()),
                ]),
            ),
            cache_hit_total: registry.register_counter_with_labels(
                "ai_cache_hit_total",
                "Total AI cache hits",
                HashMap::from([("type".to_string(), "".to_string())]),
            ),
        });
    }

    pub fn record_llm_request(&self, provider: &str, model: &str, status: &str) {
        *self.llm_request_count.lock() += 1;
        let handles = self.handles.lock();
        if let Some(h) = handles.as_ref() {
            h.llm_request_total.inc();
        }
        tracing::debug!(
            target: "ai_metrics",
            metric = "ai_llm_request_total",
            provider = provider,
            model = model,
            status = status,
            "LLM request recorded"
        );
    }

    pub fn record_llm_tokens(&self, provider: &str, model: &str, direction: &str, count: u32) {
        *self.llm_tokens_count.lock() += count as u64;
        let handles = self.handles.lock();
        if let Some(h) = handles.as_ref() {
            h.llm_tokens_total.inc_by(count as f64);
        }
        tracing::debug!(
            target: "ai_metrics",
            metric = "ai_llm_tokens_total",
            provider = provider,
            model = model,
            direction = direction,
            count = count,
            "LLM tokens recorded"
        );
    }

    pub fn record_llm_request_duration(&self, seconds: f64) {
        let handles = self.handles.lock();
        if let Some(h) = handles.as_ref() {
            h.llm_request_seconds.observe(seconds);
        }
    }

    pub fn record_rag_recall(&self, seconds: f64) {
        *self.rag_recall_total.lock() += seconds;
        let handles = self.handles.lock();
        if let Some(h) = handles.as_ref() {
            h.rag_recall_seconds.observe(seconds);
        }
    }

    pub fn record_agent_step(&self, agent_id: &str, terminated_by: &str) {
        *self.agent_steps_count.lock() += 1;
        let handles = self.handles.lock();
        if let Some(h) = handles.as_ref() {
            h.agent_steps_total.inc();
        }
        tracing::debug!(
            target: "ai_metrics",
            metric = "ai_agent_steps_total",
            agent_id = agent_id,
            terminated_by = terminated_by,
            "Agent step recorded"
        );
    }

    pub fn record_embedding(&self, provider: &str, model: &str) {
        *self.embedding_count.lock() += 1;
        let handles = self.handles.lock();
        if let Some(h) = handles.as_ref() {
            h.embedding_total.inc();
        }
        tracing::debug!(
            target: "ai_metrics",
            metric = "ai_embedding_total",
            provider = provider,
            model = model,
            "Embedding request recorded"
        );
    }

    pub fn record_cache_hit(&self, cache_type: &str) {
        *self.cache_hit_count.lock() += 1;
        let handles = self.handles.lock();
        if let Some(h) = handles.as_ref() {
            h.cache_hit_total.inc();
        }
        tracing::debug!(
            target: "ai_metrics",
            metric = "ai_cache_hit_total",
            cache_type = cache_type,
            "Cache hit recorded"
        );
    }

    pub fn llm_request_count(&self) -> u64 {
        *self.llm_request_count.lock()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_singleton_is_consistent() {
        let m1 = AiMetrics::global();
        let m2 = AiMetrics::global();
        assert!(std::ptr::eq(m1, m2));
    }

    #[test]
    fn record_llm_request_increments_count() {
        let metrics = AiMetrics::global();
        let before = metrics.llm_request_count();
        metrics.record_llm_request("openai", "gpt-4", "success");
        let after = metrics.llm_request_count();
        assert_eq!(after, before + 1);
    }

    #[test]
    fn record_llm_tokens_does_not_panic() {
        let metrics = AiMetrics::global();
        metrics.record_llm_tokens("openai", "gpt-4", "prompt", 100);
        metrics.record_llm_tokens("openai", "gpt-4", "completion", 50);
    }

    #[test]
    fn record_llm_request_duration_does_not_panic() {
        let metrics = AiMetrics::global();
        metrics.record_llm_request_duration(1.5);
        metrics.record_llm_request_duration(0.01);
    }

    #[test]
    fn record_rag_recall_does_not_panic() {
        let metrics = AiMetrics::global();
        metrics.record_rag_recall(0.05);
        metrics.record_rag_recall(0.2);
    }

    #[test]
    fn record_agent_step_does_not_panic() {
        let metrics = AiMetrics::global();
        metrics.record_agent_step("agent-1", "natural");
        metrics.record_agent_step("agent-2", "max_steps");
    }

    #[test]
    fn record_embedding_does_not_panic() {
        let metrics = AiMetrics::global();
        metrics.record_embedding("openai", "text-embedding-3-small");
        metrics.record_embedding("local", "local");
    }

    #[test]
    fn record_cache_hit_does_not_panic() {
        let metrics = AiMetrics::global();
        metrics.record_cache_hit("semantic");
        metrics.record_cache_hit("exact");
    }

    #[test]
    fn multiple_record_calls_accumulate() {
        let metrics = AiMetrics::global();
        let before = metrics.llm_request_count();
        metrics.record_llm_request("openai", "gpt-4", "success");
        metrics.record_llm_request("claude", "claude-3", "success");
        metrics.record_llm_request("openai", "gpt-4", "error");
        let after = metrics.llm_request_count();
        assert_eq!(after, before + 3);
    }

    #[test]
    fn register_with_metrics_registry() {
        let metrics = AiMetrics::global();
        let registry = sz_rust_observability::MetricsRegistry::new();
        metrics.register(&registry);
        // register 是幂等的，再次调用不应 panic
        metrics.register(&registry);
        // 注册后 record 方法应正常工作（handles 已设置）
        let before = metrics.llm_request_count();
        metrics.record_llm_request("openai", "gpt-4", "success");
        metrics.record_llm_tokens("openai", "gpt-4", "prompt", 50);
        metrics.record_llm_request_duration(0.5);
        metrics.record_rag_recall(0.1);
        metrics.record_agent_step("agent-1", "natural");
        metrics.record_embedding("openai", "text-embedding-3-small");
        metrics.record_cache_hit("semantic");
        let after = metrics.llm_request_count();
        assert_eq!(after, before + 1);
    }
}
