// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! # sz-rust-plugin-sdk — 插件开发 SDK（spec §5.5）
//!
//! 生命周期钩子 + API 稳定性保证 + 版本兼容声明 + 插件上下文。
//!
//! ## 快速开始
//!
//! ```rust,ignore
//! use sz_rust_plugin_sdk::{Plugin, PluginDescriptor, StabilityLevel, PluginLifecycle};
//!
//! struct MyPlugin;
//!
//! #[async_trait::async_trait]
//! impl PluginLifecycle for MyPlugin {
//!     async fn on_install(&self) -> Result<(), sz_rust_plugin_sdk::SdkError> {
//!         println!("安装中...");
//!         Ok(())
//!     }
//! }
//!
//! impl Plugin for MyPlugin {
//!     fn descriptor(&self) -> &PluginDescriptor {
//!         // 返回插件描述符
//!         unimplemented!()
//!     }
//! }
//! ```
//!
//! ## 生命周期钩子
//!
//! 实现 [`PluginLifecycle`] trait 注册回调：
//! - `on_install`：插件首次安装
//! - `on_enable`：插件启用
//! - `on_disable`：插件禁用
//! - `on_uninstall`：插件卸载（资源清理）
//! - `on_config_change`：配置变更
//!
//! ## 版本兼容
//!
//! 通过 [`PluginDescriptor`] 声明兼容宿主版本范围（semver range），
//! 宿主加载插件时调用 `check_compatibility` 校验。

#![forbid(unsafe_code)]

pub mod compat;
pub mod context;
pub mod error;
pub mod hooks;

pub use compat::*;
pub use context::*;
pub use error::*;
pub use hooks::*;

use async_trait::async_trait;

/// 插件 trait（开发者实现此 trait 定义插件）
#[async_trait]
pub trait Plugin: Send + Sync {
    /// 返回插件描述符
    fn descriptor(&self) -> &PluginDescriptor;

    /// 返回生命周期钩子实现
    fn lifecycle(&self) -> &dyn PluginLifecycle;

    /// 校验宿主版本兼容性
    ///
    /// # 后置条件
    /// - 兼容 → Ok
    /// - 不兼容 → Err(VersionIncompatible)
    fn check_compatibility(&self, host_version: &str) -> Result<(), SdkError> {
        self.descriptor().check_compatibility(host_version)
    }
}
