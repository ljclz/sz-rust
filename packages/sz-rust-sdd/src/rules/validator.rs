// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::Path;

use tokio::fs;

use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::ValidationResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IronRule {
    pub id: String,
    pub name: String,
    pub description: String,
    pub severity: Severity,
}

impl IronRule {
    pub fn new(id: impl Into<String>, name: impl Into<String>, severity: Severity) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            severity,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IronRuleViolation {
    pub rule_id: String,
    pub rule_name: String,
    pub evidence: String,
    pub severity: Severity,
}

impl IronRuleViolation {
    pub fn new(rule: &IronRule, evidence: impl Into<String>) -> Self {
        Self {
            rule_id: rule.id.clone(),
            rule_name: rule.name.clone(),
            evidence: evidence.into(),
            severity: rule.severity,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RulesValidationResult {
    pub passed: bool,
    pub violations: Vec<IronRuleViolation>,
}

impl RulesValidationResult {
    pub fn passed() -> Self {
        Self {
            passed: true,
            violations: Vec::new(),
        }
    }

    pub fn failed(violations: Vec<IronRuleViolation>) -> Self {
        Self {
            passed: false,
            violations,
        }
    }

    pub fn to_validation_result(&self) -> ValidationResult {
        if self.passed {
            ValidationResult::passed()
        } else {
            let violations = self
                .violations
                .iter()
                .map(|v| format!("[{}] {}: {}", v.severity, v.rule_name, v.evidence))
                .collect();
            ValidationResult::failed(violations)
        }
    }

    pub fn has_errors(&self) -> bool {
        self.violations
            .iter()
            .any(|v| v.severity == Severity::Error)
    }
}

pub struct RulesValidator {
    rules: Vec<IronRule>,
}

impl RulesValidator {
    pub fn new() -> Self {
        Self {
            rules: Self::builtin_rules(),
        }
    }

    pub fn with_rules(rules: Vec<IronRule>) -> Self {
        Self { rules }
    }

    pub fn rules(&self) -> &[IronRule] {
        &self.rules
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    pub async fn load_from_file(&mut self, path: &Path) -> SddResult<()> {
        if !path.exists() {
            return Err(SddError::new(
                SddErrorCode::SddTargetNotFound,
                format!("rules file not found: {:?}", path),
            ));
        }

        let content = fs::read_to_string(path).await?;
        let parsed = Self::parse_rules_file(&content);
        if parsed.is_empty() {
            self.rules = Self::builtin_rules();
        } else {
            self.rules = parsed;
        }
        Ok(())
    }

    fn parse_rules_file(content: &str) -> Vec<IronRule> {
        let mut rules = Vec::new();
        for (idx, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("---") {
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("## ") {
                let id = format!("rule-{}", idx + 1);
                let name = rest.to_string();
                let severity = if name.to_lowercase().contains("禁止")
                    || name.to_lowercase().contains("必须")
                {
                    Severity::Error
                } else {
                    Severity::Warning
                };
                rules.push(IronRule::new(id, name, severity));
            }
        }
        rules
    }

    fn builtin_rules() -> Vec<IronRule> {
        vec![
            IronRule::new("iron-01", "禁止 unsafe 代码", Severity::Error),
            IronRule::new("iron-02", "禁止 std::fs，统一 tokio::fs", Severity::Error),
            IronRule::new(
                "iron-03",
                "禁止 crate 级 #![allow(dead_code)]",
                Severity::Error,
            ),
            IronRule::new("iron-04", "async fn 必须 Send + 'static", Severity::Error),
            IronRule::new("iron-05", "敏感字段必须脱敏", Severity::Warning),
        ]
    }

    pub async fn validate(&self, code: &str) -> SddResult<RulesValidationResult> {
        let mut violations = Vec::new();

        for rule in &self.rules {
            let rule_violations = match rule.id.as_str() {
                "iron-01" => self.check_no_unsafe(code, rule),
                "iron-02" => self.check_no_std_fs(code, rule),
                "iron-03" => self.check_no_allow_dead_code(code, rule),
                "iron-04" => self.check_async_send_static(code, rule),
                "iron-05" => self.check_sensitive_redaction(code, rule),
                _ => Vec::new(),
            };
            violations.extend(rule_violations);
        }

        let has_errors = violations.iter().any(|v| v.severity == Severity::Error);
        Ok(if has_errors {
            RulesValidationResult::failed(violations)
        } else {
            RulesValidationResult {
                passed: violations.is_empty(),
                violations,
            }
        })
    }

    fn check_no_unsafe(&self, code: &str, rule: &IronRule) -> Vec<IronRuleViolation> {
        let mut violations = Vec::new();
        for (line_num, line) in code.lines().enumerate() {
            if line.contains("unsafe") && !line.trim().starts_with("//") {
                violations.push(IronRuleViolation::new(
                    rule,
                    format!("line {}: contains `unsafe` block", line_num + 1),
                ));
            }
        }
        violations
    }

    fn check_no_std_fs(&self, code: &str, rule: &IronRule) -> Vec<IronRuleViolation> {
        let mut violations = Vec::new();
        for (line_num, line) in code.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains("std::fs::") {
                violations.push(IronRuleViolation::new(
                    rule,
                    format!(
                        "line {}: uses `std::fs` (must use `tokio::fs`)",
                        line_num + 1
                    ),
                ));
            }
        }
        violations
    }

    fn check_no_allow_dead_code(&self, code: &str, rule: &IronRule) -> Vec<IronRuleViolation> {
        let mut violations = Vec::new();
        for (line_num, line) in code.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("#![allow(dead_code)]") {
                violations.push(IronRuleViolation::new(
                    rule,
                    format!(
                        "line {}: crate-level `#![allow(dead_code)]` is forbidden",
                        line_num + 1
                    ),
                ));
            }
        }
        violations
    }

    fn check_async_send_static(&self, code: &str, rule: &IronRule) -> Vec<IronRuleViolation> {
        let has_async_fn = code.contains("async fn");
        let has_send_static = code.contains("Send + 'static")
            || code.contains("Send + Sync + 'static")
            || code.contains("Send + Sync + 'static");

        if has_async_fn && !has_send_static {
            vec![IronRuleViolation::new(
                rule,
                "code contains `async fn` but lacks `Send + 'static` constraint".to_string(),
            )]
        } else {
            Vec::new()
        }
    }

    fn check_sensitive_redaction(&self, code: &str, rule: &IronRule) -> Vec<IronRuleViolation> {
        let sensitive_patterns = [
            "password",
            "secret",
            "api_key",
            "access_token",
            "private_key",
        ];
        let redaction_patterns = ["skip_serializing", "redact", "REDACTED", "SensitiveField"];

        let has_sensitive = sensitive_patterns
            .iter()
            .any(|p| code.to_lowercase().contains(p));
        let has_redaction = redaction_patterns.iter().any(|p| code.contains(p));

        if has_sensitive && !has_redaction {
            vec![IronRuleViolation::new(
                rule,
                "code references sensitive fields without redaction (skip_serializing/redact)"
                    .to_string(),
            )]
        } else {
            Vec::new()
        }
    }
}

impl Default for RulesValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rules_no_unsafe() {
        let validator = RulesValidator::new();
        let code = r#"
fn main() {
    let x = unsafe { 42 };
}
"#;
        let result = validator.validate(code).await.unwrap();
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.rule_id == "iron-01"));
    }

