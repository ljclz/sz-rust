// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 文件覆盖保护：文件已存在时暂停 + 请求用户确认。

use crate::error::CodegenError;
use crate::generator::GeneratedFile;
use std::path::Path;

/// 文件覆盖保护。
pub struct FileGuard {
    overwrite: bool,
}

impl FileGuard {
    /// 创建文件保护器。
    ///
    /// `overwrite = true` 时跳过存在检查。
    pub fn new(overwrite: bool) -> Self {
        Self { overwrite }
    }

    /// 检查文件是否可写入。
    pub fn check(&self, path: &Path) -> Result<(), CodegenError> {
        if self.overwrite {
            return Ok(());
        }
        if path.exists() {
            return Err(CodegenError::FileExists(path.display().to_string()));
        }
        Ok(())
    }

    /// 批量检查并写入文件。
    pub async fn write_files(
        &self,
        base_dir: &Path,
        files: &[GeneratedFile],
    ) -> Result<(), CodegenError> {
        for file in files {
            if std::path::Path::new(&file.path)
                .components()
                .any(|c| c == std::path::Component::ParentDir)
            {
                return Err(CodegenError::FileExists(format!(
                    "path traversal detected: {}",
                    file.path
                )));
            }
            let path = base_dir.join(&file.path);
            self.check(&path)?;
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&path, &file.content).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = FileGuard::new(false);
        let files = vec![GeneratedFile {
            path: "test.rs".into(),
            content: "fn main() {}".into(),
        }];
        guard.write_files(tmp.path(), &files).await.unwrap();
        assert!(tmp.path().join("test.rs").exists());
    }

    #[tokio::test]
    async fn refuse_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("exists.rs");
        tokio::fs::write(&path, "old content").await.unwrap();

        let guard = FileGuard::new(false);
        let files = vec![GeneratedFile {
            path: "exists.rs".into(),
            content: "new content".into(),
        }];
        let result = guard.write_files(tmp.path(), &files).await;
        assert!(result.is_err());
        assert_eq!(result.err().unwrap().error_code(), "CODEGEN_FILE_EXISTS");
    }

    #[tokio::test]
    async fn overwrite_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("exists.rs");
        tokio::fs::write(&path, "old content").await.unwrap();

        let guard = FileGuard::new(true);
        let files = vec![GeneratedFile {
            path: "exists.rs".into(),
            content: "new content".into(),
        }];
        guard.write_files(tmp.path(), &files).await.unwrap();
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn write_nested_path() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = FileGuard::new(false);
        let files = vec![GeneratedFile {
            path: "src/nested/deep.rs".into(),
            content: "fn deep() {}".into(),
        }];
        guard.write_files(tmp.path(), &files).await.unwrap();
        assert!(tmp.path().join("src/nested/deep.rs").exists());
    }

    #[tokio::test]
    async fn refuse_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let guard = FileGuard::new(true);
        let files = vec![GeneratedFile {
            path: "../../../etc/passwd".into(),
            content: "malicious".into(),
        }];
        let result = guard.write_files(tmp.path(), &files).await;
        assert!(result.is_err());
    }
}
