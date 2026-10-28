// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use crate::reporter::risk_analyzer::Risk;
use crate::validator::ValidationReport;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Suggestion {
    pub description: String,
}

pub struct SuggestionBuilder;

impl SuggestionBuilder {
    pub fn build(risks: &[Risk], validation: &ValidationReport) -> Vec<Suggestion> {
        let mut suggestions = Vec::new();
        for risk in risks {
            match risk.severity.as_str() {
                "high" => {
                    suggestions.push(Suggestion {
                        description: format!("优先处理高风险项: {}", risk.title),
                    });
                }
                "medium" => {
                    suggestions.push(Suggestion {
                        description: format!("计划处理中风险项: {}", risk.title),
                    });
                }
                _ => {}
            }
        }
        if validation.passed > 0 && validation.failed == 0 {
            suggestions.push(Suggestion {
                description: "所有验证通过，可考虑全量迁移".into(),
            });
        }
        if validation.failed > 0 {
            suggestions.push(Suggestion {
                description: "检查不一致接口的响应差异，调整生成代码后重新验证".into(),
            });
        }
        suggestions
    }
}
