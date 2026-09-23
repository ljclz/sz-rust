// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::Arc;

use async_trait::async_trait;
use sz_rust_addons_loader::capability_hook::{validate_capability_naming, CapabilityHook};
use sz_rust_capability::{CapError, CapResult, Capability, CapabilityRegistry, CapabilitySource};
use sz_rust_orm_facade::Pool;

use crate::models::config::ConfigType;
use crate::models::operation_log::OperationType;
use crate::models::user::UserStatus;
use crate::services::config_service::{ConfigService, UpsertConfigRequest};
use crate::services::dashboard_service::DashboardService;
use crate::services::log_service::{LogFilter, OperationLogService};
use crate::services::menu_service::{CreateMenuRequest, MenuService, UpdateMenuRequest};
use crate::services::permission_service::PermissionService;
use crate::services::role_service::{CreateRoleRequest, RoleService, UpdateRoleRequest};
use crate::services::user_service::{
    CreateUserRequest, UpdateUserRequest, UserListFilter, UserService,
};
use crate::PageResponse;

fn admin_err_to_cap(e: crate::error::AdminError) -> CapError {
    CapError::ExecutionError(e.to_string())
}

fn serde_err_to_cap(e: serde_json::Error) -> CapError {
    CapError::ExecutionError(e.to_string())
}

macro_rules! cap_struct {
    ($name:ident) => {
        pub struct $name {
            pool: Arc<Pool>,
        }
        impl $name {
            pub fn new(pool: Arc<Pool>) -> Self {
                Self { pool }
            }
        }
    };
}

cap_struct!(UserListCapability);
cap_struct!(UserCreateCapability);
cap_struct!(UserUpdateCapability);
cap_struct!(UserDeleteCapability);
cap_struct!(RoleListCapability);
cap_struct!(RoleCreateCapability);
cap_struct!(RoleUpdateCapability);
cap_struct!(RoleDeleteCapability);
cap_struct!(PermissionTreeCapability);
cap_struct!(MenuTreeCapability);
cap_struct!(MenuCreateCapability);
cap_struct!(MenuUpdateCapability);
cap_struct!(MenuDeleteCapability);
cap_struct!(ConfigListCapability);
cap_struct!(ConfigUpdateCapability);
cap_struct!(LogListCapability);
cap_struct!(DashboardCapability);

#[async_trait]
impl Capability for UserListCapability {
    fn name(&self) -> &'static str {
        "admin.user_list"
    }
    fn description(&self) -> &'static str {
        "用户列表查询"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "username": {"type": "string"}, "status": {"type": "string"}, "page": {"type": "integer"}, "size": {"type": "integer"}}})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "user", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = UserService::new(self.pool.clone());
        let filter = UserListFilter {
            username: args
                .get("username")
                .and_then(|v| v.as_str())
                .map(String::from),
            status: args
                .get("status")
                .and_then(|v| v.as_str())
                .and_then(UserStatus::parse_str),
        };
        let page = args.get("page").and_then(|v| v.as_i64()).unwrap_or(1);
        let size = args.get("size").and_then(|v| v.as_i64()).unwrap_or(20);
        let result = svc
            .list(filter, tenant_id, page, size)
            .await
            .map_err(admin_err_to_cap)?;
        let resp = PageResponse::from_page_result(result);
        serde_json::to_value(resp).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for UserCreateCapability {
    fn name(&self) -> &'static str {
        "admin.user_create"
    }
    fn description(&self) -> &'static str {
        "创建用户"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "username": {"type": "string"}, "password": {"type": "string"}, "email": {"type": "string"}, "phone": {"type": "string"}}, "required": ["username", "password"]})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "user", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = UserService::new(self.pool.clone());
        let username = args
            .get("username")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("缺少 username 参数".into()))?
            .to_string();
        let password = args
            .get("password")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("缺少 password 参数".into()))?
            .to_string();
        let email = args.get("email").and_then(|v| v.as_str()).map(String::from);
        let phone = args.get("phone").and_then(|v| v.as_str()).map(String::from);
        let req = CreateUserRequest {
            username,
            password,
            email,
            phone,
        };
        let result = svc.create(req, tenant_id).await.map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for UserUpdateCapability {
    fn name(&self) -> &'static str {
        "admin.user_update"
    }
    fn description(&self) -> &'static str {
        "更新用户"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "user_id": {"type": "integer"}, "password": {"type": "string"}, "email": {"type": "string"}, "phone": {"type": "string"}}, "required": ["user_id"]})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "user", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = UserService::new(self.pool.clone());
        let user_id = args
            .get("user_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("缺少 user_id 参数".into()))?;
        let password = args
            .get("password")
            .and_then(|v| v.as_str())
            .map(String::from);
        let email = args.get("email").and_then(|v| v.as_str()).map(String::from);
        let phone = args.get("phone").and_then(|v| v.as_str()).map(String::from);
        let req = UpdateUserRequest {
            password,
            email,
            phone,
        };
        let result = svc
            .update(user_id, req, tenant_id)
            .await
            .map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for UserDeleteCapability {
    fn name(&self) -> &'static str {
        "admin.user_delete"
    }
    fn description(&self) -> &'static str {
        "删除用户"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "user_id": {"type": "integer"}}, "required": ["user_id"]})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "user", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = UserService::new(self.pool.clone());
        let user_id = args
            .get("user_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("缺少 user_id 参数".into()))?;
        svc.delete(user_id, tenant_id)
            .await
            .map_err(admin_err_to_cap)?;
        Ok(serde_json::json!({"status": "ok", "user_id": user_id}))
    }
}

