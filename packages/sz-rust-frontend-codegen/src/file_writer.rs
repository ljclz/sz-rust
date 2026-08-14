//! 原子文件写入

use std::path::{Path, PathBuf};

use crate::config::OverrideStrategy;
use crate::error::FrontendCodegenError;
use crate::path_guard::PathGuard;
use crate::report::{GeneratedFile, SkippedFile};

/// 文件写入器
pub struct FileWriter;

/// 批量写入结果
pub struct WriteResult {
    /// 成功写入的文件
    pub success: Vec<GeneratedFile>,
    /// 跳过的文件
    pub skipped: Vec<SkippedFile>,
    /// 失败的文件
    pub failed: Vec<(PathBuf, String)>,
}

impl FileWriter {
    /// 原子写入文件（临时文件 + rename）
    pub async fn write_atomic(path: &Path, content: &str) -> Result<(), FrontendCodegenError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, content).await?;
        tokio::fs::rename(&tmp, path).await?;
        Ok(())
    }

    /// 批量写入与覆盖策略
    pub async fn write_batch(
        files: Vec<(PathBuf, String)>,
        output_dir: &Path,
        strategy: OverrideStrategy,
    ) -> Result<WriteResult, FrontendCodegenError> {
        let mut success = Vec::new();
        let mut skipped = Vec::new();
        let mut failed = Vec::new();

        for (rel_path, content) in files {
            PathGuard::validate(&rel_path, Path::new("."))?;
            let full_path = output_dir.join(&rel_path);
            let exists = tokio::fs::try_exists(&full_path).await.unwrap_or(false);

            if exists && strategy == OverrideStrategy::Skip {
                skipped.push(SkippedFile {
                    path: rel_path,
                    reason: "文件已存在（Skip 策略）".to_string(),
                });
                continue;
            }

            let size_bytes = content.len() as u64;
            match Self::write_atomic(&full_path, &content).await {
                Ok(()) => success.push(GeneratedFile {
                    path: rel_path,
                    size_bytes,
                    source_model: String::new(),
                    source_template: String::new(),
                    is_overwritten: exists,
                }),
                Err(e) => failed.push((rel_path, e.to_string())),
            }
        }

        Ok(WriteResult {
            success,
            skipped,
            failed,
        })
    }
}
