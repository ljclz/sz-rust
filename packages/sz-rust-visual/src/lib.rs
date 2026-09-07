// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SZ-Rust 可视化画布
//!
//! Tauri 2.x 桌面应用，提供 SDD 编排可视化、Capability 管理、RAG 搜索、应用预览。

#![forbid(unsafe_code)]

pub mod commands;
pub mod error;
pub mod event_bridge;
pub mod models;
pub mod preview;
pub mod sdd_facade;

use std::sync::Arc;

use crate::sdd_facade::MockSddFacade;

/// 启动 Tauri 应用
pub fn run() -> error::VisualResult<()> {
    let sdd_facade: Arc<dyn sdd_facade::SddFacade> = Arc::new(MockSddFacade::new());

    tauri::Builder::default()
        .manage(sdd_facade)
        .invoke_handler(tauri::generate_handler![
            commands::sdd_start,
            commands::sdd_submit_review,
            commands::sdd_cancel,
            commands::sdd_status,
            commands::sdd_read_artifact,
            commands::cap_list,
            commands::cap_call,
            commands::rag_search,
            commands::preview_start,
            commands::preview_stop,
        ])
        .setup(|_app| {
            tracing::info!("SZ-Rust 画布已启动");
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(|e| error::VisualError::TauriError(e.to_string()))?;

    Ok(())
}
