// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 管理后台 UI 后端 API（RBAC 权限 + 动态路由 + 主题切换 + CRUD）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate 控制，默认不启用。
//!
//! # 组成
//!
//! - [`permission`]：RBAC 权限 API（spec §5.25 规则 1/6）
//! - [`route`]：动态路由 API（spec §5.25 规则 2）
//! - [`theme`]：主题切换 API（spec §5.25 规则 3）
//! - [`crud`]：CRUD 操作 API（spec §5.25 规则 4-5）

#![forbid(unsafe_code)]

pub mod crud;
pub mod error;
pub mod permission;
pub mod route;
pub mod theme;

pub use error::*;
