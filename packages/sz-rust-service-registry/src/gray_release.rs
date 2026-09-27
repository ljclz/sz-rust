// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 灰度发布（P3-2）
//!
//! 按权重将流量逐步切换到新版本实例 + 失败率阈值自动回滚。

use crate::gray_rollback::{HealthStats, RollbackDecision};
use crate::registry::{InstanceStatus, ServiceInstance};

/// 灰度发布配置
#[derive(Debug, Clone)]
pub struct GrayReleaseConfig {
    /// 新版本流量权重（0~1）
    pub new_version_weight: f32,
    /// 失败率回滚阈值（0~1）
    pub failure_rate_threshold: f32,
    /// 连续失败次数阈值（避免抖动误判）
    pub consecutive_failures: u32,
}

impl Default for GrayReleaseConfig {
    fn default() -> Self {
        Self {
            new_version_weight: 0.1,
            failure_rate_threshold: 0.5,
            consecutive_failures: 3,
        }
    }
}

impl GrayReleaseConfig {
    /// 创建配置，权重自动钳制到 [0, 1]
    pub fn new(
        new_version_weight: f32,
        failure_rate_threshold: f32,
        consecutive_failures: u32,
    ) -> Self {
        Self {
            new_version_weight: new_version_weight.clamp(0.0, 1.0),
            failure_rate_threshold: failure_rate_threshold.clamp(0.0, 1.0),
            consecutive_failures: consecutive_failures.max(1),
        }
    }
}

/// 灰度发布路由器
///
/// 按配置权重将流量分配到新旧版本实例，
/// 仅路由到健康实例，已摘除实例不分发流量。
pub struct GrayRelease {
    config: GrayReleaseConfig,
}

impl GrayRelease {
    /// 创建灰度发布路由器
    pub fn new(config: GrayReleaseConfig) -> Self {
        Self { config }
    }

    /// 获取配置引用
    pub fn config(&self) -> &GrayReleaseConfig {
        &self.config
    }

    /// 路由决策：返回可接收流量的实例列表
    ///
    /// - 仅返回健康实例（`status == Healthy`）
    /// - 新版本实例（`metadata["release"] == "canary"`）权重乘以 `new_version_weight`
    /// - 旧版本实例权重乘以 `(1 - new_version_weight)`
    /// - `new_version_weight == 0` 时仅返回旧版本
    /// - `new_version_weight == 1` 时仅返回新版本
    pub fn route(&self, instances: &[ServiceInstance]) -> Vec<ServiceInstance> {
        let healthy: Vec<&ServiceInstance> = instances
            .iter()
            .filter(|i| i.status == InstanceStatus::Healthy)
            .collect();

        let mut result: Vec<ServiceInstance> = Vec::new();
        let w_new = self.config.new_version_weight;
        let w_old = 1.0 - w_new;

        for inst in healthy {
            let is_canary = inst
                .metadata
                .get("release")
                .map(|v| v == "canary")
                .unwrap_or(false);

            let scale = if is_canary { w_new } else { w_old };
            if scale <= 0.0 {
                continue;
            }

            let mut cloned = inst.clone();
            cloned.weight = ((cloned.weight as f32) * scale).max(1.0) as u32;
            result.push(cloned);
        }

        result
    }

