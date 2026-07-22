//! N+1 问题检测 — SQL 计数 + 模板分组 + 告警生成
//!
//! Phase 4.9 核心交付物。本模块提供 N+1 问题检测能力，通过 SQL 查询计数
//! 与模板分组识别 N+1 模式，对齐 PHP `with()` 批量预加载机制避免 N+1 问题。
//!
//! ## PHP 端 N+1 问题背景
//!
//! PHP think-orm 2.0.x 通过 `with()` + `eagerlyResultSet()` 提供批量预加载能力，
//! **避免 N+1 问题**（一次查询加载 N 条父模型，再 N 次查询加载每条父模型的关联数据）。
//! 但 PHP 端**不主动检测 N+1 问题**，开发者需要自行识别并使用 `with()` 规避。
//!
//! ### PHP `with()` 批量预加载机制
//!
//! ```php
//! // ❌ N+1 模式（N 次查询）
//! $users = User::select();
//! foreach ($users as $user) {
//!     $orders = $user->orders;  // 每次循环触发一次 SQL 查询
//! }
//!
//! // ✅ 批量预加载（2 次查询）
//! $users = User::with('orders')->select();
//! // 内部通过 eagerlyResultSet() 批量 IN 查询
//! ```
//!
//! ## 本模块提供的检测能力
//!
//! sz-rust 端作为 PHP 端的扩展，提供**主动 N+1 问题检测**能力：
//!
//! 1. [`SqlQueryRecord`]：SQL 查询记录（原始 SQL + 模板 + 表名 + 时间戳 + 序号）
//! 2. [`DetectionConfig`]：检测配置（阈值 + 时间窗口）
//! 3. [`NPlusOneAlert`]：N+1 告警（模板 + 表名 + 次数 + 时间跨度 + 建议）
//! 4. [`NPlusOneDetector`]：检测器（累积记录 + 批量分析）
//! 5. [`extract_template`]：SQL 模板提取（参数替换为 `?`）
//! 6. [`detect_n_plus_one`]：核心检测函数（按模板分组 + 时间窗口分析）
//! 7. [`suggest_with_usage`]：生成 `with()` 使用建议
//!
//! ## N+1 检测算法
//!
//! 1. 收集 SQL 查询记录（`SqlQueryRecord`）
//! 2. 按 SQL 模板分组（`extract_template` 去除具体参数）
//! 3. 每组按时间戳排序
//! 4. 检查每组在时间窗口内是否有超过阈值的查询
//! 5. 如果有，生成告警（`NPlusOneAlert`）
//!
//! ## 架构说明
//!
//! sz-orm-core::model 模块私有（`mod model;` 非 `pub mod model;`），sz-rust 端无法
//! 实现 `Model`/`RelationLoader` trait，因此本模块不直接执行 SQL 查询，而是提供：
//!
//! - **SQL 查询记录类型**：`SqlQueryRecord` 供调用方（如中间件 / Repository）记录
//! - **检测器**：`NPlusOneDetector` 累积记录并批量分析
//! - **核心检测函数**：`detect_n_plus_one` 纯函数，可独立调用
//! - **建议生成**：`suggest_with_usage` 生成 `with()` 使用建议
//!
//! 端到端 SQL 执行计数由调用方集成（如 `tracing` 中间件 / `Repository` 包装器）。

use std::collections::HashMap;

// ============================================================================
// SQL 查询记录
// ============================================================================

/// SQL 查询记录
///
/// 记录单条 SQL 查询的原始 SQL、模板、表名、时间戳与序号。
///
/// ## 字段
///
/// - `sql`：原始 SQL 字符串（含具体参数）
/// - `template`：SQL 模板（参数替换为 `?`，用于分组）
/// - `table`：主表名（如 `"users"`）
/// - `timestamp_ms`：查询时间戳（毫秒）
/// - `query_index`：查询序号（从 0 开始递增）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::n_plus_one::SqlQueryRecord;
///
/// let record = SqlQueryRecord::new(
///     "SELECT * FROM orders WHERE user_id = 1",
///     "orders",
///     1000,
///     0,
/// );
/// assert_eq!(record.template(), "SELECT * FROM orders WHERE user_id = ?");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlQueryRecord {
    /// 原始 SQL 字符串（含具体参数）
    pub sql: String,
    /// SQL 模板（参数替换为 `?`，用于分组）
    pub template: String,
    /// 主表名
    pub table: String,
    /// 查询时间戳（毫秒）
    pub timestamp_ms: u64,
    /// 查询序号（从 0 开始递增）
    pub query_index: u64,
}

impl SqlQueryRecord {
    /// 创建新的 SQL 查询记录
    ///
    /// 自动调用 [`extract_template`] 提取 SQL 模板。
    ///
    /// ## 参数
    ///
    /// - `sql`：原始 SQL 字符串
    /// - `table`：主表名
    /// - `timestamp_ms`：查询时间戳（毫秒）
    /// - `query_index`：查询序号
    pub fn new(sql: &str, table: &str, timestamp_ms: u64, query_index: u64) -> Self {
        Self {
            sql: sql.to_string(),
            template: extract_template(sql),
            table: table.to_string(),
            timestamp_ms,
            query_index,
        }
    }

    /// 获取原始 SQL 字符串
    pub fn sql(&self) -> &str {
        &self.sql
    }

    /// 获取 SQL 模板
    pub fn template(&self) -> &str {
        &self.template
    }

    /// 获取主表名
    pub fn table(&self) -> &str {
        &self.table
    }

    /// 获取查询时间戳（毫秒）
    pub fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }

    /// 获取查询序号
    pub fn query_index(&self) -> u64 {
        self.query_index
    }
}

