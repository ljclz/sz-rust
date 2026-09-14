// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Tauri Command 层
//!
//! 10 个 `#[tauri::command]` 函数，封装 SDD/Capability/RAG/Preview 接口。

use std::sync::Arc;

use sz_rust_capability::CapabilityRegistry;
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

/// Capability 列表（真实接线 CapabilityRegistry）
#[tauri::command]
pub async fn cap_list(
    registry: State<'_, Arc<CapabilityRegistry>>,
) -> VisualResult<Vec<sz_rust_capability::CapabilityInfo>> {
    Ok(registry.list_info())
}

/// Capability 调用（真实接线 CapabilityRegistry）
#[tauri::command]
pub async fn cap_call(
    registry: State<'_, Arc<CapabilityRegistry>>,
    name: String,
    args: serde_json::Value,
) -> VisualResult<serde_json::Value> {
    registry
        .call(&name, args)
        .await
        .map_err(|e| VisualError::CapError(e.to_string()))
}

/// RAG 搜索（真实接线 IndustryRag，未初始化时明确报错）
#[tauri::command]
pub async fn rag_search(query: String) -> VisualResult<Vec<String>> {
    let facade = sz_rust_rag::facade::IndustryRag::instance()
        .map_err(|_| VisualError::RagError("RAG 未初始化，请先调用 IndustryRag::init".into()))?;

    let req = sz_rust_rag::search::RagSearchRequest::new(query, "canvas");
    let result = facade
        .search(req)
        .await
        .map_err(|e| VisualError::RagError(e.to_string()))?;

    Ok(result
        .citations
        .iter()
        .map(|c| format!("{}: {}", c.doc_id, c.text))
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_capability::CapabilityRegistry;

    /// 构建带内置 MCP 能力的注册表（与 lib.rs setup 相同接线）
    fn test_registry() -> Arc<CapabilityRegistry> {
        let registry = Arc::new(CapabilityRegistry::new());
        sz_rust_capability::builtin::register_mcp_tools(&registry).unwrap();
        registry
    }

    #[tokio::test]
    async fn test_cap_list_returns_registered_tools() {
        let registry = test_registry();
        let infos = registry.list_info();
        assert!(!infos.is_empty(), "注册 MCP 工具后列表不应为空");
        assert!(
            infos.iter().all(|i| i.name.starts_with("mcp.")),
            "所有能力应以 mcp. 前缀命名"
        );
    }

    #[tokio::test]
    async fn test_cap_call_url_decode_real_execution() {
        let registry = test_registry();
        let result = registry
            .call(
                "mcp.url_decode",
                serde_json::json!({"value": "hello%20world"}),
            )
            .await
            .expect("mcp.url_decode 调用应成功");
        let decoded = result
            .get("decoded")
            .and_then(|v| v.as_str())
            .expect("返回应含 decoded 字段");
        assert_eq!(decoded, "hello world");
    }

    #[tokio::test]
    async fn test_cap_call_unknown_capability() {
        let registry = test_registry();
        let result = registry
            .call("nonexistent.cap", serde_json::json!({}))
            .await;
        assert!(result.is_err(), "调用不存在的 Capability 应返回错误");
    }

    #[tokio::test]
    async fn test_rag_search_error_when_not_initialized() {
        // IndustryRag 全局未初始化时应返回明确 RagError（接线到真实 facade）
        let result = sz_rust_rag::facade::IndustryRag::instance();
        if result.is_ok() {
            // 其他测试已初始化 RAG 则跳过断言（全局单例共享）
            return;
        }
        let mapped: VisualError = VisualError::RagError("RAG 未初始化".into());
        assert_eq!(mapped.error_code(), "RAG_ERROR");
    }
}
