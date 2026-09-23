// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 内存泄漏告警守卫
//!
//! 通过外部注入当前内存使用量（避免对 `sysinfo` 的硬依赖），
//! 在超过阈值时返回 [`MemoryStatus::Warning`] / [`MemoryStatus::Critical`]，
//! 供上层中间件 / 周期巡检任务据此告警或熔断。
//!
//! # 设计
//!
//! - 阈值与采样解耦：`MemoryGuard` 仅持有阈值配置 + 当前内存值（`AtomicU64`）。
//! - 当前内存值由外部通过 [`MemoryGuard::set_current_memory_mb`] 注入，
//!   生产环境可由 `sysinfo` / `/proc` / Admin Monitor API 等任意来源填充。
//! - 全部使用 `std::sync::atomic`，无锁，线程安全。
//!
//! # 示例
//!
//! ```
//! use sz_rust_observability::{MemoryGuard, MemoryGuardConfig, MemoryStatus};
//! use std::time::Duration;
//!
//! let config = MemoryGuardConfig {
//!     warning_threshold_mb: 512,
//!     critical_threshold_mb: 1024,
//!     check_interval: Duration::from_secs(10),
//! };
//! let guard = MemoryGuard::new(config);
//! guard.set_current_memory_mb(600);
//! assert_eq!(guard.check(), MemoryStatus::Warning { used_mb: 600, threshold_mb: 512 });
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// 内存告警配置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryGuardConfig {
    /// 警告阈值（MB），超过返回 [`MemoryStatus::Warning`]
    pub warning_threshold_mb: u64,
    /// 严重阈值（MB），超过返回 [`MemoryStatus::Critical`]
    pub critical_threshold_mb: u64,
    /// 巡检间隔（仅作为配置项，实际调度由上层负责）
    pub check_interval: Duration,
}

impl Default for MemoryGuardConfig {
    fn default() -> Self {
        Self {
            warning_threshold_mb: 512,
            critical_threshold_mb: 1024,
            check_interval: Duration::from_secs(10),
        }
    }
}

/// 内存状态判定结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryStatus {
    /// 正常（`used_mb` < `warning_threshold_mb`）
    Normal {
        /// 当前已用内存（MB）
        used_mb: u64,
    },
    /// 警告（`warning_threshold_mb` <= `used_mb` < `critical_threshold_mb`）
    Warning {
        /// 当前已用内存（MB）
        used_mb: u64,
        /// 触发的告警阈值（MB）
        threshold_mb: u64,
    },
    /// 严重（`used_mb` >= `critical_threshold_mb`）
    Critical {
        /// 当前已用内存（MB）
        used_mb: u64,
        /// 触发的严重阈值（MB）
        threshold_mb: u64,
    },
}

/// 内存泄漏告警守卫
///
/// 持有阈值配置 + 当前内存值（原子），由外部注入采样值。
pub struct MemoryGuard {
    config: MemoryGuardConfig,
    current_mb: AtomicU64,
}

impl MemoryGuard {
    /// 创建守卫，初始内存使用量记为 0
    pub fn new(config: MemoryGuardConfig) -> Self {
        Self {
            config,
            current_mb: AtomicU64::new(0),
        }
    }

    /// 由外部注入当前已用内存（MB）
    ///
    /// 生产环境可由 `sysinfo` / `/proc/meminfo` / Admin Monitor API 调用。
    pub fn set_current_memory_mb(&self, used_mb: u64) {
        self.current_mb.store(used_mb, Ordering::Relaxed);
    }

    /// 读取当前已用内存（MB）
    pub fn current_memory_mb(&self) -> u64 {
        self.current_mb.load(Ordering::Relaxed)
    }

    /// 返回配置引用
    pub fn config(&self) -> &MemoryGuardConfig {
        &self.config
    }

