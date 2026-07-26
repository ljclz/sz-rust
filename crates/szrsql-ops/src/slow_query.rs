//! 慢查询日志与分析（Slow Query Log & Analysis）— Phase 7d.10
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.10 慢查询日志与分析设计。
//!
//! # 设计
//!
//! 借鉴 MySQL slow_query_log + PostgreSQL pg_stat_statements：
//! - **慢查询日志** — 记录执行时间超过阈值的 SQL，包含查询计划、扫描行数、
//!   索引使用情况等。日志支持按时间范围、用户、数据库过滤。
//! - **慢查询分析** — 聚合统计 Top N 慢查询（按次数/总时间/平均时间），
//!   生成索引建议（基于规则：WHERE/JOIN/ORDER BY 列无索引、SeqScan 大表、冗余索引）。
//!
//! ## 验证标准
//!
//! - 配置慢查询阈值 200ms → 执行 1000 条查询（10% 慢查询）→ 慢查询日志自动记录
//! - 分析报告包含 Top 10/执行计划/索引建议

use std::collections::HashMap;

// =====================================================================
//  常量
// =====================================================================

/// 默认慢查询阈值（毫秒） — MySQL 默认 10s，PostgreSQL log_min_duration_statement 默认 0（记录全部）
/// 这里采用业务通用 200ms
pub const DEFAULT_SLOW_QUERY_THRESHOLD_MS: u64 = 200;

/// 默认 Top N 数量
pub const DEFAULT_TOP_N: usize = 10;

/// 默认最大日志条目数（防止内存爆炸）
pub const DEFAULT_MAX_LOG_ENTRIES: usize = 100_000;

/// SQL 归一化后字符串最大长度（截断长 SQL）
pub const MAX_SQL_TEXT_LEN: usize = 200;

// =====================================================================
//  PlanOperator — 查询计划操作符
// =====================================================================

/// 查询计划操作符 — 借鉴 PostgreSQL EXPLAIN 输出
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlanOperator {
    /// 顺序全表扫描（Seq Scan）
    SeqScan,
    /// 索引扫描（Index Scan）
    IndexScan,
    /// 仅索引扫描（Index Only Scan）
    IndexOnlyScan,
    /// Hash JOIN
    HashJoin,
    /// 嵌套循环 JOIN（Nested Loop）
    NestedLoop,
    /// 归并 JOIN（Merge Join）
    MergeJoin,
    /// 排序（Sort）
    Sort,
    /// 聚合（Aggregate / HashAggregate）
    Aggregate,
    /// Limit
    Limit,
}

impl PlanOperator {
    /// 操作符名称
    pub fn as_str(&self) -> &'static str {
        match self {
            PlanOperator::SeqScan => "Seq Scan",
            PlanOperator::IndexScan => "Index Scan",
            PlanOperator::IndexOnlyScan => "Index Only Scan",
            PlanOperator::HashJoin => "Hash Join",
            PlanOperator::NestedLoop => "Nested Loop",
            PlanOperator::MergeJoin => "Merge Join",
            PlanOperator::Sort => "Sort",
            PlanOperator::Aggregate => "Aggregate",
            PlanOperator::Limit => "Limit",
        }
    }

    /// 是否全表扫描
    pub fn is_seq_scan(&self) -> bool {
        matches!(self, PlanOperator::SeqScan)
    }

    /// 是否使用索引
    pub fn is_index_scan(&self) -> bool {
        matches!(self, PlanOperator::IndexScan | PlanOperator::IndexOnlyScan)
    }

    /// 是否 JOIN 操作
    pub fn is_join(&self) -> bool {
        matches!(
            self,
            PlanOperator::HashJoin | PlanOperator::NestedLoop | PlanOperator::MergeJoin
        )
    }
}

impl std::fmt::Display for PlanOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  QueryPlan — 查询计划
// =====================================================================

/// 查询计划 — 简化的 EXPLAIN 输出
#[derive(Debug, Clone, PartialEq)]
pub struct QueryPlan {
    /// 计划文本（EXPLAIN 输出）
    pub plan_text: String,
    /// 根操作符
    pub root_operator: PlanOperator,
    /// 涉及的表
    pub tables: Vec<String>,
    /// 使用的索引
    pub indexes: Vec<String>,
    /// 估算成本
    pub cost: f64,
    /// 估算行数
    pub rows_estimated: u64,
}

impl QueryPlan {
    /// 构造查询计划
    pub fn new(
        plan_text: impl Into<String>,
        root_operator: PlanOperator,
        tables: Vec<String>,
        indexes: Vec<String>,
        cost: f64,
        rows_estimated: u64,
    ) -> Self {
        Self {
            plan_text: plan_text.into(),
            root_operator,
            tables,
            indexes,
            cost,
            rows_estimated,
        }
    }

    /// 是否全表扫描
    pub fn is_seq_scan(&self) -> bool {
        self.root_operator.is_seq_scan()
    }

    /// 是否使用索引
    pub fn uses_index(&self) -> bool {
        !self.indexes.is_empty() || self.root_operator.is_index_scan()
    }

    /// 表数量
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// 索引数量
    pub fn index_count(&self) -> usize {
        self.indexes.len()
    }

    /// 是否 JOIN 查询
    pub fn is_join(&self) -> bool {
        self.root_operator.is_join() || self.tables.len() > 1
    }
}

// =====================================================================
//  IndexReason — 索引建议原因
// =====================================================================

/// 索引建议原因
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IndexReason {
    /// WHERE 条件列无索引
    MissingIndexForWhere,
    /// JOIN 列无索引
    MissingIndexForJoin,
    /// ORDER BY 列无索引
    MissingIndexForOrderBy,
    /// 大表 SeqScan
    SeqScanOnLargeTable,
    /// 冗余索引
    RedundantIndex,
}

impl IndexReason {
    /// 原因描述
    pub fn as_str(&self) -> &'static str {
        match self {
            IndexReason::MissingIndexForWhere => "WHERE 条件列无索引",
            IndexReason::MissingIndexForJoin => "JOIN 列无索引",
            IndexReason::MissingIndexForOrderBy => "ORDER BY 列无索引",
            IndexReason::SeqScanOnLargeTable => "大表全表扫描",
            IndexReason::RedundantIndex => "冗余索引",
        }
    }

    /// 建议操作（创建/删除）
    pub fn is_create(&self) -> bool {
        !matches!(self, IndexReason::RedundantIndex)
    }

    /// 建议操作（删除）
    pub fn is_drop(&self) -> bool {
        matches!(self, IndexReason::RedundantIndex)
    }
}

impl std::fmt::Display for IndexReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
//  IndexAdvice — 索引建议
// =====================================================================

/// 索引建议 — 基于规则分析
#[derive(Debug, Clone, PartialEq)]
pub struct IndexAdvice {
    /// 表名
    pub table: String,
    /// 列名列表
    pub columns: Vec<String>,
    /// 建议原因
    pub reason: IndexReason,
    /// 估算加速比（>= 1.0）
    pub estimated_speedup: f64,
    /// 置信度（0.0 ~ 1.0）
    pub confidence: f64,
}

impl IndexAdvice {
    /// 构造索引建议
    pub fn new(
        table: impl Into<String>,
        columns: Vec<String>,
        reason: IndexReason,
        estimated_speedup: f64,
        confidence: f64,
    ) -> Self {
        Self {
            table: table.into(),
            columns,
            reason,
            estimated_speedup: estimated_speedup.max(1.0),
            confidence: confidence.clamp(0.0, 1.0),
        }
    }

    /// 生成 CREATE INDEX 语句
    pub fn create_index_sql(&self) -> String {
        if self.reason.is_drop() {
            return format!("-- 冗余索引，建议删除：{}", self.table);
        }
        let col_list = self.columns.join(", ");
        let index_name = format!("idx_{}_{}", self.table, self.columns.join("_"));
        format!(
            "CREATE INDEX {} ON {} ({})  -- {} 估算加速 {:.1}x 置信度 {:.0}%",
            index_name,
            self.table,
            col_list,
            self.reason,
            self.estimated_speedup,
            self.confidence * 100.0
        )
    }

    /// 是否高置信度（>= 0.8）
    pub fn is_high_confidence(&self) -> bool {
        self.confidence >= 0.8
    }
}

// =====================================================================
//  SlowQueryEntry — 慢查询日志条目
// =====================================================================

/// 慢查询日志条目
#[derive(Debug, Clone, PartialEq)]
pub struct SlowQueryEntry {
    /// 查询 ID（递增）
    pub query_id: u64,
    /// SQL 原文（截断到 200 字符）
    pub sql_text: String,
    /// SQL 归一化文本（参数替换为 ?，用于聚合）
    pub sql_normalized: String,
    /// 执行时长（毫秒）
    pub duration_ms: u64,
    /// 时间戳（秒）
    pub timestamp: u64,
    /// 用户名
    pub user: String,
    /// 数据库名
    pub database: String,
    /// 返回行数
    pub rows_returned: u64,
    /// 扫描字节数
    pub bytes_scanned: u64,
    /// 使用的索引（None 表示全表扫描）
    pub index_used: Option<String>,
    /// 涉及的表
    pub tables_accessed: Vec<String>,
    /// 查询计划
    pub plan: Option<QueryPlan>,
}

impl SlowQueryEntry {
    /// 构造慢查询条目（自动归一化 SQL）
    pub fn new(
        query_id: u64,
        sql_text: impl Into<String>,
        duration_ms: u64,
        timestamp: u64,
        user: impl Into<String>,
        database: impl Into<String>,
    ) -> Self {
        let sql_text_raw = sql_text.into();
        let sql_text = if sql_text_raw.len() > MAX_SQL_TEXT_LEN {
            sql_text_raw.chars().take(MAX_SQL_TEXT_LEN).collect()
        } else {
            sql_text_raw
        };
        let sql_normalized = normalize_sql(&sql_text);
        Self {
            query_id,
            sql_text,
            sql_normalized,
            duration_ms,
            timestamp,
            user: user.into(),
            database: database.into(),
            rows_returned: 0,
            bytes_scanned: 0,
            index_used: None,
            tables_accessed: Vec::new(),
            plan: None,
        }
    }

    /// 设置返回行数
    pub fn with_rows_returned(mut self, rows: u64) -> Self {
        self.rows_returned = rows;
        self
    }

    /// 设置扫描字节数
    pub fn with_bytes_scanned(mut self, bytes: u64) -> Self {
        self.bytes_scanned = bytes;
        self
    }

    /// 设置使用的索引
    pub fn with_index(mut self, index: impl Into<String>) -> Self {
        self.index_used = Some(index.into());
        self
    }

    /// 设置涉及的表
    pub fn with_tables(mut self, tables: Vec<String>) -> Self {
        self.tables_accessed = tables;
        self
    }

    /// 设置查询计划
    pub fn with_plan(mut self, plan: QueryPlan) -> Self {
        self.plan = Some(plan);
        self
    }

    /// 是否全表扫描
    pub fn is_seq_scan(&self) -> bool {
        self.plan.as_ref().is_some_and(|p| p.is_seq_scan())
    }

    /// 是否使用索引
    pub fn uses_index(&self) -> bool {
        self.index_used.is_some() || self.plan.as_ref().is_some_and(|p| p.uses_index())
    }

    /// 执行时长（秒）
    pub fn duration_secs(&self) -> f64 {
        self.duration_ms as f64 / 1000.0
    }
}

// =====================================================================
//  SlowQueryConfig — 慢查询配置
// =====================================================================

/// 慢查询配置
#[derive(Debug, Clone, PartialEq)]
pub struct SlowQueryConfig {
    /// 慢查询阈值（毫秒）
    pub threshold_ms: u64,
    /// 最大日志条目数
    pub max_log_entries: usize,
    /// Top N 数量
    pub top_n: usize,
}

impl Default for SlowQueryConfig {
    fn default() -> Self {
        Self {
            threshold_ms: DEFAULT_SLOW_QUERY_THRESHOLD_MS,
            max_log_entries: DEFAULT_MAX_LOG_ENTRIES,
            top_n: DEFAULT_TOP_N,
        }
    }
}

