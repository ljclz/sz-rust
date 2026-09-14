// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sz_rust_orm_facade::repository::PageResult;
use sz_rust_orm_facade::{Pool, Value};

use crate::error::AdminError;
use crate::models::operation_log::{OperationLogModel, OperationType, TargetType};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LogFilter {
    pub operator_id: Option<i64>,
    pub operation_type: Option<OperationType>,
    pub start_time: Option<chrono::DateTime<Utc>>,
    pub end_time: Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OperationLogResponse {
    pub id: i64,
    pub operator_id: i64,
    pub operator_name: String,
    pub operation_type: OperationType,
    pub target_type: TargetType,
    pub target_id: Option<i64>,
    pub detail: Option<serde_json::Value>,
    pub ip: Option<String>,
    pub tenant_id: i64,
    pub created_at: chrono::DateTime<Utc>,
}

impl From<OperationLogModel> for OperationLogResponse {
    fn from(l: OperationLogModel) -> Self {
        Self {
            id: l.id,
            operator_id: l.operator_id,
            operator_name: l.operator_name,
            operation_type: l.operation_type,
            target_type: l.target_type,
            target_id: l.target_id,
            detail: l.detail,
            ip: l.ip,
            tenant_id: l.tenant_id,
            created_at: l.created_at,
        }
    }
}

#[derive(Clone)]
pub struct OperationLogService {
    pool: Arc<Pool>,
}

impl OperationLogService {
    pub fn new(pool: Arc<Pool>) -> Self {
        Self { pool }
    }

    pub fn log_async(&self, log: OperationLogModel) {
        let pool = self.pool.clone();
        tokio::spawn(async move {
            if let Err(e) = Self::write_log(&pool, &log).await {
                tracing::warn!(
                    target: "admin_addon",
                    error = %e,
                    "操作日志写入降级"
                );
            }
        });
    }

    async fn write_log(pool: &Arc<Pool>, log: &OperationLogModel) -> Result<(), AdminError> {
        let mut conn = pool.acquire().await?;
        let detail_str = log
            .detail
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default());
        let op_type_str = operation_type_to_str(log.operation_type);
        let target_type_str = target_type_to_str(log.target_type);

        conn.execute_with_params(
            "INSERT INTO operation_logs (operator_id, operator_name, operation_type, target_type, target_id, detail, ip, tenant_id, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[
                Value::I64(log.operator_id),
                Value::String(log.operator_name.clone()),
                Value::String(op_type_str.to_string()),
                Value::String(target_type_str.to_string()),
                log.target_id.map_or(Value::Null, Value::I64),
                detail_str.map_or(Value::Null, Value::String),
                log.ip.as_ref().map_or(Value::Null, |v| Value::String(v.clone())),
                Value::I64(log.tenant_id),
                Value::DateTime(log.created_at.to_rfc3339()),
            ],
        )
        .await?;
        Ok(())
    }

    pub async fn list(
        &self,
        filter: LogFilter,
        tenant_id: i64,
        page: i64,
        size: i64,
    ) -> Result<PageResult<OperationLogResponse>, AdminError> {
        if let (Some(start), Some(end)) = (filter.start_time, filter.end_time) {
            if start > end {
                return Err(AdminError::InvalidTimeRange);
            }
        }

        let page = page.max(1);
        let size = size.clamp(1, 100);
        let offset = (page - 1) * size;

        let mut sql = String::from(
            "SELECT id, operator_id, operator_name, operation_type, target_type, target_id, detail, ip, tenant_id, created_at \
             FROM operation_logs WHERE tenant_id = ?",
        );
        let mut params: Vec<Value> = vec![Value::I64(tenant_id)];

        if let Some(op_id) = filter.operator_id {
            sql.push_str(" AND operator_id = ?");
            params.push(Value::I64(op_id));
        }
        if let Some(op_type) = filter.operation_type {
            sql.push_str(" AND operation_type = ?");
            params.push(Value::String(operation_type_to_str(op_type).to_string()));
        }
        if let Some(ref start) = filter.start_time {
            sql.push_str(" AND created_at >= ?");
            params.push(Value::DateTime(start.to_rfc3339()));
        }
        if let Some(ref end) = filter.end_time {
            sql.push_str(" AND created_at <= ?");
            params.push(Value::DateTime(end.to_rfc3339()));
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
        let items: Vec<OperationLogResponse> =
            rows.iter().filter_map(row_to_log_response).collect();

        Ok(PageResult::new(items, total, page as u64, size as u64))
    }

    pub async fn cleanup_expired(&self, retention_days: i64) -> Result<u64, AdminError> {
        let cutoff = Utc::now() - chrono::Duration::days(retention_days);
        let mut conn = self.pool.acquire().await?;
        let affected = conn
            .execute_with_params(
                "DELETE FROM operation_logs WHERE created_at < ?",
                &[Value::DateTime(cutoff.to_rfc3339())],
            )
            .await?;
        Ok(affected)
    }
}