#[async_trait]
impl Capability for RoleListCapability {
    fn name(&self) -> &'static str {
        "admin.role_list"
    }
    fn description(&self) -> &'static str {
        "角色列表查询"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}}})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "role", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = RoleService::new(self.pool.clone());
        let result = svc.list(tenant_id).await.map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for RoleCreateCapability {
    fn name(&self) -> &'static str {
        "admin.role_create"
    }
    fn description(&self) -> &'static str {
        "创建角色"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "name": {"type": "string"}, "code": {"type": "string"}, "description": {"type": "string"}}, "required": ["name", "code"]})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "role", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = RoleService::new(self.pool.clone());
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("缺少 name 参数".into()))?
            .to_string();
        let code = args
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("缺少 code 参数".into()))?
            .to_string();
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);
        let req = CreateRoleRequest {
            name,
            code,
            description,
        };
        let result = svc.create(req, tenant_id).await.map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for RoleUpdateCapability {
    fn name(&self) -> &'static str {
        "admin.role_update"
    }
    fn description(&self) -> &'static str {
        "更新角色"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "role_id": {"type": "integer"}, "name": {"type": "string"}, "description": {"type": "string"}}, "required": ["role_id"]})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "role", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = RoleService::new(self.pool.clone());
        let role_id = args
            .get("role_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("缺少 role_id 参数".into()))?;
        let name = args.get("name").and_then(|v| v.as_str()).map(String::from);
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);
        let req = UpdateRoleRequest { name, description };
        let result = svc
            .update(role_id, req, tenant_id)
            .await
            .map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for RoleDeleteCapability {
    fn name(&self) -> &'static str {
        "admin.role_delete"
    }
    fn description(&self) -> &'static str {
        "删除角色"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "role_id": {"type": "integer"}}, "required": ["role_id"]})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "role", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = RoleService::new(self.pool.clone());
        let role_id = args
            .get("role_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("缺少 role_id 参数".into()))?;
        svc.delete(role_id, tenant_id)
            .await
            .map_err(admin_err_to_cap)?;
        Ok(serde_json::json!({"status": "ok", "role_id": role_id}))
    }
}

#[async_trait]
impl Capability for PermissionTreeCapability {
    fn name(&self) -> &'static str {
        "admin.permission_tree"
    }
    fn description(&self) -> &'static str {
        "权限树查询"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}}})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "permission", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = PermissionService::new(self.pool.clone());
        let result = svc.tree(tenant_id).await.map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for MenuTreeCapability {
    fn name(&self) -> &'static str {
        "admin.menu_tree"
    }
    fn description(&self) -> &'static str {
        "菜单树查询"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "permissions": {"type": "array", "items": {"type": "string"}}}})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "menu", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = MenuService::new(self.pool.clone());
        let permissions: Vec<String> = args
            .get("permissions")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let result = svc
            .tree(&permissions, tenant_id)
            .await
            .map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for MenuCreateCapability {
    fn name(&self) -> &'static str {
        "admin.menu_create"
    }
    fn description(&self) -> &'static str {
        "创建菜单"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "name": {"type": "string"}, "code": {"type": "string"}, "path": {"type": "string"}, "icon": {"type": "string"}, "parent_id": {"type": "integer"}, "sort": {"type": "integer"}, "is_visible": {"type": "boolean"}, "permission_code": {"type": "string"}}, "required": ["name", "code", "path"]})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "menu", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = MenuService::new(self.pool.clone());
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("缺少 name 参数".into()))?
            .to_string();
        let code = args
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("缺少 code 参数".into()))?
            .to_string();
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("缺少 path 参数".into()))?
            .to_string();
        let icon = args.get("icon").and_then(|v| v.as_str()).map(String::from);
        let parent_id = args.get("parent_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let sort = args.get("sort").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let is_visible = args
            .get("is_visible")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let permission_code = args
            .get("permission_code")
            .and_then(|v| v.as_str())
            .map(String::from);
        let req = CreateMenuRequest {
            name,
            code,
            path,
            icon,
            parent_id,
            sort,
            is_visible,
            permission_code,
        };
        let result = svc.create(req, tenant_id).await.map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for MenuUpdateCapability {
    fn name(&self) -> &'static str {
        "admin.menu_update"
    }
    fn description(&self) -> &'static str {
        "更新菜单"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "menu_id": {"type": "integer"}, "name": {"type": "string"}, "path": {"type": "string"}, "icon": {"type": "string"}, "parent_id": {"type": "integer"}, "sort": {"type": "integer"}, "is_visible": {"type": "boolean"}, "permission_code": {"type": "string"}}, "required": ["menu_id"]})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "menu", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = MenuService::new(self.pool.clone());
        let menu_id = args
            .get("menu_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("缺少 menu_id 参数".into()))?;
        let name = args.get("name").and_then(|v| v.as_str()).map(String::from);
        let path = args.get("path").and_then(|v| v.as_str()).map(String::from);
        let icon = args.get("icon").and_then(|v| v.as_str()).map(String::from);
        let parent_id = args.get("parent_id").and_then(|v| v.as_i64());
        let sort = args.get("sort").and_then(|v| v.as_i64()).map(|v| v as i32);
        let is_visible = args.get("is_visible").and_then(|v| v.as_bool());
        let permission_code = args
            .get("permission_code")
            .and_then(|v| v.as_str())
            .map(String::from);
        let req = UpdateMenuRequest {
            name,
            path,
            icon,
            parent_id,
            sort,
            is_visible,
            permission_code,
        };
        let result = svc
            .update(menu_id, req, tenant_id)
            .await
            .map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for MenuDeleteCapability {
    fn name(&self) -> &'static str {
        "admin.menu_delete"
    }
    fn description(&self) -> &'static str {
        "删除菜单"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "menu_id": {"type": "integer"}}, "required": ["menu_id"]})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "menu", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = MenuService::new(self.pool.clone());
        let menu_id = args
            .get("menu_id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| CapError::ValidationError("缺少 menu_id 参数".into()))?;
        svc.delete(menu_id, tenant_id)
            .await
            .map_err(admin_err_to_cap)?;
        Ok(serde_json::json!({"status": "ok", "menu_id": menu_id}))
    }
}

