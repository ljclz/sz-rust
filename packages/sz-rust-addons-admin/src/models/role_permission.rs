// SPDX-License-Identifier: Apache-2.0
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 角色-权限关联模型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolePermissionModel {
    pub role_id: i64,
    pub permission_code: String,
    pub tenant_id: i64,
    pub created_at: DateTime<Utc>,
}
