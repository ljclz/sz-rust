//! 报告生成器：汇总回放统计 + 差异详情
//!
//! # 报告内容
//!
//! 1. **总体统计**：总 SQL 数、匹配数、不匹配数、PG 错误数、sz 错误数
//! 2. **延迟统计**：PG 18 / szrsql 的 P50/P95/P99 延迟
//! 3. **差异详情**：前 N 个不匹配 SQL 的详细信息
//! 4. **结论**：是否达到上线标准（匹配率 ≥ 99.5%）

use serde::{Deserialize, Serialize};

use crate::compare::{MatchStatus, ReplayResult};

/// 阴影回放报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadowReport {
    /// 总 SQL 数
    pub total: usize,
    /// 完全匹配数
    pub matched: usize,
    /// 不匹配数
    pub mismatched: usize,
    /// PG 18 执行错误数
    pub pg_errors: usize,
    /// szrsql 执行错误数
    pub sz_errors: usize,
    /// 双方都错误数
    pub both_errors: usize,
    /// 匹配率（0.0 ~ 1.0）
    pub match_rate: f64,
    /// PG 18 P50 延迟（毫秒）
    pub pg_p50_ms: f64,
    /// PG 18 P95 延迟（毫秒）
    pub pg_p95_ms: f64,
    /// PG 18 P99 延迟（毫秒）
    pub pg_p99_ms: f64,
    /// szrsql P50 延迟（毫秒）
    pub sz_p50_ms: f64,
    /// szrsql P95 延迟（毫秒）
    pub sz_p95_ms: f64,
    /// szrsql P99 延迟（毫秒）
    pub sz_p99_ms: f64,
    /// 是否通过上线标准
    pub passed: bool,
    /// 差异详情（前 10 个不匹配）
    pub mismatches: Vec<ReplayResult>,
}

impl ShadowReport {
    /// 从回放结果生成报告
    pub fn from_results(results: &[ReplayResult]) -> Self {
        let total = results.len();
        let mut matched = 0;
        let mut mismatched = 0;
        let mut pg_errors = 0;
        let mut sz_errors = 0;
        let mut both_errors = 0;

        let mut pg_latencies: Vec<f64> = Vec::with_capacity(total);
        let mut sz_latencies: Vec<f64> = Vec::with_capacity(total);
        let mut mismatches: Vec<ReplayResult> = Vec::new();

        for r in results {
            pg_latencies.push(r.pg_latency_ms);
            sz_latencies.push(r.sz_latency_ms);
            match &r.status {
                MatchStatus::Match => matched += 1,
                MatchStatus::Mismatch(_) => {
                    mismatched += 1;
                    if mismatches.len() < 10 {
                        mismatches.push(r.clone());
                    }
                }
                MatchStatus::PgError(_) => pg_errors += 1,
                MatchStatus::SzError(_) => sz_errors += 1,
                MatchStatus::BothError => both_errors += 1,
            }
        }

        let match_rate = if total > 0 {
            matched as f64 / total as f64
        } else {
            0.0
        };

        let pg_p50_ms = percentile(&pg_latencies, 50.0);
        let pg_p95_ms = percentile(&pg_latencies, 95.0);
        let pg_p99_ms = percentile(&pg_latencies, 99.0);
        let sz_p50_ms = percentile(&sz_latencies, 50.0);
        let sz_p95_ms = percentile(&sz_latencies, 95.0);
        let sz_p99_ms = percentile(&sz_latencies, 99.0);

        // 上线标准：匹配率 ≥ 99.5% 且无 PG 错误（PG 错误说明流量录制有问题）
        let passed = match_rate >= 0.995 && pg_errors == 0 && total > 0;

        Self {
            total,
            matched,
            mismatched,
            pg_errors,
            sz_errors,
            both_errors,
            match_rate,
            pg_p50_ms,
            pg_p95_ms,
            pg_p99_ms,
            sz_p50_ms,
            sz_p95_ms,
            sz_p99_ms,
            passed,
            mismatches,
        }
    }

    /// 序列化为 JSON 字符串
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// 生成 Markdown 摘要（用于人类阅读）
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# SzRSQL 影子回放报告\n\n");
        md.push_str("## 1. 总体统计\n\n");
        md.push_str("| 指标 | 值 |\n|------|------|\n");
        md.push_str(&format!("| 总 SQL 数 | {} |\n", self.total));
        md.push_str(&format!("| 完全匹配 | {} |\n", self.matched));
        md.push_str(&format!("| 不匹配 | {} |\n", self.mismatched));
        md.push_str(&format!("| PG 18 错误 | {} |\n", self.pg_errors));
        md.push_str(&format!("| szrsql 错误 | {} |\n", self.sz_errors));
        md.push_str(&format!("| 双方错误 | {} |\n", self.both_errors));
        md.push_str(&format!("| 匹配率 | {:.4}% |\n", self.match_rate * 100.0));
        md.push_str(&format!("| 上线标准 | {} |\n", if self.passed { "✅ 通过" } else { "❌ 未通过" }));

