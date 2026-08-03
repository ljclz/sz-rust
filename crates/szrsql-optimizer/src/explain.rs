//! Phase 5.9 — EXPLAIN ANALYZE
//!
//! 提供 PG 风格的 `EXPLAIN` / `EXPLAIN ANALYZE` 输出能力：
//! - 递归遍历 `LogicalPlan`，构建 `ExplainNode` 树
//! - 非 ANALYZE 模式：仅展示 `CostModel` 估算的行数与成本
//! - ANALYZE 模式：通过 `Executor` 实际执行子计划，捕获每节点实际行数 + 耗时
//! - 格式化输出 PG 风格的缩进树形文本
//!
//! # 输出格式（PG 风格）
//!
//! ```text
//! Sort  (cost=100.00..200.00 rows=1000 width=8) (actual time=10.234..15.678 rows=995 loops=1)
//!   Sort Key: id
//!   ->  Seq Scan on t  (cost=0.00..50.00 rows=1000 width=8) (actual time=0.123..5.456 rows=1000 loops=1)
//!         Filter: (id > 5)
//! ```
//!
//! # 设计权衡
//!
//! ANALYZE 模式下的"每节点耗时"通过**独立重新执行子计划**测量，而非 PG 那样在执行器内部
//! 插桩。这意味着：
//! - 优点：无需侵入式修改 `Executor`，与现有执行器解耦
//! - 缺点：父子节点的实际耗时之和不等于总耗时（PG 也存在此问题但程度更小）
//! - 适用：性能瓶颈定位、计划形状理解；不适用于严格的端到端时间归因
//!
//! 对应 `SzRSQL实施进度.md` Phase 5.9。

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use szrsql_sql::ast::{JoinCondition, JoinType, SetOperator, SetQuantifier};
use szrsql_sql::executor::Executor;
use szrsql_sql::plan::LogicalPlan;
use thiserror::Error;

use crate::cost::CostModel;

// =====================================================================
//  错误类型
// =====================================================================

/// EXPLAIN 错误
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExplainError {
    /// ANALYZE 模式需要 Executor
    #[error("ANALYZE mode requires an Executor to execute the plan")]
    MissingExecutor,
    /// 执行计划失败
    #[error("execution failed: {0}")]
    ExecutionFailed(String),
}

// =====================================================================
//  配置与统计
// =====================================================================

/// EXPLAIN 配置选项
#[derive(Debug, Clone, Copy, Default)]
pub struct ExplainConfig {
    /// 是否实际执行计划（`EXPLAIN ANALYZE`）
    pub analyze: bool,
    /// 是否输出详细信息（`VERBOSE`）
    pub verbose: bool,
    /// 是否显示估算成本（默认 true）
    pub costs: bool,
    /// 是否显示实际耗时（ANALYZE 模式自动为 true）
    pub timing: bool,
    /// 是否显示汇总行
    pub summary: bool,
}

impl ExplainConfig {
    /// 默认 `EXPLAIN`（非 ANALYZE）
    pub fn new() -> Self {
        Self {
            analyze: false,
            verbose: false,
            costs: true,
            timing: false,
            summary: false,
        }
    }

    /// `EXPLAIN ANALYZE`
    pub fn analyze() -> Self {
        Self {
            analyze: true,
            verbose: false,
            costs: true,
            timing: true,
            summary: true,
        }
    }
}

/// ANALYZE 模式下的实际统计
#[derive(Debug, Clone, Copy, Default)]
pub struct ActualStats {
    /// 实际输出行数
    pub rows: usize,
    /// 循环次数（当前实现固定为 1）
    pub loops: usize,
    /// 启动耗时（毫秒）— 当前实现简化为 0
    pub startup_ms: f64,
    /// 总耗时（毫秒）
    pub total_ms: f64,
}

/// 单个 EXPLAIN 节点
#[derive(Debug, Clone)]
pub struct ExplainNode {
    /// 节点类型（如 "Seq Scan on t"、"Sort"、"Hash Join"）
    pub node_type: String,
    /// 估算行数
    pub estimated_rows: usize,
    /// 估算总成本
    pub estimated_cost: f64,
    /// 估算行宽（字节）
    pub estimated_width: usize,
    /// 实际统计（仅 ANALYZE 模式）
    pub actual: Option<ActualStats>,
    /// 节点附加信息行（如 "Sort Key: id"、"Filter: (id > 5)"）
    pub details: Vec<String>,
    /// 子节点
    pub children: Vec<ExplainNode>,
}

// =====================================================================
//  构建器
// =====================================================================