#[async_trait]
impl Capability for ConfigListCapability {
    fn name(&self) -> &'static str {
        "admin.config_list"
    }
    fn description(&self) -> &'static str {
        "配置列表查询"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "group": {"type": "string"}}})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "config", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = ConfigService::new(self.pool.clone());
        let group = args.get("group").and_then(|v| v.as_str()).map(String::from);
        let result = svc.list(group, tenant_id).await.map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for ConfigUpdateCapability {
    fn name(&self) -> &'static str {
        "admin.config_update"
    }
    fn description(&self) -> &'static str {
        "更新配置"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "key": {"type": "string"}, "value": {}, "value_type": {"type": "string"}, "group": {"type": "string"}, "description": {"type": "string"}}, "required": ["key", "value", "value_type"]})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "config", "write"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = ConfigService::new(self.pool.clone());
        let key = args
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("缺少 key 参数".into()))?
            .to_string();
        let value = args
            .get("value")
            .cloned()
            .ok_or_else(|| CapError::ValidationError("缺少 value 参数".into()))?;
        let value_type = args
            .get("value_type")
            .and_then(|v| v.as_str())
            .and_then(|s| match s {
                "string" => Some(ConfigType::String),
                "number" => Some(ConfigType::Number),
                "boolean" => Some(ConfigType::Boolean),
                "json" => Some(ConfigType::Json),
                _ => None,
            })
            .ok_or_else(|| CapError::ValidationError("缺少或无效的 value_type 参数".into()))?;
        let group = args.get("group").and_then(|v| v.as_str()).map(String::from);
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);
        let req = UpsertConfigRequest {
            value,
            value_type,
            group,
            description,
        };
        let result = svc
            .upsert(key, req, tenant_id)
            .await
            .map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for LogListCapability {
    fn name(&self) -> &'static str {
        "admin.log_list"
    }
    fn description(&self) -> &'static str {
        "操作日志查询"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}, "operator_id": {"type": "integer"}, "operation_type": {"type": "string"}, "start_time": {"type": "string"}, "end_time": {"type": "string"}, "page": {"type": "integer"}, "size": {"type": "integer"}}})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "log", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = OperationLogService::new(self.pool.clone());
        let operator_id = args.get("operator_id").and_then(|v| v.as_i64());
        let operation_type = args
            .get("operation_type")
            .and_then(|v| v.as_str())
            .and_then(|s| match s {
                "create" => Some(OperationType::Create),
                "update" => Some(OperationType::Update),
                "delete" => Some(OperationType::Delete),
                "login" => Some(OperationType::Login),
                "logout" => Some(OperationType::Logout),
                _ => None,
            });
        let start_time = args
            .get("start_time")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        let end_time = args
            .get("end_time")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        let filter = LogFilter {
            operator_id,
            operation_type,
            start_time,
            end_time,
        };
        let page = args.get("page").and_then(|v| v.as_i64()).unwrap_or(1);
        let size = args.get("size").and_then(|v| v.as_i64()).unwrap_or(20);
        let result = svc
            .list(filter, tenant_id, page, size)
            .await
            .map_err(admin_err_to_cap)?;
        let resp = PageResponse::from_page_result(result);
        serde_json::to_value(resp).map_err(serde_err_to_cap)
    }
}

#[async_trait]
impl Capability for DashboardCapability {
    fn name(&self) -> &'static str {
        "admin.dashboard"
    }
    fn description(&self) -> &'static str {
        "仪表盘统计"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {"tenant_id": {"type": "integer"}}})
    }
    fn tags(&self) -> &[&'static str] {
        &["admin", "dashboard", "read"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }
    async fn call(&self, args: serde_json::Value) -> CapResult<serde_json::Value> {
        let tenant_id = args.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
        let svc = DashboardService::new(self.pool.clone());
        let result = svc.stats(tenant_id).await.map_err(admin_err_to_cap)?;
        serde_json::to_value(result).map_err(serde_err_to_cap)
    }
}

const CAPABILITY_NAMES: &[&str] = &[
    "admin.user_list",
    "admin.user_create",
    "admin.user_update",
    "admin.user_delete",
    "admin.role_list",
    "admin.role_create",
    "admin.role_update",
    "admin.role_delete",
    "admin.permission_tree",
    "admin.menu_tree",
    "admin.menu_create",
    "admin.menu_update",
    "admin.menu_delete",
    "admin.config_list",
    "admin.config_update",
    "admin.log_list",
    "admin.dashboard",
];

pub struct AdminCapabilityHook {
    pool: Arc<Pool>,
}

