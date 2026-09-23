// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 服务注册发现错误类型

use thiserror::Error;

/// 服务注册发现错误
#[derive(Debug, Error)]
pub enum RegistryError {
    /// HTTP 请求失败
    #[error("HTTP request failed: {0}")]
    Http(String),

    /// 注册中心不可达（降级本地缓存）
    #[error("Registry unreachable: {0}")]
    Unreachable(String),

    /// 实例不存在
    #[error("Instance not found: {0}")]
    InstanceNotFound(String),

    /// 无可用实例（503）
    #[error("No available instance for service: {0}")]
    NoAvailableInstance(String),

    /// 心跳失败
    #[error("Heartbeat failed for instance {0}: {1}")]
    HeartbeatFailed(String, String),

    /// 注册失败
    #[error("Register failed for instance {0}: {1}")]
    RegisterFailed(String, String),

    /// 注销失败
    #[error("Deregister failed for instance {0}: {1}")]
    DeregisterFailed(String, String),

    /// 序列化/反序列化失败
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// 负载均衡选择失败
    #[error("Load balance select failed: {0}")]
    LoadBalance(String),
}