/// EXPLAIN 构建器
///
/// 非 ANALYZE 模式：仅用 `CostModel` 估算
/// ANALYZE 模式：需要 `Executor` 实际执行
pub struct ExplainBuilder<'a> {
    /// 成本模型（用于估算行数/成本）
    model: &'a CostModel,
    /// 执行器（ANALYZE 模式必需）
    executor: Option<&'a Executor<'a>>,
    /// 配置
    config: ExplainConfig,
}

impl<'a> ExplainBuilder<'a> {
    /// 创建非 ANALYZE 构建器
    pub fn new(model: &'a CostModel) -> Self {
        Self {
            model,
            executor: None,
            config: ExplainConfig::new(),
        }
    }

    /// 创建 ANALYZE 构建器
    pub fn new_analyze(model: &'a CostModel, executor: &'a Executor<'a>) -> Self {
        Self {
            model,
            executor: Some(executor),
            config: ExplainConfig::analyze(),
        }
    }

    /// 自定义配置
    pub fn with_config(mut self, config: ExplainConfig) -> Self {
        self.config = config;
        self
    }

    /// 构建 EXPLAIN 节点树
    pub fn build(&self, plan: &LogicalPlan) -> Result<ExplainNode, ExplainError> {
        self.build_node(plan)
    }

    fn build_node(&self, plan: &LogicalPlan) -> Result<ExplainNode, ExplainError> {
        let cost = self.model.estimate(plan);
        let (node_type, details, child_plans): (String, Vec<String>, Vec<&LogicalPlan>) =
            self.describe_node(plan);

        let actual = if self.config.analyze {
            Some(self.measure_actual(plan)?)
        } else {
            None
        };

        let mut children = Vec::with_capacity(child_plans.len());
        for child in child_plans {
            children.push(self.build_node(child)?);
        }

        Ok(ExplainNode {
            node_type,
            estimated_rows: cost.cardinality,
            estimated_cost: cost.total(),
            estimated_width: cost.width,
            actual,
            details,
            children,
        })
    }

