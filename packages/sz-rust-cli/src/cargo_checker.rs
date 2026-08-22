//! cargo check 编译验证模块
//!
//! 对应 design.md 第 2.2.2.4 节，异步执行 `cargo check` 验证生成的插件骨架。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::CliError;

/// cargo check 执行结果
#[derive(Debug, Clone)]
pub struct CargoCheckResult {
    /// 编译是否成功
    pub success: bool,
    /// 编译错误列表
    pub errors: Vec<String>,
    /// 编译警告列表
    pub warnings: Vec<String>,
}

/// cargo check 执行器
pub struct CargoChecker;

/// cargo check 超时时间（30 秒，对齐铁律 5）
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

impl CargoChecker {
    /// 对指定目录执行 `cargo check`
    ///
    /// 使用 `tokio::process::Command` 异步执行，超时 30 秒。
    ///
    /// # 错误
    ///
    /// - `CliError::Generic("cargo not found")`：cargo 命令不存在
    /// - `CliError::Generic("cargo check timeout")`：执行超时
    pub async fn check(plugin_root: &Path) -> Result<CargoCheckResult, CliError> {
        let output = tokio::time::timeout(
            CHECK_TIMEOUT,
            tokio::process::Command::new("cargo")
                .arg("check")
                .current_dir(plugin_root)
                .output(),
        )
        .await
        .map_err(|_| CliError::Generic("cargo check timeout".to_string()))?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CliError::Generic("cargo not found".to_string())
            } else {
                CliError::Io(e)
            }
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        for line in stderr.lines() {
            if line.starts_with("error") || line.contains("error[") {
                errors.push(line.to_string());
            } else if line.starts_with("warning") {
                warnings.push(line.to_string());
            }
        }

        for line in stdout.lines() {
            if line.starts_with("error") || line.contains("error[") {
                errors.push(line.to_string());
            } else if line.starts_with("warning") {
                warnings.push(line.to_string());
            }
        }

        let success = output.status.success() && errors.is_empty();

        Ok(CargoCheckResult {
            success,
            errors,
            warnings,
        })
    }

    /// 回滚已写入的文件
    ///
    /// 遍历文件列表逐个删除，删除失败不阻塞流程（记录警告日志）。
    pub async fn rollback(files: &[PathBuf]) -> Vec<(PathBuf, std::io::Error)> {
        let mut failures = Vec::new();
        for file in files {
            if let Err(e) = tokio::fs::remove_file(file).await {
                eprintln!("Warning: failed to remove {}: {e}", file.display());
                failures.push((file.clone(), e));
            }
        }
        failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_check_nonexistent_dir() {
        let result = CargoChecker::check(Path::new("/nonexistent/path/12345")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore = "需要完整 workspace + sz-orm 路径依赖，CI 中由 Check job 覆盖"]
    async fn test_check_current_workspace() {
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let result = CargoChecker::check(&workspace_root).await;
        assert!(
            result.is_ok(),
            "cargo check should succeed: {:?}",
            result.err()
        );
        let check_result = result.unwrap();
        assert!(check_result.success, "workspace should compile");
    }

    #[tokio::test]
    async fn test_rollback_empty_list() {
        let failures = CargoChecker::rollback(&[]).await;
        assert!(failures.is_empty());
    }

    #[tokio::test]
    async fn test_rollback_nonexistent_file() {
        let temp = tempfile::tempdir().expect("tempdir failed");
        let nonexistent = temp.path().join("nonexistent.rs");
        let failures = CargoChecker::rollback(&[nonexistent]).await;
        assert_eq!(failures.len(), 1);
    }

    #[tokio::test]
    async fn test_rollback_existing_file() {
        let temp = tempfile::tempdir().expect("tempdir failed");
        let file = temp.path().join("test.txt");
        tokio::fs::write(&file, "test content")
            .await
            .expect("write failed");
        assert!(file.exists());

        let failures = CargoChecker::rollback(std::slice::from_ref(&file)).await;
        assert!(failures.is_empty());
        assert!(!file.exists());
    }
}
