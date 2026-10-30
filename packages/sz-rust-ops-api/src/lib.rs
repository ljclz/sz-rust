// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! sz-rust-ops-api — 运维专用 API（T054/T055）
//!
//! 提供 admin guard 保护的运维操作端点（日志级别切换 / 配置重载 / 缓存清理 / 路由刷新）
//! 与灰度发布管理（IP / 租户 / 百分比策略）。
//!
//! # 模块结构
//!
//! | 模块 | 说明 |
//! |------|------|
//! | [`admin_guard`] | admin 鉴权中间件（`x-admin-token` 校验） |
//! | [`routes`] | 4 个运维 API 端点 |
//! | [`gray_release`] | 灰度发布规则与管理器 |

#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod admin_guard;
pub mod gray_release;
pub mod routes;

pub use admin_guard::{admin_guard_middleware, AdminErrorResponse, AdminGuard};
pub use gray_release::{
    GrayContext, GrayDecision, GrayReleaseManager, GrayReleaseRule, GrayStrategy,
};
pub use routes::{build_ops_router, LogLevelRequest, OpsResponse};

/// 运维 API 错误类型。
#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    /// 无效的日志级别。
    #[error("invalid log level: {0}")]
    InvalidLogLevel(String),
    /// 鉴权失败。
    #[error("forbidden")]
    Forbidden,
}
