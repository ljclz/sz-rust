// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! PoolScaler — 连接池动态扩容（P3 L3 调优）
//!
//! 监控连接池 acquire_timeout 命中率，高时扩容 max_connections，
//! 低时回收空闲连接。受 sz-orm PoolConfig 上限约束。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// PoolScaler 配置
#[derive(Debug, Clone)]
pub struct PoolScalerConfig {
    /// 扩容阈值（acquire_timeout 命中率 > 此值时扩容）
    pub scale_up_threshold: f64,
    /// 回收阈值（空闲连接比例 > 此值时回收）
    pub scale_down_threshold: f64,
    /// 检查间隔
    pub check_interval: Duration,
    /// 最大连接数上限
    pub max_connections: usize,
    /// 最小连接数下限
    pub min_connections: usize,
}

impl Default for PoolScalerConfig {
    fn default() -> Self {
        Self {
            scale_up_threshold: 0.3,
            scale_down_threshold: 0.7,
            check_interval: Duration::from_secs(30),
            max_connections: 100,
            min_connections: 5,
        }
    }
}

/// 连接池指标快照
#[derive(Debug, Clone)]
pub struct PoolMetrics {
    /// 当前连接数
    pub current_connections: usize,
    /// 空闲连接数
    pub idle_connections: usize,
    /// acquire_timeout 命中次数
    pub timeout_count: u64,
    /// 总 acquire 次数
    pub total_acquire: u64,
}

impl PoolMetrics {
    /// acquire_timeout 命中率
    pub fn timeout_rate(&self) -> f64 {
        if self.total_acquire == 0 {
            0.0
        } else {
            self.timeout_count as f64 / self.total_acquire as f64
        }
    }

    /// 空闲连接比例
    pub fn idle_rate(&self) -> f64 {
        if self.current_connections == 0 {
            0.0
        } else {
            self.idle_connections as f64 / self.current_connections as f64
        }
    }
}

/// 连接池动态扩容器
pub struct PoolScaler {
    config: PoolScalerConfig,
    target_connections: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
    scale_up_count: Arc<AtomicUsize>,
    scale_down_count: Arc<AtomicUsize>,
}

