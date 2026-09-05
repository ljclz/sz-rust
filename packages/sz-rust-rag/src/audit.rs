// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! RAG 审计日志。

use crate::error::{RagError, RagResult};
use crate::redact::SourceCodeRedactor;
use crate::warning::RagWarningCode;
use std::path::PathBuf;

/// 审计日志条目。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RagAuditLog {
    pub trace_id: String,
    pub tenant_id: String,
    pub query_redacted: String,
    pub hit_count: u32,
    pub top_score: f32,
    pub duration_ms: u64,
    pub warnings: Vec<RagWarningCode>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 审计日志器。
pub struct RagAuditLogger {
    path: PathBuf,
    redactor: SourceCodeRedactor,
}

impl RagAuditLogger {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            redactor: SourceCodeRedactor::new(),
        }
    }

    pub fn noop() -> Self {
        Self {
            path: PathBuf::new(),
            redactor: SourceCodeRedactor::new(),
        }
    }

    /// 追加写审计日志（查询文本先脱敏）。
    pub async fn log(&self, mut entry: RagAuditLog) -> RagResult<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        entry.query_redacted = self.redactor.redact(&entry.query_redacted);
        let line = serde_json::to_string(&entry).map_err(RagError::Json)?;
        let line_with_newline = format!("{}\n", line);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        use tokio::io::AsyncWriteExt;
        file.write_all(line_with_newline.as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_logger() {
        let logger = RagAuditLogger::noop();
        let entry = RagAuditLog {
            trace_id: "t1".into(),
            tenant_id: "tenant".into(),
            query_redacted: "query".into(),
            hit_count: 5,
            top_score: 0.9,
            duration_ms: 100,
            warnings: vec![],
            timestamp: chrono::Utc::now(),
        };
        logger.log(entry).await.unwrap();
    }

    #[tokio::test]
    async fn file_logger_redacts() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let logger = RagAuditLogger::new(tmp.path().to_path_buf());
        let entry = RagAuditLog {
            trace_id: "t1".into(),
            tenant_id: "tenant".into(),
            query_redacted: r#"api_key = "sk-1234567890abcdef""#.into(),
            hit_count: 1,
            top_score: 0.5,
            duration_ms: 50,
            warnings: vec![],
            timestamp: chrono::Utc::now(),
        };
        logger.log(entry).await.unwrap();
        let content = tokio::fs::read_to_string(tmp.path()).await.unwrap();

        assert!(content.contains("***REDACTED***"));
        assert!(!content.contains("sk-1234567890abcdef"));
    }
}
