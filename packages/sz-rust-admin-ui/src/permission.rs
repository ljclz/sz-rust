// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! RBAC 权限 API（spec §5.25 规则 1/6）
//!
//! 无权限菜单/按钮不渲染；权限从后端动态获取（禁止前端硬编码）。

use std::collections::HashSet;

use crate::error::AdminUiError;

/// 用户权限集合
#[derive(Debug, Clone)]
pub struct UserPermissions {
    /// 用户 ID
    pub user_id: String,
    /// 权限列表
    pub permissions: HashSet<String>,
    /// 角色列表
    pub roles: HashSet<String>,
}

impl UserPermissions {
    /// 创建权限集合
    pub fn new(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            permissions: HashSet::new(),
            roles: HashSet::new(),
        }
    }

    /// 添加权限
    pub fn with_permission(mut self, perm: impl Into<String>) -> Self {
        self.permissions.insert(perm.into());
        self
    }

    /// 添加角色
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.roles.insert(role.into());
        self
    }

    /// 检查是否有权限
    pub fn has_permission(&self, perm: &str) -> bool {
        self.permissions.contains(perm)
    }

    /// 检查是否有任一权限
    pub fn has_any_permission(&self, perms: &[&str]) -> bool {
        perms.iter().any(|p| self.permissions.contains(*p))
    }

    /// 检查是否有全部权限
    pub fn has_all_permissions(&self, perms: &[&str]) -> bool {
        perms.iter().all(|p| self.permissions.contains(*p))
    }
}

/// 权限服务（spec §5.25 规则 1）
pub struct PermissionService;

impl PermissionService {
    /// 获取用户权限（前端动态获取，禁止硬编码，spec §5.25 禁止项）
    pub fn get_permissions(
        user_id: &str,
        rbac_perms: &HashSet<String>,
    ) -> Result<UserPermissions, AdminUiError> {
        if user_id.is_empty() {
            return Err(AdminUiError::InvalidParam("用户 ID 不能为空".into()));
        }
        Ok(UserPermissions {
            user_id: user_id.to_string(),
            permissions: rbac_perms.clone(),
            roles: HashSet::new(),
        })
    }

    /// 过滤菜单项（无权限不渲染，spec §5.25 规则 1）
    pub fn filter_items<T>(
        permissions: &UserPermissions,
        items: &[T],
        required_perm_fn: impl Fn(&T) -> &[String],
    ) -> Vec<T>
    where
        T: Clone,
    {
        items
            .iter()
            .filter(|item| {
                let required = required_perm_fn(item);
                required.is_empty() || required.iter().any(|p| permissions.has_permission(p))
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_permissions() {
        let perms = UserPermissions::new("user1")
            .with_permission("read")
            .with_permission("write")
            .with_role("admin");
        assert!(perms.has_permission("read"));
        assert!(perms.has_permission("write"));
        assert!(!perms.has_permission("delete"));
        assert!(perms.has_any_permission(&["read", "delete"]));
        assert!(!perms.has_all_permissions(&["read", "delete"]));
    }

    #[test]
    fn test_get_permissions_empty_user() {
        let perms = HashSet::new();
        let result = PermissionService::get_permissions("", &perms);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_permissions_valid() {
        let mut perms = HashSet::new();
        perms.insert("read".to_string());
        perms.insert("write".to_string());
        let result = PermissionService::get_permissions("user1", &perms).unwrap();
        assert_eq!(result.user_id, "user1");
        assert!(result.has_permission("read"));
    }

    #[test]
    fn test_filter_items() {
        let perms = UserPermissions::new("user1").with_permission("read");
        let items = vec![
            ("item1".to_string(), vec!["read".to_string()]),
            ("item2".to_string(), vec!["write".to_string()]),
            ("item3".to_string(), vec![]),
        ];
        let filtered =
            PermissionService::filter_items(&perms, &items, |item: &(String, Vec<String>)| {
                item.1.as_slice()
            });
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].0, "item1");
        assert_eq!(filtered[1].0, "item3");
    }
}
