// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! RagWarningCode 警告码。

/// RAG 警告码。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RagWarningCode {
    LowRecallScore,
    TokenBudgetExceeded,
    EmbeddingPartialFailure,
    ConfigFieldMissing,
    StoreVersionMismatch,
    IncrementalUpdateSkipped,
}

impl RagWarningCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LowRecallScore => "low_recall_score",
            Self::TokenBudgetExceeded => "token_budget_exceeded",
            Self::EmbeddingPartialFailure => "embedding_partial_failure",
            Self::ConfigFieldMissing => "config_field_missing",
            Self::StoreVersionMismatch => "store_version_mismatch",
            Self::IncrementalUpdateSkipped => "incremental_update_skipped",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warning_code_snake_case() {
        assert_eq!(RagWarningCode::LowRecallScore.as_str(), "low_recall_score");
    }

    #[test]
    fn all_warning_codes_as_str() {
        assert_eq!(
            RagWarningCode::TokenBudgetExceeded.as_str(),
            "token_budget_exceeded"
        );
        assert_eq!(
            RagWarningCode::EmbeddingPartialFailure.as_str(),
            "embedding_partial_failure"
        );
        assert_eq!(
            RagWarningCode::ConfigFieldMissing.as_str(),
            "config_field_missing"
        );
        assert_eq!(
            RagWarningCode::StoreVersionMismatch.as_str(),
            "store_version_mismatch"
        );
        assert_eq!(
            RagWarningCode::IncrementalUpdateSkipped.as_str(),
            "incremental_update_skipped"
        );
    }
}
