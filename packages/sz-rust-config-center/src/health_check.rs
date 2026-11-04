// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 配置中心健康检查 — 就绪/存活探针适配
//!
//! 提供 [`ConfigCenterHealthCheck`]，实现异步 [`HealthCheck`] trait，
//! 可注册到 sz-rust-core 的 `HealthRegistry` readiness 探针，用于检测配置中心可达性。

use async_trait::async_trait;
use parking_lot::Mutex;

/// 异步健康检查 trait（统一接口）。
///
/// 实现者返回 `Ok(())` 表示依赖可达，`Err(msg)` 表示不可达 + 错误描述。
#[async_trait]
pub trait HealthCheck: Send + Sync {
    /// 检查器名称（如 "config-center-consul"）。
    fn check_name(&self) -> &str;

    /// 执行检查。`Ok(())` 表示可达，`Err` 表示不可达。
    async fn check(&self) -> Result<(), String>;
}

/// 配置中心健康检查器。
///
/// 通过内部状态模拟配置中心可达性，供 readiness 探针使用。
/// 线程安全：内部状态用 [`parking_lot::Mutex`] 保护。
pub struct ConfigCenterHealthCheck {
    name: String,
    healthy: Mutex<bool>,
}

impl ConfigCenterHealthCheck {
    /// 创建检查器，默认健康（可达）。
    pub fn new(name: String) -> Self {
        Self {
            name,
            healthy: Mutex::new(true),
        }
    }

    /// 设置配置中心可达性。`true` 表示可达。
    pub fn set_healthy(&self, healthy: bool) {
        *self.healthy.lock() = healthy;
    }
}

#[async_trait]
impl HealthCheck for ConfigCenterHealthCheck {
    fn check_name(&self) -> &str {
        &self.name
    }

    async fn check(&self) -> Result<(), String> {
        if *self.healthy.lock() {
            Ok(())
        } else {
            Err(format!("config center '{}' is unreachable", self.name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn healthy_default_returns_ok() {
        let hc = ConfigCenterHealthCheck::new("consul".to_string());
        assert_eq!(hc.check_name(), "consul");
        assert!(hc.check().await.is_ok());
    }

    #[tokio::test]
    async fn unhealthy_returns_err() {
        let hc = ConfigCenterHealthCheck::new("consul".to_string());
        hc.set_healthy(false);
        let err = hc.check().await.unwrap_err();
        assert!(err.contains("consul"));
        assert!(err.contains("unreachable"));
    }

    #[tokio::test]
    async fn toggle_healthy() {
        let hc = ConfigCenterHealthCheck::new("nacos".to_string());
        hc.set_healthy(false);
        assert!(hc.check().await.is_err());
        hc.set_healthy(true);
        assert!(hc.check().await.is_ok());
    }

    #[tokio::test]
    async fn check_name_reflects_input() {
        let hc = ConfigCenterHealthCheck::new("config-center-prod".to_string());
        assert_eq!(hc.check_name(), "config-center-prod");
    }
}