impl SlowQueryConfig {
    /// 构造默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 自定义阈值
    pub fn with_threshold_ms(mut self, threshold_ms: u64) -> Self {
        self.threshold_ms = threshold_ms;
        self
    }

    /// 自定义最大日志条目数
    pub fn with_max_log_entries(mut self, max: usize) -> Self {
        self.max_log_entries = max;
        self
    }

    /// 自定义 Top N
    pub fn with_top_n(mut self, top_n: usize) -> Self {
        self.top_n = top_n;
        self
    }
}

// =====================================================================
//  SlowQueryLogger — 慢查询日志器
// =====================================================================

/// 慢查询日志器 — 收集执行时间超过阈值的查询
pub struct SlowQueryLogger {
    /// 日志条目（环形，超限时丢弃最旧的）
    entries: Vec<SlowQueryEntry>,
    /// 配置
    config: SlowQueryConfig,
    /// 总查询数（含未记录的快速查询）
    total_queries: u64,
    /// 已记录的慢查询数
    total_logged: u64,
    /// 已过滤（未记录）的快速查询数
    total_filtered: u64,
    /// 丢弃的条目数（环形缓冲区满时）
    dropped_entries: u64,
}

impl Default for SlowQueryLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl SlowQueryLogger {
    /// 构造默认慢查询日志器
    pub fn new() -> Self {
        Self::with_config(SlowQueryConfig::default())
    }

    /// 构造自定义配置的慢查询日志器
    pub fn with_config(config: SlowQueryConfig) -> Self {
        Self {
            entries: Vec::with_capacity(config.max_log_entries.min(10_000)),
            config,
            total_queries: 0,
            total_logged: 0,
            total_filtered: 0,
            dropped_entries: 0,
        }
    }

    /// 获取阈值（毫秒）
    pub fn threshold_ms(&self) -> u64 {
        self.config.threshold_ms
    }

    /// 获取 Top N
    pub fn top_n(&self) -> usize {
        self.config.top_n
    }

    /// 当前日志条目数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 日志是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 总查询数
    pub fn total_queries(&self) -> u64 {
        self.total_queries
    }

    /// 已记录的慢查询数
    pub fn total_logged(&self) -> u64 {
        self.total_logged
    }

    /// 已过滤（未记录）的快速查询数
    pub fn total_filtered(&self) -> u64 {
        self.total_filtered
    }

    /// 丢弃的条目数
    pub fn dropped_entries(&self) -> u64 {
        self.dropped_entries
    }

    /// 慢查询比例（0.0 ~ 1.0）
    pub fn slow_ratio(&self) -> f64 {
        if self.total_queries == 0 {
            return 0.0;
        }
        self.total_logged as f64 / self.total_queries as f64
    }

    /// 记录一个查询条目
    ///
    /// 如果执行时长 >= 阈值，记录到日志并返回 true；
    /// 否则仅累加统计并返回 false。
    /// 环形缓冲区满时丢弃最旧的条目。
    pub fn log(&mut self, entry: SlowQueryEntry) -> bool {
        self.total_queries += 1;
        if entry.duration_ms < self.config.threshold_ms {
            self.total_filtered += 1;
            return false;
        }
        if self.entries.len() >= self.config.max_log_entries {
            self.entries.remove(0);
            self.dropped_entries += 1;
        }
        self.total_logged += 1;
        self.entries.push(entry);
        true
    }

    /// 获取所有日志条目
    pub fn entries(&self) -> &[SlowQueryEntry] {
        &self.entries
    }

    /// 按时间范围过滤（秒级时间戳）
    pub fn filter_by_duration(&self, start: u64, end: u64) -> Vec<&SlowQueryEntry> {
        self.entries
            .iter()
            .filter(|e| e.timestamp >= start && e.timestamp <= end)
            .collect()
    }

    /// 按用户过滤
    pub fn filter_by_user(&self, user: &str) -> Vec<&SlowQueryEntry> {
        self.entries.iter().filter(|e| e.user == user).collect()
    }

    /// 按数据库过滤
    pub fn filter_by_database(&self, database: &str) -> Vec<&SlowQueryEntry> {
        self.entries
            .iter()
            .filter(|e| e.database == database)
            .collect()
    }

    /// 按表过滤
    pub fn filter_by_table(&self, table: &str) -> Vec<&SlowQueryEntry> {
        self.entries
            .iter()
            .filter(|e| e.tables_accessed.iter().any(|t| t == table))
            .collect()
    }

    /// 清空日志
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// 重置统计
    pub fn reset_stats(&mut self) {
        self.total_queries = 0;
        self.total_logged = 0;
        self.total_filtered = 0;
        self.dropped_entries = 0;
    }
}

// =====================================================================
//  SqlStatEntry — SQL 聚合统计
// =====================================================================

/// SQL 聚合统计（按归一化 SQL 聚合）
#[derive(Debug, Clone, PartialEq)]
pub struct SqlStatEntry {
    /// 归一化 SQL
    pub sql_normalized: String,
    /// 出现次数
    pub count: usize,
    /// 总执行时长（毫秒）
    pub total_ms: u64,
    /// 平均执行时长（毫秒）
    pub avg_ms: f64,
    /// 最大执行时长（毫秒）
    pub max_ms: u64,
    /// 最小执行时长（毫秒）
    pub min_ms: u64,
    /// 总扫描字节数
    pub total_bytes_scanned: u64,
    /// 总返回行数
    pub total_rows_returned: u64,
    /// 涉及的表（去重）
    pub tables: Vec<String>,
    /// 全表扫描次数
    pub seq_scan_count: usize,
}

impl SqlStatEntry {
    /// 构造统计条目
    pub fn new(sql_normalized: impl Into<String>) -> Self {
        Self {
            sql_normalized: sql_normalized.into(),
            count: 0,
            total_ms: 0,
            avg_ms: 0.0,
            max_ms: 0,
            min_ms: u64::MAX,
            total_bytes_scanned: 0,
            total_rows_returned: 0,
            tables: Vec::new(),
            seq_scan_count: 0,
        }
    }

    /// 累加一个慢查询条目
    pub fn accumulate(&mut self, entry: &SlowQueryEntry) {
        self.count += 1;
        self.total_ms += entry.duration_ms;
        if entry.duration_ms > self.max_ms {
            self.max_ms = entry.duration_ms;
        }
        if entry.duration_ms < self.min_ms {
            self.min_ms = entry.duration_ms;
        }
        self.avg_ms = self.total_ms as f64 / self.count as f64;
        self.total_bytes_scanned += entry.bytes_scanned;
        self.total_rows_returned += entry.rows_returned;
        for t in &entry.tables_accessed {
            if !self.tables.contains(t) {
                self.tables.push(t.clone());
            }
        }
        if entry.is_seq_scan() {
            self.seq_scan_count += 1;
        }
    }

    /// 是否所有执行都是全表扫描
    pub fn is_all_seq_scan(&self) -> bool {
        self.count > 0 && self.seq_scan_count == self.count
    }

    /// 总执行时长（秒）
    pub fn total_secs(&self) -> f64 {
        self.total_ms as f64 / 1000.0
    }
}

// =====================================================================
//  SlowQueryAnalysisReport — 慢查询分析报告
// =====================================================================

/// 慢查询分析报告
#[derive(Debug, Clone, PartialEq)]
pub struct SlowQueryAnalysisReport {
    /// 慢查询总数
    pub total_slow_queries: usize,
    /// 总执行时长（毫秒）
    pub total_duration_ms: u64,
    /// 平均执行时长（毫秒）
    pub avg_duration_ms: f64,
    /// 最大执行时长（毫秒）
    pub max_duration_ms: u64,
    /// 最小执行时长（毫秒）
    pub min_duration_ms: u64,
    /// 总扫描字节数
    pub total_bytes_scanned: u64,
    /// 总返回行数
    pub total_rows_returned: u64,
    /// 全表扫描次数
    pub seq_scan_count: usize,
    /// Top N by 次数（sql, count, total_ms）
    pub top_by_count: Vec<SqlStatEntry>,
    /// Top N by 总时间（sql, total_ms, count）
    pub top_by_total_time: Vec<SqlStatEntry>,
    /// Top N by 平均时间（sql, avg_ms, count）
    pub top_by_avg_time: Vec<SqlStatEntry>,
    /// 索引建议列表
    pub index_advice: Vec<IndexAdvice>,
}

impl SlowQueryAnalysisReport {
    /// 构造空报告
    pub fn empty() -> Self {
        Self {
            total_slow_queries: 0,
            total_duration_ms: 0,
            avg_duration_ms: 0.0,
            max_duration_ms: 0,
            min_duration_ms: 0,
            total_bytes_scanned: 0,
            total_rows_returned: 0,
            seq_scan_count: 0,
            top_by_count: Vec::new(),
            top_by_total_time: Vec::new(),
            top_by_avg_time: Vec::new(),
            index_advice: Vec::new(),
        }
    }

    /// 慢查询总数
    pub fn total_count(&self) -> usize {
        self.total_slow_queries
    }

    /// 总执行时长（秒）
    pub fn total_duration_secs(&self) -> f64 {
        self.total_duration_ms as f64 / 1000.0
    }

    /// 全表扫描比例
    pub fn seq_scan_ratio(&self) -> f64 {
        if self.total_slow_queries == 0 {
            return 0.0;
        }
        self.seq_scan_count as f64 / self.total_slow_queries as f64
    }

    /// 渲染文本报告
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("========== Slow Query Analysis Report ==========\n\n");

        // 概览
        out.push_str("Overview\n");
        out.push_str(&format!(
            "  Total slow queries: {}\n",
            self.total_slow_queries
        ));
        out.push_str(&format!(
            "  Total duration: {:.3}s ({}ms)\n",
            self.total_duration_secs(),
            self.total_duration_ms
        ));
        out.push_str(&format!("  Avg duration: {:.3}ms\n", self.avg_duration_ms));
        out.push_str(&format!("  Max duration: {}ms\n", self.max_duration_ms));
        out.push_str(&format!("  Min duration: {}ms\n", self.min_duration_ms));
        out.push_str(&format!(
            "  Total bytes scanned: {}\n",
            self.total_bytes_scanned
        ));
        out.push_str(&format!(
            "  Total rows returned: {}\n",
            self.total_rows_returned
        ));
        out.push_str(&format!(
            "  Seq scan count: {} ({:.1}%)\n",
            self.seq_scan_count,
            self.seq_scan_ratio() * 100.0
        ));
        out.push('\n');

        // Top by count
        out.push_str(&format!("Top {} by Count\n", self.top_by_count.len()));
        for (i, stat) in self.top_by_count.iter().enumerate() {
            out.push_str(&format!(
                "  {}. [{}x] {:.3}s total, {:.1}ms avg | {}\n",
                i + 1,
                stat.count,
                stat.total_secs(),
                stat.avg_ms,
                truncate_sql(&stat.sql_normalized, 80)
            ));
        }
        out.push('\n');

        // Top by total time
        out.push_str(&format!(
            "Top {} by Total Time\n",
            self.top_by_total_time.len()
        ));
        for (i, stat) in self.top_by_total_time.iter().enumerate() {
            out.push_str(&format!(
                "  {}. [{:.3}s total, {}x] {:.1}ms avg | {}\n",
                i + 1,
                stat.total_secs(),
                stat.count,
                stat.avg_ms,
                truncate_sql(&stat.sql_normalized, 80)
            ));
        }
        out.push('\n');

        // Top by avg time
        out.push_str(&format!("Top {} by Avg Time\n", self.top_by_avg_time.len()));
        for (i, stat) in self.top_by_avg_time.iter().enumerate() {
            out.push_str(&format!(
                "  {}. [{:.1}ms avg, {}x] {:.3}s total | {}\n",
                i + 1,
                stat.avg_ms,
                stat.count,
                stat.total_secs(),
                truncate_sql(&stat.sql_normalized, 80)
            ));
        }
        out.push('\n');

