// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::Path;

use crate::error::MigrationResult;

pub struct CodeWriter;

impl CodeWriter {
    pub async fn write_file(
        target_path: &Path,
        content: &str,
        allow_overwrite: bool,
    ) -> MigrationResult<()> {
        if target_path.exists() && !allow_overwrite {
            return Err(crate::error::MigrationError::GenerationTargetNotWritable(
                format!("file already exists: {}", target_path.display()),
            ));
        }
        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(target_path, content).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_new_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("handler.rs");
        CodeWriter::write_file(&path, "fn handler() {}", false)
            .await
            .unwrap();
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "fn handler() {}");
    }

    #[tokio::test]
    async fn refuse_overwrite_without_flag() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("existing.rs");
        tokio::fs::write(&path, "original").await.unwrap();
        let result = CodeWriter::write_file(&path, "new", false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn allow_overwrite_with_flag() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("existing.rs");
        tokio::fs::write(&path, "original").await.unwrap();
        CodeWriter::write_file(&path, "new", true).await.unwrap();
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "new");
    }
}
