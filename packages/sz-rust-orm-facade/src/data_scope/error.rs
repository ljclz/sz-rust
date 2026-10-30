// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据范围错误类型

use thiserror::Error;

/// 数据范围控制错误
#[derive(Debug, Clone, Error)]
pub enum DataScopeError {
    #[error("missing user context in request extension")]
    MissingUserContext,

    #[error("dept tree unavailable: {0}")]
    DeptTreeUnavailable(String),

    #[error("invalid data scope rule: {0}")]
    InvalidRule(String),

    #[error("unsafe custom condition: {0}")]
    UnsafeCustomCondition(String),

    #[error("custom generator not found: {0}")]
    GeneratorNotFound(String),

    #[error("rule not found: {0}")]
    RuleNotFound(String),

    #[error("policy not found: {0}")]
    PolicyNotFound(String),

    #[error("rule field missing: {0}")]
    RuleFieldMissing(String),

    #[error("custom generator not registered: {0}")]
    CustomGeneratorNotFound(String),

    #[error("generation conflict: current={current}")]
    GenerationConflict { current: u64 },

    #[error("config file not found: {0}")]
    ConfigFileNotFound(String),

    #[error("config parse error at line {line}: {msg}")]
    ConfigParseError { line: usize, msg: String },

    #[error("path not allowed: {0}")]
    PathNotAllowed(String),

    #[error("config file too large: size={size}, limit={limit}")]
    ConfigFileTooLarge { size: u64, limit: u64 },

    #[error("admin required")]
    AdminRequired,

    #[error("auth required")]
    AuthRequired,

    #[error("request body invalid: {0}")]
    RequestBodyInvalid(String),

    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

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

impl DataScopeError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::MissingUserContext => "DATA_SCOPE_NO_USER_CONTEXT",
            Self::DeptTreeUnavailable(_) => "DATA_SCOPE_DEPT_TREE_UNAVAILABLE",
            Self::InvalidRule(_) => "DATA_SCOPE_INVALID_RULE",
            Self::UnsafeCustomCondition(_) => "DATA_SCOPE_UNSAFE_CUSTOM",
            Self::GeneratorNotFound(_) => "DATA_SCOPE_GENERATOR_NOT_FOUND",
            Self::RuleNotFound(_) => "RULE_NOT_FOUND",
            Self::PolicyNotFound(_) => "POLICY_NOT_FOUND",
            Self::RuleFieldMissing(_) => "RULE_FIELD_MISSING",
            Self::CustomGeneratorNotFound(_) => "CUSTOM_GENERATOR_NOT_FOUND",
            Self::GenerationConflict { .. } => "GENERATION_CONFLICT",
            Self::ConfigFileNotFound(_) => "CONFIG_FILE_NOT_FOUND",
            Self::ConfigParseError { .. } => "CONFIG_PARSE_ERROR",
            Self::PathNotAllowed(_) => "PATH_NOT_ALLOWED",
            Self::ConfigFileTooLarge { .. } => "CONFIG_FILE_TOO_LARGE",
            Self::AdminRequired => "ADMIN_REQUIRED",
            Self::AuthRequired => "AUTH_REQUIRED",
            Self::RequestBodyInvalid(_) => "REQUEST_BODY_INVALID",
            Self::RateLimited { .. } => "RATE_LIMITED",
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes() {
        assert_eq!(
            DataScopeError::MissingUserContext.error_code(),
            "DATA_SCOPE_NO_USER_CONTEXT"
        );
        assert_eq!(
            DataScopeError::DeptTreeUnavailable("x".into()).error_code(),
            "DATA_SCOPE_DEPT_TREE_UNAVAILABLE"
        );
        assert_eq!(
            DataScopeError::InvalidRule("x".into()).error_code(),
            "DATA_SCOPE_INVALID_RULE"
        );
        assert_eq!(
            DataScopeError::UnsafeCustomCondition("x".into()).error_code(),
            "DATA_SCOPE_UNSAFE_CUSTOM"
        );
        assert_eq!(
            DataScopeError::GeneratorNotFound("x".into()).error_code(),
            "DATA_SCOPE_GENERATOR_NOT_FOUND"
        );
    }