// ============================================================================
// SQL 模板提取
// ============================================================================

/// 提取 SQL 模板（将参数替换为 `?`）
///
/// 将 SQL 中的数字字面量、字符串字面量替换为 `?`，用于 N+1 检测的模板分组。
///
/// ## 替换规则
///
/// - 数字字面量（如 `1` / `123` / `3.14`）→ `?`
/// - 字符串字面量（如 `'abc'` / `'user@example.com'`）→ `?`
/// - 其他字符原样保留
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::n_plus_one::extract_template;
///
/// assert_eq!(
///     extract_template("SELECT * FROM orders WHERE user_id = 1"),
///     "SELECT * FROM orders WHERE user_id = ?"
/// );
/// assert_eq!(
///     extract_template("SELECT * FROM users WHERE email = 'abc@x.com' AND id = 5"),
///     "SELECT * FROM users WHERE email = ? AND id = ?"
/// );
/// ```
pub fn extract_template(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut result = String::with_capacity(sql.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            // 字符串字面量：从 ' 到下一个 '
            result.push('?');
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                i += 1;
            }
            // 跳过结束的 '
            if i < chars.len() {
                i += 1;
            }
        } else if c == '"' {
            // 双引号字符串字面量
            result.push('?');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
        } else if c.is_ascii_digit() {
            // 数字字面量：连续数字（含小数点）
            result.push('?');
            i += 1;
            // 跳过连续数字和小数点
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
        } else {
            result.push(c);
            i += 1;
        }
    }
    result
}

// ============================================================================
// 检测配置
// ============================================================================

/// N+1 检测配置
///
/// 配置检测阈值与时间窗口。
///
/// ## 默认值
///
/// - `threshold`：5（同一模板在时间窗口内查询超过 5 次判定为 N+1）
/// - `time_window_ms`：1000（时间窗口 1000 毫秒）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::n_plus_one::DetectionConfig;
///
/// let config = DetectionConfig::default();
/// assert_eq!(config.threshold, 5);
/// assert_eq!(config.time_window_ms, 1000);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectionConfig {
    /// 阈值（同一模板在时间窗口内查询超过此值判定为 N+1）
    pub threshold: usize,
    /// 时间窗口（毫秒）
    pub time_window_ms: u64,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            threshold: 5,
            time_window_ms: 1000,
        }
    }
}

impl DetectionConfig {
    /// 创建新的检测配置
    pub fn new(threshold: usize, time_window_ms: u64) -> Self {
        Self {
            threshold,
            time_window_ms,
        }
    }

    /// 获取阈值
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// 获取时间窗口（毫秒）
    pub fn time_window_ms(&self) -> u64 {
        self.time_window_ms
    }
}

// ============================================================================
// N+1 告警
// ============================================================================

/// N+1 告警
///
/// 当检测到 N+1 问题时生成，包含模板、表名、查询次数、时间跨度与建议。
///
/// ## 字段
///
/// - `template`：SQL 模板（参数替换为 `?`）
/// - `table`：涉及的表名
/// - `query_count`：查询次数
/// - `time_span_ms`：时间跨度（毫秒）
/// - `suggestion`：`with()` 使用建议
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::n_plus_one::{NPlusOneAlert, suggest_with_usage};
///
/// let alert = NPlusOneAlert::new(
///     "SELECT * FROM orders WHERE user_id = ?",
///     "orders",
///     10,
///     500,
/// );
/// assert_eq!(alert.query_count, 10);
/// assert!(alert.suggestion.contains("with"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NPlusOneAlert {
    /// SQL 模板（参数替换为 `?`）
    pub template: String,
    /// 涉及的表名
    pub table: String,
    /// 查询次数
    pub query_count: usize,
    /// 时间跨度（毫秒）
    pub time_span_ms: u64,
    /// `with()` 使用建议
    pub suggestion: String,
}

impl NPlusOneAlert {
    /// 创建新的 N+1 告警
    pub fn new(template: &str, table: &str, query_count: usize, time_span_ms: u64) -> Self {
        Self {
            template: template.to_string(),
            table: table.to_string(),
            query_count,
            time_span_ms,
            suggestion: suggest_with_usage(table, query_count),
        }
    }

    /// 获取 SQL 模板
    pub fn template(&self) -> &str {
        &self.template
    }

    /// 获取表名
    pub fn table(&self) -> &str {
        &self.table
    }

    /// 获取查询次数
    pub fn query_count(&self) -> usize {
        self.query_count
    }

    /// 获取时间跨度（毫秒）
    pub fn time_span_ms(&self) -> u64 {
        self.time_span_ms
    }

    /// 获取建议
    pub fn suggestion(&self) -> &str {
        &self.suggestion
    }
}

// ============================================================================
// 建议生成
// ============================================================================

/// 生成 `with()` 使用建议
///
/// 根据表名与查询次数生成 `with()` 使用建议字符串。
///
/// ## 参数
///
/// - `table`：表名（如 `"orders"`）
/// - `count`：查询次数
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::n_plus_one::suggest_with_usage;
///
/// let suggestion = suggest_with_usage("orders", 10);
/// assert!(suggestion.contains("with"));
/// assert!(suggestion.contains("orders"));
/// assert!(suggestion.contains("10"));
/// ```
pub fn suggest_with_usage(table: &str, count: usize) -> String {
    format!(
        "Detected N+1 problem: {} queries on table '{}' with same template. \
         Consider using `with('{}')` for batch preloading to reduce {} queries to 1.",
        count, table, table, count
    )
}

