// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! OWASP Top 10 默认规则集（spec 5.11.2）

#![forbid(unsafe_code)]

use regex::Regex;

use super::{RiskLevel, WafRule};

/// 预置 OWASP Top 10 正则规则
///
/// 覆盖（spec 5.11.2）：
/// - SQL 注入
/// - XSS
/// - 路径遍历
/// - 命令注入
/// - SSRF
pub fn owasp_default_ruleset() -> Vec<WafRule> {
    vec![
        // SQL 注入
        WafRule {
            id: "owasp_sqli_union".to_string(),
            priority: 1,
            pattern: Regex::new(r"(?i)(union\s+select|union\s+all\s+select)")
                .expect("内置 WAF 规则正则应可编译"),
            risk: RiskLevel::High,
            description: "SQL 注入: UNION SELECT".to_string(),
        },
        WafRule {
            id: "owasp_sqli_boolean".to_string(),
            priority: 1,
            pattern: Regex::new(r"(?i)('\s*or\s*'1'='1|'\s*or\s*1=1|--\s)")
                .expect("内置 WAF 规则正则应可编译"),
            risk: RiskLevel::High,
            description: "SQL 注入: 布尔盲注".to_string(),
        },
        // XSS
        WafRule {
            id: "owasp_xss_script".to_string(),
            priority: 2,
            pattern: Regex::new(r"(?i)<script[^>]*>").expect("内置 WAF 规则正则应可编译"),
            risk: RiskLevel::High,
            description: "XSS: <script> 标签".to_string(),
        },
        WafRule {
            id: "owasp_xss_event".to_string(),
            priority: 2,
            pattern: Regex::new(r"(?i)(onerror|onload|onclick)\s*=")
                .expect("内置 WAF 规则正则应可编译"),
            risk: RiskLevel::Medium,
            description: "XSS: 事件处理器".to_string(),
        },
        // 路径遍历
        WafRule {
            id: "owasp_path_traversal".to_string(),
            priority: 3,
            pattern: Regex::new(r"(\.\./|\.\.\\|%2e%2e%2f)").expect("内置 WAF 规则正则应可编译"),
            risk: RiskLevel::High,
            description: "路径遍历".to_string(),
        },
        // 命令注入
        WafRule {
            id: "owasp_cmd_injection".to_string(),
            priority: 3,
            pattern: Regex::new(r"(;|\||`|\$\(|%0a|%0d)\s*(cat|ls|id|whoami|wget|curl|bash|sh)\s")
                .expect("内置 WAF 规则正则应可编译"),
            risk: RiskLevel::High,
            description: "命令注入".to_string(),
        },
        // SSRF
        WafRule {
            id: "owasp_ssrf".to_string(),
            priority: 4,
            pattern: Regex::new(
                r"(?i)(http|ftp|file)://(localhost|127\.0\.0\.1|0\.0\.0\.0|169\.254\.169\.254)",
            )
            .expect("内置 WAF 规则正则应可编译"),
            risk: RiskLevel::High,
            description: "SSRF: 内网地址".to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::super::rule_engine::WafRuleEngine;
    use super::super::{RequestFeature, WafConfig};
    use super::*;

    #[test]
    fn test_owasp_ruleset_not_empty() {
        let rules = owasp_default_ruleset();
        assert!(rules.len() >= 5, "OWASP 规则集应至少 5 条规则");
    }

    #[test]
    fn test_owasp_detect_sql_injection() {
        let engine = WafRuleEngine::new(owasp_default_ruleset(), WafConfig::default());
        let feature = RequestFeature::new("GET", "/users?id=1 UNION SELECT * FROM users", "", "");
        let result = engine.detect(&feature);
        assert!(result.matched, "SQL 注入应被检测");
        assert!(result.blocked, "应阻断");
    }

    #[test]
    fn test_owasp_detect_xss() {
        let engine = WafRuleEngine::new(owasp_default_ruleset(), WafConfig::default());
        let feature = RequestFeature::new("POST", "/comment", "", "<script>alert('xss')</script>");
        let result = engine.detect(&feature);
        assert!(result.matched, "XSS 应被检测");
    }

    #[test]
    fn test_owasp_detect_path_traversal() {
        let engine = WafRuleEngine::new(owasp_default_ruleset(), WafConfig::default());
        let feature = RequestFeature::new("GET", "/file?name=../../../etc/passwd", "", "");
        let result = engine.detect(&feature);
        assert!(result.matched, "路径遍历应被检测");
    }

    #[test]
    fn test_owasp_detect_ssrf() {
        let engine = WafRuleEngine::new(owasp_default_ruleset(), WafConfig::default());
        let feature = RequestFeature::new("GET", "/fetch?url=http://127.0.0.1:8080", "", "");
        let result = engine.detect(&feature);
        assert!(result.matched, "SSRF 应被检测");
    }
}
