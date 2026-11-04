// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use axum::extract::{Query, State};
use axum::Extension;
use axum::Json;
use serde::Deserialize;

use sz_rust_orm_facade::tenant::context::TenantContext;

use crate::error::AdminError;
use crate::services::log_service::{LogFilter, OperationLogResponse};
use crate::state::AdminAddonState;

#[derive(Debug, Deserialize)]
pub struct LogListQuery {
    pub page: Option<i64>,
    pub size: Option<i64>,
    pub operator_id: Option<i64>,
    pub operation_type: Option<String>,
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list_operation_logs(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Query(q): Query<LogListQuery>,
) -> Result<Json<crate::PageResponse<OperationLogResponse>>, AdminError> {
    let filter = LogFilter {
        operator_id: q.operator_id,
        operation_type: q.operation_type.as_deref().and_then(parse_operation_type),
        start_time: q.start_time,
        end_time: q.end_time,
    };
    let page = q.page.unwrap_or(1);
    let size = q.size.unwrap_or(20);
    let result = state
        .log_service
        .list(filter, ctx.tenant_id(), page, size)
        .await?;
    Ok(Json(crate::PageResponse::from_page_result(result)))
}

fn parse_operation_type(s: &str) -> Option<crate::models::operation_log::OperationType> {
    use crate::models::operation_log::OperationType;
    match s {
        "create" => Some(OperationType::Create),
        "update" => Some(OperationType::Update),
        "delete" => Some(OperationType::Delete),
        "login" => Some(OperationType::Login),
        "logout" => Some(OperationType::Logout),
        _ => None,
    }
}
