// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::Arc;

use chrono::Utc;
use serde::Deserialize;
use sz_rust_orm_facade::{Pool, Value};

use crate::error::AdminError;
use crate::models::menu::{build_menu_tree, detect_circular_reference, MenuModel, MenuTreeNode};

#[derive(Debug, Clone, Deserialize)]
pub struct CreateMenuRequest {
    pub name: String,
    pub code: String,
    pub path: String,
    pub icon: Option<String>,
    pub parent_id: i64,
    pub sort: i32,
    pub is_visible: bool,
    pub permission_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateMenuRequest {
    pub name: Option<String>,
    pub path: Option<String>,
    pub icon: Option<String>,
    pub parent_id: Option<i64>,
    pub sort: Option<i32>,
    pub is_visible: Option<bool>,
    pub permission_code: Option<String>,
}

#[derive(Clone)]
pub struct MenuService {
    pool: Arc<Pool>,
}

impl MenuService {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    pub async fn list_all(&self, tenant_id: i64) -> Result<Vec<MenuModel>, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT id, name, code, path, icon, parent_id, sort, is_visible, permission_code, tenant_id, created_at, updated_at \
                 FROM menus WHERE tenant_id = ? ORDER BY parent_id ASC, sort ASC",
                &[Value::I64(tenant_id)],
            )
            .await?;
        Ok(rows.iter().filter_map(row_to_menu).collect())
    }

    pub async fn tree(
        &self,
        user_permissions: &[String],
        tenant_id: i64,
    ) -> Result<Vec<MenuTreeNode>, AdminError> {
        let menus = self.list_all(tenant_id).await?;
        Ok(build_menu_tree(menus, user_permissions))
    }

    pub async fn create(
        &self,
        req: CreateMenuRequest,
        tenant_id: i64,
    ) -> Result<MenuModel, AdminError> {
        let mut conn = self.pool.acquire().await?;

        let exists = conn
            .query_with_params(
                "SELECT id FROM menus WHERE code = ? AND tenant_id = ? LIMIT 1",
                &[Value::String(req.code.clone()), Value::I64(tenant_id)],
            )
            .await?;
        if !exists.is_empty() {
            return Err(AdminError::MenuCodeDuplicate);
        }

        if req.parent_id > 0 {
            let parent_exists = conn
                .query_with_params(
                    "SELECT id FROM menus WHERE id = ? AND tenant_id = ? LIMIT 1",
                    &[Value::I64(req.parent_id), Value::I64(tenant_id)],
                )
                .await?;
            if parent_exists.is_empty() {
                return Err(AdminError::ParentMenuNotFound);
            }
        }

        let now = Utc::now();
        conn.execute_with_params(
            "INSERT INTO menus (name, code, path, icon, parent_id, sort, is_visible, permission_code, tenant_id, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[
                Value::String(req.name),
                Value::String(req.code.clone()),
                Value::String(req.path),
                req.icon
                    .as_ref()
                    .map_or(Value::Null, |v| Value::String(v.clone())),
                Value::I64(req.parent_id),
                Value::I32(req.sort),
                Value::Bool(req.is_visible),
                req.permission_code
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
                "SELECT id, name, code, path, icon, parent_id, sort, is_visible, permission_code, tenant_id, created_at, updated_at \
                 FROM menus WHERE code = ? AND tenant_id = ? LIMIT 1",
                &[Value::String(req.code), Value::I64(tenant_id)],
            )
            .await?;
        rows.first()
            .and_then(row_to_menu)
            .ok_or(AdminError::Internal("创建后查询失败".into()))
    }

    pub async fn update(
        &self,
        menu_id: i64,
        req: UpdateMenuRequest,
        tenant_id: i64,
    ) -> Result<MenuModel, AdminError> {
        let menus = self.list_all(tenant_id).await?;
        let existing = menus
            .iter()
            .find(|m| m.id == menu_id)
            .ok_or(AdminError::MenuNotFound)?;

        if let Some(new_parent_id) = req.parent_id {
            if new_parent_id > 0 && new_parent_id != existing.parent_id {
                let parent_exists = menus.iter().any(|m| m.id == new_parent_id);
                if !parent_exists {
                    return Err(AdminError::ParentMenuNotFound);
                }
                if detect_circular_reference(&menus, menu_id, new_parent_id) {
                    return Err(AdminError::MenuCircularReference);
                }
            }
        }

        let now = Utc::now();
        let mut parts: Vec<&str> = Vec::new();
        let mut params: Vec<Value> = Vec::new();

        if let Some(ref name) = req.name {
            parts.push("name = ?");
            params.push(Value::String(name.clone()));
        }
        if let Some(ref path) = req.path {
            parts.push("path = ?");
            params.push(Value::String(path.clone()));
        }
        if let Some(ref icon) = req.icon {
            parts.push("icon = ?");
            params.push(Value::String(icon.clone()));
        }
        if let Some(parent_id) = req.parent_id {
            parts.push("parent_id = ?");
            params.push(Value::I64(parent_id));
        }
        if let Some(sort) = req.sort {
            parts.push("sort = ?");
            params.push(Value::I32(sort));
        }
        if let Some(is_visible) = req.is_visible {
            parts.push("is_visible = ?");
            params.push(Value::Bool(is_visible));
        }
        if let Some(ref perm) = req.permission_code {
            parts.push("permission_code = ?");
            params.push(Value::String(perm.clone()));
        }
        if parts.is_empty() {
            return Ok(existing.clone());
        }

        parts.push("updated_at = ?");
        params.push(Value::DateTime(now.to_rfc3339()));
        params.push(Value::I64(menu_id));
        params.push(Value::I64(tenant_id));

        let sql = format!(
            "UPDATE menus SET {} WHERE id = ? AND tenant_id = ?",
            parts.join(", ")
        );
        let mut conn = self.pool.acquire().await?;
        conn.execute_with_params(&sql, &params).await?;

        self.get_by_id(menu_id, tenant_id).await
    }

    pub async fn delete(&self, menu_id: i64, tenant_id: i64) -> Result<(), AdminError> {
        let mut conn = self.pool.acquire().await?;

        let child_exists = conn
            .query_with_params(
                "SELECT id FROM menus WHERE parent_id = ? AND tenant_id = ? LIMIT 1",
                &[Value::I64(menu_id), Value::I64(tenant_id)],
            )
            .await?;
        if !child_exists.is_empty() {
            return Err(AdminError::HasChildMenu);
        }

        let affected = conn
            .execute_with_params(
                "DELETE FROM menus WHERE id = ? AND tenant_id = ?",
                &[Value::I64(menu_id), Value::I64(tenant_id)],
            )
            .await?;
        if affected == 0 {
            return Err(AdminError::MenuNotFound);
        }
        Ok(())
    }

    async fn get_by_id(&self, menu_id: i64, tenant_id: i64) -> Result<MenuModel, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT id, name, code, path, icon, parent_id, sort, is_visible, permission_code, tenant_id, created_at, updated_at \
                 FROM menus WHERE id = ? AND tenant_id = ? LIMIT 1",
                &[Value::I64(menu_id), Value::I64(tenant_id)],
            )
            .await?;
        rows.first()
            .and_then(row_to_menu)
            .ok_or(AdminError::MenuNotFound)
    }
}

fn row_to_menu(row: &std::collections::HashMap<String, Value>) -> Option<MenuModel> {
    let id = row.get("id")?.as_i64()?;
    let name = row.get("name")?.as_str()?.to_string();
    let code = row.get("code")?.as_str()?.to_string();
    let path = row.get("path")?.as_str()?.to_string();
    let icon = row
        .get("icon")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let parent_id = row.get("parent_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let sort = row.get("sort").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let is_visible = row
        .get("is_visible")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let permission_code = row
        .get("permission_code")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let tenant_id = row.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
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

    Some(MenuModel {
        id,
        name,
        code,
        path,
        icon,
        parent_id,
        sort,
        is_visible,
        permission_code,
        tenant_id,
        created_at,
        updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_menu_request_deserialize() {
        let json = r#"{"name":"用户管理","code":"user_mgr","path":"/admin/users","parent_id":0,"sort":1,"is_visible":true}"#;
        let req: CreateMenuRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "用户管理");
        assert_eq!(req.parent_id, 0);
        assert!(req.is_visible);
    }

    #[test]
    fn test_update_menu_request_partial() {
        let json = r#"{"sort":5}"#;
        let req: UpdateMenuRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.sort, Some(5));
        assert!(req.name.is_none());
    }
}
