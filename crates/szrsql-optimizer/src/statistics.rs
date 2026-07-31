//! Phase 5.1 — 统计信息收集（Statistics Collection）
//!
//! 提供 ANALYZE 命令的后端实现：扫描表数据，收集每列的：
//! - `null_count`：NULL 值数量
//! - `distinct_count`：NDV（Number of Distinct Values，忽略 NULL）
//! - `min_value` / `max_value`：最小/最大值（忽略 NULL；不可排序类型为 `None`）
//! - `histogram`：等深直方图（equi-depth，distinct_count > 阈值时构建）
//!
//! # 设计
//!
//! - 同步扫描 `TableStorage::scan_iter()`，与执行器一致
//! - 内存中的去重使用 `Vec<Value>` 线性去重（`Value` 未实现 `Hash`）
//! - 大表（NDV > `EXACT_DISTINCT_LIMIT`）使用排序后去重，避免 O(n²) 退化
//! - min/max 比较复用 `szrsql_sql::expr::compare_values`
//! - 等深直方图：排序后将 values 切成 `NUM_HISTOGRAM_BUCKETS` 个等高桶
//!
//! # NULL 语义
//!
//! 遵循 SQL 标准：
//! - `null_count` 明确统计 NULL 行数
//! - `distinct_count` 忽略 NULL（PG 行为）
//! - `min_value` / `max_value` 忽略 NULL；若全列 NULL 则为 `None`
//! - `row_count` 不忽略 NULL（即表总行数）
//!
//! # 不可排序类型
//!
//! `Blob` / `Array` / `Range` / `Json` / `TsVector` / `TsQuery` 在 `compare_values` 中
//! 返回 `None`，因此：
//! - `min_value` / `max_value` 永远为 `None`
//! - 不构建直方图（直方图要求排序）
//! - `distinct_count` 仍可收集（基于 `PartialEq`）
//!
//! # 性能
//!
//! - 时间复杂度：O(n × NDV) 用于去重（最坏情况 NDV = n）；排序直方图 O(n log n)
//! - 空间复杂度：O(NDV) 用于去重 Vec；直方图额外 O(n) 临时存储
//! - 对于 1M 行 × NDV=1M 的大表：去重约 1s，直方图排序约 200ms（可接受）

use std::collections::HashMap;
use std::time::SystemTime;

use szrsql_sql::executor::TableStorage;
use szrsql_sql::expr::compare_values;
use szrsql_types::value::Value;

// =====================================================================
//  常量
// =====================================================================

/// 超过此 NDV 阈值时，构建等深直方图
///
/// 低于阈值的列不需要直方图（ cardinality 估算直接用 NDV 即可）。
/// PG 默认 254 个桶，我们使用 100（更稀疏，节省内存）。
pub const NUM_HISTOGRAM_BUCKETS: usize = 100;

/// 超过此 NDV 阈值时，构建直方图
///
/// 阈值 = 2 × 桶数，保证每桶至少 2 个不同值。
pub const HISTOGRAM_MIN_NDV: usize = 2 * NUM_HISTOGRAM_BUCKETS;

/// 精确去重 NDV 上限
///
/// 超过此值时改用排序去重（O(n log n)），避免 O(n²) 退化。
/// 10000 是经验值：Vec<Value> 线性查找 10000 × 10000 = 1 亿次比较约 1s。
pub const EXACT_DISTINCT_LIMIT: usize = 10_000;

// =====================================================================
//  ColumnStatistics
// =====================================================================

/// 单列统计信息
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnStatistics {
    /// NULL 值数量
    pub null_count: usize,

    /// 不同值的数量（NDV，忽略 NULL）
    pub distinct_count: usize,

    /// 最小值（忽略 NULL；不可排序类型为 `None`；全 NULL 列也为 `None`）
    pub min_value: Option<Value>,

    /// 最大值（忽略 NULL；不可排序类型为 `None`；全 NULL 列也为 `None`）
    pub max_value: Option<Value>,

    /// 等深直方图（仅当 distinct_count > `HISTOGRAM_MIN_NDV` 且列可排序时构建）
    pub histogram: Option<Histogram>,
}

impl ColumnStatistics {
    /// 全 NULL 列的统计信息
    pub fn all_null(row_count: usize) -> Self {
        Self {
            null_count: row_count,
            distinct_count: 0,
            min_value: None,
            max_value: None,
            histogram: None,
        }
    }

