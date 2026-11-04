// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use sz_rust_ai_facade::common::AiError;
use sz_rust_sdd::error::SddError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MigrationError {
    #[error("TP6 project path not found: {0}")]
    Tp6PathNotFound(String),
    #[error("TP6 project format invalid: {0}")]
    Tp6FormatInvalid(String),
    #[error("TP6 version unsupported: {0}, requires >= 6.0.0")]
    Tp6VersionUnsupported(String),
    #[error("analysis timeout after {0} seconds")]
    AnalysisTimeout(u64),
    #[error("generation precondition not met: {0}")]
    GenerationPreconditionNotMet(String),
    #[error("generation analysis report not found")]
    GenerationAnalysisNotFound,
    #[error("generation compile-fix exhausted for interface {interface_id} (max retries: {max_retries})")]
    GenerationCompileFixExhausted {
        interface_id: String,
        max_retries: u32,
    },
    #[error("generation target not writable: {0}")]
    GenerationTargetNotWritable(String),
    #[error("validation precondition not met: {0}")]
    ValidationPreconditionNotMet(String),
    #[error("validation TP6 service unreachable: {0}")]
    ValidationTp6Unreachable(String),
    #[error("validation sz-rust service unreachable: {0}")]
    ValidationSzUnreachable(String),
    #[error("validation timeout for interface {0}")]
    ValidationTimeout(String),
    #[error("report persist failed: {0}")]
    ReportPersistFailed(String),
    #[error("case document persist failed: {0}")]
    CasePersistFailed(String),
    #[error("path invalid (contains ..): {0}")]
    PathInvalid(String),
    #[error("config invalid: {0}")]
    ConfigInvalid(String),
    #[error("SDD agent unavailable: {0}")]
    SddAgentUnavailable(String),
    #[error("SDD error: {0}")]
    Sdd(#[from] SddError),
    #[error("LLM error: {0}")]
    Llm(#[from] AiError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

impl MigrationError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Tp6PathNotFound(_) => "MIGRATION_TP6_PATH_NOT_FOUND",
            Self::Tp6FormatInvalid(_) => "MIGRATION_TP6_FORMAT_INVALID",
            Self::Tp6VersionUnsupported(_) => "MIGRATION_TP6_VERSION_UNSUPPORTED",
            Self::AnalysisTimeout(_) => "MIGRATION_ANALYSIS_TIMEOUT",
            Self::GenerationPreconditionNotMet(_) => "MIGRATION_GENERATION_PRECONDITION_NOT_MET",
            Self::GenerationAnalysisNotFound => "MIGRATION_GENERATION_ANALYSIS_NOT_FOUND",
            Self::GenerationCompileFixExhausted { .. } => {
                "MIGRATION_GENERATION_COMPILE_FIX_EXHAUSTED"
            }
            Self::GenerationTargetNotWritable(_) => "MIGRATION_GENERATION_TARGET_NOT_WRITABLE",
            Self::ValidationPreconditionNotMet(_) => "MIGRATION_VALIDATION_PRECONDITION_NOT_MET",
            Self::ValidationTp6Unreachable(_) => "MIGRATION_VALIDATION_TP6_UNREACHABLE",
            Self::ValidationSzUnreachable(_) => "MIGRATION_VALIDATION_SZ_UNREACHABLE",
            Self::ValidationTimeout(_) => "MIGRATION_VALIDATION_TIMEOUT",
            Self::ReportPersistFailed(_) => "MIGRATION_REPORT_PERSIST_FAILED",
            Self::CasePersistFailed(_) => "MIGRATION_CASE_PERSIST_FAILED",
            Self::PathInvalid(_) => "MIGRATION_PATH_INVALID",
            Self::ConfigInvalid(_) => "MIGRATION_CONFIG_INVALID",
            Self::SddAgentUnavailable(_) => "MIGRATION_SDD_AGENT_UNAVAILABLE",
            Self::Sdd(_) => "MIGRATION_SDD_ERROR",
            Self::Llm(_) => "MIGRATION_LLM_ERROR",
            Self::Io(_) => "MIGRATION_IO_ERROR",
            Self::Json(_) => "MIGRATION_JSON_ERROR",
            Self::Http(_) => "MIGRATION_HTTP_ERROR",
            Self::Toml(_) => "MIGRATION_TOML_ERROR",
            Self::Internal(_) => "MIGRATION_INTERNAL",
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            Self::AnalysisTimeout(_)
            | Self::ValidationTp6Unreachable(_)
            | Self::ValidationSzUnreachable(_)
            | Self::ValidationTimeout(_)
            | Self::Http(_) => true,
            Self::Sdd(_) | Self::Llm(_) => false,
            _ => false,
        }
    }
}

pub type MigrationResult<T> = std::result::Result<T, MigrationError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_stability() {
        assert_eq!(
            MigrationError::Tp6PathNotFound("x".into()).error_code(),
            "MIGRATION_TP6_PATH_NOT_FOUND"
        );
        assert_eq!(
            MigrationError::AnalysisTimeout(30).error_code(),
            "MIGRATION_ANALYSIS_TIMEOUT"
        );
        assert_eq!(
            MigrationError::ConfigInvalid("bad".into()).error_code(),
            "MIGRATION_CONFIG_INVALID"
        );
    }

    #[test]
    fn is_retryable_logic() {
        assert!(MigrationError::AnalysisTimeout(10).is_retryable());
        assert!(MigrationError::ValidationTimeout("iface".into()).is_retryable());
        assert!(!MigrationError::Tp6PathNotFound("x".into()).is_retryable());
        assert!(!MigrationError::ConfigInvalid("bad".into()).is_retryable());
        assert!(!MigrationError::PathInvalid("../".into()).is_retryable());
    }
}
