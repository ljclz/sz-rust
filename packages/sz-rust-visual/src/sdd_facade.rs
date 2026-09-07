// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SDD Facade trait
//!
//! 与 SDD Agent 解耦的异步 trait 接口，开源版使用 mock 实现，企业版路由到 Orchestrator。

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::error::VisualResult;
use crate::models::{PhaseEvent, ReviewDecision, SddPhase, SddSession, SddStatus};

/// SDD 编排 facade
///
/// 开源版提供 mock 实现，企业版通过 `SddFacadeImpl` 路由到 `Orchestrator`。
#[async_trait]
pub trait SddFacade: Send + Sync {
    /// 启动编排
    ///
    /// 创建新 SDD 会话，从指定阶段开始编排。
    async fn start(
        &self,
        feature: &str,
        requirement: &str,
        start_phase: SddPhase,
    ) -> VisualResult<SddSession>;

    /// 提交 HITL 审查决定
    async fn submit_review(
        &self,
        session_id: &str,
        phase: SddPhase,
        decision: ReviewDecision,
        comment: Option<&str>,
    ) -> VisualResult<SddSession>;

    /// 取消编排
    async fn cancel(&self, session_id: &str) -> VisualResult<()>;

    /// 查询编排状态
    async fn status(&self, session_id: &str) -> VisualResult<SddSession>;

    /// 订阅事件流
    ///
    /// 返回 broadcast receiver，消费 `PhaseEvent` 事件。
    async fn subscribe_events(&self) -> VisualResult<broadcast::Receiver<PhaseEvent>>;

    /// 读取产物
    async fn read_artifact(&self, session_id: &str, phase: SddPhase) -> VisualResult<String>;
}

/// Mock SDD Facade（开源版默认实现）
pub struct MockSddFacade {
    events: broadcast::Sender<PhaseEvent>,
}

impl MockSddFacade {
    /// 创建 mock facade
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self { events: tx }
    }
}

impl Default for MockSddFacade {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SddFacade for MockSddFacade {
    async fn start(
        &self,
        feature: &str,
        _requirement: &str,
        start_phase: SddPhase,
    ) -> VisualResult<SddSession> {
        let session = SddSession {
            session_id: uuid::Uuid::new_v4().to_string(),
            trace_id: uuid::Uuid::new_v4().to_string(),
            feature_name: feature.to_string(),
            current_phase: start_phase,
            status: SddStatus::Running,
            started_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        Ok(session)
    }

    async fn submit_review(
        &self,
        session_id: &str,
        _phase: SddPhase,
        _decision: ReviewDecision,
        _comment: Option<&str>,
    ) -> VisualResult<SddSession> {
        Ok(SddSession {
            session_id: session_id.to_string(),
            trace_id: String::new(),
            feature_name: String::new(),
            current_phase: SddPhase::Spec,
            status: SddStatus::Running,
            started_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
    }

    async fn cancel(&self, _session_id: &str) -> VisualResult<()> {
        Ok(())
    }

    async fn status(&self, session_id: &str) -> VisualResult<SddSession> {
        Ok(SddSession {
            session_id: session_id.to_string(),
            trace_id: String::new(),
            feature_name: String::new(),
            current_phase: SddPhase::Spec,
            status: SddStatus::Running,
            started_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        })
    }

    async fn subscribe_events(&self) -> VisualResult<broadcast::Receiver<PhaseEvent>> {
        Ok(self.events.subscribe())
    }

    async fn read_artifact(&self, _session_id: &str, _phase: SddPhase) -> VisualResult<String> {
        Ok(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_start() {
        let facade = MockSddFacade::new();
        let session = facade
            .start("test-feature", "test requirement", SddPhase::Spec)
            .await
            .unwrap();
        assert_eq!(session.feature_name, "test-feature");
        assert_eq!(session.current_phase, SddPhase::Spec);
        assert_eq!(session.status, SddStatus::Running);
    }

    #[tokio::test]
    async fn test_mock_cancel() {
        let facade = MockSddFacade::new();
        let result = facade.cancel("session-1").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_subscribe_events() {
        let facade = MockSddFacade::new();
        let _rx = facade.subscribe_events().await.unwrap();
    }

    #[tokio::test]
    async fn test_mock_read_artifact() {
        let facade = MockSddFacade::new();
        let artifact = facade
            .read_artifact("session-1", SddPhase::Spec)
            .await
            .unwrap();
        assert!(artifact.is_empty());
    }
}