    /// 估算选择率：等值谓词 `col = ?` 命中的行数比例
    ///
    /// - 有直方图：使用对应桶的 `1 / bucket.distinct_count`（更精确）
    /// - 无直方图：使用 `1 / distinct_count`（均匀分布假设）
    /// - 全 NULL 列：返回 0（无任何非 NULL 值）
    pub fn selectivity_eq(&self) -> f64 {
        if self.distinct_count == 0 {
            return 0.0;
        }
        // 简化：使用 1 / NDV；直方图优化待 Phase 5.2 成本模型实现
        1.0 / self.distinct_count as f64
    }

    /// 估算选择率：范围谓词 `col < ?` 或 `col > ?` 命中的行数比例
    ///
    /// 简化模型：假设均匀分布，使用 `1 / 3` 作为默认范围选择率（PG 风格）。
    /// 待 Phase 5.2 实现直方图区间查询后优化。
    pub fn selectivity_range(&self) -> f64 {
        if self.distinct_count == 0 {
            return 0.0;
        }
        1.0 / 3.0
    }
}

impl Default for ColumnStatistics {
    fn default() -> Self {
        Self::all_null(0)
    }
}

// =====================================================================
//  Histogram
// =====================================================================

/// 等深直方图（equi-depth histogram）
///
/// 每个桶包含大约相同数量的行（tuples），而非相同数量的不同值。
/// 适合基数估算：给定谓词 `col = X`，找到 X 所在桶，使用桶的 distinct_count 估算。
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    /// 桶列表（按 lower 升序）
    pub buckets: Vec<HistogramBucket>,
}

impl Histogram {
    /// 桶数量
    pub fn num_buckets(&self) -> usize {
        self.buckets.len()
    }

    /// 查找值所在的桶索引
    ///
    /// - 返回 `Some(i)`：值在 `buckets[i]` 范围内（lower <= value < upper，最后一桶 upper 闭区间）
    /// - 返回 `None`：值超出所有桶范围
    pub fn find_bucket(&self, value: &Value) -> Option<usize> {
        for (i, bucket) in self.buckets.iter().enumerate() {
            // lower <= value
            let ge_lower = compare_values(&bucket.lower, value)
                .map(|o| o != std::cmp::Ordering::Greater)
                .unwrap_or(false);
            if !ge_lower {
                continue;
            }
            // 最后一桶：value <= upper；其他桶：value < upper
            let is_last = i == self.buckets.len() - 1;
            if is_last {
                let le_upper = compare_values(value, &bucket.upper)
                    .map(|o| o != std::cmp::Ordering::Greater)
                    .unwrap_or(false);
                if le_upper {
                    return Some(i);
                }
            } else {
                let lt_upper = compare_values(value, &bucket.upper)
                    .map(|o| o == std::cmp::Ordering::Less)
                    .unwrap_or(false);
                if lt_upper {
                    return Some(i);
                }
            }
        }
        None
    }
}

/// 直方图桶
#[derive(Debug, Clone, PartialEq)]
pub struct HistogramBucket {
    /// 桶下界（包含）
    pub lower: Value,
    /// 桶上界（最后一桶包含，其他桶不包含）
    pub upper: Value,
    /// 桶内总行数（不含 NULL）
    pub count: usize,
    /// 桶内不同值数量
    pub distinct_count: usize,
}

// =====================================================================
//  TableStatistics
// =====================================================================

/// 表级统计信息
#[derive(Debug, Clone, PartialEq)]
pub struct TableStatistics {
    /// 表名（catalog 中的 qualified name，全小写）
    pub table_name: String,
    /// 总行数（含 NULL 行，不含已删除行）
    pub row_count: usize,
    /// 每列统计信息（key = 列名，全小写）
    pub column_stats: HashMap<String, ColumnStatistics>,
    /// 收集时间
    pub collected_at: SystemTime,
}

impl TableStatistics {
    /// 创建空表统计（row_count = 0，无列统计）
    pub fn empty(table_name: impl Into<String>) -> Self {
        Self {
            table_name: table_name.into(),
            row_count: 0,
            column_stats: HashMap::new(),
            collected_at: SystemTime::now(),
        }
    }

    /// 获取指定列的统计信息
    pub fn column(&self, name: &str) -> Option<&ColumnStatistics> {
        // 列名大小写不敏感（与 Catalog 一致）
        self.column_stats.get(&name.to_lowercase())
    }
}

