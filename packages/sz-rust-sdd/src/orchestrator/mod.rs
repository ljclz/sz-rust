// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

pub mod state_machine;
pub mod transaction;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::fs;

use crate::artifact::ArtifactStore;
use crate::capability::guard::{register_sdd_capabilities, SddCapabilityGuard};
use crate::error::{SddError, SddErrorCode, SddResult};
use crate::legacy::analyzer::LegacyAnalyzer;
use crate::model::{
    AiConfig, OrchestrationTrace, ReviewDecision, ReviewOutcome, SddConfig, SessionId,
    SessionState, SessionSummary, SessionToolWhitelist, Stage,
};
use crate::prompt::PromptBuilder;
use crate::redactor::SddRedactor;
use crate::review::gate::ReviewGate;
use crate::rules::validator::RulesValidator;
use crate::session::SessionManager;
use crate::stage::executor::{StageContext, StageExecutorRegistry};
use crate::trace::Trace;

use sz_rust_capability::{AllowAll, CapabilityRegistry};

pub struct Orchestrator {
    session_mgr: Arc<SessionManager>,
    artifact_store: Arc<ArtifactStore>,
    review_gate: Arc<ReviewGate>,
    capability_guard: Arc<SddCapabilityGuard>,
    legacy_analyzer: Arc<LegacyAnalyzer>,
    rules_validator: Arc<RulesValidator>,
    trace: Arc<Trace>,
    prompt_builder: Arc<PromptBuilder>,
    stage_registry: StageExecutorRegistry,
    redactor: SddRedactor,
    config: SddConfig,
}

impl Orchestrator {
    pub fn new(config: SddConfig) -> SddResult<Self> {
        let session_mgr = Arc::new(SessionManager::from_config(&config));
        let artifact_store = Arc::new(ArtifactStore::from_config(&config));
        let trace = Arc::new(Trace::from_config(&config));
        let review_gate = Arc::new(ReviewGate::new(artifact_store.clone(), trace.clone()));

        let registry = Arc::new(CapabilityRegistry::new());
        register_sdd_capabilities(&registry);
        let capability_guard = Arc::new(SddCapabilityGuard::with_permission_checker(
            registry,
            Arc::new(AllowAll),
        ));

        let legacy_analyzer = Arc::new(LegacyAnalyzer::from_config(&config));
        let rules_validator = Arc::new(RulesValidator::new());
        let prompt_builder = Arc::new(PromptBuilder::new()?);

        let mut stage_registry = StageExecutorRegistry::new();
        stage_registry.register(Box::new(crate::stage::spec::SpecStageExecutor::new()));
        stage_registry.register(Box::new(crate::stage::design::DesignStageExecutor::new()));
        stage_registry.register(Box::new(crate::stage::task::TaskStageExecutor::new()));
        stage_registry.register(Box::new(crate::stage::coding::CodingStageExecutor::new()));

        Ok(Self {
            session_mgr,
            artifact_store,
            review_gate,
            capability_guard,
            legacy_analyzer,
            rules_validator,
            trace,
            prompt_builder,
            stage_registry,
            redactor: SddRedactor::new(),
            config,
        })
    }

    pub fn config(&self) -> &SddConfig {
        &self.config
    }

    pub fn session_manager(&self) -> &SessionManager {
        &self.session_mgr
    }

    pub fn artifact_store(&self) -> &ArtifactStore {
        &self.artifact_store
    }

    pub fn review_gate(&self) -> &ReviewGate {
        &self.review_gate
    }

    pub fn capability_guard(&self) -> &SddCapabilityGuard {
        &self.capability_guard
    }

    pub fn legacy_analyzer(&self) -> &LegacyAnalyzer {
        &self.legacy_analyzer
    }

