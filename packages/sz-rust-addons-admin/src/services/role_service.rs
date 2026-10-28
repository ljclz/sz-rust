// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sz_rust_orm_facade::{Pool, Value};

use crate::error::AdminError;
use crate::models::role::RoleModel;

#[derive(Debug, Clone, Deserialize)]
pub struct CreateRoleRequest {
    pub name: String,
    pub code: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateRoleRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssignPermissionsRequest {
    pub permission_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RoleResponse {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub description: Option<String>,
    pub is_builtin: bool,
    pub tenant_id: i64,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

impl From<RoleModel> for RoleResponse {
    fn from(r: RoleModel) -> Self {
        Self {
            id: r.id,
            name: r.name,
            code: r.code,
            description: r.description,
            is_builtin: r.is_builtin,
            tenant_id: r.tenant_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Clone)]
pub struct RoleService {
    pool: Arc<Pool>,
}

impl RoleService {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    pub async fn list(&self, tenant_id: i64) -> Result<Vec<RoleResponse>, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT id, name, code, description, is_builtin, tenant_id, created_at, updated_at \
                 FROM roles WHERE tenant_id = ? ORDER BY id ASC",
                &[Value::I64(tenant_id)],
            )
            .await?;
        Ok(rows.iter().filter_map(row_to_role_response).collect())
    }

    pub async fn create(
        &self,
        req: CreateRoleRequest,
        tenant_id: i64,
    ) -> Result<RoleResponse, AdminError> {
        if req.name.trim().is_empty() {
            return Err(AdminError::RoleNameRequired);
        }
        RoleModel::validate_code_format(&req.code)?;
        RoleModel::validate_name(&req.name)?;

        let mut conn = self.pool.acquire().await?;
        let exists = conn
            .query_with_params(
                "SELECT id FROM roles WHERE code = ? AND tenant_id = ? LIMIT 1",
                &[Value::String(req.code.clone()), Value::I64(tenant_id)],
            )
            .await?;
        if !exists.is_empty() {
            return Err(AdminError::RoleCodeDuplicate);
        }

        let now = Utc::now();
        conn.execute_with_params(
            "INSERT INTO roles (name, code, description, is_builtin, tenant_id, created_at, updated_at) \
             VALUES (?, ?, ?, 0, ?, ?, ?)",
            &[
                Value::String(req.name),
                Value::String(req.code.clone()),
                req.description
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
                "SELECT id, name, code, description, is_builtin, tenant_id, created_at, updated_at \
                 FROM roles WHERE code = ? AND tenant_id = ? LIMIT 1",
                &[Value::String(req.code), Value::I64(tenant_id)],
            )
            .await?;
        rows.first()
            .and_then(row_to_role_response)
            .ok_or(AdminError::Internal("创建后查询失败".into()))
    }

    pub async fn update(
        &self,
        role_id: i64,
        req: UpdateRoleRequest,
        tenant_id: i64,
    ) -> Result<RoleResponse, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let exists = conn
            .query_with_params(
                "SELECT id, is_builtin FROM roles WHERE id = ? AND tenant_id = ? LIMIT 1",
                &[Value::I64(role_id), Value::I64(tenant_id)],
            )
            .await?;
        if exists.is_empty() {
            return Err(AdminError::RoleNotFound);
        }

        let now = Utc::now();
        let mut parts: Vec<&str> = Vec::new();
        let mut params: Vec<Value> = Vec::new();

        if let Some(ref name) = req.name {
            RoleModel::validate_name(name)?;
            parts.push("name = ?");
            params.push(Value::String(name.clone()));
        }
        if let Some(ref desc) = req.description {
            parts.push("description = ?");
            params.push(Value::String(desc.clone()));
        }
        if parts.is_empty() {
            return self.get_by_id(role_id, tenant_id).await;
        }

        parts.push("updated_at = ?");
        params.push(Value::DateTime(now.to_rfc3339()));
        params.push(Value::I64(role_id));
        params.push(Value::I64(tenant_id));

        let sql = format!(
            "UPDATE roles SET {} WHERE id = ? AND tenant_id = ?",
            parts.join(", ")
        );
        conn.execute_with_params(&sql, &params).await?;
        self.get_by_id(role_id, tenant_id).await
    }

    pub async fn delete(&self, role_id: i64, tenant_id: i64) -> Result<(), AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT id, is_builtin FROM roles WHERE id = ? AND tenant_id = ? LIMIT 1",
                &[Value::I64(role_id), Value::I64(tenant_id)],
            )
            .await?;
        if rows.is_empty() {
            return Err(AdminError::RoleNotFound);
        }
        if rows
            .first()
            .and_then(|r| r.get("is_builtin"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(AdminError::BuiltinRoleProtected);
        }

        conn.begin_transaction().await?;
        conn.execute_with_params(
            "DELETE FROM role_permissions WHERE role_id = ? AND tenant_id = ?",
            &[Value::I64(role_id), Value::I64(tenant_id)],
        )
        .await?;
        conn.execute_with_params(
            "DELETE FROM user_roles WHERE role_id = ? AND tenant_id = ?",
            &[Value::I64(role_id), Value::I64(tenant_id)],
        )
        .await?;
        let affected = conn
            .execute_with_params(
                "DELETE FROM roles WHERE id = ? AND tenant_id = ?",
                &[Value::I64(role_id), Value::I64(tenant_id)],
            )
            .await?;
        if affected == 0 {
            conn.rollback().await.ok();
            return Err(AdminError::RoleNotFound);
        }
        conn.commit().await?;
        Ok(())
    }

    pub async fn assign_permissions(
        &self,
        role_id: i64,
        permission_codes: Vec<String>,
        tenant_id: i64,
    ) -> Result<(), AdminError> {
        let mut conn = self.pool.acquire().await?;
        let role_exists = conn
            .query_with_params(
                "SELECT id FROM roles WHERE id = ? AND tenant_id = ? LIMIT 1",
                &[Value::I64(role_id), Value::I64(tenant_id)],
            )
            .await?;
        if role_exists.is_empty() {
            return Err(AdminError::RoleNotFound);
        }

        for code in &permission_codes {
            let perm_exists = conn
                .query_with_params(
                    "SELECT code FROM permissions WHERE code = ? AND (tenant_id = ? OR tenant_id = 0) LIMIT 1",
                    &[Value::String(code.clone()), Value::I64(tenant_id)],
                )
                .await?;
            if perm_exists.is_empty() {
                return Err(AdminError::PermissionNotFound);
            }
        }

        conn.begin_transaction().await?;
        conn.execute_with_params(
            "DELETE FROM role_permissions WHERE role_id = ? AND tenant_id = ?",
            &[Value::I64(role_id), Value::I64(tenant_id)],
        )
        .await?;
        for code in &permission_codes {
            conn.execute_with_params(
                "INSERT INTO role_permissions (role_id, permission_code, tenant_id, created_at) VALUES (?, ?, ?, ?)",
                &[
                    Value::I64(role_id),
                    Value::String(code.clone()),
                    Value::I64(tenant_id),
                    Value::DateTime(Utc::now().to_rfc3339()),
                ],
            )
            .await?;
        }
        conn.commit().await?;
        Ok(())
    }

    async fn get_by_id(&self, role_id: i64, tenant_id: i64) -> Result<RoleResponse, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT id, name, code, description, is_builtin, tenant_id, created_at, updated_at \
                 FROM roles WHERE id = ? AND tenant_id = ? LIMIT 1",
                &[Value::I64(role_id), Value::I64(tenant_id)],
            )
            .await?;
        rows.first()
            .and_then(row_to_role_response)
            .ok_or(AdminError::RoleNotFound)
    }
}