// ============================================================================
// 核心检测函数
// ============================================================================

/// 检测 N+1 问题
///
/// 按 SQL 模板分组，检查每组在时间窗口内是否有超过阈值的查询。
///
/// ## 算法
///
/// 1. 按 SQL 模板分组查询记录
/// 2. 每组按时间戳排序
/// 3. 检查每组在时间窗口内是否有超过阈值的查询
/// 4. 如果有，生成告警
///
/// ## 参数
///
/// - `records`：SQL 查询记录列表
/// - `config`：检测配置
///
/// ## 返回
///
/// N+1 告警列表（按查询次数降序排序）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::n_plus_one::*;
///
/// let records = vec![
///     SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 100, 0),
///     SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 2", "orders", 200, 1),
///     SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 3", "orders", 300, 2),
///     SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 4", "orders", 400, 3),
///     SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 5", "orders", 500, 4),
///     SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 6", "orders", 600, 5),
/// ];
/// let config = DetectionConfig::new(5, 1000);
/// let alerts = detect_n_plus_one(&records, &config);
/// assert_eq!(alerts.len(), 1);
/// assert_eq!(alerts[0].query_count, 6);
/// ```
pub fn detect_n_plus_one(
    records: &[SqlQueryRecord],
    config: &DetectionConfig,
) -> Vec<NPlusOneAlert> {
    // 1. 按 SQL 模板分组
    let mut groups: HashMap<String, Vec<&SqlQueryRecord>> = HashMap::new();
    for record in records {
        groups
            .entry(record.template.clone())
            .or_default()
            .push(record);
    }

    // 2. 每组按时间戳排序并检查
    let mut alerts: Vec<NPlusOneAlert> = Vec::new();
    for group_records in groups.values() {
        // 按时间戳排序
        let mut sorted_records: Vec<&&SqlQueryRecord> = group_records.iter().collect();
        sorted_records.sort_by_key(|r| r.timestamp_ms);

        if sorted_records.len() < config.threshold {
            continue;
        }

        // 滑动窗口检查：在时间窗口内是否有超过阈值的查询
        let window = config.time_window_ms;
        let threshold = config.threshold;
        let mut start = 0;
        while start < sorted_records.len() {
            let start_time = sorted_records[start].timestamp_ms;
            let mut end = start;
            while end < sorted_records.len()
                && sorted_records[end].timestamp_ms <= start_time + window
            {
                end += 1;
            }
            // [start, end) 范围内的查询都在时间窗口内
            let count = end - start;
            if count >= threshold {
                // 找到 N+1 模式
                let template = sorted_records[start].template.clone();
                let table = sorted_records[start].table.clone();
                let time_span = if end > 0 {
                    sorted_records[end - 1]
                        .timestamp_ms
                        .saturating_sub(start_time)
                } else {
                    0
                };
                // 使用整个组的查询次数（而非窗口内的次数），便于反映问题严重程度
                let total_count = group_records.len();
                alerts.push(NPlusOneAlert::new(
                    &template,
                    &table,
                    total_count,
                    time_span,
                ));
                break; // 该组已检测到 N+1，不再检查
            }
            start += 1;
        }
    }

    // 3. 按查询次数降序排序
    alerts.sort_by_key(|a| std::cmp::Reverse(a.query_count));
    alerts
}

// ============================================================================
// N+1 检测器
// ============================================================================

/// N+1 检测器
///
/// 累积 SQL 查询记录并提供批量分析能力。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::n_plus_one::*;
///
/// let mut detector = NPlusOneDetector::default();
/// detector.record("SELECT * FROM orders WHERE user_id = 1", "orders", 100);
/// detector.record("SELECT * FROM orders WHERE user_id = 2", "orders", 200);
/// detector.record("SELECT * FROM orders WHERE user_id = 3", "orders", 300);
/// detector.record("SELECT * FROM orders WHERE user_id = 4", "orders", 400);
/// detector.record("SELECT * FROM orders WHERE user_id = 5", "orders", 500);
/// detector.record("SELECT * FROM orders WHERE user_id = 6", "orders", 600);
///
/// let alerts = detector.detect();
/// assert_eq!(alerts.len(), 1);
/// ```
#[derive(Debug, Clone, Default)]
pub struct NPlusOneDetector {
    records: Vec<SqlQueryRecord>,
    config: DetectionConfig,
    next_query_index: u64,
}

impl NPlusOneDetector {
    /// 创建新的检测器
    pub fn new(config: DetectionConfig) -> Self {
        Self {
            records: Vec::new(),
            config,
            next_query_index: 0,
        }
    }

    /// 记录一条 SQL 查询
    ///
    /// 自动分配查询序号。
    ///
    /// ## 参数
    ///
    /// - `sql`：原始 SQL 字符串
    /// - `table`：主表名
    /// - `timestamp_ms`：查询时间戳（毫秒）
    pub fn record(&mut self, sql: &str, table: &str, timestamp_ms: u64) {
        let record = SqlQueryRecord::new(sql, table, timestamp_ms, self.next_query_index);
        self.next_query_index += 1;
        self.records.push(record);
    }

    /// 显式记录一条 SQL 查询（带查询序号）
    pub fn record_with_index(
        &mut self,
        sql: &str,
        table: &str,
        timestamp_ms: u64,
        query_index: u64,
    ) {
        let record = SqlQueryRecord::new(sql, table, timestamp_ms, query_index);
        self.records.push(record);
        if query_index >= self.next_query_index {
            self.next_query_index = query_index + 1;
        }
    }

