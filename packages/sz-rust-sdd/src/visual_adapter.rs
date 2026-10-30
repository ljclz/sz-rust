// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SDD Visual Adapter — 将 Orchestrator 适配为 SddFacade trait
//!
//! 企业版通过此模块将 SDD Agent 的 Orchestrator 桥接到画布的 SddFacade 接口，
//! 实现六方法路由 + 事件流 broadcast 桥接。

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use sz_rust_visual::error::{VisualError, VisualResult};
use sz_rust_visual::models::{PhaseEvent, ReviewDecision as VisualReviewDecision, SddPhase, SddSession, SddStatus};
use sz_rust_visual::sdd_facade::SddFacade;
use tokio::sync::broadcast;

use crate::error::{SddError, SddResult};
use crate::model::{
    AiConfig, ReviewDecision as SddReviewDecision, SessionId, SessionState, SessionToolWhitelist,
    Stage,
};
use crate::orchestrator::Orchestrator;

/// SddFacade 企业版实现，持有 Orchestrator 句柄 + 事件 broadcast
pub struct SddFacadeImpl {
    orchestrator: Arc<Orchestrator>,
    events: broadcast::Sender<PhaseEvent>,
    default_target_crate: PathBuf,
    default_ai_config: AiConfig,
    default_whitelist: SessionToolWhitelist,
}

impl SddFacadeImpl {
    /// 创建适配器
    pub fn new(orchestrator: Arc<Orchestrator>) -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self {
            orchestrator,
            events: tx,
            default_target_crate: PathBuf::from("."),
            default_ai_config: AiConfig::default(),
            default_whitelist: SessionToolWhitelist::default(),
        }
    }

    /// 自定义默认参数
    pub fn with_defaults(
        orchestrator: Arc<Orchestrator>,
        target_crate: PathBuf,
        ai_config: AiConfig,
        whitelist: SessionToolWhitelist,
    ) -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self {
            orchestrator,
            events: tx,
            default_target_crate: target_crate,
            default_ai_config: ai_config,
            default_whitelist: whitelist,
        }
    }

    /// SddPhase → Stage 映射
    fn phase_to_stage(phase: SddPhase) -> Stage {
        match phase {
            SddPhase::Requirement => Stage::Spec,
            SddPhase::Spec => Stage::Spec,
            SddPhase::Design => Stage::Design,
            SddPhase::Task => Stage::Task,
        }
    }

    /// Stage → SddPhase 映射
    fn stage_to_phase(stage: &Stage) -> SddPhase {
        match stage {
            Stage::Spec => SddPhase::Spec,
            Stage::Design => SddPhase::Design,
            Stage::Task => SddPhase::Task,
            Stage::Coding => SddPhase::Task,
            Stage::Completed => SddPhase::Task,
            Stage::Failed => SddPhase::Task,
        }
    }

    /// VisualReviewDecision → SddReviewDecision 映射
    fn map_review_decision(
        decision: VisualReviewDecision,
        comment: Option<&str>,
    ) -> SddReviewDecision {
        match decision {
            VisualReviewDecision::Approve => SddReviewDecision::Pass,
            VisualReviewDecision::Reject => SddReviewDecision::Redo {
                comment: comment.unwrap_or("rejected").to_string(),
            },
        }
    }

    /// SessionState → SddSession 映射
    fn map_session(state: &SessionState) -> SddSession {
        SddSession {
            session_id: state.id.value.clone(),
            trace_id: state.id.value.clone(),
            feature_name: state
                .target_crate
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            current_phase: Self::stage_to_phase(&state.current_stage),
            status: Self::map_status(&state.review_status),
            started_at: state.created_at,
            updated_at: state.created_at,
        }
    }

    /// ReviewStatus → SddStatus 映射
    fn map_status(review_status: &crate::model::ReviewStatus) -> SddStatus {
        use crate::model::ReviewStatus;
        match review_status {
            ReviewStatus::WaitingReview => SddStatus::AwaitingReview,
            ReviewStatus::Approved => SddStatus::Running,
            ReviewStatus::Rejected => SddStatus::Failed,
        }
    }

    /// SddError → VisualError 映射
    fn map_error(e: SddError) -> VisualError {
        VisualError::SddError(e.to_string())
    }
}