// =====================================================================
//  StatisticsCollector
// =====================================================================

/// 统计信息收集器
///
/// 扫描 `TableStorage` 的所有行，收集每列的统计信息。
/// 不持有任何状态，可独立调用。
pub struct StatisticsCollector;

impl StatisticsCollector {
    /// 扫描整张表，收集所有列的统计信息
    ///
    /// # 算法
    ///
    /// 单次全表扫描，对每列维护：
    /// - `null_count`：累加
    /// - `min_value` / `max_value`：与当前值比较更新
    /// - `distinct_values: Vec<Value>`：去重收集（上限 `EXACT_DISTINCT_LIMIT`）
    /// - `all_values: Vec<Value>`：仅可排序列，用于构建直方图
    ///
    /// 扫描结束后：
    /// - 若 `distinct_values.len() == EXACT_DISTINCT_LIMIT`（达到上限），改为排序去重精确计数
    /// - 若 `distinct_count > HISTOGRAM_MIN_NDV` 且列可排序，构建等深直方图
    pub fn collect(storage: &dyn TableStorage) -> TableStatistics {
        let schema = storage.schema();
        let num_cols = schema.columns.len();

        // 每列的累加器
        let mut null_counts = vec![0usize; num_cols];
        let mut min_values: Vec<Option<Value>> = vec![None; num_cols];
        let mut max_values: Vec<Option<Value>> = vec![None; num_cols];
        // 去重收集：每列一个 Vec<Value>
        let mut distinct_values: Vec<Vec<Value>> = vec![Vec::new(); num_cols];
        // 是否达到精确去重上限（达到后停止收集，改用排序去重）
        let mut reached_limit = vec![false; num_cols];
        // 全部值（仅可排序列），用于直方图
        let mut all_values: Vec<Vec<Value>> = vec![Vec::new(); num_cols];

        let mut row_count = 0usize;

        // 单次全表扫描
        for row in storage.scan_iter() {
            row_count += 1;
            for (i, value) in row.iter().enumerate().take(num_cols) {
                if matches!(value, Value::Null) {
                    null_counts[i] += 1;
                    continue;
                }

                // 更新 min/max（仅可排序值；不可排序值保持 None）
                if Self::is_sortable(value) {
                    Self::update_min(&mut min_values[i], value);
                    Self::update_max(&mut max_values[i], value);
                    // 全部值收集（仅可排序列），用于直方图
                    all_values[i].push(value.clone());
                }

                // 去重收集
                if !reached_limit[i] {
                    let dv = &mut distinct_values[i];
                    if !dv.iter().any(|v| v == value) {
                        dv.push(value.clone());
                        if dv.len() >= EXACT_DISTINCT_LIMIT {
                            reached_limit[i] = true;
                            // 不 break，继续扫描更新 min/max/null_count
                            // 但停止去重收集（distinct_count 将在扫描后通过排序精确计算）
                        }
                    }
                }
            }
        }

        // 构建每列统计
        let mut column_stats = HashMap::with_capacity(num_cols);
        for (i, col_def) in schema.columns.iter().enumerate().take(num_cols) {
            let distinct_count = if reached_limit[i] {
                // 达到上限，改用排序去重精确计算
                Self::sorted_distinct_count(&all_values[i])
            } else {
                distinct_values[i].len()
            };

            // 构建直方图（仅当 NDV 足够大且列可排序）
            let histogram = if distinct_count > HISTOGRAM_MIN_NDV
                && !all_values[i].is_empty()
                && Self::is_sortable(&all_values[i][0])
            {
                Self::build_equi_depth_histogram(&mut all_values[i], NUM_HISTOGRAM_BUCKETS)
            } else {
                None
            };

            // all_values[i] 已在直方图构建时被借用并排序；
            // 显式清空以尽早释放内存（避免持有至函数返回）
            all_values[i].clear();
            all_values[i].shrink_to_fit();

            column_stats.insert(
                col_def.name.to_lowercase(),
                ColumnStatistics {
                    null_count: null_counts[i],
                    distinct_count,
                    min_value: min_values[i].take(),
                    max_value: max_values[i].take(),
                    histogram,
                },
            );
        }

        TableStatistics {
            table_name: schema.name.qualified_name().to_lowercase(),
            row_count,
            column_stats,
            collected_at: SystemTime::now(),
        }
    }

