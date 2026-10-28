// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::Path;

use crate::error::MigrationResult;
use crate::validator::ValidationReport;

pub struct ValidationReportWriter;

impl ValidationReportWriter {
    pub async fn write_json(report: &ValidationReport, output_path: &Path) -> MigrationResult<()> {
        let json = serde_json::to_string_pretty(report)?;
        tokio::fs::write(output_path, json).await?;
        Ok(())
    }

    pub async fn write_markdown(
        report: &ValidationReport,
        output_path: &Path,
    ) -> MigrationResult<()> {
        let mut md = String::new();
        md.push_str("# 增量验证报告\n\n");
        md.push_str(&format!(
            "## 总览\n\n- 总接口数: {}\n- 通过: {}\n- 失败: {}\n- 跳过: {}\n\n",
            report.total_interfaces, report.passed, report.failed, report.skipped
        ));
        md.push_str("## 详细结果\n\n");
        for result in &report.results {
            let status = if result.is_consistent { "✅" } else { "❌" };
            md.push_str(&format!(
                "### {} {}\n\n- 接口: {}\n- TP6 状态: {:?}\n- sz-rust 状态: {:?}\n",
                status,
                result.interface_id,
                result.request.path,
                result.tp6_status,
                result.sz_rust_status
            ));
            if let Some(diff) = &result.diff {
                md.push_str(&format!("- 差异: {}\n", diff));
            }
            md.push('\n');
        }
        tokio::fs::write(output_path, md).await?;
        Ok(())
    }
}