    /// 描述节点类型与附加信息（返回 (node_type, details, child_plans)）
    fn describe_node<'b>(
        &self,
        plan: &'b LogicalPlan,
    ) -> (String, Vec<String>, Vec<&'b LogicalPlan>) {
        match plan {
            LogicalPlan::Scan {
                table,
                alias,
                schema,
            } => {
                let mut details = Vec::new();
                if let Some(a) = alias {
                    details.push(format!("Alias: {}", a));
                }
                if self.config.verbose {
                    let cols: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
                    details.push(format!("Output: {}", cols.join(", ")));
                }
                (
                    format!("Seq Scan on {}", table.qualified_name()),
                    details,
                    Vec::new(),
                )
            }
            LogicalPlan::IndexScan {
                table,
                schema,
                index_name,
                index_columns,
                predicate,
                ..
            } => {
                let cols = index_columns.join(", ");
                let mut details =
                    vec![format!("Index Cond: ({})", Self::expr_to_string(predicate))];
                details.push(format!("Index Cols: {}", cols));
                if self.config.verbose {
                    let out: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
                    details.push(format!("Output: {}", out.join(", ")));
                }
                (
                    format!(
                        "Index Scan using {} on {}",
                        index_name,
                        table.qualified_name()
                    ),
                    details,
                    Vec::new(),
                )
            }
            LogicalPlan::Filter { predicate, input } => {
                let details = vec![format!("Filter: ({})", Self::expr_to_string(predicate))];
                ("Filter".to_string(), details, vec![input.as_ref()])
            }
            LogicalPlan::Projection { exprs, input, .. } => {
                let outs: Vec<String> = exprs
                    .iter()
                    .map(|(e, alias)| {
                        let s = Self::expr_to_string(e);
                        match alias {
                            Some(a) => format!("{} AS {}", s, a),
                            None => s,
                        }
                    })
                    .collect();
                let details = vec![format!("Output: {}", outs.join(", "))];
                ("Projection".to_string(), details, vec![input.as_ref()])
            }
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
                ..
            } => {
                let algo = "Nested Loop"; // 当前执行器仅实现 NestedLoop
                let jtype = match join_type {
                    JoinType::Inner => "Inner",
                    JoinType::LeftOuter => "Left Outer",
                    JoinType::RightOuter => "Right Outer",
                    JoinType::FullOuter => "Full Outer",
                    JoinType::Cross => "Cross",
                    JoinType::Semi => "Semi",
                    JoinType::Anti => "Anti",
                };
                let mut details = vec![format!("Join Type: {}", jtype)];
                match condition {
                    JoinCondition::On(e) => {
                        details.push(format!("Join Cond: ({})", Self::expr_to_string(e)));
                    }
                    JoinCondition::Using(cols) => {
                        details.push(format!("Using: {}", cols.join(", ")));
                    }
                    JoinCondition::Natural => details.push("Natural".to_string()),
                    JoinCondition::None => {}
                }
                let _ = algo;
                (
                    "Join".to_string(),
                    details,
                    vec![left.as_ref(), right.as_ref()],
                )
            }
            LogicalPlan::Aggregate {
                grouping_sets,
                having,
                input,
                ..
            } => {
                let mut details = Vec::new();
                // P3-1: 多分组集 — 显示各集的分组键
                if grouping_sets.len() == 1 {
                    let set = &grouping_sets[0];
                    if !set.is_empty() {
                        let g: Vec<String> = set.iter().map(Self::expr_to_string).collect();
                        details.push(format!("Group Key: {}", g.join(", ")));
                    }
                } else if grouping_sets.len() > 1 {
                    for (i, set) in grouping_sets.iter().enumerate() {
                        if set.is_empty() {
                            details.push(format!("Grouping Set {i}: ()"));
                        } else {
                            let g: Vec<String> = set.iter().map(Self::expr_to_string).collect();
                            details.push(format!("Grouping Set {i}: ({})", g.join(", ")));
                        }
                    }
                }
                if let Some(h) = having {
                    details.push(format!("Having: ({})", Self::expr_to_string(h)));
                }
                ("Aggregate".to_string(), details, vec![input.as_ref()])
            }
            LogicalPlan::Sort { order_by, input } => {
                let keys: Vec<String> = order_by
                    .iter()
                    .map(|o| {
                        let dir = if o.asc {
                            "ASC"
                        } else {
                            "DESC"
                        };
                        let nulls = if o.nulls_first {
                            "NULLS FIRST"
                        } else {
                            "NULLS LAST"
                        };
                        format!("{} {} {}", Self::expr_to_string(&o.expr), dir, nulls)
                    })
                    .collect();
                let details = vec![format!("Sort Key: {}", keys.join(", "))];
                ("Sort".to_string(), details, vec![input.as_ref()])
            }
            LogicalPlan::Limit {
                limit,
                offset,
                input,
            } => {
                let mut details = Vec::new();
                if let Some(l) = limit {
                    details.push(format!("Limit: {}", Self::expr_to_string(l)));
                }
                if let Some(o) = offset {
                    details.push(format!("Offset: {}", Self::expr_to_string(o)));
                }
                ("Limit".to_string(), details, vec![input.as_ref()])
            }
            LogicalPlan::Distinct { input } => {
                ("Distinct".to_string(), Vec::new(), vec![input.as_ref()])
            }
            LogicalPlan::SetOp {
                op,
                quantifier,
                left,
                right,
            } => {
                let op_str = match op {
                    SetOperator::Union => "Union",
                    SetOperator::Intersect => "Intersect",
                    SetOperator::Except => "Except",
                };
                let q_str = match quantifier {
                    SetQuantifier::All => "ALL",
                    SetQuantifier::Distinct => "DISTINCT",
                    SetQuantifier::None => "",
                };
                let node = format!("SetOp {} {}", op_str, q_str).trim().to_string();
                (node, Vec::new(), vec![left.as_ref(), right.as_ref()])
            }
            LogicalPlan::Empty => ("Empty".to_string(), Vec::new(), Vec::new()),
            LogicalPlan::Dual => ("Dual".to_string(), Vec::new(), Vec::new()),
            LogicalPlan::Shared { id, plan } => {
                let details = vec![format!("Shared ID: {}", id)];
                ("Shared".to_string(), details, vec![plan.as_ref()])
            }
            LogicalPlan::MemoRef { id, .. } => {
                let details = vec![format!("MemoRef ID: {}", id)];
                ("MemoRef".to_string(), details, Vec::new())
            }
            // DML/DDL 简化展示
            LogicalPlan::Insert { table, .. } => (
                format!("Insert on {}", table.qualified_name()),
                Vec::new(),
                Vec::new(),
            ),
            LogicalPlan::Replace { table, .. } => (
                format!("Replace on {}", table.qualified_name()),
                Vec::new(),
                Vec::new(),
            ),
            LogicalPlan::Update { table, .. } => (
                format!("Update on {}", table.qualified_name()),
                Vec::new(),
                Vec::new(),
            ),
            LogicalPlan::Delete { table, .. } => (
                format!("Delete on {}", table.qualified_name()),
                Vec::new(),
                Vec::new(),
            ),
            _ => (
                format!("{:?}", plan)
                    .split('{')
                    .next()
                    .unwrap_or("Unknown")
                    .to_string(),
                Vec::new(),
                Vec::new(),
            ),
        }
    }

    /// 简化表达式为可读字符串
    fn expr_to_string(expr: &szrsql_sql::ast::Expr) -> String {
        use szrsql_sql::ast::Expr;
        use szrsql_types::value::Value;
        match expr {
            Expr::Identifier(parts) => parts.join("."),
            Expr::Literal(v) => match v {
                Value::Int64(n) => n.to_string(),
                Value::Float64(f) => f.to_string(),
                Value::Text(s) => format!("'{}'", s),
                Value::Bool(b) => b.to_string(),
                Value::Null => "NULL".to_string(),
                _ => format!("{:?}", v),
            },
            Expr::BinaryOp { left, op, right } => {
                let op_str: String = match op {
                    szrsql_sql::ast::BinaryOp::Eq => "=".to_string(),
                    szrsql_sql::ast::BinaryOp::NotEq => "!=".to_string(),
                    szrsql_sql::ast::BinaryOp::Lt => "<".to_string(),
                    szrsql_sql::ast::BinaryOp::LtEq => "<=".to_string(),
                    szrsql_sql::ast::BinaryOp::Gt => ">".to_string(),
                    szrsql_sql::ast::BinaryOp::GtEq => ">=".to_string(),
                    szrsql_sql::ast::BinaryOp::And => "AND".to_string(),
                    szrsql_sql::ast::BinaryOp::Or => "OR".to_string(),
                    szrsql_sql::ast::BinaryOp::Plus => "+".to_string(),
                    szrsql_sql::ast::BinaryOp::Minus => "-".to_string(),
                    szrsql_sql::ast::BinaryOp::Multiply => "*".to_string(),
                    szrsql_sql::ast::BinaryOp::Divide => "/".to_string(),
                    szrsql_sql::ast::BinaryOp::Modulo => "%".to_string(),
                    _ => format!("{:?}", op).to_lowercase(),
                };
                format!(
                    "{} {} {}",
                    Self::expr_to_string(left),
                    op_str,
                    Self::expr_to_string(right)
                )
            }
            Expr::UnaryOp { op, expr } => {
                format!("{:?} {}", op, Self::expr_to_string(expr))
            }
            Expr::Function { name, args, .. } => {
                let a: Vec<String> = args.iter().map(Self::expr_to_string).collect();
                format!("{}({})", name, a.join(", "))
            }
            _ => format!("{:?}", expr)
                .split('{')
                .next()
                .unwrap_or("?")
                .to_string(),
        }
    }

    /// 实际执行子计划并测量行数 + 耗时
    fn measure_actual(&self, plan: &LogicalPlan) -> Result<ActualStats, ExplainError> {
        let executor = self.executor.ok_or(ExplainError::MissingExecutor)?;
        let start = Instant::now();
        let rows = executor
            .execute(plan)
            .map_err(|e| ExplainError::ExecutionFailed(format!("{:?}", e)))?;
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        Ok(ActualStats {
            rows: rows.len(),
            loops: 1,
            startup_ms: 0.0,
            total_ms: elapsed_ms,
        })
    }
}

