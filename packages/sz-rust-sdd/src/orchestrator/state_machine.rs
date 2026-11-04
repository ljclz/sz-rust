// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::Stage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum OrchestratorState {
    SpecRunning,
    SpecPausedAtReview,
    DesignRunning,
    DesignPausedAtReview,
    TaskRunning,
    TaskPausedAtReview,
    CodingRunning,
    CodingPausedAtReview,
    Failed,
    Completed,
}

impl OrchestratorState {
    pub fn initial() -> Self {
        Self::SpecRunning
    }

    pub fn current_stage(self) -> Stage {
        match self {
            Self::SpecRunning | Self::SpecPausedAtReview => Stage::Spec,
            Self::DesignRunning | Self::DesignPausedAtReview => Stage::Design,
            Self::TaskRunning | Self::TaskPausedAtReview => Stage::Task,
            Self::CodingRunning | Self::CodingPausedAtReview => Stage::Coding,
            Self::Completed => Stage::Completed,
            Self::Failed => Stage::Failed,
        }
    }

    pub fn is_running(self) -> bool {
        matches!(
            self,
            Self::SpecRunning | Self::DesignRunning | Self::TaskRunning | Self::CodingRunning
        )
    }

    pub fn is_paused_at_review(self) -> bool {
        matches!(
            self,
            Self::SpecPausedAtReview
                | Self::DesignPausedAtReview
                | Self::TaskPausedAtReview
                | Self::CodingPausedAtReview
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }

    pub fn pause_for_review(self) -> SddResult<Self> {
        match self {
            Self::SpecRunning => Ok(Self::SpecPausedAtReview),
            Self::DesignRunning => Ok(Self::DesignPausedAtReview),
            Self::TaskRunning => Ok(Self::TaskPausedAtReview),
            Self::CodingRunning => Ok(Self::CodingPausedAtReview),
            _ => Err(SddError::new(
                SddErrorCode::SddReviewDecisionInvalid,
                format!(
                    "cannot pause for review from state {:?}: not in running state",
                    self
                ),
            )),
        }
    }

    pub fn pass(self) -> SddResult<Self> {
        match self {
            Self::SpecPausedAtReview => Ok(Self::DesignRunning),
            Self::DesignPausedAtReview => Ok(Self::TaskRunning),
            Self::TaskPausedAtReview => Ok(Self::CodingRunning),
            Self::CodingPausedAtReview => Ok(Self::Completed),
            _ => Err(SddError::new(
                SddErrorCode::SddReviewDecisionInvalid,
                format!("cannot pass from state {:?}: not paused at review", self),
            )),
        }
    }

    pub fn rollback(self, target: Stage) -> SddResult<Self> {
        let current = self.current_stage();
        if !target.is_upstream_of(current) {
            return Err(SddError::new(
                SddErrorCode::SddRollbackTargetInvalid,
                format!("rollback target {target} is downstream of current stage {current}"),
            ));
        }

        if !self.is_paused_at_review() {
            return Err(SddError::new(
                SddErrorCode::SddReviewDecisionInvalid,
                format!(
                    "cannot rollback from state {:?}: not paused at review",
                    self
                ),
            ));
        }

        Ok(match target {
            Stage::Spec => Self::SpecRunning,
            Stage::Design => Self::DesignRunning,
            Stage::Task => Self::TaskRunning,
            Stage::Coding => Self::CodingRunning,
            _ => {
                return Err(SddError::new(
                    SddErrorCode::SddRollbackTargetInvalid,
                    format!("invalid rollback target {target}"),
                ))
            }
        })
    }

    pub fn redo(self) -> SddResult<Self> {
        if !self.is_paused_at_review() {
            return Err(SddError::new(
                SddErrorCode::SddReviewDecisionInvalid,
                format!("cannot redo from state {:?}: not paused at review", self),
            ));
        }

        Ok(match self {
            Self::SpecPausedAtReview => Self::SpecRunning,
            Self::DesignPausedAtReview => Self::DesignRunning,
            Self::TaskPausedAtReview => Self::TaskRunning,
            Self::CodingPausedAtReview => Self::CodingRunning,
            _ => unreachable!(),
        })
    }

    pub fn fail(self) -> Self {
        if self.is_terminal() {
            self
        } else {
            Self::Failed
        }
    }

    pub fn complete(self) -> SddResult<Self> {
        match self {
            Self::CodingPausedAtReview => Ok(Self::Completed),
            _ => Err(SddError::new(
                SddErrorCode::SddReviewDecisionInvalid,
                format!(
                    "cannot complete from state {:?}: must be CodingPausedAtReview",
                    self
                ),
            )),
        }
    }

    pub fn advance(self, decision: &crate::model::ReviewDecision) -> SddResult<Self> {
        match decision {
            crate::model::ReviewDecision::Pass => self.pass(),
            crate::model::ReviewDecision::Rollback { target } => self.rollback(*target),
            crate::model::ReviewDecision::Redo { .. } => self.redo(),
        }
    }
}

impl std::fmt::Display for OrchestratorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::SpecRunning => "SpecRunning",
            Self::SpecPausedAtReview => "SpecPausedAtReview",
            Self::DesignRunning => "DesignRunning",
            Self::DesignPausedAtReview => "DesignPausedAtReview",
            Self::TaskRunning => "TaskRunning",
            Self::TaskPausedAtReview => "TaskPausedAtReview",
            Self::CodingRunning => "CodingRunning",
            Self::CodingPausedAtReview => "CodingPausedAtReview",
            Self::Failed => "Failed",
            Self::Completed => "Completed",
        };
        f.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ReviewDecision;

    #[test]
    fn state_machine_sequential() {
        let s0 = OrchestratorState::initial();
        assert_eq!(s0, OrchestratorState::SpecRunning);
        assert_eq!(s0.current_stage(), Stage::Spec);

        let s1 = s0.pause_for_review().unwrap();
        assert_eq!(s1, OrchestratorState::SpecPausedAtReview);

        let s2 = s1.pass().unwrap();
        assert_eq!(s2, OrchestratorState::DesignRunning);

        let s3 = s2.pause_for_review().unwrap();
        let s4 = s3.pass().unwrap();
        assert_eq!(s4, OrchestratorState::TaskRunning);

        let s5 = s4.pause_for_review().unwrap();
        let s6 = s5.pass().unwrap();
        assert_eq!(s6, OrchestratorState::CodingRunning);

        let s7 = s6.pause_for_review().unwrap();
        let s8 = s7.pass().unwrap();
        assert_eq!(s8, OrchestratorState::Completed);
        assert!(s8.is_terminal());
    }

    #[test]
    fn reject_skip() {
        let s0 = OrchestratorState::SpecRunning;

        let err = s0.pass().unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddReviewDecisionInvalid));

        let s1 = s0.pause_for_review().unwrap();
        let s2 = s1.pass().unwrap();
        assert_eq!(s2, OrchestratorState::DesignRunning);

        let err = s2.pass().unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddReviewDecisionInvalid));
    }

    #[test]
    fn rollback_target_validation() {
        let s = OrchestratorState::DesignPausedAtReview;

        let rolled = s.rollback(Stage::Spec).unwrap();
        assert_eq!(rolled, OrchestratorState::SpecRunning);

        let same = s.rollback(Stage::Design).unwrap();
        assert_eq!(same, OrchestratorState::DesignRunning);

        let err = s.rollback(Stage::Task).unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddRollbackTargetInvalid));

        let err = s.rollback(Stage::Coding).unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddRollbackTargetInvalid));
    }

    #[test]
    fn redo_from_paused() {
        let s = OrchestratorState::SpecPausedAtReview;
        let redone = s.redo().unwrap();
        assert_eq!(redone, OrchestratorState::SpecRunning);

        let s = OrchestratorState::TaskPausedAtReview;
        let redone = s.redo().unwrap();
        assert_eq!(redone, OrchestratorState::TaskRunning);
    }

    #[test]
    fn redo_from_running_rejected() {
        let s = OrchestratorState::SpecRunning;
        let err = s.redo().unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddReviewDecisionInvalid));
    }

    #[test]
    fn fail_from_any_non_terminal() {
        let s = OrchestratorState::SpecRunning;
        assert_eq!(s.fail(), OrchestratorState::Failed);

        let s = OrchestratorState::CodingPausedAtReview;
        assert_eq!(s.fail(), OrchestratorState::Failed);

        let s = OrchestratorState::Completed;
        assert_eq!(s.fail(), OrchestratorState::Completed);
    }

    #[test]
    fn advance_with_decision() {
        let s = OrchestratorState::SpecPausedAtReview;

        let next = s.advance(&ReviewDecision::Pass).unwrap();
        assert_eq!(next, OrchestratorState::DesignRunning);

        let next = s
            .advance(&ReviewDecision::Rollback {
                target: Stage::Spec,
            })
            .unwrap();
        assert_eq!(next, OrchestratorState::SpecRunning);

        let next = s
            .advance(&ReviewDecision::Redo {
                comment: "fix".to_string(),
            })
            .unwrap();
        assert_eq!(next, OrchestratorState::SpecRunning);
    }

    #[test]
    fn complete_only_from_coding_paused() {
        let s = OrchestratorState::CodingPausedAtReview;
        assert_eq!(s.complete().unwrap(), OrchestratorState::Completed);

        let s = OrchestratorState::TaskPausedAtReview;
        assert!(s.complete().is_err());
    }

    #[test]
    fn state_display() {
        assert_eq!(OrchestratorState::SpecRunning.to_string(), "SpecRunning");
        assert_eq!(
            OrchestratorState::CodingPausedAtReview.to_string(),
            "CodingPausedAtReview"
        );
    }
}
