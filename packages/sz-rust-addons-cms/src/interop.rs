// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CMS 跨插件互通类型（本地定义，避免对 sz-rust-sz300 的循环依赖）。
//!
//! 与 `sz_rust_sz300::bootstrap_interop` 保持类型等价。

use std::sync::Arc;

use serde_json::{json, Value};
use sz_rust_core::orm::repository::{EntityAttributes, InMemoryRepository, Repository};
use sz_rust_core::orm::Value as OrmValue;
use sz_rust_core::plugin::SysUser;

/// SysUser 的本地新类型。
#[derive(Debug, Clone)]
pub struct UserRecord(pub SysUser);

impl EntityAttributes for UserRecord {
    fn get_attribute(&self, field: &str) -> Option<OrmValue> {
        match field {
            "id" => Some(OrmValue::I64(self.0.id)),
            "tenant_id" => Some(OrmValue::I64(self.0.tenant_id)),
            "username" => Some(OrmValue::String(self.0.username.clone())),
            "display_name" => Some(OrmValue::String(self.0.display_name.clone())),
            "status" => Some(OrmValue::String(self.0.status.clone())),
            _ => None,
        }
    }
}

/// 共享用户仓储。
pub type SharedUserRepo = Arc<InMemoryRepository<UserRecord>>;

/// 校验 `owner_id` 指向的用户存在且属于指定租户。
pub async fn validate_owner_exists(
    shared_users: &SharedUserRepo,
    tenant_id: i64,
    owner_id: i64,
) -> Result<(), Value> {
    if owner_id == 0 {
        return Ok(());
    }
    let user = shared_users
        .find_by_id(&OrmValue::I64(owner_id))
        .map_err(|e| json!({"code": 500, "msg": format!("仓储查询失败: {e}")}))?;
    match user {
        None => Err(json!({"code": 422, "msg": "owner_id 指向的用户不存在"})),
        Some(record) if record.0.tenant_id != tenant_id => {
            Err(json!({"code": 403, "msg": "跨租户引用"}))
        }
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_core::plugin::SysUser;

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now()
    }

    fn make_user(id: i64, tenant_id: i64, username: &str) -> UserRecord {
        UserRecord(SysUser {
            id,
            tenant_id,
            username: username.to_string(),
            display_name: username.to_string(),
            password_hash: String::new(),
            email: None,
            phone: None,
            status: "active".to_string(),
            extra: json!({}),
            created_at: now(),
            updated_at: now(),
        })
    }

    #[tokio::test]
    async fn owner_zero_passes() {
        let repo: SharedUserRepo = Arc::new(InMemoryRepository::new());
        assert!(validate_owner_exists(&repo, 1, 0).await.is_ok());
    }

    #[tokio::test]
    async fn owner_exists_same_tenant_passes() {
        let repo: SharedUserRepo = Arc::new(InMemoryRepository::from_vec(vec![make_user(
            10, 1, "alice",
        )]));
        assert!(validate_owner_exists(&repo, 1, 10).await.is_ok());
    }

    #[tokio::test]
    async fn owner_not_exist_returns_422() {
        let repo: SharedUserRepo = Arc::new(InMemoryRepository::new());
        let err = validate_owner_exists(&repo, 1, 99).await.unwrap_err();
        assert_eq!(err["code"], json!(422));
    }

    #[tokio::test]
    async fn owner_cross_tenant_returns_403() {
        let repo: SharedUserRepo =
            Arc::new(InMemoryRepository::from_vec(vec![make_user(10, 2, "bob")]));
        let err = validate_owner_exists(&repo, 1, 10).await.unwrap_err();
        assert_eq!(err["code"], json!(403));
    }
}