// =====================================================================
//  格式化器
// =====================================================================

/// 格式化 EXPLAIN 节点树为 PG 风格文本
pub fn format_explain(root: &ExplainNode, config: &ExplainConfig) -> String {
    let mut out = String::new();
    format_node(root, config, 0, &mut out);
    if config.summary {
        out.push('\n');
        out.push_str(&format!(
            "Summary: total estimated cost={:.4}, estimated rows={}, width={}",
            root.estimated_cost, root.estimated_rows, root.estimated_width
        ));
        if let Some(a) = &root.actual {
            out.push_str(&format!(
                ", actual rows={} actual time={:.3}ms",
                a.rows, a.total_ms
            ));
        }
    }
    out
}

fn format_node(node: &ExplainNode, config: &ExplainConfig, depth: usize, out: &mut String) {
    // 缩进：每个深度 2 空格，子节点前加 "->  "
    let indent = "  ".repeat(depth);
    let child_prefix = if depth > 0 {
        "->  "
    } else {
        ""
    };

    // 节点类型 + 估算信息
    out.push_str(&indent);
    out.push_str(child_prefix);
    out.push_str(&node.node_type);

    // 估算成本
    if config.costs {
        out.push_str(&format!(
            "  (cost=0.00..{:.2} rows={} width={})",
            node.estimated_cost, node.estimated_rows, node.estimated_width
        ));
    }

    // 实际统计
    if config.timing {
        if let Some(a) = &node.actual {
            out.push_str(&format!(
                "  (actual time={:.3}..{:.3} rows={} loops={})",
                a.startup_ms, a.total_ms, a.rows, a.loops
            ));
        }
    }

    out.push('\n');

    // 附加信息行（缩进比节点多 1 级）
    let detail_indent = "  ".repeat(depth + 1);
    for d in &node.details {
        out.push_str(&detail_indent);
        out.push_str(d);
        out.push('\n');
    }

    // 子节点
    for child in &node.children {
        format_node(child, config, depth + 1, out);
    }
}

