// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 健康探针增强 — 就绪/存活探针分离 + 依赖级联（spec §5.14）
//!
//! 就绪探针检查依赖可用 + 资源就绪，存活探针仅检查进程本身。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;

/// 探针状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ProbeStatus {
    /// 健康
    Healthy,
    /// 不健康（含原因）
    Unhealthy(String),
}

impl ProbeStatus {
    /// 是否健康
    pub fn is_healthy(&self) -> bool {
        matches!(self, ProbeStatus::Healthy)
    }
}

/// 探针配置（spec §6.14）
#[derive(Debug, Clone)]
pub struct ProbeConfig {
    /// 端点路径（默认 /readyz + /livez，spec §6.14 规则 1）
    pub readiness_path: String,
    /// 存活探针路径
    pub liveness_path: String,
    /// 超时（默认 <= 5000ms，spec §6.14 规则 2）
    pub timeout: Duration,
    /// 重试次数（默认 <= 3，spec §6.14 规则 3）
    pub retries: u32,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            readiness_path: "/readyz".to_string(),
            liveness_path: "/livez".to_string(),
            timeout: Duration::from_secs(5),
            retries: 3,
        }
    }
}

impl ProbeConfig {
    /// 验证配置（spec §6.14 规则 2/3）
    pub fn validate(&self) -> Result<(), String> {
        if self.timeout > Duration::from_millis(5000) {
            return Err(format!(
                "超时必须 <= 5000ms，当前: {}ms",
                self.timeout.as_millis()
            ));
        }
        if self.retries > 3 {
            return Err(format!("重试次数必须 <= 3，当前: {}", self.retries));
        }
        Ok(())
    }
}

/// 依赖健康检查 trait（级联检查，spec §5.14 规则 2）
#[async_trait]
pub trait DependencyHealthCheck: Send + Sync {
    /// 检查器名称
    fn name(&self) -> &str;

    /// 检查依赖健康（超时按失败处理）
    async fn check(&self) -> ProbeStatus;
}

/// 就绪探针（检查依赖可用 + 资源就绪，spec §5.14 规则 1）
pub struct ReadinessProbe {
    dependencies: Vec<Arc<dyn DependencyHealthCheck>>,
    config: ProbeConfig,
}

impl ReadinessProbe {
    /// 创建就绪探针
    pub fn new(config: ProbeConfig) -> Self {
        Self {
            dependencies: Vec::new(),
            config,
        }
    }

    /// 添加依赖检查
    pub fn with_dependency(mut self, check: Arc<dyn DependencyHealthCheck>) -> Self {
        self.dependencies.push(check);
        self
    }

    /// 执行就绪检查（级联，任一依赖不健康 → not-ready，spec §5.14 规则 2）
    ///
    /// # 后置条件
    /// - 所有依赖健康 → Healthy
    /// - 任一依赖不健康 → Unhealthy（含失败依赖名）
    pub async fn check(&self) -> ProbeStatus {
        for dep in &self.dependencies {
            let status = tokio::time::timeout(self.config.timeout, dep.check())
                .await
                .unwrap_or_else(|_| ProbeStatus::Unhealthy(format!("{} 超时", dep.name())));
            if !status.is_healthy() {
                if let ProbeStatus::Unhealthy(reason) = status {
                    return ProbeStatus::Unhealthy(format!("{}: {}", dep.name(), reason));
                }
                return ProbeStatus::Unhealthy(dep.name().to_string());
            }
        }
        ProbeStatus::Healthy
    }

    /// 依赖数量
    pub fn dependency_count(&self) -> usize {
        self.dependencies.len()
    }

    /// 获取配置
    pub fn config(&self) -> &ProbeConfig {
        &self.config
    }
}

/// 存活探针（仅检查进程本身，不检查依赖，spec §5.14 禁止项）
pub struct LivenessProbe;

impl LivenessProbe {
    /// 创建存活探针
    pub fn new() -> Self {
        Self
    }

    /// 执行存活检查
    ///
    /// # 后置条件
    /// - 进程存活 → Healthy
    /// - 进程僵死 → 无法执行此函数
    pub async fn check(&self) -> ProbeStatus {
        ProbeStatus::Healthy
    }
}

impl Default for LivenessProbe {
    fn default() -> Self {
        Self::new()
    }
}

/// 探针检查结果（含就绪和存活状态）
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    /// 就绪状态
    pub ready: ProbeStatus,
    /// 存活状态
    pub alive: ProbeStatus,
}

/// 组合探针（同时暴露就绪和存活）
pub struct HealthProbe {
    readiness: ReadinessProbe,
    liveness: LivenessProbe,
}

impl HealthProbe {
    /// 创建组合探针
    pub fn new(config: ProbeConfig) -> Self {
        Self {
            readiness: ReadinessProbe::new(config),
            liveness: LivenessProbe::new(),
        }
    }