impl AdminCapabilityHook {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    fn build_capabilities(&self) -> Vec<Arc<dyn Capability>> {
        let pool = &self.pool;
        vec![
            Arc::new(UserListCapability::new(pool.clone())),
            Arc::new(UserCreateCapability::new(pool.clone())),
            Arc::new(UserUpdateCapability::new(pool.clone())),
            Arc::new(UserDeleteCapability::new(pool.clone())),
            Arc::new(RoleListCapability::new(pool.clone())),
            Arc::new(RoleCreateCapability::new(pool.clone())),
            Arc::new(RoleUpdateCapability::new(pool.clone())),
            Arc::new(RoleDeleteCapability::new(pool.clone())),
            Arc::new(PermissionTreeCapability::new(pool.clone())),
            Arc::new(MenuTreeCapability::new(pool.clone())),
            Arc::new(MenuCreateCapability::new(pool.clone())),
            Arc::new(MenuUpdateCapability::new(pool.clone())),
            Arc::new(MenuDeleteCapability::new(pool.clone())),
            Arc::new(ConfigListCapability::new(pool.clone())),
            Arc::new(ConfigUpdateCapability::new(pool.clone())),
            Arc::new(LogListCapability::new(pool.clone())),
            Arc::new(DashboardCapability::new(pool.clone())),
        ]
    }
}

impl CapabilityHook for AdminCapabilityHook {
    fn register_capabilities(&self, registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
        let caps = self.build_capabilities();
        let mut names = Vec::with_capacity(caps.len());
        for cap in caps {
            let name = cap.name().to_string();
            if !validate_capability_naming("admin", &name) {
                tracing::warn!(capability = %name, "命名不规范但仍注册");
            }
            registry.register(cap);
            names.push(name);
        }
        Ok(names)
    }

