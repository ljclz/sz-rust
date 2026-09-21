// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! sz-rust-addons-admin — Admin 后台管理插件
//!
//! 提供 7 大管理模块（用户/角色/权限/菜单/配置/日志/仪表盘），
//! 21 个 REST API 端点（`/api/admin/*`），17 个 `admin.*` Capability。
//!
//! # 架构
//!
//! - models：8 个领域对象 + 5 个枚举
//! - services：7 个业务服务
//! - handlers：21 个 HTTP handler
//! - guard：权限校验中间件
//! - capability_hook：17 个 Capability + CapabilityHook 实现
//!
//! # 依赖
//!
//! - sz-rust-orm-facade：ORM 仓储层（Pool、Value、DbError）
//! - sz-rust-middleware-facade：DataScopeUserContext、TenantContext
//! - sz-rust-capability：Capability trait + CapabilityRegistry
//! - sz-rust-addons-loader：CapabilityHook trait
//! - bcrypt：密码哈希
//! - sysinfo：系统状态采集

pub mod capability_hook;
pub mod error;
pub mod guard;
pub mod handlers;
pub mod models;
pub mod router;
pub mod services;
pub mod state;

use std::sync::Arc;

use axum::Router;
use serde::Serialize;
use sz_rust_orm_facade::Pool;

pub use capability_hook::AdminCapabilityHook;
pub use error::{AdminError, AdminErrorResponse};
pub use router::build_admin_addon_router;
pub use state::AdminAddonState;

#[derive(Debug, Clone, Serialize)]
pub struct PageResponse<T: Serialize> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u64,
    pub size: u64,
}

impl<T: Serialize> PageResponse<T> {
    pub fn from_page_result(p: sz_rust_orm_facade::repository::PageResult<T>) -> Self {
        Self {
            items: p.items,
            total: p.total,
            page: p.page,
            size: p.page_size,
        }
    }
}

/// Admin 插件入口
pub struct AdminAddonPlugin {
    pool: Arc<Pool>,
    admin_roles: Vec<String>,
}

impl AdminAddonPlugin {
    pub fn new(pool: Arc<Pool>, admin_roles: Vec<String>) -> Self {
        Self { pool, admin_roles }
    }

    pub fn router(&self) -> Router {
        let state = AdminAddonState::new(self.pool.clone(), self.admin_roles.clone());
        build_admin_addon_router(state)
    }

    pub fn capability_hook(&self) -> AdminCapabilityHook {
        AdminCapabilityHook::new(self.pool.clone())
    }
}
