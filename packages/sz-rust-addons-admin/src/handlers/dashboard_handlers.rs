// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team

use axum::extract::State;
use axum::Extension;
use axum::Json;
use sz_rust_orm_facade::tenant::context::TenantContext;

use crate::error::AdminError;
use crate::services::dashboard_service::DashboardStats;
use crate::state::AdminAddonState;

pub async fn get_dashboard(
    State(state): State<AdminAddonState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<Json<DashboardStats>, AdminError> {
    let stats = state.dashboard_service.stats(ctx.tenant_id()).await?;
    Ok(Json(stats))
}
