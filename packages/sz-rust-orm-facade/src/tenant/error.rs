// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户错误类型 — 13 个变体，对齐 DataScopeError 租户扩展

use serde::Serialize;
use thiserror::Error;

use crate::data_scope::error::DataScopeError;

#[derive(Debug, Clone, Error, Serialize)]
pub enum TenantError {
    #[error("tenant id required")]
    TenantIdRequired,

    #[error("tenant mismatch: header={header_tenant}, user={user_tenant}")]
    TenantMismatch {
        header_tenant: i64,
        user_tenant: i64,
    },

    #[error("tenant not found: {0}")]
    TenantNotFound(i64),

    #[error("tenant suspended: {0}")]
    TenantSuspended(i64),

    #[error("tenant disabled: {0}")]
    TenantDisabled(i64),

    #[error("tenant resolve error: {0}")]
    TenantResolveError(String),

    #[error("tenant context required")]
    TenantContextRequired,

    #[error("invalid tenant id: {0}")]
    InvalidTenantId(i64),

    #[error("tenant config reload failed: {0}")]
    TenantConfigReloadFailed(String),

    #[error("platform admin required")]
    PlatformAdminRequired,

    #[error("tenant name duplicate: {0}")]
    TenantNameDuplicate(String),

    #[error("invalid status transition: {from} -> {to}")]
    InvalidStatusTransition { from: String, to: String },

    #[error("tenant not disabled: {0}")]
    TenantNotDisabled(i64),
}

impl TenantError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::TenantIdRequired => "TENANT_ID_REQUIRED",
            Self::TenantMismatch { .. } => "TENANT_MISMATCH",
            Self::TenantNotFound(_) => "TENANT_NOT_FOUND",
            Self::TenantSuspended(_) => "TENANT_SUSPENDED",
            Self::TenantDisabled(_) => "TENANT_DISABLED",
            Self::TenantResolveError(_) => "TENANT_RESOLVE_ERROR",
            Self::TenantContextRequired => "TENANT_CONTEXT_REQUIRED",
            Self::InvalidTenantId(_) => "INVALID_TENANT_ID",
            Self::TenantConfigReloadFailed(_) => "TENANT_CONFIG_RELOAD_FAILED",
            Self::PlatformAdminRequired => "PLATFORM_ADMIN_REQUIRED",
            Self::TenantNameDuplicate(_) => "TENANT_NAME_DUPLICATE",
            Self::InvalidStatusTransition { .. } => "INVALID_STATUS_TRANSITION",
            Self::TenantNotDisabled(_) => "TENANT_NOT_DISABLED",
        }
    }

    pub fn http_status(&self) -> u16 {
        match self {
            Self::TenantIdRequired
            | Self::InvalidTenantId(_)
            | Self::TenantContextRequired
            | Self::InvalidStatusTransition { .. }
            | Self::TenantNotDisabled(_) => 400,
            Self::TenantMismatch { .. }
            | Self::TenantNotFound(_)
            | Self::TenantSuspended(_)
            | Self::TenantDisabled(_)
            | Self::PlatformAdminRequired => 403,
            Self::TenantNameDuplicate(_) => 409,
            Self::TenantResolveError(_) | Self::TenantConfigReloadFailed(_) => 500,
        }
    }
}