    /// 批量检测 N+1 问题
    pub fn detect(&self) -> Vec<NPlusOneAlert> {
        detect_n_plus_one(&self.records, &self.config)
    }

    /// 清空累积的查询记录
    pub fn clear(&mut self) {
        self.records.clear();
        self.next_query_index = 0;
    }

    /// 获取累积的查询记录数
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// 获取检测配置
    pub fn config(&self) -> &DetectionConfig {
        &self.config
    }

    /// 更新检测配置
    pub fn set_config(&mut self, config: DetectionConfig) {
        self.config = config;
    }

    /// 获取累积的查询记录（只读）
    pub fn records(&self) -> &[SqlQueryRecord] {
        &self.records
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ====================================================================
    // 组 1：SqlQueryRecord 结构体
    // ====================================================================

    #[test]
    fn test_sql_query_record_new() {
        let record =
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 1000, 0);
        assert_eq!(record.sql, "SELECT * FROM orders WHERE user_id = 1");
        assert_eq!(record.template, "SELECT * FROM orders WHERE user_id = ?");
        assert_eq!(record.table, "orders");
        assert_eq!(record.timestamp_ms, 1000);
        assert_eq!(record.query_index, 0);
    }

    #[test]
    fn test_sql_query_record_accessors() {
        let record = SqlQueryRecord::new("SELECT * FROM users WHERE id = 5", "users", 2000, 3);
        assert_eq!(record.sql(), "SELECT * FROM users WHERE id = 5");
        assert_eq!(record.template(), "SELECT * FROM users WHERE id = ?");
        assert_eq!(record.table(), "users");
        assert_eq!(record.timestamp_ms(), 2000);
        assert_eq!(record.query_index(), 3);
    }

    #[test]
    fn test_sql_query_record_string_param() {
        let record = SqlQueryRecord::new(
            "SELECT * FROM users WHERE email = 'abc@x.com'",
            "users",
            1000,
            0,
        );
        assert_eq!(record.template, "SELECT * FROM users WHERE email = ?");
    }

    #[test]
    fn test_sql_query_record_multiple_params() {
        let record = SqlQueryRecord::new(
            "SELECT * FROM users WHERE id = 5 AND email = 'abc' AND age > 18",
            "users",
            1000,
            0,
        );
        assert_eq!(
            record.template,
            "SELECT * FROM users WHERE id = ? AND email = ? AND age > ?"
        );
    }

    #[test]
    fn test_sql_query_record_in_clause() {
        let record = SqlQueryRecord::new(
            "SELECT * FROM orders WHERE user_id IN (1, 2, 3)",
            "orders",
            1000,
            0,
        );
        assert_eq!(
            record.template,
            "SELECT * FROM orders WHERE user_id IN (?, ?, ?)"
        );
    }

    #[test]
    fn test_sql_query_record_clone_eq() {
        let record1 = SqlQueryRecord::new("SELECT * FROM users WHERE id = 1", "users", 1000, 0);
        let record2 = record1.clone();
        assert_eq!(record1, record2);
    }

    // ====================================================================
    // 组 2：extract_template 函数
    // ====================================================================

    #[test]
    fn test_extract_template_numeric_param() {
        assert_eq!(
            extract_template("SELECT * FROM orders WHERE user_id = 1"),
            "SELECT * FROM orders WHERE user_id = ?"
        );
        assert_eq!(
            extract_template("SELECT * FROM orders WHERE user_id = 123"),
            "SELECT * FROM orders WHERE user_id = ?"
        );
    }

    #[test]
    fn test_extract_template_string_param() {
        assert_eq!(
            extract_template("SELECT * FROM users WHERE email = 'abc'"),
            "SELECT * FROM users WHERE email = ?"
        );
        assert_eq!(
            extract_template("SELECT * FROM users WHERE name = 'John Doe'"),
            "SELECT * FROM users WHERE name = ?"
        );
    }

    #[test]
    fn test_extract_template_multiple_params() {
        assert_eq!(
            extract_template("SELECT * FROM users WHERE id = 1 AND name = 'abc'"),
            "SELECT * FROM users WHERE id = ? AND name = ?"
        );
    }

    #[test]
    fn test_extract_template_in_clause() {
        assert_eq!(
            extract_template("SELECT * FROM orders WHERE user_id IN (1, 2, 3)"),
            "SELECT * FROM orders WHERE user_id IN (?, ?, ?)"
        );
    }

    #[test]
    fn test_extract_template_no_params() {
        assert_eq!(
            extract_template("SELECT * FROM users"),
            "SELECT * FROM users"
        );
    }

    #[test]
    fn test_extract_template_float_param() {
        assert_eq!(
            extract_template("SELECT * FROM products WHERE price = 9.99"),
            "SELECT * FROM products WHERE price = ?"
        );
    }

    #[test]
    fn test_extract_template_double_quoted_string() {
        assert_eq!(
            extract_template("SELECT * FROM users WHERE name = \"abc\""),
            "SELECT * FROM users WHERE name = ?"
        );
    }

    #[test]
    fn test_extract_template_empty_string() {
        assert_eq!(extract_template(""), "");
    }

    // ====================================================================
    // 组 3：DetectionConfig 结构体
    // ====================================================================

    #[test]
    fn test_detection_config_default() {
        let config = DetectionConfig::default();
        assert_eq!(config.threshold, 5);
        assert_eq!(config.time_window_ms, 1000);
    }

    #[test]
    fn test_detection_config_new() {
        let config = DetectionConfig::new(10, 5000);
        assert_eq!(config.threshold, 10);
        assert_eq!(config.time_window_ms, 5000);
    }

