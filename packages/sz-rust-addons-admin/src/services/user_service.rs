// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sz_rust_orm_facade::repository::PageResult;
use sz_rust_orm_facade::{Pool, Value};

use crate::error::AdminError;
use crate::models::user::{UserModel, UserStatus};

// password 为明文入参：derive(Debug) 会使 {:?} 日志泄露明文，改为手工脱敏实现
#[derive(Clone, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub email: Option<String>,
    pub phone: Option<String>,
}

impl std::fmt::Debug for CreateUserRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateUserRequest")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("email", &self.email)
            .field("phone", &self.phone)
            .finish()
    }
}

#[derive(Clone, Deserialize)]
pub struct UpdateUserRequest {
    #[serde(skip_serializing)]
    pub password: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
}

impl std::fmt::Debug for UpdateUserRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateUserRequest")
            .field("password", &"[REDACTED]")
            .field("email", &self.email)
            .field("phone", &self.phone)
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateUserStatusRequest {
    pub status: UserStatus,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssignRolesRequest {
    pub role_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UserListFilter {
    pub username: Option<String>,
    pub status: Option<UserStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserResponse {
    pub id: i64,
    pub username: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub status: UserStatus,
    pub tenant_id: i64,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
    pub last_login_at: Option<chrono::DateTime<Utc>>,
}

impl From<UserModel> for UserResponse {
    fn from(u: UserModel) -> Self {
        Self {
            id: u.id,
            username: u.username,
            email: u.email,
            phone: u.phone,
            status: u.status,
            tenant_id: u.tenant_id,
            created_at: u.created_at,
            updated_at: u.updated_at,
            last_login_at: u.last_login_at,
        }
    }
}

#[derive(Clone)]
pub struct UserService {
    pool: Arc<Pool>,
}

impl UserService {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    pub async fn list(
        &self,
        filter: UserListFilter,
        tenant_id: i64,
        page: i64,
        size: i64,
    ) -> Result<PageResult<UserResponse>, AdminError> {
        let page = page.max(1);
        let size = size.clamp(1, 100);
        let offset = (page - 1) * size;

        let mut sql = String::from(
            "SELECT id, username, email, phone, status, tenant_id, created_at, updated_at, last_login_at \
             FROM users WHERE tenant_id = ?",
        );
        let mut params: Vec<Value> = vec![Value::I64(tenant_id)];

        if let Some(ref username) = filter.username {
            sql.push_str(" AND username LIKE ?");
            params.push(Value::String(format!("%{username}%")));
        }
        if let Some(status) = filter.status {
            sql.push_str(" AND status = ?");
            params.push(Value::String(status_to_str(status).to_string()));
        }

        let count_sql = format!("SELECT COUNT(*) AS cnt FROM ({sql}) AS sub");
        let list_sql = format!("{sql} ORDER BY id DESC LIMIT ? OFFSET ?");

        let mut conn = self.pool.acquire().await?;
        let count_rows = conn.query_with_params(&count_sql, &params).await?;
        let total: u64 = count_rows
            .first()
            .and_then(|r| r.get("cnt"))
            .and_then(Value::as_i64)
            .map(|v| v as u64)
            .unwrap_or(0);

        let mut list_params = params.clone();
        list_params.push(Value::I64(size));
        list_params.push(Value::I64(offset));
        let rows = conn.query_with_params(&list_sql, &list_params).await?;
        let items: Vec<UserResponse> = rows.iter().filter_map(row_to_user_response).collect();

        Ok(PageResult::new(items, total, page as u64, size as u64))
    }

    pub async fn create(
        &self,
        req: CreateUserRequest,
        tenant_id: i64,
    ) -> Result<UserResponse, AdminError> {
        if req.username.trim().is_empty() {
            return Err(AdminError::UsernameRequired);
        }
        UserModel::validate_username(&req.username)?;
        UserModel::validate_password_strength(&req.password)?;

        let mut conn = self.pool.acquire().await?;
        let exists_rows = conn
            .query_with_params(
                "SELECT id FROM users WHERE username = ? AND tenant_id = ? LIMIT 1",
                &[Value::String(req.username.clone()), Value::I64(tenant_id)],
            )
            .await?;
        if !exists_rows.is_empty() {
            return Err(AdminError::UserDuplicate);
        }

        let hashed =
            bcrypt::hash(&req.password, 10).map_err(|e| AdminError::Internal(e.to_string()))?;
        let now = Utc::now();

        conn.execute_with_params(
            "INSERT INTO users (username, password, email, phone, status, tenant_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 'active', ?, ?, ?)",
            &[
                Value::String(req.username.clone()),
                Value::String(hashed),
                req.email
                    .as_ref()
                    .map_or(Value::Null, |v| Value::String(v.clone())),
                req.phone
                    .as_ref()
                    .map_or(Value::Null, |v| Value::String(v.clone())),
                Value::I64(tenant_id),
                Value::DateTime(now.to_rfc3339()),
                Value::DateTime(now.to_rfc3339()),
            ],
        )
        .await?;

        let rows = conn
            .query_with_params(
                "SELECT id, username, email, phone, status, tenant_id, created_at, updated_at, last_login_at \
                 FROM users WHERE username = ? AND tenant_id = ? LIMIT 1",
                &[Value::String(req.username), Value::I64(tenant_id)],
            )
            .await?;
        rows.first()
            .and_then(row_to_user_response)
            .ok_or(AdminError::Internal("创建后查询失败".into()))
    }

    pub async fn update(
        &self,
        user_id: i64,
        req: UpdateUserRequest,
        tenant_id: i64,
    ) -> Result<UserResponse, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let exists = conn
            .query_with_params(
                "SELECT id FROM users WHERE id = ? AND tenant_id = ? LIMIT 1",
                &[Value::I64(user_id), Value::I64(tenant_id)],
            )
            .await?;
        if exists.is_empty() {
            return Err(AdminError::UserNotFound);
        }

        let now = Utc::now();
        let mut parts: Vec<&str> = Vec::new();
        let mut params: Vec<Value> = Vec::new();

        if let Some(ref pwd) = req.password {
            UserModel::validate_password_strength(pwd)?;
            let hashed = bcrypt::hash(pwd, 10).map_err(|e| AdminError::Internal(e.to_string()))?;
            parts.push("password = ?");
            params.push(Value::String(hashed));
        }
        if let Some(ref email) = req.email {
            parts.push("email = ?");
            params.push(Value::String(email.clone()));
        }
        if let Some(ref phone) = req.phone {
            parts.push("phone = ?");
            params.push(Value::String(phone.clone()));
        }
        if parts.is_empty() {
            return self.get_by_id(user_id, tenant_id).await;
        }

        parts.push("updated_at = ?");
        params.push(Value::DateTime(now.to_rfc3339()));
        params.push(Value::I64(user_id));
        params.push(Value::I64(tenant_id));

        let sql = format!(
            "UPDATE users SET {} WHERE id = ? AND tenant_id = ?",
            parts.join(", ")
        );
        conn.execute_with_params(&sql, &params).await?;

        self.get_by_id(user_id, tenant_id).await
    }

    pub async fn update_status(
        &self,
        user_id: i64,
        status: UserStatus,
        tenant_id: i64,
    ) -> Result<(), AdminError> {
        let mut conn = self.pool.acquire().await?;
        let now = Utc::now();
        let affected = conn
            .execute_with_params(
                "UPDATE users SET status = ?, updated_at = ? WHERE id = ? AND tenant_id = ?",
                &[
                    Value::String(status_to_str(status).to_string()),
                    Value::DateTime(now.to_rfc3339()),
                    Value::I64(user_id),
                    Value::I64(tenant_id),
                ],
            )
            .await?;
        if affected == 0 {
            return Err(AdminError::UserNotFound);
        }
        Ok(())
    }

    pub async fn delete(&self, user_id: i64, tenant_id: i64) -> Result<(), AdminError> {
        if self.is_super_admin(user_id, tenant_id).await? {
            return Err(AdminError::SuperAdminProtected);
        }

        let mut conn = self.pool.acquire().await?;
        conn.begin_transaction().await?;
        conn.execute_with_params(
            "DELETE FROM user_roles WHERE user_id = ? AND tenant_id = ?",
            &[Value::I64(user_id), Value::I64(tenant_id)],
        )
        .await?;
        let affected = conn
            .execute_with_params(
                "DELETE FROM users WHERE id = ? AND tenant_id = ?",
                &[Value::I64(user_id), Value::I64(tenant_id)],
            )
            .await?;
        if affected == 0 {
            conn.rollback().await.ok();
            return Err(AdminError::UserNotFound);
        }
        conn.commit().await?;
        Ok(())
    }

    pub async fn assign_roles(
        &self,
        user_id: i64,
        role_ids: Vec<i64>,
        tenant_id: i64,
    ) -> Result<(), AdminError> {
        let mut conn = self.pool.acquire().await?;
        conn.begin_transaction().await?;
        conn.execute_with_params(
            "DELETE FROM user_roles WHERE user_id = ? AND tenant_id = ?",
            &[Value::I64(user_id), Value::I64(tenant_id)],
        )
        .await?;

        for role_id in &role_ids {
            conn.execute_with_params(
                "INSERT INTO user_roles (user_id, role_id, tenant_id, created_at) VALUES (?, ?, ?, ?)",
                &[
                    Value::I64(user_id),
                    Value::I64(*role_id),
                    Value::I64(tenant_id),
                    Value::DateTime(Utc::now().to_rfc3339()),
                ],
            )
            .await?;
        }
        conn.commit().await?;
        Ok(())
    }

    pub async fn is_super_admin(&self, user_id: i64, tenant_id: i64) -> Result<bool, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT r.code AS code FROM user_roles ur \
                 JOIN roles r ON ur.role_id = r.id AND ur.tenant_id = r.tenant_id \
                 WHERE ur.user_id = ? AND ur.tenant_id = ? AND r.code = 'super_admin' LIMIT 1",
                &[Value::I64(user_id), Value::I64(tenant_id)],
            )
            .await?;
        Ok(!rows.is_empty())
    }

    async fn get_by_id(&self, user_id: i64, tenant_id: i64) -> Result<UserResponse, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT id, username, email, phone, status, tenant_id, created_at, updated_at, last_login_at \
                 FROM users WHERE id = ? AND tenant_id = ? LIMIT 1",
                &[Value::I64(user_id), Value::I64(tenant_id)],
            )
            .await?;
        rows.first()
            .and_then(row_to_user_response)
            .ok_or(AdminError::UserNotFound)
    }
}