        // Index advice
        out.push_str(&format!(
            "Index Advice ({} suggestions)\n",
            self.index_advice.len()
        ));
        for (i, advice) in self.index_advice.iter().enumerate() {
            out.push_str(&format!(
                "  {}. [{}] {}\n",
                i + 1,
                advice.reason,
                advice.create_index_sql()
            ));
        }

        out
    }
}

// =====================================================================
//  SlowQueryAnalyzer — 慢查询分析器
// =====================================================================

/// 慢查询分析器 — 聚合统计 + 索引建议
pub struct SlowQueryAnalyzer {
    /// Top N 数量
    top_n: usize,
    /// 大表阈值（行数）— 超过此值的表 SeqScan 触发索引建议
    large_table_threshold: u64,
}

impl Default for SlowQueryAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl SlowQueryAnalyzer {
    /// 构造默认分析器
    pub fn new() -> Self {
        Self {
            top_n: DEFAULT_TOP_N,
            large_table_threshold: 10_000,
        }
    }

    /// 自定义 Top N
    pub fn with_top_n(mut self, top_n: usize) -> Self {
        self.top_n = top_n;
        self
    }

    /// 自定义大表阈值
    pub fn with_large_table_threshold(mut self, threshold: u64) -> Self {
        self.large_table_threshold = threshold;
        self
    }

