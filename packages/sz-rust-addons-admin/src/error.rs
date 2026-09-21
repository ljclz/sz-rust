// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Admin 插件错误类型（22 个变体）
#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("用户名已存在")]
    UserDuplicate,
    #[error("用户名为空")]
    UsernameRequired,
    #[error("密码长度不能少于8位")]
    PasswordTooShort,
    #[error("超级管理员账户不可删除")]
    SuperAdminProtected,
    #[error("无权操作其他租户数据")]
    CrossTenantDenied,
    #[error("用户不存在")]
    UserNotFound,
    #[error("角色标识已存在")]
    RoleCodeDuplicate,
    #[error("角色名称为空")]
    RoleNameRequired,
    #[error("内置角色不可删除")]
    BuiltinRoleProtected,
    #[error("角色不存在")]
    RoleNotFound,
    #[error("权限项不存在")]
    PermissionNotFound,
    #[error("菜单标识已存在")]
    MenuCodeDuplicate,
    #[error("父菜单不存在")]
    ParentMenuNotFound,
    #[error("菜单存在循环引用")]
    MenuCircularReference,
    #[error("存在子菜单，无法删除")]
    HasChildMenu,
    #[error("菜单不存在")]
    MenuNotFound,
    #[error("配置键已存在")]
    ConfigKeyDuplicate,
    #[error("配置值类型不匹配")]
    ConfigTypeMismatch,
    #[error("配置项不存在")]
    ConfigNotFound,
    #[error("开始时间不能晚于结束时间")]
    InvalidTimeRange,
    #[error("未认证")]
    AuthRequired,
    #[error("非管理员")]
    AdminRequired,
    #[error("无权限")]
    PermissionDenied,
    #[error("数据库错误: {0}")]
    Database(String),
    #[error("内部错误: {0}")]
    Internal(String),
}

