use thiserror::Error;

#[derive(Debug, Error)]
pub enum AiError {
    #[error("provider auth failed: {0}")]
    ProviderAuthFailed(String),
    #[error("provider unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("provider timeout: {0}")]
    ProviderTimeout(String),
    #[error("rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("context truncated: {before} -> {after} tokens")]
    ContextTruncated { before: u32, after: u32 },
    #[error("embedding dimension inconsistent: expected {expected}, got {actual}")]
    EmbedDimInconsistent { expected: usize, actual: usize },
    #[error("vector dimension mismatch: expected {expected}, got {actual}")]
    VectorDimMismatch { expected: usize, actual: usize },
    #[error("vector store unavailable: {0}")]
    VectorStoreUnavailable(String),
    #[error("local model load failed: {0}")]
    LocalModelLoadFailed(String),
    #[error("mcp unreachable: {0}")]
    McpUnreachable(String),
    #[error("tool not authorized: {0}")]
    ToolNotAuthorized(String),
    #[error("tool execution error: {0}")]
    ToolExecution(String),
    #[error("agent max steps ({max_steps}) exceeded")]
    AgentMaxSteps { max_steps: u32 },
    #[error("config invalid: {0}")]
    ConfigInvalid(String),
    #[error("cache error: {0}")]
    Cache(#[from] anyhow::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

impl AiError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::ProviderAuthFailed(_) => "AI_PROVIDER_AUTH_FAILED",
            Self::ProviderUnavailable(_) => "AI_PROVIDER_UNAVAILABLE",
            Self::ProviderTimeout(_) => "AI_PROVIDER_TIMEOUT",
            Self::RateLimited { .. } => "AI_RATE_LIMITED",
            Self::ContextTruncated { .. } => "AI_CONTEXT_TRUNCATED",
            Self::EmbedDimInconsistent { .. } => "AI_EMBED_DIM_INCONSISTENT",
            Self::VectorDimMismatch { .. } => "AI_VECTOR_DIM_MISMATCH",
            Self::VectorStoreUnavailable(_) => "AI_VECTOR_STORE_UNAVAILABLE",
            Self::LocalModelLoadFailed(_) => "AI_LOCAL_MODEL_LOAD_FAILED",
            Self::McpUnreachable(_) => "AI_MCP_UNREACHABLE",
            Self::ToolNotAuthorized(_) => "AI_TOOL_NOT_AUTHORIZED",
            Self::ToolExecution(_) => "AI_TOOL_EXECUTION",
            Self::AgentMaxSteps { .. } => "AI_AGENT_MAX_STEPS",
            Self::ConfigInvalid(_) => "AI_CONFIG_INVALID",
            Self::Cache(_) => "AI_CACHE",
            Self::Http(_) => "AI_HTTP",
            Self::Json(_) => "AI_JSON",
            Self::Internal(_) => "AI_INTERNAL",
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ProviderUnavailable(_)
                | Self::ProviderTimeout(_)
                | Self::RateLimited { .. }
                | Self::McpUnreachable(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_all_variants() {
        assert_eq!(
            AiError::ProviderAuthFailed("x".into()).error_code(),
            "AI_PROVIDER_AUTH_FAILED"
        );
        assert_eq!(
            AiError::ProviderUnavailable("x".into()).error_code(),
            "AI_PROVIDER_UNAVAILABLE"
        );
        assert_eq!(
            AiError::ProviderTimeout("x".into()).error_code(),
            "AI_PROVIDER_TIMEOUT"
        );
        assert_eq!(
            AiError::RateLimited {
                retry_after_ms: 100
            }
            .error_code(),
            "AI_RATE_LIMITED"
        );
        assert_eq!(
            AiError::ContextTruncated {
                before: 10,
                after: 5
            }
            .error_code(),
            "AI_CONTEXT_TRUNCATED"
        );
        assert_eq!(
            AiError::EmbedDimInconsistent {
                expected: 768,
                actual: 512
            }
            .error_code(),
            "AI_EMBED_DIM_INCONSISTENT"
        );
        assert_eq!(
            AiError::VectorDimMismatch {
                expected: 768,
                actual: 512
            }
            .error_code(),
            "AI_VECTOR_DIM_MISMATCH"
        );
        assert_eq!(
            AiError::VectorStoreUnavailable("x".into()).error_code(),
            "AI_VECTOR_STORE_UNAVAILABLE"
        );
        assert_eq!(
            AiError::LocalModelLoadFailed("x".into()).error_code(),
            "AI_LOCAL_MODEL_LOAD_FAILED"
        );
        assert_eq!(
            AiError::McpUnreachable("x".into()).error_code(),
            "AI_MCP_UNREACHABLE"
        );
        assert_eq!(
            AiError::ToolNotAuthorized("x".into()).error_code(),
            "AI_TOOL_NOT_AUTHORIZED"
        );
        assert_eq!(
            AiError::ToolExecution("x".into()).error_code(),
            "AI_TOOL_EXECUTION"
        );
        assert_eq!(
            AiError::AgentMaxSteps { max_steps: 25 }.error_code(),
            "AI_AGENT_MAX_STEPS"
        );
        assert_eq!(
            AiError::ConfigInvalid("x".into()).error_code(),
            "AI_CONFIG_INVALID"
        );
        assert_eq!(AiError::Internal("x".into()).error_code(), "AI_INTERNAL");
    }

    #[test]
    fn is_retryable_true() {
        assert!(AiError::ProviderUnavailable("x".into()).is_retryable());
        assert!(AiError::ProviderTimeout("x".into()).is_retryable());
        assert!(AiError::RateLimited { retry_after_ms: 0 }.is_retryable());
        assert!(AiError::McpUnreachable("x".into()).is_retryable());
    }

    #[test]
    fn is_retryable_false() {
        assert!(!AiError::ProviderAuthFailed("x".into()).is_retryable());
        assert!(!AiError::ContextTruncated {
            before: 1,
            after: 1
        }
        .is_retryable());
        assert!(!AiError::ToolNotAuthorized("x".into()).is_retryable());
        assert!(!AiError::AgentMaxSteps { max_steps: 1 }.is_retryable());
        assert!(!AiError::ConfigInvalid("x".into()).is_retryable());
        assert!(!AiError::Internal("x".into()).is_retryable());
    }

    #[test]
    fn error_display() {
        let e = AiError::RateLimited {
            retry_after_ms: 500,
        };
        assert!(e.to_string().contains("500"));
        let e2 = AiError::VectorDimMismatch {
            expected: 768,
            actual: 512,
        };
        assert!(e2.to_string().contains("768"));
        assert!(e2.to_string().contains("512"));
    }
}