impl From<TenantError> for DataScopeError {
    fn from(err: TenantError) -> Self {
        match err {
            TenantError::TenantIdRequired => DataScopeError::TenantIdRequired,
            TenantError::TenantMismatch {
                header_tenant,
                user_tenant,
            } => DataScopeError::TenantMismatch {
                header_tenant,
                user_tenant,
            },
            TenantError::TenantNotFound(id) => DataScopeError::TenantNotFound(id),
            TenantError::TenantSuspended(id) => DataScopeError::TenantSuspended(id),
            TenantError::TenantDisabled(id) => DataScopeError::TenantDisabled(id),
            TenantError::TenantResolveError(msg) => DataScopeError::TenantResolveError(msg),
            TenantError::TenantContextRequired => DataScopeError::TenantContextRequired,
            TenantError::InvalidTenantId(id) => DataScopeError::InvalidTenantId(id),
            TenantError::TenantConfigReloadFailed(msg) => {
                DataScopeError::TenantConfigReloadFailed(msg)
            }
            TenantError::PlatformAdminRequired => DataScopeError::PlatformAdminRequired,
            TenantError::TenantNameDuplicate(name) => DataScopeError::TenantNameDuplicate(name),
            TenantError::InvalidStatusTransition { from, to } => {
                DataScopeError::InvalidStatusTransition { from, to }
            }
            TenantError::TenantNotDisabled(id) => DataScopeError::TenantNotDisabled(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_error_code_and_status() {
        assert_eq!(
            TenantError::TenantIdRequired.error_code(),
            "TENANT_ID_REQUIRED"
        );
        assert_eq!(TenantError::TenantIdRequired.http_status(), 400);

        assert_eq!(
            TenantError::TenantMismatch {
                header_tenant: 1,
                user_tenant: 2
            }
            .error_code(),
            "TENANT_MISMATCH"
        );
        assert_eq!(
            TenantError::TenantMismatch {
                header_tenant: 1,
                user_tenant: 2
            }
            .http_status(),
            403
        );

        assert_eq!(
            TenantError::TenantNotFound(1).error_code(),
            "TENANT_NOT_FOUND"
        );
        assert_eq!(TenantError::TenantNotFound(1).http_status(), 403);

        assert_eq!(
            TenantError::TenantSuspended(1).error_code(),
            "TENANT_SUSPENDED"
        );
        assert_eq!(TenantError::TenantSuspended(1).http_status(), 403);

        assert_eq!(
            TenantError::TenantDisabled(1).error_code(),
            "TENANT_DISABLED"
        );
        assert_eq!(TenantError::TenantDisabled(1).http_status(), 403);

        assert_eq!(
            TenantError::TenantResolveError("x".into()).error_code(),
            "TENANT_RESOLVE_ERROR"
        );
        assert_eq!(
            TenantError::TenantResolveError("x".into()).http_status(),
            500
        );

        assert_eq!(
            TenantError::TenantContextRequired.error_code(),
            "TENANT_CONTEXT_REQUIRED"
        );
        assert_eq!(TenantError::TenantContextRequired.http_status(), 400);

        assert_eq!(
            TenantError::InvalidTenantId(0).error_code(),
            "INVALID_TENANT_ID"
        );
        assert_eq!(TenantError::InvalidTenantId(0).http_status(), 400);

        assert_eq!(
            TenantError::TenantConfigReloadFailed("x".into()).error_code(),
            "TENANT_CONFIG_RELOAD_FAILED"
        );
        assert_eq!(
            TenantError::TenantConfigReloadFailed("x".into()).http_status(),
            500
        );

        assert_eq!(
            TenantError::PlatformAdminRequired.error_code(),
            "PLATFORM_ADMIN_REQUIRED"
        );
        assert_eq!(TenantError::PlatformAdminRequired.http_status(), 403);

        assert_eq!(
            TenantError::TenantNameDuplicate("x".into()).error_code(),
            "TENANT_NAME_DUPLICATE"
        );
        assert_eq!(
            TenantError::TenantNameDuplicate("x".into()).http_status(),
            409
        );

        assert_eq!(
            TenantError::InvalidStatusTransition {
                from: "disabled".into(),
                to: "active".into()
            }
            .error_code(),
            "INVALID_STATUS_TRANSITION"
        );
        assert_eq!(
            TenantError::InvalidStatusTransition {
                from: "disabled".into(),
                to: "active".into()
            }
            .http_status(),
            400
        );

        assert_eq!(
            TenantError::TenantNotDisabled(1).error_code(),
            "TENANT_NOT_DISABLED"
        );
        assert_eq!(TenantError::TenantNotDisabled(1).http_status(), 400);
    }

    #[test]
    fn test_from_tenant_error_to_data_scope_error() {
        let err: DataScopeError = TenantError::TenantIdRequired.into();
        assert_eq!(err.error_code(), "TENANT_ID_REQUIRED");

        let err: DataScopeError = TenantError::TenantNotFound(5).into();
        assert_eq!(err.error_code(), "TENANT_NOT_FOUND");

        let err: DataScopeError = TenantError::PlatformAdminRequired.into();
        assert_eq!(err.error_code(), "PLATFORM_ADMIN_REQUIRED");
    }
}
