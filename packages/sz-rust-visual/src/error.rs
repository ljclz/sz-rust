// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 可视化画布错误枚举

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 可视化画布错误
#[derive(Debug, Clone, Serialize, Deserialize, Error)]
pub enum VisualError {
    /// SDD Agent 错误
    #[error("SDD error: {0}")]
    SddError(String),

    /// Capability 错误
    #[error("Capability error: {0}")]
    CapError(String),

    /// RAG 错误
    #[error("RAG error: {0}")]
    RagError(String),

    /// IO 错误
    #[error("IO error: {0}")]
    IoError(String),

    /// Tauri 错误
    #[error("Tauri error: {0}")]
    TauriError(String),

    /// 内部错误
    #[error("Internal error: {0}")]
    InternalError(String),

    /// 产物未找到
    #[error("Artifact not found: {0}")]
    ArtifactNotFound(String),

    /// 编排未找到
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    /// 编排状态非法
    #[error("Invalid session status: {0}")]
    InvalidStatus(String),
}

/// 可视化画布结果
pub type VisualResult<T> = Result<T, VisualError>;

impl VisualError {
    /// 稳定错误码
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::SddError(_) => "SDD_ERROR",
            Self::CapError(_) => "CAP_ERROR",
            Self::RagError(_) => "RAG_ERROR",
            Self::IoError(_) => "IO_ERROR",
            Self::TauriError(_) => "TAURI_ERROR",
            Self::InternalError(_) => "INTERNAL_ERROR",
            Self::ArtifactNotFound(_) => "ARTIFACT_NOT_FOUND",
            Self::SessionNotFound(_) => "SESSION_NOT_FOUND",
            Self::InvalidStatus(_) => "INVALID_STATUS",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_stability() {
        assert_eq!(VisualError::SddError("x".into()).error_code(), "SDD_ERROR");
        assert_eq!(VisualError::CapError("x".into()).error_code(), "CAP_ERROR");
        assert_eq!(VisualError::RagError("x".into()).error_code(), "RAG_ERROR");
        assert_eq!(VisualError::IoError("x".into()).error_code(), "IO_ERROR");
        assert_eq!(
            VisualError::TauriError("x".into()).error_code(),
            "TAURI_ERROR"
        );
        assert_eq!(
            VisualError::InternalError("x".into()).error_code(),
            "INTERNAL_ERROR"
        );
        assert_eq!(
            VisualError::ArtifactNotFound("x".into()).error_code(),
            "ARTIFACT_NOT_FOUND"
        );
        assert_eq!(
            VisualError::SessionNotFound("x".into()).error_code(),
            "SESSION_NOT_FOUND"
        );
        assert_eq!(
            VisualError::InvalidStatus("x".into()).error_code(),
            "INVALID_STATUS"
        );
    }

    #[test]
    fn test_error_display() {
        let err = VisualError::SddError("phase failed".to_string());
        assert!(err.to_string().contains("SDD error"));
        assert!(err.to_string().contains("phase failed"));
    }
}
