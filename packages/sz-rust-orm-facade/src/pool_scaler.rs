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

    /// 基于当前指标快照给出连接池调优建议。
    ///
    /// 以 `current_connections` 作为并发量调用 [`recommend_pool_size`]。
    ///
    /// # 示例
    ///
    /// ```
    /// use sz_rust_orm_facade::pool_scaler::PoolMetrics;
    ///
    /// let metrics = PoolMetrics {
    ///     current_connections: 8,
    ///     idle_connections: 2,
    ///     timeout_count: 0,
    ///     total_acquire: 100,
    /// };
    /// let advice = metrics.tuning_advice();
    /// assert_eq!(advice.recommended_pool_size, 12);
    /// ```
    pub fn tuning_advice(&self) -> PoolTuningAdvice {
        recommend_pool_size(self.current_connections, 0)
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
// T050 连接池自适应调优建议
// ============================================================================

/// 连接池调优建议
#[derive(Debug, Clone)]
pub struct PoolTuningAdvice {
    /// 推荐的连接池大小
    pub recommended_pool_size: usize,
    /// 预期利用率（0.0 ~ 1.0）
    pub expected_utilization: f64,
    /// 推荐原因
    pub reason: String,
}

/// 连接池默认最大连接数上限
const DEFAULT_MAX_POOL_CONNECTIONS: usize = 100;

/// 基于并发量与平均查询时长推荐最优连接池大小。
///
/// 推荐公式：`pool_size = ceil(concurrent_connections * 1.2) + 2`（含安全余量），
/// 并限制不超过默认上限 100。预期利用率为
/// `concurrent_connections / recommended_pool_size`。
///
/// # 示例
///
/// ```
/// use sz_rust_orm_facade::pool_scaler::recommend_pool_size;
///
/// let advice = recommend_pool_size(10, 5);
/// assert_eq!(advice.recommended_pool_size, 14);
/// assert!((advice.expected_utilization - (10.0 / 14.0)).abs() < 1e-9);
/// assert!(advice.reason.contains("并发量 10"));
/// ```
pub fn recommend_pool_size(
    concurrent_connections: usize,
    avg_query_duration_ms: u64,
) -> PoolTuningAdvice {
    let scaled = (concurrent_connections * 6).div_ceil(5);
    let raw = scaled + 2;
    let max = DEFAULT_MAX_POOL_CONNECTIONS;
    let recommended_pool_size = raw.min(max);
    let expected_utilization = concurrent_connections as f64 / recommended_pool_size as f64;
    let duration_part = if avg_query_duration_ms > 0 {
        format!("，平均查询时长 {avg_query_duration_ms} ms")
    } else {
        String::new()
    };
    let reason = if raw > max {
        format!(
            "并发量 {concurrent_connections}{duration_part} 所需池大小 {raw} 超过上限 {max}，已截断至 {max}"
        )
    } else {
        format!(
            "基于并发量 {concurrent_connections}{duration_part} 按 ceil(n*1.2)+2 推荐池大小 {recommended_pool_size}，预期利用率 {expected_utilization:.3}"
        )
    };
    PoolTuningAdvice {
        recommended_pool_size,
        expected_utilization,
        reason,
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

    #[test]
    fn test_recommend_pool_size_basic() {
        let advice = recommend_pool_size(10, 5);
        assert_eq!(advice.recommended_pool_size, 14);
        assert!((advice.expected_utilization - (10.0 / 14.0)).abs() < 1e-9);
        assert!(advice.reason.contains("并发量 10"));
        assert!(advice.reason.contains("5 ms"));
    }

    #[test]
    fn test_recommend_pool_size_zero_concurrency() {
        let advice = recommend_pool_size(0, 0);
        assert_eq!(advice.recommended_pool_size, 2);
        assert_eq!(advice.expected_utilization, 0.0);
    }

    #[test]
    fn test_recommend_pool_size_capped_at_max() {
        let advice = recommend_pool_size(100, 10);
        assert_eq!(advice.recommended_pool_size, 100);
        assert!(advice.reason.contains("截断"));
    }

    #[test]
    fn test_tuning_advice_from_metrics() {
        let metrics = PoolMetrics {
            current_connections: 8,
            idle_connections: 2,
            timeout_count: 0,
            total_acquire: 100,
        };
        let advice = metrics.tuning_advice();
        assert_eq!(advice.recommended_pool_size, 12);
        assert!((advice.expected_utilization - (8.0 / 12.0)).abs() < 1e-9);
    }
}
