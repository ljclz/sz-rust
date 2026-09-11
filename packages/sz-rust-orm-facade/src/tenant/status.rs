// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户状态机 — Active ↔ Suspended → Disabled（终态）

use serde::{Deserialize, Serialize};

use super::error::TenantError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    #[default]
    Active,
    Suspended,
    Disabled,
}

impl TenantStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Disabled => "disabled",
        }
    }

    pub fn can_accept_request(&self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn transition_to(&self, target: TenantStatus) -> Result<TenantStatus, TenantError> {
        match (self, target) {
            (Self::Active, Self::Suspended) | (Self::Suspended, Self::Active) => Ok(target),
            (Self::Active, Self::Disabled) | (Self::Suspended, Self::Disabled) => Ok(target),
            (Self::Disabled, _) => Err(TenantError::InvalidStatusTransition {
                from: self.as_str().into(),
                to: target.as_str().into(),
            }),
            (Self::Active, Self::Active) | (Self::Suspended, Self::Suspended) => Ok(target),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_can_accept_request() {
        assert!(TenantStatus::Active.can_accept_request());
        assert!(!TenantStatus::Suspended.can_accept_request());
        assert!(!TenantStatus::Disabled.can_accept_request());
    }

    #[test]
    fn test_valid_transitions() {
        assert_eq!(
            TenantStatus::Active
                .transition_to(TenantStatus::Suspended)
                .unwrap(),
            TenantStatus::Suspended
        );
        assert_eq!(
            TenantStatus::Suspended
                .transition_to(TenantStatus::Active)
                .unwrap(),
            TenantStatus::Active
        );
        assert_eq!(
            TenantStatus::Active
                .transition_to(TenantStatus::Disabled)
                .unwrap(),
            TenantStatus::Disabled
        );
        assert_eq!(
            TenantStatus::Suspended
                .transition_to(TenantStatus::Disabled)
                .unwrap(),
            TenantStatus::Disabled
        );
    }

    #[test]
    fn test_invalid_transition_disabled_to_active() {
        let result = TenantStatus::Disabled.transition_to(TenantStatus::Active);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.error_code(), "INVALID_STATUS_TRANSITION");
    }

    #[test]
    fn test_status_serialize_snake_case() {
        let json = serde_json::to_string(&TenantStatus::Active).unwrap();
        assert_eq!(json, "\"active\"");
        let json = serde_json::to_string(&TenantStatus::Suspended).unwrap();
        assert_eq!(json, "\"suspended\"");
        let json = serde_json::to_string(&TenantStatus::Disabled).unwrap();
        assert_eq!(json, "\"disabled\"");
    }
}
