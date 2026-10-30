// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 编译验证 + 测试验证：对生成代码执行 cargo check 和 cargo test。

use crate::error::CodegenError;
use crate::generator::GeneratedFile;
use std::path::Path;

/// 验证结果。
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub check_passed: bool,
    pub test_passed: bool,
    pub check_output: String,
    pub test_output: String,
}

impl ValidationResult {
    pub fn all_passed(&self) -> bool {
        self.check_passed && self.test_passed
    }
}

/// 代码验证器：执行 cargo check + cargo test。
pub struct Validator;

impl Default for Validator {
    fn default() -> Self {
        Self::new()
    }
}

impl Validator {
    /// 创建验证器。
    pub fn new() -> Self {
        Self
    }

    /// 将生成文件写入临时目录并验证。
    pub async fn validate(
        &self,
        files: &[GeneratedFile],
    ) -> Result<ValidationResult, CodegenError> {
        let tmpdir = tempfile::tempdir()?;
        let dir = tmpdir.path();

        for file in files {
            let path = dir.join(&file.path);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&path, &file.content).await?;
        }

        let check_result = self.run_cargo_check(dir).await;
        let test_result = self.run_cargo_test(dir).await;

        Ok(ValidationResult {
            check_passed: check_result.0,
            test_passed: test_result.0,
            check_output: check_result.1,
            test_output: test_result.1,
        })
    }

    async fn run_cargo_check(&self, dir: &Path) -> (bool, String) {
        let output = tokio::process::Command::new("cargo")
            .arg("check")
            .current_dir(dir)
            .output()
            .await;

        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                (o.status.success(), format!("{stdout}\n{stderr}"))
            }
            Err(e) => (false, format!("failed to run cargo check: {e}")),
        }
    }

    async fn run_cargo_test(&self, dir: &Path) -> (bool, String) {
        let output = tokio::process::Command::new("cargo")
            .arg("test")
            .current_dir(dir)
            .output()
            .await;

        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout).to_string();
                let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                (o.status.success(), format!("{stdout}\n{stderr}"))
            }
            Err(e) => (false, format!("failed to run cargo test: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_result_all_passed() {
        let r = ValidationResult {
            check_passed: true,
            test_passed: true,
            check_output: "".into(),
            test_output: "".into(),
        };
        assert!(r.all_passed());
    }

    #[test]
    fn validation_result_partial_failure() {
        let r = ValidationResult {
            check_passed: true,
            test_passed: false,
            check_output: "".into(),
            test_output: "test failed".into(),
        };
        assert!(!r.all_passed());
    }

    #[tokio::test]
    async fn validate_empty_files() {
        let v = Validator::new();
        let result = v.validate(&[]).await.unwrap();
        assert!(!result.check_passed || result.check_output.contains("error"));
    }
}
