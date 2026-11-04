// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据权限管理后台 API — 管理员鉴权 + 13 个 REST 端点
//!
//! 模块组成：
//! - [`error_response`]：统一错误响应 `ApiErrorResponse`，按错误码映射 HTTP 状态码
//! - [`guard`]：管理员鉴权中间件 `admin_guard_middleware`
//! - [`router`]：`AdminApiState` 与 `build_admin_router`，挂载全部 13 个端点
//! - [`handlers`]：13 个 handler 实现（规则/策略/配置三大类）

pub mod error_response;
pub mod guard;
pub mod handlers;
pub mod router;

pub use error_response::ApiErrorResponse;
pub use router::{build_admin_router, AdminApiState};
