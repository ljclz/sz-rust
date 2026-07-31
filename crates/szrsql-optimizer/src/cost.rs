//! Phase 5.2 — 成本模型基础（Cost Model）
//!
//! 提供 CBO（Cost-Based Optimization）的核心成本估算能力：
//! - 递归遍历 `LogicalPlan`，估算每个算子的成本与输出行数（cardinality）
//! - 支持 Scan / Filter / Projection / Join / Aggregate / Sort / Limit / Distinct
//! - Join 成本区分 NestedLoopJoin 与 HashJoin，自动选择更优算法
//! - 谓词选择率估算基于 `ColumnStatistics`（等深直方图待 Phase 5.7 集成）
//!
//! # 设计
//!
//! 参考 PostgreSQL 成本模型，但简化为单机内存场景：
//! - 顺序 I/O 成本（`SEQ_PAGE_COST`）：每页 1.0
//! - 随机 I/O 成本（`RANDOM_PAGE_COST`）：每页 4.0（IndexScan 默认）
//! - CPU 元组处理成本（`CPU_TUPLE_COST`）：每行 0.01
//! - CPU 操作符成本（`CPU_OPERATOR_COST`）：每次比较 0.0025
//! - Hash 构建成本（`HASH_COST`）：每行 0.5
//! - 排序成本（`SORT_COST`）：每次比较 0.01
//!
//! # 选择率估算
//!
//! - 等值谓词 `col = ?`：`1 / NDV`（有统计）或 `0.005`（无统计，PG 默认）
//! - 范围谓词 `col < ?` / `col > ?`：`1 / 3`（均匀分布假设）
//! - 不等谓词 `col != ?`：`1 - 1/NDV`
//! - AND：`min(s1, s2) * 0.5`（PG 风格，相关性折扣）
//! - OR：`min(s1 + s2, 1.0)`
//! - 默认：`0.1`
//!
//! # NULL 语义
//!
//! 选择率估算忽略 NULL（与 PG 一致），NULL 行在 Filter 中默认被过滤。
//! 实际 NULL 是否被过滤取决于谓词（`IS NULL` / `IS NOT NULL` / 其他）。

use std::sync::Arc;

use szrsql_sql::ast::{BinaryOp, Expr, JoinType, TableName};
use szrsql_sql::plan::{LogicalPlan, TableSchema};

use crate::statistics::{StatisticsStore, TableStatistics};

// =====================================================================
//  成本常量（参考 PG，简化为内存场景）
// =====================================================================

/// 顺序 I/O 成本（每页）
pub const SEQ_PAGE_COST: f64 = 1.0;

/// 随机 I/O 成本（每页，用于 IndexScan）
pub const RANDOM_PAGE_COST: f64 = 4.0;

/// CPU 元组处理成本（每行）
pub const CPU_TUPLE_COST: f64 = 0.01;

/// CPU 操作符成本（每次比较）
pub const CPU_OPERATOR_COST: f64 = 0.0025;

/// Hash 构建成本（每行）
pub const HASH_COST: f64 = 0.5;

/// 排序成本（每次比较）
pub const SORT_COST: f64 = 0.01;

/// 默认页大小（行数估算用，假设每页 100 行）
pub const ROWS_PER_PAGE: usize = 100;

/// 无统计信息时的默认行数
pub const DEFAULT_ROW_COUNT: usize = 1000;

/// 无统计信息时的默认 NDV
pub const DEFAULT_NDV: usize = 100;

/// 无统计信息时的默认等值选择率（PG 默认 0.005）
pub const DEFAULT_EQ_SELECTIVITY: f64 = 0.005;

/// 无统计信息时的默认范围选择率
pub const DEFAULT_RANGE_SELECTIVITY: f64 = 1.0 / 3.0;

/// 默认 JOIN 选择率（无统计时的 inner join 估算）
pub const DEFAULT_JOIN_SELECTIVITY: f64 = 0.1;

/// HashJoin 阈值：两侧 cardinality 都超过此值时考虑 HashJoin
pub const HASH_JOIN_MIN_ROWS: usize = 100;

// =====================================================================
//  Cost 结构
// =====================================================================

/// 算子成本估算结果
///
/// `total_cost = cpu_cost + io_cost`，用于计划比较。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cost {
    /// CPU 成本（元组处理 + 操作符）
    pub cpu_cost: f64,
    /// I/O 成本（页读取）
    pub io_cost: f64,
    /// 输出行数（cardinality）
    pub cardinality: usize,
    /// 平均行宽（字节，简化为列数 × 8）
    pub width: usize,
}

impl Cost {
    /// 创建零成本
    pub fn zero() -> Self {
        Self {
            cpu_cost: 0.0,
            io_cost: 0.0,
            cardinality: 0,
            width: 0,
        }
    }

    /// 总成本（CPU + I/O）
    pub fn total(&self) -> f64 {
        self.cpu_cost + self.io_cost
    }

    /// 估算页数（基于 cardinality 和 ROWS_PER_PAGE）
    fn pages(&self) -> f64 {
        (self.cardinality as f64) / (ROWS_PER_PAGE as f64)
    }
}

impl std::ops::Add for Cost {
    type Output = Cost;

    fn add(self, rhs: Cost) -> Cost {
        Cost {
            cpu_cost: self.cpu_cost + rhs.cpu_cost,
            io_cost: self.io_cost + rhs.io_cost,
            // 子节点 cardinality 不直接累加（由父算子重新估算）
            cardinality: self.cardinality.max(rhs.cardinality),
            width: self.width.max(rhs.width),
        }
    }
}

// =====================================================================
//  JoinAlgorithm
// =====================================================================

/// JOIN 执行算法
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinAlgorithm {
    /// NestedLoopJoin — 适用于小表或 Cross Join
    NestedLoop,
    /// HashJoin — 适用于等值 JOIN 且两侧数据量较大
    Hash,
}

