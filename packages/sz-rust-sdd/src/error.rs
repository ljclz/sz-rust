// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SddErrorCode {
    SddSessionNotFound,
    SddSessionCorrupted,
    SddSessionLimitExceeded,
    SddStageGenFailed,
    SddRollbackTargetInvalid,
    SddReviewDecisionInvalid,
    SddReviewNotifyFailed,
    SddReviewUnauthorized,
    SddCapabilityUnauthorized,
    SddMcpToolFailed,
    SddAiFacadeIncompatible,
    SddTargetNotFound,
    SddTargetNotCrate,
    SddLegacyAnalysisTimeout,
    SddStorePermissionDenied,
    SddArtifactVersionNotFound,
    SddCliArgMissing,
    SddCliArgInvalid,
}

impl SddErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SddSessionNotFound => "SDD_SESSION_NOT_FOUND",
            Self::SddSessionCorrupted => "SDD_SESSION_CORRUPTED",
            Self::SddSessionLimitExceeded => "SDD_SESSION_LIMIT_EXCEEDED",
            Self::SddStageGenFailed => "SDD_STAGE_GEN_FAILED",
            Self::SddRollbackTargetInvalid => "SDD_ROLLBACK_TARGET_INVALID",
            Self::SddReviewDecisionInvalid => "SDD_REVIEW_DECISION_INVALID",
            Self::SddReviewNotifyFailed => "SDD_REVIEW_NOTIFY_FAILED",
            Self::SddReviewUnauthorized => "SDD_REVIEW_UNAUTHORIZED",
            Self::SddCapabilityUnauthorized => "SDD_CAPABILITY_UNAUTHORIZED",
            Self::SddMcpToolFailed => "SDD_MCP_TOOL_FAILED",
            Self::SddAiFacadeIncompatible => "SDD_AI_FACADE_INCOMPATIBLE",
            Self::SddTargetNotFound => "SDD_TARGET_NOT_FOUND",
            Self::SddTargetNotCrate => "SDD_TARGET_NOT_CRATE",
            Self::SddLegacyAnalysisTimeout => "SDD_LEGACY_ANALYSIS_TIMEOUT",
            Self::SddStorePermissionDenied => "SDD_STORE_PERMISSION_DENIED",
            Self::SddArtifactVersionNotFound => "SDD_ARTIFACT_VERSION_NOT_FOUND",
            Self::SddCliArgMissing => "SDD_CLI_ARG_MISSING",
            Self::SddCliArgInvalid => "SDD_CLI_ARG_INVALID",
        }
    }
}

impl fmt::Display for SddErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SddError {
    #[error("[{code}] {message}")]
    WithCode { code: SddErrorCode, message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Uuid(#[from] uuid::Error),

    #[error(transparent)]
    Tera(#[from] tera::Error),
}

impl SddError {
    pub fn new(code: SddErrorCode, message: impl Into<String>) -> Self {
        Self::WithCode {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> Option<&SddErrorCode> {
        match self {
            Self::WithCode { code, .. } => Some(code),
            _ => None,
        }
    }

    pub fn is_session_not_found(&self) -> bool {
        matches!(self.code(), Some(SddErrorCode::SddSessionNotFound))
    }

    pub fn is_session_corrupted(&self) -> bool {
        matches!(self.code(), Some(SddErrorCode::SddSessionCorrupted))
    }

    pub fn is_session_limit_exceeded(&self) -> bool {
        matches!(self.code(), Some(SddErrorCode::SddSessionLimitExceeded))
    }
}

impl From<SddErrorCode> for SddError {
    fn from(code: SddErrorCode) -> Self {
        Self::WithCode {
            code,
            message: code.as_str().to_string(),
        }
    }
}

pub type SddResult<T> = std::result::Result<T, SddError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_display_matches_spec() {
        assert_eq!(
            SddErrorCode::SddSessionNotFound.to_string(),
            "SDD_SESSION_NOT_FOUND"
        );
        assert_eq!(
            SddErrorCode::SddStageGenFailed.to_string(),
            "SDD_STAGE_GEN_FAILED"
        );
        assert_eq!(
            SddErrorCode::SddLegacyAnalysisTimeout.to_string(),
            "SDD_LEGACY_ANALYSIS_TIMEOUT"
        );
    }

    #[test]
    fn error_code_serde_roundtrip() {
        let code = SddErrorCode::SddReviewDecisionInvalid;
        let json = serde_json::to_string(&code).unwrap();
        let decoded: SddErrorCode = serde_json::from_str(&json).unwrap();
        assert_eq!(code, decoded);
    }

    #[test]
    fn error_constructors() {
        let err = SddError::new(SddErrorCode::SddSessionNotFound, "session abc not found");
        assert!(err.is_session_not_found());
        assert!(!err.is_session_corrupted());
    }
}
