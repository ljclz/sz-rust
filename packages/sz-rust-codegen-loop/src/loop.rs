// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 闭环迭代：生成 → 安全扫描 → 编译验证 → 失败反馈 → 重新生成（最多 N 轮）。

use crate::error::CodegenError;
use crate::generator::{GeneratedFile, Generator};
use crate::parser::CodegenTask;
use crate::security::SecurityScanner;
use crate::validator::{ValidationResult, Validator};
use std::sync::Arc;
use tracing::{info, warn};

/// 闭环迭代状态。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodegenStatus {
    Success,
    Failed,
    MaxIterationsExceeded,
}

/// 闭环迭代结果。
#[derive(Debug, Clone)]
pub struct CodegenResult {
    pub files: Vec<GeneratedFile>,
    pub iterations: u32,
    pub status: CodegenStatus,
    pub last_validation: Option<ValidationResult>,
    pub errors: Vec<String>,
}

/// 闭环迭代配置。
#[derive(Debug, Clone)]
pub struct CodegenLoopConfig {
    pub max_iterations: u32,
    pub skip_validation: bool,
}

impl Default for CodegenLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 3,
            skip_validation: false,
        }
    }
}

/// 代码生成闭环。
pub struct CodegenLoop {
    generator: Arc<Generator>,
    validator: Arc<Validator>,
    config: CodegenLoopConfig,
}

impl CodegenLoop {
    /// 创建闭环。
    pub fn new(
        generator: Arc<Generator>,
        validator: Arc<Validator>,
        config: CodegenLoopConfig,
    ) -> Self {
        Self {
            generator,
            validator,
            config,
        }
    }

    /// 执行闭环迭代。
    ///
    /// 流程：生成 → 安全扫描 → 编译验证 → 失败则反馈重新生成（最多 `max_iterations` 轮）。
    pub async fn run(&self, task: &CodegenTask) -> Result<CodegenResult, CodegenError> {
        let mut errors: Vec<String> = Vec::new();
        let mut best_files: Vec<GeneratedFile> = Vec::new();
        let mut best_validation: Option<ValidationResult> = None;

        for iteration in 1..=self.config.max_iterations {
            info!(iteration, "codegen loop iteration");

            let files = match self.generator.generate(task).await {
                Ok(f) => f,
                Err(e) => {
                    let msg = format!("iteration {iteration}: generation failed: {e}");
                    warn!("{msg}");
                    errors.push(msg);
                    continue;
                }
            };

            if let Err(e) = SecurityScanner::scan_and_guard(&files) {
                let msg = format!("iteration {iteration}: security violation: {e}");
                warn!("{msg}");
                errors.push(msg);
                best_files = files;
                continue;
            }

            if self.config.skip_validation {
                return Ok(CodegenResult {
                    files,
                    iterations: iteration,
                    status: CodegenStatus::Success,
                    last_validation: None,
                    errors,
                });
            }

            let validation = self.validator.validate(&files).await?;
            if validation.all_passed() {
                return Ok(CodegenResult {
                    files,
                    iterations: iteration,
                    status: CodegenStatus::Success,
                    last_validation: Some(validation),
                    errors,
                });
            }

            let msg = format!(
                "iteration {iteration}: validation failed (check: {}, test: {})",
                validation.check_passed, validation.test_passed
            );
            warn!("{msg}");
            errors.push(msg);
            best_files = files;
            best_validation = Some(validation);
        }

        Ok(CodegenResult {
            files: best_files,
            iterations: self.config.max_iterations,
            status: CodegenStatus::MaxIterationsExceeded,
            last_validation: best_validation,
            errors,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default() {
        let config = CodegenLoopConfig::default();
        assert_eq!(config.max_iterations, 3);
        assert!(!config.skip_validation);
    }

    #[test]
    fn codegen_status_serde() {
        let s = serde_json::to_string(&CodegenStatus::Success).unwrap();
        assert_eq!(s, "\"success\"");
        let s: CodegenStatus = serde_json::from_str("\"failed\"").unwrap();
        assert_eq!(s, CodegenStatus::Failed);
    }

    #[test]
    fn codegen_result_status_variants() {
        let variants = [
            CodegenStatus::Success,
            CodegenStatus::Failed,
            CodegenStatus::MaxIterationsExceeded,
        ];
        assert_eq!(variants.len(), 3);
    }
}
