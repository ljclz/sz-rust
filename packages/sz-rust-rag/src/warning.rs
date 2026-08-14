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
}
