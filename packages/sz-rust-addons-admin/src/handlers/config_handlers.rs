// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Extension;
use axum::Json;
use serde::Deserialize;
use sz_rust_middleware_facade::data_scope::DataScopeUserContext;
use sz_rust_orm_facade::tenant::context::TenantContext;

use crate::error::AdminError;
use crate::handlers::user_handlers::log_operation;
use crate::models::operation_log::{OperationType, TargetType};
use crate::services::config_service::{ConfigResponse, UpsertConfigRequest};
use crate::state::AdminAddonState;

#[derive(Debug, Deserialize)]
pub struct ConfigListQuery {
    pub group: Option<String>,
}

pub async fn list_configs(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Query(q): Query<ConfigListQuery>,
) -> Result<Json<Vec<ConfigResponse>>, AdminError> {
    let configs = state.config_service.list(q.group, ctx.tenant_id()).await?;
    Ok(Json(configs))
}

pub async fn upsert_config(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(config_key): Path<String>,
    Json(req): Json<UpsertConfigRequest>,
) -> Result<Json<ConfigResponse>, AdminError> {
    let config = state
        .config_service
        .upsert(config_key.clone(), req, ctx.tenant_id())
        .await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Update,
        TargetType::Config,
        Some(config.id),
    );
    Ok(Json(config))
}

pub async fn delete_config(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(user_ctx): Extension<DataScopeUserContext>,
    Path(config_key): Path<String>,
) -> Result<StatusCode, AdminError> {
    state
        .config_service
        .delete(&config_key, ctx.tenant_id())
        .await?;
    log_operation(
        &state,
        user_ctx.user_id,
        user_ctx.user_id.to_string(),
        ctx.tenant_id(),
        OperationType::Delete,
        TargetType::Config,
        None,
    );
    Ok(StatusCode::NO_CONTENT)
}