    #[test]
    fn test_new_error_codes() {
        assert_eq!(
            DataScopeError::RuleNotFound("x".into()).error_code(),
            "RULE_NOT_FOUND"
        );
        assert_eq!(
            DataScopeError::PolicyNotFound("x".into()).error_code(),
            "POLICY_NOT_FOUND"
        );
        assert_eq!(
            DataScopeError::RuleFieldMissing("x".into()).error_code(),
            "RULE_FIELD_MISSING"
        );
        assert_eq!(
            DataScopeError::CustomGeneratorNotFound("x".into()).error_code(),
            "CUSTOM_GENERATOR_NOT_FOUND"
        );
        assert_eq!(
            DataScopeError::GenerationConflict { current: 5 }.error_code(),
            "GENERATION_CONFLICT"
        );
        assert_eq!(
            DataScopeError::ConfigFileNotFound("x".into()).error_code(),
            "CONFIG_FILE_NOT_FOUND"
        );
        assert_eq!(
            DataScopeError::ConfigParseError {
                line: 3,
                msg: "bad".into()
            }
            .error_code(),
            "CONFIG_PARSE_ERROR"
        );
        assert_eq!(
            DataScopeError::PathNotAllowed("x".into()).error_code(),
            "PATH_NOT_ALLOWED"
        );
        assert_eq!(
            DataScopeError::ConfigFileTooLarge {
                size: 100,
                limit: 50
            }
            .error_code(),
            "CONFIG_FILE_TOO_LARGE"
        );
        assert_eq!(DataScopeError::AdminRequired.error_code(), "ADMIN_REQUIRED");
        assert_eq!(DataScopeError::AuthRequired.error_code(), "AUTH_REQUIRED");
        assert_eq!(
            DataScopeError::RequestBodyInvalid("x".into()).error_code(),
            "REQUEST_BODY_INVALID"
        );
        assert_eq!(
            DataScopeError::RateLimited {
                retry_after_secs: 30
            }
            .error_code(),
            "RATE_LIMITED"
        );
    }

    #[test]
    fn test_tenant_error_codes() {
        assert_eq!(
            DataScopeError::TenantIdRequired.error_code(),
            "TENANT_ID_REQUIRED"
        );
        assert_eq!(
            DataScopeError::TenantMismatch {
                header_tenant: 1,
                user_tenant: 2
            }
            .error_code(),
            "TENANT_MISMATCH"
        );
        assert_eq!(
            DataScopeError::TenantNotFound(1).error_code(),
            "TENANT_NOT_FOUND"
        );
        assert_eq!(
            DataScopeError::TenantSuspended(1).error_code(),
            "TENANT_SUSPENDED"
        );
        assert_eq!(
            DataScopeError::TenantDisabled(1).error_code(),
            "TENANT_DISABLED"
        );
        assert_eq!(
            DataScopeError::TenantResolveError("x".into()).error_code(),
            "TENANT_RESOLVE_ERROR"
        );
        assert_eq!(
            DataScopeError::TenantContextRequired.error_code(),
            "TENANT_CONTEXT_REQUIRED"
        );
        assert_eq!(
            DataScopeError::InvalidTenantId(0).error_code(),
            "INVALID_TENANT_ID"
        );
        assert_eq!(
            DataScopeError::TenantConfigReloadFailed("x".into()).error_code(),
            "TENANT_CONFIG_RELOAD_FAILED"
        );
        assert_eq!(
            DataScopeError::PlatformAdminRequired.error_code(),
            "PLATFORM_ADMIN_REQUIRED"
        );
        assert_eq!(
            DataScopeError::TenantNameDuplicate("x".into()).error_code(),
            "TENANT_NAME_DUPLICATE"
        );
        assert_eq!(
            DataScopeError::InvalidStatusTransition {
                from: "disabled".into(),
                to: "active".into()
            }
            .error_code(),
            "INVALID_STATUS_TRANSITION"
        );
        assert_eq!(
            DataScopeError::TenantNotDisabled(1).error_code(),
            "TENANT_NOT_DISABLED"
        );
    }
}
