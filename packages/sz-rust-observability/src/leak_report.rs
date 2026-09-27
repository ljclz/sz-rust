// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 泄漏报告（P4-4）

use std::collections::HashMap;

/// 资源类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// 连接池活跃连接
    ConnectionPool,
    /// 异步任务持有
    AsyncTask,
    /// 缓存条目
    CacheEntry,
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceKind::ConnectionPool => write!(f, "connection_pool"),
            ResourceKind::AsyncTask => write!(f, "async_task"),
            ResourceKind::CacheEntry => write!(f, "cache_entry"),
        }
    }
}

/// 泄漏资源
#[derive(Debug, Clone, PartialEq)]
pub struct LeakedResource {
    /// 资源类型
    pub kind: ResourceKind,
    /// 泄漏数量
    pub count: usize,
}

/// 增长趋势
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrowthTrend {
    /// 稳定（未持续增长）
    Stable,
    /// 单调增长（疑似泄漏）
    MonotonicGrowth,
    /// 周期波动（正常）
    Fluctuating,
}

/// 泄漏报告
#[derive(Debug, Clone)]
pub struct LeakReport {
    /// 是否检测到泄漏
    pub leaked: bool,
    /// 增长趋势
    pub trend: GrowthTrend,
    /// 疑似根因
    pub root_cause: Option<String>,
    /// 泄漏的资源列表
    pub resources: Vec<LeakedResource>,
    /// 采样数
    pub sample_count: usize,
}

impl LeakReport {
    /// 创建无泄漏报告
    pub fn no_leak(sample_count: usize) -> Self {
        Self {
            leaked: false,
            trend: GrowthTrend::Stable,
            root_cause: None,
            resources: Vec::new(),
            sample_count,
        }
    }

    /// 创建泄漏报告
    pub fn leak(
        trend: GrowthTrend,
        root_cause: impl Into<String>,
        resources: Vec<LeakedResource>,
        sample_count: usize,
    ) -> Self {
        Self {
            leaked: true,
            trend,
            root_cause: Some(root_cause.into()),
            resources,
            sample_count,
        }
    }
}

/// 资源快照
#[derive(Debug, Clone)]
pub struct ResourceSnapshot {
    /// 采样时间戳（毫秒）
    pub timestamp_ms: u64,
    /// 各资源类型的当前持有数
    pub counts: HashMap<ResourceKind, usize>,
}

impl ResourceSnapshot {
    /// 创建快照
    pub fn new(timestamp_ms: u64) -> Self {
        Self {
            timestamp_ms,
            counts: HashMap::new(),
        }
    }

    /// 记录资源数
    pub fn with_count(mut self, kind: ResourceKind, count: usize) -> Self {
        self.counts.insert(kind, count);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_leak_report() {
        let report = LeakReport::no_leak(10);
        assert!(!report.leaked);
        assert_eq!(report.trend, GrowthTrend::Stable);
        assert!(report.root_cause.is_none());
        assert!(report.resources.is_empty());
        assert_eq!(report.sample_count, 10);
    }

    #[test]
    fn test_leak_report() {
        let resources = vec![LeakedResource {
            kind: ResourceKind::ConnectionPool,
            count: 5,
        }];
        let report = LeakReport::leak(
            GrowthTrend::MonotonicGrowth,
            "connection not returned to pool",
            resources.clone(),
            20,
        );
        assert!(report.leaked);
        assert_eq!(report.trend, GrowthTrend::MonotonicGrowth);
        assert!(report.root_cause.is_some());
        assert_eq!(report.resources, resources);
        assert_eq!(report.sample_count, 20);
    }

    #[test]
    fn test_resource_kind_display() {
        assert_eq!(ResourceKind::ConnectionPool.to_string(), "connection_pool");
        assert_eq!(ResourceKind::AsyncTask.to_string(), "async_task");
        assert_eq!(ResourceKind::CacheEntry.to_string(), "cache_entry");
    }

    #[test]
    fn test_resource_snapshot() {
        let snap = ResourceSnapshot::new(1000)
            .with_count(ResourceKind::ConnectionPool, 5)
            .with_count(ResourceKind::AsyncTask, 3);
        assert_eq!(snap.timestamp_ms, 1000);
        assert_eq!(snap.counts.get(&ResourceKind::ConnectionPool), Some(&5));
        assert_eq!(snap.counts.get(&ResourceKind::AsyncTask), Some(&3));
        assert_eq!(snap.counts.get(&ResourceKind::CacheEntry), None);
    }
}
