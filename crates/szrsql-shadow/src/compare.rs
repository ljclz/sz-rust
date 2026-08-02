//! 结果比对器：比对 PG 18 与 szrsql 的执行结果
//!
//! # 比对规则
//!
//! 1. **行数比对**：双方行数必须一致
//! 2. **列数比对**：每行列数必须一致
//! 3. **值比对**：每个单元格的字符串表示必须一致
//!    - NULL → "NULL"
//!    - 整数 → 十进制字符串
//!    - 浮点 → 保留 6 位小数
//!    - 文本 → 原文
//! 4. **延迟比对**：记录 P50/P95/P99 延迟

use serde::{Deserialize, Serialize};

/// 单条 SQL 的比对结果
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MatchStatus {
    /// 结果完全一致
    Match,
    /// 结果不一致（含差异详情）
    Mismatch(String),
    /// PG 18 执行错误
    PgError(String),
    /// szrsql 执行错误
    SzError(String),
    /// 双方都执行错误（错误码比对，当前仅记录）
    BothError,
}

/// 单条 SQL 的回放结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResult {
    /// SQL 文本
    pub sql: String,
    /// PG 18 行数（SELECT 为结果行数，DML 为 affected_rows）
    pub pg_rows: i64,
    /// szrsql 行数
    pub sz_rows: i64,
    /// PG 18 执行延迟（毫秒）
    pub pg_latency_ms: f64,
    /// szrsql 执行延迟（毫秒）
    pub sz_latency_ms: f64,
    /// 比对状态
    pub status: MatchStatus,
}

/// 比对两个结果集（行数 + 每行每列）
///
/// # 参数
/// - `sql`: SQL 文本（用于错误消息）
/// - `sz_rows`: szrsql 结果集（字符串矩阵）
/// - `pg_rows`: PG 18 结果集（字符串矩阵）
pub fn compare_results(sql: &str, sz_rows: &[Vec<String>], pg_rows: &[Vec<String>]) -> MatchStatus {
    // 行数比对
    if sz_rows.len() != pg_rows.len() {
        return MatchStatus::Mismatch(format!(
            "row count mismatch for SQL [{sql}]: szrsql={}, pg={}",
            sz_rows.len(),
            pg_rows.len()
        ));
    }

    // 逐行逐列比对
    for (i, (sz_row, pg_row)) in sz_rows.iter().zip(pg_rows.iter()).enumerate() {
        if sz_row.len() != pg_row.len() {
            return MatchStatus::Mismatch(format!(
                "column count mismatch at row {i} for SQL [{sql}]: szrsql={}, pg={}",
                sz_row.len(),
                pg_row.len()
            ));
        }
        for (j, (sz_v, pg_v)) in sz_row.iter().zip(pg_row.iter()).enumerate() {
            if sz_v != pg_v {
                return MatchStatus::Mismatch(format!(
                    "value mismatch at row {i} col {j} for SQL [{sql}]: szrsql='{sz_v}', pg='{pg_v}'"
                ));
            }
        }
    }

    MatchStatus::Match
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_match() {
        let sz = vec![vec!["1".to_string(), "a".to_string()]];
        let pg = vec![vec!["1".to_string(), "a".to_string()]];
        assert_eq!(compare_results("SELECT 1", &sz, &pg), MatchStatus::Match);
    }

    #[test]
    fn compare_row_count_mismatch() {
        let sz = vec![vec!["1".to_string()]];
        let pg = vec![vec!["1".to_string()], vec!["2".to_string()]];
        match compare_results("SELECT 1", &sz, &pg) {
            MatchStatus::Mismatch(msg) => assert!(msg.contains("row count mismatch")),
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn compare_value_mismatch() {
        let sz = vec![vec!["1".to_string()]];
        let pg = vec![vec!["2".to_string()]];
        match compare_results("SELECT 1", &sz, &pg) {
            MatchStatus::Mismatch(msg) => assert!(msg.contains("value mismatch")),
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn compare_column_count_mismatch() {
        let sz = vec![vec!["1".to_string()]];
        let pg = vec![vec!["1".to_string(), "a".to_string()]];
        match compare_results("SELECT 1", &sz, &pg) {
            MatchStatus::Mismatch(msg) => assert!(msg.contains("column count mismatch")),
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn compare_empty_results_match() {
        let sz: Vec<Vec<String>> = vec![];
        let pg: Vec<Vec<String>> = vec![];
        assert_eq!(compare_results("SELECT 1", &sz, &pg), MatchStatus::Match);
    }
}