impl PoolScaler {
    /// 创建 PoolScaler
    pub fn new(config: PoolScalerConfig) -> Self {
        let initial = config.min_connections;
        Self {
            config,
            target_connections: Arc::new(AtomicUsize::new(initial)),
            running: Arc::new(AtomicBool::new(false)),
            scale_up_count: Arc::new(AtomicUsize::new(0)),
            scale_down_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 当前目标连接数
    pub fn target_connections(&self) -> usize {
        self.target_connections.load(Ordering::Acquire)
    }

    /// 是否正在运行
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// 扩容次数
    pub fn scale_up_count(&self) -> usize {
        self.scale_up_count.load(Ordering::Relaxed)
    }

    /// 回收次数
    pub fn scale_down_count(&self) -> usize {
        self.scale_down_count.load(Ordering::Relaxed)
    }

    /// 根据指标扩容
    pub fn scale_up(&self, metrics: &PoolMetrics) {
        if metrics.timeout_rate() > self.config.scale_up_threshold {
            let current = self.target_connections.load(Ordering::Relaxed);
            let new_target = (current + (current / 4)).min(self.config.max_connections);
            self.target_connections.store(new_target, Ordering::Release);
            self.scale_up_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 根据指标回收
    pub fn scale_down(&self, metrics: &PoolMetrics) {
        if metrics.idle_rate() > self.config.scale_down_threshold {
            let current = self.target_connections.load(Ordering::Relaxed);
            let new_target = (current - (current / 4)).max(self.config.min_connections);
            self.target_connections.store(new_target, Ordering::Release);
            self.scale_down_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 根据指标自动调整（扩容或回收）
    pub fn adjust(&self, metrics: &PoolMetrics) {
        if metrics.timeout_rate() > self.config.scale_up_threshold {
            self.scale_up(metrics);
        } else if metrics.idle_rate() > self.config.scale_down_threshold {
            self.scale_down(metrics);
        }
    }
}

impl std::fmt::Debug for PoolScaler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let target = self.target_connections.load(Ordering::Acquire);
        write!(
            f,
            "PoolScaler {{ target: {target}, running: {} }}",
            self.running.load(Ordering::Relaxed)
        )
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_metrics_timeout_rate() {
        let metrics = PoolMetrics {
            current_connections: 10,
            idle_connections: 3,
            timeout_count: 5,
            total_acquire: 100,
        };
        assert_eq!(metrics.timeout_rate(), 0.05);
    }

    #[test]
    fn test_pool_metrics_timeout_rate_zero() {
        let metrics = PoolMetrics {
            current_connections: 10,
            idle_connections: 3,
            timeout_count: 0,
            total_acquire: 0,
        };
        assert_eq!(metrics.timeout_rate(), 0.0);
    }

    #[test]
    fn test_pool_metrics_idle_rate() {
        let metrics = PoolMetrics {
            current_connections: 10,
            idle_connections: 7,
            timeout_count: 0,
            total_acquire: 100,
        };
        assert_eq!(metrics.idle_rate(), 0.7);
    }

    #[test]
    fn test_config_default() {
        let config = PoolScalerConfig::default();
        assert_eq!(config.scale_up_threshold, 0.3);
        assert_eq!(config.scale_down_threshold, 0.7);
        assert_eq!(config.check_interval, Duration::from_secs(30));
    }

    #[test]
    fn test_scaler_new() {
        let config = PoolScalerConfig::default();
        let scaler = PoolScaler::new(config);
        assert_eq!(scaler.target_connections(), 5);
        assert!(!scaler.is_running());
    }

    #[test]
    fn test_scale_up() {
        let config = PoolScalerConfig::default();
        let scaler = PoolScaler::new(config);
        scaler.target_connections.store(10, Ordering::Relaxed);
        let metrics = PoolMetrics {
            current_connections: 10,
            idle_connections: 2,
            timeout_count: 50,
            total_acquire: 100,
        };
        scaler.scale_up(&metrics);
        assert!(scaler.target_connections() > 10);
        assert_eq!(scaler.scale_up_count(), 1);
    }

    #[test]
    fn test_scale_down() {
        let config = PoolScalerConfig::default();
        let scaler = PoolScaler::new(config);
        scaler.target_connections.store(20, Ordering::Relaxed);
        let metrics = PoolMetrics {
            current_connections: 20,
            idle_connections: 18,
            timeout_count: 0,
            total_acquire: 100,
        };
        scaler.scale_down(&metrics);
        assert!(scaler.target_connections() < 20);
        assert_eq!(scaler.scale_down_count(), 1);
    }

    #[test]
    fn test_scale_up_max_cap() {
        let config = PoolScalerConfig::default();
        let scaler = PoolScaler::new(config);
        scaler.target_connections.store(95, Ordering::Relaxed);
        let metrics = PoolMetrics {
            current_connections: 95,
            idle_connections: 0,
            timeout_count: 50,
            total_acquire: 100,
        };
        scaler.scale_up(&metrics);
        assert_eq!(scaler.target_connections(), 100);
    }

    #[test]
    fn test_scale_down_min_cap() {
        let config = PoolScalerConfig::default();
        let scaler = PoolScaler::new(config);
        scaler.target_connections.store(6, Ordering::Relaxed);
        let metrics = PoolMetrics {
            current_connections: 6,
            idle_connections: 5,
            timeout_count: 0,
            total_acquire: 100,
        };
        scaler.scale_down(&metrics);
        // 6 - 6/4 = 6 - 1 = 5, max(5) = 5
        assert_eq!(scaler.target_connections(), 5);
    }

    #[test]
    fn test_adjust_scale_up() {
        let config = PoolScalerConfig::default();
        let scaler = PoolScaler::new(config);
        scaler.target_connections.store(10, Ordering::Relaxed);
        let metrics = PoolMetrics {
            current_connections: 10,
            idle_connections: 0,
            timeout_count: 50,
            total_acquire: 100,
        };
        scaler.adjust(&metrics);
        assert_eq!(scaler.scale_up_count(), 1);
        assert_eq!(scaler.scale_down_count(), 0);
    }

    #[test]
    fn test_adjust_scale_down() {
        let config = PoolScalerConfig::default();
        let scaler = PoolScaler::new(config);
        scaler.target_connections.store(20, Ordering::Relaxed);
        let metrics = PoolMetrics {
            current_connections: 20,
            idle_connections: 18,
            timeout_count: 0,
            total_acquire: 100,
        };
        scaler.adjust(&metrics);
        assert_eq!(scaler.scale_up_count(), 0);
        assert_eq!(scaler.scale_down_count(), 1);
    }

    #[test]
    fn test_pool_metrics_idle_rate_zero_connections() {
        let metrics = PoolMetrics {
            current_connections: 0,
            idle_connections: 0,
            timeout_count: 0,
            total_acquire: 0,
        };
        assert_eq!(metrics.idle_rate(), 0.0);
    }

    #[test]
    fn test_pool_scaler_debug_format() {
        let scaler = PoolScaler::new(PoolScalerConfig::default());
        let s = format!("{scaler:?}");
        assert!(s.contains("PoolScaler"));
        assert!(s.contains("target: 5"));
    }
}
