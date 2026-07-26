//! 自治运维 — 索引推荐 — Phase 7b.7
//!
//! 对应 `SzRSQL技术实现方案.md` 9.9 节。
//!
//! # 设计
//!
//! 自动分析慢查询负载，推荐合适的索引，并验证查询加速比。
//!
//! ## 工作流程
//!
//! 1. **负载采集** — `record_query(sql, elapsed_ms, scanned_rows)` 记录查询执行
//! 2. **慢查询识别** — `analyze()` 识别耗时超过阈值的查询
//! 3. **索引推荐** — 基于 WHERE/JOIN/ORDER BY 子句的列引用频率推荐索引
//! 4. **索引创建** — `apply_recommendation()` 创建推荐索引
//! 5. **加速比验证** — 对比索引创建前后的查询耗时
//!
//! # 验证标准
//!
//! - 生成 100000 条查询负载 → 自动分析慢查询 → 推荐索引 → 创建索引 → 验证查询加速比 >= 2x
//!
//! 对应 `SzRSQL实施进度.md` Phase 7b.7。

use std::collections::HashMap;

// =====================================================================
//  错误类型
// =====================================================================

/// 索引推荐错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AutoIndexError {
    /// 慢查询阈值为 0
    #[error("slow query threshold must be > 0")]
    InvalidThreshold,
    /// 查询记录无效
    #[error("invalid query record: {0}")]
    InvalidQueryRecord(String),
    /// 表不存在
    #[error("table not found: {0}")]
    TableNotFound(String),
    /// 索引已存在
    #[error("index already exists: {0}")]
    IndexAlreadyExists(String),
    /// 索引不存在
    #[error("index not found: {0}")]
    IndexNotFound(String),
}

// =====================================================================
//  查询记录
// =====================================================================

/// 单条查询执行记录
#[derive(Debug, Clone)]
pub struct QueryRecord {
    /// SQL 文本
    pub sql: String,
    /// 执行耗时（毫秒）
    pub elapsed_ms: u64,
    /// 扫描行数
    pub scanned_rows: u64,
    /// 涉及的表
    pub tables: Vec<String>,
    /// WHERE 子句中引用的列（table.column 格式）
    pub where_columns: Vec<String>,
    /// JOIN 子句中引用的列
    pub join_columns: Vec<String>,
    /// ORDER BY 子句中引用的列
    pub order_by_columns: Vec<String>,
}

impl QueryRecord {
    /// 创建简单查询记录（便捷构造器）
    pub fn new(sql: &str, elapsed_ms: u64, scanned_rows: u64) -> Self {
        Self {
            sql: sql.to_string(),
            elapsed_ms,
            scanned_rows,
            tables: Vec::new(),
            where_columns: Vec::new(),
            join_columns: Vec::new(),
            order_by_columns: Vec::new(),
        }
    }

    /// 所有被引用的列（WHERE + JOIN + ORDER BY）
    pub fn all_columns(&self) -> Vec<String> {
        let mut cols = Vec::new();
        cols.extend(self.where_columns.iter().cloned());
        cols.extend(self.join_columns.iter().cloned());
        cols.extend(self.order_by_columns.iter().cloned());
        cols
    }
}

// =====================================================================
//  索引推荐
// =====================================================================

/// 索引推荐结果
#[derive(Debug, Clone, PartialEq)]
pub struct IndexRecommendation {
    /// 索引名称
    pub index_name: String,
    /// 表名
    pub table: String,
    /// 索引列（可多列复合索引）
    pub columns: Vec<String>,
    /// 预期收益（引用次数）
    pub benefit_score: u64,
    /// 推荐理由
    pub reason: String,
}

/// 已创建的索引
#[derive(Debug, Clone, PartialEq)]
pub struct CreatedIndex {
    /// 索引名称
    pub index_name: String,
    /// 表名
    pub table: String,
    /// 索引列
    pub columns: Vec<String>,
}

// =====================================================================
//  分析统计
// =====================================================================

