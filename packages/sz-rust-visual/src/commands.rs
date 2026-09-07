// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Tauri Command 层
//!
//! 10 个 `#[tauri::command]` 函数，封装 SDD/Capability/RAG/Preview 接口。

use std::sync::Arc;

use tauri::State;

use crate::error::{VisualError, VisualResult};
use crate::models::{DeviceType, ReviewDecision, SddPhase};
use crate::sdd_facade::SddFacade;

/// SDD 启动编排
#[tauri::command]
pub async fn sdd_start(
    sdd: State<'_, Arc<dyn SddFacade>>,
    feature: String,
    requirement: String,
    start_phase: SddPhase,
) -> VisualResult<crate::models::SddSession> {
    sdd.start(&feature, &requirement, start_phase).await
}

/// SDD 提交审查
#[tauri::command]
pub async fn sdd_submit_review(
    sdd: State<'_, Arc<dyn SddFacade>>,
    session_id: String,
    phase: SddPhase,
    decision: ReviewDecision,
    comment: Option<String>,
) -> VisualResult<crate::models::SddSession> {
    sdd.submit_review(&session_id, phase, decision, comment.as_deref())
        .await
}

/// SDD 取消编排
#[tauri::command]
pub async fn sdd_cancel(
    sdd: State<'_, Arc<dyn SddFacade>>,
    session_id: String,
) -> VisualResult<()> {
    sdd.cancel(&session_id).await
}

/// SDD 查询状态
#[tauri::command]
pub async fn sdd_status(
    sdd: State<'_, Arc<dyn SddFacade>>,
    session_id: String,
) -> VisualResult<crate::models::SddSession> {
    sdd.status(&session_id).await
}

/// SDD 读取产物
#[tauri::command]
pub async fn sdd_read_artifact(
    sdd: State<'_, Arc<dyn SddFacade>>,
    session_id: String,
    phase: SddPhase,
) -> VisualResult<String> {
    sdd.read_artifact(&session_id, phase).await
}

/// Capability 列表
#[tauri::command]
pub async fn cap_list() -> VisualResult<Vec<String>> {
    Ok(vec![])
}

/// Capability 调用
#[tauri::command]
pub async fn cap_call(name: String, _args: serde_json::Value) -> VisualResult<serde_json::Value> {
    Err(VisualError::CapError(format!(
        "Capability {name} 未实现（开源版占位）"
    )))
}

/// RAG 搜索
#[tauri::command]
pub async fn rag_search(query: String) -> VisualResult<Vec<String>> {
    tracing::info!("rag_search: {query}（开源版占位）");
    Ok(vec![])
}

/// 预览启动
#[tauri::command]
pub async fn preview_start(feature: String, device: DeviceType) -> VisualResult<String> {
    crate::preview::PreviewService::start(&feature, device).await
}

/// 预览停止
#[tauri::command]
pub async fn preview_stop(url: String) -> VisualResult<()> {
    crate::preview::PreviewService::stop(&url).await
}
