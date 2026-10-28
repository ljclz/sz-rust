// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sz_rust_middleware_facade::data_scope::DataScopeUserContext;
use sz_rust_orm_facade::Value;

use crate::error::{AdminError, AdminErrorResponse};
use crate::state::AdminAddonState;

pub async fn permission_guard_middleware(
    State(state): State<AdminAddonState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let user = req.extensions().get::<DataScopeUserContext>().cloned();
    let user = match user {
        Some(u) => u,
        None => {
            return error_response(AdminError::AuthRequired);
        }
    };

    if user.is_super {
        return next.run(req).await;
    }

    let required_perm = map_route_to_permission(req.method(), req.uri().path());
    let Some(perm) = required_perm else {
        return next.run(req).await;
    };

    match check_user_permission(&state, &user, perm).await {
        Ok(true) => next.run(req).await,
        Ok(false) => error_response(AdminError::PermissionDenied),
        Err(_) => error_response(AdminError::Database("权限查询失败".into())),
    }
}

async fn check_user_permission(
    state: &AdminAddonState,
    user: &DataScopeUserContext,
    permission_code: &str,
) -> Result<bool, AdminError> {
    let mut conn = state.pool.acquire().await?;
    let rows = conn
        .query_with_params(
            "SELECT rp.permission_code FROM user_roles ur \
             JOIN role_permissions rp ON ur.role_id = rp.role_id AND ur.tenant_id = rp.tenant_id \
             WHERE ur.user_id = ? AND ur.tenant_id = ? AND rp.permission_code = ? LIMIT 1",
            &[
                Value::I64(user.user_id),
                Value::I64(0),
                Value::String(permission_code.to_string()),
            ],
        )
        .await?;
    Ok(!rows.is_empty())
}

pub fn map_route_to_permission(method: &Method, path: &str) -> Option<&'static str> {
    if !path.starts_with("/api/admin/") {
        return None;
    }

    let path = &path["/api/admin/".len()..];

    if path.starts_with("users") {
        return map_users_route(method, path);
    }
    if path.starts_with("roles") {
        return map_roles_route(method, path);
    }
    if path.starts_with("permissions") {
        return Some("admin:permission:list");
    }
    if path.starts_with("menus") {
        return map_menus_route(method, path);
    }
    if path.starts_with("configs") {
        return map_configs_route(method, path);
    }
    if path.starts_with("operation-logs") {
        return Some("admin:log:list");
    }
    if path.starts_with("dashboard") {
        return Some("admin:dashboard:view");
    }
    None
}

fn map_users_route(method: &Method, path: &str) -> Option<&'static str> {
    let rest = &path["users".len()..];
    if rest.is_empty() || rest == "/" {
        return match *method {
            Method::GET => Some("admin:user:list"),
            Method::POST => Some("admin:user:create"),
            _ => None,
        };
    }
    if let Some(rest) = rest.strip_prefix('/') {
        if rest.contains("/status") {
            return Some("admin:user:update");
        }
        if rest.contains("/roles") {
            return Some("admin:user:update");
        }
        return match *method {
            Method::PUT => Some("admin:user:update"),
            Method::DELETE => Some("admin:user:delete"),
            _ => None,
        };
    }
    None
}

fn map_roles_route(method: &Method, path: &str) -> Option<&'static str> {
    let rest = &path["roles".len()..];
    if rest.is_empty() || rest == "/" {
        return match *method {
            Method::GET => Some("admin:role:list"),
            Method::POST => Some("admin:role:create"),
            _ => None,
        };
    }
    if let Some(rest) = rest.strip_prefix('/') {
        if rest.contains("/permissions") {
            return Some("admin:role:update");
        }
        return match *method {
            Method::PUT => Some("admin:role:update"),
            Method::DELETE => Some("admin:role:delete"),
            _ => None,
        };
    }
    None
}

fn map_menus_route(method: &Method, path: &str) -> Option<&'static str> {
    let rest = &path["menus".len()..];
    if rest.is_empty() || rest == "/" || rest == "/tree" {
        return match *method {
            Method::GET => Some("admin:menu:list"),
            Method::POST => Some("admin:menu:create"),
            _ => None,
        };
    }
    if rest.starts_with('/') {
        return match *method {
            Method::PUT => Some("admin:menu:update"),
            Method::DELETE => Some("admin:menu:delete"),
            _ => None,
        };
    }
    None
}

