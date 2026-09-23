// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 配置中心错误类型

use thiserror::Error;

/// 配置中心错误
#[derive(Debug, Error)]
pub enum ConfigCenterError {
    /// HTTP 请求失败
    #[error("HTTP request failed: {0}")]
    Http(String),

    /// 配置解析失败
    #[error("Config parse failed: {0}")]
    Parse(String),

    /// 配置不存在
    #[error("Config not found: {0}")]
    NotFound(String),

    /// 版本不存在
    #[error("Version not found: {0}")]
    VersionNotFound(u64),

    /// 灰度匹配失败
    #[error("Gray match failed: {0}")]
    GrayMatch(String),

    /// 加解密失败
    #[error("Crypto error: {0}")]
    Crypto(String),

    /// 健康检查失败
    #[error("Health check failed: {0}")]
    HealthCheck(String),

    /// 连接失败
    #[error("Connection failed: {0}")]
    Connection(String),
}