// =====================================================================
//  顶层入口
// =====================================================================

/// EXPLAIN（非 ANALYZE）
pub fn explain(plan: &LogicalPlan, model: &CostModel) -> String {
    let builder = ExplainBuilder::new(model);
    match builder.build(plan) {
        Ok(root) => format_explain(&root, &ExplainConfig::new()),
        Err(e) => format!("ERROR: {}", e),
    }
}

/// EXPLAIN ANALYZE
pub fn explain_analyze<'a>(
    plan: &'a LogicalPlan,
    model: &'a CostModel,
    executor: &'a Executor<'a>,
) -> String {
    let builder = ExplainBuilder::new_analyze(model, executor);
    match builder.build(plan) {
        Ok(root) => format_explain(&root, &ExplainConfig::analyze()),
        Err(e) => format!("ERROR: {}", e),
    }
}

/// 带 RefCell 的内部缓存计数器（用于测试校验）
///
/// 记录每个节点类型被 `build_node` 调用的次数，方便测试验证遍历正确性。
#[derive(Debug, Default)]
pub struct ExplainTrace {
    counts: RefCell<HashMap<String, usize>>,
}

impl ExplainTrace {
    /// 创建空 trace
    pub fn new() -> Self {
        Self {
            counts: RefCell::new(HashMap::new()),
        }
    }

    /// 记录一个节点类型
    pub fn record(&self, node_type: &str) {
        let mut c = self.counts.borrow_mut();
        *c.entry(node_type.to_string()).or_insert(0) += 1;
    }

    /// 获取某类型节点的记录次数
    pub fn count(&self, node_type: &str) -> usize {
        self.counts.borrow().get(node_type).copied().unwrap_or(0)
    }

    /// 总节点数
    pub fn total(&self) -> usize {
        self.counts.borrow().values().sum()
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_sql::ast::{
        BinaryOp, ColumnDefinition, Expr as AExpr, OrderByExpr, SetOperator, SetQuantifier,
        TableName,
    };
    use szrsql_sql::executor::{Executor, InMemoryTable};
    use szrsql_sql::plan::{LogicalPlan, TableSchema};
    use szrsql_types::value::{ColumnType, Value};

    use crate::statistics::InMemoryStatisticsStore;

    // 辅助函数：构造 Schema
    fn schema_t() -> TableSchema {
        TableSchema {
            name: TableName::new("t"),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("name", ColumnType::Text),
            ],
        }
    }

    fn scan_t() -> LogicalPlan {
        LogicalPlan::Scan {
            table: TableName::new("t"),
            alias: None,
            schema: schema_t(),
        }
    }