    /// 添加依赖检查
    pub fn with_dependency(mut self, check: Arc<dyn DependencyHealthCheck>) -> Self {
        self.readiness.dependencies.push(check);
        self
    }

    /// 执行全部检查
    pub async fn check(&self) -> ProbeResult {
        ProbeResult {
            ready: self.readiness.check().await,
            alive: self.liveness.check().await,
        }
    }

    /// 获取就绪探针
    pub fn readiness(&self) -> &ReadinessProbe {
        &self.readiness
    }

    /// 获取存活探针
    pub fn liveness(&self) -> &LivenessProbe {
        &self.liveness
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HealthyDep {
        name: String,
    }

    #[async_trait]
    impl DependencyHealthCheck for HealthyDep {
        fn name(&self) -> &str {
            &self.name
        }
        async fn check(&self) -> ProbeStatus {
            ProbeStatus::Healthy
        }
    }

    struct UnhealthyDep {
        name: String,
        reason: String,
    }

    #[async_trait]
    impl DependencyHealthCheck for UnhealthyDep {
        fn name(&self) -> &str {
            &self.name
        }
        async fn check(&self) -> ProbeStatus {
            ProbeStatus::Unhealthy(self.reason.clone())
        }
    }

    struct SlowDep {
        name: String,
    }

    #[async_trait]
    impl DependencyHealthCheck for SlowDep {
        fn name(&self) -> &str {
            &self.name
        }
        async fn check(&self) -> ProbeStatus {
            tokio::time::sleep(Duration::from_secs(10)).await;
            ProbeStatus::Healthy
        }
    }

    #[test]
    fn test_probe_config_validate_ok() {
        let config = ProbeConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_probe_config_validate_timeout_too_long() {
        let config = ProbeConfig {
            timeout: Duration::from_secs(6),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_probe_config_validate_too_many_retries() {
        let config = ProbeConfig {
            retries: 5,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[tokio::test]
    async fn test_readiness_no_dependencies() {
        let probe = ReadinessProbe::new(ProbeConfig::default());
        assert_eq!(probe.check().await, ProbeStatus::Healthy);
    }

    #[tokio::test]
    async fn test_readiness_all_healthy() {
        let probe = ReadinessProbe::new(ProbeConfig::default())
            .with_dependency(Arc::new(HealthyDep {
                name: "db".to_string(),
            }))
            .with_dependency(Arc::new(HealthyDep {
                name: "cache".to_string(),
            }));
        assert_eq!(probe.check().await, ProbeStatus::Healthy);
    }

    #[tokio::test]
    async fn test_readiness_one_unhealthy() {
        let probe = ReadinessProbe::new(ProbeConfig::default())
            .with_dependency(Arc::new(HealthyDep {
                name: "db".to_string(),
            }))
            .with_dependency(Arc::new(UnhealthyDep {
                name: "cache".to_string(),
                reason: "connection refused".to_string(),
            }));
        let status = probe.check().await;
        assert!(!status.is_healthy());
        assert!(matches!(status, ProbeStatus::Unhealthy(ref r) if r.contains("cache")));
    }

    #[tokio::test]
    async fn test_readiness_timeout() {
        let probe = ReadinessProbe::new(ProbeConfig {
            timeout: Duration::from_millis(100),
            ..Default::default()
        })
        .with_dependency(Arc::new(SlowDep {
            name: "slow".to_string(),
        }));
        let status = probe.check().await;
        assert!(!status.is_healthy());
        assert!(matches!(status, ProbeStatus::Unhealthy(ref r) if r.contains("超时")));
    }

    #[tokio::test]
    async fn test_liveness_always_healthy() {
        let probe = LivenessProbe::new();
        assert_eq!(probe.check().await, ProbeStatus::Healthy);
    }

    #[tokio::test]
    async fn test_health_probe_both_healthy() {
        let probe =
            HealthProbe::new(ProbeConfig::default()).with_dependency(Arc::new(HealthyDep {
                name: "db".to_string(),
            }));
        let result = probe.check().await;
        assert!(result.ready.is_healthy());
        assert!(result.alive.is_healthy());
    }

    #[tokio::test]
    async fn test_health_probe_ready_unhealthy() {
        let probe =
            HealthProbe::new(ProbeConfig::default()).with_dependency(Arc::new(UnhealthyDep {
                name: "db".to_string(),
                reason: "down".to_string(),
            }));
        let result = probe.check().await;
        assert!(!result.ready.is_healthy());
        assert!(result.alive.is_healthy());
    }

    #[test]
    fn test_probe_status_is_healthy() {
        assert!(ProbeStatus::Healthy.is_healthy());
        assert!(!ProbeStatus::Unhealthy("err".to_string()).is_healthy());
    }
}
