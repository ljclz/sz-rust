// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! OTel 全链路 span 属性 + baggage 传播（spec §5.11）
//!
//! span 关键属性标注 + 敏感信息脱敏 + baggage 跨服务传播。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Baggage 键值对（跨服务传播业务维度信息，spec §5.11 规则 3）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Baggage(pub HashMap<String, String>);

impl Baggage {
    /// baggage 键值对数量上限（spec §6.11 规则 3，防止无限增长）
    pub const MAX_ENTRIES: usize = 64;

    /// 创建空 baggage
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// 插入键值对（超限拒绝，spec §6.11 规则 3）
    ///
    /// 返回 `true` 表示插入成功，`false` 表示因超限被拒绝
    pub fn insert(&mut self, key: String, value: String) -> bool {
        if self.0.len() >= Self::MAX_ENTRIES && !self.0.contains_key(&key) {
            return false;
        }
        self.0.insert(key, value);
        true
    }

    /// 获取值
    pub fn get(&self, key: &str) -> Option<&String> {
        self.0.get(key)
    }

    /// 键值对数量
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 序列化为 W3C baggage 格式（`key1=val1,key2=val2`）
    pub fn to_w3c_string(&self) -> String {
        let pairs: Vec<String> = self.0.iter().map(|(k, v)| format!("{k}={v}")).collect();
        pairs.join(",")
    }

    /// 从 W3C baggage 格式解析
    pub fn from_w3c(s: &str) -> Self {
        let mut map = HashMap::new();
        for pair in s.split(',') {
            let pair = pair.trim();
            if let Some((k, v)) = pair.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        Self(map)
    }
}

/// span 关键属性（spec §5.11 规则 4）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SpanAttributes {
    /// HTTP URL
    pub http_url: Option<String>,
    /// DB 语句
    pub db_statement: Option<String>,
    /// 缓存键
    pub cache_key: Option<String>,
    /// MQTT topic
    pub mqtt_topic: Option<String>,
}

/// 敏感信息模式（用于脱敏匹配）
const SENSITIVE_PATTERNS: &[&str] = &[
    "password",
    "passwd",
    "token",
    "secret",
    "api_key",
    "apikey",
    "authorization",
    "credential",
];

impl SpanAttributes {
    /// 创建空属性
    pub fn new() -> Self {
        Self::default()
    }

    /// 脱敏敏感信息（spec §5.11 禁止项：密钥/密码/Token 不得出现在 span 属性）
    ///
    /// 对 db_statement/cache_key 中的敏感信息替换为 `***`
    pub fn desensitize(&mut self) {
        if let Some(ref mut stmt) = self.db_statement {
            *stmt = desensitize_string(stmt);
        }
        if let Some(ref mut key) = self.cache_key {
            *key = desensitize_string(key);
        }
        if let Some(ref mut url) = self.http_url {
            *url = desensitize_string(url);
        }
    }

    /// 检查是否包含敏感信息
    pub fn has_sensitive_info(&self) -> bool {
        self.db_statement
            .as_ref()
            .map(|s| contains_sensitive(s))
            .unwrap_or(false)
            || self
                .cache_key
                .as_ref()
                .map(|s| contains_sensitive(s))
                .unwrap_or(false)
            || self
                .http_url
                .as_ref()
                .map(|s| contains_sensitive(s))
                .unwrap_or(false)
    }
}

/// 检查字符串是否包含敏感关键词
fn contains_sensitive(s: &str) -> bool {
    let lower = s.to_lowercase();
    SENSITIVE_PATTERNS.iter().any(|p| lower.contains(p))
}

/// 脱敏字符串中的敏感信息
fn desensitize_string(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut result = s.to_string();
    for pattern in SENSITIVE_PATTERNS {
        if lower.contains(pattern) {
            // 简化脱敏：如果包含敏感词，将值部分替换为 ***
            // 匹配 `key=value` 或 `key: value` 格式
            if let Some(pos) = lower.find(pattern) {
                let after_pattern = &s[pos + pattern.len()..];
                if let Some(eq_pos) = after_pattern.find('=') {
                    let value_start = pos + pattern.len() + eq_pos + 1;
                    if value_start < s.len() {
                        result = format!("{}***", &s[..value_start]);
                    }
                } else if let Some(colon_pos) = after_pattern.find(':') {
                    let value_start = pos + pattern.len() + colon_pos + 1;
                    if value_start < s.len() {
                        result = format!("{}***", &s[..value_start]);
                    }
                } else {
                    result = format!("{}***", &s[..pos + pattern.len()]);
                }
            }
        }
    }
    result
}

/// span 上下文（trace_id + span_id）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpanContext {
    /// trace ID
    pub trace_id: String,
    /// span ID
    pub span_id: String,
}

