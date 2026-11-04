// SPDX-License-Identifier: Apache-2.0
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 操作类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationType {
    Create,
    Update,
    Delete,
    Login,
    Logout,
}

/// 操作对象类型枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetType {
    User,
    Role,
    Permission,
    Menu,
    Config,
}

/// 操作日志模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationLogModel {
    pub id: i64,
    pub operator_id: i64,
    pub operator_name: String,
    pub operation_type: OperationType,
    pub target_type: TargetType,
    pub target_id: Option<i64>,
    pub detail: Option<serde_json::Value>,
    pub ip: Option<String>,
    pub tenant_id: i64,
    pub created_at: DateTime<Utc>,
}

impl OperationLogModel {
    pub fn new(
        operator_id: i64,
        operator_name: String,
        operation_type: OperationType,
        target_type: TargetType,
        tenant_id: i64,
    ) -> Self {
        Self {
            id: 0,
            operator_id,
            operator_name,
            operation_type,
            target_type,
            target_id: None,
            detail: None,
            ip: None,
            tenant_id,
            created_at: Utc::now(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_log_model_new_defaults() {
        let log = OperationLogModel::new(
            7,
            "admin".into(),
            OperationType::Create,
            TargetType::User,
            0,
        );
        assert_eq!(log.id, 0, "new 应将 id 初始化为 0");
        assert_eq!(log.operator_id, 7);
        assert_eq!(log.operator_name, "admin");
        assert_eq!(log.operation_type, OperationType::Create);
        assert_eq!(log.target_type, TargetType::User);
        assert_eq!(log.tenant_id, 0);
        assert!(log.target_id.is_none(), "target_id 应为 None");
        assert!(log.detail.is_none(), "detail 应为 None");
        assert!(log.ip.is_none(), "ip 应为 None");
    }

    #[test]
    fn test_operation_log_model_new_all_variants() {
        let pairs = [
            (OperationType::Create, TargetType::User),
            (OperationType::Update, TargetType::Role),
            (OperationType::Delete, TargetType::Permission),
            (OperationType::Login, TargetType::Menu),
            (OperationType::Logout, TargetType::Config),
        ];
        for (op, target) in pairs {
            let log = OperationLogModel::new(1, "u".into(), op, target, 1);
            assert_eq!(log.operation_type, op);
            assert_eq!(log.target_type, target);
        }
    }

    #[test]
    fn test_operation_type_serde_roundtrip() {
        for op in [
            OperationType::Create,
            OperationType::Update,
            OperationType::Delete,
            OperationType::Login,
            OperationType::Logout,
        ] {
            let json = serde_json::to_string(&op).unwrap();
            let back: OperationType = serde_json::from_str(&json).unwrap();
            assert_eq!(op, back);
        }
    }

    #[test]
    fn test_target_type_serde_roundtrip() {
        for t in [
            TargetType::User,
            TargetType::Role,
            TargetType::Permission,
            TargetType::Menu,
            TargetType::Config,
        ] {
            let json = serde_json::to_string(&t).unwrap();
            let back: TargetType = serde_json::from_str(&json).unwrap();
            assert_eq!(t, back);
        }
    }
}
