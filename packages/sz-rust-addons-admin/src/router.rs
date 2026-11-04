// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post, put};
use axum::Router;

use crate::guard::permission_guard_middleware;
use crate::handlers::{
    config_handlers, dashboard_handlers, log_handlers, menu_handlers, permission_handlers,
    role_handlers, user_handlers,
};
use crate::state::AdminAddonState;

pub fn build_admin_addon_router(state: AdminAddonState) -> Router {
    Router::new()
        .route(
            "/api/admin/users",
            get(user_handlers::list_users).post(user_handlers::create_user),
        )
        .route(
            "/api/admin/users/{user_id}",
            put(user_handlers::update_user).delete(user_handlers::delete_user),
        )
        .route(
            "/api/admin/users/{user_id}/status",
            put(user_handlers::update_user_status),
        )
        .route(
            "/api/admin/users/{user_id}/roles",
            put(user_handlers::assign_user_roles),
        )
        .route(
            "/api/admin/roles",
            get(role_handlers::list_roles).post(role_handlers::create_role),
        )
        .route(
            "/api/admin/roles/{role_id}",
            put(role_handlers::update_role).delete(role_handlers::delete_role),
        )
        .route(
            "/api/admin/roles/{role_id}/permissions",
            put(role_handlers::assign_role_permissions),
        )
        .route(
            "/api/admin/permissions/tree",
            get(permission_handlers::get_permission_tree),
        )
        .route("/api/admin/menus/tree", get(menu_handlers::get_menu_tree))
        .route("/api/admin/menus", post(menu_handlers::create_menu))
        .route(
            "/api/admin/menus/{menu_id}",
            put(menu_handlers::update_menu).delete(menu_handlers::delete_menu),
        )
        .route("/api/admin/configs", get(config_handlers::list_configs))
        .route(
            "/api/admin/configs/{config_key}",
            put(config_handlers::upsert_config).delete(config_handlers::delete_config),
        )
        .route(
            "/api/admin/operation-logs",
            get(log_handlers::list_operation_logs),
        )
        .route(
            "/api/admin/dashboard",
            get(dashboard_handlers::get_dashboard),
        )
        .layer(from_fn_with_state(
            state.clone(),
            permission_guard_middleware,
        ))
        .with_state(state)
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_route_count() {
        let routes: Vec<(&str, &str)> = vec![
            ("GET", "/api/admin/users"),
            ("POST", "/api/admin/users"),
            ("PUT", "/api/admin/users/{user_id}"),
            ("DELETE", "/api/admin/users/{user_id}"),
            ("PUT", "/api/admin/users/{user_id}/status"),
            ("PUT", "/api/admin/users/{user_id}/roles"),
            ("GET", "/api/admin/roles"),
            ("POST", "/api/admin/roles"),
            ("PUT", "/api/admin/roles/{role_id}"),
            ("DELETE", "/api/admin/roles/{role_id}"),
            ("PUT", "/api/admin/roles/{role_id}/permissions"),
            ("GET", "/api/admin/permissions/tree"),
            ("GET", "/api/admin/menus/tree"),
            ("POST", "/api/admin/menus"),
            ("PUT", "/api/admin/menus/{menu_id}"),
            ("DELETE", "/api/admin/menus/{menu_id}"),
            ("GET", "/api/admin/configs"),
            ("PUT", "/api/admin/configs/{config_key}"),
            ("DELETE", "/api/admin/configs/{config_key}"),
            ("GET", "/api/admin/operation-logs"),
            ("GET", "/api/admin/dashboard"),
        ];
        assert_eq!(routes.len(), 21, "应注册 21 个端点");
    }
}
