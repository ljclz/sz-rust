// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use crate::analyzer::Tp6AnalysisReport;
use crate::generator::GenerationResult;
use crate::validator::ValidationReport;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Risk {
    pub title: String,
    pub severity: String,
    pub description: String,
}

pub struct RiskAnalyzer;

impl RiskAnalyzer {
    pub fn analyze(
        analysis: &Tp6AnalysisReport,
        generation: &GenerationResult,
        validation: &ValidationReport,
    ) -> Vec<Risk> {
        let mut risks = Vec::new();
        if analysis.dependency_graph.has_cycle {
            risks.push(Risk {
                title: "依赖环".into(),
                severity: "high".into(),
                description: "TP6 项目存在循环依赖，迁移顺序需人工干预".into(),
            });
        }
        if !generation.failed_interfaces.is_empty() {
            risks.push(Risk {
                title: "生成失败接口".into(),
                severity: "medium".into(),
                description: format!(
                    "{} 个接口代码生成失败，需手动迁移",
                    generation.failed_interfaces.len()
                ),
            });
        }
        if validation.failed > 0 {
            risks.push(Risk {
                title: "验证失败".into(),
                severity: "high".into(),
                description: format!("{} 个接口验证不一致", validation.failed),
            });
        }
        if validation.skipped > 0 {
            risks.push(Risk {
                title: "验证跳过".into(),
                severity: "low".into(),
                description: format!("{} 个接口被跳过", validation.skipped),
            });
        }
        risks
    }
}