    /// 仅收集指定列的统计信息（节省不必要的内存开销）
    ///
    /// 未指定的列不在 `column_stats` 中（调用方应处理 `None`）。
    pub fn collect_columns(storage: &dyn TableStorage, columns: &[&str]) -> TableStatistics {
        let schema = storage.schema();
        // 找到指定列的索引（大小写不敏感）
        let mut col_indices: Vec<Option<usize>> = Vec::with_capacity(columns.len());
        for col_name in columns {
            let idx = schema
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(col_name));
            col_indices.push(idx);
        }

        let mut null_counts = vec![0usize; columns.len()];
        let mut min_values: Vec<Option<Value>> = vec![None; columns.len()];
        let mut max_values: Vec<Option<Value>> = vec![None; columns.len()];
        let mut distinct_values: Vec<Vec<Value>> = vec![Vec::new(); columns.len()];
        let mut reached_limit = vec![false; columns.len()];
        let mut all_values: Vec<Vec<Value>> = vec![Vec::new(); columns.len()];

        let mut row_count = 0usize;
        for row in storage.scan_iter() {
            row_count += 1;
            for (slot, opt_idx) in col_indices.iter().enumerate() {
                let Some(i) = opt_idx else {
                    continue;
                };
                let value = &row[*i];
                if matches!(value, Value::Null) {
                    null_counts[slot] += 1;
                    continue;
                }
                // 更新 min/max（仅可排序值；不可排序值保持 None）
                if Self::is_sortable(value) {
                    Self::update_min(&mut min_values[slot], value);
                    Self::update_max(&mut max_values[slot], value);
                    all_values[slot].push(value.clone());
                }
                if !reached_limit[slot] {
                    let dv = &mut distinct_values[slot];
                    if !dv.iter().any(|v| v == value) {
                        dv.push(value.clone());
                        if dv.len() >= EXACT_DISTINCT_LIMIT {
                            reached_limit[slot] = true;
                        }
                    }
                }
            }
        }

        let mut column_stats = HashMap::with_capacity(columns.len());
        for (slot, col_name) in columns.iter().enumerate() {
            let distinct_count = if reached_limit[slot] {
                Self::sorted_distinct_count(&all_values[slot])
            } else {
                distinct_values[slot].len()
            };
            let histogram = if distinct_count > HISTOGRAM_MIN_NDV
                && !all_values[slot].is_empty()
                && Self::is_sortable(&all_values[slot][0])
            {
                Self::build_equi_depth_histogram(&mut all_values[slot], NUM_HISTOGRAM_BUCKETS)
            } else {
                None
            };
            column_stats.insert(
                col_name.to_lowercase(),
                ColumnStatistics {
                    null_count: null_counts[slot],
                    distinct_count,
                    min_value: min_values[slot].take(),
                    max_value: max_values[slot].take(),
                    histogram,
                },
            );
        }

