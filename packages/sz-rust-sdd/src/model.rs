// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Stage {
    Spec,
    Design,
    Task,
    Coding,
    Completed,
    Failed,
}

impl Stage {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }

    pub fn is_executable(self) -> bool {
        matches!(self, Self::Spec | Self::Design | Self::Task | Self::Coding)
    }

    pub fn next(self) -> Option<Self> {
        match self {
            Self::Spec => Some(Self::Design),
            Self::Design => Some(Self::Task),
            Self::Task => Some(Self::Coding),
            Self::Coding => Some(Self::Completed),
            Self::Completed | Self::Failed => None,
        }
    }

    pub fn is_upstream_of(self, other: Self) -> bool {
        let order = |s: Self| match s {
            Self::Spec => 0,
            Self::Design => 1,
            Self::Task => 2,
            Self::Coding => 3,
            Self::Completed => 4,
            Self::Failed => 5,
        };
        order(self) <= order(other)
    }

    pub fn artifact_filename(self, version: u32) -> String {
        let name = match self {
            Self::Spec => "spec",
            Self::Design => "design",
            Self::Task => "task",
            Self::Coding => "coding",
            Self::Completed => "completed",
            Self::Failed => "failed",
        };
        format!("{name}_v{version}.md")
    }
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Spec => "Spec",
            Self::Design => "Design",
            Self::Task => "Task",
            Self::Coding => "Coding",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ReviewStatus {
    WaitingReview,
    Approved,
    RolledBack,
    Redoing,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewDecision {
    Pass,
    Rollback { target: Stage },
    Redo { comment: String },
}

impl ReviewDecision {
    pub fn validate(&self, current_stage: Stage) -> Result<(), super::error::SddError> {
        match self {
            Self::Pass => Ok(()),
            Self::Rollback { target } => {
                if target.is_upstream_of(current_stage) {
                    Ok(())
                } else {
                    Err(super::error::SddError::new(
                        super::error::SddErrorCode::SddRollbackTargetInvalid,
                        format!(
                            "rollback target {target} is downstream of current stage {current_stage}"
                        ),
                    ))
                }
            }
            Self::Redo { comment } => {
                if comment.trim().is_empty() {
                    Err(super::error::SddError::new(
                        super::error::SddErrorCode::SddReviewDecisionInvalid,
                        "redo decision requires non-empty comment",
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ArtifactVersion {
    pub value: u32,
    pub parent: Option<u32>,
}

impl ArtifactVersion {
    pub fn initial() -> Self {
        Self {
            value: 1,
            parent: None,
        }
    }

    pub fn next(self) -> Self {
        Self {
            value: self.value + 1,
            parent: Some(self.value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SessionId {
    pub value: String,
}

impl SessionId {
    pub fn new() -> Self {
        Self {
            value: uuid::Uuid::new_v4().to_string(),
        }
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.value)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RetryStrategy {
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryStrategy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1000,
            max_delay_ms: 30000,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiConfig {
    pub provider: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub retry_strategy: RetryStrategy,
}

impl AiConfig {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            temperature: 0.7,
            max_tokens: 8192,
            retry_strategy: RetryStrategy::default(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionToolWhitelist {
    allowed: HashSet<String>,
    locked: bool,
}

impl SessionToolWhitelist {
    pub fn new(allowed: HashSet<String>) -> Self {
        Self {
            allowed,
            locked: false,
        }
    }

    pub fn lock(&mut self) {
        self.locked = true;
    }

    pub fn is_allowed(&self, tool: &str) -> bool {
        self.allowed.contains(tool)
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn allowed_tools(&self) -> &HashSet<String> {
        &self.allowed
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionState {
    pub id: SessionId,
    pub current_stage: Stage,
    pub review_status: ReviewStatus,
    pub artifact_version_pointers: HashMap<Stage, ArtifactVersion>,
    pub ai_config: AiConfig,
    pub target_crate: PathBuf,
    pub whitelist: SessionToolWhitelist,
    pub created_at: DateTime<Utc>,
    pub rules_path: PathBuf,
}

impl SessionState {
    pub fn new(
        id: SessionId,
        target_crate: PathBuf,
        ai_config: AiConfig,
        whitelist: SessionToolWhitelist,
        rules_path: PathBuf,
    ) -> Self {
        Self {
            id,
            current_stage: Stage::Spec,
            review_status: ReviewStatus::WaitingReview,
            artifact_version_pointers: HashMap::new(),
            ai_config,
            target_crate,
            whitelist,
            created_at: Utc::now(),
            rules_path,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactReviewStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiCallRecord {
    pub provider: String,
    pub model: String,
    pub prompt_summary: String,
    pub token_usage: TokenUsage,
    pub duration_ms: u64,
    pub retry_count: u32,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArtifactMeta {
    pub generated_at: DateTime<Utc>,
    pub generated_stage: Stage,
    pub review_status: ArtifactReviewStatus,
    pub ai_call_records: Vec<AiCallRecord>,
    pub parent_version: Option<ArtifactVersion>,
}

impl ArtifactMeta {
    pub fn new(stage: Stage) -> Self {
        Self {
            generated_at: Utc::now(),
            generated_stage: stage,
            review_status: ArtifactReviewStatus::Pending,
            ai_call_records: Vec::new(),
            parent_version: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Artifact {
    pub session_id: SessionId,
    pub stage: Stage,
    pub version: ArtifactVersion,
    pub content: String,
    pub meta: ArtifactMeta,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArtifactSummary {
    pub stage: Stage,
    pub version: ArtifactVersion,
    pub content_length: usize,
    pub section_titles: Vec<String>,
    pub task_count: Option<usize>,
    pub code_change_lines: Option<usize>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArtifactDiff {
    pub stage: Stage,
    pub v1: ArtifactVersion,
    pub v2: ArtifactVersion,
    pub added_lines: usize,
    pub removed_lines: usize,
    pub modified_lines: usize,
    pub diff_text: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewRequest {
    pub session_id: SessionId,
    pub stage: Stage,
    pub artifact_version: ArtifactVersion,
    pub artifact_path: PathBuf,
    pub summary: ArtifactSummary,
    pub diff_preview: String,
    pub created_at: DateTime<Utc>,
}

impl ReviewRequest {
    pub fn new(
        session_id: SessionId,
        stage: Stage,
        artifact_version: ArtifactVersion,
        artifact_path: PathBuf,
        summary: ArtifactSummary,
        diff_preview: String,
    ) -> Self {
        Self {
            session_id,
            stage,
            artifact_version,
            artifact_path,
            summary,
            diff_preview,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewOutcome {
    pub session_id: SessionId,
    pub decision: ReviewDecision,
    pub new_stage: Stage,
    pub new_review_status: ReviewStatus,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub current_stage: Stage,
    pub review_status: ReviewStatus,
    pub target_crate: PathBuf,
    pub created_at: DateTime<Utc>,
}

impl From<&SessionState> for SessionSummary {
    fn from(state: &SessionState) -> Self {
        Self {
            id: state.id.clone(),
            current_stage: state.current_stage,
            review_status: state.review_status,
            target_crate: state.target_crate.clone(),
            created_at: state.created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StageRecord {
    pub stage: Stage,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub duration_ms: u64,
    pub artifact_version: ArtifactVersion,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewDecisionRecord {
    pub stage: Stage,
    pub decision: ReviewDecision,
    pub decided_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RollbackEvent {
    pub from_stage: Stage,
    pub to_stage: Stage,
    pub rolled_back_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OrchestrationTrace {
    pub session_id: SessionId,
    pub stage_records: Vec<StageRecord>,
    pub ai_call_records: Vec<AiCallRecord>,
    pub review_decisions: Vec<ReviewDecisionRecord>,
    pub rollback_history: Vec<RollbackEvent>,
    pub total_duration_ms: u64,
}

impl OrchestrationTrace {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            stage_records: Vec::new(),
            ai_call_records: Vec::new(),
            review_decisions: Vec::new(),
            rollback_history: Vec::new(),
            total_duration_ms: 0,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SddConfig {
    pub store_root: PathBuf,
    pub max_concurrent_sessions: u64,
    pub stage_timeout_ms: u64,
    pub legacy_analysis_timeout_ms: u64,
    pub trace_query_timeout_ms: u64,
}

impl Default for SddConfig {
    fn default() -> Self {
        Self {
            store_root: PathBuf::from(".codeartsdoer/sdd-sessions"),
            max_concurrent_sessions: 8,
            stage_timeout_ms: 120_000,
            legacy_analysis_timeout_ms: 30_000,
            trace_query_timeout_ms: 2_000,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationResult {
    pub passed: bool,
    pub violations: Vec<String>,
}

impl ValidationResult {
    pub fn passed() -> Self {
        Self {
            passed: true,
            violations: Vec::new(),
        }
    }

    pub fn failed(violations: Vec<String>) -> Self {
        Self {
            passed: false,
            violations,
        }
    }

    pub fn and(self, other: Self) -> Self {
        if self.passed && other.passed {
            Self::passed()
        } else {
            let mut violations = self.violations;
            violations.extend(other.violations);
            Self::failed(violations)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_ordering_and_next() {
        assert_eq!(Stage::Spec.next(), Some(Stage::Design));
        assert_eq!(Stage::Design.next(), Some(Stage::Task));
        assert_eq!(Stage::Task.next(), Some(Stage::Coding));
        assert_eq!(Stage::Coding.next(), Some(Stage::Completed));
        assert_eq!(Stage::Completed.next(), None);
        assert_eq!(Stage::Failed.next(), None);
    }

    #[test]
    fn stage_upstream_of() {
        assert!(Stage::Spec.is_upstream_of(Stage::Design));
        assert!(Stage::Spec.is_upstream_of(Stage::Spec));
        assert!(!Stage::Design.is_upstream_of(Stage::Spec));
        assert!(Stage::Task.is_upstream_of(Stage::Coding));
    }

    #[test]
    fn stage_is_terminal() {
        assert!(Stage::Completed.is_terminal());
        assert!(Stage::Failed.is_terminal());
        assert!(!Stage::Spec.is_terminal());
    }

    #[test]
    fn stage_artifact_filename() {
        assert_eq!(Stage::Spec.artifact_filename(1), "spec_v1.md");
        assert_eq!(Stage::Design.artifact_filename(3), "design_v3.md");
    }

    #[test]
    fn artifact_version_increment() {
        let v1 = ArtifactVersion::initial();
        assert_eq!(v1.value, 1);
        assert_eq!(v1.parent, None);

        let v2 = v1.next();
        assert_eq!(v2.value, 2);
        assert_eq!(v2.parent, Some(1));

        let v3 = v2.next();
        assert_eq!(v3.value, 3);
        assert_eq!(v3.parent, Some(2));
    }

    #[test]
    fn review_decision_pass_valid() {
        let decision = ReviewDecision::Pass;
        assert!(decision.validate(Stage::Spec).is_ok());
    }

    #[test]
    fn review_decision_rollback_upstream_ok() {
        let decision = ReviewDecision::Rollback {
            target: Stage::Spec,
        };
        assert!(decision.validate(Stage::Design).is_ok());
    }

    #[test]
    fn review_decision_rollback_downstream_rejected() {
        let decision = ReviewDecision::Rollback {
            target: Stage::Coding,
        };
        let err = decision.validate(Stage::Spec).unwrap_err();
        assert_eq!(
            err.code(),
            Some(&super::super::error::SddErrorCode::SddRollbackTargetInvalid)
        );
    }

    #[test]
    fn review_decision_redo_empty_comment_rejected() {
        let decision = ReviewDecision::Redo {
            comment: "   ".to_string(),
        };
        let err = decision.validate(Stage::Spec).unwrap_err();
        assert_eq!(
            err.code(),
            Some(&super::super::error::SddErrorCode::SddReviewDecisionInvalid)
        );
    }

    #[test]
    fn review_decision_redo_nonempty_comment_ok() {
        let decision = ReviewDecision::Redo {
            comment: "add more detail".to_string(),
        };
        assert!(decision.validate(Stage::Spec).is_ok());
    }

    #[test]
    fn session_id_unique() {
        let id1 = SessionId::new();
        let id2 = SessionId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn session_tool_whitelist_lock() {
        let mut wl =
            SessionToolWhitelist::new(["tool_a", "tool_b"].into_iter().map(String::from).collect());
        assert!(!wl.is_locked());
        assert!(wl.is_allowed("tool_a"));
        assert!(!wl.is_allowed("tool_c"));
        wl.lock();
        assert!(wl.is_locked());
    }

    #[test]
    fn validation_result_combine() {
        let r1 = ValidationResult::passed();
        let r2 = ValidationResult::passed();
        assert!(r1.and(r2).passed);

        let r3 = ValidationResult::failed(vec!["violation1".to_string()]);
        let r4 = ValidationResult::passed();
        let combined = r3.and(r4);
        assert!(!combined.passed);
        assert_eq!(combined.violations, vec!["violation1".to_string()]);
    }

    #[test]
    fn model_serde_roundtrip_session_state() {
        let state = SessionState::new(
            SessionId::new(),
            PathBuf::from("/tmp/crate"),
            AiConfig::new("claude", "claude-3"),
            SessionToolWhitelist::new(HashSet::new()),
            PathBuf::from("/tmp/rules.md"),
        );
        let json = serde_json::to_string(&state).unwrap();
        let decoded: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(state.current_stage, decoded.current_stage);
        assert_eq!(state.target_crate, decoded.target_crate);
    }

    #[test]
    fn model_serde_roundtrip_review_decision() {
        let decision = ReviewDecision::Redo {
            comment: "fix this".to_string(),
        };
        let json = serde_json::to_string(&decision).unwrap();
        let decoded: ReviewDecision = serde_json::from_str(&json).unwrap();
        match decoded {
            ReviewDecision::Redo { comment } => assert_eq!(comment, "fix this"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn model_serde_roundtrip_artifact() {
        let artifact = Artifact {
            session_id: SessionId::new(),
            stage: Stage::Spec,
            version: ArtifactVersion::initial(),
            content: "# Spec\n".to_string(),
            meta: ArtifactMeta::new(Stage::Spec),
        };
        let json = serde_json::to_string(&artifact).unwrap();
        let decoded: Artifact = serde_json::from_str(&json).unwrap();
        assert_eq!(artifact.content, decoded.content);
        assert_eq!(artifact.stage, decoded.stage);
    }

    #[test]
    fn model_serde_roundtrace() {
        let trace = OrchestrationTrace::new(SessionId::new());
        let json = serde_json::to_string(&trace).unwrap();
        let decoded: OrchestrationTrace = serde_json::from_str(&json).unwrap();
        assert_eq!(trace.stage_records, decoded.stage_records);
    }

    #[test]
    fn sdd_config_default() {
        let config = SddConfig::default();
        assert_eq!(config.max_concurrent_sessions, 8);
        assert_eq!(config.stage_timeout_ms, 120_000);
        assert_eq!(config.legacy_analysis_timeout_ms, 30_000);
    }
}
