// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
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
    field_filter_total: AtomicU64,
    field_all_visible_total: AtomicU64,
    field_hidden_total: AtomicU64,
    rule_total: AtomicU64,
    policy_total: AtomicU64,
    reload_total: AtomicU64,
    reload_failed_total: AtomicU64,
    tenant_request_total: AtomicU64,
    tenant_isolation_bypass_total: AtomicU64,
    tenant_resolve_failed_total: AtomicU64,
    tenant_active_count: AtomicU64,
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
            field_filter_total: AtomicU64::new(0),
            field_all_visible_total: AtomicU64::new(0),
            field_hidden_total: AtomicU64::new(0),
            rule_total: AtomicU64::new(0),
            policy_total: AtomicU64::new(0),
            reload_total: AtomicU64::new(0),
            reload_failed_total: AtomicU64::new(0),
            tenant_request_total: AtomicU64::new(0),
            tenant_isolation_bypass_total: AtomicU64::new(0),
            tenant_resolve_failed_total: AtomicU64::new(0),
            tenant_active_count: AtomicU64::new(0),
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

    pub fn record_field_filter(&self, table: &str, hidden_count: usize) {
        self.field_filter_total.fetch_add(1, Ordering::Relaxed);
        if hidden_count == 0 {
            self.field_all_visible_total.fetch_add(1, Ordering::Relaxed);
        } else {
            self.field_hidden_total.fetch_add(1, Ordering::Relaxed);
        }
        tracing::debug!(
            target: "data_scope_metrics",
            table = table,
            hidden_count = hidden_count,
            "field_filter"
        );
    }

    pub fn field_filter_total(&self) -> u64 {
        self.field_filter_total.load(Ordering::Relaxed)
    }

    pub fn field_all_visible_total(&self) -> u64 {
        self.field_all_visible_total.load(Ordering::Relaxed)
    }

    pub fn field_hidden_total(&self) -> u64 {
        self.field_hidden_total.load(Ordering::Relaxed)
    }

    /// 设置规则总数
    pub fn set_rule_total(&self, n: u64) {
        self.rule_total.store(n, Ordering::Relaxed);
    }

    /// 获取规则总数
    pub fn rule_total(&self) -> u64 {
        self.rule_total.load(Ordering::Relaxed)
    }

    /// 设置策略总数
    pub fn set_policy_total(&self, n: u64) {
        self.policy_total.store(n, Ordering::Relaxed);
    }

    /// 获取策略总数
    pub fn policy_total(&self) -> u64 {
        self.policy_total.load(Ordering::Relaxed)
    }

    /// 记录一次配置重载
    pub fn record_reload(&self) {
        self.reload_total.fetch_add(1, Ordering::Relaxed);
    }

    /// 获取重载总次数
    pub fn reload_total(&self) -> u64 {
        self.reload_total.load(Ordering::Relaxed)
    }

    /// 记录一次重载失败
    pub fn record_reload_failed(&self) {
        self.reload_failed_total.fetch_add(1, Ordering::Relaxed);
    }

    /// 获取重载失败总次数
    pub fn reload_failed_total(&self) -> u64 {
        self.reload_failed_total.load(Ordering::Relaxed)
    }

    pub fn record_tenant_request(&self) {
        self.tenant_request_total.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(target: "data_scope_metrics", "tenant_request");
    }

    pub fn tenant_request_total(&self) -> u64 {
        self.tenant_request_total.load(Ordering::Relaxed)
    }

    pub fn record_tenant_isolation_bypass(&self) {
        self.tenant_isolation_bypass_total
            .fetch_add(1, Ordering::Relaxed);
        tracing::debug!(target: "data_scope_metrics", "tenant_isolation_bypass");
    }

    pub fn tenant_isolation_bypass_total(&self) -> u64 {
        self.tenant_isolation_bypass_total.load(Ordering::Relaxed)
    }

    pub fn record_tenant_resolve_failed(&self) {
        self.tenant_resolve_failed_total
            .fetch_add(1, Ordering::Relaxed);
        tracing::debug!(target: "data_scope_metrics", "tenant_resolve_failed");
    }

    pub fn tenant_resolve_failed_total(&self) -> u64 {
        self.tenant_resolve_failed_total.load(Ordering::Relaxed)
    }

    pub fn set_tenant_active_count(&self, n: u64) {
        self.tenant_active_count.store(n, Ordering::Relaxed);
    }

    pub fn tenant_active_count(&self) -> u64 {
        self.tenant_active_count.load(Ordering::Relaxed)
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

    #[test]
    fn test_avg_eval_ms_zero_count() {
        let metrics = DataScopeMetrics::new();
        assert_eq!(metrics.avg_eval_ms(), 0.0);
    }

    #[test]
    fn test_default() {
        let metrics = DataScopeMetrics::default();
        assert_eq!(metrics.hit_total(), 0);
        assert_eq!(metrics.avg_eval_ms(), 0.0);
    }

    #[test]
    fn test_field_metrics() {
        let metrics = DataScopeMetrics::new();
        metrics.record_field_filter("employee", 0);
        metrics.record_field_filter("employee", 3);
        metrics.record_field_filter("order", 1);
        assert_eq!(metrics.field_filter_total(), 3);
        assert_eq!(metrics.field_all_visible_total(), 1);
        assert_eq!(metrics.field_hidden_total(), 2);
    }

    #[test]
    fn test_new_metrics_set_and_get() {
        let metrics = DataScopeMetrics::new();
        // 新字段初始为 0
        assert_eq!(metrics.rule_total(), 0);
        assert_eq!(metrics.policy_total(), 0);
        assert_eq!(metrics.reload_total(), 0);
        assert_eq!(metrics.reload_failed_total(), 0);

        // set / record
        metrics.set_rule_total(42);
        metrics.set_policy_total(7);
        metrics.record_reload();
        metrics.record_reload();
        metrics.record_reload_failed();
        assert_eq!(metrics.rule_total(), 42);
        assert_eq!(metrics.policy_total(), 7);
        assert_eq!(metrics.reload_total(), 2);
        assert_eq!(metrics.reload_failed_total(), 1);

        // set 覆盖
        metrics.set_rule_total(100);
        assert_eq!(metrics.rule_total(), 100);
    }

    #[test]
    fn test_tenant_metrics_set_and_get() {
        let metrics = DataScopeMetrics::new();
        assert_eq!(metrics.tenant_request_total(), 0);
        assert_eq!(metrics.tenant_isolation_bypass_total(), 0);
        assert_eq!(metrics.tenant_resolve_failed_total(), 0);
        assert_eq!(metrics.tenant_active_count(), 0);

        metrics.record_tenant_request();
        metrics.record_tenant_request();
        metrics.record_tenant_isolation_bypass();
        metrics.record_tenant_resolve_failed();
        metrics.set_tenant_active_count(5);

        assert_eq!(metrics.tenant_request_total(), 2);
        assert_eq!(metrics.tenant_isolation_bypass_total(), 1);
        assert_eq!(metrics.tenant_resolve_failed_total(), 1);
        assert_eq!(metrics.tenant_active_count(), 5);
    }
}
