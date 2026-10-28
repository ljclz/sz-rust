// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

/// 工作流 Prometheus 指标，对齐 spec 4.4.2。
///
/// 4 个指标：
/// - `workflow_instance_active`（Gauge）
/// - `workflow_task_pending`（Gauge）
/// - `workflow_transition_duration_seconds`（Histogram — 简化为计数+总和）
/// - `workflow_plugin_node_error_total`（Counter）
pub struct WorkflowMetrics {
    instance_active: AtomicI64,
    task_pending: AtomicI64,
    transition_count: AtomicU64,
    transition_total_us: AtomicU64,
    plugin_node_error: AtomicU64,
}

impl Default for WorkflowMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkflowMetrics {
    pub fn new() -> Self {
        Self {
            instance_active: AtomicI64::new(0),
            task_pending: AtomicI64::new(0),
            transition_count: AtomicU64::new(0),
            transition_total_us: AtomicU64::new(0),
            plugin_node_error: AtomicU64::new(0),
        }
    }

    pub fn set_instance_active(&self, count: i64) {
        self.instance_active.store(count, Ordering::Relaxed);
    }

    pub fn set_task_pending(&self, count: i64) {
        self.task_pending.store(count, Ordering::Relaxed);
    }

    pub fn record_transition_duration(&self, duration: Duration) {
        self.transition_count.fetch_add(1, Ordering::Relaxed);
        self.transition_total_us
            .fetch_add(duration.as_micros() as u64, Ordering::Relaxed);
    }

    pub fn record_plugin_node_error(&self) {
        self.plugin_node_error.fetch_add(1, Ordering::Relaxed);
    }

    pub fn instance_active(&self) -> i64 {
        self.instance_active.load(Ordering::Relaxed)
    }
    pub fn task_pending(&self) -> i64 {
        self.task_pending.load(Ordering::Relaxed)
    }
    pub fn transition_count(&self) -> u64 {
        self.transition_count.load(Ordering::Relaxed)
    }
    pub fn transition_avg_us(&self) -> f64 {
        let count = self.transition_count.load(Ordering::Relaxed);
        if count == 0 {
            0.0
        } else {
            self.transition_total_us.load(Ordering::Relaxed) as f64 / count as f64
        }
    }
    pub fn plugin_node_errors(&self) -> u64 {
        self.plugin_node_error.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_basic() {
        let m = WorkflowMetrics::new();
        m.set_instance_active(5);
        m.set_task_pending(10);
        m.record_transition_duration(Duration::from_millis(3));
        m.record_transition_duration(Duration::from_millis(7));
        m.record_plugin_node_error();
        m.record_plugin_node_error();

        assert_eq!(m.instance_active(), 5);
        assert_eq!(m.task_pending(), 10);
        assert_eq!(m.transition_count(), 2);
        assert_eq!(m.transition_avg_us(), 5000.0);
        assert_eq!(m.plugin_node_errors(), 2);
    }
}
