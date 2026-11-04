// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! sz-rust-plugin-sdk 错误类型
//!
//! 错误码段: 17040-17049

use thiserror::Error;

use crate::hooks::LifecycleHook;

/// 插件 SDK 错误
#[derive(Debug, Error)]
pub enum SdkError {
    /// 版本不兼容（spec §5.5 规则 3，code: 17040）
    #[error("版本不兼容：插件要求 {0}，当前宿主 {1} (code: 17040)")]
    VersionIncompatible(String, String),
    /// 钩子回调失败（code: 17041）
    #[error("钩子 `{0:?}` 回调失败: {1} (code: 17041)")]
    HookFailed(LifecycleHook, String),
    /// 直接访问宿主内部状态（spec §5.5 禁止项，code: 17042）
    #[error("禁止直接访问宿主内部状态 (code: 17042)")]
    InternalStateAccess,
    /// 配置错误（code: 17043）
    #[error("配置错误: {0} (code: 17043)")]
    Config(String),
    /// 参数无效（code: 17044）
    #[error("参数无效: {0} (code: 17044)")]
    InvalidParam(String),
    /// 内部错误（code: 17045）
    #[error("内部错误: {0} (code: 17045)")]
    Internal(String),
}
