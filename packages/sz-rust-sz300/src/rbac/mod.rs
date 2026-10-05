// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.8.0 RBAC 细粒度权限模块
//!
//! - [`roles`]：sz300 角色定义与权限矩阵
//! - [`guard`]：axum 中间件适配，从 JWT 提取用户 → 查 RbacEngine → allow/deny

pub mod guard;
pub mod roles;