    fn capability_names(&self) -> Vec<String> {
        CAPABILITY_NAMES.iter().map(|s| s.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use sz_rust_orm_facade::{Connection, ConnectionFactory, DbError, PoolConfig, Value};

    type QueryRows = Vec<HashMap<String, Value>>;

    fn mock_row(sql: &str) -> QueryRows {
        let sql = sql.to_ascii_lowercase();
        if sql.contains("select id from") {
            if sql.contains("where id =") {
                let mut row = HashMap::new();
                row.insert("id".into(), Value::I64(1));
                return vec![row];
            }
            return vec![];
        }
        if sql.contains("count(*)") {
            let mut row = HashMap::new();
            row.insert("cnt".into(), Value::I64(1));
            return vec![row];
        }
        if sql.contains("from users") {
            let mut row = HashMap::new();
            row.insert("id".into(), Value::I64(1));
            row.insert("username".into(), Value::String("alice".into()));
            row.insert("email".into(), Value::String("alice@example.com".into()));
            row.insert("phone".into(), Value::Null);
            row.insert("status".into(), Value::String("active".into()));
            row.insert("tenant_id".into(), Value::I64(0));
            row.insert(
                "created_at".into(),
                Value::String("2026-01-01T00:00:00+00:00".into()),
            );
            row.insert(
                "updated_at".into(),
                Value::String("2026-01-01T00:00:00+00:00".into()),
            );
            row.insert("last_login_at".into(), Value::Null);
            return vec![row];
        }
        if sql.contains("from roles") {
            let mut row = HashMap::new();
            row.insert("id".into(), Value::I64(1));
            row.insert("name".into(), Value::String("Admin".into()));
            row.insert("code".into(), Value::String("admin".into()));
            row.insert("description".into(), Value::Null);
            row.insert("is_builtin".into(), Value::I64(0));
            row.insert("tenant_id".into(), Value::I64(0));
            row.insert(
                "created_at".into(),
                Value::String("2026-01-01T00:00:00+00:00".into()),
            );
            row.insert(
                "updated_at".into(),
                Value::String("2026-01-01T00:00:00+00:00".into()),
            );
            return vec![row];
        }
        if sql.contains("from menus") {
            if sql.contains("where parent_id") {
                return vec![];
            }
            let mut row = HashMap::new();
            row.insert("id".into(), Value::I64(1));
            row.insert("name".into(), Value::String("Dashboard".into()));
            row.insert("code".into(), Value::String("dashboard".into()));
            row.insert("path".into(), Value::String("/dashboard".into()));
            row.insert("icon".into(), Value::Null);
            row.insert("parent_id".into(), Value::I64(0));
            row.insert("sort".into(), Value::I32(1));
            row.insert("is_visible".into(), Value::I64(1));
            row.insert("permission_code".into(), Value::Null);
            row.insert("tenant_id".into(), Value::I64(0));
            row.insert(
                "created_at".into(),
                Value::String("2026-01-01T00:00:00+00:00".into()),
            );
            row.insert(
                "updated_at".into(),
                Value::String("2026-01-01T00:00:00+00:00".into()),
            );
            return vec![row];
        }
        if sql.contains("from configs") {
            let mut row = HashMap::new();
            row.insert("id".into(), Value::I64(1));
            row.insert("key".into(), Value::String("site_name".into()));
            row.insert("value".into(), Value::String("My Site".into()));
            row.insert("value_type".into(), Value::String("string".into()));
            row.insert("group".into(), Value::Null);
            row.insert("description".into(), Value::Null);
            row.insert("tenant_id".into(), Value::I64(0));
            row.insert(
                "created_at".into(),
                Value::String("2026-01-01T00:00:00+00:00".into()),
            );
            row.insert(
                "updated_at".into(),
                Value::String("2026-01-01T00:00:00+00:00".into()),
            );
            return vec![row];
        }
        if sql.contains("from permissions") {
            let mut row = HashMap::new();
            row.insert("id".into(), Value::I64(1));
            row.insert("code".into(), Value::String("admin.user_list".into()));
            row.insert("name".into(), Value::String("用户列表".into()));
            row.insert("module".into(), Value::String("admin".into()));
            row.insert("resource".into(), Value::String("user".into()));
            row.insert("action".into(), Value::String("list".into()));
            row.insert("tenant_id".into(), Value::I64(0));
            row.insert(
                "created_at".into(),
                Value::String("2026-01-01T00:00:00+00:00".into()),
            );
            return vec![row];
        }
        if sql.contains("from operation_logs") {
            let mut row = HashMap::new();
            row.insert("id".into(), Value::I64(1));
            row.insert("operator_id".into(), Value::I64(1));
            row.insert("operator_name".into(), Value::String("admin".into()));
            row.insert("operation_type".into(), Value::String("create".into()));
            row.insert("target_type".into(), Value::String("user".into()));
            row.insert("target_id".into(), Value::I64(1));
            row.insert("detail".into(), Value::Null);
            row.insert("ip".into(), Value::Null);
            row.insert("tenant_id".into(), Value::I64(0));
            row.insert(
                "created_at".into(),
                Value::String("2026-01-01T00:00:00+00:00".into()),
            );
            return vec![row];
        }
        vec![]
    }

    struct MockConnection;

    impl Connection for MockConnection {
        fn execute<'a>(
            &'a mut self,
            _sql: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
            Box::pin(async { Ok(0) })
        }
        fn query<'a>(
            &'a mut self,
            sql: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
            let rows = mock_row(sql);
            Box::pin(async move { Ok(rows) })
        }
        fn execute_with_params<'a>(
            &'a mut self,
            _sql: &'a str,
            _params: &'a [Value],
        ) -> Pin<Box<dyn Future<Output = Result<u64, DbError>> + Send + 'a>> {
            Box::pin(async { Ok(1) })
        }
        fn query_with_params<'a>(
            &'a mut self,
            sql: &'a str,
            _params: &'a [Value],
        ) -> Pin<Box<dyn Future<Output = Result<QueryRows, DbError>> + Send + 'a>> {
            let rows = mock_row(sql);
            Box::pin(async move { Ok(rows) })
        }
        fn begin_transaction<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
        fn commit<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
        fn rollback<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
        fn is_connected(&self) -> bool {
            true
        }
        fn ping<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
            Box::pin(async { true })
        }
        fn close<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<(), DbError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    struct MockConnectionFactory;

    #[async_trait]
    impl ConnectionFactory for MockConnectionFactory {
        async fn create(&self) -> Result<Box<dyn Connection>, DbError> {
            Ok(Box::new(MockConnection))
        }
    }

    fn make_mock_pool() -> Arc<Pool> {
        let config = PoolConfig::default();
        let factory: Arc<dyn ConnectionFactory> = Arc::new(MockConnectionFactory);
        Arc::new(Pool::new(config, factory).expect("mock pool creation should not fail"))
    }

    #[test]
    fn test_capability_names_count_17() {
        assert_eq!(CAPABILITY_NAMES.len(), 17);
    }

    #[test]
    fn test_capability_names_all_prefixed_admin() {
        for name in CAPABILITY_NAMES {
            assert!(
                name.starts_with("admin."),
                "能力名称 {name} 应以 admin. 开头"
            );
        }
    }

    #[test]
    fn test_capability_hook_names_match() {
        let pool = make_mock_pool();
        let hook = AdminCapabilityHook::new(pool);
        let names = hook.capability_names();
        assert_eq!(names.len(), 17);
        assert!(names.contains(&"admin.user_list".to_string()));
        assert!(names.contains(&"admin.dashboard".to_string()));
    }

    #[test]
    fn test_capability_tags_format() {
        let pool = make_mock_pool();
        let user_cap = UserListCapability::new(pool.clone());
        let role_cap = RoleListCapability::new(pool.clone());
        let dash_cap = DashboardCapability::new(pool);
        let tags_sets: Vec<&[&'static str]> =
            vec![user_cap.tags(), role_cap.tags(), dash_cap.tags()];
        for tags in &tags_sets {
            assert_eq!(tags.len(), 3, "每个能力 tags 长度应为 3");
            assert!(tags.contains(&"admin"), "tags 应包含 admin");
        }
    }

    #[test]
    fn test_unregister_plugin_capabilities_removes_all() {
        let registry = CapabilityRegistry::new();
        let pool = make_mock_pool();
        let hook = AdminCapabilityHook::new(pool);
        let registered = hook.register_capabilities(&registry).unwrap();
        assert_eq!(registered.len(), 17);
        assert_eq!(registry.len(), 17);
        let removed = sz_rust_addons_loader::capability_hook::unregister_plugin_capabilities(
            &registry, "admin",
        );
        assert_eq!(removed.len(), 17);
        assert_eq!(registry.len(), 0);
    }

    #[tokio::test]
    async fn test_user_list_capability_calls_service() {
        let pool = make_mock_pool();
        let cap = UserListCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 0, "page": 1, "size": 10});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "UserListCapability call 应成功");
        let value = result.unwrap();
        assert!(value.is_object(), "返回应为 JSON 对象");
        assert!(value.get("items").is_some(), "应包含 items 字段");
        assert!(value.get("total").is_some(), "应包含 total 字段");
    }

    #[tokio::test]
    async fn test_dashboard_capability_calls_service() {
        let pool = make_mock_pool();
        let cap = DashboardCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 0});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "DashboardCapability call 应成功");
        let value = result.unwrap();
        assert!(value.get("user_count").is_some(), "应包含 user_count 字段");
        assert!(value.get("role_count").is_some(), "应包含 role_count 字段");
    }

    #[tokio::test]
    async fn test_role_list_capability_calls_service() {
        let pool = make_mock_pool();
        let cap = RoleListCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 0});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "RoleListCapability call 应成功");
        let value = result.unwrap();
        assert!(value.is_array(), "角色列表应返回数组");
    }

    #[tokio::test]
    async fn test_permission_tree_capability_calls_service() {
        let pool = make_mock_pool();
        let cap = PermissionTreeCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 0});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "PermissionTreeCapability call 应成功");
        let value = result.unwrap();
        assert!(value.get("modules").is_some(), "应包含 modules 字段");
    }

    #[tokio::test]
    async fn test_user_create_capability_validation_error() {
        let pool = make_mock_pool();
        let cap = UserCreateCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 0});
        let result = cap.call(args).await;
        assert!(result.is_err(), "缺少 username 应返回错误");
        let err = result.unwrap_err();
        assert!(
            matches!(err, CapError::ValidationError(_)),
            "应为 ValidationError"
        );
    }

    // —— 12 个 Capability call 方法覆盖 ——

    #[tokio::test]
    async fn test_user_create_capability_call_with_params() {
        let pool = make_mock_pool();
        let cap = UserCreateCapability::new(pool);
        let args =
            serde_json::json!({"tenant_id": 0, "username": "alice", "password": "password123"});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "user create should succeed: {result:?}");
        let value = result.unwrap();
        assert!(value.get("username").is_some(), "应返回 username 字段");
    }

    #[tokio::test]
    async fn test_user_create_capability_missing_password() {
        let pool = make_mock_pool();
        let cap = UserCreateCapability::new(pool);
        let args = serde_json::json!({"username": "alice"});
        let result = cap.call(args).await;
        assert!(matches!(result.unwrap_err(), CapError::ValidationError(_)));
    }

    #[tokio::test]
    async fn test_user_update_capability_call() {
        let pool = make_mock_pool();
        let cap = UserUpdateCapability::new(pool);
        let args = serde_json::json!({"user_id": 1, "email": "new@example.com"});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "user update should succeed: {result:?}");
        let value = result.unwrap();
        assert!(value.get("username").is_some(), "应返回 username 字段");
    }

    #[tokio::test]
    async fn test_user_update_capability_missing_user_id() {
        let pool = make_mock_pool();
        let cap = UserUpdateCapability::new(pool);
        let args = serde_json::json!({"email": "x@example.com"});
        assert!(matches!(
            cap.call(args).await.unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_user_delete_capability_call_success() {
        let pool = make_mock_pool();
        let cap = UserDeleteCapability::new(pool);
        let args = serde_json::json!({"user_id": 1});
        let result = cap.call(args).await.expect("delete should succeed");
        assert_eq!(result["status"], "ok");
        assert_eq!(result["user_id"], 1);
    }

    #[tokio::test]
    async fn test_user_delete_capability_missing_user_id() {
        let pool = make_mock_pool();
        let cap = UserDeleteCapability::new(pool);
        assert!(matches!(
            cap.call(serde_json::json!({})).await.unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_role_create_capability_call_with_params() {
        let pool = make_mock_pool();
        let cap = RoleCreateCapability::new(pool);
        let args = serde_json::json!({"name": "编辑者", "code": "editor"});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "role create should succeed: {result:?}");
    }

    #[tokio::test]
    async fn test_role_create_capability_missing_code() {
        let pool = make_mock_pool();
        let cap = RoleCreateCapability::new(pool);
        let args = serde_json::json!({"name": "编辑者"});
        assert!(matches!(
            cap.call(args).await.unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_role_update_capability_call() {
        let pool = make_mock_pool();
        let cap = RoleUpdateCapability::new(pool);
        let args = serde_json::json!({"role_id": 1, "name": "改名"});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "role update should succeed: {result:?}");
        let value = result.unwrap();
        assert!(value.get("name").is_some(), "应返回 name 字段");
    }

    #[tokio::test]
    async fn test_role_update_capability_missing_role_id() {
        let pool = make_mock_pool();
        let cap = RoleUpdateCapability::new(pool);
        assert!(matches!(
            cap.call(serde_json::json!({"name": "x"}))
                .await
                .unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_role_delete_capability_call() {
        let pool = make_mock_pool();
        let cap = RoleDeleteCapability::new(pool);
        let args = serde_json::json!({"role_id": 1});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "role delete should succeed: {result:?}");
        let value = result.unwrap();
        assert_eq!(value["status"], "ok");
    }

    #[tokio::test]
    async fn test_role_delete_capability_missing_role_id() {
        let pool = make_mock_pool();
        let cap = RoleDeleteCapability::new(pool);
        assert!(matches!(
            cap.call(serde_json::json!({})).await.unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_menu_tree_capability_call() {
        let pool = make_mock_pool();
        let cap = MenuTreeCapability::new(pool);
        let args = serde_json::json!({"permissions": ["admin:user:list"]});
        let result = cap.call(args).await.expect("menu tree should succeed");
        assert!(result.is_array(), "空菜单树应返回数组");
    }

    #[tokio::test]
    async fn test_menu_create_capability_call_with_params() {
        let pool = make_mock_pool();
        let cap = MenuCreateCapability::new(pool);
        let args =
            serde_json::json!({"name": "用户管理", "code": "user_mgr", "path": "/admin/users"});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "menu create should succeed: {result:?}");
    }

    #[tokio::test]
    async fn test_menu_create_capability_missing_path() {
        let pool = make_mock_pool();
        let cap = MenuCreateCapability::new(pool);
        let args = serde_json::json!({"name": "x", "code": "x"});
        assert!(matches!(
            cap.call(args).await.unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_menu_update_capability_call() {
        let pool = make_mock_pool();
        let cap = MenuUpdateCapability::new(pool);
        let args = serde_json::json!({"menu_id": 1, "name": "改名"});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "menu update should succeed: {result:?}");
        let value = result.unwrap();
        assert!(value.get("name").is_some(), "应返回 name 字段");
    }

    #[tokio::test]
    async fn test_menu_update_capability_missing_menu_id() {
        let pool = make_mock_pool();
        let cap = MenuUpdateCapability::new(pool);
        assert!(matches!(
            cap.call(serde_json::json!({"name": "x"}))
                .await
                .unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_menu_delete_capability_call_success() {
        let pool = make_mock_pool();
        let cap = MenuDeleteCapability::new(pool);
        let args = serde_json::json!({"menu_id": 1});
        let result = cap.call(args).await.expect("menu delete should succeed");
        assert_eq!(result["status"], "ok");
    }

    #[tokio::test]
    async fn test_menu_delete_capability_missing_menu_id() {
        let pool = make_mock_pool();
        let cap = MenuDeleteCapability::new(pool);
        assert!(matches!(
            cap.call(serde_json::json!({})).await.unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_config_list_capability_call() {
        let pool = make_mock_pool();
        let cap = ConfigListCapability::new(pool);
        let args = serde_json::json!({"group": "system"});
        let result = cap.call(args).await.expect("config list should succeed");
        assert!(result.is_array(), "空配置列表应返回数组");
    }

    #[tokio::test]
    async fn test_config_update_capability_call_with_params() {
        let pool = make_mock_pool();
        let cap = ConfigUpdateCapability::new(pool);
        let args =
            serde_json::json!({"key": "site_name", "value": "MySite", "value_type": "string"});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "config update should succeed: {result:?}");
        let value = result.unwrap();
        assert!(value.get("key").is_some(), "应返回 key 字段");
    }

    #[tokio::test]
    async fn test_config_update_capability_missing_key() {
        let pool = make_mock_pool();
        let cap = ConfigUpdateCapability::new(pool);
        let args = serde_json::json!({"value": "x", "value_type": "string"});
        assert!(matches!(
            cap.call(args).await.unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_config_update_capability_invalid_value_type() {
        let pool = make_mock_pool();
        let cap = ConfigUpdateCapability::new(pool);
        let args = serde_json::json!({"key": "k", "value": "x", "value_type": "unknown"});
        assert!(matches!(
            cap.call(args).await.unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_config_update_capability_missing_value() {
        let pool = make_mock_pool();
        let cap = ConfigUpdateCapability::new(pool);
        let args = serde_json::json!({"key": "k", "value_type": "string"});
        assert!(matches!(
            cap.call(args).await.unwrap_err(),
            CapError::ValidationError(_)
        ));
    }

    #[tokio::test]
    async fn test_log_list_capability_call() {
        let pool = make_mock_pool();
        let cap = LogListCapability::new(pool);
        let args = serde_json::json!({"operator_id": 1, "operation_type": "create", "page": 1, "size": 10});
        let result = cap.call(args).await.expect("log list should succeed");
        assert!(result.get("items").is_some(), "应返回 items");
    }

    #[tokio::test]
    async fn test_log_list_capability_with_time_range() {
        let pool = make_mock_pool();
        let cap = LogListCapability::new(pool);
        let args = serde_json::json!({
            "start_time": "2026-01-01T00:00:00+00:00",
            "end_time": "2026-12-31T00:00:00+00:00"
        });
        let result = cap
            .call(args)
            .await
            .expect("log list with time should succeed");
        assert!(result.get("total").is_some());
    }

    #[tokio::test]
    async fn test_all_capability_names_and_tags_consistent() {
        let pool = make_mock_pool();
        let caps: Vec<Arc<dyn Capability>> = vec![
            Arc::new(UserListCapability::new(pool.clone())),
            Arc::new(UserCreateCapability::new(pool.clone())),
            Arc::new(UserUpdateCapability::new(pool.clone())),
            Arc::new(UserDeleteCapability::new(pool.clone())),
            Arc::new(RoleListCapability::new(pool.clone())),
            Arc::new(RoleCreateCapability::new(pool.clone())),
            Arc::new(RoleUpdateCapability::new(pool.clone())),
            Arc::new(RoleDeleteCapability::new(pool.clone())),
            Arc::new(PermissionTreeCapability::new(pool.clone())),
            Arc::new(MenuTreeCapability::new(pool.clone())),
            Arc::new(MenuCreateCapability::new(pool.clone())),
            Arc::new(MenuUpdateCapability::new(pool.clone())),
            Arc::new(MenuDeleteCapability::new(pool.clone())),
            Arc::new(ConfigListCapability::new(pool.clone())),
            Arc::new(ConfigUpdateCapability::new(pool.clone())),
            Arc::new(LogListCapability::new(pool.clone())),
            Arc::new(DashboardCapability::new(pool)),
        ];
        assert_eq!(caps.len(), 17);
        for cap in &caps {
            assert!(cap.name().starts_with("admin."));
            assert!(!cap.description().is_empty());
            assert_eq!(cap.tags().len(), 3);
            assert_eq!(cap.source(), CapabilitySource::Plugin);
            let schema = cap.schema();
            assert!(schema.is_object(), "schema 应为对象");
            assert!(schema.get("type").is_some(), "schema 应有 type 字段");
        }
    }

    #[tokio::test]
    async fn test_user_list_capability_with_filters() {
        let pool = make_mock_pool();
        let cap = UserListCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "username": "alice", "status": "active", "page": 2, "size": 5});
        let result = cap.call(args).await.expect("filtered list should succeed");
        assert!(result.get("items").is_some());
    }

    #[tokio::test]
    async fn test_user_create_capability_with_email_phone() {
        let pool = make_mock_pool();
        let cap = UserCreateCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "username": "bob", "password": "pass12345", "email": "bob@test.com", "phone": "12345"});
        let result = cap.call(args).await;
        assert!(
            result.is_ok(),
            "user create with email/phone should succeed: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_user_update_capability_with_all_fields() {
        let pool = make_mock_pool();
        let cap = UserUpdateCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "user_id": 1, "password": "newpass12", "email": "new@test.com", "phone": "99999"});
        let result = cap.call(args).await;
        assert!(
            result.is_ok(),
            "user update with all fields should succeed: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_user_delete_capability_with_tenant() {
        let pool = make_mock_pool();
        let cap = UserDeleteCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "user_id": 2});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "user delete should succeed: {result:?}");
    }

    #[tokio::test]
    async fn test_role_list_capability_with_tenant() {
        let pool = make_mock_pool();
        let cap = RoleListCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1});
        let result = cap.call(args).await.expect("role list should succeed");
        assert!(result.is_array(), "role list should return array");
    }

    #[tokio::test]
    async fn test_role_create_capability_with_description() {
        let pool = make_mock_pool();
        let cap = RoleCreateCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "name": "Editor", "code": "editor", "description": "内容编辑者"});
        let result = cap.call(args).await;
        assert!(
            result.is_ok(),
            "role create with desc should succeed: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_role_update_capability_with_all_fields() {
        let pool = make_mock_pool();
        let cap = RoleUpdateCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "role_id": 1, "name": "Admin2", "description": "Updated"});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "role update should succeed: {result:?}");
    }

    #[tokio::test]
    async fn test_role_delete_capability_with_tenant() {
        let pool = make_mock_pool();
        let cap = RoleDeleteCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "role_id": 2});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "role delete should succeed: {result:?}");
    }

    #[tokio::test]
    async fn test_permission_tree_capability_with_tenant() {
        let pool = make_mock_pool();
        let cap = PermissionTreeCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1});
        let result = cap
            .call(args)
            .await
            .expect("permission tree should succeed");
        assert!(result.is_object(), "permission tree should return object");
        assert!(
            result.get("modules").is_some(),
            "permission tree should have modules field"
        );
    }

    #[tokio::test]
    async fn test_menu_tree_capability_with_permissions() {
        let pool = make_mock_pool();
        let cap = MenuTreeCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "permissions": ["admin.user_list", "admin.role_list"]});
        let result = cap.call(args).await.expect("menu tree should succeed");
        assert!(result.is_array(), "menu tree should return array");
    }

    #[tokio::test]
    async fn test_menu_create_capability_with_all_fields() {
        let pool = make_mock_pool();
        let cap = MenuCreateCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "name": "新菜单", "code": "new_menu", "path": "/new", "icon": "icon-new", "parent_id": 0, "sort": 5, "is_visible": true, "permission_code": "admin.new"});
        let result = cap.call(args).await;
        assert!(
            result.is_ok(),
            "menu create with all fields should succeed: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_menu_update_capability_with_all_fields() {
        let pool = make_mock_pool();
        let cap = MenuUpdateCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "menu_id": 1, "name": "改名", "path": "/new-path", "icon": "new-icon", "parent_id": 0, "sort": 3, "is_visible": false, "permission_code": "admin.updated"});
        let result = cap.call(args).await;
        assert!(
            result.is_ok(),
            "menu update with all fields should succeed: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_menu_delete_capability_with_tenant() {
        let pool = make_mock_pool();
        let cap = MenuDeleteCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "menu_id": 2});
        let result = cap.call(args).await;
        assert!(result.is_ok(), "menu delete should succeed: {result:?}");
    }

    #[tokio::test]
    async fn test_config_list_capability_with_tenant() {
        let pool = make_mock_pool();
        let cap = ConfigListCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "group": "system"});
        let result = cap.call(args).await.expect("config list should succeed");
        assert!(result.is_array(), "config list should return array");
    }

    #[tokio::test]
    async fn test_config_update_capability_with_tenant() {
        let pool = make_mock_pool();
        let cap = ConfigUpdateCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "key": "site_name", "value": "MySite", "value_type": "string"});
        let result = cap.call(args).await;
        assert!(
            result.is_ok(),
            "config update with tenant should succeed: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_log_list_capability_with_tenant() {
        let pool = make_mock_pool();
        let cap = LogListCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1, "operator_id": 1, "operation_type": "create", "target_type": "user", "page": 1, "size": 10});
        let result = cap.call(args).await.expect("log list should succeed");
        assert!(result.get("items").is_some());
    }

    #[tokio::test]
    async fn test_dashboard_capability_with_tenant() {
        let pool = make_mock_pool();
        let cap = DashboardCapability::new(pool);
        let args = serde_json::json!({"tenant_id": 1});
        let result = cap.call(args).await.expect("dashboard should succeed");
        assert!(result.get("user_count").is_some() || result.is_object());
    }
}
