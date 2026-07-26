//! Phase 7d.20 — pg_stat_statements 查询统计。
//!
//! 提供类似 PostgreSQL pg_stat_statements 视图的查询级聚合统计：
//! 每个归一化 SQL 的调用次数、总耗时、平均/最小/最大耗时、返回行数等。
//!
//! # 设计
//!
//! - `normalize_sql()` SQL 归一化：替换数字、字符串常量为 `?`，便于聚合相同模板的查询
//! - `query_id()` 归一化 SQL 的稳定 hash（u64），作为查询统计聚合键
//! - `QueryStats` 单查询统计（calls/total_time_ms/min/max/mean/stdev/rows）
//! - `QueryStatsCollector` 收集器：按 query_id 聚合
//! - 提供 `to_pg_stat_statements_rows()` 返回 pg_stat_statements 视图行格式
//!
//! # pg_stat_statements 视图字段
//!
//! ```sql
//! SELECT queryid, query, calls, total_time, mean_time, min_time, max_time, rows
//! FROM pg_stat_statements;
//! ```
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_ops::query_stats::QueryStatsCollector;
//!
//! let mut c = QueryStatsCollector::new();
//! c.record_query("SELECT * FROM users WHERE id = 1", 5.2, 10);
//! c.record_query("SELECT * FROM users WHERE id = 2", 4.8, 10);
//! // 两条查询归一化后相同，统计合并
//! let rows = c.to_pg_stat_statements_rows();
//! assert_eq!(rows.len(), 1);
//! assert_eq!(rows[0].calls, 2);
//! ```

use std::collections::HashMap;

// =====================================================================
//  常量
// =====================================================================

/// SQL 文本最大长度（截断超长 SQL，防止内存爆炸）。
pub const MAX_QUERY_TEXT_LEN: usize = 1024;

/// 默认 Top N 数量。
pub const DEFAULT_TOP_N: usize = 10;

// =====================================================================
//  SQL 归一化
// =====================================================================

