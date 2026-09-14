// SPDX-License-Identifier: Apache-2.0
use crate::error::AdminError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 用户状态枚举
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    #[default]
    Active,
    Disabled,
    Locked,
}

impl UserStatus {
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            "locked" => Some(Self::Locked),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Locked => "locked",
        }
    }
}

/// 用户模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserModel {
    pub id: i64,
    pub username: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub status: UserStatus,
    pub tenant_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl UserModel {
    pub fn validate_username(username: &str) -> Result<(), AdminError> {
        let len = username.len();
        if !(3..=50).contains(&len) {
            Err(AdminError::UsernameRequired)
        } else {
            Ok(())
        }
    }

    pub fn validate_password_strength(password: &str) -> Result<(), AdminError> {
        if password.len() < 8 {
            Err(AdminError::PasswordTooShort)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_skip_serializing() {
        let user = UserModel {
            id: 1,
            username: "admin".into(),
            password: "secret_hash".into(),
            email: None,
            phone: None,
            status: UserStatus::Active,
            tenant_id: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login_at: None,
        };
        let json = serde_json::to_value(&user).unwrap();
        assert!(
            json.get("password").is_none(),
            "password should be skip_serializing"
        );
        assert_eq!(json["username"], "admin");
    }

    #[test]
    fn test_user_status_serde() {
        let json = serde_json::to_string(&UserStatus::Active).unwrap();
        assert_eq!(json, "\"active\"");
        let status: UserStatus = serde_json::from_str("\"disabled\"").unwrap();
        assert_eq!(status, UserStatus::Disabled);
    }

    #[test]
    fn test_validate_password_strength_boundary() {
        assert!(UserModel::validate_password_strength("1234567").is_err());
        assert!(UserModel::validate_password_strength("12345678").is_ok());
        assert!(UserModel::validate_password_strength("abcdefghijklmnop").is_ok());
    }

    #[test]
    fn test_validate_username_boundary() {
        assert!(UserModel::validate_username("ab").is_err());
        assert!(UserModel::validate_username("abc").is_ok());
        assert!(UserModel::validate_username(&"a".repeat(50)).is_ok());
        assert!(UserModel::validate_username(&"a".repeat(51)).is_err());
    }
}
