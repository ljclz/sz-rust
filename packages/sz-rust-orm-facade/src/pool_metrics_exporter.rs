// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 连接池 Prometheus 指标暴露（spec §5.6 规则 4）
//!
//! 生成 Prometheus text exposition 格式的连接池指标。

use std::time::Instant;

/// 连接池指标快照
#[derive(Debug, Clone)]
pub struct PoolMetricsSnapshot {
    /// 当前连接数
    pub current_connections: usize,
    /// 空闲连接数
    pub idle_connections: usize,
    /// 活跃连接数
    pub active_connections: usize,
    /// 驱逐事件总数
    pub eviction_total: u64,
    /// 扩容事件总数
    pub scale_up_total: u64,
    /// 缩容事件总数
    pub scale_down_total: u64,
    /// acquire_timeout 命中次数
    pub timeout_count: u64,
    /// 总 acquire 次数
    pub total_acquire: u64,
}

impl PoolMetricsSnapshot {
    /// 创建新指标快照
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        current: usize,
        idle: usize,
        active: usize,
        evictions: u64,
        scale_up: u64,
        scale_down: u64,
        timeout: u64,
        total_acquire: u64,
    ) -> Self {
        Self {
            current_connections: current,
            idle_connections: idle,
            active_connections: active,
            eviction_total: evictions,
            scale_up_total: scale_up,
            scale_down_total: scale_down,
            timeout_count: timeout,
            total_acquire,
        }
    }

    /// acquire_timeout 命中率
    pub fn timeout_rate(&self) -> f64 {
        if self.total_acquire == 0 {
            0.0
        } else {
            self.timeout_count as f64 / self.total_acquire as f64
        }
    }
}

/// 连接池指标导出器（Prometheus text exposition 格式）
pub struct PoolMetricsExporter {
    prefix: String,
}

impl PoolMetricsExporter {
    /// 创建导出器，默认前缀 `sz_rust_pool`
    pub fn new() -> Self {
        Self {
            prefix: "sz_rust_pool".to_string(),
        }
    }

    /// 创建带自定义前缀的导出器
    pub fn with_prefix(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }

    /// 导出 Prometheus text exposition 格式指标（spec §5.6 规则 4）
    pub fn export(&self, snapshot: &PoolMetricsSnapshot) -> String {
        let p = &self.prefix;
        let mut out = String::with_capacity(512);

        // Gauge: 当前连接数
        out.push_str(&format!("# HELP {p}_connections 当前连接数\n"));
        out.push_str(&format!("# TYPE {p}_connections gauge\n"));
        out.push_str(&format!(
            "{p}_connections {}\n",
            snapshot.current_connections
        ));

        // Gauge: 空闲连接数
        out.push_str(&format!("# HELP {p}_idle_connections 空闲连接数\n"));
        out.push_str(&format!("# TYPE {p}_idle_connections gauge\n"));
        out.push_str(&format!(
            "{p}_idle_connections {}\n",
            snapshot.idle_connections
        ));

        // Gauge: 活跃连接数
        out.push_str(&format!("# HELP {p}_active_connections 活跃连接数\n"));
        out.push_str(&format!("# TYPE {p}_active_connections gauge\n"));
        out.push_str(&format!(
            "{p}_active_connections {}\n",
            snapshot.active_connections
        ));

        // Counter: 驱逐事件
        out.push_str(&format!("# HELP {p}_evictions_total 驱逐事件总数\n"));
        out.push_str(&format!("# TYPE {p}_evictions_total counter\n"));
        out.push_str(&format!(
            "{p}_evictions_total {}\n",
            snapshot.eviction_total
        ));

        // Counter: 扩容事件
        out.push_str(&format!("# HELP {p}_scale_up_total 扩容事件总数\n"));
        out.push_str(&format!("# TYPE {p}_scale_up_total counter\n"));
        out.push_str(&format!("{p}_scale_up_total {}\n", snapshot.scale_up_total));

        // Counter: 缩容事件
        out.push_str(&format!("# HELP {p}_scale_down_total 缩容事件总数\n"));
        out.push_str(&format!("# TYPE {p}_scale_down_total counter\n"));
        out.push_str(&format!(
            "{p}_scale_down_total {}\n",
            snapshot.scale_down_total
        ));

        // Counter: acquire_timeout
        out.push_str(&format!(
            "# HELP {p}_timeout_total acquire_timeout 命中次数\n"
        ));
        out.push_str(&format!("# TYPE {p}_timeout_total counter\n"));
        out.push_str(&format!("{p}_timeout_total {}\n", snapshot.timeout_count));

        out
    }
}

impl Default for PoolMetricsExporter {
    fn default() -> Self {
        Self::new()
    }
}