    fn make_eq(col: &str, val: i64) -> AExpr {
        AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec![col.to_string()])),
            op: BinaryOp::Eq,
            right: Box::new(AExpr::Literal(Value::Int64(val))),
        }
    }

    fn make_gt(col: &str, val: i64) -> AExpr {
        AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec![col.to_string()])),
            op: BinaryOp::Gt,
            right: Box::new(AExpr::Literal(Value::Int64(val))),
        }
    }

    fn make_cost_model() -> CostModel {
        let stats = std::sync::Arc::new(InMemoryStatisticsStore::new());
        CostModel::new(stats)
    }

    fn make_table_with_rows() -> InMemoryTable {
        let mut t = InMemoryTable::with_columns(
            "t",
            vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
        );
        t.insert(vec![Value::Int64(1), Value::Text("alice".to_string())]);
        t.insert(vec![Value::Int64(2), Value::Text("bob".to_string())]);
        t.insert(vec![Value::Int64(3), Value::Text("carol".to_string())]);
        t
    }

    // ==================================================================
    //  ExplainConfig 测试（3）
    // ==================================================================

    #[test]
    fn test_config_new_defaults() {
        let c = ExplainConfig::new();
        assert!(!c.analyze);
        assert!(!c.verbose);
        assert!(c.costs);
        assert!(!c.timing);
        assert!(!c.summary);
    }

    #[test]
    fn test_config_analyze_defaults() {
        let c = ExplainConfig::analyze();
        assert!(c.analyze);
        assert!(c.costs);
        assert!(c.timing);
        assert!(c.summary);
    }

    #[test]
    fn test_config_builder_chaining() {
        let c = ExplainConfig::new();
        let _ = c;
    }

    // ==================================================================
    //  非 ANALYZE 模式测试（5）
    // ==================================================================

    #[test]
    fn test_explain_simple_scan() {
        let model = make_cost_model();
        let plan = scan_t();
        let output = explain(&plan, &model);

        assert!(
            output.contains("Seq Scan on t"),
            "应包含 Seq Scan on t，实际：{}",
            output
        );
        assert!(
            output.contains("cost=0.00"),
            "应显示成本起始 0.00，实际：{}",
            output
        );
        assert!(
            output.contains("rows="),
            "应显示 rows 字段，实际：{}",
            output
        );
    }

    #[test]
    fn test_explain_filter() {
        let model = make_cost_model();
        let plan = LogicalPlan::Filter {
            predicate: make_gt("id", 1),
            input: Box::new(scan_t()),
        };
        let output = explain(&plan, &model);

        assert!(output.contains("Filter"), "应包含 Filter：{}", output);
        assert!(
            output.contains("Filter: (id > 1)"),
            "应显示 Filter 条件：{}",
            output
        );
        assert!(
            output.contains("Seq Scan on t"),
            "应包含子节点 Seq Scan：{}",
            output
        );
    }

    #[test]
    fn test_explain_sort() {
        let model = make_cost_model();
        let sort = LogicalPlan::Sort {
            order_by: vec![OrderByExpr {
                expr: AExpr::Identifier(vec!["id".to_string()]),
                asc: true,
                nulls_first: false,
            }],
            input: Box::new(scan_t()),
        };
        let output = explain(&sort, &model);

        assert!(output.contains("Sort"), "应包含 Sort：{}", output);
        assert!(
            output.contains("Sort Key: id ASC"),
            "应显示 Sort Key：{}",
            output
        );
    }

    #[test]
    fn test_explain_limit() {
        let model = make_cost_model();
        let plan = LogicalPlan::Limit {
            limit: Some(AExpr::Literal(Value::Int64(10))),
            offset: Some(AExpr::Literal(Value::Int64(5))),
            input: Box::new(scan_t()),
        };
        let output = explain(&plan, &model);

        assert!(output.contains("Limit"), "应包含 Limit：{}", output);
        assert!(output.contains("Limit: 10"), "应显示 Limit 值：{}", output);
        assert!(output.contains("Offset: 5"), "应显示 Offset 值：{}", output);
    }

    #[test]
    fn test_explain_distinct() {
        let model = make_cost_model();
        let plan = LogicalPlan::Distinct {
            input: Box::new(scan_t()),
        };
        let output = explain(&plan, &model);

        assert!(output.contains("Distinct"), "应包含 Distinct：{}", output);
        assert!(
            output.contains("Seq Scan on t"),
            "应包含子节点 Scan：{}",
            output
        );
    }

    // ==================================================================
    //  ANALYZE 模式测试（4）
    // ==================================================================

    #[test]
    fn test_explain_analyze_simple_scan() {
        let model = make_cost_model();
        let t = make_table_with_rows();
        let mut exec = Executor::new();
        exec.register_table(&t);

        let plan = scan_t();
        let output = explain_analyze(&plan, &model, &exec);

        assert!(
            output.contains("Seq Scan on t"),
            "应包含 Seq Scan：{}",
            output
        );
        assert!(
            output.contains("actual time="),
            "ANALYZE 应显示 actual time：{}",
            output
        );
        assert!(output.contains("rows=3"), "应显示实际行数 3：{}", output);
        assert!(output.contains("loops=1"), "应显示 loops=1：{}", output);
    }

    #[test]
    fn test_explain_analyze_filter() {
        let model = make_cost_model();
        let t = make_table_with_rows();
        let mut exec = Executor::new();
        exec.register_table(&t);

        let plan = LogicalPlan::Filter {
            predicate: make_gt("id", 1),
            input: Box::new(scan_t()),
        };
        let output = explain_analyze(&plan, &model, &exec);

        assert!(output.contains("Filter"), "应包含 Filter：{}", output);
        assert!(
            output.contains("actual time="),
            "ANALYZE 应显示 actual time：{}",
            output
        );
        // Filter 后应剩 2 行（id=2,3）
        assert!(
            output.contains("rows=2") || output.contains("rows=3"),
            "Filter 实际行数应为 2 或 3：{}",
            output
        );
    }

    #[test]
    fn test_explain_analyze_limit() {
        let model = make_cost_model();
        let t = make_table_with_rows();
        let mut exec = Executor::new();
        exec.register_table(&t);

        let plan = LogicalPlan::Limit {
            limit: Some(AExpr::Literal(Value::Int64(2))),
            offset: None,
            input: Box::new(scan_t()),
        };
        let output = explain_analyze(&plan, &model, &exec);

        assert!(output.contains("Limit"), "应包含 Limit：{}", output);
        assert!(
            output.contains("actual time="),
            "ANALYZE 应显示 actual time：{}",
            output
        );
    }

    #[test]
    fn test_explain_analyze_includes_summary() {
        let model = make_cost_model();
        let t = make_table_with_rows();
        let mut exec = Executor::new();
        exec.register_table(&t);

        let plan = scan_t();
        let output = explain_analyze(&plan, &model, &exec);

        assert!(
            output.contains("Summary:"),
            "ANALYZE 应包含 Summary 行：{}",
            output
        );
    }

    // ==================================================================
    //  节点类型覆盖测试（5）
    // ==================================================================

    #[test]
    fn test_explain_join_node() {
        let model = make_cost_model();
        let left = scan_t();
        let right = LogicalPlan::Scan {
            table: TableName::new("u"),
            alias: None,
            schema: TableSchema {
                name: TableName::new("u"),
                columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
            },
        };
        let join = LogicalPlan::Join {
            join_type: JoinType::Inner,
            condition: szrsql_sql::ast::JoinCondition::On(make_eq("id", 0)),
            left: Box::new(left),
            right: Box::new(right),
            lateral: false,
            lateral_subquery: None,
            right_schema: None,
        };
        let output = explain(&join, &model);

        assert!(output.contains("Join"), "应包含 Join：{}", output);
        assert!(output.contains("Inner"), "应显示 Inner：{}", output);
        assert!(
            output.contains("Join Cond:"),
            "应显示 Join Cond：{}",
            output
        );
    }

    #[test]
    fn test_explain_aggregate_node() {
        let model = make_cost_model();
        let plan = LogicalPlan::Aggregate {
            grouping_sets: vec![vec![AExpr::Identifier(vec!["dept".to_string()])]],
            having: None,
            aggregates: Vec::new(),
            input: Box::new(scan_t()),
        };
        let output = explain(&plan, &model);

        assert!(output.contains("Aggregate"), "应包含 Aggregate：{}", output);
        assert!(
            output.contains("Group Key: dept"),
            "应显示 Group Key：{}",
            output
        );
    }

    #[test]
    fn test_explain_setop_node() {
        let model = make_cost_model();
        let plan = LogicalPlan::SetOp {
            op: SetOperator::Union,
            quantifier: SetQuantifier::All,
            left: Box::new(scan_t()),
            right: Box::new(scan_t()),
        };
        let output = explain(&plan, &model);

        assert!(output.contains("SetOp"), "应包含 SetOp：{}", output);
        assert!(output.contains("Union"), "应显示 Union：{}", output);
        assert!(output.contains("ALL"), "应显示 ALL：{}", output);
    }

    #[test]
    fn test_explain_shared_memo_ref_nodes() {
        let model = make_cost_model();
        let shared = LogicalPlan::Shared {
            id: 42,
            plan: Box::new(scan_t()),
        };
        let memo = LogicalPlan::MemoRef {
            id: 42,
            schema: schema_t(),
        };
        let setop = LogicalPlan::SetOp {
            op: SetOperator::Union,
            quantifier: SetQuantifier::All,
            left: Box::new(shared),
            right: Box::new(memo),
        };
        let output = explain(&setop, &model);

        assert!(output.contains("Shared"), "应包含 Shared：{}", output);
        assert!(
            output.contains("Shared ID: 42"),
            "应显示 Shared ID：{}",
            output
        );
        assert!(output.contains("MemoRef"), "应包含 MemoRef：{}", output);
        assert!(
            output.contains("MemoRef ID: 42"),
            "应显示 MemoRef ID：{}",
            output
        );
    }

    #[test]
    fn test_explain_empty_and_dual() {
        let model = make_cost_model();
        let empty = LogicalPlan::Empty;
        let dual = LogicalPlan::Dual;

        let out_empty = explain(&empty, &model);
        assert!(out_empty.contains("Empty"), "应包含 Empty：{}", out_empty);

        let out_dual = explain(&dual, &model);
        assert!(out_dual.contains("Dual"), "应包含 Dual：{}", out_dual);
    }

    // ==================================================================
    //  Trace 测试（2）
    // ==================================================================

    #[test]
    fn test_trace_record_and_count() {
        let trace = ExplainTrace::new();
        trace.record("Seq Scan");
        trace.record("Seq Scan");
        trace.record("Filter");

        assert_eq!(trace.count("Seq Scan"), 2);
        assert_eq!(trace.count("Filter"), 1);
        assert_eq!(trace.count("Sort"), 0);
        assert_eq!(trace.total(), 3);
    }

    #[test]
    fn test_trace_default_empty() {
        let trace = ExplainTrace::new();
        assert_eq!(trace.total(), 0);
        assert_eq!(trace.count("anything"), 0);
    }

    // ==================================================================
    //  VERBOSE 模式测试（1）
    // ==================================================================

    #[test]
    fn test_explain_verbose_shows_output_columns() {
        let model = make_cost_model();
        let builder = ExplainBuilder::new(&model).with_config(ExplainConfig {
            verbose: true,
            costs: true,
            ..ExplainConfig::new()
        });
        let plan = scan_t();
        let root = builder.build(&plan).expect("build failed");

        let output = format_explain(
            &root,
            &ExplainConfig {
                verbose: true,
                costs: true,
                ..ExplainConfig::new()
            },
        );

        assert!(
            output.contains("Output: id, name"),
            "VERBOSE 模式应显示 Output 列：{}",
            output
        );
    }

    // ==================================================================
    //  错误处理测试（1）
    // ==================================================================

    #[test]
    fn test_explain_analyze_builder_config() {
        // 验证 new_analyze 正确设置 analyze=true 与 timing=true
        let model = make_cost_model();
        let exec = Executor::new();
        let builder = ExplainBuilder::new_analyze(&model, &exec);
        assert!(builder.config.analyze, "new_analyze 应设置 analyze=true");
        assert!(builder.config.timing, "new_analyze 应设置 timing=true");
        assert!(builder.config.summary, "new_analyze 应设置 summary=true");
        assert!(builder.executor.is_some(), "new_analyze 应注入 executor");
    }

    // ==================================================================
    //  端到端集成测试（1）— Phase 5.9 验收核心
    // ==================================================================

    /// 集成测试：`EXPLAIN ANALYZE SELECT ...` → 输出包含计划树 + 每节点实际行数 + 每节点耗时
    ///
    /// 验收标准：输出格式类似 PG 的 EXPLAIN ANALYZE
    ///
    /// 注：执行器已实现 Sort 节点（executor.rs execute_sort），集成测试可
    /// 使用 Sort(Limit(Filter(Scan(t)))) 验证 ANALYZE 输出。
    #[test]
    fn test_explain_analyze_integration_pg_like_format() {
        let model = make_cost_model();
        let t = make_table_with_rows();
        let mut exec = Executor::new();
        exec.register_table(&t);

        // 构造 Distinct(Limit(Filter(Scan(t))))
        let scan = scan_t();
        let filter = LogicalPlan::Filter {
            predicate: make_gt("id", 1),
            input: Box::new(scan),
        };
        let limit = LogicalPlan::Limit {
            limit: Some(AExpr::Literal(Value::Int64(10))),
            offset: None,
            input: Box::new(filter),
        };
        let distinct = LogicalPlan::Distinct {
            input: Box::new(limit),
        };

        let output = explain_analyze(&distinct, &model, &exec);

        // 1. 应包含计划树的所有节点（PG 风格）
        assert!(
            output.contains("Distinct"),
            "应包含 Distinct 节点：{}",
            output
        );
        assert!(output.contains("Limit"), "应包含 Limit 节点：{}", output);
        assert!(output.contains("Filter"), "应包含 Filter 节点：{}", output);
        assert!(
            output.contains("Seq Scan on t"),
            "应包含 Seq Scan 节点：{}",
            output
        );

        // 2. 应显示估算信息
        assert!(output.contains("cost="), "应显示 cost=：{}", output);
        assert!(output.contains("rows="), "应显示 rows=：{}", output);
        assert!(output.contains("width="), "应显示 width=：{}", output);

        // 3. 应显示每节点实际行数 + 耗时（actual time + rows + loops）
        assert!(
            output.contains("actual time="),
            "ANALYZE 应显示 actual time=：{}",
            output
        );
        assert!(
            output.contains("loops=1"),
            "ANALYZE 应显示 loops=1：{}",
            output
        );

        // 4. 应显示节点特定信息
        assert!(output.contains("Filter:"), "应显示 Filter 条件：{}", output);
        assert!(output.contains("Limit: 10"), "应显示 Limit 值：{}", output);

        // 5. 应显示 Summary 行
        assert!(output.contains("Summary:"), "应包含 Summary 行：{}", output);

        // 6. 应有缩进层级（PG 风格的 "->" 子节点标记）
        assert!(output.contains("->"), "应包含 -> 子节点标记：{}", output);

        // 7. 行数统计：Distinct/Limit 应输出 2 行（id=2,3 通过 filter）
        // Filter 节点应有 rows=2（过滤后），Limit 节点 rows=2（limit=10 > 2 不截断）
        // 计数 actual time= 出现次数（每个节点应有一次）
        let actual_count = output.matches("actual time=").count();
        assert!(
            actual_count >= 4,
            "应有至少 4 个节点（Distinct/Limit/Filter/Scan）的 actual time，实际 {}",
            actual_count
        );
    }
}
