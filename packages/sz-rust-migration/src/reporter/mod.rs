// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

pub mod risk_analyzer;
pub mod suggestion_builder;

use std::path::Path;

use crate::analyzer::Tp6AnalysisReport;
use crate::error::MigrationResult;
use crate::generator::GenerationResult;
use crate::validator::ValidationReport;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MigrationReport {
    pub migration_id: String,
    pub analysis_summary: AnalysisSummary,
    pub generation_summary: GenerationSummary,
    pub validation_summary: ValidationSummary,
    pub risks: Vec<risk_analyzer::Risk>,
    pub suggestions: Vec<suggestion_builder::Suggestion>,
    pub coverage_percent: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnalysisSummary {
    pub tp6_version: String,
    pub route_count: usize,
    pub model_count: usize,
    pub controller_count: usize,
    pub middleware_count: usize,
    pub has_cycle: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GenerationSummary {
    pub generated_files: usize,
    pub failed_interfaces: usize,
    pub total_retries: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationSummary {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
}

pub struct MigrationReporter;

impl MigrationReporter {
    pub fn build(
        migration_id: &str,
        analysis: &Tp6AnalysisReport,
        generation: &GenerationResult,
        validation: &ValidationReport,
    ) -> MigrationReport {
        let analysis_summary = AnalysisSummary {
            tp6_version: analysis.version.clone(),
            route_count: analysis.routes.len(),
            model_count: analysis.models.len(),
            controller_count: analysis.controllers.len(),
            middleware_count: analysis.middlewares.len(),
            has_cycle: analysis.dependency_graph.has_cycle,
        };
        let generation_summary = GenerationSummary {
            generated_files: generation.generated_files.len(),
            failed_interfaces: generation.failed_interfaces.len(),
            total_retries: generation.total_retries,
        };
        let validation_summary = ValidationSummary {
            total: validation.total_interfaces,
            passed: validation.passed,
            failed: validation.failed,
            skipped: validation.skipped,
        };
        let risks = risk_analyzer::RiskAnalyzer::analyze(analysis, generation, validation);
        let suggestions = suggestion_builder::SuggestionBuilder::build(&risks, validation);
        let coverage_percent = if validation.total_interfaces > 0 {
            (validation.passed as f64 / validation.total_interfaces as f64) * 100.0
        } else {
            0.0
        };
        MigrationReport {
            migration_id: migration_id.to_string(),
            analysis_summary,
            generation_summary,
            validation_summary,
            risks,
            suggestions,
            coverage_percent,
        }
    }

    pub async fn write_markdown(
        report: &MigrationReport,
        output_path: &Path,
    ) -> MigrationResult<()> {
        let mut md = String::new();
        md.push_str("# 迁移总报告\n\n");
        md.push_str(&format!("**迁移 ID**: {}\n\n", report.migration_id));
        md.push_str(&format!("**覆盖率**: {:.1}%\n\n", report.coverage_percent));
        md.push_str("## 分析摘要\n\n");
        md.push_str(&format!(
            "- TP6 版本: {}\n",
            report.analysis_summary.tp6_version
        ));
        md.push_str(&format!(
            "- 路由: {}\n",
            report.analysis_summary.route_count
        ));
        md.push_str(&format!(
            "- 模型: {}\n",
            report.analysis_summary.model_count
        ));
        md.push_str(&format!(
            "- 控制器: {}\n",
            report.analysis_summary.controller_count
        ));
        md.push_str(&format!(
            "- 中间件: {}\n",
            report.analysis_summary.middleware_count
        ));
        md.push_str(&format!(
            "- 依赖环: {}\n\n",
            if report.analysis_summary.has_cycle {
                "有"
            } else {
                "无"
            }
        ));
        md.push_str("## 生成摘要\n\n");
        md.push_str(&format!(
            "- 生成文件: {}\n",
            report.generation_summary.generated_files
        ));
        md.push_str(&format!(
            "- 失败接口: {}\n",
            report.generation_summary.failed_interfaces
        ));
        md.push_str(&format!(
            "- 总重试: {}\n\n",
            report.generation_summary.total_retries
        ));
        md.push_str("## 验证摘要\n\n");
        md.push_str(&format!("- 总接口: {}\n", report.validation_summary.total));
        md.push_str(&format!("- 通过: {}\n", report.validation_summary.passed));
        md.push_str(&format!("- 失败: {}\n", report.validation_summary.failed));
        md.push_str(&format!(
            "- 跳过: {}\n\n",
            report.validation_summary.skipped
        ));
        if !report.risks.is_empty() {
            md.push_str("## 风险项\n\n");
            for risk in &report.risks {
                md.push_str(&format!(
                    "- **{}** ({}): {}\n",
                    risk.title, risk.severity, risk.description
                ));
            }
            md.push('\n');
        }
        if !report.suggestions.is_empty() {
            md.push_str("## 迁移建议\n\n");
            for s in &report.suggestions {
                md.push_str(&format!("- {}\n", s.description));
            }
        }
        tokio::fs::write(output_path, md).await?;
        Ok(())
    }
}
