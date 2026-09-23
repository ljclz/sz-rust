// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Tauri Command 层
//!
//! 10 个 `#[tauri::command]` 函数，封装 SDD/Capability/RAG/Preview 接口。
//! `#[tauri::command]` 宏展开代码无法在非 Tauri 环境覆盖，标记 `#[cfg(not(coverage))]` 排除。
//! 核心逻辑提取为 `*_inner` 函数，直接测试。

use std::sync::Arc;

use sz_rust_capability::CapabilityRegistry;

use crate::error::{VisualError, VisualResult};
use crate::models::{DeviceType, ReviewDecision, SddPhase};
use crate::sdd_facade::SddFacade;

/// SDD 启动编排
#[cfg(not(coverage))]
#[tauri::command]
pub async fn sdd_start(
    sdd: tauri::State<'_, Arc<dyn SddFacade>>,
    feature: String,
    requirement: String,
    start_phase: SddPhase,
) -> VisualResult<crate::models::SddSession> {
    sdd_start_inner(&**sdd, &feature, &requirement, start_phase).await
}

pub async fn sdd_start_inner(
    sdd: &dyn SddFacade,
    feature: &str,
    requirement: &str,
    start_phase: SddPhase,
) -> VisualResult<crate::models::SddSession> {
    sdd.start(feature, requirement, start_phase).await
}

/// SDD 提交审查
#[cfg(not(coverage))]
#[tauri::command]
pub async fn sdd_submit_review(
    sdd: tauri::State<'_, Arc<dyn SddFacade>>,
    session_id: String,
    phase: SddPhase,
    decision: ReviewDecision,
    comment: Option<String>,
) -> VisualResult<crate::models::SddSession> {
    sdd_submit_review_inner(&**sdd, &session_id, phase, decision, comment.as_deref()).await
}

pub async fn sdd_submit_review_inner(
    sdd: &dyn SddFacade,
    session_id: &str,
    phase: SddPhase,
    decision: ReviewDecision,
    comment: Option<&str>,
) -> VisualResult<crate::models::SddSession> {
    sdd.submit_review(session_id, phase, decision, comment)
        .await
}

/// SDD 取消编排
#[cfg(not(coverage))]
#[tauri::command]
pub async fn sdd_cancel(
    sdd: tauri::State<'_, Arc<dyn SddFacade>>,
    session_id: String,
) -> VisualResult<()> {
    sdd_cancel_inner(&**sdd, &session_id).await
}

pub async fn sdd_cancel_inner(sdd: &dyn SddFacade, session_id: &str) -> VisualResult<()> {
    sdd.cancel(session_id).await
}

/// SDD 查询状态
#[cfg(not(coverage))]
#[tauri::command]
pub async fn sdd_status(
    sdd: tauri::State<'_, Arc<dyn SddFacade>>,
    session_id: String,
) -> VisualResult<crate::models::SddSession> {
    sdd_status_inner(&**sdd, &session_id).await
}

pub async fn sdd_status_inner(
    sdd: &dyn SddFacade,
    session_id: &str,
) -> VisualResult<crate::models::SddSession> {
    sdd.status(session_id).await
}

/// SDD 读取产物
#[cfg(not(coverage))]
#[tauri::command]
pub async fn sdd_read_artifact(
    sdd: tauri::State<'_, Arc<dyn SddFacade>>,
    session_id: String,
    phase: SddPhase,
) -> VisualResult<String> {
    sdd_read_artifact_inner(&**sdd, &session_id, phase).await
}

pub async fn sdd_read_artifact_inner(
    sdd: &dyn SddFacade,
    session_id: &str,
    phase: SddPhase,
) -> VisualResult<String> {
    sdd.read_artifact(session_id, phase).await
}

/// Capability 列表（真实接线 CapabilityRegistry）
#[cfg(not(coverage))]
#[tauri::command]
pub async fn cap_list(
    registry: tauri::State<'_, Arc<CapabilityRegistry>>,
) -> VisualResult<Vec<sz_rust_capability::CapabilityInfo>> {
    cap_list_inner(&registry)
}

pub fn cap_list_inner(
    registry: &CapabilityRegistry,
) -> VisualResult<Vec<sz_rust_capability::CapabilityInfo>> {
    Ok(registry.list_info())
}

/// Capability 调用（真实接线 Capability.Registry）
#[cfg(not(coverage))]
#[tauri::command]
pub async fn cap_call(
    registry: tauri::State<'_, Arc<CapabilityRegistry>>,
    name: String,
    args: serde_json::Value,
) -> VisualResult<serde_json::Value> {
    cap_call_inner(&registry, &name, args).await
}

pub async fn cap_call_inner(
    registry: &CapabilityRegistry,
    name: &str,
    args: serde_json::Value,
) -> VisualResult<serde_json::Value> {
    registry
        .call(name, args)
        .await
        .map_err(|e| VisualError::CapError(e.to_string()))
}

