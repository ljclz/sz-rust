//! RagError 统一错误类型。

use sz_rust_ai_facade::common::AiError;
use sz_rust_capability::CapError;

/// RAG 统一错误类型。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RagError {
    #[error("config invalid: {0}")]
    ConfigInvalid(String),
    #[error("corpus scan failed: {0}")]
    CorpusScanFailed(String),
    #[error("chunking failed: {0}")]
    ChunkingFailed(String),
    #[error("embedding error: {0}")]
    Embedding(#[from] AiError),
    #[error("vector store error: {0}")]
    VectorStore(String),
    #[error("store load failed: {0}")]
    StoreLoadFailed(String),
    #[error("store save failed: {0}")]
    StoreSaveFailed(String),
    #[error("term not found: {0}")]
    TermNotFound(String),
    #[error("rule not found: {0}")]
    RuleNotFound(String),
    #[error("template not found: {0}")]
    TemplateNotFound(String),
    #[error("search failed: {0}")]
    SearchFailed(String),
    #[error("capability error: {0}")]
    Capability(#[from] CapError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

impl RagError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid(_) => "RAG_CONFIG_INVALID",
            Self::CorpusScanFailed(_) => "RAG_CORPUS_SCAN_FAILED",
            Self::ChunkingFailed(_) => "RAG_CHUNKING_FAILED",
            Self::Embedding(_) => "RAG_EMBEDDING",
            Self::VectorStore(_) => "RAG_VECTOR_STORE",
            Self::StoreLoadFailed(_) => "RAG_STORE_LOAD_FAILED",
            Self::StoreSaveFailed(_) => "RAG_STORE_SAVE_FAILED",
            Self::TermNotFound(_) => "RAG_TERM_NOT_FOUND",
            Self::RuleNotFound(_) => "RAG_RULE_NOT_FOUND",
            Self::TemplateNotFound(_) => "RAG_TEMPLATE_NOT_FOUND",
            Self::SearchFailed(_) => "RAG_SEARCH_FAILED",
            Self::Capability(_) => "RAG_CAPABILITY",
            Self::Io(_) => "RAG_IO",
            Self::Json(_) => "RAG_JSON",
            Self::Toml(_) => "RAG_TOML",
            Self::Notify(_) => "RAG_NOTIFY",
            Self::Internal(_) => "RAG_INTERNAL",
        }
    }

    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Embedding(inner) => inner.is_retryable(),
            Self::VectorStore(_) | Self::Io(_) => true,
            _ => false,
        }
    }
}

pub type RagResult<T> = Result<T, RagError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_prefix() {
        assert!(RagError::ConfigInvalid("x".into())
            .error_code()
            .starts_with("RAG_"));
        assert!(RagError::Internal("x".into())
            .error_code()
            .starts_with("RAG_"));
    }

    #[test]
    fn is_retryable_correct() {
        assert!(RagError::VectorStore("x".into()).is_retryable());
        assert!(!RagError::ConfigInvalid("x".into()).is_retryable());
    }
}
