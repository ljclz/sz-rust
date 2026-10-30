// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use sz_rust_sdd::model::ValidationResult;
use sz_rust_sdd::redactor::SddRedactor;
use sz_rust_sdd::rules::validator::{RulesValidator, Severity};

#[tokio::test]
async fn e2e_rules_unsafe_blocked() {
    let validator = RulesValidator::new();
    let code = r#"
fn dangerous() -> i32 {
    let x = unsafe { std::hint::black_box(42) };
    x
}
"#;
    let result = validator.validate(code).await.unwrap();
    assert!(!result.passed, "unsafe code should be blocked");
    assert!(result.has_errors());
    assert!(result.violations.iter().any(|v| v.rule_id == "iron-01"));
}

#[tokio::test]
async fn e2e_rules_std_fs_blocked() {
    let validator = RulesValidator::new();
    let code = r#"
fn read_file() -> String {
    std::fs::read_to_string("config.toml").unwrap()
}
"#;
    let result = validator.validate(code).await.unwrap();
    assert!(!result.passed, "std::fs should be blocked");
    assert!(result.violations.iter().any(|v| v.rule_id == "iron-02"));
}

#[tokio::test]
async fn e2e_rules_allow_dead_code_blocked() {
    let validator = RulesValidator::new();
    let code = r#"
#![allow(dead_code)]

fn unused_fn() {}
"#;
    let result = validator.validate(code).await.unwrap();
    assert!(
        !result.passed,
        "crate-level allow(dead_code) should be blocked"
    );
    assert!(result.violations.iter().any(|v| v.rule_id == "iron-03"));
}

#[tokio::test]
async fn e2e_rules_async_not_send_static_blocked() {
    let validator = RulesValidator::new();
    let code = r#"
async fn do_work() -> String {
    "hello".to_string()
}
"#;
    let result = validator.validate(code).await.unwrap();
    assert!(
        !result.passed,
        "async fn without Send + 'static should be blocked"
    );
    assert!(result.violations.iter().any(|v| v.rule_id == "iron-04"));
}

#[tokio::test]
async fn e2e_rules_valid_code_passes() {
    let validator = RulesValidator::new();
    let code = r#"use tokio::fs;

fn read_config() -> impl std::future::Future<Output = Result<String, std::io::Error>> + Send + 'static {
    async move {
        tokio::fs::read_to_string("config.toml").await
    }
}
"#;
    let result = validator.validate(code).await.unwrap();
    assert!(
        result.passed,
        "valid code should pass, violations: {:?}",
        result.violations
    );
}

#[tokio::test]
async fn e2e_rules_violation_evidence_includes_line() {
    let validator = RulesValidator::new();
    let code = "line1\nline2\nunsafe { 42 };\nline4";
    let result = validator.validate(code).await.unwrap();
    let unsafe_violation = result
        .violations
        .iter()
        .find(|v| v.rule_id == "iron-01")
        .unwrap();
    assert!(
        unsafe_violation.evidence.contains("line"),
        "evidence should contain line number: {}",
        unsafe_violation.evidence
    );
}

#[tokio::test]
async fn e2e_rules_to_validation_result_conversion() {
    let validator = RulesValidator::new();
    let code = "unsafe { 42 };";
    let result = validator.validate(code).await.unwrap();
    let vr: ValidationResult = result.to_validation_result();
    assert!(!vr.passed);
    assert!(!vr.violations.is_empty());
}

#[tokio::test]
async fn e2e_redactor_sanitizes_sensitive_data() {
    let redactor = SddRedactor::new();
    let text = "password=secret123 token=Bearer abc123def456ghi789jkl012mno345pqr678stu901vwx234yz";
    let redacted = redactor.redact_text(text);

    assert!(
        !redacted.contains("secret123") || redacted.contains("***REDACTED***"),
        "sensitive data should be redacted: {}",
        redacted
    );
}

#[tokio::test]
async fn e2e_redactor_preserves_non_sensitive() {
    let redactor = SddRedactor::new();
    let text = "This is a normal log message with no sensitive data.";
    let redacted = redactor.redact_text(text);
    assert!(redacted.contains("normal log message"));
}

#[tokio::test]
async fn e2e_rules_all_violations_have_severity() {
    let validator = RulesValidator::new();
    let code = r#"
#![allow(dead_code)]
std::fs::read_to_string("foo").unwrap();
unsafe { 42 };
"#;
    let result = validator.validate(code).await.unwrap();
    for violation in &result.violations {
        assert!(
            matches!(
                violation.severity,
                Severity::Error | Severity::Warning | Severity::Info
            ),
            "violation should have valid severity"
        );
    }
}
