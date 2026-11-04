// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use axum::extract::State;
use axum::Extension;
use axum::Json;
use sz_rust_orm_facade::tenant::context::TenantContext;

use crate::error::AdminError;
use crate::services::permission_service::PermissionTree;
use crate::state::AdminAddonState;

pub async fn get_permission_tree(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<Json<PermissionTree>, AdminError> {
    let tree = state.permission_service.tree(ctx.tenant_id()).await?;
    Ok(Json(tree))
}
