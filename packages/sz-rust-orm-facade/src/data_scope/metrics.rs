//! Data Scope 指标采集 — DataScopeMetrics
//!
//! 使用 tracing 日志记录指标（对齐 observability 模块的轻量级方案）。
//! 生产环境可通过 tracing subscriber 导出至 Prometheus。

use std::sync::atomic::{AtomicU64, Ordering};

/// Data Scope 指标
pub struct DataScopeMetrics {
    hit_total: AtomicU64,
    bypass_total: AtomicU64,
    reject_total: AtomicU64,
    eval_total_ms: AtomicU64,
    eval_count: AtomicU64,
}

impl DataScopeMetrics {
    /// 创建指标实例
    pub fn new() -> Self {
        Self {
            hit_total: AtomicU64::new(0),
            bypass_total: AtomicU64::new(0),
            reject_total: AtomicU64::new(0),
            eval_total_ms: AtomicU64::new(0),
            eval_count: AtomicU64::new(0),
        }
    }

    /// 记录命中（数据范围条件注入成功）
    pub fn record_hit(&self, table: &str, mode: &str) {
        self.hit_total.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(
            target: "data_scope_metrics",
            table = table,
            mode = mode,
            "data_scope_hit"
        );
    }

    /// 记录绕过（超级管理员）
    pub fn record_bypass(&self, user_id: i64, table: &str) {
        self.bypass_total.fetch_add(1, Ordering::Relaxed);
        tracing::info!(
            target: "data_scope_audit",
            user_id = user_id,
            table = table,
            "super bypass: user_id={}, table={}", user_id, table
        );
    }

    /// 记录拒绝（错误发生）
    pub fn record_reject(&self, error_code: &str, table: &str) {
        self.reject_total.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            target: "data_scope_metrics",
            error_code = error_code,
            table = table,
            "data_scope_reject"
        );
    }

    /// 记录评估耗时
    pub fn record_eval(&self, elapsed_ms: u64) {
        self.eval_total_ms.fetch_add(elapsed_ms, Ordering::Relaxed);
        self.eval_count.fetch_add(1, Ordering::Relaxed);
    }

    /// 获取命中总数
    pub fn hit_total(&self) -> u64 {
        self.hit_total.load(Ordering::Relaxed)
    }

    /// 获取绕过总数
    pub fn bypass_total(&self) -> u64 {
        self.bypass_total.load(Ordering::Relaxed)
    }

    /// 获取拒绝总数
    pub fn reject_total(&self) -> u64 {
        self.reject_total.load(Ordering::Relaxed)
    }

    /// 获取平均评估耗时（毫秒）
    pub fn avg_eval_ms(&self) -> f64 {
        let count = self.eval_count.load(Ordering::Relaxed);
        if count == 0 {
            0.0
        } else {
            self.eval_total_ms.load(Ordering::Relaxed) as f64 / count as f64
        }
    }
}

impl Default for DataScopeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_record() {
        let metrics = DataScopeMetrics::new();
        metrics.record_hit("order", "dept");
        metrics.record_hit("order", "dept");
        metrics.record_bypass(1, "order");
        metrics.record_reject("DATA_SCOPE_NO_USER_CONTEXT", "order");
        metrics.record_eval(5);
        metrics.record_eval(15);
        assert_eq!(metrics.hit_total(), 2);
        assert_eq!(metrics.bypass_total(), 1);
        assert_eq!(metrics.reject_total(), 1);
        assert_eq!(metrics.avg_eval_ms(), 10.0);
    }
}
