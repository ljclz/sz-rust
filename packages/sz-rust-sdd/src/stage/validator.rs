// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use async_trait::async_trait;

use crate::error::SddResult;
use crate::model::{Artifact, Stage, ValidationResult};

#[async_trait]
pub trait ArtifactValidator: Send + Sync + 'static {
    fn stage(&self) -> Stage;
    async fn validate(&self, artifact: &Artifact) -> SddResult<ValidationResult>;
}

const SPEC_REQUIRED_SECTIONS: &[&str] = &[
    "组件定位",
    "领域术语",
    "角色边界",
    "DFX",
    "核心能力",
    "数据约束",
];

const DESIGN_REQUIRED_SECTION: &str = "需求与存量功能关系分析";

const FORBIDDEN_PATTERNS: &[&str] = &["unsafe", "std::fs", "#![allow(dead_code)]"];

pub struct SpecValidator;

impl SpecValidator {
    pub fn new() -> Self {
        Self
    }

    fn check_sections(content: &str) -> Vec<String> {
        let mut violations = Vec::new();
        for section in SPEC_REQUIRED_SECTIONS {
            if !content.contains(section) {
                violations.push(format!("缺少章节: {section}"));
            }
        }
        violations
    }
}

impl Default for SpecValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ArtifactValidator for SpecValidator {
    fn stage(&self) -> Stage {
        Stage::Spec
    }

    async fn validate(&self, artifact: &Artifact) -> SddResult<ValidationResult> {
        let violations = Self::check_sections(&artifact.content);
        Ok(if violations.is_empty() {
            ValidationResult::passed()
        } else {
            ValidationResult::failed(violations)
        })
    }
}

pub struct DesignValidator;

impl DesignValidator {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DesignValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ArtifactValidator for DesignValidator {
    fn stage(&self) -> Stage {
        Stage::Design
    }

    async fn validate(&self, artifact: &Artifact) -> SddResult<ValidationResult> {
        if artifact.content.contains(DESIGN_REQUIRED_SECTION) {
            Ok(ValidationResult::passed())
        } else {
            Ok(ValidationResult::failed(vec![format!(
                "缺少章节: {DESIGN_REQUIRED_SECTION}"
            )]))
        }
    }
}

pub struct TaskValidator;

impl TaskValidator {
    pub fn new() -> Self {
        Self
    }

    fn check_tasks(content: &str) -> Vec<String> {
        let mut violations = Vec::new();
        let task_lines: Vec<&str> = content
            .lines()
            .filter(|l| l.trim_start().starts_with("- [ ]") || l.trim_start().starts_with("- [x]"))
            .collect();

        if task_lines.is_empty() {
            violations.push("未找到任务条目（- [ ] 或 - [x]）".to_string());
            return violations;
        }

        let has_command =
            content.contains("cargo") || content.contains("npm") || content.contains("命令");
        let has_validation =
            content.contains("验证") || content.contains("test") || content.contains("check");

        if !has_command {
            violations.push("任务缺少可执行命令".to_string());
        }
        if !has_validation {
            violations.push("任务缺少验证方法".to_string());
        }

        violations
    }
}

impl Default for TaskValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ArtifactValidator for TaskValidator {
    fn stage(&self) -> Stage {
        Stage::Task
    }

    async fn validate(&self, artifact: &Artifact) -> SddResult<ValidationResult> {
        let violations = Self::check_tasks(&artifact.content);
        Ok(if violations.is_empty() {
            ValidationResult::passed()
        } else {
            ValidationResult::failed(violations)
        })
    }
}

pub struct CodingValidator;

impl CodingValidator {
    pub fn new() -> Self {
        Self
    }

    fn check_forbidden_patterns(content: &str) -> Vec<String> {
        let mut violations = Vec::new();
        for pattern in FORBIDDEN_PATTERNS {
            if content.contains(pattern) {
                violations.push(format!("代码包含禁止模式: {pattern}"));
            }
        }
        violations
    }
}

impl Default for CodingValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ArtifactValidator for CodingValidator {
    fn stage(&self) -> Stage {
        Stage::Coding
    }

