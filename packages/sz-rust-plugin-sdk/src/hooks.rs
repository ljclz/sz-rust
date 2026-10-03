// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 生命周期钩子（spec §5.5 规则 1）
//!
//! 插件开发者实现 `PluginLifecycle` trait，注册生命周期回调。
//! 宿主在对应事件触发时调用回调。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::SdkError;

/// 生命周期钩子类型（spec §6.5 规则 2）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleHook {
    /// 安装时
    OnInstall,
    /// 启用时
    OnEnable,
    /// 禁用时
    OnDisable,
    /// 卸载时
    OnUninstall,
    /// 配置变更时
    OnConfigChange,
}

/// 插件生命周期钩子 trait
///
/// 插件开发者实现此 trait，注册生命周期回调。
/// 宿主在对应事件触发时调用回调。
#[async_trait]
pub trait PluginLifecycle: Send + Sync {
    /// 安装钩子（插件首次安装时调用）
    async fn on_install(&self) -> Result<(), SdkError> {
        Ok(())
    }
    /// 启用钩子
    async fn on_enable(&self) -> Result<(), SdkError> {
        Ok(())
    }
    /// 禁用钩子
    async fn on_disable(&self) -> Result<(), SdkError> {
        Ok(())
    }
    /// 卸载钩子（资源清理）
    async fn on_uninstall(&self) -> Result<(), SdkError> {
        Ok(())
    }
    /// 配置变更钩子
    async fn on_config_change(&self, _new_config: &serde_json::Value) -> Result<(), SdkError> {
        Ok(())
    }
}

/// 默认生命周期实现（所有钩子空操作）
pub struct DefaultLifecycle;

#[async_trait]
impl PluginLifecycle for DefaultLifecycle {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_default_lifecycle_all_hooks_pass() {
        let lifecycle = DefaultLifecycle;
        assert!(lifecycle.on_install().await.is_ok());
        assert!(lifecycle.on_enable().await.is_ok());
        assert!(lifecycle.on_disable().await.is_ok());
        assert!(lifecycle.on_uninstall().await.is_ok());
        assert!(lifecycle
            .on_config_change(&serde_json::json!({}))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_custom_lifecycle() {
        struct CustomLifecycle;
        #[async_trait]
        impl PluginLifecycle for CustomLifecycle {
            async fn on_install(&self) -> Result<(), SdkError> {
                Err(SdkError::Internal("自定义安装失败".to_string()))
            }
        }

        let lifecycle = CustomLifecycle;
        let result = lifecycle.on_install().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_lifecycle_hook_serialization() {
        let hook = LifecycleHook::OnInstall;
        let json = serde_json::to_string(&hook).unwrap();
        assert_eq!(json, "\"OnInstall\"");

        let deserialized: LifecycleHook = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, LifecycleHook::OnInstall);
    }
}
