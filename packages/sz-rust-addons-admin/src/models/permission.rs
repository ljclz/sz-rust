// SPDX-License-Identifier: Apache-2.0
use crate::error::AdminError;
use serde::{Deserialize, Serialize};

/// 权限模块枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionModule {
    Admin,
    Tenant,
    DataPerm,
    System,
}

impl PermissionModule {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Tenant => "tenant",
            Self::DataPerm => "data_perm",
            Self::System => "system",
        }
    }
}

/// 权限项模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionModel {
    pub id: i64,
    pub code: String,
    pub name: String,
    pub module: PermissionModule,
    pub description: Option<String>,
    pub tenant_id: i64,
}

impl PermissionModel {
    /// 解析权限码格式 `{module}:{resource}:{action}`
    pub fn parse_code(code: &str) -> Result<(String, String, String), AdminError> {
        let parts: Vec<&str> = code.split(':').collect();
        if parts.len() != 3 {
            return Err(AdminError::PermissionNotFound);
        }
        if parts.iter().any(|p| p.is_empty()) {
            return Err(AdminError::PermissionNotFound);
        }
        Ok((
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_code_valid() {
        let (m, r, a) = PermissionModel::parse_code("admin:user:list").unwrap();
        assert_eq!(m, "admin");
        assert_eq!(r, "user");
        assert_eq!(a, "list");
    }

    #[test]
    fn test_parse_code_invalid() {
        assert!(PermissionModel::parse_code("admin:user").is_err());
        assert!(PermissionModel::parse_code("admin:user:list:extra").is_err());
        assert!(PermissionModel::parse_code("admin::list").is_err());
        assert!(PermissionModel::parse_code("").is_err());
    }
}
