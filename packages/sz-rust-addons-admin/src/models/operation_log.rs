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
