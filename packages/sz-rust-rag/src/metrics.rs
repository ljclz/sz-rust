// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Prometheus 指标注册与记录。

use prometheus::{Gauge, Histogram, IntCounter, Registry};

/// RAG Prometheus 指标集合。
pub struct RagMetrics {
    registry: Registry,
    retrieve_duration: Histogram,
    embedding_calls_total: IntCounter,
    vector_store_errors_total: IntCounter,
    index_size: Gauge,
    recall_score_avg: Gauge,
}

impl RagMetrics {
    /// 注册所有指标到新建的 Registry。
    pub fn register() -> Self {
        let registry = Registry::new();

        let retrieve_duration = Histogram::with_opts(
            prometheus::HistogramOpts::new(
                "rag_retrieve_duration_seconds",
                "RAG retrieval latency",
            )
            .buckets(vec![
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
            ]),
        )
        .expect("histogram opts");
        registry
            .register(Box::new(retrieve_duration.clone()))
            .expect("register retrieve_duration");

        let embedding_calls_total =
            IntCounter::new("rag_embedding_calls_total", "Total embedding API calls")
                .expect("counter opts");
        registry
            .register(Box::new(embedding_calls_total.clone()))
            .expect("register embedding_calls_total");

        let vector_store_errors_total =
            IntCounter::new("rag_vector_store_errors_total", "Total vector store errors")
                .expect("counter opts");
        registry
            .register(Box::new(vector_store_errors_total.clone()))
            .expect("register vector_store_errors_total");

        let index_size =
            Gauge::new("rag_index_size", "Current vector index size").expect("gauge opts");
        registry
            .register(Box::new(index_size.clone()))
            .expect("register index_size");

        let recall_score_avg =
            Gauge::new("rag_recall_score_avg", "Average recall top score").expect("gauge opts");
        registry
            .register(Box::new(recall_score_avg.clone()))
            .expect("register recall_score_avg");

        Self {
            registry,
            retrieve_duration,
            embedding_calls_total,
            vector_store_errors_total,
            index_size,
            recall_score_avg,
        }
    }

    /// 记录一次检索的耗时与 top score。
    pub fn record_retrieve(&self, duration: std::time::Duration, top_score: f32) {
        self.retrieve_duration.observe(duration.as_secs_f64());
        self.recall_score_avg.set(top_score as f64);
    }

    /// 记数一次 embedding API 调用。
    pub fn record_embedding_call(&self) {
        self.embedding_calls_total.inc();
    }

    /// 计数一次向量存储错误。
    pub fn record_vector_store_error(&self) {
        self.vector_store_errors_total.inc();
    }

    /// 设置当前索引大小。
    pub fn set_index_size(&self, size: u64) {
        self.index_size.set(size as f64);
    }

    /// 暴露内部 Registry（供 /metrics 端点采集）。
    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_record() {
        let m = RagMetrics::register();
        m.record_embedding_call();
        m.record_embedding_call();
        m.record_vector_store_error();
        m.set_index_size(42);
        m.record_retrieve(std::time::Duration::from_millis(120), 0.88);

        let text = prometheus::TextEncoder::new();
        let mf = m.registry().gather();
        let output = text.encode_to_string(&mf).unwrap();
        assert!(output.contains("rag_embedding_calls_total 2"));
        assert!(output.contains("rag_vector_store_errors_total 1"));
        assert!(output.contains("rag_index_size 42"));
    }
}