    /// 分析慢查询日志，生成报告
    pub fn analyze(&self, entries: &[SlowQueryEntry]) -> SlowQueryAnalysisReport {
        if entries.is_empty() {
            return SlowQueryAnalysisReport::empty();
        }

        // 聚合统计
        let mut stats_map: HashMap<String, SqlStatEntry> = HashMap::new();
        let mut total_duration_ms: u64 = 0;
        let mut max_duration_ms: u64 = 0;
        let mut min_duration_ms: u64 = u64::MAX;
        let mut total_bytes_scanned: u64 = 0;
        let mut total_rows_returned: u64 = 0;
        let mut seq_scan_count: usize = 0;

        for entry in entries {
            let stat = stats_map
                .entry(entry.sql_normalized.clone())
                .or_insert_with(|| SqlStatEntry::new(entry.sql_normalized.clone()));
            stat.accumulate(entry);

            total_duration_ms += entry.duration_ms;
            if entry.duration_ms > max_duration_ms {
                max_duration_ms = entry.duration_ms;
            }
            if entry.duration_ms < min_duration_ms {
                min_duration_ms = entry.duration_ms;
            }
            total_bytes_scanned += entry.bytes_scanned;
            total_rows_returned += entry.rows_returned;
            if entry.is_seq_scan() {
                seq_scan_count += 1;
            }
        }

        let total_slow_queries = entries.len();
        let avg_duration_ms = total_duration_ms as f64 / total_slow_queries as f64;

        // 收集所有统计并排序
        let stats: Vec<SqlStatEntry> = stats_map.into_values().collect();

        // Top by count
        let mut top_by_count = stats.clone();
        top_by_count.sort_by_key(|b| std::cmp::Reverse(b.count));
        top_by_count.truncate(self.top_n);

        // Top by total time
        let mut top_by_total_time = stats.clone();
        top_by_total_time.sort_by_key(|b| std::cmp::Reverse(b.total_ms));
        top_by_total_time.truncate(self.top_n);

        // Top by avg time
        let mut top_by_avg_time = stats.clone();
        top_by_avg_time.sort_by(|a, b| {
            b.avg_ms
                .partial_cmp(&a.avg_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        top_by_avg_time.truncate(self.top_n);

        // 生成索引建议
        let mut index_advice = Vec::new();
        let mut seen_advice: HashMap<String, IndexReason> = HashMap::new();
        for entry in entries {
            for advice in self.suggest_indexes(entry) {
                let key = format!("{}|{}", advice.table, advice.columns.join(","));
                if let std::collections::hash_map::Entry::Vacant(e) = seen_advice.entry(key) {
                    e.insert(advice.reason);
                    index_advice.push(advice);
                }
            }
        }

        SlowQueryAnalysisReport {
            total_slow_queries,
            total_duration_ms,
            avg_duration_ms,
            max_duration_ms,
            min_duration_ms,
            total_bytes_scanned,
            total_rows_returned,
            seq_scan_count,
            top_by_count,
            top_by_total_time,
            top_by_avg_time,
            index_advice,
        }
    }

    /// 为单个慢查询生成索引建议
    ///
    /// 规则：
    /// 1. SeqScan on large table → 建议创建索引
    /// 2. WHERE 列无索引 → 建议创建索引
    /// 3. JOIN 列无索引 → 建议创建索引
    /// 4. ORDER BY 列无索引 → 建议创建索引
    pub fn suggest_indexes(&self, entry: &SlowQueryEntry) -> Vec<IndexAdvice> {
        let mut advices = Vec::new();

        let plan = match &entry.plan {
            Some(p) => p,
            None => return advices,
        };

        // 规则1：大表 SeqScan
        if plan.is_seq_scan() && plan.rows_estimated >= self.large_table_threshold {
            for table in &plan.tables {
                if let Some(where_cols) = extract_where_columns(&entry.sql_text, table) {
                    if !where_cols.is_empty() {
                        advices.push(IndexAdvice::new(
                            table.clone(),
                            where_cols,
                            IndexReason::SeqScanOnLargeTable,
                            5.0,
                            0.9,
                        ));
                    } else {
                        advices.push(IndexAdvice::new(
                            table.clone(),
                            vec!["id".to_string()],
                            IndexReason::SeqScanOnLargeTable,
                            3.0,
                            0.7,
                        ));
                    }
                } else {
                    advices.push(IndexAdvice::new(
                        table.clone(),
                        vec!["id".to_string()],
                        IndexReason::SeqScanOnLargeTable,
                        3.0,
                        0.7,
                    ));
                }
            }
        }

        // 规则2：WHERE 列无索引
        for table in &plan.tables {
            if plan.indexes.iter().any(|idx| idx.starts_with(table)) {
                continue; // 已有索引
            }
            if let Some(where_cols) = extract_where_columns(&entry.sql_text, table) {
                for col in &where_cols {
                    if !plan.indexes.iter().any(|idx| idx.contains(col)) {
                        let mut cols = vec![col.clone()];
                        // 多列 WHERE 时建议复合索引
                        if where_cols.len() > 1 {
                            cols = where_cols.clone();
                        }
                        advices.push(IndexAdvice::new(
                            table.clone(),
                            cols,
                            IndexReason::MissingIndexForWhere,
                            4.0,
                            0.85,
                        ));
                        break;
                    }
                }
            }
        }

        // 规则3：JOIN 列无索引
        if plan.is_join() && plan.tables.len() >= 2 {
            if let Some(join_cols) = extract_join_columns(&entry.sql_text) {
                for (table, col) in &join_cols {
                    if !plan.indexes.iter().any(|idx| idx.contains(col)) {
                        advices.push(IndexAdvice::new(
                            table.clone(),
                            vec![col.clone()],
                            IndexReason::MissingIndexForJoin,
                            6.0,
                            0.9,
                        ));
                    }
                }
            }
        }

        // 规则4：ORDER BY 列无索引
        if let Some(order_cols) = extract_order_by_columns(&entry.sql_text) {
            for table in &plan.tables {
                if !plan
                    .indexes
                    .iter()
                    .any(|idx| order_cols.iter().any(|col| idx.contains(col)))
                {
                    advices.push(IndexAdvice::new(
                        table.clone(),
                        order_cols.clone(),
                        IndexReason::MissingIndexForOrderBy,
                        2.5,
                        0.75,
                    ));
                    break;
                }
            }
        }

        advices
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// SQL 归一化 — 将数字、字符串字面量替换为 ?
///
/// 例如：`SELECT * FROM t WHERE id = 123 AND name = 'abc'` →
///      `SELECT * FROM t WHERE id = ? AND name = ?`
pub fn normalize_sql(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len());
    let chars: Vec<char> = sql.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // 字符串字面量 'xxx'
        if c == '\'' {
            result.push('?');
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    // 检查是否转义 ''
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
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
        // 数字字面量
        if c.is_ascii_digit() {
            result.push('?');
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            continue;
        }
        // 双引号标识符 "xxx"（保留）
        if c == '"' {
            result.push(c);
            i += 1;
            while i < chars.len() {
                result.push(chars[i]);
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        result.push(c);
        i += 1;
    }
    // 压缩连续空格
    let mut collapsed = String::with_capacity(result.len());
    let mut prev_space = false;
    for c in result.chars() {
        if c.is_whitespace() {
            if !prev_space {
                collapsed.push(' ');
                prev_space = true;
            }
        } else {
            collapsed.push(c);
            prev_space = false;
        }
    }
    collapsed.trim().to_string()
}

/// 从 SQL 中提取 WHERE 子句的列名（针对指定表）
///
/// 简化实现：匹配 `WHERE col = ?` 或 `WHERE col > ?` 等模式
pub fn extract_where_columns(sql: &str, _table: &str) -> Option<Vec<String>> {
    let upper = sql.to_uppercase();
    let where_pos = upper.find("WHERE")?;
    let after_where = &sql[where_pos + 5..];
    // 截断到 GROUP BY / ORDER BY / LIMIT 之前
    let end_pos = ["GROUP BY", "ORDER BY", "LIMIT", "HAVING"]
        .iter()
        .filter_map(|kw| after_where.to_uppercase().find(kw))
        .min()
        .unwrap_or(after_where.len());
    let where_clause = &after_where[..end_pos];

    let mut cols = Vec::new();
    // 匹配模式：col OP value，OP ∈ {=, >, <, >=, <=, <>, !=, LIKE, IN, BETWEEN}
    let patterns = [
        "=",
        ">",
        "<",
        ">=",
        "<=",
        "<>",
        "!=",
        " LIKE ",
        " IN ",
        " BETWEEN ",
    ];
    let tokens: Vec<&str> = where_clause.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i].trim_matches(|c: char| c == '(' || c == ')' || c == ',');
        // 检查下一个 token 是否是操作符
        if i + 1 < tokens.len() {
            let next = tokens[i + 1].to_uppercase();
            let is_op = patterns
                .iter()
                .any(|p| next == p.trim() || next.starts_with(p.trim()));
            if is_op && !tok.is_empty() {
                // 去掉表名前缀 t.col → col
                let col = if let Some(dot_pos) = tok.rfind('.') {
                    &tok[dot_pos + 1..]
                } else {
                    tok
                };
                // 跳过 SQL 关键字
                let upper_col = col.to_uppercase();
                if !matches!(upper_col.as_str(), "AND" | "OR" | "NOT" | "NULL" | "WHERE") {
                    let clean =
                        col.trim_matches(|c: char| c == '(' || c == ')' || c == ',' || c == '\'');
                    if !clean.is_empty() && !cols.contains(&clean.to_string()) {
                        cols.push(clean.to_string());
                    }
                }
            }
        }
        i += 1;
    }
    if cols.is_empty() {
        None
    } else {
        Some(cols)
    }
}

/// 从 SQL 中提取 JOIN 列（表名，列名）
///
/// 简化实现：匹配 `JOIN t2 ON t1.id = t2.id`
pub fn extract_join_columns(sql: &str) -> Option<Vec<(String, String)>> {
    let upper = sql.to_uppercase();
    let join_pos = upper.find(" JOIN ")?;
    let after_join = &sql[join_pos + 6..];
    let on_pos = after_join.to_uppercase().find(" ON ")?;
    let after_on = &after_join[on_pos + 4..];

    // 截断到 WHERE / GROUP BY / ORDER BY / LIMIT 之前
    let end_pos = ["WHERE", "GROUP BY", "ORDER BY", "LIMIT"]
        .iter()
        .filter_map(|kw| after_on.to_uppercase().find(kw))
        .min()
        .unwrap_or(after_on.len());
    let on_clause = &after_on[..end_pos];

    let mut result = Vec::new();
    // 匹配 t1.col = t2.col
    for part in on_clause.split("AND") {
        let part = part.trim();
        if let Some(eq_pos) = part.find('=') {
            let left = part[..eq_pos].trim();
            let right = part[eq_pos + 1..].trim();
            for side in [left, right] {
                if let Some(dot_pos) = side.rfind('.') {
                    let table =
                        side[..dot_pos].trim_matches(|c: char| c == '(' || c == ')' || c == ' ');
                    let col = &side[dot_pos + 1..];
                    if !table.is_empty() && !col.is_empty() {
                        result.push((table.to_string(), col.to_string()));
                    }
                }
            }
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// 从 SQL 中提取 ORDER BY 列
pub fn extract_order_by_columns(sql: &str) -> Option<Vec<String>> {
    let upper = sql.to_uppercase();
    let order_pos = upper.find("ORDER BY")?;
    let after_order = &sql[order_pos + 8..];
    // 截断到 LIMIT 之前
    let end_pos = after_order
        .to_uppercase()
        .find("LIMIT")
        .unwrap_or(after_order.len());
    let order_clause = &after_order[..end_pos];
    let mut cols = Vec::new();
    for part in order_clause.split(',') {
        let part = part.trim();
        // 去掉 ASC/DESC
        let col = part
            .to_uppercase()
            .replace(" ASC", "")
            .replace(" DESC", "")
            .trim()
            .to_lowercase();
        if !col.is_empty() {
            // 去掉表名前缀
            let col = if let Some(dot_pos) = col.rfind('.') {
                col[dot_pos + 1..].to_string()
            } else {
                col
            };
            cols.push(col);
        }
    }
    if cols.is_empty() {
        None
    } else {
        Some(cols)
    }
}

/// 截断 SQL 到指定长度（保留可读性）
fn truncate_sql(sql: &str, max_len: usize) -> String {
    if sql.len() <= max_len {
        sql.to_string()
    } else {
        format!("{}...", &sql[..max_len])
    }
}

/// 生成混合查询负载（用于测试）
///
/// - `count`：查询总数
/// - `slow_ratio`：慢查询比例（0.0 ~ 1.0）
pub fn generate_mixed_queries(count: usize, slow_ratio: f64) -> Vec<SlowQueryEntry> {
    let mut queries = Vec::with_capacity(count);
    let slow_count = (count as f64 * slow_ratio) as usize;
    let templates = [
        "SELECT * FROM users WHERE id = ?",
        "SELECT * FROM orders WHERE user_id = ? AND status = ?",
        "SELECT * FROM products WHERE category = ? AND price > ?",
        "SELECT u.name, o.total FROM users u JOIN orders o ON u.id = o.user_id WHERE o.created_at > ?",
        "SELECT * FROM large_table WHERE col1 = ? AND col2 = ? AND col3 > ? ORDER BY col1",
    ];
    for i in 0..count {
        let template = templates[i % templates.len()];
        let is_slow = i < slow_count;
        let duration_ms = if is_slow {
            200 + (i as u64 % 800) // 200~1000ms
        } else {
            i as u64 % 200 // 0~199ms
        };
        let entry = SlowQueryEntry::new(
            i as u64,
            template.replace('?', &format!("{}", i)),
            duration_ms,
            i as u64,
            "test_user",
            "test_db",
        );
        queries.push(entry);
    }
    queries
}

// =====================================================================
//  测试模块
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  PlanOperator 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_plan_operator_as_str() {
        assert_eq!(PlanOperator::SeqScan.as_str(), "Seq Scan");
        assert_eq!(PlanOperator::IndexScan.as_str(), "Index Scan");
        assert_eq!(PlanOperator::IndexOnlyScan.as_str(), "Index Only Scan");
        assert_eq!(PlanOperator::HashJoin.as_str(), "Hash Join");
        assert_eq!(PlanOperator::NestedLoop.as_str(), "Nested Loop");
        assert_eq!(PlanOperator::MergeJoin.as_str(), "Merge Join");
        assert_eq!(PlanOperator::Sort.as_str(), "Sort");
        assert_eq!(PlanOperator::Aggregate.as_str(), "Aggregate");
        assert_eq!(PlanOperator::Limit.as_str(), "Limit");
    }

    #[test]
    fn test_plan_operator_predicates() {
        assert!(PlanOperator::SeqScan.is_seq_scan());
        assert!(!PlanOperator::IndexScan.is_seq_scan());
        assert!(PlanOperator::IndexScan.is_index_scan());
        assert!(PlanOperator::IndexOnlyScan.is_index_scan());
        assert!(!PlanOperator::SeqScan.is_index_scan());
        assert!(PlanOperator::HashJoin.is_join());
        assert!(PlanOperator::NestedLoop.is_join());
        assert!(PlanOperator::MergeJoin.is_join());
        assert!(!PlanOperator::Sort.is_join());
    }

    #[test]
    fn test_plan_operator_display() {
        assert_eq!(format!("{}", PlanOperator::SeqScan), "Seq Scan");
        assert_eq!(format!("{}", PlanOperator::HashJoin), "Hash Join");
    }

    // -----------------------------------------------------------------
    //  QueryPlan 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_query_plan_new() {
        let plan = QueryPlan::new(
            "Seq Scan on users",
            PlanOperator::SeqScan,
            vec!["users".to_string()],
            vec![],
            100.0,
            50000,
        );
        assert_eq!(plan.plan_text, "Seq Scan on users");
        assert_eq!(plan.root_operator, PlanOperator::SeqScan);
        assert_eq!(plan.tables, vec!["users"]);
        assert!(plan.indexes.is_empty());
        assert_eq!(plan.cost, 100.0);
        assert_eq!(plan.rows_estimated, 50000);
    }

    #[test]
    fn test_query_plan_is_seq_scan() {
        let seq = QueryPlan::new(
            "Seq Scan on users",
            PlanOperator::SeqScan,
            vec!["users".to_string()],
            vec![],
            100.0,
            50000,
        );
        assert!(seq.is_seq_scan());
        assert!(!seq.uses_index());

        let idx = QueryPlan::new(
            "Index Scan using idx_users_id on users",
            PlanOperator::IndexScan,
            vec!["users".to_string()],
            vec!["idx_users_id".to_string()],
            5.0,
            1,
        );
        assert!(!idx.is_seq_scan());
        assert!(idx.uses_index());
    }

    #[test]
    fn test_query_plan_uses_index() {
        let with_idx = QueryPlan::new(
            "Index Scan",
            PlanOperator::IndexScan,
            vec!["t".to_string()],
            vec!["idx_t".to_string()],
            5.0,
            10,
        );
        assert!(with_idx.uses_index());

        let without_idx = QueryPlan::new(
            "Seq Scan",
            PlanOperator::SeqScan,
            vec!["t".to_string()],
            vec![],
            100.0,
            10000,
        );
        assert!(!without_idx.uses_index());
    }

    #[test]
    fn test_query_plan_table_count() {
        let single = QueryPlan::new(
            "Seq Scan on t",
            PlanOperator::SeqScan,
            vec!["t".to_string()],
            vec![],
            10.0,
            100,
        );
        assert_eq!(single.table_count(), 1);

        let join = QueryPlan::new(
            "Hash Join",
            PlanOperator::HashJoin,
            vec!["t1".to_string(), "t2".to_string()],
            vec![],
            50.0,
            1000,
        );
        assert_eq!(join.table_count(), 2);
        assert!(join.is_join());
    }

    #[test]
    fn test_query_plan_index_count() {
        let plan = QueryPlan::new(
            "Index Scan",
            PlanOperator::IndexScan,
            vec!["t".to_string()],
            vec!["idx1".to_string(), "idx2".to_string()],
            5.0,
            10,
        );
        assert_eq!(plan.index_count(), 2);
    }

    #[test]
    fn test_query_plan_is_join() {
        let join = QueryPlan::new(
            "Hash Join",
            PlanOperator::HashJoin,
            vec!["t1".to_string(), "t2".to_string()],
            vec![],
            50.0,
            1000,
        );
        assert!(join.is_join());

        let single = QueryPlan::new(
            "Seq Scan on t",
            PlanOperator::SeqScan,
            vec!["t".to_string()],
            vec![],
            10.0,
            100,
        );
        assert!(!single.is_join());
    }

    // -----------------------------------------------------------------
    //  IndexReason 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_index_reason_as_str() {
        assert_eq!(
            IndexReason::MissingIndexForWhere.as_str(),
            "WHERE 条件列无索引"
        );
        assert_eq!(IndexReason::MissingIndexForJoin.as_str(), "JOIN 列无索引");
        assert_eq!(
            IndexReason::MissingIndexForOrderBy.as_str(),
            "ORDER BY 列无索引"
        );
        assert_eq!(IndexReason::SeqScanOnLargeTable.as_str(), "大表全表扫描");
        assert_eq!(IndexReason::RedundantIndex.as_str(), "冗余索引");
    }

    #[test]
    fn test_index_reason_is_create_drop() {
        assert!(IndexReason::MissingIndexForWhere.is_create());
        assert!(IndexReason::MissingIndexForJoin.is_create());
        assert!(IndexReason::MissingIndexForOrderBy.is_create());
        assert!(IndexReason::SeqScanOnLargeTable.is_create());
        assert!(!IndexReason::RedundantIndex.is_create());

        assert!(!IndexReason::MissingIndexForWhere.is_drop());
        assert!(IndexReason::RedundantIndex.is_drop());
    }

    #[test]
    fn test_index_reason_display() {
        assert_eq!(
            format!("{}", IndexReason::MissingIndexForWhere),
            "WHERE 条件列无索引"
        );
    }

    // -----------------------------------------------------------------
    //  IndexAdvice 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_index_advice_new() {
        let advice = IndexAdvice::new(
            "users",
            vec!["email".to_string()],
            IndexReason::MissingIndexForWhere,
            4.0,
            0.85,
        );
        assert_eq!(advice.table, "users");
        assert_eq!(advice.columns, vec!["email"]);
        assert_eq!(advice.reason, IndexReason::MissingIndexForWhere);
        assert_eq!(advice.estimated_speedup, 4.0);
        assert_eq!(advice.confidence, 0.85);
    }

    #[test]
    fn test_index_advice_clamps_values() {
        let advice = IndexAdvice::new(
            "t",
            vec!["c".to_string()],
            IndexReason::MissingIndexForWhere,
            0.5, // < 1.0 应被钳制到 1.0
            1.5, // > 1.0 应被钳制到 1.0
        );
        assert_eq!(advice.estimated_speedup, 1.0);
        assert_eq!(advice.confidence, 1.0);

        let advice2 = IndexAdvice::new(
            "t",
            vec!["c".to_string()],
            IndexReason::MissingIndexForWhere,
            5.0,
            -0.5, // < 0.0 应被钳制到 0.0
        );
        assert_eq!(advice2.confidence, 0.0);
    }

    #[test]
    fn test_index_advice_create_index_sql() {
        let advice = IndexAdvice::new(
            "users",
            vec!["email".to_string()],
            IndexReason::MissingIndexForWhere,
            4.0,
            0.85,
        );
        let sql = advice.create_index_sql();
        assert!(sql.contains("CREATE INDEX"));
        assert!(sql.contains("idx_users_email"));
        assert!(sql.contains("ON users (email)"));
        assert!(sql.contains("WHERE 条件列无索引"));
    }

    #[test]
    fn test_index_advice_create_index_sql_composite() {
        let advice = IndexAdvice::new(
            "orders",
            vec!["user_id".to_string(), "status".to_string()],
            IndexReason::MissingIndexForWhere,
            5.0,
            0.9,
        );
        let sql = advice.create_index_sql();
        assert!(sql.contains("idx_orders_user_id_status"));
        assert!(sql.contains("ON orders (user_id, status)"));
    }

    #[test]
    fn test_index_advice_redundant_sql() {
        let advice = IndexAdvice::new(
            "users",
            vec!["id".to_string()],
            IndexReason::RedundantIndex,
            1.0,
            0.5,
        );
        let sql = advice.create_index_sql();
        assert!(sql.contains("冗余索引"));
        assert!(!sql.contains("CREATE INDEX"));
    }

    #[test]
    fn test_index_advice_is_high_confidence() {
        let high = IndexAdvice::new(
            "t",
            vec!["c".to_string()],
            IndexReason::MissingIndexForWhere,
            4.0,
            0.9,
        );
        assert!(high.is_high_confidence());

        let low = IndexAdvice::new(
            "t",
            vec!["c".to_string()],
            IndexReason::MissingIndexForWhere,
            4.0,
            0.5,
        );
        assert!(!low.is_high_confidence());
    }

    // -----------------------------------------------------------------
    //  SlowQueryEntry 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_slow_query_entry_new() {
        let entry = SlowQueryEntry::new(
            1,
            "SELECT * FROM users WHERE id = 123",
            500,
            1000,
            "admin",
            "testdb",
        );
        assert_eq!(entry.query_id, 1);
        assert_eq!(entry.sql_text, "SELECT * FROM users WHERE id = 123");
        assert_eq!(entry.duration_ms, 500);
        assert_eq!(entry.timestamp, 1000);
        assert_eq!(entry.user, "admin");
        assert_eq!(entry.database, "testdb");
        assert_eq!(entry.rows_returned, 0);
        assert_eq!(entry.bytes_scanned, 0);
        assert!(entry.index_used.is_none());
        assert!(entry.tables_accessed.is_empty());
        assert!(entry.plan.is_none());
    }

    #[test]
    fn test_slow_query_entry_normalized() {
        let entry = SlowQueryEntry::new(
            1,
            "SELECT * FROM users WHERE id = 123 AND name = 'abc'",
            500,
            1000,
            "admin",
            "testdb",
        );
        assert_eq!(
            entry.sql_normalized,
            "SELECT * FROM users WHERE id = ? AND name = ?"
        );
    }

    #[test]
    fn test_slow_query_entry_truncates_long_sql() {
        let long_sql = format!("SELECT * FROM t WHERE x = {}", "1".repeat(300));
        let entry = SlowQueryEntry::new(1, long_sql.clone(), 500, 1000, "u", "d");
        assert!(entry.sql_text.len() <= MAX_SQL_TEXT_LEN);
    }

    #[test]
    fn test_slow_query_entry_with_rows_returned() {
        let entry =
            SlowQueryEntry::new(1, "SELECT * FROM t", 100, 0, "u", "d").with_rows_returned(42);
        assert_eq!(entry.rows_returned, 42);
    }

    #[test]
    fn test_slow_query_entry_with_bytes_scanned() {
        let entry =
            SlowQueryEntry::new(1, "SELECT * FROM t", 100, 0, "u", "d").with_bytes_scanned(1024);
        assert_eq!(entry.bytes_scanned, 1024);
    }

    #[test]
    fn test_slow_query_entry_with_index() {
        let entry =
            SlowQueryEntry::new(1, "SELECT * FROM t", 100, 0, "u", "d").with_index("idx_t_id");
        assert_eq!(entry.index_used, Some("idx_t_id".to_string()));
        assert!(entry.uses_index());
    }

    #[test]
    fn test_slow_query_entry_with_tables() {
        let entry = SlowQueryEntry::new(1, "SELECT * FROM t", 100, 0, "u", "d")
            .with_tables(vec!["users".to_string(), "orders".to_string()]);
        assert_eq!(entry.tables_accessed, vec!["users", "orders"]);
    }

    #[test]
    fn test_slow_query_entry_with_plan() {
        let plan = QueryPlan::new(
            "Seq Scan on users",
            PlanOperator::SeqScan,
            vec!["users".to_string()],
            vec![],
            100.0,
            50000,
        );
        let entry = SlowQueryEntry::new(1, "SELECT * FROM users", 500, 0, "u", "d").with_plan(plan);
        assert!(entry.plan.is_some());
        assert!(entry.is_seq_scan());
    }

    #[test]
    fn test_slow_query_entry_is_seq_scan() {
        let with_seq =
            SlowQueryEntry::new(1, "SELECT * FROM t", 100, 0, "u", "d").with_plan(QueryPlan::new(
                "Seq Scan",
                PlanOperator::SeqScan,
                vec!["t".to_string()],
                vec![],
                100.0,
                1000,
            ));
        assert!(with_seq.is_seq_scan());

        let with_idx =
            SlowQueryEntry::new(1, "SELECT * FROM t", 100, 0, "u", "d").with_plan(QueryPlan::new(
                "Index Scan",
                PlanOperator::IndexScan,
                vec!["t".to_string()],
                vec!["idx".to_string()],
                5.0,
                1,
            ));
        assert!(!with_idx.is_seq_scan());
    }

    #[test]
    fn test_slow_query_entry_uses_index() {
        let with_idx =
            SlowQueryEntry::new(1, "SELECT * FROM t", 100, 0, "u", "d").with_index("idx_t");
        assert!(with_idx.uses_index());

        let without = SlowQueryEntry::new(1, "SELECT * FROM t", 100, 0, "u", "d");
        assert!(!without.uses_index());
    }

    #[test]
    fn test_slow_query_entry_duration_secs() {
        let entry = SlowQueryEntry::new(1, "SELECT 1", 1500, 0, "u", "d");
        assert!((entry.duration_secs() - 1.5).abs() < 0.001);
    }

    // -----------------------------------------------------------------
    //  SlowQueryConfig 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_slow_query_config_default() {
        let config = SlowQueryConfig::default();
        assert_eq!(config.threshold_ms, DEFAULT_SLOW_QUERY_THRESHOLD_MS);
        assert_eq!(config.max_log_entries, DEFAULT_MAX_LOG_ENTRIES);
        assert_eq!(config.top_n, DEFAULT_TOP_N);
    }

    #[test]
    fn test_slow_query_config_with_threshold_ms() {
        let config = SlowQueryConfig::new().with_threshold_ms(500);
        assert_eq!(config.threshold_ms, 500);
    }

    #[test]
    fn test_slow_query_config_with_max_log_entries() {
        let config = SlowQueryConfig::new().with_max_log_entries(1000);
        assert_eq!(config.max_log_entries, 1000);
    }

    #[test]
    fn test_slow_query_config_with_top_n() {
        let config = SlowQueryConfig::new().with_top_n(20);
        assert_eq!(config.top_n, 20);
    }

    // -----------------------------------------------------------------
    //  SlowQueryLogger 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_logger_new() {
        let logger = SlowQueryLogger::new();
        assert_eq!(logger.threshold_ms(), DEFAULT_SLOW_QUERY_THRESHOLD_MS);
        assert_eq!(logger.top_n(), DEFAULT_TOP_N);
        assert_eq!(logger.len(), 0);
        assert!(logger.is_empty());
        assert_eq!(logger.total_queries(), 0);
        assert_eq!(logger.total_logged(), 0);
        assert_eq!(logger.total_filtered(), 0);
        assert_eq!(logger.dropped_entries(), 0);
    }

