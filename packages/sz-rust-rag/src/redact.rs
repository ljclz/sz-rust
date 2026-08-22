//! 源码脱敏器。

use regex::Regex;

const REDACTED: &str = "***REDACTED***";

/// 源码脱敏器：内置 API Key / 密码 / Secret / PEM 规则集。
pub struct SourceCodeRedactor {
    rules: Vec<Regex>,
}

impl SourceCodeRedactor {
    pub fn new() -> Self {
        let patterns: &[&str] = &[
            r#"(?i)(api[_-]?key|apikey)\s*[:=]\s*["'][A-Za-z0-9_\-]{16,}["']"#,
            r#"(?i)(secret|password|passwd|pwd)\s*[:=]\s*["'][^"']{4,}["']"#,
            r#"(?i)(token|access[_-]?token)\s*[:=]\s*["'][A-Za-z0-9_\-\.]{16,}["']"#,
            r#"-----BEGIN [A-Z ]+PRIVATE KEY-----[\s\S]*?-----END [A-Z ]+PRIVATE KEY-----"#,
            r#"(?i)(aws[_-]?secret[_-]?access[_-]?key)\s*[:=]\s*["'][A-Za-z0-9/+=]{40}["']"#,
            r#"(?i)(bearer)\s+[A-Za-z0-9_\-\.]{20,}"#,
            r#"(?i)(mongodb|postgres|mysql|redis)://[^:\s]+:[^@\s]+@"#,
        ];
        let rules = patterns
            .iter()
            .map(|p| Regex::new(p).expect("valid regex"))
            .collect();
        Self { rules }
    }

    /// 脱敏文本，敏感字面量替换为 ***REDACTED***。
    pub fn redact(&self, text: &str) -> String {
        let mut result = text.to_string();
        for rule in &self.rules {
            result = rule.replace_all(&result, REDACTED).to_string();
        }
        result
    }
}

impl Default for SourceCodeRedactor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_api_key() {
        let r = SourceCodeRedactor::new();
        let input = r#"let api_key = "sk-1234567890abcdef""#;
        let out = r.redact(input);
        assert!(out.contains(REDACTED));
        assert!(!out.contains("sk-1234567890abcdef"));
    }

    #[test]
    fn redact_password() {
        let r = SourceCodeRedactor::new();
        let input = r#"password: "mysecret123""#;
        let out = r.redact(input);
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn redact_pem() {
        let r = SourceCodeRedactor::new();
        let input =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
        let out = r.redact(input);
        assert!(out.contains(REDACTED));
        assert!(!out.contains("MIIEowIBAAKCAQEA"));
    }

    #[test]
    fn redact_no_match() {
        let r = SourceCodeRedactor::new();
        let input = "fn foo() { let x = 42; }";
        let out = r.redact(input);
        assert_eq!(out, input);
    }

    #[test]
    fn redact_connection_string() {
        let r = SourceCodeRedactor::new();
        let input = "mongodb://user:pass123@host:27017";
        let out = r.redact(input);
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn redact_default() {
        let r = SourceCodeRedactor::default();
        let input = r#"api_key = "sk-1234567890abcdef""#;
        let out = r.redact(input);
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn redact_token_and_bearer() {
        let r = SourceCodeRedactor::new();
        let input = r#"token = "abcd1234efgh5678ijkl9012""#;
        let out = r.redact(input);
        assert!(out.contains(REDACTED));
        let input2 = "Bearer abcdef1234567890ijklmnop";
        let out2 = r.redact(input2);
        assert!(out2.contains(REDACTED));
    }

    #[test]
    fn redact_aws_secret() {
        let r = SourceCodeRedactor::new();
        let input = r#"aws_secret_access_key = "abcdefghijklmnopqrstuvwxyz0123456789ABCD""#;
        let out = r.redact(input);
        assert!(out.contains(REDACTED));
    }
}
