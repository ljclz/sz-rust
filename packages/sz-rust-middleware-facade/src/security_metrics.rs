// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 安全中间件指标聚合 — 统一暴露 4 个安全中间件的运行时指标
//!
//! 对齐 design §2.2.2 横切接口。

use std::sync::atomic::{AtomicU64, Ordering};

/// 安全中间件运行时指标
#[derive(Debug, Default)]
pub struct SecurityMetrics {
    /// 安全响应头注入次数
    headers_injected: AtomicU64,
    /// IP 拒绝次数
    ip_rejected: AtomicU64,
    /// 审计日志写入次数
    audit_logged: AtomicU64,
    /// 请求体过大拒绝次数
    body_too_large: AtomicU64,
}

impl SecurityMetrics {
    /// 创建空指标
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录安全响应头注入
    pub fn record_headers_injected(&self) {
        self.headers_injected.fetch_add(1, Ordering::Relaxed);
    }

    /// 记数：安全响应头注入次数
    pub fn headers_injected(&self) -> u64 {
        self.headers_injected.load(Ordering::Relaxed)
    }

    /// 记录 IP 拒绝
    pub fn record_ip_rejected(&self) {
        self.ip_rejected.fetch_add(1, Ordering::Relaxed);
    }

    /// 计数：IP 拒绝次数
    pub fn ip_rejected(&self) -> u64 {
        self.ip_rejected.load(Ordering::Relaxed)
    }

    /// 记录审计日志写入
    pub fn record_audit_logged(&self) {
        self.audit_logged.fetch_add(1, Ordering::Relaxed);
    }

    /// 计数：审计日志写入次数
    pub fn audit_logged(&self) -> u64 {
        self.audit_logged.load(Ordering::Relaxed)
    }

    /// 记录请求体过大拒绝
    pub fn record_body_too_large(&self) {
        self.body_too_large.fetch_add(1, Ordering::Relaxed);
    }

    /// 计数：请求体过大拒绝次数
    pub fn body_too_large(&self) -> u64 {
        self.body_too_large.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_count() {
        let metrics = SecurityMetrics::new();
        metrics.record_headers_injected();
        metrics.record_headers_injected();
        metrics.record_ip_rejected();
        metrics.record_audit_logged();
        metrics.record_audit_logged();
        metrics.record_audit_logged();
        metrics.record_body_too_large();

        assert_eq!(metrics.headers_injected(), 2);
        assert_eq!(metrics.ip_rejected(), 1);
        assert_eq!(metrics.audit_logged(), 3);
        assert_eq!(metrics.body_too_large(), 1);
    }

    #[test]
    fn test_metrics_default_zero() {
        let metrics = SecurityMetrics::new();
        assert_eq!(metrics.headers_injected(), 0);
        assert_eq!(metrics.ip_rejected(), 0);
        assert_eq!(metrics.audit_logged(), 0);
        assert_eq!(metrics.body_too_large(), 0);
    }
}