    #[test]
    fn test_logger_with_config() {
        let config = SlowQueryConfig::new()
            .with_threshold_ms(500)
            .with_max_log_entries(100)
            .with_top_n(5);
        let logger = SlowQueryLogger::with_config(config);
        assert_eq!(logger.threshold_ms(), 500);
        assert_eq!(logger.top_n(), 5);
    }

    #[test]
    fn test_logger_logs_slow_query() {
        let mut logger = SlowQueryLogger::new();
        let entry = SlowQueryEntry::new(1, "SELECT * FROM t", 500, 0, "u", "d");
        assert!(logger.log(entry));
        assert_eq!(logger.len(), 1);
        assert_eq!(logger.total_queries(), 1);
        assert_eq!(logger.total_logged(), 1);
        assert_eq!(logger.total_filtered(), 0);
    }

    #[test]
    fn test_logger_filters_fast_query() {
        let mut logger = SlowQueryLogger::new();
        let entry = SlowQueryEntry::new(1, "SELECT * FROM t", 100, 0, "u", "d");
        assert!(!logger.log(entry));
        assert_eq!(logger.len(), 0);
        assert_eq!(logger.total_queries(), 1);
        assert_eq!(logger.total_logged(), 0);
        assert_eq!(logger.total_filtered(), 1);
    }

    #[test]
    fn test_logger_logs_at_threshold_boundary() {
        let mut logger = SlowQueryLogger::new();
        // 恰好等于阈值，应记录
        let entry = SlowQueryEntry::new(
            1,
            "SELECT * FROM t",
            DEFAULT_SLOW_QUERY_THRESHOLD_MS,
            0,
            "u",
            "d",
        );
        assert!(logger.log(entry));
        assert_eq!(logger.len(), 1);
    }

    #[test]
    fn test_logger_slow_ratio() {
        let mut logger = SlowQueryLogger::new();
        // 2 慢 + 8 快 = 20% 慢查询比例
        for i in 0..10 {
            let duration = if i < 2 {
                500
            } else {
                100
            };
            let entry = SlowQueryEntry::new(i, "SELECT * FROM t", duration, 0, "u", "d");
            logger.log(entry);
        }
        assert_eq!(logger.total_queries(), 10);
        assert_eq!(logger.total_logged(), 2);
        assert_eq!(logger.total_filtered(), 8);
        assert!((logger.slow_ratio() - 0.2).abs() < 0.001);
    }

    #[test]
    fn test_logger_slow_ratio_no_queries() {
        let logger = SlowQueryLogger::new();
        assert_eq!(logger.slow_ratio(), 0.0);
    }

    #[test]
    fn test_logger_eviction() {
        let config = SlowQueryConfig::new()
            .with_threshold_ms(0)
            .with_max_log_entries(3);
        let mut logger = SlowQueryLogger::with_config(config);
        for i in 0..5 {
            let entry = SlowQueryEntry::new(i, "SELECT * FROM t", 100, 0, "u", "d");
            logger.log(entry);
        }
        // 容量 3，写入 5，应丢弃 2 个
        assert_eq!(logger.len(), 3);
        assert_eq!(logger.dropped_entries(), 2);
        assert_eq!(logger.total_logged(), 5);
        // 保留的是最后 3 个（query_id 2, 3, 4）
        assert_eq!(logger.entries()[0].query_id, 2);
        assert_eq!(logger.entries()[2].query_id, 4);
    }

    #[test]
    fn test_logger_filter_by_duration() {
        let mut logger = SlowQueryLogger::with_config(SlowQueryConfig::new().with_threshold_ms(0));
        for i in 0..10 {
            let entry = SlowQueryEntry::new(i, "SELECT * FROM t", 500, i * 10, "u", "d");
            logger.log(entry);
        }
        let filtered = logger.filter_by_duration(30, 60);
        assert_eq!(filtered.len(), 4); // 时间戳 30, 40, 50, 60
    }

    #[test]
    fn test_logger_filter_by_user() {
        let mut logger = SlowQueryLogger::with_config(SlowQueryConfig::new().with_threshold_ms(0));
        logger.log(SlowQueryEntry::new(1, "SELECT 1", 500, 0, "alice", "db1"));
        logger.log(SlowQueryEntry::new(2, "SELECT 2", 500, 0, "bob", "db1"));
        logger.log(SlowQueryEntry::new(3, "SELECT 3", 500, 0, "alice", "db1"));

        let alice = logger.filter_by_user("alice");
        assert_eq!(alice.len(), 2);
        let bob = logger.filter_by_user("bob");
        assert_eq!(bob.len(), 1);
    }

    #[test]
    fn test_logger_filter_by_database() {
        let mut logger = SlowQueryLogger::with_config(SlowQueryConfig::new().with_threshold_ms(0));
        logger.log(SlowQueryEntry::new(1, "SELECT 1", 500, 0, "u", "db1"));
        logger.log(SlowQueryEntry::new(2, "SELECT 2", 500, 0, "u", "db2"));
        logger.log(SlowQueryEntry::new(3, "SELECT 3", 500, 0, "u", "db1"));

        let db1 = logger.filter_by_database("db1");
        assert_eq!(db1.len(), 2);
    }

