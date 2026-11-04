// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::Arc;

use sz_rust_orm_facade::Pool;

use crate::services::{
    config_service::ConfigService, dashboard_service::DashboardService,
    log_service::OperationLogService, menu_service::MenuService,
    permission_service::PermissionService, role_service::RoleService, user_service::UserService,
};

/// Admin 插件共享状态
#[derive(Clone)]
pub struct AdminAddonState {
    pub pool: Arc<Pool>,
    pub user_service: UserService,
    pub role_service: RoleService,
    pub permission_service: PermissionService,
    pub menu_service: MenuService,
    pub config_service: ConfigService,
    pub log_service: OperationLogService,
    pub dashboard_service: DashboardService,
    pub admin_roles: Vec<String>,
}

impl AdminAddonState {
    pub fn new(pool: Arc<Pool>, admin_roles: Vec<String>) -> Self {
        Self {
            user_service: UserService::new(pool.clone()),
            role_service: RoleService::new(pool.clone()),
            permission_service: PermissionService::new(pool.clone()),
            menu_service: MenuService::new(pool.clone()),
            config_service: ConfigService::new(pool.clone()),
            log_service: OperationLogService::new(pool.clone()),
            dashboard_service: DashboardService::new(pool.clone()),
            pool,
            admin_roles,
        }
    }
}