impl AdminError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::UserDuplicate => "USER_DUPLICATE",
            Self::UsernameRequired => "USERNAME_REQUIRED",
            Self::PasswordTooShort => "PASSWORD_TOO_SHORT",
            Self::SuperAdminProtected => "SUPER_ADMIN_PROTECTED",
            Self::CrossTenantDenied => "CROSS_TENANT_DENIED",
            Self::UserNotFound => "USER_NOT_FOUND",
            Self::RoleCodeDuplicate => "ROLE_CODE_DUPLICATE",
            Self::RoleNameRequired => "ROLE_NAME_REQUIRED",
            Self::BuiltinRoleProtected => "BUILTIN_ROLE_PROTECTED",
            Self::RoleNotFound => "ROLE_NOT_FOUND",
            Self::PermissionNotFound => "PERMISSION_NOT_FOUND",
            Self::MenuCodeDuplicate => "MENU_CODE_DUPLICATE",
            Self::ParentMenuNotFound => "PARENT_MENU_NOT_FOUND",
            Self::MenuCircularReference => "MENU_CIRCULAR_REFERENCE",
            Self::HasChildMenu => "HAS_CHILD_MENU",
            Self::MenuNotFound => "MENU_NOT_FOUND",
            Self::ConfigKeyDuplicate => "CONFIG_KEY_DUPLICATE",
            Self::ConfigTypeMismatch => "CONFIG_TYPE_MISMATCH",
            Self::ConfigNotFound => "CONFIG_NOT_FOUND",
            Self::InvalidTimeRange => "INVALID_TIME_RANGE",
            Self::AuthRequired => "AUTH_REQUIRED",
            Self::AdminRequired => "ADMIN_REQUIRED",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::Database(_) => "DATABASE_ERROR",
            Self::Internal(_) => "INTERNAL_ERROR",
        }
    }

    pub fn http_status(&self) -> StatusCode {
        match self.error_code() {
            "USER_DUPLICATE"
            | "ROLE_CODE_DUPLICATE"
            | "MENU_CODE_DUPLICATE"
            | "CONFIG_KEY_DUPLICATE" => StatusCode::CONFLICT,
            "USERNAME_REQUIRED"
            | "PASSWORD_TOO_SHORT"
            | "ROLE_NAME_REQUIRED"
            | "PERMISSION_NOT_FOUND"
            | "PARENT_MENU_NOT_FOUND"
            | "MENU_CIRCULAR_REFERENCE"
            | "HAS_CHILD_MENU"
            | "CONFIG_TYPE_MISMATCH"
            | "INVALID_TIME_RANGE" => StatusCode::BAD_REQUEST,
            "SUPER_ADMIN_PROTECTED"
            | "CROSS_TENANT_DENIED"
            | "BUILTIN_ROLE_PROTECTED"
            | "ADMIN_REQUIRED"
            | "PERMISSION_DENIED" => StatusCode::FORBIDDEN,
            "AUTH_REQUIRED" => StatusCode::UNAUTHORIZED,
            "USER_NOT_FOUND" | "ROLE_NOT_FOUND" | "MENU_NOT_FOUND" | "CONFIG_NOT_FOUND" => {
                StatusCode::NOT_FOUND
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Admin 统一错误响应
#[derive(Debug, Serialize)]
pub struct AdminErrorResponse {
    pub code: String,
    pub message: String,
    pub details: serde_json::Value,
}

impl AdminErrorResponse {
    pub fn from_error(err: &AdminError) -> Self {
        Self {
            code: err.error_code().to_string(),
            message: err.to_string(),
            details: serde_json::Value::Null,
        }
    }

    pub fn from_error_with_details(err: &AdminError, details: serde_json::Value) -> Self {
        Self {
            code: err.error_code().to_string(),
            message: err.to_string(),
            details,
        }
    }
}

impl From<sz_rust_orm_facade::DbError> for AdminError {
    fn from(e: sz_rust_orm_facade::DbError) -> Self {
        match &e {
            sz_rust_orm_facade::DbError::NotFound(_) => AdminError::UserNotFound,
            sz_rust_orm_facade::DbError::UniqueViolation(msg) => {
                if msg.contains("username") {
                    AdminError::UserDuplicate
                } else if msg.contains("code") {
                    AdminError::RoleCodeDuplicate
                } else {
                    AdminError::Database(e.to_string())
                }
            }
            _ => AdminError::Database(e.to_string()),
        }
    }
}

impl From<sz_rust_orm_facade::PoolError> for AdminError {
    fn from(e: sz_rust_orm_facade::PoolError) -> Self {
        AdminError::Database(format!("连接池错误: {e}"))
    }
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        let status = self.http_status();
        let body = AdminErrorResponse::from_error(&self);
        (status, axum::Json(body)).into_response()
    }
}

impl IntoResponse for AdminErrorResponse {
    fn into_response(self) -> Response {
        let code = self.code.as_str();
        let status = match code {
            "USER_DUPLICATE"
            | "ROLE_CODE_DUPLICATE"
            | "MENU_CODE_DUPLICATE"
            | "CONFIG_KEY_DUPLICATE" => StatusCode::CONFLICT,
            "USERNAME_REQUIRED"
            | "PASSWORD_TOO_SHORT"
            | "ROLE_NAME_REQUIRED"
            | "PERMISSION_NOT_FOUND"
            | "PARENT_MENU_NOT_FOUND"
            | "MENU_CIRCULAR_REFERENCE"
            | "HAS_CHILD_MENU"
            | "CONFIG_TYPE_MISMATCH"
            | "INVALID_TIME_RANGE" => StatusCode::BAD_REQUEST,
            "SUPER_ADMIN_PROTECTED"
            | "CROSS_TENANT_DENIED"
            | "BUILTIN_ROLE_PROTECTED"
            | "ADMIN_REQUIRED"
            | "PERMISSION_DENIED" => StatusCode::FORBIDDEN,
            "AUTH_REQUIRED" => StatusCode::UNAUTHORIZED,
            "USER_NOT_FOUND" | "ROLE_NOT_FOUND" | "MENU_NOT_FOUND" | "CONFIG_NOT_FOUND" => {
                StatusCode::NOT_FOUND
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, axum::Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_mapping_all_codes() {
        assert_eq!(
            AdminError::UserDuplicate.http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AdminError::UsernameRequired.http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AdminError::PasswordTooShort.http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AdminError::SuperAdminProtected.http_status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            AdminError::CrossTenantDenied.http_status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            AdminError::UserNotFound.http_status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AdminError::RoleCodeDuplicate.http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AdminError::RoleNameRequired.http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AdminError::BuiltinRoleProtected.http_status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            AdminError::RoleNotFound.http_status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AdminError::PermissionNotFound.http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AdminError::MenuCodeDuplicate.http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AdminError::ParentMenuNotFound.http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AdminError::MenuCircularReference.http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AdminError::HasChildMenu.http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AdminError::MenuNotFound.http_status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AdminError::ConfigKeyDuplicate.http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            AdminError::ConfigTypeMismatch.http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AdminError::ConfigNotFound.http_status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AdminError::InvalidTimeRange.http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AdminError::AuthRequired.http_status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            AdminError::AdminRequired.http_status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            AdminError::PermissionDenied.http_status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            AdminError::Database("x".into()).http_status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            AdminError::Internal("x".into()).http_status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn test_error_code_naming() {
        let codes = [
            AdminError::UserDuplicate.error_code(),
            AdminError::UsernameRequired.error_code(),
            AdminError::PasswordTooShort.error_code(),
            AdminError::SuperAdminProtected.error_code(),
            AdminError::CrossTenantDenied.error_code(),
            AdminError::UserNotFound.error_code(),
            AdminError::RoleCodeDuplicate.error_code(),
            AdminError::RoleNameRequired.error_code(),
            AdminError::BuiltinRoleProtected.error_code(),
            AdminError::RoleNotFound.error_code(),
            AdminError::PermissionNotFound.error_code(),
            AdminError::MenuCodeDuplicate.error_code(),
            AdminError::ParentMenuNotFound.error_code(),
            AdminError::MenuCircularReference.error_code(),
            AdminError::HasChildMenu.error_code(),
            AdminError::MenuNotFound.error_code(),
            AdminError::ConfigKeyDuplicate.error_code(),
            AdminError::ConfigTypeMismatch.error_code(),
            AdminError::ConfigNotFound.error_code(),
            AdminError::InvalidTimeRange.error_code(),
            AdminError::AuthRequired.error_code(),
            AdminError::AdminRequired.error_code(),
            AdminError::PermissionDenied.error_code(),
        ];
        for code in codes {
            assert!(
                code.chars().all(|c| c.is_ascii_uppercase() || c == '_'),
                "error code {code} should be UPPER_SNAKE_CASE"
            );
        }
    }

    #[test]
    fn test_from_error_json_shape() {
        let err = AdminError::UserDuplicate;
        let resp = AdminErrorResponse::from_error(&err);
        assert_eq!(resp.code, "USER_DUPLICATE");
        assert_eq!(resp.message, "用户名已存在");
        assert!(resp.details.is_null());
    }

    #[test]
    fn test_from_error_with_details() {
        let err = AdminError::ConfigTypeMismatch;
        let details = serde_json::json!({"expected": "boolean", "got": "string"});
        let resp = AdminErrorResponse::from_error_with_details(&err, details.clone());
        assert_eq!(resp.code, "CONFIG_TYPE_MISMATCH");
        assert_eq!(resp.details, details);
    }
}
