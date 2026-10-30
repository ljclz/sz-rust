// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 人工审核：关键场景下生成结果经人工审核后再写入。

use crate::error::CodegenError;
use crate::generator::GeneratedFile;
use async_trait::async_trait;

/// 审核决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecision {
    Approve,
    Reject(String),
}

/// 人工审核回调 trait。
#[async_trait]
pub trait ReviewCallback: Send + Sync {
    /// 审核生成文件，返回批准或拒绝。
    async fn review(&self, files: &[GeneratedFile]) -> ReviewDecision;
}

/// 自动批准（测试用）。
pub struct AutoApprove;

#[async_trait]
impl ReviewCallback for AutoApprove {
    async fn review(&self, _files: &[GeneratedFile]) -> ReviewDecision {
        ReviewDecision::Approve
    }
}

/// 自动拒绝（测试用）。
pub struct AutoReject;

#[async_trait]
impl ReviewCallback for AutoReject {
    async fn review(&self, _files: &[GeneratedFile]) -> ReviewDecision {
        ReviewDecision::Reject("auto reject".into())
    }
}

/// 执行人工审核。
pub async fn require_review(
    callback: &dyn ReviewCallback,
    files: &[GeneratedFile],
) -> Result<(), CodegenError> {
    match callback.review(files).await {
        ReviewDecision::Approve => Ok(()),
        ReviewDecision::Reject(reason) => Err(CodegenError::ReviewRejected(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_file() -> GeneratedFile {
        GeneratedFile {
            path: "test.rs".into(),
            content: "fn main() {}".into(),
        }
    }

    #[tokio::test]
    async fn auto_approve_passes() {
        let callback = AutoApprove;
        let files = vec![make_file()];
        assert!(require_review(&callback, &files).await.is_ok());
    }

    #[tokio::test]
    async fn auto_reject_fails() {
        let callback = AutoReject;
        let files = vec![make_file()];
        let result = require_review(&callback, &files).await;
        assert!(result.is_err());
        assert_eq!(
            result.err().unwrap().error_code(),
            "CODEGEN_REVIEW_REJECTED"
        );
    }
}
