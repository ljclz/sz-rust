// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T2.10 基线报告生成器
//!
//! 读取 criterion --save-baseline 输出 JSON，生成 markdown 报告
//! 对应 spec 4.4.1（数字附来源标注）

use std::collections::BTreeMap;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let crate_version = env!("CARGO_PKG_VERSION");
    let commit_hash = get_commit_hash().await?;
    let environment = get_environment_info().await?;
    let generated_at = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();

    let baseline_dir = std::path::Path::new("target/criterion");
    let mut sections = BTreeMap::new();

    let bench_names = [
        "chat_latency",
        "stream_throughput",
        "embedding_batch",
        "rag_three_stage",
        "agent_multi_round",
        "concurrent_qps_p99",
        "context_truncation",
        "memory_rss",
    ];

    for name in &bench_names {
        let estimates_path = baseline_dir.join(name).join("estimates.json");
        if estimates_path.exists() {
            let content = tokio::fs::read_to_string(&estimates_path).await?;
            sections.insert(name.to_string(), parse_estimates(&content));
        } else {
            sections.insert(
                name.to_string(),
                "（未找到基线数据，请先运行 `cargo bench --bench ai_facade_bench -- --save-baseline current`）".to_string(),
            );
        }
    }

    let report = build_report(
        crate_version,
        &commit_hash,
        &environment,
        &generated_at,
        &sections,
    );

    let report_path = std::path::Path::new("docs/audit/ai-facade-performance-baseline.md");
    if let Some(parent) = report_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&report_path, &report).await?;
    println!("报告已生成: {}", report_path.display());
    Ok(())
}

fn parse_estimates(json: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
        let mut parts = Vec::new();

        if let Some(mean) = v.get("mean").and_then(|m| m.get("point_estimate")) {
            parts.push(format!(
                "| 指标 | 值 | 来源 |\n|------|----|------|\n| Mean | {:.3} ns | criterion estimates.json:mean.point_estimate |",
                mean.as_f64().unwrap_or(0.0)
            ));
        }

        if let Some(p99) = v
            .get("median")
            .and_then(|m| m.get("confidence_interval"))
            .and_then(|ci| ci.get("upper_bound"))
        {
            parts.push(format!(
                "| Median Upper Bound | {:.3} ns | criterion estimates.json:median.confidence_interval.upper_bound |",
                p99.as_f64().unwrap_or(0.0)
            ));
        }

        if let Some(stddev) = v.get("std_dev").and_then(|s| s.get("point_estimate")) {
            parts.push(format!(
                "| Std Dev | {:.3} ns | criterion estimates.json:std_dev.point_estimate |",
                stddev.as_f64().unwrap_or(0.0)
            ));
        }

        if parts.is_empty() {
            "（无法解析 estimates.json）".to_string()
        } else {
            parts.join("\n")
        }
    } else {
        "（JSON 解析失败）".to_string()
    }
}

fn build_report(
    crate_version: &str,
    commit_hash: &str,
    environment: &str,
    generated_at: &str,
    sections: &BTreeMap<String, String>,
) -> String {
    let mut report = String::new();

    report.push_str("# AI Facade 性能基线报告\n\n");
    report.push_str("> 自动生成，禁止手动编辑。所有数字附来源标注（spec 4.4.1）。\n\n");
    report.push_str("## 元信息\n\n");
    report.push_str("| 字段 | 值 | 来源 |\n");
    report.push_str("|------|----|------|\n");
    report.push_str(&format!(
        "| crate_version | {} | env!(CARGO_PKG_VERSION) |\n",
        crate_version
    ));
    report.push_str(&format!(
        "| commit_hash | {} | git rev-parse HEAD |\n",
        commit_hash
    ));
    report.push_str(&format!(
        "| environment | {} | uname -a / system info |\n",
        environment
    ));
    report.push_str(&format!(
        "| generated_at | {} | chrono::Utc::now() |\n",
        generated_at
    ));
    report.push_str(
        "| sampling_method | criterion 0.5 estimates.json | criterion --save-baseline |\n",
    );
    report.push_str(
        "| mock_environment | true | 所有 Provider 指向 mock，排除网络（spec 5.2.1.9） |\n\n",
    );

    report.push_str("## 基准结果\n\n");
    for (name, content) in sections {
        report.push_str(&format!("### {}\n\n", name));
        report.push_str(content);
        report.push_str("\n\n");
    }

    report.push_str("## 与上一版本对比\n\n");
    report.push_str("| 指标 | 当前基线 | 上一基线 | 对比结果 | 来源 |\n");
    report.push_str("|------|---------|---------|---------|------|\n");
    report.push_str("| chat_latency P99 | 见上方 | 待对比 | na | 需先运行 --baseline previous |\n");
    report.push_str(
        "| stream_throughput TTFT | 见上方 | 待对比 | na | 需先运行 --baseline previous |\n",
    );
    report.push_str(
        "| embedding_batch items/s | 见上方 | 待对比 | na | 需先运行 --baseline previous |\n",
    );
    report.push_str(
        "| rag_three_stage 总耗时 | 见上方 | 待对比 | na | 需先运行 --baseline previous |\n",
    );
    report.push_str(
        "| agent_multi_round 总延迟 | 见上方 | 待对比 | na | 需先运行 --baseline previous |\n",
    );
    report.push_str(
        "| concurrent_qps_p99 QPS | 见上方 | 待对比 | na | 需先运行 --baseline previous |\n",
    );
    report.push_str(
        "| context_truncation P99 | 见上方 | 待对比 | na | 需先运行 --baseline previous |\n",
    );
    report.push_str(
        "| memory_rss 中位数 | 见上方 | 待对比 | na | 需先运行 --baseline previous |\n\n",
    );

    report.push_str("## CI 门禁\n\n");
    report.push_str("- 回归门禁：`cargo bench --bench ai_facade_bench -- --baseline previous`，criterion 输出回归项时 CI 失败（spec 4.4.4）\n");
    report.push_str("- 单场景执行时间 ≤ 5 分钟（spec 4.1.2）\n");
    report.push_str("- 总压测时长 ≤ 30 分钟（8 场景 × 5 分钟）\n");
    report.push_str(
        "- 所有 Provider 指向 mock server，报告标注\"mock 环境，排除网络\"（spec 5.2.1.9）\n",
    );

    report
}

async fn get_commit_hash() -> Result<String, std::io::Error> {
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn get_environment_info() -> Result<String, std::io::Error> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    Ok(format!("{} {}", os, arch))
}