    /// 检查是否需要回滚
    ///
    /// 当失败率超过阈值 **且** 连续失败次数达到阈值时，返回 `Rollback`。
    /// 连续失败阈值避免因瞬时抖动误判回滚。
    pub fn check_rollback(&self, stats: &HealthStats) -> RollbackDecision {
        if stats.failure_rate > self.config.failure_rate_threshold
            && stats.consecutive_failures >= self.config.consecutive_failures
        {
            RollbackDecision::Rollback {
                failure_rate: stats.failure_rate,
                consecutive_failures: stats.consecutive_failures,
            }
        } else {
            RollbackDecision::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_instance(id: &str, release: Option<&str>, status: InstanceStatus) -> ServiceInstance {
        let mut inst = ServiceInstance::new("svc", "127.0.0.1", 8080);
        inst.instance_id = id.to_string();
        if let Some(r) = release {
            inst.metadata.insert("release".to_string(), r.to_string());
        }
        inst.status = status;
        inst
    }

    #[test]
    fn test_config_clamp() {
        let cfg = GrayReleaseConfig::new(-0.5, 1.5, 0);
        assert_eq!(cfg.new_version_weight, 0.0);
        assert_eq!(cfg.failure_rate_threshold, 1.0);
        assert_eq!(cfg.consecutive_failures, 1);
    }

    #[test]
    fn test_route_only_healthy() {
        let instances = vec![
            make_instance("old-1", None, InstanceStatus::Healthy),
            make_instance("old-2", None, InstanceStatus::Unhealthy),
            make_instance("old-3", None, InstanceStatus::Maintenance),
        ];
        let gr = GrayRelease::new(GrayReleaseConfig::default());
        let routed = gr.route(&instances);
        assert_eq!(routed.len(), 1, "only healthy instances routed");
        assert_eq!(routed[0].instance_id, "old-1");
    }

    #[test]
    fn test_route_weight_split() {
        let instances = vec![
            make_instance("old-1", None, InstanceStatus::Healthy),
            make_instance("canary-1", Some("canary"), InstanceStatus::Healthy),
        ];
        let cfg = GrayReleaseConfig::new(0.1, 0.5, 3);
        let gr = GrayRelease::new(cfg);
        let routed = gr.route(&instances);
        assert_eq!(routed.len(), 2);

        let old = routed.iter().find(|i| i.instance_id == "old-1").unwrap();
        let canary = routed.iter().find(|i| i.instance_id == "canary-1").unwrap();
        assert_eq!(old.weight, 1, "old weight = 1 * 0.9 = 0.9 -> max(1) = 1");
        assert_eq!(
            canary.weight, 1,
            "canary weight = 1 * 0.1 = 0.1 -> max(1) = 1"
        );
    }

    #[test]
    fn test_route_zero_new_weight() {
        let instances = vec![
            make_instance("old-1", None, InstanceStatus::Healthy),
            make_instance("canary-1", Some("canary"), InstanceStatus::Healthy),
        ];
        let cfg = GrayReleaseConfig::new(0.0, 0.5, 3);
        let gr = GrayRelease::new(cfg);
        let routed = gr.route(&instances);
        assert_eq!(routed.len(), 1);
        assert_eq!(routed[0].instance_id, "old-1");
    }

    #[test]
    fn test_route_full_new_weight() {
        let instances = vec![
            make_instance("old-1", None, InstanceStatus::Healthy),
            make_instance("canary-1", Some("canary"), InstanceStatus::Healthy),
        ];
        let cfg = GrayReleaseConfig::new(1.0, 0.5, 3);
        let gr = GrayRelease::new(cfg);
        let routed = gr.route(&instances);
        assert_eq!(routed.len(), 1);
        assert_eq!(routed[0].instance_id, "canary-1");
    }

    #[test]
    fn test_check_rollback_continue() {
        let gr = GrayRelease::new(GrayReleaseConfig::new(0.1, 0.5, 3));
        let stats = HealthStats {
            failure_rate: 0.3,
            consecutive_failures: 2,
        };
        assert_eq!(gr.check_rollback(&stats), RollbackDecision::Continue);
    }

    #[test]
    fn test_check_rollback_triggered() {
        let gr = GrayRelease::new(GrayReleaseConfig::new(0.1, 0.5, 3));
        let stats = HealthStats {
            failure_rate: 0.6,
            consecutive_failures: 3,
        };
        let decision = gr.check_rollback(&stats);
        match decision {
            RollbackDecision::Rollback {
                failure_rate,
                consecutive_failures,
            } => {
                assert!((failure_rate - 0.6).abs() < f32::EPSILON);
                assert_eq!(consecutive_failures, 3);
            }
            _ => panic!("expected Rollback"),
        }
    }

    #[test]
    fn test_check_rollback_low_failure_rate_no_rollback() {
        let gr = GrayRelease::new(GrayReleaseConfig::new(0.1, 0.5, 3));
        let stats = HealthStats {
            failure_rate: 0.4,
            consecutive_failures: 10,
        };
        assert_eq!(gr.check_rollback(&stats), RollbackDecision::Continue);
    }

    #[test]
    fn test_check_rollback_low_consecutive_no_rollback() {
        let gr = GrayRelease::new(GrayReleaseConfig::new(0.1, 0.5, 3));
        let stats = HealthStats {
            failure_rate: 0.9,
            consecutive_failures: 2,
        };
        assert_eq!(gr.check_rollback(&stats), RollbackDecision::Continue);
    }
}