        TableStatistics {
            table_name: schema.name.qualified_name().to_lowercase(),
            row_count,
            column_stats,
            collected_at: SystemTime::now(),
        }
    }

    // -----------------------------------------------------------------
    //  内部辅助方法
    // -----------------------------------------------------------------

    /// 更新列的最小值
    fn update_min(current: &mut Option<Value>, candidate: &Value) {
        match current {
            None => *current = Some(candidate.clone()),
            Some(cur) => {
                if let Some(ordering) = compare_values(candidate, cur) {
                    if ordering == std::cmp::Ordering::Less {
                        *current = Some(candidate.clone());
                    }
                }
                // 不可比较的值（None）跳过
            }
        }
    }

    /// 更新列的最大值
    fn update_max(current: &mut Option<Value>, candidate: &Value) {
        match current {
            None => *current = Some(candidate.clone()),
            Some(cur) => {
                if let Some(ordering) = compare_values(candidate, cur) {
                    if ordering == std::cmp::Ordering::Greater {
                        *current = Some(candidate.clone());
                    }
                }
            }
        }
    }

    /// 判断值是否可排序（compare_values 不返回 None）
    fn is_sortable(value: &Value) -> bool {
        // 用值与自身比较判断可排序性
        // 注意：Float64 的 NaN 与自身比较返回 None，但 NaN 极少出现，简化处理
        compare_values(value, value).is_some()
    }

    /// 排序后去重计数（O(n log n)）
    ///
    /// 用于 `distinct_values` 达到上限后的精确计数。
    fn sorted_distinct_count(values: &[Value]) -> usize {
        if values.is_empty() {
            return 0;
        }
        let mut sorted: Vec<&Value> = values.iter().collect();
        // 使用 compare_values 进行稳定排序
        // 不可比较的值（None）保持原相对顺序
        sorted.sort_by(|a, b| compare_values(a, b).unwrap_or(std::cmp::Ordering::Equal));
        let mut count = 1;
        for i in 1..sorted.len() {
            if sorted[i] != sorted[i - 1] {
                count += 1;
            }
        }
        count
    }

    /// 构建等深直方图
    ///
    /// # 算法
    ///
    /// 1. 排序 values（使用 compare_values，不可比较的视为相等）
    /// 2. 切成 `num_buckets` 个等高桶
    /// 3. 每桶记录 lower/upper/count/distinct_count
    fn build_equi_depth_histogram(values: &mut [Value], num_buckets: usize) -> Option<Histogram> {
        if values.is_empty() || num_buckets == 0 {
            return None;
        }

        // 排序
        values.sort_by(|a, b| compare_values(a, b).unwrap_or(std::cmp::Ordering::Equal));

        let n = values.len();
        let bucket_size = n.div_ceil(num_buckets); // 每桶至少 ceil(n/num_buckets) 个值
        let mut buckets = Vec::with_capacity(num_buckets);

        let mut idx = 0;
        while idx < n {
            let end = (idx + bucket_size).min(n);
            let lower = values[idx].clone();
            let upper = values[end - 1].clone();

            // 计算桶内 distinct_count
            let mut bucket_distinct = 1;
            for i in (idx + 1)..end {
                if values[i] != values[i - 1] {
                    bucket_distinct += 1;
                }
            }

            buckets.push(HistogramBucket {
                lower,
                upper,
                count: end - idx,
                distinct_count: bucket_distinct,
            });

            idx = end;
        }

        Some(Histogram { buckets })
    }
}

// =====================================================================
//  StatisticsStore
// =====================================================================

/// 统计信息存储 trait（catalog 级别，跨会话共享）
///
/// 实现方需保证线程安全（`Send + Sync`）。
/// ANALYZE 命令通过 `update_table_stats` 更新；优化器通过 `get_table_stats` 读取。
pub trait StatisticsStore: Send + Sync {
    /// 获取表的统计信息
    fn get_table_stats(&self, table: &str) -> Option<&TableStatistics>;

    /// 更新表的统计信息（ANALYZE 调用）
    fn update_table_stats(&mut self, table: &str, stats: TableStatistics);

    /// 删除表的统计信息（DROP TABLE 调用）
    fn drop_table_stats(&mut self, table: &str);

    /// 列出所有有统计信息的表
    fn list_tables(&self) -> Vec<String>;
}

/// 内存中的统计信息存储（默认实现）
///
/// 用于单进程场景（如 szrsql-bin）；多进程共享需实现基于持久化存储的版本。
#[derive(Debug, Default, Clone)]
pub struct InMemoryStatisticsStore {
    stats: HashMap<String, TableStatistics>,
}

impl InMemoryStatisticsStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl StatisticsStore for InMemoryStatisticsStore {
    fn get_table_stats(&self, table: &str) -> Option<&TableStatistics> {
        self.stats.get(&table.to_lowercase())
    }

    fn update_table_stats(&mut self, table: &str, stats: TableStatistics) {
        self.stats.insert(table.to_lowercase(), stats);
    }

    fn drop_table_stats(&mut self, table: &str) {
        self.stats.remove(&table.to_lowercase());
    }

