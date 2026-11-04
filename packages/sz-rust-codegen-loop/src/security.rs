// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 安全扫描：复用 cli/safety_validator 执行铁律检查。
//!
//! 禁止 unsafe / std::fs / 裸 unwrap / 硬编码密钥。

use crate::error::CodegenError;
use crate::generator::GeneratedFile;
use sz_rust_cli::safety_validator::{SafetyValidator, Violation};

/// 安全扫描结果。
#[derive(Debug, Clone)]
pub struct SecurityScanResult {
    pub passed: bool,
    pub violations: Vec<Violation>,
}

/// 安全扫描器。
pub struct SecurityScanner;

impl SecurityScanner {
    /// 对生成文件执行安全扫描。
    ///
    /// # 示例
    ///
    /// ```
    /// use sz_rust_codegen_loop::generator::GeneratedFile;
    /// use sz_rust_codegen_loop::security::SecurityScanner;
    /// let files = vec![GeneratedFile {
    ///     path: "test.rs".into(),
    ///     content: "fn main() {}".into(),
    /// }];
    /// let result = SecurityScanner::scan(&files);
    /// assert!(result.passed);
    /// ```
    pub fn scan(files: &[GeneratedFile]) -> SecurityScanResult {
        let file_tuples: Vec<(String, String)> = files
            .iter()
            .map(|f| (f.path.clone(), f.content.clone()))
            .collect();
        let violations = SafetyValidator::validate_files(&file_tuples);
        let passed = violations.is_empty();
        SecurityScanResult { passed, violations }
    }

    /// 扫描并拒绝不安全的代码。
    pub fn scan_and_guard(files: &[GeneratedFile]) -> Result<(), CodegenError> {
        let result = Self::scan(files);
        if !result.passed {
            let messages: Vec<String> = result
                .violations
                .iter()
                .map(|v| format!("{}:{} {} ({})", v.file, v.line, v.message, v.rule))
                .collect();
            return Err(CodegenError::SecurityViolation(messages.join("; ")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_file(path: &str, content: &str) -> GeneratedFile {
        GeneratedFile {
            path: path.into(),
            content: content.into(),
        }
    }

    #[test]
    fn scan_safe_code() {
        let files = vec![make_file("main.rs", "fn main() { println!(\"hello\"); }")];
        let result = SecurityScanner::scan(&files);
        assert!(result.passed);
    }

    #[test]
    fn scan_detects_unsafe() {
        let files = vec![make_file("main.rs", "unsafe { let r = &1; }")];
        let result = SecurityScanner::scan(&files);
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.rule.contains("铁律3")));
    }

    #[test]
    fn scan_detects_std_fs() {
        let files = vec![make_file("main.rs", "std::fs::read_to_string(\"file\");")];
        let result = SecurityScanner::scan(&files);
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.rule.contains("铁律4")));
    }

    #[test]
    fn scan_detects_unwrap() {
        let files = vec![make_file("main.rs", "let x = opt.unwrap();")];
        let result = SecurityScanner::scan(&files);
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.rule.contains("铁律2")));
    }

    #[test]
    fn scan_and_guard_passes() {
        let files = vec![make_file("main.rs", "fn main() {}")];
        assert!(SecurityScanner::scan_and_guard(&files).is_ok());
    }

    #[test]
    fn scan_and_guard_rejects() {
        let files = vec![make_file("main.rs", "unsafe {}")];
        assert!(SecurityScanner::scan_and_guard(&files).is_err());
    }

    #[test]
    fn scan_multiple_files() {
        let files = vec![
            make_file("safe.rs", "fn main() {}"),
            make_file("unsafe.rs", "unsafe {}"),
        ];
        let result = SecurityScanner::scan(&files);
        assert!(!result.passed);
        assert!(result.violations.iter().any(|v| v.file == "unsafe.rs"));
    }

    #[test]
    fn scan_empty_files() {
        let result = SecurityScanner::scan(&[]);
        assert!(result.passed);
    }
}