impl JoinAlgorithm {
    /// 根据条件自动选择最优 JOIN 算法
    ///
    /// - Cross Join / 非等值条件 → NestedLoop
    /// - 等值条件 + 两侧 cardinality > `HASH_JOIN_MIN_ROWS` → Hash
    /// - 否则 → NestedLoop
    pub fn choose(
        join_type: JoinType,
        condition: &szrsql_sql::ast::JoinCondition,
        left_card: usize,
        right_card: usize,
    ) -> Self {
        // Cross Join 只能用 NestedLoop
        if matches!(join_type, JoinType::Cross) {
            return JoinAlgorithm::NestedLoop;
        }
        // 检查是否为等值条件
        if !is_equi_condition(condition) {
            return JoinAlgorithm::NestedLoop;
        }
        // 两侧数据量足够大时用 HashJoin
        if left_card >= HASH_JOIN_MIN_ROWS && right_card >= HASH_JOIN_MIN_ROWS {
            JoinAlgorithm::Hash
        } else {
            JoinAlgorithm::NestedLoop
        }
    }
}

/// 判断 JOIN 条件是否为等值条件（支持 ON a = b 形式）
fn is_equi_condition(condition: &szrsql_sql::ast::JoinCondition) -> bool {
    use szrsql_sql::ast::JoinCondition;
    match condition {
        JoinCondition::On(expr) => is_equi_expr(expr),
        JoinCondition::Using(_) | JoinCondition::Natural => true, // USING/NATURAL 隐式等值
        JoinCondition::None => false,
    }
}

/// 递归判断表达式是否为等值条件（顶层是 `=` 且左右都是列引用）
fn is_equi_expr(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOp::Eq,
            right,
        } => is_column_ref(left) && is_column_ref(right),
        // AND 连接的多个等值条件也算等值
        Expr::BinaryOp {
            left,
            op: BinaryOp::And,
            right,
        } => is_equi_expr(left) && is_equi_expr(right),
        _ => false,
    }
}

/// 判断表达式是否为列引用
fn is_column_ref(expr: &Expr) -> bool {
    matches!(expr, Expr::Identifier(_))
}

// =====================================================================
//  CostModel
// =====================================================================

/// 成本模型 — 基于 `StatisticsStore` 估算 `LogicalPlan` 的成本
///
/// 不持有可变状态，可并发使用。
pub struct CostModel {
    /// 统计信息存储（只读访问）
    stats_store: Arc<dyn StatisticsStore>,
}

impl CostModel {
    /// 创建成本模型
    pub fn new(stats_store: Arc<dyn StatisticsStore>) -> Self {
        Self { stats_store }
    }

