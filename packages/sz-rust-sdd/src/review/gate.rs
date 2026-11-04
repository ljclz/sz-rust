// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::HashSet;
use std::sync::Arc;

use crate::artifact::ArtifactStore;
use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::{
    ArtifactVersion, ReviewDecision, ReviewOutcome, ReviewRequest, ReviewStatus, SessionId, Stage,
};
use crate::orchestrator::state_machine::OrchestratorState;
use crate::redactor::SddRedactor;
use crate::trace::Trace;

pub struct ReviewGate {
    artifact_store: Arc<ArtifactStore>,
    trace: Arc<Trace>,
    redactor: SddRedactor,
    processed_decisions: parking_lot::Mutex<HashSet<String>>,
}

impl ReviewGate {
    pub fn new(artifact_store: Arc<ArtifactStore>, trace: Arc<Trace>) -> Self {
        Self {
            artifact_store,
            trace,
            redactor: SddRedactor::new(),
            processed_decisions: parking_lot::Mutex::new(HashSet::new()),
        }
    }

    pub async fn create_review_request(
        &self,
        session_id: &SessionId,
        stage: Stage,
        version: ArtifactVersion,
    ) -> SddResult<ReviewRequest> {
        let _artifact = self
            .artifact_store
            .load_artifact(session_id, stage, version)
            .await?;

        let summary = self
            .artifact_store
            .compute_summary(session_id, stage, version)
            .await?;

        let versions = self.artifact_store.list_versions(session_id, stage).await?;

        let diff_preview = if versions.len() > 1 {
            let prev_version = versions[versions.len() - 2];
            let diff = self
                .artifact_store
                .diff_artifacts(session_id, stage, prev_version, version)
                .await?;
            self.redactor.redact_text(&diff.diff_text)
        } else {
            "(首次生成，无 diff)".to_string()
        };

        let artifact_path = self
            .artifact_store
            .store_root()
            .join(&session_id.value)
            .join(stage.artifact_filename(version.value));

        let mut request = ReviewRequest::new(
            session_id.clone(),
            stage,
            version,
            artifact_path,
            summary,
            diff_preview,
        );

        let redacted_summary = self.redactor.redact_text(&request.diff_preview);
        request.diff_preview = redacted_summary;

        Ok(request)
    }

    pub async fn handle_decision(
        &self,
        session_id: &SessionId,
        current_stage: Stage,
        current_state: OrchestratorState,
        decision: ReviewDecision,
    ) -> SddResult<ReviewOutcome> {
        decision.validate(current_stage)?;

        let decision_key = self.decision_key(session_id, current_stage, &decision);
        {
            let processed = self.processed_decisions.lock();
            if processed.contains(&decision_key) {
                return Err(SddError::new(
                    SddErrorCode::SddReviewDecisionInvalid,
                    "决策已处理（幂等校验）：重复提交相同决策只生效一次",
                ));
            }
        }

        let _new_state = current_state.advance(&decision)?;

        let (new_stage, new_review_status) = match &decision {
            ReviewDecision::Pass => {
                let next_stage = current_stage.next().unwrap_or(Stage::Completed);
                (next_stage, ReviewStatus::Approved)
            }
            ReviewDecision::Rollback { target } => (*target, ReviewStatus::RolledBack),
            ReviewDecision::Redo { .. } => (current_stage, ReviewStatus::Redoing),
        };

        self.trace
            .record_review_decision(session_id, current_stage, decision.clone())
            .await?;

        if let ReviewDecision::Rollback { target } = &decision {
            self.trace
                .record_rollback(session_id, current_stage, *target)
                .await?;
        }

        {
            let mut processed = self.processed_decisions.lock();
            processed.insert(decision_key);
        }

        Ok(ReviewOutcome {
            session_id: session_id.clone(),
            decision,
            new_stage,
            new_review_status,
        })
    }

