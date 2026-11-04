// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 插件管理控制器（spec §5.10）
//!
//! 端点：
//! - `POST /api/v1/plugin/install` — 安装插件
//! - `POST /api/v1/plugin/uninstall` — 卸载插件
//! - `GET  /api/v1/plugin/list` — 列出已安装插件

use crate::services::plugin_manager::{InstalledPlugin, PluginError};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use serde_json::{json, Value};

/// 安装请求体
#[derive(Debug, Deserialize)]
pub struct InstallRequest {
    /// 插件 ID
    pub plugin_id: String,
}

/// 卸载请求体
#[derive(Debug, Deserialize)]
pub struct UninstallRequest {
    /// 插件 ID
    pub plugin_id: String,
}

/// 安装插件（spec §5.10.1 规则 2）
#[tracing::instrument(skip(state))]
pub async fn install(
    State(state): State<AppState>,
    Json(req): Json<InstallRequest>,
) -> impl IntoResponse {
    let mgr = state.plugin_manager.clone();
    match mgr.install(&req.plugin_id).await {
        Ok(record) => (
            StatusCode::OK,
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": record,
            })),
        ),
        Err(e) => plugin_error_response(e),
    }
}

/// 卸载插件（spec §5.10.1 规则 3）
#[tracing::instrument(skip(state))]
pub async fn uninstall(
    State(state): State<AppState>,
    Json(req): Json<UninstallRequest>,
) -> impl IntoResponse {
    let mgr = state.plugin_manager.clone();
    match mgr.uninstall(&req.plugin_id).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": null,
            })),
        ),
        Err(e) => plugin_error_response(e),
    }
}

/// 列出已安装插件（spec §5.10.1）
#[tracing::instrument(skip(state))]
pub async fn list(State(state): State<AppState>) -> Json<Value> {
    let mgr = state.plugin_manager.clone();
    let plugins: Vec<InstalledPlugin> = mgr.list().await;
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": plugins,
    }))
}

/// 将 PluginError 映射为 HTTP 响应
fn plugin_error_response(e: PluginError) -> (StatusCode, Json<Value>) {
    let (status, msg) = match &e {
        PluginError::MarketUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "插件市场不可用"),
        PluginError::SignatureInvalid => (StatusCode::BAD_REQUEST, "签名校验失败"),
        PluginError::NotFound(_) => (StatusCode::NOT_FOUND, "插件未找到"),
        PluginError::AlreadyInstalled(_) => (StatusCode::CONFLICT, "插件已安装"),
    };
    (status, Json(json!({ "code": 0, "msg": msg, "data": null })))
}