    async fn validate(&self, artifact: &Artifact) -> SddResult<ValidationResult> {
        let violations = Self::check_forbidden_patterns(&artifact.content);
        Ok(if violations.is_empty() {
            ValidationResult::passed()
        } else {
            ValidationResult::failed(violations)
        })
    }
}

pub fn default_validators() -> Vec<Box<dyn ArtifactValidator>> {
    vec![
        Box::new(SpecValidator::new()),
        Box::new(DesignValidator::new()),
        Box::new(TaskValidator::new()),
        Box::new(CodingValidator::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ArtifactMeta, ArtifactVersion, SessionId};

    fn make_artifact(stage: Stage, content: &str) -> Artifact {
        Artifact {
            session_id: SessionId::new(),
            stage,
            version: ArtifactVersion::initial(),
            content: content.to_string(),
            meta: ArtifactMeta::new(stage),
        }
    }

    #[tokio::test]
    async fn artifact_validator_spec_pass() {
        let validator = SpecValidator::new();
        let content = r#"
# 组件定位
定位说明

# 领域术语
术语表

# 角色边界
角色说明

# DFX 约束
性能约束

# 核心能力
能力列表

# 数据约束
数据模型
"#;
        let artifact = make_artifact(Stage::Spec, content);
        let result = validator.validate(&artifact).await.unwrap();
        assert!(result.passed);
    }

    #[tokio::test]
    async fn artifact_validator_spec_fail() {
        let validator = SpecValidator::new();
        let artifact = make_artifact(Stage::Spec, "# Only one section");
        let result = validator.validate(&artifact).await.unwrap();
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.contains("组件定位")));
    }

    #[tokio::test]
    async fn artifact_validator_design_pass() {
        let validator = DesignValidator::new();
        let content = "# 需求与存量功能关系分析\n对比说明";
        let artifact = make_artifact(Stage::Design, content);
        let result = validator.validate(&artifact).await.unwrap();
        assert!(result.passed);
    }

    #[tokio::test]
    async fn artifact_validator_design_fail() {
        let validator = DesignValidator::new();
        let artifact = make_artifact(Stage::Design, "# Missing section");
        let result = validator.validate(&artifact).await.unwrap();
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn artifact_validator_task_pass() {
        let validator = TaskValidator::new();
        let content = r#"
- [ ] 任务1：实现功能
  - 命令：cargo build
  - 验证：cargo test
- [x] 任务2：已完成
  - 命令：cargo clippy
  - 验证：cargo check
"#;
        let artifact = make_artifact(Stage::Task, content);
        let result = validator.validate(&artifact).await.unwrap();
        assert!(result.passed);
    }

    #[tokio::test]
    async fn artifact_validator_task_fail_no_command() {
        let validator = TaskValidator::new();
        let content = "- [ ] 任务1：实现功能\n  - 验证：检查输出";
        let artifact = make_artifact(Stage::Task, content);
        let result = validator.validate(&artifact).await.unwrap();
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn artifact_validator_coding_pass() {
        let validator = CodingValidator::new();
        let content = "use tokio::fs;\nasync fn main() { fs::read(\"file\").await?; }";
        let artifact = make_artifact(Stage::Coding, content);
        let result = validator.validate(&artifact).await.unwrap();
        assert!(result.passed);
    }

    #[tokio::test]
    async fn artifact_validator_coding_fail_unsafe() {
        let validator = CodingValidator::new();
        let content = "unsafe { let p = 0 as *const i32; }";
        let artifact = make_artifact(Stage::Coding, content);
        let result = validator.validate(&artifact).await.unwrap();
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.contains("unsafe")));
    }

    #[tokio::test]
    async fn artifact_validator_coding_fail_std_fs() {
        let validator = CodingValidator::new();
        let content = "use std::fs;\nfn main() { fs::read_to_string(\"file\").unwrap(); }";
        let artifact = make_artifact(Stage::Coding, content);
        let result = validator.validate(&artifact).await.unwrap();
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.contains("std::fs")));
    }

    #[tokio::test]
    async fn default_validators_cover_all_stages() {
        let validators = default_validators();
        let stages: Vec<Stage> = validators.iter().map(|v| v.stage()).collect();
        assert!(stages.contains(&Stage::Spec));
        assert!(stages.contains(&Stage::Design));
        assert!(stages.contains(&Stage::Task));
        assert!(stages.contains(&Stage::Coding));
    }
}