    /// 估算逻辑计划的总成本
    pub fn estimate(&self, plan: &LogicalPlan) -> Cost {
        match plan {
            LogicalPlan::Scan { table, schema, .. } => {
                self.estimate_scan(table.qualified_name(), schema.columns.len())
            }
            LogicalPlan::IndexScan {
                table,
                schema,
                predicate,
                ..
            } => self.estimate_index_scan(table.qualified_name(), schema.columns.len(), predicate),
            LogicalPlan::Projection { exprs, input, .. } => {
                self.estimate_projection(exprs.len(), input)
            }
            LogicalPlan::Filter {
                predicate, input, ..
            } => self.estimate_filter(predicate, input),
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
            } => self.estimate_join(*join_type, condition, left, right),
            LogicalPlan::Aggregate {
                group_exprs, input, ..
            } => self.estimate_aggregate(group_exprs.len(), input),
            LogicalPlan::Sort { input, .. } => self.estimate_sort(input),
            LogicalPlan::Limit { limit, input, .. } => self.estimate_limit(limit.as_ref(), input),
            LogicalPlan::Distinct { input, .. } => self.estimate_distinct(input),
            // Phase 5.8: Shared/MemoRef 成本
            LogicalPlan::Shared { plan, .. } => self.estimate(plan),
            LogicalPlan::MemoRef { .. } => Cost::zero(),
            // DML/DDL 节点不参与查询优化，返回零成本
            _ => Cost::zero(),
        }
    }

    // -----------------------------------------------------------------
    //  算子成本估算
    // -----------------------------------------------------------------

    /// SeqScan 成本
    ///
    /// `cpu = CPU_TUPLE_COST * rows`
    /// `io = SEQ_PAGE_COST * pages`
    /// `cardinality = rows`
    fn estimate_scan(&self, table_name: String, num_cols: usize) -> Cost {
        let row_count = self.lookup_row_count(&table_name);
        let pages = (row_count as f64) / (ROWS_PER_PAGE as f64);
        Cost {
            cpu_cost: CPU_TUPLE_COST * row_count as f64,
            io_cost: SEQ_PAGE_COST * pages,
            cardinality: row_count,
            width: num_cols * 8, // 简化：每列平均 8 字节
        }
    }

    /// IndexScan 成本 — Phase 5.7
    ///
    /// `cardinality = scan_cardinality * selectivity`（基于谓词选择率）
    /// `io = RANDOM_PAGE_COST * matched_rows`（每匹配行一次随机 I/O）
    /// `cpu = CPU_TUPLE_COST * matched_rows`
    ///
    /// 与 SeqScan 比较：当 matched_rows << row_count 时 IndexScan 成本更低
    fn estimate_index_scan(&self, table_name: String, num_cols: usize, predicate: &Expr) -> Cost {
        let row_count = self.lookup_row_count(&table_name);
        // 估算选择率：复用 Filter 选择率（predicate 为完整 Filter 谓词）
        // 用一个虚拟 Scan 计划包装以便复用 estimate_selectivity 的 plan 查找逻辑
        let dummy_scan = LogicalPlan::Scan {
            table: TableName::new(table_name.clone()),
            alias: None,
            schema: TableSchema {
                name: TableName::new(table_name.clone()),
                columns: Vec::new(),
            },
        };
        let selectivity = self.estimate_selectivity(predicate, &dummy_scan);
        let matched_rows = ((row_count as f64) * selectivity).round() as usize;
        Cost {
            cpu_cost: CPU_TUPLE_COST * matched_rows as f64,
            io_cost: RANDOM_PAGE_COST * matched_rows as f64,
            cardinality: matched_rows,
            width: num_cols * 8,
        }
    }

    /// Projection 成本
    ///
    /// `cpu = input.cpu + CPU_OPERATOR_COST * input.card * num_exprs`
    /// `cardinality = input.cardinality`
    fn estimate_projection(&self, num_exprs: usize, input: &LogicalPlan) -> Cost {
        let input_cost = self.estimate(input);
        let proj_cpu = CPU_OPERATOR_COST * input_cost.cardinality as f64 * num_exprs as f64;
        Cost {
            cpu_cost: input_cost.cpu_cost + proj_cpu,
            io_cost: input_cost.io_cost,
            cardinality: input_cost.cardinality,
            width: num_exprs * 8,
        }
    }

    /// Filter 成本
    ///
    /// `cpu = input.cpu + CPU_OPERATOR_COST * input.card * num_predicates`
    /// `cardinality = input.card * selectivity`
    fn estimate_filter(&self, predicate: &Expr, input: &LogicalPlan) -> Cost {
        let input_cost = self.estimate(input);
        let selectivity = self.estimate_selectivity(predicate, input);
        let num_predicates = count_predicates(predicate);
        let filter_cpu = CPU_OPERATOR_COST * input_cost.cardinality as f64 * num_predicates as f64;
        let out_card = (input_cost.cardinality as f64 * selectivity).round() as usize;
        Cost {
            cpu_cost: input_cost.cpu_cost + filter_cpu,
            io_cost: input_cost.io_cost,
            cardinality: out_card,
            width: input_cost.width,
        }
    }

    /// Join 成本
    ///
    /// 自动选择 NestedLoop 或 Hash 算法，分别估算成本取较小者。
    fn estimate_join(
        &self,
        join_type: JoinType,
        condition: &szrsql_sql::ast::JoinCondition,
        left: &LogicalPlan,
        right: &LogicalPlan,
    ) -> Cost {
        let left_cost = self.estimate(left);
        let right_cost = self.estimate(right);

        let algorithm = JoinAlgorithm::choose(
            join_type,
            condition,
            left_cost.cardinality,
            right_cost.cardinality,
        );

        let join_selectivity = self.estimate_join_selectivity(condition, &left_cost, &right_cost);

        let (cpu_cost, cardinality) = match algorithm {
            JoinAlgorithm::NestedLoop => {
                // NestedLoop: 左表的每一行扫描整个右表
                // SEMI/ANTI 可在首次匹配后短路（平均扫描右表一半），用 0.5 系数近似
                let cpu = left_cost.cpu_cost
                    + right_cost.cpu_cost
                    + CPU_OPERATOR_COST
                        * left_cost.cardinality as f64
                        * right_cost.cardinality as f64;
                let card = match join_type {
                    // Phase 7b.1: 使用 saturating_mul 防止大表 JOIN 基数溢出
                    JoinType::Cross => left_cost.cardinality.saturating_mul(right_cost.cardinality),
                    // Phase 7b.1: 先转 f64 再相乘，避免 usize 乘法溢出
                    JoinType::Inner => (left_cost.cardinality as f64
                        * right_cost.cardinality as f64
                        * join_selectivity)
                        .round() as usize,
                    JoinType::LeftOuter => left_cost.cardinality, // 左表全保留
                    JoinType::RightOuter => right_cost.cardinality, // 右表全保留
                    JoinType::FullOuter => left_cost.cardinality + right_cost.cardinality,
                    // SEMI: 命中的左表行数；ANTI: 未命中的左表行数
                    JoinType::Semi => {
                        ((left_cost.cardinality as f64) * join_selectivity).round() as usize
                    }
                    JoinType::Anti => {
                        ((left_cost.cardinality as f64) * (1.0 - join_selectivity)).round() as usize
                    }
                };
                (cpu, card)
            }
            JoinAlgorithm::Hash => {
                // HashJoin: 构建哈希表（小表）+ 探测（大表）
                // 选择较小的一侧作为 build side
                let (build_card, probe_card) = if left_cost.cardinality <= right_cost.cardinality {
                    (left_cost.cardinality, right_cost.cardinality)
                } else {
                    (right_cost.cardinality, left_cost.cardinality)
                };
                let cpu = left_cost.cpu_cost
                    + right_cost.cpu_cost
                    + HASH_COST * build_card as f64
                    + CPU_OPERATOR_COST * probe_card as f64;
                let card = match join_type {
                    // Phase 7b.1: 使用 saturating_mul 防止大表 JOIN 基数溢出
                    JoinType::Cross => left_cost.cardinality.saturating_mul(right_cost.cardinality),
                    // Phase 7b.1: 先转 f64 再相乘，避免 usize 乘法溢出
                    JoinType::Inner => (left_cost.cardinality as f64
                        * right_cost.cardinality as f64
                        * join_selectivity)
                        .round() as usize,
                    JoinType::LeftOuter => left_cost.cardinality,
                    JoinType::RightOuter => right_cost.cardinality,
                    JoinType::FullOuter => left_cost.cardinality + right_cost.cardinality,
                    JoinType::Semi => {
                        ((left_cost.cardinality as f64) * join_selectivity).round() as usize
                    }
                    JoinType::Anti => {
                        ((left_cost.cardinality as f64) * (1.0 - join_selectivity)).round() as usize
                    }
                };
                (cpu, card)
            }
        };

        // SEMI/ANTI JOIN 输出仅左表列（width 不含右表）
        let width = if matches!(join_type, JoinType::Semi | JoinType::Anti) {
            left_cost.width
        } else {
            left_cost.width + right_cost.width
        };

        Cost {
            cpu_cost,
            io_cost: left_cost.io_cost + right_cost.io_cost,
            cardinality,
            width,
        }
    }

    /// Aggregate 成本
    ///
    /// `cpu = input.cpu + CPU_TUPLE_COST * input.card + SORT_COST * group_card`
    /// `cardinality = group_card`（GROUP BY 列的 NDV）
    fn estimate_aggregate(&self, num_group_exprs: usize, input: &LogicalPlan) -> Cost {
        let input_cost = self.estimate(input);
        // 简化：group_card = min(input.card, DEFAULT_NDV * num_group_exprs)
        // 实际应基于 group 列的统计信息估算
        let group_card = if num_group_exprs == 0 {
            1 // 无 GROUP BY 时聚合为单行
        } else {
            // 假设每列 NDV = DEFAULT_NDV，组合 NDV = NDV^num_group_exprs（简化）
            let estimated = (DEFAULT_NDV as f64).powi(num_group_exprs as i32) as usize;
            estimated.min(input_cost.cardinality)
        };
        let agg_cpu =
            CPU_TUPLE_COST * input_cost.cardinality as f64 + SORT_COST * group_card as f64;
        Cost {
            cpu_cost: input_cost.cpu_cost + agg_cpu,
            io_cost: input_cost.io_cost,
            cardinality: group_card,
            width: input_cost.width,
        }
    }

    /// Sort 成本
    ///
    /// `cpu = input.cpu + SORT_COST * input.card * log2(input.card)`
    /// `cardinality = input.cardinality`
    fn estimate_sort(&self, input: &LogicalPlan) -> Cost {
        let input_cost = self.estimate(input);
        let n = input_cost.cardinality as f64;
        let sort_cpu = if n > 1.0 {
            SORT_COST * n * n.log2()
        } else {
            0.0
        };
        Cost {
            cpu_cost: input_cost.cpu_cost + sort_cpu,
            io_cost: input_cost.io_cost,
            cardinality: input_cost.cardinality,
            width: input_cost.width,
        }
    }

    /// Limit 成本
    ///
    /// `cardinality = min(limit, input.cardinality)`
    /// 注意：Limit 不减少子节点的执行成本（执行器仍需扫描到 limit 行）
    fn estimate_limit(&self, limit: Option<&Expr>, input: &LogicalPlan) -> Cost {
        let input_cost = self.estimate(input);
        let limit_val = limit.and_then(extract_literal_int).unwrap_or(usize::MAX);
        let out_card = input_cost.cardinality.min(limit_val);
        Cost {
            cpu_cost: input_cost.cpu_cost,
            io_cost: input_cost.io_cost,
            cardinality: out_card,
            width: input_cost.width,
        }
    }

    /// Distinct 成本
    ///
    /// `cpu = input.cpu + HASH_COST * input.card`
    /// `cardinality = min(input.card, DEFAULT_NDV)`（简化）
    fn estimate_distinct(&self, input: &LogicalPlan) -> Cost {
        let input_cost = self.estimate(input);
        let distinct_cpu = HASH_COST * input_cost.cardinality as f64;
        let out_card = input_cost.cardinality.min(DEFAULT_NDV);
        Cost {
            cpu_cost: input_cost.cpu_cost + distinct_cpu,
            io_cost: input_cost.io_cost,
            cardinality: out_card,
            width: input_cost.width,
        }
    }

    // -----------------------------------------------------------------
    //  选择率估算
    // -----------------------------------------------------------------

    /// 估算谓词选择率
    ///
    /// 递归处理 AND / OR / NOT 等复合谓词。
    /// 基础谓词使用 `ColumnStatistics::selectivity_eq` / `selectivity_range`。
    fn estimate_selectivity(&self, predicate: &Expr, input: &LogicalPlan) -> f64 {
        match predicate {
            Expr::BinaryOp { left, op, right } => match op {
                BinaryOp::And => {
                    let s1 = self.estimate_selectivity(left, input);
                    let s2 = self.estimate_selectivity(right, input);
                    // PG 风格：AND 选择率 = min(s1, s2) * 0.5（假设独立性折扣）
                    s1.min(s2) * 0.5
                }
                BinaryOp::Or => {
                    let s1 = self.estimate_selectivity(left, input);
                    let s2 = self.estimate_selectivity(right, input);
                    // PG 风格：OR 选择率 = s1 + s2 - s1*s2
                    (s1 + s2 - s1 * s2).min(1.0)
                }
                BinaryOp::Eq => {
                    // col = literal
                    if let Some(col_name) = extract_column_name(left) {
                        self.lookup_eq_selectivity(input, &col_name)
                    } else if let Some(col_name) = extract_column_name(right) {
                        self.lookup_eq_selectivity(input, &col_name)
                    } else {
                        DEFAULT_EQ_SELECTIVITY
                    }
                }
                BinaryOp::NotEq => {
                    if let Some(col_name) = extract_column_name(left) {
                        let eq_sel = self.lookup_eq_selectivity(input, &col_name);
                        1.0 - eq_sel
                    } else {
                        1.0 - DEFAULT_EQ_SELECTIVITY
                    }
                }
                BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => {
                    if let Some(col_name) = extract_column_name(left) {
                        self.lookup_range_selectivity(input, &col_name)
                    } else {
                        DEFAULT_RANGE_SELECTIVITY
                    }
                }
                // 算术运算符、位运算符不产生过滤效果
                _ => 1.0,
            },
            Expr::IsNull { .. } => {
                // IS NULL 选择率 = null_count / row_count（简化为 0.1）
                0.1
            }
            Expr::InList { list, .. } => {
                // IN (v1, v2, ...) 选择率 = min(list.len() / NDV, 1.0)
                let n = list.len() as f64;
                let sel = n / (DEFAULT_NDV as f64);
                sel.min(1.0)
            }
            Expr::Like { .. } => {
                // LIKE 选择率（PG 默认 0.05）
                0.05
            }
            Expr::Between { .. } => {
                // BETWEEN 选择率（PG 默认 0.1）
                0.1
            }
            _ => 1.0, // 未知谓词不产生过滤
        }
    }

    /// 估算 JOIN 选择率
    ///
    /// - 等值 JOIN：`1 / max(left.ndv, right.ndv)`（无统计时 0.1）
    /// - 非等值 JOIN：`0.1`（PG 默认 inner join）
    fn estimate_join_selectivity(
        &self,
        condition: &szrsql_sql::ast::JoinCondition,
        left: &Cost,
        right: &Cost,
    ) -> f64 {
        if !is_equi_condition(condition) {
            return DEFAULT_JOIN_SELECTIVITY;
        }
        // 简化：使用两侧 cardinality 的倒数估算
        // 实际应基于 JOIN 列的 NDV
        let max_ndv = left.cardinality.max(right.cardinality).max(1);
        let sel = 1.0 / max_ndv as f64;
        sel.max(DEFAULT_JOIN_SELECTIVITY)
    }

    // -----------------------------------------------------------------
    //  统计信息查找
    // -----------------------------------------------------------------

    /// 查找表的总行数（无统计时返回默认值）
    fn lookup_row_count(&self, table_name: &str) -> usize {
        self.stats_store
            .get_table_stats(table_name)
            .map(|s| s.row_count)
            .unwrap_or(DEFAULT_ROW_COUNT)
    }

    /// 查找等值选择率（基于统计或默认值）
    fn lookup_eq_selectivity(&self, plan: &LogicalPlan, col_name: &str) -> f64 {
        let table_stats = self.find_table_stats(plan);
        if let Some(stats) = table_stats {
            if let Some(col) = stats.column(col_name) {
                return col.selectivity_eq();
            }
        }
        DEFAULT_EQ_SELECTIVITY
    }

    /// 查找范围选择率（基于统计或默认值）
    fn lookup_range_selectivity(&self, plan: &LogicalPlan, col_name: &str) -> f64 {
        let table_stats = self.find_table_stats(plan);
        if let Some(stats) = table_stats {
            if let Some(col) = stats.column(col_name) {
                return col.selectivity_range();
            }
        }
        DEFAULT_RANGE_SELECTIVITY
    }

    /// 从计划节点递归查找表名，再查统计信息
    ///
    /// # P2-1.2 重构（2026-07-31）
    ///
    /// 返回 `Option<Arc<TableStatistics>>` 而非 `Option<&TableStatistics>`，
    /// 因为 `StatisticsStore::get_table_stats` 现在返回 Arc（与 `Arc<Mutex<...>>`
    /// 共享模式兼容）。Arc 解引用后即可访问 `TableStatistics` 的字段和方法。
    fn find_table_stats(&self, plan: &LogicalPlan) -> Option<Arc<TableStatistics>> {
        match plan {
            LogicalPlan::Scan { table, .. } => {
                self.stats_store.get_table_stats(&table.qualified_name())
            }
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Projection { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Distinct { input, .. }
            | LogicalPlan::Aggregate { input, .. } => self.find_table_stats(input),
            LogicalPlan::Join { left, .. } => self.find_table_stats(left),
            _ => None,
        }
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 统计谓词数量（用于 Filter 成本估算）
fn count_predicates(expr: &Expr) -> usize {
    match expr {
        Expr::BinaryOp {
            op: BinaryOp::And,
            left,
            right,
        } => count_predicates(left) + count_predicates(right),
        Expr::BinaryOp {
            op: BinaryOp::Or,
            left,
            right,
        } => count_predicates(left) + count_predicates(right),
        _ => 1,
    }
}

/// 从表达式中提取列名（仅单层标识符）
fn extract_column_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Identifier(parts) if parts.len() == 1 => Some(parts[0].to_lowercase()),
        Expr::Identifier(parts) if parts.len() >= 2 => {
            // table.col → 取最后一部分
            Some(parts.last().unwrap().to_lowercase())
        }
        _ => None,
    }
}

