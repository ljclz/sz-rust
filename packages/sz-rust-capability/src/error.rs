// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
/// Capability Registry 错误类型。
///
/// 6 个变体覆盖所有业务错误场景，标注 `#[non_exhaustive]` 允许未来新增变体不破坏外部 match。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CapError {
    /// 能力未找到（按名称查找不存在）。
    #[error("能力未找到: {0}")]
    NotFound(String),
    /// 参数校验失败（缺少必填字段或类型不匹配）。
    #[error("参数校验失败: {0}")]
    ValidationError(String),
    /// 能力执行失败（透传能力内部错误）。
    #[error("执行失败: {0}")]
    ExecutionError(String),
    /// 权限不足（上层授权中间件拒绝）。
    #[error("权限不足: {0}")]
    PermissionDenied(String),
    /// 需要人工确认（HITL 闸门）。
    #[error("需要人工确认")]
    ConfirmationRequired,
    /// Registry 未初始化（facade 未调用 `init()`）。
    #[error("Registry 未初始化")]
    NotInitialized,
}

/// Capability Registry Result 类型别名。
pub type CapResult<T> = Result<T, CapError>;