    #[test]
    fn test_logger_filter_by_table() {
        let mut logger = SlowQueryLogger::with_config(SlowQueryConfig::new().with_threshold_ms(0));
        logger.log(
            SlowQueryEntry::new(1, "SELECT 1", 500, 0, "u", "d")
                .with_tables(vec!["users".to_string()]),
        );
        logger.log(
            SlowQueryEntry::new(2, "SELECT 2", 500, 0, "u", "d")
                .with_tables(vec!["orders".to_string()]),
        );
        logger.log(
            SlowQueryEntry::new(3, "SELECT 3", 500, 0, "u", "d")
                .with_tables(vec!["users".to_string(), "orders".to_string()]),
        );

        let users = logger.filter_by_table("users");
        assert_eq!(users.len(), 2);
    }

    #[test]
    fn test_logger_clear() {
        let mut logger = SlowQueryLogger::with_config(SlowQueryConfig::new().with_threshold_ms(0));
        logger.log(SlowQueryEntry::new(1, "SELECT 1", 500, 0, "u", "d"));
        logger.log(SlowQueryEntry::new(2, "SELECT 2", 500, 0, "u", "d"));
        assert_eq!(logger.len(), 2);
        logger.clear();
        assert_eq!(logger.len(), 0);
        // 统计不重置
        assert_eq!(logger.total_logged(), 2);
    }

    #[test]
    fn test_logger_reset_stats() {
        let mut logger = SlowQueryLogger::with_config(SlowQueryConfig::new().with_threshold_ms(0));
        logger.log(SlowQueryEntry::new(1, "SELECT 1", 500, 0, "u", "d"));
        logger.reset_stats();
        assert_eq!(logger.total_queries(), 0);
        assert_eq!(logger.total_logged(), 0);
        assert_eq!(logger.total_filtered(), 0);
        assert_eq!(logger.dropped_entries(), 0);
    }