/// SQL 归一化：将数字、字符串、日期等常量替换为 `?`，便于聚合相同模板的查询。
///
/// 例如：
/// - `SELECT * FROM t WHERE id = 1` → `SELECT * FROM t WHERE id = ?`
/// - `SELECT * FROM t WHERE name = 'foo'` → `SELECT * FROM t WHERE name = ?`
/// - `INSERT INTO t VALUES (1, 'a', 2.5)` → `INSERT INTO t VALUES (?, ?, ?)`
///
/// 仅做词法级替换，不解析 SQL 语法，足够用于查询统计聚合。
pub fn normalize_sql(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut result = String::with_capacity(sql.len());
    let mut i = 0;
    let n = chars.len();

    while i < n {
        let c = chars[i];

        // 字符串字面量 '...'（含转义 ''）
        if c == '\'' {
            result.push('?');
            i += 1;
            while i < n {
                if chars[i] == '\'' {
                    // 检查是否为转义 ''
                    if i + 1 < n && chars[i + 1] == '\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // 数字字面量（含小数、负号前缀由调用方处理）
        if c.is_ascii_digit() {
            result.push('?');
            i += 1;
            // 跳过后续数字和小数点
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            continue;
        }

        // 双引号标识符 "..."（保持不变，不替换）
        if c == '"' {
            result.push(c);
            i += 1;
            while i < n {
                result.push(chars[i]);
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // 单行注释 --...
        if c == '-' && i + 1 < n && chars[i + 1] == '-' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        // 多行注释 /*...*/
        if c == '/' && i + 1 < n && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 < n {
                i += 2;
            }
            continue;
        }

        // 大小写归一化为小写（PostgreSQL 默认 unquoted identifier 折叠为小写）
        if c.is_ascii_uppercase() {
            result.push(c.to_ascii_lowercase());
        } else {
            result.push(c);
        }
        i += 1;
    }

    // 截断超长 SQL
    if result.len() > MAX_QUERY_TEXT_LEN {
        result.truncate(MAX_QUERY_TEXT_LEN);
    }
    result
}

/// 计算归一化 SQL 的稳定 query_id（u64 hash）。
///
/// 使用 FNV-1a 变体（与 PostgreSQL pg_stat_statements 的 queryid 语义一致：
/// 相同归一化 SQL → 相同 query_id）。
pub fn query_id(normalized_sql: &str) -> u64 {
    // FNV-1a 64-bit
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for b in normalized_sql.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// =====================================================================
//  QueryStats — 单查询统计
// =====================================================================

/// 单个归一化查询的聚合统计（类似 pg_stat_statements 单行）。
#[derive(Debug, Clone, PartialEq)]
pub struct QueryStats {
    /// 归一化 SQL 的稳定 hash（query_id）。
    pub query_id: u64,
    /// 归一化后的 SQL 文本（常量替换为 `?`，小写折叠）。
    pub query: String,
    /// 调用次数。
    pub calls: u64,
    /// 总耗时（毫秒）。
    pub total_time_ms: f64,
    /// 最小单次耗时（毫秒）。
    pub min_time_ms: f64,
    /// 最大单次耗时（毫秒）。
    pub max_time_ms: f64,
    /// 总返回行数。
    pub rows: u64,
}

impl QueryStats {
    /// 创建空统计。
    pub fn new(query_id: u64, query: String) -> Self {
        Self {
            query_id,
            query,
            calls: 0,
            total_time_ms: 0.0,
            min_time_ms: f64::MAX,
            max_time_ms: 0.0,
            rows: 0,
        }
    }

    /// 平均耗时（毫秒）。
    pub fn mean_time_ms(&self) -> f64 {
        if self.calls == 0 {
            0.0
        } else {
            self.total_time_ms / self.calls as f64
        }
    }

    /// 平均返回行数。
    pub fn mean_rows(&self) -> f64 {
        if self.calls == 0 {
            0.0
        } else {
            self.rows as f64 / self.calls as f64
        }
    }

    /// 记录一次查询执行。
    pub fn record(&mut self, elapsed_ms: f64, row_count: u64) {
        self.calls += 1;
        self.total_time_ms += elapsed_ms;
        if elapsed_ms < self.min_time_ms {
            self.min_time_ms = elapsed_ms;
        }
        if elapsed_ms > self.max_time_ms {
            self.max_time_ms = elapsed_ms;
        }
        self.rows += row_count;
    }
}

// =====================================================================
//  PgStatStatementsRow — pg_stat_statements 视图行
// =====================================================================

/// pg_stat_statements 视图单行（PostgreSQL 风格）。
#[derive(Debug, Clone, PartialEq)]
pub struct PgStatStatementsRow {
    /// 归一化 SQL 的稳定 hash。
    pub queryid: u64,
    /// 归一化后的 SQL 文本。
    pub query: String,
    /// 调用次数。
    pub calls: u64,
    /// 总耗时（毫秒）。
    pub total_time: f64,
    /// 平均耗时（毫秒）。
    pub mean_time: f64,
    /// 最小单次耗时（毫秒）。
    pub min_time: f64,
    /// 最大单次耗时（毫秒）。
    pub max_time: f64,
    /// 总返回行数。
    pub rows: u64,
}

impl From<&QueryStats> for PgStatStatementsRow {
    fn from(stats: &QueryStats) -> Self {
        Self {
            queryid: stats.query_id,
            query: stats.query.clone(),
            calls: stats.calls,
            total_time: stats.total_time_ms,
            mean_time: stats.mean_time_ms(),
            min_time: if stats.calls == 0 {
                0.0
            } else {
                stats.min_time_ms
            },
            max_time: stats.max_time_ms,
            rows: stats.rows,
        }
    }
}

// =====================================================================
//  QueryStatsCollector — 查询统计收集器
// =====================================================================

/// 查询统计收集器：按 query_id 聚合。
///
/// 线程安全策略：本类型非 `Sync`，应由单线程或外部锁保护。
pub struct QueryStatsCollector {
    /// 按 query_id 聚合的统计。
    stats: HashMap<u64, QueryStats>,
}

impl Default for QueryStatsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryStatsCollector {
    /// 创建空收集器。
    pub fn new() -> Self {
        Self {
            stats: HashMap::new(),
        }
    }

    /// 记录一次查询执行。
    ///
    /// - `sql`：原始 SQL（将自动归一化）
    /// - `elapsed_ms`：本次执行耗时（毫秒）
    /// - `row_count`：本次返回行数
    pub fn record_query(&mut self, sql: &str, elapsed_ms: f64, row_count: u64) {
        let normalized = normalize_sql(sql);
        let qid = query_id(&normalized);
        let entry = self
            .stats
            .entry(qid)
            .or_insert_with(|| QueryStats::new(qid, normalized));
        entry.record(elapsed_ms, row_count);
    }

    /// 获取指定 query_id 的统计（若存在）。
    pub fn get(&self, query_id: u64) -> Option<&QueryStats> {
        self.stats.get(&query_id)
    }

    /// 已统计的查询模板数量。
    pub fn query_count(&self) -> usize {
        self.stats.len()
    }

    /// 总调用次数（所有查询合计）。
    pub fn total_calls(&self) -> u64 {
        self.stats.values().map(|s| s.calls).sum()
    }

    /// 总耗时（毫秒，所有查询合计）。
    pub fn total_time_ms(&self) -> f64 {
        self.stats.values().map(|s| s.total_time_ms).sum()
    }

    /// 总返回行数（所有查询合计）。
    pub fn total_rows(&self) -> u64 {
        self.stats.values().map(|s| s.rows).sum()
    }

    /// 清空所有统计。
    pub fn clear(&mut self) {
        self.stats.clear();
    }

    /// 返回按总耗时降序排列的所有查询统计。
    pub fn sorted_by_total_time(&self) -> Vec<&QueryStats> {
        let mut list: Vec<_> = self.stats.values().collect();
        list.sort_by(|a, b| {
            b.total_time_ms
                .partial_cmp(&a.total_time_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        list
    }

    /// 返回 Top N 查询（按总耗时降序）。
    pub fn top_queries_by_time(&self, n: usize) -> Vec<&QueryStats> {
        let mut list: Vec<_> = self.stats.values().collect();
        list.sort_by(|a, b| {
            b.total_time_ms
                .partial_cmp(&a.total_time_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        list.into_iter().take(n).collect()
    }

    /// 返回 Top N 查询（按调用次数降序）。
    pub fn top_queries_by_calls(&self, n: usize) -> Vec<&QueryStats> {
        let mut list: Vec<_> = self.stats.values().collect();
        list.sort_by_key(|b| std::cmp::Reverse(b.calls));
        list.into_iter().take(n).collect()
    }

    /// 转换为 pg_stat_statements 视图行列表（按总耗时降序）。
    ///
    /// 等价于 `SELECT queryid, query, calls, total_time, mean_time, min_time, max_time, rows FROM pg_stat_statements ORDER BY total_time DESC;`
    pub fn to_pg_stat_statements_rows(&self) -> Vec<PgStatStatementsRow> {
        self.sorted_by_total_time()
            .into_iter()
            .map(PgStatStatementsRow::from)
            .collect()
    }

    /// 重置统计但保留查询模板（calls/time/rows 清零）。
    ///
    /// 用于 AWR 快照间隔统计：在快照点 T1 调用 reset_stats，
    /// 在 T2 调用 to_pg_stat_statements_rows 获取 T1→T2 区间统计。
    pub fn reset_stats(&mut self) {
        for stats in self.stats.values_mut() {
            stats.calls = 0;
            stats.total_time_ms = 0.0;
            stats.min_time_ms = f64::MAX;
            stats.max_time_ms = 0.0;
            stats.rows = 0;
        }
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== normalize_sql ====================

    #[test]
    fn test_normalize_integer() {
        let normalized = normalize_sql("SELECT * FROM t WHERE id = 1");
        assert_eq!(normalized, "select * from t where id = ?");
    }

    #[test]
    fn test_normalize_float() {
        let normalized = normalize_sql("SELECT * FROM t WHERE price = 99.95");
        assert_eq!(normalized, "select * from t where price = ?");
    }

    #[test]
    fn test_normalize_string_literal() {
        let normalized = normalize_sql("SELECT * FROM t WHERE name = 'foo'");
        assert_eq!(normalized, "select * from t where name = ?");
    }

    #[test]
    fn test_normalize_string_with_escape() {
        // SQL 转义 '' 表示字面量 '
        let normalized = normalize_sql("SELECT * FROM t WHERE name = 'it''s'");
        assert_eq!(normalized, "select * from t where name = ?");
    }

    #[test]
    fn test_normalize_multiple_constants() {
        let normalized = normalize_sql("INSERT INTO t VALUES (1, 'a', 2.5)");
        assert_eq!(normalized, "insert into t values (?, ?, ?)");
    }

    #[test]
    fn test_normalize_case_folding() {
        let normalized = normalize_sql("SELECT ID, Name FROM Users WHERE ID = 1");
        assert_eq!(normalized, "select id, name from users where id = ?");
    }

    #[test]
    fn test_normalize_preserves_quoted_identifier() {
        let normalized = normalize_sql("SELECT \"MyColumn\" FROM \"MyTable\"");
        assert_eq!(normalized, "select \"MyColumn\" from \"MyTable\"");
    }

    #[test]
    fn test_normalize_strips_line_comment() {
        let normalized = normalize_sql("SELECT 1 -- this is a comment\nFROM t");
        assert_eq!(normalized, "select ? \nfrom t");
    }

    #[test]
    fn test_normalize_strips_block_comment() {
        let normalized = normalize_sql("SELECT 1 /* comment */ FROM t");
        assert_eq!(normalized, "select ?  from t");
    }

    #[test]
    fn test_normalize_truncates_long_sql() {
        let long_sql = format!("SELECT {}", "1, ".repeat(10000));
        let normalized = normalize_sql(&long_sql);
        assert!(normalized.len() <= MAX_QUERY_TEXT_LEN);
    }

    #[test]
    fn test_normalize_same_template_different_constants() {
        let a = normalize_sql("SELECT * FROM t WHERE id = 1");
        let b = normalize_sql("SELECT * FROM t WHERE id = 999");
        assert_eq!(a, b);
    }

    // ==================== query_id ====================

    #[test]
    fn test_query_id_stable_for_same_normalized() {
        let sql1 = "SELECT * FROM t WHERE id = 1";
        let sql2 = "SELECT * FROM t WHERE id = 999";
        assert_eq!(
            query_id(&normalize_sql(sql1)),
            query_id(&normalize_sql(sql2))
        );
    }

    #[test]
    fn test_query_id_different_for_different_templates() {
        let sql1 = "SELECT * FROM t WHERE id = 1";
        let sql2 = "SELECT * FROM t WHERE name = 'foo'";
        assert_ne!(
            query_id(&normalize_sql(sql1)),
            query_id(&normalize_sql(sql2))
        );
    }

    #[test]
    fn test_query_id_deterministic() {
        let sql = "select * from t where id = ?";
        assert_eq!(query_id(sql), query_id(sql));
    }

    // ==================== QueryStats ====================

    #[test]
    fn test_query_stats_new() {
        let stats = QueryStats::new(42, "select * from t".to_string());
        assert_eq!(stats.query_id, 42);
        assert_eq!(stats.query, "select * from t");
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.total_time_ms, 0.0);
        assert_eq!(stats.rows, 0);
        assert_eq!(stats.mean_time_ms(), 0.0);
        assert_eq!(stats.mean_rows(), 0.0);
    }

    #[test]
    fn test_query_stats_record() {
        let mut stats = QueryStats::new(1, "select * from t".to_string());
        stats.record(10.0, 5);
        stats.record(20.0, 15);
        stats.record(30.0, 10);

        assert_eq!(stats.calls, 3);
        assert_eq!(stats.total_time_ms, 60.0);
        assert_eq!(stats.min_time_ms, 10.0);
        assert_eq!(stats.max_time_ms, 30.0);
        assert_eq!(stats.rows, 30);
        assert_eq!(stats.mean_time_ms(), 20.0);
        assert_eq!(stats.mean_rows(), 10.0);
    }

    // ==================== PgStatStatementsRow ====================

    #[test]
    fn test_pg_stat_statements_row_from_stats() {
        let mut stats = QueryStats::new(42, "select * from t".to_string());
        stats.record(10.0, 5);
        stats.record(20.0, 15);

        let row = PgStatStatementsRow::from(&stats);
        assert_eq!(row.queryid, 42);
        assert_eq!(row.query, "select * from t");
        assert_eq!(row.calls, 2);
        assert_eq!(row.total_time, 30.0);
        assert_eq!(row.mean_time, 15.0);
        assert_eq!(row.min_time, 10.0);
        assert_eq!(row.max_time, 20.0);
        assert_eq!(row.rows, 20);
    }

    #[test]
    fn test_pg_stat_statements_row_zero_calls() {
        let stats = QueryStats::new(0, "select 1".to_string());
        let row = PgStatStatementsRow::from(&stats);
        assert_eq!(row.min_time, 0.0); // 0 次调用时 min_time 应为 0
        assert_eq!(row.calls, 0);
    }

    // ==================== QueryStatsCollector ====================

    #[test]
    fn test_collector_record_single_query() {
        let mut c = QueryStatsCollector::new();
        c.record_query("SELECT * FROM t WHERE id = 1", 5.0, 1);
        c.record_query("SELECT * FROM t WHERE id = 2", 7.0, 1);

        // 两条 SQL 归一化后相同，应合并
        assert_eq!(c.query_count(), 1);
        assert_eq!(c.total_calls(), 2);
        assert_eq!(c.total_time_ms(), 12.0);
        assert_eq!(c.total_rows(), 2);
    }

    #[test]
    fn test_collector_record_distinct_queries() {
        let mut c = QueryStatsCollector::new();
        c.record_query("SELECT * FROM t WHERE id = 1", 5.0, 1);
        c.record_query("SELECT * FROM t WHERE name = 'foo'", 10.0, 1);
        c.record_query("UPDATE t SET x = 1 WHERE id = 2", 3.0, 0);

        assert_eq!(c.query_count(), 3);
        assert_eq!(c.total_calls(), 3);
    }

    #[test]
    fn test_collector_clear() {
        let mut c = QueryStatsCollector::new();
        c.record_query("SELECT 1", 1.0, 1);
        assert_eq!(c.query_count(), 1);

        c.clear();
        assert_eq!(c.query_count(), 0);
    }

    #[test]
    fn test_collector_sorted_by_total_time() {
        let mut c = QueryStatsCollector::new();
        c.record_query("SELECT * FROM a", 10.0, 1);
        c.record_query("SELECT * FROM b", 50.0, 1);
        c.record_query("SELECT * FROM c", 30.0, 1);

        let sorted = c.sorted_by_total_time();
        assert_eq!(sorted.len(), 3);
        // 降序：b(50) > c(30) > a(10)
        assert!(sorted[0].total_time_ms >= sorted[1].total_time_ms);
        assert!(sorted[1].total_time_ms >= sorted[2].total_time_ms);
        assert_eq!(sorted[0].query, "select * from b");
    }

    #[test]
    fn test_collector_top_queries_by_time() {
        let mut c = QueryStatsCollector::new();
        c.record_query("SELECT * FROM a", 10.0, 1);
        c.record_query("SELECT * FROM b", 50.0, 1);
        c.record_query("SELECT * FROM c", 30.0, 1);
        c.record_query("SELECT * FROM d", 5.0, 1);

        let top2 = c.top_queries_by_time(2);
        assert_eq!(top2.len(), 2);
        assert!(top2[0].total_time_ms >= top2[1].total_time_ms);
    }

    #[test]
    fn test_collector_top_queries_by_calls() {
        let mut c = QueryStatsCollector::new();
        // query A: 5 次
        for _ in 0..5 {
            c.record_query("SELECT * FROM a", 1.0, 1);
        }
        // query B: 10 次
        for _ in 0..10 {
            c.record_query("SELECT * FROM b", 1.0, 1);
        }

        let top_by_calls = c.top_queries_by_calls(1);
        assert_eq!(top_by_calls.len(), 1);
        assert_eq!(top_by_calls[0].calls, 10);
        assert_eq!(top_by_calls[0].query, "select * from b");
    }

    #[test]
    fn test_collector_to_pg_stat_statements_rows() {
        let mut c = QueryStatsCollector::new();
        c.record_query("SELECT * FROM a", 10.0, 1);
        c.record_query("SELECT * FROM b", 50.0, 1);

        let rows = c.to_pg_stat_statements_rows();
        assert_eq!(rows.len(), 2);
        // 降序：b(50) > a(10)
        assert_eq!(rows[0].query, "select * from b");
        assert_eq!(rows[0].total_time, 50.0);
        assert_eq!(rows[1].query, "select * from a");
        assert_eq!(rows[1].total_time, 10.0);
    }

    #[test]
    fn test_collector_reset_stats() {
        let mut c = QueryStatsCollector::new();
        c.record_query("SELECT * FROM a", 10.0, 1);
        c.record_query("SELECT * FROM a", 20.0, 1);
        assert_eq!(c.total_calls(), 2);

        c.reset_stats();
        // 保留查询模板，但清零统计
        assert_eq!(c.query_count(), 1);
        assert_eq!(c.total_calls(), 0);
        assert_eq!(c.total_time_ms(), 0.0);
        assert_eq!(c.total_rows(), 0);

        // 再次记录可继续聚合
        c.record_query("SELECT * FROM a", 5.0, 1);
        assert_eq!(c.total_calls(), 1);
        assert_eq!(c.total_time_ms(), 5.0);
    }

    // ==================== 端到端：模拟查询统计 ====================

    #[test]
    fn test_end_to_end_query_stats() {
        // 模拟：3 个查询模板，共 100 次调用
        let mut c = QueryStatsCollector::new();

        // query1: SELECT * FROM users WHERE id = ?  调用 50 次，每次 2ms
        for i in 0..50u32 {
            c.record_query(&format!("SELECT * FROM users WHERE id = {}", i), 2.0, 1);
        }
        // query2: SELECT * FROM orders WHERE user_id = ?  调用 30 次，每次 5ms
        for i in 0..30u32 {
            c.record_query(
                &format!("SELECT * FROM orders WHERE user_id = {}", i),
                5.0,
                10,
            );
        }
        // query3: INSERT INTO logs VALUES (?, ?)  调用 20 次，每次 1ms
        for i in 0..20u32 {
            c.record_query(&format!("INSERT INTO logs VALUES ({}, 'msg')", i), 1.0, 0);
        }

        let rows = c.to_pg_stat_statements_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(c.total_calls(), 100);

        // 降序：query2(150ms) > query1(100ms) > query3(20ms)
        assert_eq!(rows[0].query, "select * from orders where user_id = ?");
        assert_eq!(rows[0].calls, 30);
        assert_eq!(rows[0].total_time, 150.0);
        assert_eq!(rows[0].mean_time, 5.0);
        assert_eq!(rows[0].rows, 300);

        assert_eq!(rows[1].query, "select * from users where id = ?");
        assert_eq!(rows[1].calls, 50);
        assert_eq!(rows[1].total_time, 100.0);
        assert_eq!(rows[1].mean_time, 2.0);

        assert_eq!(rows[2].query, "insert into logs values (?, ?)");
        assert_eq!(rows[2].calls, 20);
        assert_eq!(rows[2].total_time, 20.0);
        assert_eq!(rows[2].rows, 0);

        // Top 2 by time
        let top2 = c.top_queries_by_time(2);
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].query, "select * from orders where user_id = ?");
        assert_eq!(top2[1].query, "select * from users where id = ?");

        // Top 1 by calls
        let top1_calls = c.top_queries_by_calls(1);
        assert_eq!(top1_calls[0].calls, 50);
        assert_eq!(top1_calls[0].query, "select * from users where id = ?");
    }

    #[test]
    fn test_end_to_end_awr_snapshot_interval() {
        // 模拟 AWR 快照间隔统计：T1 记录 → reset → T2 记录 → 仅获取 T1→T2 区间统计
        let mut c = QueryStatsCollector::new();

        // T0 之前的历史记录
        c.record_query("SELECT * FROM a", 100.0, 1);
        c.record_query("SELECT * FROM a", 100.0, 1);

        // 快照点 T1：reset
        c.reset_stats();

        // T1 → T2 区间的查询
        c.record_query("SELECT * FROM a", 5.0, 1);
        c.record_query("SELECT * FROM a", 10.0, 1);
        c.record_query("SELECT * FROM b", 50.0, 1);

        // 获取 T1→T2 区间统计
        let rows = c.to_pg_stat_statements_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(c.total_calls(), 3);
        // a: 5+10=15ms, b: 50ms → 降序 b, a
        assert_eq!(rows[0].query, "select * from b");
        assert_eq!(rows[0].total_time, 50.0);
        assert_eq!(rows[1].query, "select * from a");
        assert_eq!(rows[1].total_time, 15.0);
        assert_eq!(rows[1].calls, 2);
    }

    #[test]
    fn test_end_to_edge_empty_collector() {
        let c = QueryStatsCollector::new();
        assert_eq!(c.query_count(), 0);
        assert_eq!(c.total_calls(), 0);
        assert_eq!(c.total_time_ms(), 0.0);
        assert_eq!(c.total_rows(), 0);
        assert!(c.to_pg_stat_statements_rows().is_empty());
        assert!(c.top_queries_by_time(10).is_empty());
    }
}