fn status_to_str(s: UserStatus) -> &'static str {
    match s {
        UserStatus::Active => "active",
        UserStatus::Disabled => "disabled",
        UserStatus::Locked => "locked",
    }
}

fn str_to_status(s: &str) -> UserStatus {
    match s {
        "disabled" => UserStatus::Disabled,
        "locked" => UserStatus::Locked,
        _ => UserStatus::Active,
    }
}

fn row_to_user_response(row: &std::collections::HashMap<String, Value>) -> Option<UserResponse> {
    let id = row.get("id")?.as_i64()?;
    let username = row.get("username")?.as_str()?.to_string();
    let status = row
        .get("status")
        .and_then(|v| v.as_str())
        .map(str_to_status)
        .unwrap_or(UserStatus::Active);
    let tenant_id = row.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let email = row
        .get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let phone = row
        .get("phone")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let created_at = row
        .get("created_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    let updated_at = row
        .get("updated_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    let last_login_at = row
        .get("last_login_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    Some(UserResponse {
        id,
        username,
        email,
        phone,
        status,
        tenant_id,
        created_at,
        updated_at,
        last_login_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_user_request_deserialize() {
        let json =
            r#"{"username":"admin","password":"12345678","email":"a@b.com","phone":"13800000000"}"#;
        let req: CreateUserRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.username, "admin");
        assert_eq!(req.password, "12345678");
        assert_eq!(req.email.as_deref(), Some("a@b.com"));
    }

    #[test]
    fn test_update_user_request_partial() {
        let json = r#"{"email":"new@b.com"}"#;
        let req: UpdateUserRequest = serde_json::from_str(json).unwrap();
        assert!(req.password.is_none());
        assert_eq!(req.email.as_deref(), Some("new@b.com"));
    }

    #[test]
    fn test_user_list_filter_default() {
        let filter = UserListFilter::default();
        assert!(filter.username.is_none());
        assert!(filter.status.is_none());
    }

    #[test]
    fn test_user_response_from_model_no_password() {
        let model = UserModel {
            id: 1,
            username: "admin".into(),
            password: "hashed".into(),
            email: None,
            phone: None,
            status: UserStatus::Active,
            tenant_id: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login_at: None,
        };
        let resp: UserResponse = model.into();
        assert_eq!(resp.id, 1);
        assert_eq!(resp.username, "admin");
    }

    #[test]
    fn test_status_to_str_roundtrip() {
        for s in [UserStatus::Active, UserStatus::Disabled, UserStatus::Locked] {
            assert_eq!(str_to_status(status_to_str(s)), s);
        }
    }
}