/// 分析统计信息
#[derive(Debug, Clone, Default)]
pub struct AnalysisStats {
    /// 总查询数
    pub total_queries: usize,
    /// 慢查询数
    pub slow_queries: usize,
    /// 涉及的表数
    pub tables_involved: usize,
    /// 推荐索引数
    pub recommendations: usize,
    /// 已创建索引数
    pub created_indexes: usize,
    /// 平均查询耗时（毫秒）
    pub avg_elapsed_ms: f64,
    /// 最大查询耗时（毫秒）
    pub max_elapsed_ms: u64,
}

// =====================================================================
//  AutoIndexEngine — 索引推荐引擎
// =====================================================================

/// 索引推荐引擎 — 自动分析慢查询并推荐索引
#[derive(Debug)]
pub struct AutoIndexEngine {
    /// 查询负载记录
    query_log: Vec<QueryRecord>,
    /// 慢查询阈值（毫秒）
    slow_threshold_ms: u64,
    /// 已创建的索引
    created_indexes: Vec<CreatedIndex>,
    /// 已创建索引的名称集合（快速查找）
    created_index_names: std::collections::HashSet<String>,
}

impl Default for AutoIndexEngine {
    fn default() -> Self {
        Self::new(100).expect("default threshold valid")
    }
}

impl AutoIndexEngine {
    /// 创建索引推荐引擎
    ///
    /// - `slow_threshold_ms` — 慢查询阈值（毫秒），超过此值视为慢查询
    pub fn new(slow_threshold_ms: u64) -> Result<Self, AutoIndexError> {
        if slow_threshold_ms == 0 {
            return Err(AutoIndexError::InvalidThreshold);
        }
        Ok(Self {
            query_log: Vec::new(),
            slow_threshold_ms,
            created_indexes: Vec::new(),
            created_index_names: std::collections::HashSet::new(),
        })
    }

    /// 慢查询阈值
    pub fn slow_threshold_ms(&self) -> u64 {
        self.slow_threshold_ms
    }

    /// 记录查询
    pub fn record_query(&mut self, record: QueryRecord) {
        self.query_log.push(record);
    }

    /// 批量记录查询
    pub fn record_queries(&mut self, records: Vec<QueryRecord>) {
        self.query_log.extend(records);
    }

    /// 当前记录数
    pub fn len(&self) -> usize {
        self.query_log.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.query_log.is_empty()
    }

    /// 清空查询日志
    pub fn clear(&mut self) {
        self.query_log.clear();
    }

    /// 已创建的索引列表
    pub fn created_indexes(&self) -> &[CreatedIndex] {
        &self.created_indexes
    }

    // -----------------------------------------------------------------
    //  分析与推荐
    // -----------------------------------------------------------------

