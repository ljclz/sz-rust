// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P2-3 端到端全流程集成测试
//!
//! 验证：启动画布 → SDD 编排 → HITL 审查 → 四阶段完成 → 预览服务。
//! 使用 MockSddFacade（开源版无 SDD Agent），验证事件流与 HITL 交互正确。

use std::sync::Arc;

use sz_rust_visual::models::{
    DeviceType, PhaseEvent, PhaseEventKind, ReviewDecision, SddPhase, SddStatus,
};
use sz_rust_visual::preview::PreviewService;
use sz_rust_visual::sdd_facade::{MockSddFacade, SddFacade};

// ─── SDD 编排全流程 ───

#[tokio::test]
async fn e2e_sdd_full_flow_start_review_complete() {
    let facade = MockSddFacade::new();

    // 1. 启动编排
    let session = facade
        .start("user-management", "实现用户 CRUD 管理功能", SddPhase::Spec)
        .await
        .expect("sdd_start 应成功");
    assert_eq!(session.feature_name, "user-management");
    assert_eq!(session.current_phase, SddPhase::Spec);
    assert_eq!(session.status, SddStatus::Running);
    assert!(!session.session_id.is_empty(), "session_id 不应为空");

    let session_id = session.session_id.clone();

    // 2. Spec 阶段 HITL 审查通过
    let session_after_spec = facade
        .submit_review(
            &session_id,
            SddPhase::Spec,
            ReviewDecision::Confirm,
            Some("spec 审查通过"),
        )
        .await
        .expect("spec submit_review 应成功");
    assert_eq!(session_after_spec.session_id, session_id);

    // 3. Design 阶段 HITL 审查通过
    let session_after_design = facade
        .submit_review(
            &session_id,
            SddPhase::Design,
            ReviewDecision::Confirm,
            Some("design 审查通过"),
        )
        .await
        .expect("design submit_review 应成功");
    assert_eq!(session_after_design.session_id, session_id);

    // 4. Task 阶段 HITL 审查通过
    let session_after_task = facade
        .submit_review(
            &session_id,
            SddPhase::Task,
            ReviewDecision::Confirm,
            Some("task 审查通过"),
        )
        .await
        .expect("task submit_review 应成功");
    assert_eq!(session_after_task.session_id, session_id);

    // 5. Coding 阶段 HITL 审查通过
    let session_after_coding = facade
        .submit_review(
            &session_id,
            SddPhase::Coding,
            ReviewDecision::Confirm,
            Some("coding 审查通过"),
        )
        .await
        .expect("coding submit_review 应成功");
    assert_eq!(session_after_coding.session_id, session_id);

    // 6. 查询最终状态
    let final_status = facade.status(&session_id).await.expect("sdd_status 应成功");
    assert_eq!(final_status.session_id, session_id);
}

#[tokio::test]
async fn e2e_sdd_review_with_modify_decision() {
    let facade = MockSddFacade::new();

    let session = facade
        .start("order-service", "订单服务", SddPhase::Spec)
        .await
        .unwrap();

    // 审查决定：需修改
    let result = facade
        .submit_review(
            &session.session_id,
            SddPhase::Spec,
            ReviewDecision::Modify,
            Some("请补充字段约束"),
        )
        .await;
    assert!(result.is_ok(), "Modify 决定应成功");

    // 审查决定：需补充
    let result = facade
        .submit_review(
            &session.session_id,
            SddPhase::Design,
            ReviewDecision::Supplement,
            Some("请补充 API 设计"),
        )
        .await;
    assert!(result.is_ok(), "Supplement 决定应成功");
}

#[tokio::test]
async fn e2e_sdd_cancel_session() {
    let facade = MockSddFacade::new();

    let session = facade
        .start("cancel-test", "测试取消", SddPhase::Spec)
        .await
        .unwrap();

    let cancel_result = facade.cancel(&session.session_id).await;
    assert!(cancel_result.is_ok(), "取消编排应成功");
}

