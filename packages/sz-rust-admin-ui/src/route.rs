// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 动态路由 API（spec §5.25 规则 2，§6.25 规则 2）
//!
//! 根据用户权限动态生成前端路由，无权限路由不注册。

use crate::permission::UserPermissions;

/// 路由元信息
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouteMeta {
    /// 路由标题
    pub title: String,
    /// 图标
    pub icon: Option<String>,
    /// 所需权限
    pub required_permissions: Vec<String>,
    /// 是否隐藏
    pub hidden: bool,
}

/// 前端路由定义（动态生成，spec §5.25 规则 2）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FrontendRoute {
    /// 路由路径
    pub path: String,
    /// 路由名称
    pub name: String,
    /// 组件路径
    pub component: String,
    /// 路由元信息
    pub meta: RouteMeta,
    /// 子路由
    pub children: Vec<FrontendRoute>,
}

impl FrontendRoute {
    /// 创建路由
    pub fn new(
        path: impl Into<String>,
        name: impl Into<String>,
        component: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            name: name.into(),
            component: component.into(),
            meta: RouteMeta {
                title: String::new(),
                icon: None,
                required_permissions: Vec::new(),
                hidden: false,
            },
            children: Vec::new(),
        }
    }

    /// 设置标题
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.meta.title = title.into();
        self
    }

    /// 设置所需权限
    pub fn with_permission(mut self, perm: impl Into<String>) -> Self {
        self.meta.required_permissions.push(perm.into());
        self
    }

    /// 添加子路由
    pub fn with_child(mut self, child: FrontendRoute) -> Self {
        self.children.push(child);
        self
    }
}

/// 路由服务（spec §5.25 规则 2）
pub struct RouteService;

impl RouteService {
    /// 动态生成前端路由（按权限过滤，spec §5.25 规则 2）
    ///
    /// 无权限路由不注册。
    pub fn generate_routes(
        permissions: &UserPermissions,
        all_routes: &[FrontendRoute],
    ) -> Vec<FrontendRoute> {
        all_routes
            .iter()
            .filter_map(|route| Self::filter_route(permissions, route))
            .collect()
    }

    /// 递归过滤路由
    fn filter_route(permissions: &UserPermissions, route: &FrontendRoute) -> Option<FrontendRoute> {
        let has_perm = route.meta.required_permissions.is_empty()
            || route
                .meta
                .required_permissions
                .iter()
                .any(|p| permissions.has_permission(p));
        if !has_perm {
            return None;
        }
        let filtered_children: Vec<FrontendRoute> = route
            .children
            .iter()
            .filter_map(|child| Self::filter_route(permissions, child))
            .collect();
        Some(FrontendRoute {
            path: route.path.clone(),
            name: route.name.clone(),
            component: route.component.clone(),
            meta: route.meta.clone(),
            children: filtered_children,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_builder() {
        let route = FrontendRoute::new("/users", "users", "UsersView")
            .with_title("用户管理")
            .with_permission("user:read");
        assert_eq!(route.path, "/users");
        assert_eq!(route.meta.title, "用户管理");
        assert_eq!(route.meta.required_permissions, vec!["user:read"]);
    }

    #[test]
    fn test_generate_routes_with_permission() {
        let perms = UserPermissions::new("user1").with_permission("user:read");
        let routes = vec![
            FrontendRoute::new("/users", "users", "UsersView").with_permission("user:read"),
            FrontendRoute::new("/settings", "settings", "SettingsView")
                .with_permission("settings:read"),
        ];
        let filtered = RouteService::generate_routes(&perms, &routes);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "/users");
    }

    #[test]
    fn test_generate_routes_no_permission() {
        let perms = UserPermissions::new("user1");
        let routes =
            vec![FrontendRoute::new("/users", "users", "UsersView").with_permission("user:read")];
        let filtered = RouteService::generate_routes(&perms, &routes);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_generate_routes_no_required_permission() {
        let perms = UserPermissions::new("user1");
        let routes = vec![FrontendRoute::new("/home", "home", "HomeView")];
        let filtered = RouteService::generate_routes(&perms, &routes);
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_generate_routes_nested() {
        let perms = UserPermissions::new("user1").with_permission("user:read");
        let routes = vec![FrontendRoute::new("/users", "users", "UsersView")
            .with_permission("user:read")
            .with_child(
                FrontendRoute::new("/users/list", "userList", "UserListView")
                    .with_permission("user:read"),
            )
            .with_child(
                FrontendRoute::new("/users/settings", "userSettings", "UserSettingsView")
                    .with_permission("settings:read"),
            )];
        let filtered = RouteService::generate_routes(&perms, &routes);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].children.len(), 1);
        assert_eq!(filtered[0].children[0].path, "/users/list");
    }
}
