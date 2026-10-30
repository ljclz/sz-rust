// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! API 网关错误类型

use thiserror::Error;

/// API 网关错误
#[derive(Debug, Error)]
pub enum GatewayError {
    /// 路由不匹配
    #[error("No route matched: {0}")]
    NoRoute(String),

    /// 后端不可用（503）
    #[error("Backend unavailable: {0}")]
    BackendUnavailable(String),

    /// 鉴权失败（401）
    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    /// 限流（429）
    #[error("Rate limited: {0}")]
    RateLimited(String),

    /// 熔断（503）
    #[error("Circuit broken: {0}")]
    CircuitBroken(String),

    /// 转发失败
    #[error("Forward failed: {0}")]
    Forward(String),

    /// 协议转换失败（400）
    #[error("Protocol conversion failed: {0}")]
    ProtocolConversion(String),

    /// 配置错误
    #[error("Config error: {0}")]
    Config(String),
}