/// 连接池事件追踪器（记录驱逐/扩缩容事件）
pub struct PoolEventTracker {
    eviction_total: std::sync::atomic::AtomicU64,
    scale_up_total: std::sync::atomic::AtomicU64,
    scale_down_total: std::sync::atomic::AtomicU64,
    timeout_count: std::sync::atomic::AtomicU64,
    total_acquire: std::sync::atomic::AtomicU64,
    started_at: Instant,
}

impl PoolEventTracker {
    /// 创建事件追踪器
    pub fn new() -> Self {
        Self {
            eviction_total: std::sync::atomic::AtomicU64::new(0),
            scale_up_total: std::sync::atomic::AtomicU64::new(0),
            scale_down_total: std::sync::atomic::AtomicU64::new(0),
            timeout_count: std::sync::atomic::AtomicU64::new(0),
            total_acquire: std::sync::atomic::AtomicU64::new(0),
            started_at: Instant::now(),
        }
    }

    /// 记录驱逐事件
    pub fn record_eviction(&self) {
        self.eviction_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// 记录扩容事件
    pub fn record_scale_up(&self) {
        self.scale_up_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// 记录缩容事件
    pub fn record_scale_down(&self) {
        self.scale_down_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// 记录 acquire（含 timeout 标记）
    pub fn record_acquire(&self, timed_out: bool) {
        self.total_acquire
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if timed_out {
            self.timeout_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// 生成指标快照
    pub fn snapshot(&self, current: usize, idle: usize, active: usize) -> PoolMetricsSnapshot {
        PoolMetricsSnapshot::new(
            current,
            idle,
            active,
            self.eviction_total
                .load(std::sync::atomic::Ordering::Relaxed),
            self.scale_up_total
                .load(std::sync::atomic::Ordering::Relaxed),
            self.scale_down_total
                .load(std::sync::atomic::Ordering::Relaxed),
            self.timeout_count
                .load(std::sync::atomic::Ordering::Relaxed),
            self.total_acquire
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// 运行时长
    pub fn uptime(&self) -> std::time::Duration {
        Instant::now().duration_since(self.started_at)
    }
}

impl Default for PoolEventTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_contains_all_metrics() {
        let exporter = PoolMetricsExporter::new();
        let snapshot = PoolMetricsSnapshot::new(10, 5, 5, 3, 2, 1, 4, 100);
        let output = exporter.export(&snapshot);
        assert!(output.contains("sz_rust_pool_connections 10"));
        assert!(output.contains("sz_rust_pool_idle_connections 5"));
        assert!(output.contains("sz_rust_pool_active_connections 5"));
        assert!(output.contains("sz_rust_pool_evictions_total 3"));
        assert!(output.contains("sz_rust_pool_scale_up_total 2"));
        assert!(output.contains("sz_rust_pool_scale_down_total 1"));
        assert!(output.contains("sz_rust_pool_timeout_total 4"));
    }

    #[test]
    fn test_export_has_prometheus_format() {
        let exporter = PoolMetricsExporter::new();
        let snapshot = PoolMetricsSnapshot::new(1, 1, 0, 0, 0, 0, 0, 0);
        let output = exporter.export(&snapshot);
        assert!(output.contains("# HELP"));
        assert!(output.contains("# TYPE"));
        assert!(output.contains("gauge"));
        assert!(output.contains("counter"));
    }

    #[test]
    fn test_export_custom_prefix() {
        let exporter = PoolMetricsExporter::with_prefix("my_pool");
        let snapshot = PoolMetricsSnapshot::new(5, 2, 3, 0, 0, 0, 0, 0);
        let output = exporter.export(&snapshot);
        assert!(output.contains("my_pool_connections 5"));
    }

    #[test]
    fn test_timeout_rate() {
        let snapshot = PoolMetricsSnapshot::new(10, 5, 5, 0, 0, 0, 10, 100);
        assert_eq!(snapshot.timeout_rate(), 0.1);
    }

    #[test]
    fn test_timeout_rate_zero_acquire() {
        let snapshot = PoolMetricsSnapshot::new(10, 5, 5, 0, 0, 0, 0, 0);
        assert_eq!(snapshot.timeout_rate(), 0.0);
    }

    #[test]
    fn test_event_tracker() {
        let tracker = PoolEventTracker::new();
        tracker.record_eviction();
        tracker.record_eviction();
        tracker.record_scale_up();
        tracker.record_scale_down();
        tracker.record_acquire(false);
        tracker.record_acquire(true);

        let snap = tracker.snapshot(10, 5, 5);
        assert_eq!(snap.eviction_total, 2);
        assert_eq!(snap.scale_up_total, 1);
        assert_eq!(snap.scale_down_total, 1);
        assert_eq!(snap.timeout_count, 1);
        assert_eq!(snap.total_acquire, 2);
    }

    #[test]
    fn test_event_tracker_uptime() {
        let tracker = PoolEventTracker::new();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(tracker.uptime() >= std::time::Duration::from_millis(10));
    }
}
