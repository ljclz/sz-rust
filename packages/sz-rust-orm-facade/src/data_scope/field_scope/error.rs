// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 字段级权限错误枚举

use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum FieldScopeError {
    #[error("field scope policy load failed: {0}")]
    PolicyLoadFailed(String),

    #[error("field scope policy not found for table: {0}")]
    PolicyNotFound(String),
}

impl FieldScopeError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::PolicyLoadFailed(_) => "FIELD_SCOPE_POLICY_LOAD_FAILED",
            Self::PolicyNotFound(_) => "FIELD_SCOPE_POLICY_NOT_FOUND",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes() {
        assert_eq!(
            FieldScopeError::PolicyLoadFailed("x".into()).error_code(),
            "FIELD_SCOPE_POLICY_LOAD_FAILED"
        );
        assert_eq!(
            FieldScopeError::PolicyNotFound("x".into()).error_code(),
            "FIELD_SCOPE_POLICY_NOT_FOUND"
        );
    }
}