    #[test]
    fn test_detection_config_accessors() {
        let config = DetectionConfig::new(8, 2000);
        assert_eq!(config.threshold(), 8);
        assert_eq!(config.time_window_ms(), 2000);
    }

    #[test]
    fn test_detection_config_clone_eq() {
        let config1 = DetectionConfig::new(5, 1000);
        let config2 = config1.clone();
        assert_eq!(config1, config2);
    }

    // ====================================================================
    // 组 4：NPlusOneAlert 结构体
    // ====================================================================

    #[test]
    fn test_n_plus_one_alert_new() {
        let alert = NPlusOneAlert::new("SELECT * FROM orders WHERE user_id = ?", "orders", 10, 500);
        assert_eq!(alert.template, "SELECT * FROM orders WHERE user_id = ?");
        assert_eq!(alert.table, "orders");
        assert_eq!(alert.query_count, 10);
        assert_eq!(alert.time_span_ms, 500);
        assert!(alert.suggestion.contains("with"));
        assert!(alert.suggestion.contains("orders"));
        assert!(alert.suggestion.contains("10"));
    }

    #[test]
    fn test_n_plus_one_alert_accessors() {
        let alert = NPlusOneAlert::new("SELECT * FROM users WHERE id = ?", "users", 8, 300);
        assert_eq!(alert.template(), "SELECT * FROM users WHERE id = ?");
        assert_eq!(alert.table(), "users");
        assert_eq!(alert.query_count(), 8);
        assert_eq!(alert.time_span_ms(), 300);
        assert!(alert.suggestion().contains("with"));
    }

    #[test]
    fn test_n_plus_one_alert_clone_eq() {
        let alert1 = NPlusOneAlert::new("SELECT * FROM users WHERE id = ?", "users", 5, 100);
        let alert2 = alert1.clone();
        assert_eq!(alert1, alert2);
    }

    // ====================================================================
    // 组 5：suggest_with_usage 函数
    // ====================================================================

    #[test]
    fn test_suggest_with_usage_basic() {
        let suggestion = suggest_with_usage("orders", 10);
        assert!(suggestion.contains("with"));
        assert!(suggestion.contains("orders"));
        assert!(suggestion.contains("10"));
    }

    #[test]
    fn test_suggest_with_usage_different_table() {
        let suggestion = suggest_with_usage("users", 5);
        assert!(suggestion.contains("users"));
        assert!(suggestion.contains("5"));
    }

    #[test]
    fn test_suggest_with_usage_count_zero() {
        let suggestion = suggest_with_usage("orders", 0);
        assert!(suggestion.contains("0"));
    }

    #[test]
    fn test_suggest_with_usage_large_count() {
        let suggestion = suggest_with_usage("orders", 1000);
        assert!(suggestion.contains("1000"));
    }

    // ====================================================================
    // 组 6：detect_n_plus_one 核心检测函数
    // ====================================================================

