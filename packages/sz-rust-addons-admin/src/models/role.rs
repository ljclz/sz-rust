// SPDX-License-Identifier: Apache-2.0
use crate::error::AdminError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 角色模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleModel {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub description: Option<String>,
    pub is_builtin: bool,
    pub tenant_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl RoleModel {
    pub fn validate_code_format(code: &str) -> Result<(), AdminError> {
        let len = code.len();
        if !(2..=50).contains(&len) {
            return Err(AdminError::RoleNameRequired);
        }
        if !code.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            return Err(AdminError::RoleNameRequired);
        }
        Ok(())
    }

    pub fn validate_name(name: &str) -> Result<(), AdminError> {
        let len = name.len();
        if !(2..=50).contains(&len) {
            Err(AdminError::RoleNameRequired)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_code_format() {
        assert!(RoleModel::validate_code_format("abc").is_ok());
        assert!(RoleModel::validate_code_format("a_b").is_ok());
        assert!(RoleModel::validate_code_format("1abc").is_err());
        assert!(RoleModel::validate_code_format("Abc").is_err());
        assert!(RoleModel::validate_code_format("a").is_err());
    }

    #[test]
    fn test_validate_name() {
        assert!(RoleModel::validate_name("ab").is_ok());
        assert!(RoleModel::validate_name(&"a".repeat(50)).is_ok());
        assert!(RoleModel::validate_name("a").is_err());
        assert!(RoleModel::validate_name(&"a".repeat(51)).is_err());
    }
}