    pub fn rules_validator(&self) -> &RulesValidator {
        &self.rules_validator
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    pub fn redactor(&self) -> &SddRedactor {
        &self.redactor
    }

    pub async fn init_session(
        &self,
        requirement: String,
        target_crate: PathBuf,
        ai_config: AiConfig,
        whitelist: SessionToolWhitelist,
    ) -> SddResult<SessionId> {
        self.capability_guard.check_session_create(0).await?;

        let rules_path = self.config.store_root.join("rules.md");
        let state = self
            .session_mgr
            .create_session(requirement, target_crate, ai_config, whitelist, rules_path)
            .await?;

        self.trace
            .record_stage_start(&state.id, Stage::Spec)
            .await?;

        Ok(state.id)
    }

    pub async fn resume_session(&self, session_id: &SessionId) -> SddResult<SessionState> {
        self.capability_guard.check_session_resume(0).await?;

        let state = self.session_mgr.load_session(session_id).await?;

        let session_dir = self.session_mgr.store_root().join(&session_id.value);
        transaction::Transaction::verify_consistency(&session_dir, &state).await?;

        Ok(state)
    }

    pub async fn submit_review(
        &self,
        session_id: &SessionId,
        decision: ReviewDecision,
    ) -> SddResult<ReviewOutcome> {
        self.capability_guard.check_review_submit(0).await?;

        let state = self.session_mgr.load_session(session_id).await?;

        let current_stage = state.current_stage;
        let orch_state = self.derive_orchestrator_state(&state);

        if !orch_state.is_paused_at_review() {
            return Err(SddError::new(
                SddErrorCode::SddReviewDecisionInvalid,
                format!(
                    "cannot submit review: session is not paused at review (current stage: {})",
                    current_stage
                ),
            ));
        }

        let outcome = self
            .review_gate
            .handle_decision(session_id, current_stage, orch_state, decision.clone())
            .await?;

        let mut new_state = state;
        new_state.current_stage = outcome.new_stage;
        new_state.review_status = outcome.new_review_status;

        self.session_mgr.save_session(&new_state).await?;

        Ok(outcome)
    }

    pub async fn query_trace(&self, session_id: &SessionId) -> SddResult<OrchestrationTrace> {
        let _state = self.session_mgr.load_session(session_id).await?;
        self.trace.query_trace(session_id).await
    }

    pub async fn list_sessions(&self) -> SddResult<Vec<SessionSummary>> {
        self.session_mgr.list_sessions().await
    }

    pub async fn run_stage(&self, session_id: &SessionId) -> SddResult<Stage> {
        let state = self.session_mgr.load_session(session_id).await?;
        let current_stage = state.current_stage;

        if !current_stage.is_executable() {
            return Err(SddError::new(
                SddErrorCode::SddReviewDecisionInvalid,
                format!(
                    "cannot run stage {}: not an executable stage",
                    current_stage
                ),
            ));
        }

        let requirement = self.load_requirement(session_id).await?;

        let ctx = StageContext::new(
            session_id.clone(),
            state.clone(),
            requirement,
            state.target_crate.clone(),
            self.artifact_store.clone(),
            self.trace.clone(),
            self.prompt_builder.clone(),
        );

        let executor = self.stage_registry.get(current_stage).ok_or_else(|| {
            SddError::new(
                SddErrorCode::SddStageGenFailed,
                format!("no executor registered for stage {}", current_stage),
            )
        })?;

        let stage_start = std::time::Instant::now();
        let meta = executor.execute(&ctx).await?;
        let duration_ms = stage_start.elapsed().as_millis() as u64;

        let versions = self
            .artifact_store
            .list_versions(session_id, current_stage)
            .await?;
        let latest_version = versions.last().copied().unwrap_or_else(|| {
            meta.parent_version
                .unwrap_or_else(crate::model::ArtifactVersion::initial)
        });

        let artifact = self
            .artifact_store
            .load_artifact(session_id, current_stage, latest_version)
            .await?;

        let validation = executor.validate_artifact(&artifact).await?;
        if !validation.passed {
            return Err(SddError::new(
                SddErrorCode::SddStageGenFailed,
                format!(
                    "artifact validation failed for stage {}: {:?}",
                    current_stage, validation.violations
                ),
            ));
        }

        self.trace
            .record_stage_end(session_id, current_stage, duration_ms, latest_version.value)
            .await?;

        let mut updated_state = state;
        updated_state
            .artifact_version_pointers
            .insert(current_stage, latest_version);
        updated_state.review_status = crate::model::ReviewStatus::WaitingReview;
        self.session_mgr.save_session(&updated_state).await?;

        let next_stage = current_stage.next().unwrap_or(Stage::Completed);
        Ok(next_stage)
    }

    pub async fn export_artifact(
        &self,
        session_id: &SessionId,
        stage: Stage,
        version: crate::model::ArtifactVersion,
        output_path: &std::path::Path,
    ) -> SddResult<()> {
        self.artifact_store
            .export_artifact(session_id, stage, version, output_path)
            .await
    }

    async fn load_requirement(&self, session_id: &SessionId) -> SddResult<String> {
        let path = self
            .session_mgr
            .store_root()
            .join(&session_id.value)
            .join("requirement.txt");
        fs::read_to_string(&path).await.map_err(|e| {
            SddError::new(
                SddErrorCode::SddSessionCorrupted,
                format!("requirement file not readable: {e}"),
            )
        })
    }

    fn derive_orchestrator_state(&self, state: &SessionState) -> state_machine::OrchestratorState {
        use crate::model::ReviewStatus;
        use state_machine::OrchestratorState;

        let has_artifact = state
            .artifact_version_pointers
            .contains_key(&state.current_stage);

        match (state.current_stage, state.review_status, has_artifact) {
            (Stage::Spec, ReviewStatus::WaitingReview, true) => {
                OrchestratorState::SpecPausedAtReview
            }
            (Stage::Spec, _, _) => OrchestratorState::SpecRunning,
            (Stage::Design, ReviewStatus::WaitingReview, true) => {
                OrchestratorState::DesignPausedAtReview
            }
            (Stage::Design, _, _) => OrchestratorState::DesignRunning,
            (Stage::Task, ReviewStatus::WaitingReview, true) => {
                OrchestratorState::TaskPausedAtReview
            }
            (Stage::Task, _, _) => OrchestratorState::TaskRunning,
            (Stage::Coding, ReviewStatus::WaitingReview, true) => {
                OrchestratorState::CodingPausedAtReview
            }
            (Stage::Coding, _, _) => OrchestratorState::CodingRunning,
            (Stage::Completed, _, _) => OrchestratorState::Completed,
            (Stage::Failed, _, _) => OrchestratorState::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn test_config() -> SddConfig {
        let store_root = std::env::temp_dir().join(format!("sdd-orch-{}", uuid::Uuid::new_v4()));
        SddConfig {
            store_root,
            max_concurrent_sessions: 8,
            stage_timeout_ms: 120_000,
            legacy_analysis_timeout_ms: 30_000,
            trace_query_timeout_ms: 2_000,
        }
    }

    fn test_ai_config() -> AiConfig {
        AiConfig::new("test-provider", "test-model")
    }

    fn test_whitelist() -> SessionToolWhitelist {
        SessionToolWhitelist::new(HashSet::new())
    }

    #[tokio::test]
    async fn orchestrator_init_resume() {
        let config = test_config();
        fs::create_dir_all(&config.store_root).await.unwrap();
        let orch = Orchestrator::new(config.clone()).unwrap();

        let target = std::env::temp_dir().join("sdd-test-target");
        fs::create_dir_all(&target).await.unwrap();

        let session_id = orch
            .init_session(
                "Build a hello world CLI".to_string(),
                target.clone(),
                test_ai_config(),
                test_whitelist(),
            )
            .await
            .unwrap();

        let state = orch.resume_session(&session_id).await.unwrap();
        assert_eq!(state.current_stage, Stage::Spec);
    }

    #[tokio::test]
    async fn orchestrator_list_sessions() {
        let config = test_config();
        fs::create_dir_all(&config.store_root).await.unwrap();
        let orch = Orchestrator::new(config.clone()).unwrap();

        let target = std::env::temp_dir().join("sdd-test-target-list");
        fs::create_dir_all(&target).await.unwrap();

        let _id1 = orch
            .init_session(
                "req1".to_string(),
                target.clone(),
                test_ai_config(),
                test_whitelist(),
            )
            .await
            .unwrap();
        let _id2 = orch
            .init_session(
                "req2".to_string(),
                target.clone(),
                test_ai_config(),
                test_whitelist(),
            )
            .await
            .unwrap();

        let sessions = orch.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2);
    }

    #[tokio::test]
    async fn orchestrator_query_trace() {
        let config = test_config();
        fs::create_dir_all(&config.store_root).await.unwrap();
        let orch = Orchestrator::new(config.clone()).unwrap();

        let target = std::env::temp_dir().join("sdd-test-target-trace");
        fs::create_dir_all(&target).await.unwrap();

        let session_id = orch
            .init_session(
                "trace test".to_string(),
                target.clone(),
                test_ai_config(),
                test_whitelist(),
            )
            .await
            .unwrap();

        let trace = orch.query_trace(&session_id).await.unwrap();
        assert_eq!(trace.session_id, session_id);
    }

    #[tokio::test]
    async fn orchestrator_resume_not_found() {
        let config = test_config();
        fs::create_dir_all(&config.store_root).await.unwrap();
        let orch = Orchestrator::new(config.clone()).unwrap();

        let fake_id = SessionId::new();
        let err = orch.resume_session(&fake_id).await.unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddSessionNotFound));
    }