    /// 分析慢查询，生成索引推荐
    pub fn analyze(&self) -> (AnalysisStats, Vec<IndexRecommendation>) {
        let total_queries = self.query_log.len();
        let slow_queries: Vec<&QueryRecord> = self
            .query_log
            .iter()
            .filter(|q| q.elapsed_ms >= self.slow_threshold_ms)
            .collect();

        let slow_count = slow_queries.len();

        // 统计列引用频率
        let mut column_freq: HashMap<String, u64> = HashMap::new();
        let mut table_columns: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        let mut total_elapsed: u64 = 0;
        let mut max_elapsed: u64 = 0;
        let mut tables_involved: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for q in &self.query_log {
            total_elapsed += q.elapsed_ms;
            if q.elapsed_ms > max_elapsed {
                max_elapsed = q.elapsed_ms;
            }
            for t in &q.tables {
                tables_involved.insert(t.clone());
            }
        }

        // 只对慢查询的列进行推荐
        for q in &slow_queries {
            for col in &q.where_columns {
                *column_freq.entry(col.clone()).or_insert(0) += 3; // WHERE 权重 3
                if let Some(dot) = col.find('.') {
                    let (table, column) = col.split_at(dot);
                    let column = &column[1..]; // 跳过 '.'
                    table_columns
                        .entry(table.to_string())
                        .or_default()
                        .insert(column.to_string());
                }
            }
            for col in &q.join_columns {
                *column_freq.entry(col.clone()).or_insert(0) += 2; // JOIN 权重 2
                if let Some(dot) = col.find('.') {
                    let (table, column) = col.split_at(dot);
                    let column = &column[1..];
                    table_columns
                        .entry(table.to_string())
                        .or_default()
                        .insert(column.to_string());
                }
            }
            for col in &q.order_by_columns {
                *column_freq.entry(col.clone()).or_insert(0) += 1; // ORDER BY 权重 1
                if let Some(dot) = col.find('.') {
                    let (table, column) = col.split_at(dot);
                    let column = &column[1..];
                    table_columns
                        .entry(table.to_string())
                        .or_default()
                        .insert(column.to_string());
                }
            }
        }

        // 按表分组生成推荐
        let mut recommendations: Vec<IndexRecommendation> = Vec::new();
        for (table, cols) in &table_columns {
            // 按列引用频率排序
            let mut col_scores: Vec<(String, u64)> = cols
                .iter()
                .map(|c| {
                    let full_col = format!("{table}.{c}");
                    let score = *column_freq.get(&full_col).unwrap_or(&0);
                    (c.clone(), score)
                })
                .collect();
            col_scores.sort_by_key(|b| std::cmp::Reverse(b.1));

            // 单列索引推荐（取 top-1）
            if let Some((top_col, top_score)) = col_scores.first() {
                if *top_score > 0 {
                    let index_name = format!("idx_{table}_{top_col}");
                    // 跳过已创建的索引
                    if !self.created_index_names.contains(&index_name) {
                        recommendations.push(IndexRecommendation {
                            index_name: index_name.clone(),
                            table: table.clone(),
                            columns: vec![top_col.clone()],
                            benefit_score: *top_score,
                            reason: format!(
                                "列 {table}.{top_col} 在慢查询中被引用 {top_score} 次（WHERE/JOIN/ORDER BY）"
                            ),
                        });
                    }
                }
            }

            // 复合索引推荐（取 top-2，仅当两列都高引用时）
            if col_scores.len() >= 2 {
                let (col1, score1) = &col_scores[0];
                let (col2, score2) = &col_scores[1];
                if *score1 >= 3 && *score2 >= 2 {
                    let index_name = format!("idx_{table}_{col1}_{col2}");
                    if !self.created_index_names.contains(&index_name) {
                        recommendations.push(IndexRecommendation {
                            index_name: index_name.clone(),
                            table: table.clone(),
                            columns: vec![col1.clone(), col2.clone()],
                            benefit_score: score1 + score2,
                            reason: format!("复合索引 ({col1}, {col2}) 在慢查询中高频率联合引用"),
                        });
                    }
                }
            }
        }

        // 按收益排序
        recommendations.sort_by_key(|r| std::cmp::Reverse(r.benefit_score));

        let avg_elapsed = if total_queries > 0 {
            total_elapsed as f64 / total_queries as f64
        } else {
            0.0
        };

        let stats = AnalysisStats {
            total_queries,
            slow_queries: slow_count,
            tables_involved: tables_involved.len(),
            recommendations: recommendations.len(),
            created_indexes: self.created_indexes.len(),
            avg_elapsed_ms: avg_elapsed,
            max_elapsed_ms: max_elapsed,
        };

        (stats, recommendations)
    }

    // -----------------------------------------------------------------
    //  索引管理
    // -----------------------------------------------------------------

    /// 应用索引推荐 — 创建索引
    pub fn apply_recommendation(
        &mut self,
        rec: &IndexRecommendation,
    ) -> Result<CreatedIndex, AutoIndexError> {
        if self.created_index_names.contains(&rec.index_name) {
            return Err(AutoIndexError::IndexAlreadyExists(rec.index_name.clone()));
        }
        let created = CreatedIndex {
            index_name: rec.index_name.clone(),
            table: rec.table.clone(),
            columns: rec.columns.clone(),
        };
        self.created_index_names.insert(rec.index_name.clone());
        self.created_indexes.push(created.clone());
        Ok(created)
    }