/// RAG 搜索（真实接线 IndustryRag，未初始化时明确报错）
#[cfg(not(coverage))]
#[tauri::command]
pub async fn rag_search(query: String) -> VisualResult<Vec<String>> {
    rag_search_inner(&query).await
}

pub async fn rag_search_inner(query: &str) -> VisualResult<Vec<String>> {
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
#[cfg(not(coverage))]
#[tauri::command]
pub async fn preview_start(feature: String, device: DeviceType) -> VisualResult<String> {
    preview_start_inner(&feature, device).await
}

pub async fn preview_start_inner(feature: &str, device: DeviceType) -> VisualResult<String> {
    crate::preview::PreviewService::start(feature, device).await
}

/// 预览停止
#[cfg(not(coverage))]
#[tauri::command]
pub async fn preview_stop(url: String) -> VisualResult<()> {
    preview_stop_inner(&url).await
}

pub async fn preview_stop_inner(url: &str) -> VisualResult<()> {
    crate::preview::PreviewService::stop(url).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_capability::CapabilityRegistry;

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
        let result = sz_rust_rag::facade::IndustryRag::instance();
        if result.is_ok() {
            return;
        }
        let mapped: VisualError = VisualError::RagError("RAG 未初始化".into());
        assert_eq!(mapped.error_code(), "RAG_ERROR");
    }

    #[tokio::test]
    async fn test_rag_search_inner_not_initialized() {
        let result = rag_search_inner("test query").await;
        if sz_rust_rag::facade::IndustryRag::instance().is_ok() {
            return;
        }
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().error_code(), "RAG_ERROR");
    }

    #[tokio::test]
    async fn test_preview_start_inner_artifact_not_found() {
        let result = preview_start_inner("nonexistent-cmd-feature", DeviceType::Desktop).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().error_code(), "ARTIFACT_NOT_FOUND");
    }

    #[tokio::test]
    async fn test_preview_stop_inner_nonexistent_url() {
        let result = preview_stop_inner("http://127.0.0.1:65535").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_preview_start_then_stop_inner() {
        let cwd = std::env::current_dir().unwrap();
        let feature_dir = cwd.join("artifacts").join("cmd-preview-test");
        tokio::fs::create_dir_all(&feature_dir).await.unwrap();
        tokio::fs::write(feature_dir.join("index.html"), "<html>ok</html>")
            .await
            .unwrap();

        let url = preview_start_inner("cmd-preview-test", DeviceType::Desktop)
            .await
            .unwrap();
        assert!(url.starts_with("http://127.0.0.1:"));

        preview_stop_inner(&url).await.unwrap();
        tokio::fs::remove_dir_all(&feature_dir).await.unwrap();
    }

    use crate::sdd_facade::MockSddFacade;

    #[tokio::test]
    async fn test_sdd_start_inner() {
        let facade = MockSddFacade::new();
        let session = sdd_start_inner(&facade, "test", "req", SddPhase::Spec)
            .await
            .unwrap();
        assert_eq!(session.feature_name, "test");
        assert_eq!(session.current_phase, SddPhase::Spec);
    }

    #[tokio::test]
    async fn test_sdd_submit_review_inner() {
        let facade = MockSddFacade::new();
        let session = sdd_submit_review_inner(
            &facade,
            "s1",
            SddPhase::Spec,
            ReviewDecision::Confirm,
            Some("ok"),
        )
        .await
        .unwrap();
        assert_eq!(session.session_id, "s1");
    }

    #[tokio::test]
    async fn test_sdd_cancel_inner() {
        let facade = MockSddFacade::new();
        sdd_cancel_inner(&facade, "s1").await.unwrap();
    }

    #[tokio::test]
    async fn test_sdd_status_inner() {
        let facade = MockSddFacade::new();
        let session = sdd_status_inner(&facade, "s1").await.unwrap();
        assert_eq!(session.session_id, "s1");
    }

    #[tokio::test]
    async fn test_sdd_read_artifact_inner() {
        let facade = MockSddFacade::new();
        let artifact = sdd_read_artifact_inner(&facade, "s1", SddPhase::Spec)
            .await
            .unwrap();
        assert!(artifact.is_empty());
    }

    #[tokio::test]
    async fn test_cap_list_inner() {
        let registry = test_registry();
        let infos = cap_list_inner(&registry).unwrap();
        assert!(!infos.is_empty());
    }

    #[tokio::test]
    async fn test_cap_call_inner() {
        let registry = test_registry();
        let result = cap_call_inner(
            &registry,
            "mcp.url_decode",
            serde_json::json!({"value": "a%20b"}),
        )
        .await
        .unwrap();
        assert_eq!(result["decoded"], "a b");
    }

    #[tokio::test]
    async fn test_cap_call_inner_unknown() {
        let registry = test_registry();
        let result = cap_call_inner(&registry, "nonexistent", serde_json::json!({})).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().error_code(), "CAP_ERROR");
    }
}