        md.push_str("\n## 2. 延迟统计\n\n");
        md.push_str("| 数据库 | P50 (ms) | P95 (ms) | P99 (ms) |\n");
        md.push_str("|--------|----------|----------|----------|\n");
        md.push_str(&format!(
            "| PG 18 | {:.3} | {:.3} | {:.3} |\n",
            self.pg_p50_ms, self.pg_p95_ms, self.pg_p99_ms
        ));
        md.push_str(&format!(
            "| szrsql | {:.3} | {:.3} | {:.3} |\n",
            self.sz_p50_ms, self.sz_p95_ms, self.sz_p99_ms
        ));

        if !self.mismatches.is_empty() {
            md.push_str("\n## 3. 差异详情（前 10 个）\n\n");
            for (i, r) in self.mismatches.iter().enumerate() {
                md.push_str(&format!("### {}. SQL: `{}`\n\n", i + 1, r.sql));
                if let MatchStatus::Mismatch(reason) = &r.status {
                    md.push_str(&format!("- **原因**：{reason}\n"));
                }
                md.push_str(&format!("- PG 18 行数：{}\n", r.pg_rows));
                md.push_str(&format!("- szrsql 行数：{}\n", r.sz_rows));
                md.push_str(&format!("- PG 18 延迟：{:.3} ms\n", r.pg_latency_ms));
                md.push_str(&format!("- szrsql 延迟：{:.3} ms\n", r.sz_latency_ms));
                md.push('\n');
            }
        }

        md
    }
}

/// 计算分位数（线性插值法）
///
/// # 参数
/// - `data`: 数据样本
/// - `p`: 百分位数（0.0 ~ 100.0）
fn percentile(data: &[f64], p: f64) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut sorted = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    if n == 1 {
        return sorted[0];
    }
    let rank = (p / 100.0) * (n - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    if lower == upper {
        return sorted[lower];
    }
    let frac = rank - lower as f64;
    sorted[lower] * (1.0 - frac) + sorted[upper] * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_all_match() {
        let results = vec![
            ReplayResult {
                sql: "SELECT 1".to_string(),
                pg_rows: 1,
                sz_rows: 1,
                pg_latency_ms: 1.0,
                sz_latency_ms: 0.5,
                status: MatchStatus::Match,
            },
            ReplayResult {
                sql: "SELECT 2".to_string(),
                pg_rows: 1,
                sz_rows: 1,
                pg_latency_ms: 2.0,
                sz_latency_ms: 1.0,
                status: MatchStatus::Match,
            },
        ];
        let report = ShadowReport::from_results(&results);
        assert_eq!(report.total, 2);
        assert_eq!(report.matched, 2);
        assert_eq!(report.mismatched, 0);
        assert_eq!(report.match_rate, 1.0);
        assert!(report.passed);
    }

    #[test]
    fn report_with_mismatches() {
        let results = vec![
            ReplayResult {
                sql: "SELECT 1".to_string(),
                pg_rows: 1,
                sz_rows: 1,
                pg_latency_ms: 1.0,
                sz_latency_ms: 0.5,
                status: MatchStatus::Match,
            },
            ReplayResult {
                sql: "SELECT 2".to_string(),
                pg_rows: 1,
                sz_rows: 0,
                pg_latency_ms: 1.0,
                sz_latency_ms: 0.5,
                status: MatchStatus::Mismatch("row count mismatch".to_string()),
            },
        ];
        let report = ShadowReport::from_results(&results);
        assert_eq!(report.matched, 1);
        assert_eq!(report.mismatched, 1);
        assert!(!report.passed); // 匹配率 50% < 99.5%
        assert_eq!(report.mismatches.len(), 1);
    }

    #[test]
    fn report_empty_results() {
        let report = ShadowReport::from_results(&[]);
        assert_eq!(report.total, 0);
        assert_eq!(report.match_rate, 0.0);
        assert!(!report.passed); // 0 个 SQL 不算通过
    }

    #[test]
    fn percentile_basic() {
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((percentile(&data, 50.0) - 3.0).abs() < 0.001);
        assert!((percentile(&data, 0.0) - 1.0).abs() < 0.001);
        assert!((percentile(&data, 100.0) - 5.0).abs() < 0.001);
    }

    #[test]
    fn percentile_empty() {
        assert_eq!(percentile(&[], 50.0), 0.0);
    }

    #[test]
    fn report_to_json_works() {
        let results = vec![ReplayResult {
            sql: "SELECT 1".to_string(),
            pg_rows: 1,
            sz_rows: 1,
            pg_latency_ms: 1.0,
            sz_latency_ms: 0.5,
            status: MatchStatus::Match,
        }];
        let report = ShadowReport::from_results(&results);
        let json = report.to_json().unwrap();
        // to_string_pretty 默认带空格 `"passed": true`
        assert!(json.contains("\"passed\":") && json.contains("true"));
    }

    #[test]
    fn report_to_markdown_works() {
        let results = vec![ReplayResult {
            sql: "SELECT 1".to_string(),
            pg_rows: 1,
            sz_rows: 1,
            pg_latency_ms: 1.0,
            sz_latency_ms: 0.5,
            status: MatchStatus::Match,
        }];
        let report = ShadowReport::from_results(&results);
        let md = report.to_markdown();
        assert!(md.contains("# SzRSQL 影子回放报告"));
        assert!(md.contains("匹配率"));
    }
}