    #[tokio::test]
    async fn orchestrator_submit_review_not_paused() {
        let config = test_config();
        fs::create_dir_all(&config.store_root).await.unwrap();
        let orch = Orchestrator::new(config.clone()).unwrap();

        let target = std::env::temp_dir().join("sdd-test-target-review");
        fs::create_dir_all(&target).await.unwrap();

        let session_id = orch
            .init_session(
                "review test".to_string(),
                target.clone(),
                test_ai_config(),
                test_whitelist(),
            )
            .await
            .unwrap();

        let err = orch
            .submit_review(&session_id, ReviewDecision::Pass)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddReviewDecisionInvalid));
    }

    #[tokio::test]
    async fn orchestrator_capability_guard_initialized() {
        let config = test_config();
        fs::create_dir_all(&config.store_root).await.unwrap();
        let orch = Orchestrator::new(config).unwrap();

        assert!(orch.capability_guard.check_session_create(0).await.is_ok());
        assert!(orch.capability_guard.check_llm_chat(0).await.is_ok());
        assert!(orch
            .capability_guard
            .check_mcp_call("test", 0)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn orchestrator_stage_registry_has_all_stages() {
        let config = test_config();
        fs::create_dir_all(&config.store_root).await.unwrap();
        let orch = Orchestrator::new(config).unwrap();

        let stages = orch.stage_registry.stages();
        assert!(stages.contains(&Stage::Spec));
        assert!(stages.contains(&Stage::Design));
        assert!(stages.contains(&Stage::Task));
        assert!(stages.contains(&Stage::Coding));
    }
}
