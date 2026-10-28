// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 代码生成错误类型。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodegenError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("generation error: {0}")]
    Generation(String),
    #[error("security violation: {0}")]
    SecurityViolation(String),
    #[error("validation failed: {0}")]
    ValidationFailed(String),
    #[error("max iterations ({0}) exceeded")]
    MaxIterationsExceeded(u32),
    #[error("human review rejected: {0}")]
    ReviewRejected(String),
    #[error("file exists and overwrite not confirmed: {0}")]
    FileExists(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ai error: {0}")]
    Ai(#[from] sz_rust_ai_facade::common::AiError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl CodegenError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Parse(_) => "CODEGEN_PARSE",
            Self::Generation(_) => "CODEGEN_GENERATION",
            Self::SecurityViolation(_) => "CODEGEN_SECURITY_VIOLATION",
            Self::ValidationFailed(_) => "CODEGEN_VALIDATION_FAILED",
            Self::MaxIterationsExceeded(_) => "CODEGEN_MAX_ITERATIONS",
            Self::ReviewRejected(_) => "CODEGEN_REVIEW_REJECTED",
            Self::FileExists(_) => "CODEGEN_FILE_EXISTS",
            Self::Io(_) => "CODEGEN_IO",
            Self::Ai(_) => "CODEGEN_AI",
            Self::Json(_) => "CODEGEN_JSON",
        }
    }
}