    // -----------------------------------------------------------------
    //  SqlStatEntry 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_sql_stat_entry_new() {
        let stat = SqlStatEntry::new("SELECT * FROM t WHERE id = ?");
        assert_eq!(stat.sql_normalized, "SELECT * FROM t WHERE id = ?");
        assert_eq!(stat.count, 0);
        assert_eq!(stat.total_ms, 0);
        assert_eq!(stat.avg_ms, 0.0);
        assert_eq!(stat.max_ms, 0);
        assert_eq!(stat.min_ms, u64::MAX);
    }

    #[test]
    fn test_sql_stat_entry_accumulate() {
        let mut stat = SqlStatEntry::new("SELECT * FROM t WHERE id = ?");
        let entry1 = SlowQueryEntry::new(1, "SELECT * FROM t WHERE id = 1", 100, 0, "u", "d")
            .with_rows_returned(10)
            .with_bytes_scanned(1000);
        let entry2 = SlowQueryEntry::new(2, "SELECT * FROM t WHERE id = 2", 300, 0, "u", "d")
            .with_rows_returned(20)
            .with_bytes_scanned(2000);

        stat.accumulate(&entry1);
        assert_eq!(stat.count, 1);
        assert_eq!(stat.total_ms, 100);
        assert_eq!(stat.avg_ms, 100.0);
        assert_eq!(stat.max_ms, 100);
        assert_eq!(stat.min_ms, 100);

        stat.accumulate(&entry2);
        assert_eq!(stat.count, 2);
        assert_eq!(stat.total_ms, 400);
        assert_eq!(stat.avg_ms, 200.0);
        assert_eq!(stat.max_ms, 300);
        assert_eq!(stat.min_ms, 100);
        assert_eq!(stat.total_bytes_scanned, 3000);
        assert_eq!(stat.total_rows_returned, 30);
    }

    #[test]
    fn test_sql_stat_entry_is_all_seq_scan() {
        let mut stat = SqlStatEntry::new("SELECT * FROM t");
        let e1 =
            SlowQueryEntry::new(1, "SELECT * FROM t", 100, 0, "u", "d").with_plan(QueryPlan::new(
                "Seq Scan",
                PlanOperator::SeqScan,
                vec!["t".to_string()],
                vec![],
                10.0,
                100,
            ));
        let e2 =
            SlowQueryEntry::new(2, "SELECT * FROM t", 200, 0, "u", "d").with_plan(QueryPlan::new(
                "Seq Scan",
                PlanOperator::SeqScan,
                vec!["t".to_string()],
                vec![],
                10.0,
                100,
            ));
        stat.accumulate(&e1);
        stat.accumulate(&e2);
        assert!(stat.is_all_seq_scan());
    }

    #[test]
    fn test_sql_stat_entry_total_secs() {
        let mut stat = SqlStatEntry::new("SELECT * FROM t");
        stat.total_ms = 1500;
        assert!((stat.total_secs() - 1.5).abs() < 0.001);
    }

    // -----------------------------------------------------------------
    //  SlowQueryAnalysisReport 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_report_empty() {
        let report = SlowQueryAnalysisReport::empty();
        assert_eq!(report.total_slow_queries, 0);
        assert_eq!(report.total_duration_ms, 0);
        assert!(report.top_by_count.is_empty());
        assert!(report.index_advice.is_empty());
    }

    #[test]
    fn test_report_total_duration_secs() {
        let mut report = SlowQueryAnalysisReport::empty();
        report.total_duration_ms = 2500;
        assert!((report.total_duration_secs() - 2.5).abs() < 0.001);
    }

    #[test]
    fn test_report_seq_scan_ratio() {
        let mut report = SlowQueryAnalysisReport::empty();
        report.total_slow_queries = 10;
        report.seq_scan_count = 3;
        assert!((report.seq_scan_ratio() - 0.3).abs() < 0.001);
    }

    #[test]
    fn test_report_seq_scan_ratio_no_queries() {
        let report = SlowQueryAnalysisReport::empty();
        assert_eq!(report.seq_scan_ratio(), 0.0);
    }

    #[test]
    fn test_report_render_contains_sections() {
        let mut report = SlowQueryAnalysisReport::empty();
        report.total_slow_queries = 5;
        report.total_duration_ms = 2500;
        report.avg_duration_ms = 500.0;
        report.max_duration_ms = 1000;
        report.min_duration_ms = 100;
        report.seq_scan_count = 2;
        report.top_by_count = vec![SqlStatEntry::new("SELECT * FROM t")];
        report.top_by_total_time = vec![SqlStatEntry::new("SELECT * FROM t")];
        report.top_by_avg_time = vec![SqlStatEntry::new("SELECT * FROM t")];
        report.index_advice = vec![IndexAdvice::new(
            "t",
            vec!["id".to_string()],
            IndexReason::SeqScanOnLargeTable,
            3.0,
            0.8,
        )];

        let text = report.render();
        assert!(text.contains("Slow Query Analysis Report"));
        assert!(text.contains("Overview"));
        assert!(text.contains("Total slow queries: 5"));
        assert!(text.contains("Top 1 by Count"));
        assert!(text.contains("Top 1 by Total Time"));
        assert!(text.contains("Top 1 by Avg Time"));
        assert!(text.contains("Index Advice (1 suggestions)"));
    }

    // -----------------------------------------------------------------
    //  SlowQueryAnalyzer 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_analyzer_new() {
        let analyzer = SlowQueryAnalyzer::new();
        assert_eq!(analyzer.top_n, DEFAULT_TOP_N);
    }

    #[test]
    fn test_analyzer_with_top_n() {
        let analyzer = SlowQueryAnalyzer::new().with_top_n(20);
        assert_eq!(analyzer.top_n, 20);
    }

    #[test]
    fn test_analyzer_with_large_table_threshold() {
        let analyzer = SlowQueryAnalyzer::new().with_large_table_threshold(50000);
        assert_eq!(analyzer.large_table_threshold, 50000);
    }

    #[test]
    fn test_analyzer_analyze_empty() {
        let analyzer = SlowQueryAnalyzer::new();
        let report = analyzer.analyze(&[]);
        assert_eq!(report.total_slow_queries, 0);
    }

    #[test]
    fn test_analyzer_analyze_basic() {
        let analyzer = SlowQueryAnalyzer::new();
        let entries = vec![
            SlowQueryEntry::new(1, "SELECT * FROM t WHERE id = 1", 500, 0, "u", "d"),
            SlowQueryEntry::new(2, "SELECT * FROM t WHERE id = 2", 300, 0, "u", "d"),
            SlowQueryEntry::new(3, "SELECT * FROM t WHERE id = 3", 700, 0, "u", "d"),
        ];
        let report = analyzer.analyze(&entries);
        assert_eq!(report.total_slow_queries, 3);
        assert_eq!(report.total_duration_ms, 1500);
        assert!((report.avg_duration_ms - 500.0).abs() < 0.001);
        assert_eq!(report.max_duration_ms, 700);
        assert_eq!(report.min_duration_ms, 300);
    }

    #[test]
    fn test_analyzer_analyze_aggregates_same_normalized_sql() {
        let analyzer = SlowQueryAnalyzer::new();
        let entries = vec![
            SlowQueryEntry::new(1, "SELECT * FROM t WHERE id = 1", 500, 0, "u", "d"),
            SlowQueryEntry::new(2, "SELECT * FROM t WHERE id = 2", 300, 0, "u", "d"),
            SlowQueryEntry::new(3, "SELECT * FROM t WHERE id = 3", 700, 0, "u", "d"),
        ];
        let report = analyzer.analyze(&entries);
        // 3 条 SQL 归一化后相同，应聚合成 1 个统计条目
        assert_eq!(report.top_by_count.len(), 1);
        assert_eq!(report.top_by_count[0].count, 3);
        assert_eq!(report.top_by_count[0].total_ms, 1500);
    }

    #[test]
    fn test_analyzer_analyze_top_by_count_ordering() {
        let analyzer = SlowQueryAnalyzer::new();
        let entries = vec![
            SlowQueryEntry::new(1, "SELECT * FROM a WHERE id = 1", 500, 0, "u", "d"),
            SlowQueryEntry::new(2, "SELECT * FROM b WHERE id = 1", 300, 0, "u", "d"),
            SlowQueryEntry::new(3, "SELECT * FROM a WHERE id = 2", 500, 0, "u", "d"),
            SlowQueryEntry::new(4, "SELECT * FROM a WHERE id = 3", 500, 0, "u", "d"),
        ];
        let report = analyzer.analyze(&entries);
        // a 出现 3 次，b 出现 1 次
        assert_eq!(report.top_by_count[0].count, 3);
        assert!(report.top_by_count[0].sql_normalized.contains("FROM a"));
    }

    #[test]
    fn test_analyzer_analyze_top_by_total_time_ordering() {
        let analyzer = SlowQueryAnalyzer::new();
        let entries = vec![
            SlowQueryEntry::new(1, "SELECT * FROM a WHERE id = 1", 100, 0, "u", "d"),
            SlowQueryEntry::new(2, "SELECT * FROM b WHERE id = 1", 900, 0, "u", "d"),
        ];
        let report = analyzer.analyze(&entries);
        // b 总时间 900，a 总时间 100
        assert_eq!(report.top_by_total_time[0].total_ms, 900);
    }

    #[test]
    fn test_analyzer_analyze_top_by_avg_time_ordering() {
        let analyzer = SlowQueryAnalyzer::new();
        let entries = vec![
            SlowQueryEntry::new(1, "SELECT * FROM a WHERE id = 1", 100, 0, "u", "d"),
            SlowQueryEntry::new(2, "SELECT * FROM a WHERE id = 2", 200, 0, "u", "d"),
            SlowQueryEntry::new(3, "SELECT * FROM b WHERE id = 1", 500, 0, "u", "d"),
        ];
        let report = analyzer.analyze(&entries);
        // a 平均 150，b 平均 500
        assert!((report.top_by_avg_time[0].avg_ms - 500.0).abs() < 0.001);
    }

    #[test]
    fn test_analyzer_analyze_truncates_top_n() {
        let analyzer = SlowQueryAnalyzer::new().with_top_n(2);
        let table_names = ["alpha", "beta", "gamma", "delta", "epsilon"];
        let entries: Vec<_> = (0..5)
            .map(|i| {
                SlowQueryEntry::new(
                    i,
                    format!("SELECT * FROM {} WHERE id = {}", table_names[i as usize], i),
                    500,
                    0,
                    "u",
                    "d",
                )
            })
            .collect();
        let report = analyzer.analyze(&entries);
        assert_eq!(report.top_by_count.len(), 2);
        assert_eq!(report.top_by_total_time.len(), 2);
        assert_eq!(report.top_by_avg_time.len(), 2);
    }

    #[test]
    fn test_analyzer_suggest_indexes_seq_scan_large_table() {
        let analyzer = SlowQueryAnalyzer::new().with_large_table_threshold(1000);
        let entry =
            SlowQueryEntry::new(1, "SELECT * FROM users WHERE email = 'x'", 500, 0, "u", "d")
                .with_plan(QueryPlan::new(
                    "Seq Scan on users",
                    PlanOperator::SeqScan,
                    vec!["users".to_string()],
                    vec![],
                    1000.0,
                    50000,
                ));
        let advices = analyzer.suggest_indexes(&entry);
        assert!(!advices.is_empty());
        let has_seq_scan_advice = advices
            .iter()
            .any(|a| a.reason == IndexReason::SeqScanOnLargeTable);
        assert!(has_seq_scan_advice);
    }

    #[test]
    fn test_analyzer_suggest_indexes_no_advice_for_index_scan() {
        let analyzer = SlowQueryAnalyzer::new();
        let entry = SlowQueryEntry::new(1, "SELECT * FROM t WHERE id = 1", 500, 0, "u", "d")
            .with_plan(QueryPlan::new(
                "Index Scan",
                PlanOperator::IndexScan,
                vec!["t".to_string()],
                vec!["idx_t_id".to_string()],
                5.0,
                1,
            ));
        let advices = analyzer.suggest_indexes(&entry);
        assert!(advices.is_empty());
    }

    #[test]
    fn test_analyzer_suggest_indexes_no_advice_without_plan() {
        let analyzer = SlowQueryAnalyzer::new();
        let entry = SlowQueryEntry::new(1, "SELECT * FROM t WHERE id = 1", 500, 0, "u", "d");
        let advices = analyzer.suggest_indexes(&entry);
        assert!(advices.is_empty());
    }

    #[test]
    fn test_analyzer_suggest_indexes_where_missing() {
        let analyzer = SlowQueryAnalyzer::new();
        let entry =
            SlowQueryEntry::new(1, "SELECT * FROM users WHERE email = 'x'", 500, 0, "u", "d")
                .with_plan(QueryPlan::new(
                    "Seq Scan on users",
                    PlanOperator::SeqScan,
                    vec!["users".to_string()],
                    vec![],
                    100.0,
                    100,
                ));
        let advices = analyzer.suggest_indexes(&entry);
        // 小表 SeqScan 不触发 SeqScanOnLargeTable，但 WHERE 列无索引触发
        let has_where_advice = advices
            .iter()
            .any(|a| a.reason == IndexReason::MissingIndexForWhere);
        assert!(has_where_advice);
    }

    #[test]
    fn test_analyzer_suggest_indexes_join_missing() {
        let analyzer = SlowQueryAnalyzer::new();
        let sql =
            "SELECT u.name, o.total FROM users u JOIN orders o ON u.id = o.user_id WHERE u.id = 1";
        let entry = SlowQueryEntry::new(1, sql, 500, 0, "u", "d").with_plan(QueryPlan::new(
            "Hash Join",
            PlanOperator::HashJoin,
            vec!["users".to_string(), "orders".to_string()],
            vec![],
            100.0,
            100,
        ));
        let advices = analyzer.suggest_indexes(&entry);
        let has_join_advice = advices
            .iter()
            .any(|a| a.reason == IndexReason::MissingIndexForJoin);
        assert!(has_join_advice);
    }

    #[test]
    fn test_analyzer_suggest_indexes_order_by_missing() {
        let analyzer = SlowQueryAnalyzer::new();
        let sql = "SELECT * FROM users WHERE id = 1 ORDER BY created_at";
        let entry = SlowQueryEntry::new(1, sql, 500, 0, "u", "d").with_plan(QueryPlan::new(
            "Sort",
            PlanOperator::Sort,
            vec!["users".to_string()],
            vec![],
            100.0,
            100,
        ));
        let advices = analyzer.suggest_indexes(&entry);
        let has_order_advice = advices
            .iter()
            .any(|a| a.reason == IndexReason::MissingIndexForOrderBy);
        assert!(has_order_advice);
    }

    #[test]
    fn test_analyzer_analyze_deduplicates_advice() {
        let analyzer = SlowQueryAnalyzer::new().with_large_table_threshold(1000);
        let plan = QueryPlan::new(
            "Seq Scan on users",
            PlanOperator::SeqScan,
            vec!["users".to_string()],
            vec![],
            1000.0,
            50000,
        );
        let entries = vec![
            SlowQueryEntry::new(1, "SELECT * FROM users WHERE email = 'a'", 500, 0, "u", "d")
                .with_plan(plan.clone()),
            SlowQueryEntry::new(2, "SELECT * FROM users WHERE email = 'b'", 600, 0, "u", "d")
                .with_plan(plan.clone()),
            SlowQueryEntry::new(3, "SELECT * FROM users WHERE email = 'c'", 700, 0, "u", "d")
                .with_plan(plan),
        ];
        let report = analyzer.analyze(&entries);
        // 同一表 + 同一列的索引建议应去重
        let seq_scan_advice: Vec<_> = report
            .index_advice
            .iter()
            .filter(|a| a.reason == IndexReason::SeqScanOnLargeTable)
            .collect();
        assert!(seq_scan_advice.len() <= 1);
    }

    // -----------------------------------------------------------------
    //  normalize_sql 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_normalize_sql_string_literal() {
        let sql = "SELECT * FROM t WHERE name = 'hello'";
        assert_eq!(normalize_sql(sql), "SELECT * FROM t WHERE name = ?");
    }

    #[test]
    fn test_normalize_sql_number() {
        let sql = "SELECT * FROM t WHERE id = 123";
        assert_eq!(normalize_sql(sql), "SELECT * FROM t WHERE id = ?");
    }

    #[test]
    fn test_normalize_sql_float() {
        let sql = "SELECT * FROM t WHERE price = 12.99";
        assert_eq!(normalize_sql(sql), "SELECT * FROM t WHERE price = ?");
    }

    #[test]
    fn test_normalize_sql_multiple_params() {
        let sql = "SELECT * FROM t WHERE id = 1 AND name = 'abc' AND age > 20";
        assert_eq!(
            normalize_sql(sql),
            "SELECT * FROM t WHERE id = ? AND name = ? AND age > ?"
        );
    }

    #[test]
    fn test_normalize_sql_escaped_quote() {
        let sql = "SELECT * FROM t WHERE name = 'O''Brien'";
        assert_eq!(normalize_sql(sql), "SELECT * FROM t WHERE name = ?");
    }

    #[test]
    fn test_normalize_sql_collapse_whitespace() {
        let sql = "SELECT  *   FROM    t   WHERE   id   =   1";
        assert_eq!(normalize_sql(sql), "SELECT * FROM t WHERE id = ?");
    }

    #[test]
    fn test_normalize_sql_preserves_double_quoted_identifiers() {
        let sql = "SELECT * FROM \"my table\" WHERE id = 1";
        assert_eq!(
            normalize_sql(sql),
            "SELECT * FROM \"my table\" WHERE id = ?"
        );
    }

    #[test]
    fn test_normalize_sql_empty() {
        assert_eq!(normalize_sql(""), "");
    }

    #[test]
    fn test_normalize_sql_no_params() {
        assert_eq!(normalize_sql("SELECT * FROM t"), "SELECT * FROM t");
    }

    // -----------------------------------------------------------------
    //  extract_where_columns 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_where_columns_basic() {
        let sql = "SELECT * FROM users WHERE email = 'x'";
        let cols = extract_where_columns(sql, "users");
        assert!(cols.is_some());
        assert_eq!(cols.unwrap(), vec!["email"]);
    }

    #[test]
    fn test_extract_where_columns_multiple() {
        let sql = "SELECT * FROM users WHERE id = 1 AND name = 'x' AND age > 20";
        let cols = extract_where_columns(sql, "users").unwrap();
        assert!(cols.contains(&"id".to_string()));
        assert!(cols.contains(&"name".to_string()));
        assert!(cols.contains(&"age".to_string()));
    }

    #[test]
    fn test_extract_where_columns_table_prefix() {
        let sql = "SELECT * FROM users WHERE users.email = 'x'";
        let cols = extract_where_columns(sql, "users").unwrap();
        assert!(cols.contains(&"email".to_string()));
    }

    #[test]
    fn test_extract_where_columns_no_where() {
        let sql = "SELECT * FROM users";
        assert!(extract_where_columns(sql, "users").is_none());
    }

    #[test]
    fn test_extract_where_columns_truncated_at_group_by() {
        let sql = "SELECT * FROM users WHERE id = 1 GROUP BY status";
        let cols = extract_where_columns(sql, "users").unwrap();
        assert!(cols.contains(&"id".to_string()));
        assert!(!cols.contains(&"status".to_string()));
    }

    // -----------------------------------------------------------------
    //  extract_join_columns 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_join_columns_basic() {
        let sql = "SELECT * FROM users u JOIN orders o ON u.id = o.user_id";
        let cols = extract_join_columns(sql).unwrap();
        assert!(cols.contains(&("u".to_string(), "id".to_string())));
        assert!(cols.contains(&("o".to_string(), "user_id".to_string())));
    }

    #[test]
    fn test_extract_join_columns_no_join() {
        let sql = "SELECT * FROM users";
        assert!(extract_join_columns(sql).is_none());
    }

    #[test]
    fn test_extract_join_columns_multiple_conditions() {
        let sql = "SELECT * FROM a JOIN b ON a.id = b.aid AND a.code = b.code";
        let cols = extract_join_columns(sql).unwrap();
        assert!(cols.len() >= 2);
    }

    // -----------------------------------------------------------------
    //  extract_order_by_columns 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_order_by_columns_basic() {
        let sql = "SELECT * FROM t ORDER BY name";
        let cols = extract_order_by_columns(sql).unwrap();
        assert_eq!(cols, vec!["name"]);
    }

    #[test]
    fn test_extract_order_by_columns_multiple() {
        let sql = "SELECT * FROM t ORDER BY name, age DESC";
        let cols = extract_order_by_columns(sql).unwrap();
        assert!(cols.contains(&"name".to_string()));
        assert!(cols.contains(&"age".to_string()));
    }

    #[test]
    fn test_extract_order_by_columns_no_order_by() {
        let sql = "SELECT * FROM t";
        assert!(extract_order_by_columns(sql).is_none());
    }

    #[test]
    fn test_extract_order_by_columns_with_limit() {
        let sql = "SELECT * FROM t ORDER BY id LIMIT 10";
        let cols = extract_order_by_columns(sql).unwrap();
        assert_eq!(cols, vec!["id"]);
    }

    // -----------------------------------------------------------------
    //  generate_mixed_queries 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_generate_mixed_queries_basic() {
        let queries = generate_mixed_queries(100, 0.1);
        assert_eq!(queries.len(), 100);
    }

    #[test]
    fn test_generate_mixed_queries_slow_ratio() {
        let queries = generate_mixed_queries(100, 0.1);
        let slow_count = queries
            .iter()
            .filter(|q| q.duration_ms >= DEFAULT_SLOW_QUERY_THRESHOLD_MS)
            .count();
        assert_eq!(slow_count, 10);
    }

    #[test]
    fn test_generate_mixed_queries_zero_slow() {
        let queries = generate_mixed_queries(50, 0.0);
        let slow_count = queries
            .iter()
            .filter(|q| q.duration_ms >= DEFAULT_SLOW_QUERY_THRESHOLD_MS)
            .count();
        assert_eq!(slow_count, 0);
    }

    #[test]
    fn test_generate_mixed_queries_all_slow() {
        let queries = generate_mixed_queries(50, 1.0);
        let slow_count = queries
            .iter()
            .filter(|q| q.duration_ms >= DEFAULT_SLOW_QUERY_THRESHOLD_MS)
            .count();
        assert_eq!(slow_count, 50);
    }

    // -----------------------------------------------------------------
    //  集成测试
    // -----------------------------------------------------------------

    #[test]
    fn test_integration_1000_queries_10_percent_slow() {
        // 验证标准：配置慢查询阈值 200ms → 执行 1000 条查询（10% 慢查询）→ 慢查询日志自动记录
        let mut logger = SlowQueryLogger::new();
        let queries = generate_mixed_queries(1000, 0.1);
        for q in queries {
            logger.log(q);
        }

        assert_eq!(logger.total_queries(), 1000);
        assert_eq!(logger.total_logged(), 100);
        assert_eq!(logger.total_filtered(), 900);
        assert!((logger.slow_ratio() - 0.1).abs() < 0.001);
        assert_eq!(logger.len(), 100);
    }

    #[test]
    fn test_integration_analysis_report_top_10() {
        // 验证标准：分析报告包含 Top 10
        let mut logger = SlowQueryLogger::new();
        let queries = generate_mixed_queries(1000, 0.1);
        for q in queries {
            logger.log(q);
        }

        let analyzer = SlowQueryAnalyzer::new().with_top_n(10);
        let report = analyzer.analyze(logger.entries());

        assert_eq!(report.total_slow_queries, 100);
        assert!(report.top_by_count.len() <= 10);
        assert!(report.top_by_total_time.len() <= 10);
        assert!(report.top_by_avg_time.len() <= 10);
    }

    #[test]
    fn test_integration_index_advice_reasonable() {
        // 验证标准：索引建议合理
        let mut logger = SlowQueryLogger::new();
        let plan = QueryPlan::new(
            "Seq Scan on large_table",
            PlanOperator::SeqScan,
            vec!["large_table".to_string()],
            vec![],
            10000.0,
            1_000_000,
        );
        for i in 0..10 {
            let entry = SlowQueryEntry::new(
                i,
                format!(
                    "SELECT * FROM large_table WHERE col1 = {} AND col2 > {}",
                    i,
                    i * 10
                ),
                500 + i * 50,
                i,
                "u",
                "d",
            )
            .with_plan(plan.clone())
            .with_bytes_scanned(100_000_000);
            logger.log(entry);
        }

        let analyzer = SlowQueryAnalyzer::new().with_large_table_threshold(10000);
        let report = analyzer.analyze(logger.entries());

        assert!(!report.index_advice.is_empty());
        // 应有 SeqScanOnLargeTable 建议
        let has_seq_advice = report
            .index_advice
            .iter()
            .any(|a| a.reason == IndexReason::SeqScanOnLargeTable);
        assert!(has_seq_advice);
    }

    #[test]
    fn test_integration_full_workflow() {
        // 完整工作流：日志记录 → 聚合分析 → Top 10 → 索引建议
        let mut logger = SlowQueryLogger::with_config(
            SlowQueryConfig::new()
                .with_threshold_ms(200)
                .with_max_log_entries(10000)
                .with_top_n(10),
        );

        // 生成混合负载：500 条快 + 100 条慢
        let fast: Vec<_> = (0..500)
            .map(|i| {
                SlowQueryEntry::new(
                    i,
                    format!("SELECT * FROM fast_table WHERE id = {}", i),
                    50,
                    i,
                    "u",
                    "d",
                )
            })
            .collect();
        let slow: Vec<_> = (0..100)
            .map(|i| {
                let plan = QueryPlan::new(
                    "Seq Scan on slow_table",
                    PlanOperator::SeqScan,
                    vec!["slow_table".to_string()],
                    vec![],
                    1000.0,
                    100_000,
                );
                SlowQueryEntry::new(
                    500 + i,
                    format!(
                        "SELECT * FROM slow_table WHERE user_id = {} AND status = {}",
                        i, i
                    ),
                    300 + i * 10,
                    i,
                    "u",
                    "d",
                )
                .with_plan(plan)
                .with_bytes_scanned(50_000_000)
            })
            .collect();

        for q in fast.into_iter().chain(slow) {
            logger.log(q);
        }

        assert_eq!(logger.total_queries(), 600);
        assert_eq!(logger.total_logged(), 100);
        assert_eq!(logger.total_filtered(), 500);

        let analyzer = SlowQueryAnalyzer::new()
            .with_top_n(10)
            .with_large_table_threshold(10000);
        let report = analyzer.analyze(logger.entries());

        assert_eq!(report.total_slow_queries, 100);
        assert!(report.total_duration_ms > 0);
        assert!(report.max_duration_ms >= 300);
        assert!(report.min_duration_ms >= 200);
        assert_eq!(report.seq_scan_count, 100);
        assert!(!report.index_advice.is_empty());

        let text = report.render();
        assert!(text.contains("Slow Query Analysis Report"));
        assert!(text.contains("Total slow queries: 100"));
        assert!(text.contains("Seq scan count: 100"));
    }

    #[test]
    fn test_integration_normalized_aggregation() {
        // 验证归一化聚合：不同参数的相同 SQL 模板聚合成一条
        let mut logger = SlowQueryLogger::new();
        for i in 0..50 {
            let entry = SlowQueryEntry::new(
                i,
                format!("SELECT * FROM t WHERE id = {} AND name = 'user{}'", i, i),
                500,
                i,
                "u",
                "d",
            );
            logger.log(entry);
        }

        let analyzer = SlowQueryAnalyzer::new();
        let report = analyzer.analyze(logger.entries());

        // 所有 50 条 SQL 归一化后相同，应聚合成 1 个统计条目
        assert_eq!(report.top_by_count.len(), 1);
        assert_eq!(report.top_by_count[0].count, 50);
        assert_eq!(report.top_by_count[0].total_ms, 25000);
    }

    #[test]
    fn test_integration_join_query_advice() {
        // JOIN 查询索引建议
        let mut logger = SlowQueryLogger::new();
        let plan = QueryPlan::new(
            "Hash Join",
            PlanOperator::HashJoin,
            vec!["users".to_string(), "orders".to_string()],
            vec![],
            500.0,
            10000,
        );
        let sql =
            "SELECT u.name, o.total FROM users u JOIN orders o ON u.id = o.user_id WHERE u.active = 1";
        let entry = SlowQueryEntry::new(1, sql, 800, 0, "u", "d").with_plan(plan);
        logger.log(entry);

        let analyzer = SlowQueryAnalyzer::new();
        let report = analyzer.analyze(logger.entries());

        // 应有 JOIN 列索引建议
        let join_advice: Vec<_> = report
            .index_advice
            .iter()
            .filter(|a| a.reason == IndexReason::MissingIndexForJoin)
            .collect();
        assert!(!join_advice.is_empty());
    }

    #[test]
    fn test_integration_order_by_advice() {
        // ORDER BY 索引建议
        let mut logger = SlowQueryLogger::new();
        let plan = QueryPlan::new(
            "Sort",
            PlanOperator::Sort,
            vec!["events".to_string()],
            vec![],
            200.0,
            1000,
        );
        let sql = "SELECT * FROM events WHERE type = 'login' ORDER BY created_at DESC LIMIT 100";
        let entry = SlowQueryEntry::new(1, sql, 600, 0, "u", "d").with_plan(plan);
        logger.log(entry);

        let analyzer = SlowQueryAnalyzer::new();
        let report = analyzer.analyze(logger.entries());

        let order_advice: Vec<_> = report
            .index_advice
            .iter()
            .filter(|a| a.reason == IndexReason::MissingIndexForOrderBy)
            .collect();
        assert!(!order_advice.is_empty());
    }

    #[test]
    fn test_integration_no_advice_for_indexed_queries() {
        // 已使用索引的查询不应有索引建议
        let mut logger = SlowQueryLogger::new();
        let plan = QueryPlan::new(
            "Index Scan using idx_users_email on users",
            PlanOperator::IndexScan,
            vec!["users".to_string()],
            vec!["idx_users_email".to_string()],
            5.0,
            1,
        );
        let entry =
            SlowQueryEntry::new(1, "SELECT * FROM users WHERE email = 'x'", 300, 0, "u", "d")
                .with_plan(plan);
        logger.log(entry);

        let analyzer = SlowQueryAnalyzer::new();
        let report = analyzer.analyze(logger.entries());
        assert!(report.index_advice.is_empty());
    }

    #[test]
    fn test_integration_render_report_full() {
        // 完整渲染报告验证
        let mut logger = SlowQueryLogger::new();
        let entries = generate_mixed_queries(100, 0.5);
        for e in entries {
            logger.log(e);
        }

        let analyzer = SlowQueryAnalyzer::new();
        let report = analyzer.analyze(logger.entries());
        let text = report.render();

        assert!(text.contains("Slow Query Analysis Report"));
        assert!(text.contains("Overview"));
        assert!(text.contains("Total slow queries:"));
        assert!(text.contains("Top"));
        assert!(text.contains("Index Advice"));
    }

    #[test]
    fn test_integration_logger_eviction_with_stats() {
        // 环形缓冲区淘汰 + 统计正确性
        let config = SlowQueryConfig::new()
            .with_threshold_ms(0)
            .with_max_log_entries(10);
        let mut logger = SlowQueryLogger::with_config(config);

        for i in 0..25 {
            let entry = SlowQueryEntry::new(i, "SELECT * FROM t", 100, i, "u", "d");
            logger.log(entry);
        }

        assert_eq!(logger.len(), 10);
        assert_eq!(logger.dropped_entries(), 15);
        assert_eq!(logger.total_logged(), 25);
        // 保留的是最后 10 个
        assert_eq!(logger.entries()[0].query_id, 15);
        assert_eq!(logger.entries()[9].query_id, 24);
    }

    #[test]
    fn test_integration_all_slow_queries_logged() {
        // 验证标准：慢查询 100% 记录
        let mut logger = SlowQueryLogger::new();
        for i in 0..100 {
            let entry = SlowQueryEntry::new(
                i,
                format!("SELECT * FROM t WHERE id = {}", i),
                250,
                i,
                "u",
                "d",
            );
            logger.log(entry);
        }
        assert_eq!(logger.total_logged(), 100);
        assert_eq!(logger.len(), 100);
        // 全部慢查询都被记录
        for entry in logger.entries() {
            assert!(entry.duration_ms >= DEFAULT_SLOW_QUERY_THRESHOLD_MS);
        }
    }

    #[test]
    fn test_integration_filter_combinations() {
        // 多维过滤组合测试
        let mut logger = SlowQueryLogger::with_config(SlowQueryConfig::new().with_threshold_ms(0));
        for i in 0..20 {
            let user = if i % 2 == 0 {
                "alice"
            } else {
                "bob"
            };
            let db = if i % 3 == 0 {
                "db1"
            } else {
                "db2"
            };
            let entry = SlowQueryEntry::new(i, "SELECT * FROM t", 500, i, user, db);
            logger.log(entry);
        }

        // 时间范围 + 用户
        let filtered: Vec<_> = logger
            .filter_by_duration(5, 15)
            .into_iter()
            .filter(|e| e.user == "alice")
            .collect();
        // 时间戳 5~15 之间有 11 条，其中 alice 是偶数时间戳
        assert!(!filtered.is_empty());
        for e in &filtered {
            assert!(e.timestamp >= 5 && e.timestamp <= 15);
            assert_eq!(e.user, "alice");
        }
    }
}