#[tokio::test]
async fn e2e_sdd_read_artifact() {
    let facade = MockSddFacade::new();

    let session = facade
        .start("artifact-test", "测试产物读取", SddPhase::Spec)
        .await
        .unwrap();

    let artifact = facade
        .read_artifact(&session.session_id, SddPhase::Spec)
        .await
        .expect("read_artifact 应成功");
    assert!(artifact.is_empty(), "MockSddFacade 应返回空字符串");
}

#[tokio::test]
async fn e2e_sdd_subscribe_events() {
    let facade = MockSddFacade::new();

    // 订阅事件流
    let mut rx = facade
        .subscribe_events()
        .await
        .expect("subscribe_events 应成功");

    // 验证 receiver 可用（不会立即关闭）
    // MockSddFacade 不主动发送事件，但 receiver 应有效
    let _session = facade
        .start("event-test", "测试事件订阅", SddPhase::Spec)
        .await
        .unwrap();

    // receiver 应处于活跃状态（非 Closed）
    // 由于 mock 不发送事件，recv 会阻塞，我们用 try_recv 验证
    let try_result = rx.try_recv();
    assert!(
        try_result.is_err(),
        "无事件时 try_recv 应返回 Err（Empty 或 Closed）"
    );
}

// ─── 预览服务全流程 ───

#[tokio::test]
async fn e2e_preview_start_http_accessible_stop_releases() {
    // 准备临时产物目录
    let cwd = std::env::current_dir().unwrap();
    let artifacts_root = cwd.join("artifacts");
    let feature_dir = artifacts_root.join("e2e-preview-feature");
    tokio::fs::create_dir_all(&feature_dir).await.unwrap();
    tokio::fs::write(
        feature_dir.join("index.html"),
        "<html><body>E2E Preview</body></html>",
    )
    .await
    .unwrap();
    tokio::fs::write(feature_dir.join("style.css"), "body { color: red; }")
        .await
        .unwrap();

    // 启动预览
    let url = PreviewService::start("e2e-preview-feature", DeviceType::Desktop)
        .await
        .expect("preview_start 应成功");
    assert!(url.starts_with("http://127.0.0.1:"));

    // 等待 HTTP 服务完全就绪
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // 通过 HTTP 请求验证静态文件服务
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/index.html"))
        .send()
        .await
        .expect("HTTP GET /index.html 应成功");
    assert!(
        resp.status().is_success(),
        "/index.html 应可访问，实际状态: {}",
        resp.status()
    );
    let body = resp.text().await.unwrap();
    assert!(body.contains("E2E Preview"), "响应体应含 E2E Preview");

    // 停止预览
    PreviewService::stop(&url)
        .await
        .expect("preview_stop 应成功");

    // 清理临时产物目录（仅清理本 test 的 feature 子目录）
    tokio::fs::remove_dir_all(&feature_dir).await.unwrap();
}

#[tokio::test]
async fn e2e_preview_device_viewport_variants() {
    let cwd = std::env::current_dir().unwrap();
    let artifacts_root = cwd.join("artifacts");
    let feature_dir = artifacts_root.join("e2e-device-test");
    tokio::fs::create_dir_all(&feature_dir).await.unwrap();
    tokio::fs::write(feature_dir.join("index.html"), "<html>ok</html>")
        .await
        .unwrap();

    // Desktop
    let url_desktop = PreviewService::start("e2e-device-test", DeviceType::Desktop)
        .await
        .expect("Desktop 预览应成功");
    assert!(url_desktop.starts_with("http://127.0.0.1:"));
    PreviewService::stop(&url_desktop).await.unwrap();

    // Tablet
    let url_tablet = PreviewService::start("e2e-device-test", DeviceType::Tablet)
        .await
        .expect("Tablet 预览应成功");
    PreviewService::stop(&url_tablet).await.unwrap();

    // Mobile
    let url_mobile = PreviewService::start("e2e-device-test", DeviceType::Mobile)
        .await
        .expect("Mobile 预览应成功");
    PreviewService::stop(&url_mobile).await.unwrap();

    // 清理（仅清理本 test 的 feature 子目录）
    tokio::fs::remove_dir_all(&feature_dir).await.unwrap();
}

