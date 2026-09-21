// SPDX-License-Identifier: Apache-2.0
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 用户-角色关联模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRoleModel {
    pub user_id: i64,
    pub role_id: i64,
    pub tenant_id: i64,
    pub created_at: DateTime<Utc>,
}
