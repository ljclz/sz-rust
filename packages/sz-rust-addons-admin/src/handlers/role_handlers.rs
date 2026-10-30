// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use sz_rust_middleware_facade::data_scope::DataScopeUserContext;
use sz_rust_orm_facade::tenant::context::TenantContext;

use crate::error::AdminError;
use crate::handlers::user_handlers::log_operation;
use crate::models::operation_log::{OperationType, TargetType};
use crate::services::role_service::{
    AssignPermissionsRequest, CreateRoleRequest, RoleResponse, UpdateRoleRequest,
};
use crate::state::AdminAddonState;

pub async fn list_roles(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<Json<Vec<RoleResponse>>, AdminError> {
    let roles = state.role_service.list(ctx.tenant_id()).await?;
    Ok(Json(roles))
}

pub async fn create_role(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Json(req): Json<CreateRoleRequest>,
) -> Result<(StatusCode, Json<RoleResponse>), AdminError> {
    let role = state.role_service.create(req, ctx.tenant_id()).await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Create,
        TargetType::Role,
        Some(role.id),
    );
    Ok((StatusCode::CREATED, Json(role)))
}

pub async fn update_role(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(role_id): Path<i64>,
    Json(req): Json<UpdateRoleRequest>,
) -> Result<Json<RoleResponse>, AdminError> {
    let role = state
        .role_service
        .update(role_id, req, ctx.tenant_id())
        .await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Update,
        TargetType::Role,
        Some(role_id),
    );
    Ok(Json(role))
}

pub async fn delete_role(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(role_id): Path<i64>,
) -> Result<StatusCode, AdminError> {
    state.role_service.delete(role_id, ctx.tenant_id()).await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Delete,
        TargetType::Role,
        Some(role_id),
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn assign_role_permissions(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(role_id): Path<i64>,
    Json(req): Json<AssignPermissionsRequest>,
) -> Result<StatusCode, AdminError> {
    state
        .role_service
        .assign_permissions(role_id, req.permission_codes, ctx.tenant_id())
        .await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Update,
        TargetType::Role,
        Some(role_id),
    );
    Ok(StatusCode::OK)
}