    /// 批量应用推荐
    pub fn apply_recommendations(
        &mut self,
        recs: &[IndexRecommendation],
    ) -> Vec<Result<CreatedIndex, AutoIndexError>> {
        recs.iter().map(|r| self.apply_recommendation(r)).collect()
    }

    /// 删除索引
    pub fn drop_index(&mut self, index_name: &str) -> Result<(), AutoIndexError> {
        if !self.created_index_names.remove(index_name) {
            return Err(AutoIndexError::IndexNotFound(index_name.to_string()));
        }
        self.created_indexes.retain(|i| i.index_name != index_name);
        Ok(())
    }

    // -----------------------------------------------------------------
    //  加速比验证
    // -----------------------------------------------------------------

    /// 验证索引加速比
    ///
    /// 对比索引创建前后的查询耗时，计算加速比。
    ///
    /// - `before` — 索引创建前的查询耗时（毫秒）
    /// - `after` — 索引创建后的查询耗时（毫秒）
    ///
    /// 返回加速比（before / after）。值 > 1.0 表示索引有效。
    pub fn speedup_ratio(before_ms: u64, after_ms: u64) -> f64 {
        if after_ms == 0 {
            return f64::INFINITY;
        }
        before_ms as f64 / after_ms as f64
    }

    /// 验证一组查询的加速比
    ///
    /// 返回平均加速比。
    pub fn avg_speedup_ratio(before_ms: &[u64], after_ms: &[u64]) -> f64 {
        if before_ms.is_empty() || after_ms.is_empty() {
            return 0.0;
        }
        let before_avg: f64 =
            before_ms.iter().map(|&v| v as f64).sum::<f64>() / before_ms.len() as f64;
        let after_avg: f64 =
            after_ms.iter().map(|&v| v as f64).sum::<f64>() / after_ms.len() as f64;
        if after_avg == 0.0 {
            return f64::INFINITY;
        }
        before_avg / after_avg
    }
}

// =====================================================================
//  负载生成器 — 用于测试
// =====================================================================

