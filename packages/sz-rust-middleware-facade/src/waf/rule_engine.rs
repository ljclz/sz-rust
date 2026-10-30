// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! WAF 规则引擎（spec 5.11.2/5.11.6）

#![forbid(unsafe_code)]

use super::*;

/// WAF 规则引擎
pub struct WafRuleEngine {
    store: rule_store::WafRuleStore,
    config: WafConfig,
}

impl WafRuleEngine {
    /// 创建规则引擎
    pub fn new(rules: Vec<WafRule>, config: WafConfig) -> Self {
        Self {
            store: rule_store::WafRuleStore::new(rules),
            config,
        }
    }

    /// 检测请求特征
    ///
    /// 流程（spec 5.11.2）：
    /// 1. 加载规则集（按优先级排序）
    /// 2. 遍历规则检测请求特征
    /// 3. 命中则阻断（Block 模式）或记录（Detect 模式）
    pub fn detect(&self, feature: &RequestFeature) -> WafDetectResult {
        let rules = self.store.get_rules();
        let text = feature.detectable_text();

        for rule in rules.iter() {
            if let Some(captures) = rule.pattern.captures(&text) {
                let matched_value = captures
                    .get(0)
                    .map(|m| sanitize_match(m.as_str()))
                    .unwrap_or_default();

                let blocked = self.config.mode == WafMode::Block;

                tracing::warn!(
                    rule_id = %rule.id,
                    risk = ?rule.risk,
                    blocked,
                    "WAF 规则命中"
                );

                return WafDetectResult {
                    matched: true,
                    rule_id: Some(rule.id.clone()),
                    risk: Some(rule.risk),
                    blocked,
                    matched_value: Some(matched_value),
                };
            }
        }

        WafDetectResult::clean()
    }

    /// 热加载新规则集（spec 5.11.4）
    pub fn reload(&self, rules: Vec<WafRule>) {
        self.store.reload(rules);
    }

    /// 获取配置
    pub fn config(&self) -> &WafConfig {
        &self.config
    }

    /// 获取规则数量
    pub fn rule_count(&self) -> usize {
        self.store.get_rules().len()
    }
}

/// 脱敏匹配值（spec 5.11.12）
fn sanitize_match(value: &str) -> String {
    if value.len() > 100 {
        format!("{}...(truncated)", &value[..100])
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(id: &str, pattern: &str, priority: i32, risk: RiskLevel) -> WafRule {
        WafRule {
            id: id.to_string(),
            priority,
            pattern: Regex::new(pattern).unwrap(),
            risk,
            description: "test rule".to_string(),
        }
    }

    #[test]
    fn test_detect_sql_injection() {
        let rules = vec![make_rule(
            "sqli_001",
            r"(?i)(union\s+select|or\s+1=1|'\s*or\s*'1'='1)",
            1,
            RiskLevel::High,
        )];
        let engine = WafRuleEngine::new(rules, WafConfig::default());

        let feature = RequestFeature::new("GET", "/users?id=1' OR '1'='1", "", "");
        let result = engine.detect(&feature);

        assert!(result.matched, "SQL 注入应被检测");
        assert!(result.blocked, "Block 模式应阻断");
        assert_eq!(result.risk, Some(RiskLevel::High));
    }

    #[test]
    fn test_detect_mode_no_block() {
        let rules = vec![make_rule("xss_001", r"<script[^>]*>", 1, RiskLevel::High)];
        let config = WafConfig {
            mode: WafMode::Detect,
            ..Default::default()
        };
        let engine = WafRuleEngine::new(rules, config);

        let feature = RequestFeature::new("POST", "/api", "", "<script>alert(1)</script>");
        let result = engine.detect(&feature);

        assert!(result.matched, "XSS 应被检测");
        assert!(!result.blocked, "Detect 模式不应阻断");
    }

    #[test]
    fn test_clean_request_not_matched() {
        let rules = vec![make_rule(
            "sqli_001",
            r"(?i)union\s+select",
            1,
            RiskLevel::High,
        )];
        let engine = WafRuleEngine::new(rules, WafConfig::default());

        let feature = RequestFeature::new("GET", "/users?id=123", "", "");
        let result = engine.detect(&feature);

        assert!(!result.matched, "正常请求不应命中");
    }

    #[test]
    fn test_rule_priority_order() {
        let rules = vec![
            make_rule("low_priority", r"(?i)select", 10, RiskLevel::Low),
            make_rule("high_priority", r"(?i)union\s+select", 1, RiskLevel::High),
        ];
        let engine = WafRuleEngine::new(rules, WafConfig::default());

        let feature = RequestFeature::new("GET", "/q=UNION SELECT", "", "");
        let result = engine.detect(&feature);

        assert_eq!(result.rule_id, Some("high_priority".to_string()));
    }
}
