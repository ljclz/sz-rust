// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 项目语料扫描。

use crate::error::{RagError, RagResult};
use std::path::{Path, PathBuf};

/// 扫描得到的源文件。
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub crate_name: String,
    pub path: PathBuf,
    pub content: String,
}

/// 项目语料扫描器。
pub struct ProjectCorpusScanner;

impl ProjectCorpusScanner {
    /// 扫描 workspace 下所有 crate 的 src/**/*.rs 文件。
    pub async fn scan(workspace_root: &Path) -> RagResult<Vec<SourceFile>> {
        let packages_dir = workspace_root.join("packages");
        if !tokio::fs::try_exists(&packages_dir).await.unwrap_or(false) {
            return Err(RagError::CorpusScanFailed(format!(
                "packages dir not found: {}",
                packages_dir.display()
            )));
        }

        let mut results = Vec::new();
        let mut subdirs = tokio::fs::read_dir(&packages_dir).await?;
        while let Some(entry) = subdirs.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let crate_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            let src_dir = path.join("src");
            if tokio::fs::try_exists(&src_dir).await.unwrap_or(false) {
                Self::scan_dir(&src_dir, &crate_name, &mut results).await?;
            }
        }
        Ok(results)
    }

    /// 仅扫描指定 crate（增量向量化用）。
    pub async fn scan_crate(workspace_root: &Path, crate_name: &str) -> RagResult<Vec<SourceFile>> {
        let src_dir = workspace_root.join("packages").join(crate_name).join("src");
        if !tokio::fs::try_exists(&src_dir).await.unwrap_or(false) {
            return Err(RagError::CorpusScanFailed(format!(
                "crate src not found: {}",
                src_dir.display()
            )));
        }
        let mut results = Vec::new();
        Self::scan_dir(&src_dir, crate_name, &mut results).await?;
        Ok(results)
    }

    async fn scan_dir(
        dir: &Path,
        crate_name: &str,
        results: &mut Vec<SourceFile>,
    ) -> RagResult<()> {
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                Box::pin(Self::scan_dir(&path, crate_name, results)).await?;
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let content = tokio::fs::read_to_string(&path).await?;
                results.push(SourceFile {
                    crate_name: crate_name.to_string(),
                    path,
                    content,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scan_nonexistent() {
        let result = ProjectCorpusScanner::scan(Path::new("/nonexistent/path")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn scan_crate_nonexistent() {
        let result = ProjectCorpusScanner::scan_crate(Path::new("/nonexistent"), "foo").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn scan_real_workspace() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let result = ProjectCorpusScanner::scan(workspace).await;
        assert!(result.is_ok());
        let files = result.unwrap();
        assert!(!files.is_empty(), "workspace should contain rust files");
        assert!(files.iter().any(|f| f.crate_name.contains("sz-rust-rag")));
    }

    #[tokio::test]
    async fn scan_crate_real() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let result = ProjectCorpusScanner::scan_crate(workspace, "sz-rust-rag").await;
        assert!(result.is_ok());
        let files = result.unwrap();
        assert!(!files.is_empty());
        assert!(files.iter().all(|f| f.crate_name == "sz-rust-rag"));
        assert!(files.iter().any(|f| f.path.extension().unwrap() == "rs"));
    }

    #[tokio::test]
    async fn scan_crate_with_subdirs() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let result = ProjectCorpusScanner::scan_crate(workspace, "sz-rust-rag").await;
        let files = result.unwrap();
        assert!(files.iter().any(|f| f
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("lib.rs")));
    }
}