    #[tokio::test]
    async fn rules_no_std_fs() {
        let validator = RulesValidator::new();
        let code = r#"
fn read() -> String {
    std::fs::read_to_string("foo").unwrap()
}
"#;
        let result = validator.validate(code).await.unwrap();
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.rule_id == "iron-02"));
    }

    #[tokio::test]
    async fn rules_no_allow_dead_code() {
        let validator = RulesValidator::new();
        let code = r#"
#![allow(dead_code)]

fn main() {}
"#;
        let result = validator.validate(code).await.unwrap();
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.rule_id == "iron-03"));
    }

    #[tokio::test]
    async fn rules_async_send_static() {
        let validator = RulesValidator::new();
        let code = r#"
async fn do_work() -> String {
    "hello".to_string()
}
"#;
        let result = validator.validate(code).await.unwrap();
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.rule_id == "iron-04"));
    }

    #[tokio::test]
    async fn rules_pass_valid_code() {
        let validator = RulesValidator::new();
        let code = r#"
use tokio::fs;

async fn read_file(path: &str) -> Result<String, std::io::Error>
where
    Self: Send + 'static,
{
    fs::read_to_string(path).await
}
"#;
        let result = validator.validate(code).await.unwrap();
        assert!(
            result.passed,
            "expected valid code to pass, violations: {:?}",
            result.violations
        );
    }

    #[tokio::test]
    async fn rules_sensitive_field_without_redaction() {
        let validator = RulesValidator::new();
        let code = r#"
struct Config {
    password: String,
}
"#;
        let result = validator.validate(code).await.unwrap();
        assert!(result.violations.iter().any(|v| v.rule_id == "iron-05"));
    }

    #[tokio::test]
    async fn rules_sensitive_field_with_redaction() {
        let validator = RulesValidator::new();
        let code = r#"
struct Config {
    #[serde(skip_serializing)]
    password: String,
}
"#;
        let result = validator.validate(code).await.unwrap();
        assert!(
            !result.violations.iter().any(|v| v.rule_id == "iron-05"),
            "redacted field should not trigger iron-05"
        );
    }

    #[tokio::test]
    async fn rules_load_from_file() {
        let dir = std::env::temp_dir().join(format!("sdd-rules-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).await.unwrap();
        let rules_path = dir.join("project_rules.md");
        fs::write(
            &rules_path,
            r#"# Project Rules

## 1. 禁止 unsafe 代码
All unsafe blocks are forbidden.

## 2. 必须 使用 tokio::fs
std::fs is forbidden.

## 3. 禁止 allow dead_code
Crate-level allow(dead_code) is forbidden.
"#,
        )
        .await
        .unwrap();

        let mut validator = RulesValidator::new();
        validator.load_from_file(&rules_path).await.unwrap();
        assert!(validator.rule_count() >= 3);
    }

    #[tokio::test]
    async fn rules_to_validation_result() {
        let validator = RulesValidator::new();
        let code = "unsafe { 42 };";
        let result = validator.validate(code).await.unwrap();
        assert!(!result.passed);

        let vr = result.to_validation_result();
        assert!(!vr.passed);
        assert!(!vr.violations.is_empty());
    }

    #[tokio::test]
    async fn rules_comment_ignored_for_std_fs() {
        let validator = RulesValidator::new();
        let code = r#"
// This line mentions std::fs but is a comment
async fn read() -> Result<String, std::io::Error>
where
    Self: Send + 'static,
{
    tokio::fs::read_to_string("foo").await
}
"#;
        let result = validator.validate(code).await.unwrap();
        assert!(
            !result.violations.iter().any(|v| v.rule_id == "iron-02"),
            "comment mentioning std::fs should not trigger iron-02"
        );
    }
}