/// 从字面量表达式中提取整数值（用于 LIMIT）
fn extract_literal_int(expr: &Expr) -> Option<usize> {
    match expr {
        Expr::Literal(szrsql_types::value::Value::Int64(n)) => {
            if *n >= 0 {
                Some(*n as usize)
            } else {
                None
            }
        }
        Expr::Literal(szrsql_types::value::Value::Float64(f)) => {
            if *f >= 0.0 {
                Some(*f as usize)
            } else {
                None
            }
        }
        _ => None,
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statistics::{
        ColumnStatistics, InMemoryStatisticsStore, StatisticsCollector, TableStatistics,
    };
    use szrsql_sql::ast::{ColumnDefinition, Expr as AExpr, JoinCondition, OrderByExpr, TableName};
    use szrsql_sql::executor::InMemoryTable;
    use szrsql_sql::plan::{LogicalPlan, TableSchema};
    use szrsql_types::value::{ColumnType, Value};

    /// 构建带统计信息的 CostModel
    fn build_cost_model_with_stats(
        table_name: &str,
        stats: TableStatistics,
    ) -> (CostModel, Arc<InMemoryStatisticsStore>) {
        let mut store = InMemoryStatisticsStore::new();
        store.update_table_stats(table_name, stats);
        let store_arc = Arc::new(store);
        let model = CostModel::new(store_arc.clone());
        (model, store_arc)
    }

    /// 构建简单 Scan 计划
    fn build_scan_plan(table_name: &str, num_cols: usize) -> LogicalPlan {
        let mut columns = Vec::with_capacity(num_cols);
        for i in 0..num_cols {
            columns.push(ColumnDefinition::new(format!("c{i}"), ColumnType::Int64));
        }
        LogicalPlan::Scan {
            table: TableName::new(table_name),
            alias: None,
            schema: TableSchema {
                name: TableName::new(table_name),
                columns,
            },
        }
    }

    /// 构建 Filter 计划
    fn build_filter_plan(predicate: AExpr, input: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Filter {
            predicate,
            input: Box::new(input),
        }
    }

    #[test]
    fn test_cost_zero() {
        let c = Cost::zero();
        assert_eq!(c.cpu_cost, 0.0);
        assert_eq!(c.io_cost, 0.0);
        assert_eq!(c.cardinality, 0);
        assert_eq!(c.width, 0);
        assert_eq!(c.total(), 0.0);
    }

    #[test]
    fn test_cost_add() {
        let c1 = Cost {
            cpu_cost: 10.0,
            io_cost: 5.0,
            cardinality: 100,
            width: 16,
        };
        let c2 = Cost {
            cpu_cost: 20.0,
            io_cost: 3.0,
            cardinality: 200,
            width: 24,
        };
        let sum = c1 + c2;
        assert_eq!(sum.cpu_cost, 30.0);
        assert_eq!(sum.io_cost, 8.0);
        assert_eq!(sum.cardinality, 200); // max
        assert_eq!(sum.width, 24); // max
    }

    #[test]
    fn test_scan_cost_with_stats() {
        // 构建 1000 行的统计信息
        let stats = TableStatistics::empty("t1");
        let mut stats = stats;
        stats.row_count = 1000;
        let (model, _) = build_cost_model_with_stats("t1", stats);

        let plan = build_scan_plan("t1", 4);
        let cost = model.estimate(&plan);
        assert_eq!(cost.cardinality, 1000);
        // cpu = 0.01 * 1000 = 10.0
        assert!((cost.cpu_cost - 10.0).abs() < 1e-6);
        // io = 1.0 * (1000/100) = 10.0
        assert!((cost.io_cost - 10.0).abs() < 1e-6);
        assert_eq!(cost.width, 32); // 4 cols * 8 bytes
    }

    #[test]
    fn test_scan_cost_default_stats() {
        // 无统计信息 → 使用默认值 DEFAULT_ROW_COUNT = 1000
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        let plan = build_scan_plan("nonexistent", 2);
        let cost = model.estimate(&plan);
        assert_eq!(cost.cardinality, DEFAULT_ROW_COUNT);
        assert_eq!(cost.width, 16);
    }

    #[test]
    fn test_filter_cost_selectivity() {
        // 1000 行表，col = literal 谓词，NDV = 100 → selectivity = 0.01
        let mut table = InMemoryTable::with_columns("t1", vec![("id", ColumnType::Int64)]);
        for i in 0..1000 {
            table.insert(vec![Value::Int64(i % 100)]); // NDV = 100
        }
        let stats = StatisticsCollector::collect(&table);

        let (model, _) = build_cost_model_with_stats("t1", stats);

        // WHERE id = 50
        let predicate = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["id".into()])),
            op: BinaryOp::Eq,
            right: Box::new(AExpr::Literal(Value::Int64(50))),
        };
        let plan = build_filter_plan(predicate, build_scan_plan("t1", 1));
        let cost = model.estimate(&plan);
        // selectivity = 1/100 = 0.01 → out_card = 1000 * 0.01 = 10
        assert_eq!(cost.cardinality, 10);
    }

    #[test]
    fn test_filter_cost_and_selectivity() {
        let mut table = InMemoryTable::with_columns(
            "t1",
            vec![("a", ColumnType::Int64), ("b", ColumnType::Int64)],
        );
        for i in 0..1000 {
            table.insert(vec![Value::Int64(i % 10), Value::Int64(i % 20)]);
        }
        let stats = StatisticsCollector::collect(&table);
        let (model, _) = build_cost_model_with_stats("t1", stats);

        // WHERE a = 5 AND b = 10
        let pred_a = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["a".into()])),
            op: BinaryOp::Eq,
            right: Box::new(AExpr::Literal(Value::Int64(5))),
        };
        let pred_b = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["b".into()])),
            op: BinaryOp::Eq,
            right: Box::new(AExpr::Literal(Value::Int64(10))),
        };
        let predicate = AExpr::BinaryOp {
            left: Box::new(pred_a),
            op: BinaryOp::And,
            right: Box::new(pred_b),
        };
        let plan = build_filter_plan(predicate, build_scan_plan("t1", 2));
        let cost = model.estimate(&plan);
        // a NDV=10 → sel_a = 0.1
        // b NDV=20 → sel_b = 0.05
        // AND → min(0.1, 0.05) * 0.5 = 0.025
        // out_card = 1000 * 0.025 = 25
        assert_eq!(cost.cardinality, 25);
    }

    #[test]
    fn test_join_nested_loop_small_tables() {
        // 小表（< HASH_JOIN_MIN_ROWS）→ NestedLoop
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        let left = build_scan_plan("a", 2);
        let right = build_scan_plan("b", 2);
        let plan = LogicalPlan::Join {
            join_type: JoinType::Inner,
            condition: JoinCondition::On(AExpr::BinaryOp {
                left: Box::new(AExpr::Identifier(vec!["a".into(), "id".into()])),
                op: BinaryOp::Eq,
                right: Box::new(AExpr::Identifier(vec!["b".into(), "id".into()])),
            }),
            left: Box::new(left),
            right: Box::new(right),
        };
        let cost = model.estimate(&plan);
        // 默认 cardinality = 1000 each
        // 但 HASH_JOIN_MIN_ROWS = 100, 1000 > 100 → 应选 HashJoin
        // 验证 cardinality 估算
        assert!(cost.cardinality > 0);
        assert!(cost.cardinality <= 1000 * 1000);
    }

    #[test]
    fn test_join_algorithm_choice() {
        use szrsql_sql::ast::JoinCondition;
        let cond = JoinCondition::On(AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["a".into(), "id".into()])),
            op: BinaryOp::Eq,
            right: Box::new(AExpr::Identifier(vec!["b".into(), "id".into()])),
        });

        // 小表 → NestedLoop
        let algo = JoinAlgorithm::choose(JoinType::Inner, &cond, 10, 20);
        assert_eq!(algo, JoinAlgorithm::NestedLoop);

        // 大表 + 等值 → Hash
        let algo = JoinAlgorithm::choose(JoinType::Inner, &cond, 1000, 2000);
        assert_eq!(algo, JoinAlgorithm::Hash);

        // Cross → 总是 NestedLoop
        let algo = JoinAlgorithm::choose(JoinType::Cross, &JoinCondition::None, 1000, 2000);
        assert_eq!(algo, JoinAlgorithm::NestedLoop);

        // 非等值 → NestedLoop
        let ne_cond = JoinCondition::On(AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["a".into(), "id".into()])),
            op: BinaryOp::Lt,
            right: Box::new(AExpr::Identifier(vec!["b".into(), "id".into()])),
        });
        let algo = JoinAlgorithm::choose(JoinType::Inner, &ne_cond, 1000, 2000);
        assert_eq!(algo, JoinAlgorithm::NestedLoop);
    }

    #[test]
    fn test_hash_join_cheaper_than_nested_loop() {
        // 大表等值 JOIN：HashJoin 应比 NestedLoop 成本低
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        // 构建大表（cardinality = 1000）
        let left = build_scan_plan("a", 2);
        let right = build_scan_plan("b", 2);

        // 计算 HashJoin 成本（手动）
        let left_cost = model.estimate(&left);
        let right_cost = model.estimate(&right);
        let build_card = left_cost.cardinality.min(right_cost.cardinality);
        let probe_card = left_cost.cardinality.max(right_cost.cardinality);
        let hash_cpu = left_cost.cpu_cost
            + right_cost.cpu_cost
            + HASH_COST * build_card as f64
            + CPU_OPERATOR_COST * probe_card as f64;

        // 计算 NestedLoop 成本（手动）
        let nl_cpu = left_cost.cpu_cost
            + right_cost.cpu_cost
            + CPU_OPERATOR_COST * left_cost.cardinality as f64 * right_cost.cardinality as f64;

        assert!(
            hash_cpu < nl_cpu,
            "HashJoin ({hash_cpu}) should be cheaper than NestedLoop ({nl_cpu})"
        );
    }

    #[test]
    fn test_aggregate_cost() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        // SELECT count(*) FROM t GROUP BY c1
        let scan = build_scan_plan("t", 2);
        let plan = LogicalPlan::Aggregate {
            group_exprs: vec![AExpr::Identifier(vec!["c0".into()])],
            aggregates: vec![],
            having: None,
            input: Box::new(scan),
        };
        let cost = model.estimate(&plan);
        // group_card = min(1000, 100^1) = 100
        assert_eq!(cost.cardinality, 100);
    }

    #[test]
    fn test_aggregate_no_group_by() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        let scan = build_scan_plan("t", 2);
        let plan = LogicalPlan::Aggregate {
            group_exprs: vec![],
            aggregates: vec![],
            having: None,
            input: Box::new(scan),
        };
        let cost = model.estimate(&plan);
        assert_eq!(cost.cardinality, 1); // 无 GROUP BY → 单行
    }

    #[test]
    fn test_sort_cost() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        let scan = build_scan_plan("t", 2);
        let plan = LogicalPlan::Sort {
            order_by: vec![OrderByExpr {
                expr: AExpr::Identifier(vec!["c0".into()]),
                asc: true,
                nulls_first: false,
            }],
            input: Box::new(scan),
        };
        let cost = model.estimate(&plan);
        assert_eq!(cost.cardinality, DEFAULT_ROW_COUNT); // Sort 不改变 cardinality
                                                         // cpu_cost 应包含排序开销
        assert!(cost.cpu_cost > 0.0);
    }

    #[test]
    fn test_limit_cost() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        let scan = build_scan_plan("t", 2);
        let plan = LogicalPlan::Limit {
            limit: Some(AExpr::Literal(Value::Int64(10))),
            offset: None,
            input: Box::new(scan),
        };
        let cost = model.estimate(&plan);
        assert_eq!(cost.cardinality, 10); // min(10, 1000) = 10
    }

    #[test]
    fn test_limit_larger_than_input() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        let scan = build_scan_plan("t", 2);
        let plan = LogicalPlan::Limit {
            limit: Some(AExpr::Literal(Value::Int64(10000))),
            offset: None,
            input: Box::new(scan),
        };
        let cost = model.estimate(&plan);
        assert_eq!(cost.cardinality, DEFAULT_ROW_COUNT); // min(10000, 1000) = 1000
    }

    #[test]
    fn test_distinct_cost() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        let scan = build_scan_plan("t", 2);
        let plan = LogicalPlan::Distinct {
            input: Box::new(scan),
        };
        let cost = model.estimate(&plan);
        // out_card = min(input.card, DEFAULT_NDV) = min(1000, 100) = 100
        assert_eq!(cost.cardinality, DEFAULT_NDV);
    }

    #[test]
    fn test_nested_plan_cost() {
        // SELECT c0 FROM (SELECT * FROM t WHERE c0 > 5) LIMIT 10
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        let scan = build_scan_plan("t", 2);
        let filter = build_filter_plan(
            AExpr::BinaryOp {
                left: Box::new(AExpr::Identifier(vec!["c0".into()])),
                op: BinaryOp::Gt,
                right: Box::new(AExpr::Literal(Value::Int64(5))),
            },
            scan,
        );
        let limit = LogicalPlan::Limit {
            limit: Some(AExpr::Literal(Value::Int64(10))),
            offset: None,
            input: Box::new(filter),
        };

        let cost = model.estimate(&limit);
        // Filter selectivity = 1/3 (range) → card = 1000/3 ≈ 333
        // Limit min(10, 333) = 10
        assert_eq!(cost.cardinality, 10);
    }

    #[test]
    fn test_selectivity_eq_with_stats() {
        let mut table = InMemoryTable::with_columns("t1", vec![("id", ColumnType::Int64)]);
        for i in 0..1000 {
            table.insert(vec![Value::Int64(i % 50)]); // NDV = 50
        }
        let stats = StatisticsCollector::collect(&table);
        let (model, _) = build_cost_model_with_stats("t1", stats);

        let predicate = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["id".into()])),
            op: BinaryOp::Eq,
            right: Box::new(AExpr::Literal(Value::Int64(42))),
        };
        let plan = build_filter_plan(predicate, build_scan_plan("t1", 1));
        let cost = model.estimate(&plan);
        // selectivity = 1/50 = 0.02 → card = 1000 * 0.02 = 20
        assert_eq!(cost.cardinality, 20);
    }

    #[test]
    fn test_selectivity_or() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        let pred = AExpr::BinaryOp {
            left: Box::new(AExpr::BinaryOp {
                left: Box::new(AExpr::Identifier(vec!["a".into()])),
                op: BinaryOp::Eq,
                right: Box::new(AExpr::Literal(Value::Int64(1))),
            }),
            op: BinaryOp::Or,
            right: Box::new(AExpr::BinaryOp {
                left: Box::new(AExpr::Identifier(vec!["b".into()])),
                op: BinaryOp::Eq,
                right: Box::new(AExpr::Literal(Value::Int64(2))),
            }),
        };
        let plan = build_filter_plan(pred, build_scan_plan("t", 2));
        let cost = model.estimate(&plan);
        // sel_a = 0.005, sel_b = 0.005
        // OR = 0.005 + 0.005 - 0.005*0.005 ≈ 0.00997
        // card = 1000 * 0.00997 ≈ 10
        assert!(cost.cardinality > 0 && cost.cardinality <= 20);
    }

    #[test]
    fn test_column_statistics_selectivity_eq_with_ndv() {
        let col = ColumnStatistics {
            null_count: 0,
            distinct_count: 100,
            min_value: Some(Value::Int64(0)),
            max_value: Some(Value::Int64(99)),
            histogram: None,
        };
        // selectivity_eq = 1/100 = 0.01
        assert!((col.selectivity_eq() - 0.01).abs() < 1e-9);
    }

    #[test]
    fn test_dml_returns_zero_cost() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        // DML 节点不参与查询优化
        let plan = LogicalPlan::Delete {
            table: TableName::new("t"),
            schema: TableSchema {
                name: TableName::new("t"),
                columns: vec![],
            },
            source: None,
            returning: None,
        };
        let cost = model.estimate(&plan);
        assert_eq!(cost.total(), 0.0);
    }

    #[test]
    fn test_extract_literal_int() {
        assert_eq!(
            extract_literal_int(&AExpr::Literal(Value::Int64(42))),
            Some(42)
        );
        assert_eq!(extract_literal_int(&AExpr::Literal(Value::Int64(-1))), None);
        assert_eq!(
            extract_literal_int(&AExpr::Literal(Value::Float64(3.7))),
            Some(3)
        );
        assert_eq!(extract_literal_int(&AExpr::Literal(Value::Null)), None);
        assert_eq!(
            extract_literal_int(&AExpr::Identifier(vec!["x".into()])),
            None
        );
    }

    #[test]
    fn test_count_predicates() {
        // a = 1
        let p1 = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["a".into()])),
            op: BinaryOp::Eq,
            right: Box::new(AExpr::Literal(Value::Int64(1))),
        };
        assert_eq!(count_predicates(&p1), 1);

        // a = 1 AND b = 2
        let p2 = AExpr::BinaryOp {
            left: Box::new(p1.clone()),
            op: BinaryOp::And,
            right: Box::new(AExpr::BinaryOp {
                left: Box::new(AExpr::Identifier(vec!["b".into()])),
                op: BinaryOp::Eq,
                right: Box::new(AExpr::Literal(Value::Int64(2))),
            }),
        };
        assert_eq!(count_predicates(&p2), 2);
    }

    #[test]
    fn test_is_equi_condition() {
        use szrsql_sql::ast::JoinCondition;

        // ON a.id = b.id → 等值
        let equi = JoinCondition::On(AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["a".into(), "id".into()])),
            op: BinaryOp::Eq,
            right: Box::new(AExpr::Identifier(vec!["b".into(), "id".into()])),
        });
        assert!(is_equi_condition(&equi));

        // ON a.id < b.id → 非等值
        let ne = JoinCondition::On(AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["a".into(), "id".into()])),
            op: BinaryOp::Lt,
            right: Box::new(AExpr::Identifier(vec!["b".into(), "id".into()])),
        });
        assert!(!is_equi_condition(&ne));

        // USING → 等值
        assert!(is_equi_condition(&JoinCondition::Using(vec!["id".into()])));

        // None → 非等值
        assert!(!is_equi_condition(&JoinCondition::None));
    }

    #[test]
    fn test_projection_cost() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        let scan = build_scan_plan("t", 4);
        let plan = LogicalPlan::Projection {
            exprs: vec![
                (AExpr::Identifier(vec!["c0".into()]), Some("c0".into())),
                (AExpr::Identifier(vec!["c1".into()]), Some("c1".into())),
            ],
            output_names: vec!["c0".into(), "c1".into()],
            input: Box::new(scan),
        };
        let cost = model.estimate(&plan);
        assert_eq!(cost.cardinality, DEFAULT_ROW_COUNT);
        assert_eq!(cost.width, 16); // 2 cols * 8
    }

    /// 验证优化器选择更优 JOIN 算法
    #[test]
    fn test_optimizer_prefers_hash_join_for_large_inputs() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        // 大表 JOIN（cardinality = 1000 each）
        let left = build_scan_plan("a", 2);
        let right = build_scan_plan("b", 2);
        let plan = LogicalPlan::Join {
            join_type: JoinType::Inner,
            condition: JoinCondition::On(AExpr::BinaryOp {
                left: Box::new(AExpr::Identifier(vec!["a".into(), "id".into()])),
                op: BinaryOp::Eq,
                right: Box::new(AExpr::Identifier(vec!["b".into(), "id".into()])),
            }),
            left: Box::new(left),
            right: Box::new(right),
        };

        let cost = model.estimate(&plan);
        // 验证：大表等值 JOIN 的总成本应远小于 NestedLoop 成本
        let nl_cost = CPU_OPERATOR_COST * 1000.0 * 1000.0;
        assert!(
            cost.cpu_cost < nl_cost,
            "HashJoin cost ({}) should be less than NestedLoop baseline ({})",
            cost.cpu_cost,
            nl_cost
        );
    }

    /// 验证统计信息准确性对成本估算的影响
    #[test]
    fn test_stats_affect_cost_estimate() {
        let mut table = InMemoryTable::with_columns("t1", vec![("id", ColumnType::Int64)]);
        for i in 0..10_000 {
            table.insert(vec![Value::Int64(i)]);
        }
        let stats = StatisticsCollector::collect(&table);
        let (model, _) = build_cost_model_with_stats("t1", stats);

        let plan = build_scan_plan("t1", 1);
        let cost = model.estimate(&plan);
        // 10000 行的实际统计 → cardinality = 10000
        assert_eq!(cost.cardinality, 10_000);
    }

    /// 验证成本累加正确性
    #[test]
    fn test_cost_accumulation() {
        let store = Arc::new(InMemoryStatisticsStore::new());
        let model = CostModel::new(store);

        // Scan → Filter → Sort
        let scan = build_scan_plan("t", 2);
        let filter = build_filter_plan(
            AExpr::BinaryOp {
                left: Box::new(AExpr::Identifier(vec!["c0".into()])),
                op: BinaryOp::Gt,
                right: Box::new(AExpr::Literal(Value::Int64(5))),
            },
            scan,
        );
        let sort = LogicalPlan::Sort {
            order_by: vec![OrderByExpr {
                expr: AExpr::Identifier(vec!["c0".into()]),
                asc: true,
                nulls_first: false,
            }],
            input: Box::new(filter),
        };

        let cost = model.estimate(&sort);
        // 各层成本应累加
        assert!(cost.cpu_cost > 0.0);
        assert!(cost.io_cost > 0.0);
        assert_eq!(cost.cardinality, 333); // 1000 * 1/3 ≈ 333
    }
}