#[async_trait]
impl SddFacade for SddFacadeImpl {
    async fn start(
        &self,
        feature: &str,
        requirement: &str,
        start_phase: SddPhase,
    ) -> VisualResult<SddSession> {
        let _ = feature;

        let session_id = self
            .orchestrator
            .init_session(
                requirement.to_string(),
                self.default_target_crate.clone(),
                self.default_ai_config.clone(),
                self.default_whitelist.clone(),
            )
            .await
            .map_err(Self::map_error)?;

        let _ = self.events.send(PhaseEvent {
            session_id: session_id.value.clone(),
            phase: start_phase,
            event_type: "phase_start".to_string(),
            message: format!("编排启动: {feature}"),
            timestamp: chrono::Utc::now(),
        });

        let state = self
            .orchestrator
            .resume_session(&session_id)
            .await
            .map_err(Self::map_error)?;

        Ok(Self::map_session(&state))
    }

    async fn submit_review(
        &self,
        session_id: &str,
        _phase: SddPhase,
        decision: VisualReviewDecision,
        comment: Option<&str>,
    ) -> VisualResult<SddSession> {
        let sid = SessionId {
            value: session_id.to_string(),
        };
        let sdd_decision = Self::map_review_decision(decision, comment);

        let outcome = self
            .orchestrator
            .submit_review(&sid, sdd_decision)
            .await
            .map_err(Self::map_error)?;

        let _ = self.events.send(PhaseEvent {
            session_id: session_id.to_string(),
            phase: _phase,
            event_type: "review_submitted".to_string(),
            message: format!("审查结果: {:?}", outcome),
            timestamp: chrono::Utc::now(),
        });

        let state = self
            .orchestrator
            .resume_session(&sid)
            .await
            .map_err(Self::map_error)?;

        Ok(Self::map_session(&state))
    }

    async fn cancel(&self, session_id: &str) -> VisualResult<()> {
        self.orchestrator
            .session_manager()
            .release_session();

        let _ = self.events.send(PhaseEvent {
            session_id: session_id.to_string(),
            phase: SddPhase::Requirement,
            event_type: "cancelled".to_string(),
            message: "编排已取消".to_string(),
            timestamp: chrono::Utc::now(),
        });

        Ok(())
    }

    async fn status(&self, session_id: &str) -> VisualResult<SddSession> {
        let sid = SessionId {
            value: session_id.to_string(),
        };
        let state = self
            .orchestrator
            .resume_session(&sid)
            .await
            .map_err(Self::map_error)?;
        Ok(Self::map_session(&state))
    }

    async fn subscribe_events(&self) -> VisualResult<broadcast::Receiver<PhaseEvent>> {
        Ok(self.events.subscribe())
    }

    async fn read_artifact(&self, session_id: &str, phase: SddPhase) -> VisualResult<String> {
        let sid = SessionId {
            value: session_id.to_string(),
        };
        let stage = Self::phase_to_stage(phase);

        let content = self
            .orchestrator
            .export_artifact(&sid, stage)
            .await
            .map_err(Self::map_error)?;

        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_to_stage_mapping() {
        assert!(matches!(
            SddFacadeImpl::phase_to_stage(SddPhase::Spec),
            Stage::Spec
        ));
        assert!(matches!(
            SddFacadeImpl::phase_to_stage(SddPhase::Design),
            Stage::Design
        ));
        assert!(matches!(
            SddFacadeImpl::phase_to_stage(SddPhase::Task),
            Stage::Task
        ));
    }

    #[test]
    fn test_stage_to_phase_mapping() {
        assert!(matches!(
            SddFacadeImpl::stage_to_phase(&Stage::Spec),
            SddPhase::Spec
        ));
        assert!(matches!(
            SddFacadeImpl::stage_to_phase(&Stage::Design),
            SddPhase::Design
        ));
    }

    #[test]
    fn test_review_decision_mapping() {
        let pass = SddFacadeImpl::map_review_decision(VisualReviewDecision::Approve, None);
        assert!(matches!(pass, SddReviewDecision::Pass));

        let redo = SddFacadeImpl::map_review_decision(
            VisualReviewDecision::Reject,
            Some("需要修改"),
        );
        match redo {
            SddReviewDecision::Redo { comment } => assert_eq!(comment, "需要修改"),
            _ => panic!("应为 Redo"),
        }
    }
}