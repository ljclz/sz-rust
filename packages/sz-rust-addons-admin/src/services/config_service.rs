// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sz_rust_orm_facade::{Pool, Value};

use crate::error::AdminError;
use crate::models::config::{ConfigModel, ConfigType};

#[derive(Debug, Clone, Deserialize)]
pub struct UpsertConfigRequest {
    pub value: serde_json::Value,
    pub value_type: ConfigType,
    pub group: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigResponse {
    pub id: i64,
    pub key: String,
    pub value: serde_json::Value,
    pub value_type: ConfigType,
    pub group: Option<String>,
    pub description: Option<String>,
    pub tenant_id: i64,
    pub created_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
}

impl From<ConfigModel> for ConfigResponse {
    fn from(c: ConfigModel) -> Self {
        Self {
            id: c.id,
            key: c.key,
            value: c.value,
            value_type: c.value_type,
            group: c.group,
            description: c.description,
            tenant_id: c.tenant_id,
            created_at: c.created_at,
            updated_at: c.updated_at,
        }
    }
}

#[derive(Clone)]
pub struct ConfigService {
    pool: Arc<Pool>,
}

impl ConfigService {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    pub async fn list(
        &self,
        group: Option<String>,
        tenant_id: i64,
    ) -> Result<Vec<ConfigResponse>, AdminError> {
        let mut sql = String::from(
            "SELECT id, `key`, value, value_type, `group`, description, tenant_id, created_at, updated_at \
             FROM configs WHERE (tenant_id = ? OR tenant_id = 0)",
        );
        let mut params: Vec<Value> = vec![Value::I64(tenant_id)];

        if let Some(ref g) = group {
            sql.push_str(" AND `group` = ?");
            params.push(Value::String(g.clone()));
        }
        sql.push_str(" ORDER BY tenant_id DESC, `key` ASC");

        let mut conn = self.pool.acquire().await?;
        let rows = conn.query_with_params(&sql, &params).await?;
        Ok(rows.iter().filter_map(row_to_config_response).collect())
    }

    pub async fn upsert(
        &self,
        key: String,
        req: UpsertConfigRequest,
        tenant_id: i64,
    ) -> Result<ConfigResponse, AdminError> {
        ConfigModel::validate_value_type(&req.value, req.value_type)?;

        let value_str =
            serde_json::to_string(&req.value).map_err(|e| AdminError::Internal(e.to_string()))?;
        let type_str = config_type_to_str(req.value_type);
        let now = Utc::now();

        let mut conn = self.pool.acquire().await?;
        let exists = conn
            .query_with_params(
                "SELECT id FROM configs WHERE `key` = ? AND tenant_id = ? LIMIT 1",
                &[Value::String(key.clone()), Value::I64(tenant_id)],
            )
            .await?;

        if !exists.is_empty() {
            conn.execute_with_params(
                "UPDATE configs SET value = ?, value_type = ?, `group` = ?, description = ?, updated_at = ? \
                 WHERE `key` = ? AND tenant_id = ?",
                &[
                    Value::String(value_str),
                    Value::String(type_str.to_string()),
                    req.group
                        .as_ref()
                        .map_or(Value::Null, |v| Value::String(v.clone())),
                    req.description
                        .as_ref()
                        .map_or(Value::Null, |v| Value::String(v.clone())),
                    Value::DateTime(now.to_rfc3339()),
                    Value::String(key.clone()),
                    Value::I64(tenant_id),
                ],
            )
            .await?;
        } else {
            conn.execute_with_params(
                "INSERT INTO configs (`key`, value, value_type, `group`, description, tenant_id, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                &[
                    Value::String(key.clone()),
                    Value::String(value_str),
                    Value::String(type_str.to_string()),
                    req.group
                        .as_ref()
                        .map_or(Value::Null, |v| Value::String(v.clone())),
                    req.description
                        .as_ref()
                        .map_or(Value::Null, |v| Value::String(v.clone())),
                    Value::I64(tenant_id),
                    Value::DateTime(now.to_rfc3339()),
                    Value::DateTime(now.to_rfc3339()),
                ],
            )
            .await?;
        }

        self.get_by_key(&key, tenant_id).await
    }

    pub async fn delete(&self, key: &str, tenant_id: i64) -> Result<(), AdminError> {
        let mut conn = self.pool.acquire().await?;
        let affected = conn
            .execute_with_params(
                "DELETE FROM configs WHERE `key` = ? AND tenant_id = ?",
                &[Value::String(key.to_string()), Value::I64(tenant_id)],
            )
            .await?;
        if affected == 0 {
            return Err(AdminError::ConfigNotFound);
        }
        Ok(())
    }

    pub async fn get_with_inheritance(
        &self,
        key: &str,
        tenant_id: i64,
    ) -> Result<Option<ConfigResponse>, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT id, `key`, value, value_type, `group`, description, tenant_id, created_at, updated_at \
                 FROM configs WHERE `key` = ? AND (tenant_id = ? OR tenant_id = 0) \
                 ORDER BY tenant_id DESC LIMIT 1",
                &[Value::String(key.to_string()), Value::I64(tenant_id)],
            )
            .await?;
        Ok(rows.first().and_then(row_to_config_response))
    }

    async fn get_by_key(&self, key: &str, tenant_id: i64) -> Result<ConfigResponse, AdminError> {
        let mut conn = self.pool.acquire().await?;
        let rows = conn
            .query_with_params(
                "SELECT id, `key`, value, value_type, `group`, description, tenant_id, created_at, updated_at \
                 FROM configs WHERE `key` = ? AND tenant_id = ? LIMIT 1",
                &[Value::String(key.to_string()), Value::I64(tenant_id)],
            )
            .await?;
        rows.first()
            .and_then(row_to_config_response)
            .ok_or(AdminError::ConfigNotFound)
    }
}

pub(crate) fn config_type_to_str(t: ConfigType) -> &'static str {
    match t {
        ConfigType::String => "string",
        ConfigType::Number => "number",
        ConfigType::Boolean => "boolean",
        ConfigType::Json => "json",
    }
}

fn str_to_config_type(s: &str) -> ConfigType {
    match s {
        "number" => ConfigType::Number,
        "boolean" => ConfigType::Boolean,
        "json" => ConfigType::Json,
        _ => ConfigType::String,
    }
}

fn row_to_config_response(
    row: &std::collections::HashMap<String, Value>,
) -> Option<ConfigResponse> {
    let id = row.get("id")?.as_i64()?;
    let key = row.get("key")?.as_str()?.to_string();
    let value_str = row.get("value")?.as_str()?;
    let value: serde_json::Value =
        serde_json::from_str(value_str).unwrap_or(serde_json::Value::Null);
    let value_type = row
        .get("value_type")
        .and_then(|v| v.as_str())
        .map(str_to_config_type)
        .unwrap_or(ConfigType::String);
    let group = row
        .get("group")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let description = row
        .get("description")
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

    Some(ConfigResponse {
        id,
        key,
        value,
        value_type,
        group,
        description,
        tenant_id,
        created_at,
        updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_config_request_deserialize() {
        let json =
            r#"{"value":true,"value_type":"boolean","group":"system","description":"启用标志"}"#;
        let req: UpsertConfigRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.value_type, ConfigType::Boolean);
        assert!(req.value.is_boolean());
    }

    #[test]
    fn test_config_type_to_str_roundtrip() {
        for t in [
            ConfigType::String,
            ConfigType::Number,
            ConfigType::Boolean,
            ConfigType::Json,
        ] {
            assert_eq!(str_to_config_type(config_type_to_str(t)), t);
        }
    }
}
