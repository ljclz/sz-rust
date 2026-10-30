// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! WAF 规则引擎（spec 5.11）
//!
//! 检测并阻断 OWASP Top 10 攻击。

#![forbid(unsafe_code)]

pub mod owasp_ruleset;
pub mod rule_engine;
pub mod rule_store;

use regex::Regex;

/// WAF 模式（spec 5.11.3 + 6.8.3）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WafMode {
    /// 检测模式：仅记录告警
    Detect,
    /// 阻断模式：记录并返回 403
    Block,
}

impl Default for WafMode {
    fn default() -> Self {
        WafMode::Block
    }
}

/// 风险等级（spec 6.8.1/6.8.2/6.8.5）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskLevel {
    /// 低风险
    Low,
    /// 中风险
    Medium,
    /// 高风险
    High,
}

/// WAF 规则
#[derive(Debug, Clone)]
pub struct WafRule {
    /// 规则 ID（唯一，spec 6.8.4）
    pub id: String,
    /// 优先级（数值越小越高）
    pub priority: i32,
    /// 匹配正则
    pub pattern: Regex,
    /// 风险等级
    pub risk: RiskLevel,
    /// 规则描述
    pub description: String,
}

/// WAF 配置
#[derive(Debug, Clone)]
pub struct WafConfig {
    /// 模式（默认 Block）
    pub mode: WafMode,
    /// 请求体检测深度
    pub body_detect_depth: usize,
}

impl Default for WafConfig {
    fn default() -> Self {
        Self {
            mode: WafMode::Block,
            body_detect_depth: 1024,
        }
    }
}

/// WAF 检测结果
#[derive(Debug, Clone)]
pub struct WafDetectResult {
    /// 是否命中规则
    pub matched: bool,
    /// 命中的规则 ID
    pub rule_id: Option<String>,
    /// 风险等级
    pub risk: Option<RiskLevel>,
    /// 是否阻断
    pub blocked: bool,
    /// 匹配的值（脱敏后）
    pub matched_value: Option<String>,
}

impl WafDetectResult {
    /// 未命中
    pub fn clean() -> Self {
        Self {
            matched: false,
            rule_id: None,
            risk: None,
            blocked: false,
            matched_value: None,
        }
    }
}

/// 请求特征
#[derive(Debug, Clone)]
pub struct RequestFeature {
    /// HTTP 方法
    pub method: String,
    /// 请求路径
    pub path: String,
    /// 查询参数
    pub query: String,
    /// 请求体（截断到 body_detect_depth）
    pub body: String,
}

impl RequestFeature {
    /// 从请求构建特征
    pub fn new(method: &str, path: &str, query: &str, body: &str) -> Self {
        Self {
            method: method.to_string(),
            path: path.to_string(),
            query: query.to_string(),
            body: body.to_string(),
        }
    }

    /// 获取所有可检测的文本
    pub fn detectable_text(&self) -> String {
        format!("{} {} {} {}", self.method, self.path, self.query, self.body)
    }
}
