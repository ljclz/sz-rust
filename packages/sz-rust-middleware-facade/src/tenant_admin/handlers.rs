// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 租户管理 API — 13 个 handler 实现

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};

use sz_rust_orm_facade::tenant::config::{ConfigSource, TenantConfigType};
use sz_rust_orm_facade::tenant::context::TenantContext;
use sz_rust_orm_facade::tenant::error::TenantError;
use sz_rust_orm_facade::tenant::record::{Tenant, TenantListFilter};
use sz_rust_orm_facade::tenant::scoped_table::TenantScopedTable;
use sz_rust_orm_facade::tenant::status::TenantStatus;

use crate::data_perm_admin::error_response::ApiErrorResponse;

use super::router::TenantAdminApiState;

#[derive(Debug, Deserialize)]
pub struct CreateTenantRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTenantRequest {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListTenantsQuery {
    pub status: Option<String>,
    pub page: Option<usize>,
    pub size: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct PagedTenantsResponse {
    pub tenants: Vec<Tenant>,
    pub total: usize,
    pub page: usize,
    pub size: usize,
}

#[derive(Debug, Deserialize)]
pub struct SetConfigRequest {
    pub config_value: String,
    pub config_type: TenantConfigType,
}

#[derive(Debug, Serialize)]
pub struct TenantConfigWithSource {
    pub tenant_id: i64,
    pub config_key: String,
    pub config_value: String,
    pub config_type: TenantConfigType,
    pub is_global: bool,
    pub source: ConfigSource,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

fn err_response(err: TenantError) -> Response {
    ApiErrorResponse::from_error(err.into(), 0).into_response()
}

fn parse_status(s: &str) -> Option<TenantStatus> {
    match s {
        "active" => Some(TenantStatus::Active),
        "suspended" => Some(TenantStatus::Suspended),
        "disabled" => Some(TenantStatus::Disabled),
        _ => None,
    }
}

// ==================== 租户 CRUD handler ====================

pub async fn create_tenant(
    State(state): State<TenantAdminApiState>,
    Json(req): Json<CreateTenantRequest>,
) -> Response {
    match state.manager.create_tenant(req.name).await {
        Ok(tenant) => (StatusCode::CREATED, Json(tenant)).into_response(),
        Err(err) => err_response(err),
    }
}

pub async fn list_tenants(
    State(state): State<TenantAdminApiState>,
    Query(query): Query<ListTenantsQuery>,
) -> Response {
    let size = query.size.unwrap_or(20).clamp(1, 100);
    let page = query.page.unwrap_or(1).max(1);
    let status = query.status.as_deref().and_then(parse_status);
    let filter = TenantListFilter { status };
    let all = state.manager.list_tenants(&filter);
    let total = all.len();
    let start = (page.saturating_sub(1)) * size;
    let tenants: Vec<Tenant> = if start >= total {
        Vec::new()
    } else {
        let end = (start + size).min(total);
        all[start..end].to_vec()
    };
    Json(PagedTenantsResponse {
        tenants,
        total,
        page,
        size,
    })
    .into_response()
}

pub async fn get_tenant(
    State(state): State<TenantAdminApiState>,
    Path(tenant_id): Path<i64>,
) -> Response {
    match state.manager.get_tenant(tenant_id) {
        Some(tenant) => Json(tenant).into_response(),
        None => err_response(TenantError::TenantNotFound(tenant_id)),
    }
}

pub async fn update_tenant(
    State(state): State<TenantAdminApiState>,
    Path(tenant_id): Path<i64>,
    Json(req): Json<UpdateTenantRequest>,
) -> Response {
    match state.manager.update_tenant(tenant_id, req.name).await {
        Ok(tenant) => Json(tenant).into_response(),
        Err(err) => err_response(err),
    }
}

pub async fn delete_tenant(
    State(state): State<TenantAdminApiState>,
    Path(tenant_id): Path<i64>,
) -> Response {
    match state.manager.delete_tenant(tenant_id).await {
        Ok(tenant) => Json(tenant).into_response(),
        Err(err) => err_response(err),
    }
}

pub async fn suspend_tenant(
    State(state): State<TenantAdminApiState>,
    Path(tenant_id): Path<i64>,
) -> Response {
    match state.manager.suspend_tenant(tenant_id).await {
        Ok(tenant) => Json(tenant).into_response(),
        Err(err) => err_response(err),
    }
}

pub async fn activate_tenant(
    State(state): State<TenantAdminApiState>,
    Path(tenant_id): Path<i64>,
) -> Response {
    match state.manager.activate_tenant(tenant_id).await {
        Ok(tenant) => Json(tenant).into_response(),
        Err(err) => err_response(err),
    }
}

pub async fn disable_tenant(
    State(state): State<TenantAdminApiState>,
    Path(tenant_id): Path<i64>,
) -> Response {
    match state.manager.disable_tenant(tenant_id).await {
        Ok(tenant) => Json(tenant).into_response(),
        Err(err) => err_response(err),
    }
}

// ==================== 租户配置 handler ====================

pub async fn list_tenant_configs(
    State(state): State<TenantAdminApiState>,
    Extension(ctx): Extension<TenantContext>,
) -> Response {
    let configs = state.manager.list_tenant_configs(ctx.tenant_id());
    let result: Vec<TenantConfigWithSource> = configs
        .into_iter()
        .map(|(config, source)| TenantConfigWithSource {
            tenant_id: config.tenant_id,
            config_key: config.config_key,
            config_value: config.config_value,
            config_type: config.config_type,
            is_global: config.is_global,
            source,
            updated_at: config.updated_at,
        })
        .collect();
    Json(serde_json::json!({ "configs": result, "total": result.len() })).into_response()
}

pub async fn set_tenant_config(
    State(state): State<TenantAdminApiState>,
    Extension(ctx): Extension<TenantContext>,
    Path(config_key): Path<String>,
    Json(req): Json<SetConfigRequest>,
) -> Response {
    match state
        .manager
        .set_tenant_config(
            ctx.tenant_id(),
            config_key,
            req.config_value,
            req.config_type,
        )
        .await
    {
        Ok(config) => Json(config).into_response(),
        Err(err) => err_response(err),
    }
}

pub async fn delete_tenant_config(
    State(state): State<TenantAdminApiState>,
    Extension(ctx): Extension<TenantContext>,
    Path(config_key): Path<String>,
) -> Response {
    match state
        .manager
        .delete_tenant_config(ctx.tenant_id(), &config_key)
        .await
    {
        Ok(config) => Json(config).into_response(),
        Err(err) => err_response(err),
    }
}

// ==================== 全局配置 handler ====================

pub async fn list_global_configs(State(state): State<TenantAdminApiState>) -> Response {
    Json(state.manager.list_global_configs()).into_response()
}

pub async fn set_global_config(
    State(state): State<TenantAdminApiState>,
    Path(config_key): Path<String>,
    Json(req): Json<SetConfigRequest>,
) -> Response {
    match state
        .manager
        .set_global_config(config_key, req.config_value, req.config_type)
        .await
    {
        Ok(config) => Json(config).into_response(),
        Err(err) => err_response(err),
    }
}

// ==================== 隔离表清单 handler ====================

pub async fn register_scoped_table(
    State(state): State<TenantAdminApiState>,
    Json(table): Json<TenantScopedTable>,
) -> Response {
    match state.manager.register_scoped_table(table).await {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(err) => err_response(err),
    }
}

pub async fn list_scoped_tables(State(state): State<TenantAdminApiState>) -> Response {
    Json(state.manager.list_scoped_tables()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use sz_rust_orm_facade::data_scope::ext::audit::TracingAuditLogger;
    use sz_rust_orm_facade::data_scope::ext::generation::PolicyGeneration;
    use sz_rust_orm_facade::data_scope::ext::notifier::ChangeNotifier;
    use sz_rust_orm_facade::data_scope::metrics::DataScopeMetrics;
    use sz_rust_orm_facade::tenant::config::TenantConfigRegistry;
    use sz_rust_orm_facade::tenant::context::TenantResolveSource;
    use sz_rust_orm_facade::tenant::ext::config_loader::TenantConfigLoader;
    use sz_rust_orm_facade::tenant::ext::hot_reload::TenantHotReloadManager;
    use sz_rust_orm_facade::tenant::record::TenantRecordRegistry;
    use sz_rust_orm_facade::tenant::scoped_table::TenantScopedTableRegistry;

    fn make_state() -> TenantAdminApiState {
        let manager = Arc::new(TenantHotReloadManager::new(
            Arc::new(TenantRecordRegistry::new()),
            Arc::new(TenantScopedTableRegistry::new()),
            Arc::new(TenantConfigRegistry::new()),
            PolicyGeneration::new(),
            ChangeNotifier::new(64),
            Arc::new(DataScopeMetrics::new()),
            Arc::new(TracingAuditLogger),
        ));
        let path_guard = sz_rust_orm_facade::data_scope::ext::path_guard::PathGuard::new(vec![
            std::env::current_dir().unwrap(),
            std::env::temp_dir(),
        ]);
        let config_loader = Arc::new(TenantConfigLoader::new(
            manager.clone(),
            path_guard,
            TenantConfigLoader::DEFAULT_MAX_FILE_SIZE,
        ));
        TenantAdminApiState {
            manager,
            config_loader,
            metrics: Arc::new(DataScopeMetrics::new()),
            tenant_admin_roles: vec!["tenant_admin".into()],
        }
    }

    #[tokio::test]
    async fn test_create_tenant_handler_success() {
        let state = make_state();
        let resp = create_tenant(
            State(state),
            Json(CreateTenantRequest {
                name: "acme".into(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_create_tenant_handler_duplicate_409() {
        let state = make_state();
        state.manager.create_tenant("acme".into()).await.unwrap();
        let resp = create_tenant(
            State(state),
            Json(CreateTenantRequest {
                name: "acme".into(),
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_list_tenants_handler_pagination() {
        let state = make_state();
        for i in 0..5 {
            state
                .manager
                .create_tenant(format!("tenant_{i}"))
                .await
                .unwrap();
        }
        let query = ListTenantsQuery {
            status: None,
            page: Some(1),
            size: Some(2),
        };
        let resp = list_tenants(State(state), Query(query)).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total"], 5);
        assert_eq!(json["page"], 1);
        assert_eq!(json["size"], 2);
        assert_eq!(json["tenants"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_get_tenant_handler_found() {
        let state = make_state();
        let t = state.manager.create_tenant("acme".into()).await.unwrap();
        let resp = get_tenant(State(state), Path(t.tenant_id)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_tenant_handler_not_found() {
        let state = make_state();
        let resp = get_tenant(State(state), Path(999)).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_suspend_tenant_handler_success() {
        let state = make_state();
        let t = state.manager.create_tenant("acme".into()).await.unwrap();
        let resp = suspend_tenant(State(state), Path(t.tenant_id)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_disable_tenant_handler_success() {
        let state = make_state();
        let t = state.manager.create_tenant("acme".into()).await.unwrap();
        let resp = disable_tenant(State(state), Path(t.tenant_id)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_delete_tenant_handler_not_disabled_400() {
        let state = make_state();
        let t = state.manager.create_tenant("acme".into()).await.unwrap();
        let resp = delete_tenant(State(state), Path(t.tenant_id)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_delete_tenant_handler_disabled_success() {
        let state = make_state();
        let t = state.manager.create_tenant("acme".into()).await.unwrap();
        state.manager.disable_tenant(t.tenant_id).await.unwrap();
        let resp = delete_tenant(State(state), Path(t.tenant_id)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_list_tenant_configs_handler() {
        let state = make_state();
        state
            .manager
            .set_tenant_config(1, "theme", "dark", TenantConfigType::String)
            .await
            .unwrap();
        let ctx = TenantContext::new(1, false, TenantResolveSource::Header);
        let resp = list_tenant_configs(State(state), Extension(ctx)).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total"], 1);
    }

    #[tokio::test]
    async fn test_set_tenant_config_handler_success() {
        let state = make_state();
        let ctx = TenantContext::new(1, false, TenantResolveSource::Header);
        let resp = set_tenant_config(
            State(state),
            Extension(ctx),
            Path("theme".to_string()),
            Json(SetConfigRequest {
                config_value: "dark".into(),
                config_type: TenantConfigType::String,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_list_global_configs_handler() {
        let state = make_state();
        state
            .manager
            .set_global_config("theme", "dark", TenantConfigType::String)
            .await
            .unwrap();
        let resp = list_global_configs(State(state)).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_set_global_config_handler_success() {
        let state = make_state();
        let resp = set_global_config(
            State(state),
            Path("theme".to_string()),
            Json(SetConfigRequest {
                config_value: "dark".into(),
                config_type: TenantConfigType::String,
            }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_register_scoped_table_handler_success() {
        let state = make_state();
        let resp =
            register_scoped_table(State(state), Json(TenantScopedTable::new("orders"))).await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_list_scoped_tables_handler() {
        let state = make_state();
        state
            .manager
            .register_scoped_table(TenantScopedTable::new("orders"))
            .await
            .unwrap();
        let resp = list_scoped_tables(State(state)).await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(!json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_invalid_status_transition_400() {
        let state = make_state();
        let t = state.manager.create_tenant("acme".into()).await.unwrap();
        state.manager.disable_tenant(t.tenant_id).await.unwrap();
        let resp = activate_tenant(State(state), Path(t.tenant_id)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
