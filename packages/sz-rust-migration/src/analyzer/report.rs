// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::Path;

use crate::analyzer::Tp6AnalysisReport;
use crate::error::MigrationResult;

pub struct AnalysisReportWriter;

impl AnalysisReportWriter {
    pub async fn write_json(report: &Tp6AnalysisReport, output_path: &Path) -> MigrationResult<()> {
        let json = serde_json::to_string_pretty(report)?;
        tokio::fs::write(output_path, json).await?;
        Ok(())
    }

    pub async fn write_markdown(
        report: &Tp6AnalysisReport,
        output_path: &Path,
    ) -> MigrationResult<()> {
        let mut md = String::new();
        md.push_str("# TP6 项目分析报告\n\n");
        md.push_str(&format!("## 版本: {}\n\n", report.version));
        md.push_str(&format!("## 路由 ({})\n\n", report.routes.len()));
        for route in &report.routes {
            md.push_str(&format!(
                "- `{}` {} → {}::{}\n",
                route.method, route.path_pattern, route.controller, route.action
            ));
        }
        md.push_str(&format!("\n## 模型 ({})\n\n", report.models.len()));
        for model in &report.models {
            md.push_str(&format!(
                "- **{}** (table: {}), relations: {}\n",
                model.class_name,
                model.table_name,
                model.relations.len()
            ));
        }
        md.push_str(&format!("\n## 控制器 ({})\n\n", report.controllers.len()));
        for controller in &report.controllers {
            md.push_str(&format!(
                "- **{}** (actions: {})\n",
                controller.class_name,
                controller.actions.len()
            ));
        }
        md.push_str(&format!("\n## 中间件 ({})\n\n", report.middlewares.len()));
        for mw in &report.middlewares {
            md.push_str(&format!("- {}\n", mw.class_name));
        }
        md.push_str("\n## 依赖图\n\n");
        md.push_str(&format!(
            "- 节点: {}\n",
            report.dependency_graph.nodes.len()
        ));
        md.push_str(&format!("- 边: {}\n", report.dependency_graph.edges.len()));
        md.push_str(&format!(
            "- 环检测: {}\n",
            if report.dependency_graph.has_cycle {
                "有环"
            } else {
                "无环"
            }
        ));
        md.push_str("\n## 复杂度评估\n\n");
        md.push_str(&format!(
            "- Low: {}\n- Medium: {}\n- High: {}\n",
            report.complexity_summary.low,
            report.complexity_summary.medium,
            report.complexity_summary.high
        ));
        tokio::fs::write(output_path, md).await?;
        Ok(())
    }
}