    /// 判定当前内存状态
    ///
    /// - `used_mb >= critical_threshold_mb` → [`MemoryStatus::Critical`]
    /// - `used_mb >= warning_threshold_mb` → [`MemoryStatus::Warning`]
    /// - 否则 → [`MemoryStatus::Normal`]
    pub fn check(&self) -> MemoryStatus {
        let used_mb = self.current_memory_mb();
        if used_mb >= self.config.critical_threshold_mb {
            MemoryStatus::Critical {
                used_mb,
                threshold_mb: self.config.critical_threshold_mb,
            }
        } else if used_mb >= self.config.warning_threshold_mb {
            MemoryStatus::Warning {
                used_mb,
                threshold_mb: self.config.warning_threshold_mb,
            }
        } else {
            MemoryStatus::Normal { used_mb }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_thresholds() {
        let cfg = MemoryGuardConfig::default();
        assert_eq!(cfg.warning_threshold_mb, 512);
        assert_eq!(cfg.critical_threshold_mb, 1024);
        assert_eq!(cfg.check_interval, Duration::from_secs(10));
    }

    #[test]
    fn test_normal_status_below_warning() {
        let guard = MemoryGuard::new(MemoryGuardConfig::default());
        guard.set_current_memory_mb(100);
        assert_eq!(guard.check(), MemoryStatus::Normal { used_mb: 100 });
    }

    #[test]
    fn test_normal_status_zero() {
        let guard = MemoryGuard::new(MemoryGuardConfig::default());
        assert_eq!(guard.check(), MemoryStatus::Normal { used_mb: 0 });
    }

    #[test]
    fn test_warning_status_at_threshold() {
        let guard = MemoryGuard::new(MemoryGuardConfig::default());
        guard.set_current_memory_mb(512);
        assert_eq!(
            guard.check(),
            MemoryStatus::Warning {
                used_mb: 512,
                threshold_mb: 512,
            }
        );
    }

    #[test]
    fn test_warning_status_between_thresholds() {
        let guard = MemoryGuard::new(MemoryGuardConfig::default());
        guard.set_current_memory_mb(800);
        assert_eq!(
            guard.check(),
            MemoryStatus::Warning {
                used_mb: 800,
                threshold_mb: 512,
            }
        );
    }

    #[test]
    fn test_critical_status_at_threshold() {
        let guard = MemoryGuard::new(MemoryGuardConfig::default());
        guard.set_current_memory_mb(1024);
        assert_eq!(
            guard.check(),
            MemoryStatus::Critical {
                used_mb: 1024,
                threshold_mb: 1024,
            }
        );
    }

    #[test]
    fn test_critical_status_above_threshold() {
        let guard = MemoryGuard::new(MemoryGuardConfig::default());
        guard.set_current_memory_mb(2048);
        assert_eq!(
            guard.check(),
            MemoryStatus::Critical {
                used_mb: 2048,
                threshold_mb: 1024,
            }
        );
    }

    #[test]
    fn test_custom_config() {
        let config = MemoryGuardConfig {
            warning_threshold_mb: 100,
            critical_threshold_mb: 200,
            check_interval: Duration::from_secs(5),
        };
        let guard = MemoryGuard::new(config);
        guard.set_current_memory_mb(150);
        assert_eq!(
            guard.check(),
            MemoryStatus::Warning {
                used_mb: 150,
                threshold_mb: 100,
            }
        );
    }

    #[test]
    fn test_current_memory_mb_round_trip() {
        let guard = MemoryGuard::new(MemoryGuardConfig::default());
        guard.set_current_memory_mb(42);
        assert_eq!(guard.current_memory_mb(), 42);
    }

    #[test]
    fn test_config_accessor() {
        let config = MemoryGuardConfig::default();
        let guard = MemoryGuard::new(config);
        assert_eq!(guard.config().warning_threshold_mb, 512);
        assert_eq!(guard.config().critical_threshold_mb, 1024);
    }

    #[test]
    fn test_concurrent_set_and_check() {
        use std::sync::Arc;
        let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig::default()));
        let g1 = guard.clone();
        let h1 = std::thread::spawn(move || {
            for v in 0..100u64 {
                g1.set_current_memory_mb(v);
            }
        });
        let g2 = guard.clone();
        let h2 = std::thread::spawn(move || {
            for _ in 0..100 {
                let _ = g2.check();
            }
        });
        h1.join().unwrap();
        h2.join().unwrap();
    }
}
