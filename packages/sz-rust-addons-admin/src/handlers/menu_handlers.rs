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
use crate::models::menu::MenuTreeNode;
use crate::models::operation_log::{OperationType, TargetType};
use crate::services::menu_service::{CreateMenuRequest, UpdateMenuRequest};
use crate::state::AdminAddonState;

pub async fn get_menu_tree(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
) -> Result<Json<Vec<MenuTreeNode>>, AdminError> {
    let user_permissions = state
        .permission_service
        .list_all(ctx.tenant_id())
        .await?
        .into_iter()
        .map(|(code, _, _)| code)
        .collect::<Vec<_>>();
    let _ = user_ctx;
    let tree = state
        .menu_service
        .tree(&user_permissions, ctx.tenant_id())
        .await?;
    Ok(Json(tree))
}

pub async fn create_menu(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Json(req): Json<CreateMenuRequest>,
) -> Result<(StatusCode, Json<crate::models::menu::MenuModel>), AdminError> {
    let menu = state.menu_service.create(req, ctx.tenant_id()).await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Create,
        TargetType::Menu,
        Some(menu.id),
    );
    Ok((StatusCode::CREATED, Json(menu)))
}

pub async fn update_menu(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(menu_id): Path<i64>,
    Json(req): Json<UpdateMenuRequest>,
) -> Result<Json<crate::models::menu::MenuModel>, AdminError> {
    let menu = state
        .menu_service
        .update(menu_id, req, ctx.tenant_id())
        .await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Update,
        TargetType::Menu,
        Some(menu_id),
    );
    Ok(Json(menu))
}

pub async fn delete_menu(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(menu_id): Path<i64>,
) -> Result<StatusCode, AdminError> {
    state.menu_service.delete(menu_id, ctx.tenant_id()).await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Delete,
        TargetType::Menu,
        Some(menu_id),
    );
    Ok(StatusCode::NO_CONTENT)
}
