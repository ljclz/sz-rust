// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use sz_rust_middleware_facade::data_scope::DataScopeUserContext;
use sz_rust_orm_facade::tenant::context::TenantContext;

use crate::error::AdminError;
use crate::models::operation_log::{OperationLogModel, OperationType, TargetType};
use crate::services::user_service::{
    AssignRolesRequest, CreateUserRequest, UpdateUserRequest, UpdateUserStatusRequest,
    UserListFilter, UserResponse,
};
use crate::state::AdminAddonState;

#[derive(Debug, Deserialize)]
pub struct UserListQuery {
    pub page: Option<i64>,
    pub size: Option<i64>,
    pub username: Option<String>,
    pub status: Option<String>,
}

pub async fn list_users(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Query(q): Query<UserListQuery>,
) -> Result<Json<crate::PageResponse<UserResponse>>, AdminError> {
    let filter = UserListFilter {
        username: q.username,
        status: q.status.as_deref().and_then(parse_status),
    };
    let page = q.page.unwrap_or(1);
    let size = q.size.unwrap_or(20);
    let result = state
        .user_service
        .list(filter, ctx.tenant_id(), page, size)
        .await?;
    Ok(Json(crate::PageResponse::from_page_result(result)))
}

pub async fn create_user(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Json(req): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserResponse>), AdminError> {
    let user = state.user_service.create(req, ctx.tenant_id()).await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Create,
        TargetType::User,
        Some(user.id),
    );
    Ok((StatusCode::CREATED, Json(user)))
}

pub async fn update_user(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(user_id): Path<i64>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>, AdminError> {
    let user = state
        .user_service
        .update(user_id, req, ctx.tenant_id())
        .await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Update,
        TargetType::User,
        Some(user_id),
    );
    Ok(Json(user))
}

pub async fn update_user_status(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(user_id): Path<i64>,
    Json(req): Json<UpdateUserStatusRequest>,
) -> Result<StatusCode, AdminError> {
    state
        .user_service
        .update_status(user_id, req.status, ctx.tenant_id())
        .await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Update,
        TargetType::User,
        Some(user_id),
    );
    Ok(StatusCode::OK)
}

pub async fn delete_user(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(user_id): Path<i64>,
) -> Result<StatusCode, AdminError> {
    state.user_service.delete(user_id, ctx.tenant_id()).await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Delete,
        TargetType::User,
        Some(user_id),
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn assign_user_roles(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(user_id): Path<i64>,
    Json(req): Json<AssignRolesRequest>,
) -> Result<StatusCode, AdminError> {
    state
        .user_service
        .assign_roles(user_id, req.role_ids, ctx.tenant_id())
        .await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Update,
        TargetType::User,
        Some(user_id),
    );
    Ok(StatusCode::OK)
}

fn parse_status(s: &str) -> Option<crate::models::user::UserStatus> {
    use crate::models::user::UserStatus;
    match s {
        "active" => Some(UserStatus::Active),
        "disabled" => Some(UserStatus::Disabled),
        "locked" => Some(UserStatus::Locked),
        _ => None,
    }
}

pub(crate) fn log_operation(
    state: &AdminAddonState,
    operator_id: i64,
    operator_name: String,
    tenant_id: i64,
    op_type: OperationType,
    target_type: TargetType,
    target_id: Option<i64>,
) {
    let log = OperationLogModel {
        id: 0,
        operator_id,
        operator_name,
        operation_type: op_type,
        target_type,
        target_id,
        detail: None,
        ip: None,
        tenant_id,
        created_at: Utc::now(),
    };
    state.log_service.log_async(log);
}