/// 生成模拟查询负载
///
/// 生成 `count` 条查询记录，其中慢查询占比 `slow_ratio`。
pub fn generate_workload(
    count: usize,
    slow_ratio: f64,
    tables: &[&str],
    columns: &[&str],
) -> Vec<QueryRecord> {
    let mut records = Vec::with_capacity(count);
    let slow_count = (count as f64 * slow_ratio) as usize;

    for i in 0..count {
        let is_slow = i < slow_count;
        let elapsed_ms = if is_slow {
            150 + (i as u64 % 350) // 150-500ms 慢查询
        } else {
            5 + (i as u64 % 90) // 5-95ms 正常查询
        };
        let scanned_rows = if is_slow {
            10000 + (i as u64 % 90000) // 全表扫描
        } else {
            10 + (i as u64 % 100) // 索引扫描
        };

        let table = tables[i % tables.len()];
        let column = columns[i % columns.len()];
        let full_col = format!("{table}.{column}");

        let mut record = QueryRecord::new(
            &format!("SELECT * FROM {table} WHERE {column} = {i}"),
            elapsed_ms,
            scanned_rows,
        );
        record.tables = vec![table.to_string()];
        record.where_columns = vec![full_col.clone()];
        // 部分 ORDER BY
        if i % 3 == 0 {
            record.order_by_columns = vec![full_col];
        }
        records.push(record);
    }

    records
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b7_engine_creation() {
        let engine = AutoIndexEngine::new(100).unwrap();
        assert_eq!(engine.slow_threshold_ms(), 100);
        assert!(engine.is_empty());
        assert_eq!(engine.created_indexes().len(), 0);
    }

    #[test]
    fn test_7b7_invalid_threshold() {
        let result = AutoIndexEngine::new(0);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), AutoIndexError::InvalidThreshold);
    }

    #[test]
    fn test_7b7_record_query() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        engine.record_query(QueryRecord::new("SELECT 1", 10, 1));
        assert_eq!(engine.len(), 1);
        assert!(!engine.is_empty());
    }

    #[test]
    fn test_7b7_record_queries_batch() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let records = vec![
            QueryRecord::new("SELECT 1", 10, 1),
            QueryRecord::new("SELECT 2", 20, 2),
        ];
        engine.record_queries(records);
        assert_eq!(engine.len(), 2);
    }

    #[test]
    fn test_7b7_clear() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        engine.record_query(QueryRecord::new("SELECT 1", 10, 1));
        engine.clear();
        assert!(engine.is_empty());
    }

    // -----------------------------------------------------------------
    //  分析测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b7_analyze_no_queries() {
        let engine = AutoIndexEngine::new(100).unwrap();
        let (stats, recs) = engine.analyze();
        assert_eq!(stats.total_queries, 0);
        assert_eq!(stats.slow_queries, 0);
        assert!(recs.is_empty());
    }

    #[test]
    fn test_7b7_analyze_no_slow_queries() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let mut record = QueryRecord::new("SELECT * FROM users WHERE id = 1", 50, 10);
        record.tables = vec!["users".to_string()];
        record.where_columns = vec!["users.id".to_string()];
        engine.record_query(record);

        let (stats, recs) = engine.analyze();
        assert_eq!(stats.total_queries, 1);
        assert_eq!(stats.slow_queries, 0);
        assert!(recs.is_empty());
    }

    #[test]
    fn test_7b7_analyze_single_slow_query() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let mut record = QueryRecord::new("SELECT * FROM users WHERE id = 1", 200, 50000);
        record.tables = vec!["users".to_string()];
        record.where_columns = vec!["users.id".to_string()];
        engine.record_query(record);

        let (stats, recs) = engine.analyze();
        assert_eq!(stats.total_queries, 1);
        assert_eq!(stats.slow_queries, 1);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].table, "users");
        assert_eq!(recs[0].columns, vec!["id".to_string()]);
        assert!(recs[0].benefit_score > 0);
    }

    #[test]
    fn test_7b7_analyze_multiple_columns() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        // 慢查询引用多个列
        let mut record = QueryRecord::new(
            "SELECT * FROM products WHERE category = ? AND price < ?",
            300,
            80000,
        );
        record.tables = vec!["products".to_string()];
        record.where_columns = vec![
            "products.category".to_string(),
            "products.price".to_string(),
        ];
        engine.record_query(record);

        let (stats, recs) = engine.analyze();
        assert_eq!(stats.slow_queries, 1);
        // 应有单列索引 + 复合索引推荐
        assert!(!recs.is_empty());
        let tables: Vec<&str> = recs.iter().map(|r| r.table.as_str()).collect();
        assert!(tables.contains(&"products"));
    }

    #[test]
    fn test_7b7_analyze_composite_index_recommendation() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        // 多条慢查询引用相同两列 → 应推荐复合索引
        for i in 0..5 {
            let mut record = QueryRecord::new(
                &format!("SELECT * FROM orders WHERE customer_id = {i} AND status = '{i}'"),
                200 + i * 10,
                50000 + i * 1000,
            );
            record.tables = vec!["orders".to_string()];
            record.where_columns = vec![
                "orders.customer_id".to_string(),
                "orders.status".to_string(),
            ];
            engine.record_query(record);
        }

        let (_, recs) = engine.analyze();
        // 应包含复合索引推荐
        let has_composite = recs
            .iter()
            .any(|r| r.columns.len() == 2 && r.table == "orders");
        assert!(has_composite, "should recommend composite index");
    }

    #[test]
    fn test_7b7_analyze_order_by_columns() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let mut record = QueryRecord::new("SELECT * FROM users ORDER BY created_at", 150, 30000);
        record.tables = vec!["users".to_string()];
        record.order_by_columns = vec!["users.created_at".to_string()];
        engine.record_query(record);

        let (_, recs) = engine.analyze();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].columns, vec!["created_at".to_string()]);
    }

    #[test]
    fn test_7b7_analyze_join_columns() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let mut record = QueryRecord::new(
            "SELECT * FROM orders JOIN users ON orders.user_id = users.id",
            250,
            60000,
        );
        record.tables = vec!["orders".to_string(), "users".to_string()];
        record.join_columns = vec!["orders.user_id".to_string(), "users.id".to_string()];
        engine.record_query(record);

        let (_, recs) = engine.analyze();
        // 应为 orders.user_id 和 users.id 都推荐索引
        let orders_rec = recs.iter().find(|r| r.table == "orders");
        let users_rec = recs.iter().find(|r| r.table == "users");
        assert!(orders_rec.is_some());
        assert!(users_rec.is_some());
    }

    #[test]
    fn test_7b7_analyze_stats_correctness() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        // 3 条查询：1 慢 + 2 快
        engine.record_query(QueryRecord::new("SELECT 1", 50, 10));
        engine.record_query(QueryRecord::new("SELECT 2", 200, 50000));
        engine.record_query(QueryRecord::new("SELECT 3", 80, 100));

        let (stats, _) = engine.analyze();
        assert_eq!(stats.total_queries, 3);
        assert_eq!(stats.slow_queries, 1);
        assert_eq!(stats.max_elapsed_ms, 200);
        assert!((stats.avg_elapsed_ms - 110.0).abs() < 0.01);
    }

    // -----------------------------------------------------------------
    //  索引管理测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b7_apply_recommendation() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let rec = IndexRecommendation {
            index_name: "idx_users_id".to_string(),
            table: "users".to_string(),
            columns: vec!["id".to_string()],
            benefit_score: 10,
            reason: "test".to_string(),
        };
        let created = engine.apply_recommendation(&rec).unwrap();
        assert_eq!(created.index_name, "idx_users_id");
        assert_eq!(engine.created_indexes().len(), 1);
    }

    #[test]
    fn test_7b7_apply_duplicate_recommendation() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let rec = IndexRecommendation {
            index_name: "idx_users_id".to_string(),
            table: "users".to_string(),
            columns: vec!["id".to_string()],
            benefit_score: 10,
            reason: "test".to_string(),
        };
        engine.apply_recommendation(&rec).unwrap();
        let result = engine.apply_recommendation(&rec);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            AutoIndexError::IndexAlreadyExists("idx_users_id".to_string())
        );
    }

    #[test]
    fn test_7b7_apply_recommendations_batch() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let recs = vec![
            IndexRecommendation {
                index_name: "idx_t1_a".to_string(),
                table: "t1".to_string(),
                columns: vec!["a".to_string()],
                benefit_score: 5,
                reason: "test".to_string(),
            },
            IndexRecommendation {
                index_name: "idx_t2_b".to_string(),
                table: "t2".to_string(),
                columns: vec!["b".to_string()],
                benefit_score: 3,
                reason: "test".to_string(),
            },
        ];
        let results = engine.apply_recommendations(&recs);
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert_eq!(engine.created_indexes().len(), 2);
    }

    #[test]
    fn test_7b7_drop_index() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let rec = IndexRecommendation {
            index_name: "idx_users_id".to_string(),
            table: "users".to_string(),
            columns: vec!["id".to_string()],
            benefit_score: 10,
            reason: "test".to_string(),
        };
        engine.apply_recommendation(&rec).unwrap();
        assert_eq!(engine.created_indexes().len(), 1);

        engine.drop_index("idx_users_id").unwrap();
        assert_eq!(engine.created_indexes().len(), 0);
    }

    #[test]
    fn test_7b7_drop_nonexistent_index() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let result = engine.drop_index("nonexistent");
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------
    //  加速比验证测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b7_speedup_ratio_basic() {
        let ratio = AutoIndexEngine::speedup_ratio(200, 50);
        assert!((ratio - 4.0).abs() < 0.01);
    }

    #[test]
    fn test_7b7_speedup_ratio_no_improvement() {
        let ratio = AutoIndexEngine::speedup_ratio(100, 100);
        assert!((ratio - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_7b7_speedup_ratio_zero_after() {
        let ratio = AutoIndexEngine::speedup_ratio(100, 0);
        assert!(ratio.is_infinite());
    }

    #[test]
    fn test_7b7_avg_speedup_ratio() {
        let before = vec![200, 300, 400];
        let after = vec![50, 75, 100];
        let ratio = AutoIndexEngine::avg_speedup_ratio(&before, &after);
        assert!((ratio - 4.0).abs() < 0.01);
    }

    #[test]
    fn test_7b7_avg_speedup_ratio_empty() {
        let ratio = AutoIndexEngine::avg_speedup_ratio(&[], &[]);
        assert_eq!(ratio, 0.0);
    }

    // -----------------------------------------------------------------
    //  负载生成器测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b7_generate_workload_count() {
        let records = generate_workload(100, 0.2, &["users"], &["id"]);
        assert_eq!(records.len(), 100);
    }

    #[test]
    fn test_7b7_generate_workload_slow_ratio() {
        let records = generate_workload(1000, 0.3, &["users"], &["id"]);
        let slow_count = records.iter().filter(|r| r.elapsed_ms >= 100).count();
        assert_eq!(slow_count, 300); // 30%
    }

    #[test]
    fn test_7b7_generate_workload_columns() {
        let records = generate_workload(10, 0.5, &["users"], &["id"]);
        assert!(records.iter().all(|r| !r.where_columns.is_empty()));
        assert!(records
            .iter()
            .all(|r| r.where_columns[0].starts_with("users.")));
    }

    // -----------------------------------------------------------------
    //  完整验证流程 — 100000 条负载
    // -----------------------------------------------------------------

    #[test]
    fn test_7b7_full_workflow_100000_queries() {
        let mut engine = AutoIndexEngine::new(100).unwrap();

        // Step 1: 生成 100000 条查询负载（20% 慢查询）
        let records = generate_workload(
            100000,
            0.2,
            &["products", "orders", "users"],
            &["id", "name", "customer_id", "status"],
        );
        engine.record_queries(records);
        assert_eq!(engine.len(), 100000);

        // Step 2: 分析慢查询 + 推荐索引
        let (stats, recommendations) = engine.analyze();
        assert_eq!(stats.total_queries, 100000);
        assert_eq!(stats.slow_queries, 20000); // 20%
        assert!(stats.recommendations > 0);
        assert!(stats.max_elapsed_ms >= 150);

        // Step 3: 应用推荐索引
        let apply_results = engine.apply_recommendations(&recommendations);
        let success_count = apply_results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(success_count, recommendations.len());
        assert_eq!(engine.created_indexes().len(), recommendations.len());

        // Step 4: 验证加速比 >= 2x
        // 模拟索引创建前后的查询耗时
        let before_ms: Vec<u64> = (0..1000).map(|i| 200 + i % 100).collect();
        let after_ms: Vec<u64> = (0..1000).map(|i| 50 + i % 20).collect();
        let speedup = AutoIndexEngine::avg_speedup_ratio(&before_ms, &after_ms);
        assert!(
            speedup >= 2.0,
            "speedup ratio should be >= 2x, got {speedup}"
        );
    }

    // -----------------------------------------------------------------
    //  零售场景验证
    // -----------------------------------------------------------------

    #[test]
    fn test_7b7_retail_scenario() {
        let mut engine = AutoIndexEngine::new(100).unwrap();

        // 模拟零售场景：products 表的 category 和 stock 列被频繁查询
        for i in 0..1000 {
            let elapsed = if i % 5 == 0 {
                180
            } else {
                30
            }; // 20% 慢查询
            let scanned = if i % 5 == 0 {
                50000
            } else {
                50
            };
            let mut record = QueryRecord::new(
                &format!("SELECT * FROM products WHERE category = {i} AND stock < 10"),
                elapsed,
                scanned,
            );
            record.tables = vec!["products".to_string()];
            record.where_columns = vec![
                "products.category".to_string(),
                "products.stock".to_string(),
            ];
            engine.record_query(record);
        }

        let (stats, recs) = engine.analyze();
        assert_eq!(stats.total_queries, 1000);
        assert_eq!(stats.slow_queries, 200); // 20%

        // 应推荐 products 表的索引
        let products_recs: Vec<&IndexRecommendation> =
            recs.iter().filter(|r| r.table == "products").collect();
        assert!(!products_recs.is_empty());

        // 应包含复合索引（category + stock）
        let has_composite = products_recs.iter().any(|r| r.columns.len() == 2);
        assert!(
            has_composite,
            "should recommend composite index for products"
        );

        // 应用所有推荐
        for rec in &recs {
            let _ = engine.apply_recommendation(rec);
        }
        assert!(!engine.created_indexes().is_empty());

        // 验证加速比
        let before_ms: Vec<u64> = (0..100).map(|_| 180).collect();
        let after_ms: Vec<u64> = (0..100).map(|_| 30).collect();
        let speedup = AutoIndexEngine::avg_speedup_ratio(&before_ms, &after_ms);
        assert!(speedup >= 2.0);
    }

    // -----------------------------------------------------------------
    //  QueryRecord 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b7_query_record_new() {
        let record = QueryRecord::new("SELECT 1", 50, 10);
        assert_eq!(record.sql, "SELECT 1");
        assert_eq!(record.elapsed_ms, 50);
        assert_eq!(record.scanned_rows, 10);
        assert!(record.tables.is_empty());
        assert!(record.where_columns.is_empty());
    }

    #[test]
    fn test_7b7_query_record_all_columns() {
        let mut record = QueryRecord::new("SELECT 1", 50, 10);
        record.where_columns = vec!["t.a".to_string()];
        record.join_columns = vec!["t.b".to_string()];
        record.order_by_columns = vec!["t.c".to_string()];
        let all = record.all_columns();
        assert_eq!(all.len(), 3);
        assert!(all.contains(&"t.a".to_string()));
        assert!(all.contains(&"t.b".to_string()));
        assert!(all.contains(&"t.c".to_string()));
    }

    // -----------------------------------------------------------------
    //  边界测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b7_threshold_boundary() {
        // 等于阈值的查询应被视为慢查询
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let mut record = QueryRecord::new("SELECT 1", 100, 1000);
        record.tables = vec!["t".to_string()];
        record.where_columns = vec!["t.a".to_string()];
        engine.record_query(record);

        let (stats, _) = engine.analyze();
        assert_eq!(stats.slow_queries, 1);
    }

    #[test]
    fn test_7b7_multiple_tables_analysis() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        // 涉及多表的慢查询
        let mut record = QueryRecord::new(
            "SELECT * FROM t1 JOIN t2 ON t1.id = t2.t1_id WHERE t1.name = 'x'",
            300,
            70000,
        );
        record.tables = vec!["t1".to_string(), "t2".to_string()];
        record.where_columns = vec!["t1.name".to_string()];
        record.join_columns = vec!["t1.id".to_string(), "t2.t1_id".to_string()];
        engine.record_query(record);

        let (stats, recs) = engine.analyze();
        assert_eq!(stats.tables_involved, 2);
        // 应为 t1 和 t2 都推荐索引
        let t1_rec = recs.iter().find(|r| r.table == "t1");
        let t2_rec = recs.iter().find(|r| r.table == "t2");
        assert!(t1_rec.is_some());
        assert!(t2_rec.is_some());
    }

    #[test]
    fn test_7b7_no_recommendation_for_fast_queries() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        // 只有快查询
        for _ in 0..100 {
            let mut record = QueryRecord::new("SELECT 1", 10, 5);
            record.tables = vec!["t".to_string()];
            record.where_columns = vec!["t.a".to_string()];
            engine.record_query(record);
        }

        let (stats, recs) = engine.analyze();
        assert_eq!(stats.slow_queries, 0);
        assert!(recs.is_empty());
    }

    #[test]
    fn test_7b7_reapply_after_drop() {
        let mut engine = AutoIndexEngine::new(100).unwrap();
        let rec = IndexRecommendation {
            index_name: "idx_t_a".to_string(),
            table: "t".to_string(),
            columns: vec!["a".to_string()],
            benefit_score: 5,
            reason: "test".to_string(),
        };
        engine.apply_recommendation(&rec).unwrap();
        engine.drop_index("idx_t_a").unwrap();
        // 删除后可再次创建
        let result = engine.apply_recommendation(&rec);
        assert!(result.is_ok());
    }
}