impl SpanContext {
    /// 创建 span 上下文
    pub fn new(trace_id: impl Into<String>, span_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: span_id.into(),
        }
    }

    /// 注入 W3C traceparent header 格式
    pub fn to_traceparent(&self) -> String {
        format!("00-{}-{}-01", self.trace_id, self.span_id)
    }

    /// 从 W3C traceparent header 解析
    pub fn from_traceparent(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() >= 4 {
            Some(Self {
                trace_id: parts[1].to_string(),
                span_id: parts[2].to_string(),
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_baggage_insert_within_limit() {
        let mut b = Baggage::new();
        for i in 0..Baggage::MAX_ENTRIES {
            assert!(b.insert(format!("k{i}"), format!("v{i}")));
        }
        assert_eq!(b.len(), Baggage::MAX_ENTRIES);
    }

    #[test]
    fn test_baggage_insert_exceeds_limit() {
        let mut b = Baggage::new();
        for i in 0..Baggage::MAX_ENTRIES {
            b.insert(format!("k{i}"), format!("v{i}"));
        }
        assert!(!b.insert("new_key".to_string(), "val".to_string()));
        assert_eq!(b.len(), Baggage::MAX_ENTRIES);
    }

    #[test]
    fn test_baggage_update_existing_within_limit() {
        let mut b = Baggage::new();
        for i in 0..Baggage::MAX_ENTRIES {
            b.insert(format!("k{i}"), format!("v{i}"));
        }
        assert!(b.insert("k0".to_string(), "updated".to_string()));
        assert_eq!(b.get("k0").unwrap(), "updated");
    }

    #[test]
    fn test_baggage_w3c_round_trip() {
        let mut b = Baggage::new();
        b.insert("key1".into(), "val1".into());
        b.insert("key2".into(), "val2".into());
        let s = b.to_w3c_string();
        let parsed = Baggage::from_w3c(&s);
        assert_eq!(parsed.get("key1").unwrap(), "val1");
        assert_eq!(parsed.get("key2").unwrap(), "val2");
    }

    #[test]
    fn test_baggage_from_w3c_empty() {
        let b = Baggage::from_w3c("");
        assert!(b.is_empty());
    }

    #[test]
    fn test_baggage_not_empty_after_insert() {
        let mut b = Baggage::new();
        b.insert("key1".into(), "val1".into());
        assert!(!b.is_empty(), "插入键值对后 baggage 不应为空");
    }

    #[test]
    fn test_baggage_from_w3c_with_whitespace() {
        let b = Baggage::from_w3c(" key1 = val1 , key2 = val2 ");
        assert_eq!(b.get("key1").map(|s| s.as_str()), Some("val1"));
        assert_eq!(b.get("key2").map(|s| s.as_str()), Some("val2"));
    }

    #[test]
    fn test_baggage_from_w3c_malformed_pair_skipped() {
        let b = Baggage::from_w3c("key1=val1,noequals");
        assert_eq!(b.len(), 1, "缺少 '=' 的分段应被跳过");
        assert_eq!(b.get("key1").map(|s| s.as_str()), Some("val1"));
    }

    #[test]
    fn test_span_attributes_desensitize_db_statement() {
        let mut attrs = SpanAttributes {
            http_url: None,
            db_statement: Some("SELECT * FROM users WHERE password='secret123'".to_string()),
            cache_key: None,
            mqtt_topic: None,
        };
        attrs.desensitize();
        assert!(!attrs.db_statement.as_ref().unwrap().contains("secret123"));
        assert!(attrs.db_statement.as_ref().unwrap().contains("***"));
    }

    #[test]
    fn test_span_attributes_desensitize_cache_key() {
        let mut attrs = SpanAttributes {
            http_url: None,
            db_statement: None,
            cache_key: Some("token=abc123".to_string()),
            mqtt_topic: None,
        };
        attrs.desensitize();
        assert!(!attrs.cache_key.as_ref().unwrap().contains("abc123"));
    }

    #[test]
    fn test_span_attributes_desensitize_no_sensitive() {
        let mut attrs = SpanAttributes {
            http_url: Some("http://localhost:8080/api".to_string()),
            db_statement: Some("SELECT * FROM users WHERE id = 1".to_string()),
            cache_key: Some("user:1:profile".to_string()),
            mqtt_topic: None,
        };
        let original = attrs.clone();
        attrs.desensitize();
        assert_eq!(attrs, original);
    }

    #[test]
    fn test_desensitize_string_exact_output_equals_form() {
        // `password=abc123` → `password=***`（输入仅含 password 一个敏感词，避免二次脱敏）。
        // 精确断言可杀死 desensitize_string 中 value_start 算术（`+`→`-`/`*`）变异体。
        assert_eq!(desensitize_string("password=abc123"), "password=***");
    }

    #[test]
    fn test_desensitize_string_exact_output_colon_form() {
        // `api_key: secret` → `api_key:***`（冒号分支，仅含 api_key 一个敏感词）。
        // 现有测试未覆盖冒号分支，导致该分支所有变异体存活。
        assert_eq!(desensitize_string("api_key: secret"), "api_key:***");
    }

    #[test]
    fn test_desensitize_string_uppercase() {
        // 大写敏感词：`to_lowercase()` 被替换为 `to_string()` 的变异体在此输入下会漏脱敏。
        assert_eq!(desensitize_string("Password=abc"), "Password=***");
    }

    #[test]
    fn test_desensitize_string_empty_value_after_equals_unchanged() {
        // value_start == s.len() 时原始逻辑不追加 `***`；`<`→`<=` 变异体在此输入下会错误截断。
        assert_eq!(desensitize_string("password="), "password=");
    }

    #[test]
    fn test_desensitize_string_empty_value_after_colon_unchanged() {
        assert_eq!(desensitize_string("api_key:"), "api_key:");
    }

    #[test]
    fn test_desensitize_string_token_no_separator() {
        // 无 `=`/`:` 分隔符 → 走 else 分支，只保留敏感词前缀。
        assert_eq!(desensitize_string("token abc"), "token***");
    }

    #[test]
    fn test_desensitize_string_equals_with_space() {
        // `password = abc`（'=' 前有空格）：eq_pos = 1，`pattern.len() + eq_pos` 的
        // `+`→`-` 变异体会产生不同 value_start，精确断言可将其杀死。
        assert_eq!(desensitize_string("password = abc"), "password =***");
    }

    #[test]
    fn test_desensitize_string_colon_with_space() {
        // `api_key : secret`（':' 前有空格）：colon_pos = 1，同上杀死冒号分支算术变异体。
        assert_eq!(desensitize_string("api_key : secret"), "api_key :***");
    }

    #[test]
    fn test_span_attributes_has_sensitive() {
        let attrs = SpanAttributes {
            http_url: None,
            db_statement: Some("password=test".to_string()),
            cache_key: None,
            mqtt_topic: None,
        };
        assert!(attrs.has_sensitive_info());
    }

    #[test]
    fn test_span_attributes_no_sensitive() {
        let attrs = SpanAttributes {
            http_url: Some("http://api/users".to_string()),
            db_statement: Some("SELECT 1".to_string()),
            cache_key: None,
            mqtt_topic: Some("sensors/temp".to_string()),
        };
        assert!(!attrs.has_sensitive_info());
    }

    #[test]
    fn test_span_attributes_has_sensitive_http_url_only() {
        // db/cache 均无敏感信息，仅 http_url 含 token —— 用于杀死 has_sensitive_info
        // 中最后一个 `||` 被替换为 `&&` 的变异体（原始语义：任一字段敏感即 true）
        let attrs = SpanAttributes {
            http_url: Some("http://api?token=abc123".to_string()),
            db_statement: Some("SELECT 1".to_string()),
            cache_key: Some("user:1".to_string()),
            mqtt_topic: None,
        };
        assert!(
            attrs.has_sensitive_info(),
            "http_url 含 token 时整体应判定为敏感"
        );
    }

    #[test]
    fn test_span_attributes_desensitize_http_url() {
        let mut attrs = SpanAttributes {
            http_url: Some("http://api?token=abc123".to_string()),
            db_statement: None,
            cache_key: None,
            mqtt_topic: None,
        };
        attrs.desensitize();
        assert!(
            !attrs.http_url.as_ref().unwrap().contains("abc123"),
            "http_url 中的 token 值应被脱敏"
        );
    }

    #[test]
    fn test_span_context_traceparent_round_trip() {
        let ctx = SpanContext::new("0af7651916cd43dd8448eb211c80319c", "b7ad6b7169203331");
        let tp = ctx.to_traceparent();
        let parsed = SpanContext::from_traceparent(&tp).unwrap();
        assert_eq!(parsed, ctx);
    }

    #[test]
    fn test_span_context_from_invalid_traceparent() {
        assert!(SpanContext::from_traceparent("invalid").is_none());
    }

    #[test]
    fn test_span_context_from_traceparent_more_than_four_parts() {
        // `>= 4` 被替换为 `== 4` 的变异体对 5 段输入返回 None，此测试杀死该变异体。
        let parsed = SpanContext::from_traceparent(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01-extra",
        )
        .unwrap();
        assert_eq!(parsed.trace_id, "0af7651916cd43dd8448eb211c80319c");
        assert_eq!(parsed.span_id, "b7ad6b7169203331");
    }
}
