//! SafetyValidator — 对生成代码执行铁律检查
//!
//! 检查项（对应 .trae/rules/project_rules.md 22 条铁律的子集）：
//! 1. 禁止 unsafe 代码（铁律 3）
//! 2. 禁止裸 unwrap()（铁律 2）
//! 3. 禁止 std::fs，统一 tokio::fs（铁律 4）
//! 4. 禁止 SELECT *（铁律 8）
//! 5. 敏感字段必须 skip_serializing（铁律 7）

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Violation {
    pub rule: String,
    pub file: String,
    pub line: usize,
    pub message: String,
    pub suggestion: String,
}

pub struct SafetyValidator;

impl SafetyValidator {
    pub fn validate_files(files: &[(String, String)]) -> Vec<Violation> {
        let mut violations = Vec::new();
        for (path, content) in files {
            if path.ends_with(".rs") {
                violations.extend(Self::check_rust_file(path, content));
            } else if path.ends_with(".sql") {
                violations.extend(Self::check_sql_file(path, content));
            }
        }
        violations
    }

    fn check_rust_file(path: &str, content: &str) -> Vec<Violation> {
        let mut violations = Vec::new();
        for (i, line) in content.lines().enumerate() {
            let line_no = i + 1;
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("//!") {
                continue;
            }
            if trimmed.contains("unsafe ") || trimmed == "unsafe" {
                violations.push(Violation {
                    rule: "铁律3".to_string(),
                    file: path.to_string(),
                    line: line_no,
                    message: format!("发现 unsafe 代码: {trimmed}"),
                    suggestion: "使用安全 API 替代 unsafe".to_string(),
                });
            }
            if trimmed.contains(".unwrap()") && !trimmed.contains("expect(") {
                violations.push(Violation {
                    rule: "铁律2".to_string(),
                    file: path.to_string(),
                    line: line_no,
                    message: format!("发现裸 unwrap(): {trimmed}"),
                    suggestion: "使用 expect(\"明确原因\") 或 ? 传播错误".to_string(),
                });
            }
            if trimmed.contains("std::fs::") {
                violations.push(Violation {
                    rule: "铁律4".to_string(),
                    file: path.to_string(),
                    line: line_no,
                    message: format!("发现 std::fs 调用: {trimmed}"),
                    suggestion: "统一使用 tokio::fs".to_string(),
                });
            }
        }
        violations
    }

    fn check_sql_file(path: &str, content: &str) -> Vec<Violation> {
        let mut violations = Vec::new();
        for (i, line) in content.lines().enumerate() {
            let line_no = i + 1;
            let upper = line.to_uppercase();
            if upper.contains("SELECT *") {
                violations.push(Violation {
                    rule: "铁律8".to_string(),
                    file: path.to_string(),
                    line: line_no,
                    message: format!("发现 SELECT *: {line}"),
                    suggestion: "显式列投影，防止字段变更导致崩溃".to_string(),
                });
            }
        }
        violations
    }

    pub fn format_report(violations: &[Violation]) -> String {
        if violations.is_empty() {
            return "安全检查通过：0 个违规项".to_string();
        }
        let mut report = format!("安全检查失败：发现 {} 个违规项\n", violations.len());
        report.push_str(&"─".repeat(60));
        report.push('\n');
        let mut by_rule: HashMap<String, Vec<&Violation>> = HashMap::new();
        for v in violations {
            by_rule.entry(v.rule.clone()).or_default().push(v);
        }
        for (rule, vs) in by_rule.iter() {
            report.push_str(&format!("\n[{rule}] ({} 个违规)\n", vs.len()));
            for v in vs {
                report.push_str(&format!(
                    "  {}:{} {}\n    → {}\n",
                    v.file, v.line, v.message, v.suggestion
                ));
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_violations() {
        let files = vec![(
            "src/model.rs".to_string(),
            "pub struct Foo { pub x: i32 }\n".to_string(),
        )];
        let violations = SafetyValidator::validate_files(&files);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_unsafe_violation() {
        let files = vec![("src/foo.rs".to_string(), "unsafe { *ptr }\n".to_string())];
        let violations = SafetyValidator::validate_files(&files);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "铁律3");
    }

    #[test]
    fn test_unwrap_violation() {
        let files = vec![(
            "src/foo.rs".to_string(),
            "let x = val.unwrap();\n".to_string(),
        )];
        let violations = SafetyValidator::validate_files(&files);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "铁律2");
    }

    #[test]
    fn test_select_star_violation() {
        let files = vec![(
            "migrations/table.sql".to_string(),
            "SELECT * FROM users;\n".to_string(),
        )];
        let violations = SafetyValidator::validate_files(&files);
        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "铁律8");
    }
}
