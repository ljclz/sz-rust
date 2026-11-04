// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::HashSet;

use serde_json::Value;

use sz_rust_rag::redact::SourceCodeRedactor;

const SENSITIVE_FIELD_NAMES: &[&str] = &[
    "api_key",
    "apikey",
    "api-key",
    "secret",
    "password",
    "passwd",
    "pwd",
    "token",
    "access_token",
    "access-token",
    "refresh_token",
    "refresh-token",
    "private_key",
    "privatekey",
    "private-key",
    "aws_secret_access_key",
    "aws-secret-access-key",
    "bearer",
    "connection_string",
    "conn_str",
    "database_url",
    "db_url",
    "jwt_secret",
    "session_secret",
    "encryption_key",
];

const REDACTED_PLACEHOLDER: &str = "<redacted>";

pub struct SddRedactor {
    text_redactor: SourceCodeRedactor,
    sensitive_fields: HashSet<String>,
}

impl SddRedactor {
    pub fn new() -> Self {
        let sensitive_fields = SENSITIVE_FIELD_NAMES
            .iter()
            .map(|s| s.to_string())
            .collect();
        Self {
            text_redactor: SourceCodeRedactor::new(),
            sensitive_fields,
        }
    }

    pub fn redact_text(&self, text: &str) -> String {
        self.text_redactor.redact(text)
    }

    pub fn redact_json(&self, json: &mut Value) {
        match json {
            Value::Object(map) => {
                let keys: Vec<String> = map.keys().cloned().collect();
                for key in keys {
                    let is_sensitive = self.is_sensitive_field(&key);
                    if let Some(val) = map.get_mut(&key) {
                        if is_sensitive && (val.is_string() || val.is_number()) {
                            *val = Value::String(REDACTED_PLACEHOLDER.to_string());
                        } else {
                            self.redact_json(val);
                        }
                    }
                }
            }
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    self.redact_json(item);
                }
            }
            _ => {}
        }
    }

    pub fn redact_json_string(&self, json_str: &str) -> String {
        match serde_json::from_str::<Value>(json_str) {
            Ok(mut value) => {
                self.redact_json(&mut value);
                serde_json::to_string(&value).unwrap_or_else(|_| json_str.to_string())
            }
            Err(_) => self.redact_text(json_str),
        }
    }

    pub fn redact(&self, text: &str) -> String {
        let text_redacted = self.redact_text(text);
        self.redact_json_string(&text_redacted)
    }

    fn is_sensitive_field(&self, field: &str) -> bool {
        let lower = field.to_lowercase();
        if self.sensitive_fields.contains(&lower) {
            return true;
        }
        self.sensitive_fields.iter().any(|s| lower.contains(s))
    }
}

impl Default for SddRedactor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redactor_sensitive_fields_api_key() {
        let r = SddRedactor::new();
        let input = r#"{"api_key": "sk-1234567890abcdef", "name": "test"}"#;
        let output = r.redact_json_string(input);
        assert!(output.contains(REDACTED_PLACEHOLDER));
        assert!(!output.contains("sk-1234567890abcdef"));
        assert!(output.contains("test"));
    }

    #[test]
    fn redactor_sensitive_fields_password() {
        let r = SddRedactor::new();
        let input = r#"{"password": "my_secret_pwd", "user": "admin"}"#;
        let output = r.redact_json_string(input);
        assert!(output.contains(REDACTED_PLACEHOLDER));
        assert!(!output.contains("my_secret_pwd"));
    }

    #[test]
    fn redactor_sensitive_fields_token() {
        let r = SddRedactor::new();
        let input = r#"{"token": "bearer_abc123xyz456", "id": 42}"#;
        let output = r.redact_json_string(input);
        assert!(output.contains(REDACTED_PLACEHOLDER));
        assert!(!output.contains("bearer_abc123xyz456"));
    }

    #[test]
    fn redactor_sensitive_fields_pem() {
        let r = SddRedactor::new();
        let pem =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
        let output = r.redact_text(pem);
        assert!(output.contains("REDACTED") || output.contains("<redacted"));
    }

    #[test]
    fn redactor_preserves_structure() {
        let r = SddRedactor::new();
        let input = r#"{"api_key": "secret123", "nested": {"password": "pwd"}, "list": [1, 2]}"#;
        let output = r.redact_json_string(input);
        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.is_object());
        assert!(parsed["nested"].is_object());
        assert!(parsed["list"].is_array());
        assert_eq!(parsed["list"][0], 1);
    }

    #[test]
    fn redactor_nested_sensitive() {
        let r = SddRedactor::new();
        let input = r#"{"config": {"database": {"password": "db_pwd_123"}}}"#;
        let output = r.redact_json_string(input);
        assert!(output.contains(REDACTED_PLACEHOLDER));
        assert!(!output.contains("db_pwd_123"));
    }

    #[test]
    fn redactor_combined_text_and_json() {
        let r = SddRedactor::new();
        let input = r#"api_key = "sk-test1234567890abcdef""#;
        let output = r.redact_text(input);
        assert!(output.contains("REDACTED") || !output.contains("sk-test1234567890abcdef"));
    }
}