fn map_configs_route(method: &Method, path: &str) -> Option<&'static str> {
    let rest = &path["configs".len()..];
    if rest.is_empty() || rest == "/" {
        return match *method {
            Method::GET => Some("admin:config:list"),
            _ => None,
        };
    }
    if rest.starts_with('/') {
        return match *method {
            Method::PUT | Method::DELETE => Some("admin:config:update"),
            _ => None,
        };
    }
    None
}

fn error_response(err: AdminError) -> Response {
    let status = err.http_status();
    let body = AdminErrorResponse::from_error(&err);
    (status, axum::Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_users_routes() {
        assert_eq!(
            map_route_to_permission(&Method::GET, "/api/admin/users"),
            Some("admin:user:list")
        );
        assert_eq!(
            map_route_to_permission(&Method::POST, "/api/admin/users"),
            Some("admin:user:create")
        );
        assert_eq!(
            map_route_to_permission(&Method::PUT, "/api/admin/users/1"),
            Some("admin:user:update")
        );
        assert_eq!(
            map_route_to_permission(&Method::DELETE, "/api/admin/users/1"),
            Some("admin:user:delete")
        );
        assert_eq!(
            map_route_to_permission(&Method::PUT, "/api/admin/users/1/status"),
            Some("admin:user:update")
        );
        assert_eq!(
            map_route_to_permission(&Method::PUT, "/api/admin/users/1/roles"),
            Some("admin:user:update")
        );
    }

    #[test]
    fn test_map_roles_routes() {
        assert_eq!(
            map_route_to_permission(&Method::GET, "/api/admin/roles"),
            Some("admin:role:list")
        );
        assert_eq!(
            map_route_to_permission(&Method::POST, "/api/admin/roles"),
            Some("admin:role:create")
        );
        assert_eq!(
            map_route_to_permission(&Method::PUT, "/api/admin/roles/1"),
            Some("admin:role:update")
        );
        assert_eq!(
            map_route_to_permission(&Method::DELETE, "/api/admin/roles/1"),
            Some("admin:role:delete")
        );
        assert_eq!(
            map_route_to_permission(&Method::PUT, "/api/admin/roles/1/permissions"),
            Some("admin:role:update")
        );
    }

    #[test]
    fn test_map_other_routes() {
        assert_eq!(
            map_route_to_permission(&Method::GET, "/api/admin/permissions/tree"),
            Some("admin:permission:list")
        );
        assert_eq!(
            map_route_to_permission(&Method::GET, "/api/admin/menus/tree"),
            Some("admin:menu:list")
        );
        assert_eq!(
            map_route_to_permission(&Method::POST, "/api/admin/menus"),
            Some("admin:menu:create")
        );
        assert_eq!(
            map_route_to_permission(&Method::PUT, "/api/admin/menus/1"),
            Some("admin:menu:update")
        );
        assert_eq!(
            map_route_to_permission(&Method::DELETE, "/api/admin/menus/1"),
            Some("admin:menu:delete")
        );
        assert_eq!(
            map_route_to_permission(&Method::GET, "/api/admin/configs"),
            Some("admin:config:list")
        );
        assert_eq!(
            map_route_to_permission(&Method::PUT, "/api/admin/configs/site_name"),
            Some("admin:config:update")
        );
        assert_eq!(
            map_route_to_permission(&Method::DELETE, "/api/admin/configs/site_name"),
            Some("admin:config:update")
        );
        assert_eq!(
            map_route_to_permission(&Method::GET, "/api/admin/operation-logs"),
            Some("admin:log:list")
        );
        assert_eq!(
            map_route_to_permission(&Method::GET, "/api/admin/dashboard"),
            Some("admin:dashboard:view")
        );
    }

    #[test]
    fn test_map_non_admin_route() {
        assert_eq!(map_route_to_permission(&Method::GET, "/api/other"), None);
    }
}