fn row_to_role_response(row: &std::collections::HashMap<String, Value>) -> Option<RoleResponse> {
    let id = row.get("id")?.as_i64()?;
    let name = row.get("name")?.as_str()?.to_string();
    let code = row.get("code")?.as_str()?.to_string();
    let is_builtin = row
        .get("is_builtin")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let tenant_id = row.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let description = row
        .get("description")
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

    Some(RoleResponse {
        id,
        name,
        code,
        description,
        is_builtin,
        tenant_id,
        created_at,
        updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_role_request_deserialize() {
        let json = r#"{"name":"管理员","code":"admin","description":"系统管理员"}"#;
        let req: CreateRoleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "管理员");
        assert_eq!(req.code, "admin");
    }

    #[test]
    fn test_assign_permissions_request_deserialize() {
        let json = r#"{"permission_codes":["admin:user:list","admin:role:list"]}"#;
        let req: AssignPermissionsRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.permission_codes.len(), 2);
    }

    #[test]
    fn test_role_response_from_model() {
        let model = RoleModel {
            id: 1,
            name: "管理员".into(),
            code: "admin".into(),
            description: None,
            is_builtin: false,
            tenant_id: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let resp: RoleResponse = model.into();
        assert_eq!(resp.id, 1);
        assert!(!resp.is_builtin);
    }
}