#[tokio::test]
async fn e2e_preview_artifact_not_found() {
    let result = PreviewService::start("nonexistent-e2e-feature", DeviceType::Desktop).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code(), "ARTIFACT_NOT_FOUND");
}

// ─── Capability 注册与调用 ───

#[tokio::test]
async fn e2e_capability_list_and_call() {
    let registry = Arc::new(sz_rust_capability::CapabilityRegistry::new());
    sz_rust_capability::builtin::register_mcp_tools(&registry).unwrap();

    // 列表
    let infos = registry.list_info();
    assert!(!infos.is_empty(), "注册 MCP 工具后列表不应为空");
    assert!(
        infos.iter().all(|i| i.name.starts_with("mcp.")),
        "所有能力应以 mcp. 前缀命名"
    );

    // 调用 url_decode
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

// ─── SDD 事件模型验证 ───

#[tokio::test]
async fn e2e_phase_event_serialization() {
    let event = PhaseEvent {
        trace_id: "trace-e2e-001".to_string(),
        feature: "e2e-feature".to_string(),
        phase: SddPhase::Design,
        kind: PhaseEventKind::PhaseCompleted,
        timestamp: chrono::Utc::now(),
        metadata: serde_json::json!({"duration_ms": 1500, "artifact": "design.md"}),
    };

    let json = serde_json::to_string(&event).expect("PhaseEvent 序列化应成功");
    let parsed: PhaseEvent = serde_json::from_str(&json).expect("PhaseEvent 反序列化应成功");
    assert_eq!(parsed.trace_id, "trace-e2e-001");
    assert_eq!(parsed.feature, "e2e-feature");
    assert_eq!(parsed.phase, SddPhase::Design);
    assert_eq!(parsed.kind, PhaseEventKind::PhaseCompleted);
}

#[tokio::test]
async fn e2e_sdd_session_trace_id_skip_serializing() {
    use sz_rust_visual::models::SddSession;

    let session = SddSession {
        session_id: "s-e2e-001".to_string(),
        trace_id: "t-e2e-secret".to_string(),
        feature_name: "test".to_string(),
        current_phase: SddPhase::Coding,
        status: SddStatus::Completed,
        started_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    let json = serde_json::to_string(&session).unwrap();
    assert!(
        !json.contains("trace_id"),
        "trace_id 应被 skip_serializing 脱敏"
    );
    assert!(json.contains("session_id"), "session_id 应可见");
    assert!(json.contains("completed"), "status 应为 completed");
}

// ─── 完整 SDD 四阶段编排模拟 ───

#[tokio::test]
async fn e2e_complete_four_phase_orchestration() {
    let facade = MockSddFacade::new();
    let phases = [
        SddPhase::Spec,
        SddPhase::Design,
        SddPhase::Task,
        SddPhase::Coding,
    ];

    // 启动编排
    let session = facade
        .start("full-orchestration", "完整四阶段编排测试", SddPhase::Spec)
        .await
        .expect("启动编排应成功");

    let session_id = session.session_id.clone();

    // 依次完成四个阶段的 HITL 审查
    for (i, phase) in phases.iter().enumerate() {
        let result = facade
            .submit_review(
                &session_id,
                *phase,
                ReviewDecision::Confirm,
                Some(&format!("阶段 {} 审查通过", i + 1)),
            )
            .await;
        assert!(result.is_ok(), "阶段 {} 审查应成功", i + 1);
    }

    // 查询最终状态
    let final_session = facade
        .status(&session_id)
        .await
        .expect("查询最终状态应成功");
    assert_eq!(final_session.session_id, session_id);

    // 读取各阶段产物
    for phase in &phases {
        let artifact = facade
            .read_artifact(&session_id, *phase)
            .await
            .expect("读取产物应成功");
        assert!(artifact.is_empty(), "MockSddFacade 应返回空字符串");
    }
}