    fn decision_key(
        &self,
        session_id: &SessionId,
        stage: Stage,
        decision: &ReviewDecision,
    ) -> String {
        format!("{}:{}:{:?}", session_id.value, stage, decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArtifactMeta, ArtifactReviewStatus};
    use tokio::fs;

    async fn setup_gate() -> (ReviewGate, SessionId) {
        let root = std::env::temp_dir().join(format!("sdd-gate-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).await.unwrap();
        let store = Arc::new(ArtifactStore::new(root.clone()));
        let trace = Arc::new(Trace::new(root));
        let session_id = SessionId::new();
        fs::create_dir_all(store.store_root().join(&session_id.value))
            .await
            .unwrap();
        (ReviewGate::new(store, trace), session_id)
    }

    async fn save_sample_artifact(
        gate: &ReviewGate,
        session_id: &SessionId,
        stage: Stage,
        content: &str,
    ) -> ArtifactVersion {
        let meta = ArtifactMeta {
            generated_at: chrono::Utc::now(),
            generated_stage: stage,
            review_status: ArtifactReviewStatus::Pending,
            ai_call_records: Vec::new(),
            parent_version: None,
        };
        gate.artifact_store
            .save_artifact(session_id, stage, content, meta)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn review_gate_create_request() {
        let (gate, session_id) = setup_gate().await;
        let version = save_sample_artifact(&gate, &session_id, Stage::Spec, "# Spec\n内容").await;

        let request = gate
            .create_review_request(&session_id, Stage::Spec, version)
            .await
            .unwrap();

        assert_eq!(request.session_id, session_id);
        assert_eq!(request.stage, Stage::Spec);
        assert_eq!(request.artifact_version, version);
    }

    #[tokio::test]
    async fn review_gate_pass() {
        let (gate, session_id) = setup_gate().await;

        let outcome = gate
            .handle_decision(
                &session_id,
                Stage::Spec,
                OrchestratorState::SpecPausedAtReview,
                ReviewDecision::Pass,
            )
            .await
            .unwrap();

        assert_eq!(outcome.new_stage, Stage::Design);
        assert_eq!(outcome.new_review_status, ReviewStatus::Approved);
    }

    #[tokio::test]
    async fn review_gate_rollback() {
        let (gate, session_id) = setup_gate().await;

        let outcome = gate
            .handle_decision(
                &session_id,
                Stage::Design,
                OrchestratorState::DesignPausedAtReview,
                ReviewDecision::Rollback {
                    target: Stage::Spec,
                },
            )
            .await
            .unwrap();

        assert_eq!(outcome.new_stage, Stage::Spec);
        assert_eq!(outcome.new_review_status, ReviewStatus::RolledBack);
    }

    #[tokio::test]
    async fn review_gate_redo() {
        let (gate, session_id) = setup_gate().await;

        let outcome = gate
            .handle_decision(
                &session_id,
                Stage::Spec,
                OrchestratorState::SpecPausedAtReview,
                ReviewDecision::Redo {
                    comment: "增加更多细节".to_string(),
                },
            )
            .await
            .unwrap();

        assert_eq!(outcome.new_stage, Stage::Spec);
        assert_eq!(outcome.new_review_status, ReviewStatus::Redoing);
    }

    #[tokio::test]
    async fn review_idempotent() {
        let (gate, session_id) = setup_gate().await;

        let decision = ReviewDecision::Pass;
        let _outcome = gate
            .handle_decision(
                &session_id,
                Stage::Spec,
                OrchestratorState::SpecPausedAtReview,
                decision.clone(),
            )
            .await
            .unwrap();

        let err = gate
            .handle_decision(
                &session_id,
                Stage::Spec,
                OrchestratorState::SpecPausedAtReview,
                decision,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddReviewDecisionInvalid));
    }

    #[tokio::test]
    async fn review_decision_validation_rollback_downstream() {
        let (gate, session_id) = setup_gate().await;

        let err = gate
            .handle_decision(
                &session_id,
                Stage::Spec,
                OrchestratorState::SpecPausedAtReview,
                ReviewDecision::Rollback {
                    target: Stage::Coding,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddRollbackTargetInvalid));
    }

    #[tokio::test]
    async fn review_decision_validation_redo_empty_comment() {
        let (gate, session_id) = setup_gate().await;

        let err = gate
            .handle_decision(
                &session_id,
                Stage::Spec,
                OrchestratorState::SpecPausedAtReview,
                ReviewDecision::Redo {
                    comment: "  ".to_string(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddReviewDecisionInvalid));
    }
}