pub(crate) fn operation_type_to_str(t: OperationType) -> &'static str {
    match t {
        OperationType::Create => "create",
        OperationType::Update => "update",
        OperationType::Delete => "delete",
        OperationType::Login => "login",
        OperationType::Logout => "logout",
    }
}

pub(crate) fn str_to_operation_type(s: &str) -> OperationType {
    match s {
        "update" => OperationType::Update,
        "delete" => OperationType::Delete,
        "login" => OperationType::Login,
        "logout" => OperationType::Logout,
        _ => OperationType::Create,
    }
}

pub(crate) fn target_type_to_str(t: TargetType) -> &'static str {
    match t {
        TargetType::User => "user",
        TargetType::Role => "role",
        TargetType::Permission => "permission",
        TargetType::Menu => "menu",
        TargetType::Config => "config",
    }
}

pub(crate) fn str_to_target_type(s: &str) -> TargetType {
    match s {
        "role" => TargetType::Role,
        "permission" => TargetType::Permission,
        "menu" => TargetType::Menu,
        "config" => TargetType::Config,
        _ => TargetType::User,
    }
}

fn row_to_log_response(
    row: &std::collections::HashMap<String, Value>,
) -> Option<OperationLogResponse> {
    let id = row.get("id")?.as_i64()?;
    let operator_id = row.get("operator_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let operator_name = row
        .get("operator_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let operation_type = row
        .get("operation_type")
        .and_then(|v| v.as_str())
        .map(str_to_operation_type)
        .unwrap_or(OperationType::Create);
    let target_type = row
        .get("target_type")
        .and_then(|v| v.as_str())
        .map(str_to_target_type)
        .unwrap_or(TargetType::User);
    let target_id = row.get("target_id").and_then(|v| v.as_i64());
    let detail = row
        .get("detail")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str(s).ok());
    let ip = row
        .get("ip")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let tenant_id = row.get("tenant_id").and_then(|v| v.as_i64()).unwrap_or(0);
    let created_at = row
        .get("created_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    Some(OperationLogResponse {
        id,
        operator_id,
        operator_name,
        operation_type,
        target_type,
        target_id,
        detail,
        ip,
        tenant_id,
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_filter_default() {
        let filter = LogFilter::default();
        assert!(filter.operator_id.is_none());
        assert!(filter.start_time.is_none());
    }

    #[test]
    fn test_operation_type_roundtrip() {
        for t in [
            OperationType::Create,
            OperationType::Update,
            OperationType::Delete,
            OperationType::Login,
            OperationType::Logout,
        ] {
            assert_eq!(str_to_operation_type(operation_type_to_str(t)), t);
        }
    }

    #[test]
    fn test_target_type_roundtrip() {
        for t in [
            TargetType::User,
            TargetType::Role,
            TargetType::Permission,
            TargetType::Menu,
            TargetType::Config,
        ] {
            assert_eq!(str_to_target_type(target_type_to_str(t)), t);
        }
    }

    #[test]
    fn test_invalid_time_range_detection() {
        let start = Utc::now();
        let end = start - chrono::Duration::hours(1);
        assert!(start > end);
    }
}