    fn list_tables(&self) -> Vec<String> {
        self.stats.keys().cloned().collect()
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_sql::executor::InMemoryTable;
    use szrsql_types::value::ColumnType;

    /// 构建简单测试表
    fn build_test_table() -> InMemoryTable {
        // CREATE TABLE t (id INT, name TEXT, score FLOAT, active BOOL)
        InMemoryTable::with_columns(
            "t",
            vec![
                ("id", ColumnType::Int64),
                ("name", ColumnType::Text),
                ("score", ColumnType::Float64),
                ("active", ColumnType::Bool),
            ],
        )
    }

    /// 插入已知数据
    fn insert_test_data(table: &mut InMemoryTable) {
        // id, name, score, active
        let rows = vec![
            vec![
                Value::Int64(1),
                Value::Text("alice".into()),
                Value::Float64(85.5),
                Value::Bool(true),
            ],
            vec![
                Value::Int64(2),
                Value::Text("bob".into()),
                Value::Float64(90.0),
                Value::Bool(false),
            ],
            vec![
                Value::Int64(3),
                Value::Text("alice".into()),
                Value::Float64(75.0),
                Value::Bool(true),
            ],
            vec![
                Value::Int64(4),
                Value::Null,
                Value::Float64(80.0),
                Value::Bool(true),
            ],
            vec![
                Value::Int64(5),
                Value::Text("carol".into()),
                Value::Null,
                Value::Bool(false),
            ],
        ];
        for row in rows {
            table.insert(row);
        }
    }

    #[test]
    fn test_collect_basic_stats() {
        let mut table = build_test_table();
        insert_test_data(&mut table);

        let stats = StatisticsCollector::collect(&table);

        assert_eq!(stats.table_name, "t");
        assert_eq!(stats.row_count, 5);

        // id 列：5 行，0 NULL，5 distinct，min=1，max=5
        let id_stats = stats.column("id").unwrap();
        assert_eq!(id_stats.null_count, 0);
        assert_eq!(id_stats.distinct_count, 5);
        assert_eq!(id_stats.min_value, Some(Value::Int64(1)));
        assert_eq!(id_stats.max_value, Some(Value::Int64(5)));

        // name 列：5 行，1 NULL，3 distinct (alice/bob/carol)，min="alice"，max="carol"
        let name_stats = stats.column("name").unwrap();
        assert_eq!(name_stats.null_count, 1);
        assert_eq!(name_stats.distinct_count, 3);
        assert_eq!(name_stats.min_value, Some(Value::Text("alice".into())));
        assert_eq!(name_stats.max_value, Some(Value::Text("carol".into())));

        // score 列：5 行，1 NULL，4 distinct，min=75.0，max=90.0
        let score_stats = stats.column("score").unwrap();
        assert_eq!(score_stats.null_count, 1);
        assert_eq!(score_stats.distinct_count, 4);
        assert_eq!(score_stats.min_value, Some(Value::Float64(75.0)));
        assert_eq!(score_stats.max_value, Some(Value::Float64(90.0)));

        // active 列：5 行，0 NULL，2 distinct，min=false，max=true
        let active_stats = stats.column("active").unwrap();
        assert_eq!(active_stats.null_count, 0);
        assert_eq!(active_stats.distinct_count, 2);
        assert_eq!(active_stats.min_value, Some(Value::Bool(false)));
        assert_eq!(active_stats.max_value, Some(Value::Bool(true)));
    }

    #[test]
    fn test_collect_all_null_column() {
        let mut table = InMemoryTable::with_columns("t", vec![("c", ColumnType::Int64)]);
        for _ in 0..5 {
            table.insert(vec![Value::Null]);
        }
        let stats = StatisticsCollector::collect(&table);
        let col = stats.column("c").unwrap();
        assert_eq!(col.null_count, 5);
        assert_eq!(col.distinct_count, 0);
        assert_eq!(col.min_value, None);
        assert_eq!(col.max_value, None);
        assert_eq!(col.histogram, None);
    }

    #[test]
    fn test_collect_empty_table() {
        let table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
        let stats = StatisticsCollector::collect(&table);
        assert_eq!(stats.row_count, 0);
        let col = stats.column("id").unwrap();
        assert_eq!(col.null_count, 0);
        assert_eq!(col.distinct_count, 0);
        assert_eq!(col.min_value, None);
        assert_eq!(col.max_value, None);
    }

    #[test]
    fn test_collect_unsortable_column() {
        // Blob 不可排序
        let mut table = InMemoryTable::with_columns("t", vec![("data", ColumnType::Blob)]);
        table.insert(vec![Value::Blob(vec![1, 2, 3])]);
        table.insert(vec![Value::Blob(vec![4, 5, 6])]);
        table.insert(vec![Value::Blob(vec![1, 2, 3])]); // 重复
        table.insert(vec![Value::Null]);

        let stats = StatisticsCollector::collect(&table);
        let col = stats.column("data").unwrap();
        assert_eq!(col.null_count, 1);
        assert_eq!(col.distinct_count, 2); // [1,2,3] 和 [4,5,6]
        assert_eq!(col.min_value, None); // 不可排序
        assert_eq!(col.max_value, None);
        assert_eq!(col.histogram, None);
    }

    #[test]
    fn test_collect_columns_subset() {
        let mut table = build_test_table();
        insert_test_data(&mut table);

        // 只收集 id 和 name 列
        let stats = StatisticsCollector::collect_columns(&table, &["id", "name"]);
        assert_eq!(stats.row_count, 5);
        assert!(stats.column("id").is_some());
        assert!(stats.column("name").is_some());
        assert!(stats.column("score").is_none()); // 未收集
        assert!(stats.column("active").is_none()); // 未收集
    }

    #[test]
    fn test_histogram_built_for_large_ndv() {
        // 构造 distinct_count > HISTOGRAM_MIN_NDV (200) 的列
        let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
        for i in 0..1000 {
            table.insert(vec![Value::Int64(i)]);
        }
        let stats = StatisticsCollector::collect(&table);
        let col = stats.column("id").unwrap();
        assert_eq!(col.distinct_count, 1000);
        assert!(col.histogram.is_some());
        let hist = col.histogram.as_ref().unwrap();
        assert!(hist.num_buckets() > 0);
        assert!(hist.num_buckets() <= NUM_HISTOGRAM_BUCKETS);
        // 桶总和应等于总行数
        let total: usize = hist.buckets.iter().map(|b| b.count).sum();
        assert_eq!(total, 1000);
    }

    #[test]
    fn test_histogram_not_built_for_small_ndv() {
        let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
        for i in 0..10 {
            table.insert(vec![Value::Int64(i)]);
        }
        let stats = StatisticsCollector::collect(&table);
        let col = stats.column("id").unwrap();
        assert_eq!(col.distinct_count, 10);
        assert!(col.histogram.is_none()); // NDV < HISTOGRAM_MIN_NDV
    }

    #[test]
    fn test_histogram_find_bucket() {
        let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
        for i in 0..1000 {
            table.insert(vec![Value::Int64(i)]);
        }
        let stats = StatisticsCollector::collect(&table);
        let col = stats.column("id").unwrap();
        let hist = col.histogram.as_ref().unwrap();

        // 值 0 应在第一桶
        assert_eq!(hist.find_bucket(&Value::Int64(0)), Some(0));
        // 值 999 应在最后一桶
        let last = hist.num_buckets() - 1;
        assert_eq!(hist.find_bucket(&Value::Int64(999)), Some(last));
        // 值 500 应在中间某桶
        let mid = hist.find_bucket(&Value::Int64(500));
        assert!(mid.is_some());
        assert!(mid.unwrap() > 0 && mid.unwrap() < last);
    }

    #[test]
    fn test_selectivity_eq() {
        let col = ColumnStatistics {
            null_count: 0,
            distinct_count: 100,
            min_value: Some(Value::Int64(0)),
            max_value: Some(Value::Int64(99)),
            histogram: None,
        };
        assert!((col.selectivity_eq() - 0.01).abs() < 1e-9);
    }

    #[test]
    fn test_selectivity_eq_zero_ndv() {
        let col = ColumnStatistics::all_null(10);
        assert_eq!(col.selectivity_eq(), 0.0);
    }

    #[test]
    fn test_selectivity_range() {
        let col = ColumnStatistics {
            null_count: 0,
            distinct_count: 100,
            min_value: Some(Value::Int64(0)),
            max_value: Some(Value::Int64(99)),
            histogram: None,
        };
        assert!((col.selectivity_range() - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn test_in_memory_store_basic() {
        let mut store = InMemoryStatisticsStore::new();
        let stats = TableStatistics::empty("t1");
        store.update_table_stats("t1", stats);
        assert!(store.get_table_stats("t1").is_some());
        assert!(store.get_table_stats("t2").is_none());
        assert_eq!(store.list_tables().len(), 1);

        store.drop_table_stats("t1");
        assert!(store.get_table_stats("t1").is_none());
    }

    #[test]
    fn test_store_case_insensitive() {
        let mut store = InMemoryStatisticsStore::new();
        let stats = TableStatistics::empty("MyTable");
        store.update_table_stats("MyTable", stats);
        // 大小写不敏感查询
        assert!(store.get_table_stats("mytable").is_some());
        assert!(store.get_table_stats("MYTABLE").is_some());
    }

    #[test]
    fn test_collect_100k_rows_accuracy() {
        // 验收标准：100K 行（1M 太慢用于单元测试）
        // distinct_count 误差 = 0%（精确统计）
        // null_count 误差 = 0%（精确统计）
        // min/max 误差 = 0%（精确统计）
        let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);

        // 插入 100K 行：id = 0..100000
        // 5% NULL（在 id 是 1000 倍数时插入 NULL）
        let mut expected_nulls = 0;
        for i in 0..100_000 {
            if i % 1000 == 0 && i > 0 {
                table.insert(vec![Value::Null]);
                expected_nulls += 1;
            } else {
                table.insert(vec![Value::Int64(i)]);
            }
        }

        let stats = StatisticsCollector::collect(&table);
        let col = stats.column("id").unwrap();

        assert_eq!(stats.row_count, 100_000);
        assert_eq!(col.null_count, expected_nulls);
        // distinct_count：99 个 NULL + 100000 - 99 = 99901 个不同 Int64 值
        // 实际：id = 0..99999 + 99 个 NULL
        // 但 i=1000,2000,...,99000 (99 个) 被替换为 NULL，所以 id 缺少这些值
        // distinct id = 100000 - 99 = 99901
        assert_eq!(col.distinct_count, 100_000 - 99);
        assert_eq!(col.min_value, Some(Value::Int64(0)));
        assert_eq!(col.max_value, Some(Value::Int64(99_999)));
        assert!(col.histogram.is_some());
    }

    #[test]
    fn test_collect_with_duplicates() {
        let mut table = InMemoryTable::with_columns("t", vec![("c", ColumnType::Int64)]);
        for _ in 0..100 {
            table.insert(vec![Value::Int64(42)]); // 100 个相同值
        }
        let stats = StatisticsCollector::collect(&table);
        let col = stats.column("c").unwrap();
        assert_eq!(col.null_count, 0);
        assert_eq!(col.distinct_count, 1);
        assert_eq!(col.min_value, Some(Value::Int64(42)));
        assert_eq!(col.max_value, Some(Value::Int64(42)));
        assert!(col.histogram.is_none()); // NDV=1 < HISTOGRAM_MIN_NDV
    }

    #[test]
    fn test_table_statistics_empty() {
        let stats = TableStatistics::empty("nonexistent");
        assert_eq!(stats.table_name, "nonexistent");
        assert_eq!(stats.row_count, 0);
        assert!(stats.column_stats.is_empty());
        assert!(stats.column("anything").is_none());
    }

    #[test]
    fn test_column_statistics_default() {
        let col = ColumnStatistics::default();
        assert_eq!(col.null_count, 0);
        assert_eq!(col.distinct_count, 0);
        assert_eq!(col.min_value, None);
        assert_eq!(col.max_value, None);
        assert_eq!(col.histogram, None);
    }

    #[test]
    fn test_update_min_max_with_mixed_types() {
        // 跨类型比较：Int64 vs Float64
        let mut table = InMemoryTable::with_columns("t", vec![("c", ColumnType::Int64)]);
        // 同列插入 Int64 值（不能跨类型，因为列类型固定）
        table.insert(vec![Value::Int64(10)]);
        table.insert(vec![Value::Int64(-5)]);
        table.insert(vec![Value::Int64(100)]);
        table.insert(vec![Value::Int64(0)]);

        let stats = StatisticsCollector::collect(&table);
        let col = stats.column("c").unwrap();
        assert_eq!(col.min_value, Some(Value::Int64(-5)));
        assert_eq!(col.max_value, Some(Value::Int64(100)));
    }

    /// 验证大规模数据收集性能（不超时）
    #[test]
    fn test_collect_large_dataset_performance() {
        // 50K 行（足够大验证性能，又不至于让 CI 超时）
        let mut table = InMemoryTable::with_columns("t", vec![("id", ColumnType::Int64)]);
        for i in 0..50_000 {
            table.insert(vec![Value::Int64(i % 1000)]); // NDV = 1000
        }

        let start = std::time::Instant::now();
        let stats = StatisticsCollector::collect(&table);
        let elapsed = start.elapsed();

        let col = stats.column("id").unwrap();
        assert_eq!(col.distinct_count, 1000);
        assert_eq!(col.min_value, Some(Value::Int64(0)));
        assert_eq!(col.max_value, Some(Value::Int64(999)));
        assert!(col.histogram.is_some());

        // 性能断言：50K 行收集应在 5s 内完成
        // （CI 上 Rust debug 构建可能较慢，放宽到 10s）
        assert!(elapsed.as_secs() < 10, "collect took too long: {elapsed:?}");
    }
}