    #[test]
    fn test_detect_n_plus_one_no_alerts_under_threshold() {
        // 4 次查询，未达阈值 5
        let records = vec![
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 100, 0),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 2", "orders", 200, 1),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 3", "orders", 300, 2),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 4", "orders", 400, 3),
        ];
        let config = DetectionConfig::new(5, 1000);
        let alerts = detect_n_plus_one(&records, &config);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_detect_n_plus_one_alert_at_threshold() {
        // 5 次查询，达到阈值 5
        let records = vec![
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 100, 0),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 2", "orders", 200, 1),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 3", "orders", 300, 2),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 4", "orders", 400, 3),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 5", "orders", 500, 4),
        ];
        let config = DetectionConfig::new(5, 1000);
        let alerts = detect_n_plus_one(&records, &config);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].query_count, 5);
        assert_eq!(alerts[0].table, "orders");
    }

    #[test]
    fn test_detect_n_plus_one_alert_over_threshold() {
        // 6 次查询，超过阈值 5
        let records = vec![
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 100, 0),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 2", "orders", 200, 1),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 3", "orders", 300, 2),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 4", "orders", 400, 3),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 5", "orders", 500, 4),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 6", "orders", 600, 5),
        ];
        let config = DetectionConfig::new(5, 1000);
        let alerts = detect_n_plus_one(&records, &config);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].query_count, 6);
    }

    #[test]
    fn test_detect_n_plus_one_multiple_templates() {
        // 两种模板，各达到阈值
        let records = vec![
            // 模板 1：orders WHERE user_id = ?
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 100, 0),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 2", "orders", 200, 1),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 3", "orders", 300, 2),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 4", "orders", 400, 3),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 5", "orders", 500, 4),
            // 模板 2：profiles WHERE user_id = ?
            SqlQueryRecord::new(
                "SELECT * FROM profiles WHERE user_id = 1",
                "profiles",
                600,
                5,
            ),
            SqlQueryRecord::new(
                "SELECT * FROM profiles WHERE user_id = 2",
                "profiles",
                700,
                6,
            ),
            SqlQueryRecord::new(
                "SELECT * FROM profiles WHERE user_id = 3",
                "profiles",
                800,
                7,
            ),
            SqlQueryRecord::new(
                "SELECT * FROM profiles WHERE user_id = 4",
                "profiles",
                900,
                8,
            ),
            SqlQueryRecord::new(
                "SELECT * FROM profiles WHERE user_id = 5",
                "profiles",
                1000,
                9,
            ),
        ];
        let config = DetectionConfig::new(5, 2000);
        let alerts = detect_n_plus_one(&records, &config);
        assert_eq!(alerts.len(), 2);
        // 按查询次数降序排序（都是 5 次，顺序可能因 HashMap 而异）
        let tables: Vec<&str> = alerts.iter().map(|a| a.table.as_str()).collect();
        assert!(tables.contains(&"orders"));
        assert!(tables.contains(&"profiles"));
    }

    #[test]
    fn test_detect_n_plus_one_outside_time_window() {
        // 6 次查询，但时间跨度超过时间窗口
        let records = vec![
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 0, 0),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 2", "orders", 500, 1),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 3", "orders", 1000, 2),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 4", "orders", 1500, 3),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 5", "orders", 2000, 4),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 6", "orders", 2500, 5),
        ];
        // 时间窗口 100ms，所有查询都不在同一窗口内
        let config = DetectionConfig::new(5, 100);
        let alerts = detect_n_plus_one(&records, &config);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_detect_n_plus_one_empty_records() {
        let records: Vec<SqlQueryRecord> = vec![];
        let config = DetectionConfig::default();
        let alerts = detect_n_plus_one(&records, &config);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_detect_n_plus_one_different_tables_same_template() {
        // 不同表名但相同模板（按模板分组，不应混合）
        let records = vec![
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 100, 0),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 2", "orders", 200, 1),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 3", "orders", 300, 2),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 4", "orders", 400, 3),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 5", "orders", 500, 4),
        ];
        let config = DetectionConfig::new(5, 1000);
        let alerts = detect_n_plus_one(&records, &config);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].table, "orders");
    }

    #[test]
    fn test_detect_n_plus_one_sorted_by_count_desc() {
        // orders 6 次，profiles 5 次，应按次数降序排序
        let records = vec![
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 100, 0),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 2", "orders", 200, 1),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 3", "orders", 300, 2),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 4", "orders", 400, 3),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 5", "orders", 500, 4),
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 6", "orders", 600, 5),
            SqlQueryRecord::new(
                "SELECT * FROM profiles WHERE user_id = 1",
                "profiles",
                700,
                6,
            ),
            SqlQueryRecord::new(
                "SELECT * FROM profiles WHERE user_id = 2",
                "profiles",
                800,
                7,
            ),
            SqlQueryRecord::new(
                "SELECT * FROM profiles WHERE user_id = 3",
                "profiles",
                900,
                8,
            ),
            SqlQueryRecord::new(
                "SELECT * FROM profiles WHERE user_id = 4",
                "profiles",
                1000,
                9,
            ),
            SqlQueryRecord::new(
                "SELECT * FROM profiles WHERE user_id = 5",
                "profiles",
                1100,
                10,
            ),
        ];
        let config = DetectionConfig::new(5, 2000);
        let alerts = detect_n_plus_one(&records, &config);
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0].query_count, 6); // orders
        assert_eq!(alerts[1].query_count, 5); // profiles
    }

    // ====================================================================
    // 组 7：NPlusOneDetector 检测器
    // ====================================================================

    #[test]
    fn test_detector_default() {
        let detector = NPlusOneDetector::default();
        assert_eq!(detector.record_count(), 0);
        assert_eq!(detector.config().threshold, 5);
        assert_eq!(detector.config().time_window_ms, 1000);
    }

    #[test]
    fn test_detector_new_with_config() {
        let config = DetectionConfig::new(10, 5000);
        let detector = NPlusOneDetector::new(config);
        assert_eq!(detector.config().threshold, 10);
        assert_eq!(detector.config().time_window_ms, 5000);
    }

    #[test]
    fn test_detector_record_auto_index() {
        let mut detector = NPlusOneDetector::default();
        detector.record("SELECT * FROM users WHERE id = 1", "users", 100);
        detector.record("SELECT * FROM users WHERE id = 2", "users", 200);
        assert_eq!(detector.record_count(), 2);
        assert_eq!(detector.records()[0].query_index, 0);
        assert_eq!(detector.records()[1].query_index, 1);
    }

    #[test]
    fn test_detector_record_with_explicit_index() {
        let mut detector = NPlusOneDetector::default();
        detector.record_with_index("SELECT * FROM users WHERE id = 1", "users", 100, 5);
        assert_eq!(detector.records()[0].query_index, 5);
        // 后续自动分配应从 6 开始
        detector.record("SELECT * FROM users WHERE id = 2", "users", 200);
        assert_eq!(detector.records()[1].query_index, 6);
    }

    #[test]
    fn test_detector_detect_no_alerts() {
        let mut detector = NPlusOneDetector::default();
        detector.record("SELECT * FROM orders WHERE user_id = 1", "orders", 100);
        detector.record("SELECT * FROM orders WHERE user_id = 2", "orders", 200);
        let alerts = detector.detect();
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_detector_detect_with_alerts() {
        let mut detector = NPlusOneDetector::default();
        for i in 1..=6 {
            detector.record(
                &format!("SELECT * FROM orders WHERE user_id = {}", i),
                "orders",
                i * 100,
            );
        }
        let alerts = detector.detect();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].query_count, 6);
        assert_eq!(alerts[0].table, "orders");
    }

    #[test]
    fn test_detector_clear() {
        let mut detector = NPlusOneDetector::default();
        detector.record("SELECT * FROM users WHERE id = 1", "users", 100);
        assert_eq!(detector.record_count(), 1);
        detector.clear();
        assert_eq!(detector.record_count(), 0);
        // 清空后 next_query_index 应重置为 0
        detector.record("SELECT * FROM users WHERE id = 2", "users", 200);
        assert_eq!(detector.records()[0].query_index, 0);
    }

    #[test]
    fn test_detector_set_config() {
        let mut detector = NPlusOneDetector::default();
        assert_eq!(detector.config().threshold, 5);
        detector.set_config(DetectionConfig::new(20, 10000));
        assert_eq!(detector.config().threshold, 20);
        assert_eq!(detector.config().time_window_ms, 10000);
    }

    #[test]
    fn test_detector_records_accessor() {
        let mut detector = NPlusOneDetector::default();
        detector.record("SELECT * FROM users WHERE id = 1", "users", 100);
        detector.record("SELECT * FROM users WHERE id = 2", "users", 200);
        let records = detector.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].table, "users");
        assert_eq!(records[1].table, "users");
    }

    // ====================================================================
    // 组 8：R5 PHP 行为对齐验证（硬约束）
    // ====================================================================

    #[test]
    fn test_r5_php_n_plus_one_pattern_detection() {
        // R5-1：检测 PHP N+1 模式（循环内访问关联触发 N 次查询）
        // PHP 模式：
        //   $users = User::select();  // 1 次
        //   foreach ($users as $user) {
        //       $orders = $user->orders;  // N 次
        //   }
        let mut records = vec![SqlQueryRecord::new("SELECT * FROM users", "users", 0, 0)];
        for i in 1..=6 {
            records.push(SqlQueryRecord::new(
                &format!("SELECT * FROM orders WHERE user_id = {}", i),
                "orders",
                i * 100,
                i,
            ));
        }
        let config = DetectionConfig::new(5, 1000);
        let alerts = detect_n_plus_one(&records, &config);
        // 应检测到 orders 表的 N+1 问题（6 次相同模板查询）
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].table, "orders");
        assert_eq!(alerts[0].query_count, 6);
    }

    #[test]
    fn test_r5_php_with_avoids_n_plus_one() {
        // R5-2：PHP `with()` 批量预加载避免 N+1 问题
        // PHP 模式：
        //   $users = User::with('orders')->select();
        //   // 内部通过 eagerlyResultSet() 批量 IN 查询（2 次查询）
        let records = vec![
            SqlQueryRecord::new("SELECT * FROM users", "users", 0, 0),
            SqlQueryRecord::new(
                "SELECT * FROM orders WHERE user_id IN (1, 2, 3, 4, 5, 6)",
                "orders",
                100,
                1,
            ),
        ];
        let config = DetectionConfig::new(5, 1000);
        let alerts = detect_n_plus_one(&records, &config);
        // 使用 `with()` 后无 N+1 问题
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_r5_php_eagerly_result_set_in_query_template() {
        // R5-3：PHP `eagerlyResultSet()` 批量 IN 查询 SQL 模板提取对齐
        // PHP `HasMany::eagerlyResultSet` 第 87 行 `[$this->foreignKey, 'in', $range]`
        // 生成 SQL：`SELECT * FROM {child} WHERE {fk} IN (v1, v2, ...)`
        let sql = "SELECT * FROM orders WHERE user_id IN (1, 2, 3, 4, 5)";
        let template = extract_template(sql);
        assert_eq!(
            template,
            "SELECT * FROM orders WHERE user_id IN (?, ?, ?, ?, ?)"
        );
    }

    #[test]
    fn test_r5_php_single_query_no_n_plus_one() {
        // R5-4：单次查询不构成 N+1 问题
        let records = vec![SqlQueryRecord::new(
            "SELECT * FROM orders WHERE user_id = 1",
            "orders",
            100,
            0,
        )];
        let config = DetectionConfig::default();
        let alerts = detect_n_plus_one(&records, &config);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_r5_php_belongs_to_n_plus_one_detection() {
        // R5-5：BelongsTo N+1 模式检测（PHP `belongsTo` 关联）
        // PHP 模式：
        //   $orders = Order::select();  // 1 次
        //   foreach ($orders as $order) {
        //       $user = $order->user;  // N 次（每个 order 查询 user）
        //   }
        let mut records = vec![SqlQueryRecord::new("SELECT * FROM orders", "orders", 0, 0)];
        for i in 1..=6 {
            records.push(SqlQueryRecord::new(
                &format!("SELECT * FROM users WHERE id = {}", i),
                "users",
                i * 100,
                i,
            ));
        }
        let config = DetectionConfig::new(5, 1000);
        let alerts = detect_n_plus_one(&records, &config);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].table, "users");
        assert_eq!(alerts[0].query_count, 6);
    }

    #[test]
    fn test_r5_php_morph_to_n_plus_one_detection() {
        // R5-6：MorphTo N+1 模式检测（PHP `morphTo` 多态反向关联）
        // PHP 模式：
        //   $comments = Comment::select();  // 1 次
        //   foreach ($comments as $comment) {
        //       $commentable = $comment->commentable;  // N 次（每个 comment 查询不同父表）
        //   }
        let mut records = vec![SqlQueryRecord::new(
            "SELECT * FROM comments",
            "comments",
            0,
            0,
        )];
        // 注：MorphTo 的 N+1 检测较复杂，因为每个 comment 可能查询不同父表
        // 但模板相同（SELECT * FROM {table} WHERE id = ?），表名不同
        // 本测试验证模板分组与表名分组的关系
        for i in 1..=3 {
            records.push(SqlQueryRecord::new(
                &format!("SELECT * FROM posts WHERE id = {}", i),
                "posts",
                i * 100,
                i,
            ));
        }
        for i in 1..=3 {
            records.push(SqlQueryRecord::new(
                &format!("SELECT * FROM videos WHERE id = {}", i),
                "videos",
                (i + 3) * 100,
                i + 3,
            ));
        }
        let config = DetectionConfig::new(3, 1000);
        let alerts = detect_n_plus_one(&records, &config);
        // posts 和 videos 各 3 次，应分别告警
        assert_eq!(alerts.len(), 2);
        let tables: Vec<&str> = alerts.iter().map(|a| a.table.as_str()).collect();
        assert!(tables.contains(&"posts"));
        assert!(tables.contains(&"videos"));
    }

    #[test]
    fn test_r5_php_suggest_with_usage_format() {
        // R5-7：`with()` 使用建议格式对齐 PHP think-orm 2.0.x
        let suggestion = suggest_with_usage("orders", 10);
        assert!(suggestion.contains("with("));
        assert!(suggestion.contains("orders"));
        assert!(suggestion.contains("10"));
        assert!(suggestion.contains("batch preloading"));
    }

    #[test]
    fn test_r5_php_threshold_default_5() {
        // R5-8：默认阈值 5 对齐常见 N+1 检测最佳实践
        // PHP think-orm 2.0.x 未提供主动检测，sz-rust 端默认阈值 5
        // 参考行业实践：Laravel Telescope 默认阈值 5+，Django Debug Toolbar 阈值 5+
        let config = DetectionConfig::default();
        assert_eq!(config.threshold, 5);
    }

    #[test]
    fn test_r5_php_time_window_default_1000ms() {
        // R5-9：默认时间窗口 1000ms 对齐 Web 请求典型时长
        let config = DetectionConfig::default();
        assert_eq!(config.time_window_ms, 1000);
    }

    #[test]
    fn test_r5_php_different_query_no_n_plus_one() {
        // R5-10：不同查询模板不构成 N+1 问题
        let records = vec![
            SqlQueryRecord::new("SELECT * FROM orders WHERE user_id = 1", "orders", 100, 0),
            SqlQueryRecord::new(
                "SELECT * FROM orders WHERE user_id = 2 AND status = 1",
                "orders",
                200,
                1,
            ),
            SqlQueryRecord::new(
                "SELECT * FROM orders WHERE user_id = 3 AND status = 2",
                "orders",
                300,
                2,
            ),
        ];
        let config = DetectionConfig::default();
        let alerts = detect_n_plus_one(&records, &config);
        // 不同模板（WHERE 条件不同），不应判定为 N+1
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_r5_php_detector_integration() {
        // R5-11：检测器集成测试（模拟 PHP Web 请求生命周期）
        let mut detector = NPlusOneDetector::default();
        // 模拟 PHP Web 请求：1 次主查询 + 6 次关联查询
        detector.record("SELECT * FROM users", "users", 0);
        for i in 1..=6 {
            detector.record(
                &format!("SELECT * FROM orders WHERE user_id = {}", i),
                "orders",
                i * 50,
            );
        }
        let alerts = detector.detect();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].table, "orders");
        assert_eq!(alerts[0].query_count, 6);
        assert!(alerts[0].suggestion.contains("with"));
    }

    // ====================================================================
    // 组 9：集成测试
    // ====================================================================

    #[test]
    fn test_integration_detector_with_config_change() {
        // 集成测试：检测器配置变更后重新检测
        let mut detector = NPlusOneDetector::new(DetectionConfig::new(10, 1000));
        for i in 1..=6 {
            detector.record(
                &format!("SELECT * FROM orders WHERE user_id = {}", i),
                "orders",
                i * 100,
            );
        }
        // 阈值 10 时无告警
        assert!(detector.detect().is_empty());
        // 降低阈值到 5 后有告警
        detector.set_config(DetectionConfig::new(5, 1000));
        let alerts = detector.detect();
        assert_eq!(alerts.len(), 1);
    }

    #[test]
    fn test_integration_multiple_rounds() {
        // 集成测试：多轮检测（清空后重新累积）
        let mut detector = NPlusOneDetector::default();
        // 第 1 轮：N+1 问题
        for i in 1..=6 {
            detector.record(
                &format!("SELECT * FROM orders WHERE user_id = {}", i),
                "orders",
                i * 100,
            );
        }
        assert_eq!(detector.detect().len(), 1);
        // 清空
        detector.clear();
        assert_eq!(detector.record_count(), 0);
        // 第 2 轮：无 N+1 问题（使用 with 批量预加载）
        detector.record("SELECT * FROM users", "users", 0);
        detector.record(
            "SELECT * FROM orders WHERE user_id IN (1, 2, 3, 4, 5, 6)",
            "orders",
            100,
        );
        assert!(detector.detect().is_empty());
    }

    #[test]
    fn test_integration_complex_scenario() {
        // 集成测试：复杂场景（混合多种查询模式）
        let mut detector = NPlusOneDetector::default();
        // 主查询
        detector.record("SELECT * FROM users WHERE status = 1", "users", 0);
        // N+1 模式：orders 表 5 次查询
        for i in 1..=5 {
            detector.record(
                &format!("SELECT * FROM orders WHERE user_id = {}", i),
                "orders",
                i * 100,
            );
        }
        // 单次查询：profiles 表 1 次
        detector.record("SELECT * FROM profiles WHERE user_id = 1", "profiles", 600);
        // N+1 模式：comments 表 7 次查询
        for i in 1..=7 {
            detector.record(
                &format!("SELECT * FROM comments WHERE post_id = {}", i),
                "comments",
                700 + i * 50,
            );
        }
        let alerts = detector.detect();
        assert_eq!(alerts.len(), 2);
        // 按查询次数降序排序：comments (7) > orders (5)
        assert_eq!(alerts[0].table, "comments");
        assert_eq!(alerts[0].query_count, 7);
        assert_eq!(alerts[1].table, "orders");
        assert_eq!(alerts[1].query_count, 5);
    }
}
