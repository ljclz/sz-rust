//! Volcano 迭代器执行器 — Batch 5.2
//!
//! 提供流式（逐行）执行模型，与现有批量执行器 (`executor.rs`) 互补。
//! 每个算子实现 `ExecutionPlan` trait，通过 `next()` 逐行拉取数据。

use crate::ast::{Expr, OrderByExpr};
use crate::expr::{ExprEvaluator, RowContext};
use crate::plan::LogicalPlan;
use szrsql_types::value::Value;

/// 行类型别名
pub type Row = Vec<Value>;

// =====================================================================
//  核心 trait
// =====================================================================

/// Volcano 迭代器执行计划
pub trait ExecutionPlan {
    /// 拉取下一行（None 表示结束）
    fn next(&mut self) -> Option<Row>;

    /// 输出列名
    fn column_names(&self) -> Vec<String>;

    /// 重置迭代器到初始状态
    fn reset(&mut self);
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 将迭代器排空为 Vec<Row>
pub fn collect<P>(plan: &mut P) -> Vec<Row>
where
    P: ExecutionPlan + ?Sized,
{
    let mut rows = Vec::new();
    while let Some(row) = plan.next() {
        rows.push(row);
    }
    rows
}

/// 值比较（排序用）
fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Int64(x), Value::Int64(y)) => x.cmp(y),
        (Value::Float64(x), Value::Float64(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Int64(x), Value::Float64(y)) => {
            (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (Value::Float64(x), Value::Int64(y)) => {
            x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
        }
        (Value::Text(x), Value::Text(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::Timestamp(x), Value::Timestamp(y)) => x.cmp(y),
        (Value::Decimal(x, sx), Value::Decimal(y, sy)) => {
            let sx = *sx as u32;
            let sy = *sy as u32;
            let max_s = sx.max(sy);
            let xv = x * 10i128.pow(max_s - sx);
            let yv = y * 10i128.pow(max_s - sy);
            xv.cmp(&yv)
        }
        _ => format!("{a:?}").cmp(&format!("{b:?}")),
    }
}

/// 构建行上下文（列名 -> 值映射）
fn make_row_context(col_names: &[String], row: &Row) -> RowContext {
    let mut ctx = RowContext::new();
    for (i, name) in col_names.iter().enumerate() {
        let val = row.get(i).cloned().unwrap_or(Value::Null);
        ctx.columns.insert(name.to_lowercase(), val);
    }
    ctx
}

// =====================================================================
//  算子实现
// =====================================================================

/// 顺序扫描 - 从内存行集合逐行输出
pub struct SeqScanExec {
    rows: Vec<Row>,
    names: Vec<String>,
    cursor: usize,
}

impl SeqScanExec {
    /// 创建顺序扫描算子
    pub fn new(rows: Vec<Row>, names: Vec<String>) -> Self {
        Self {
            rows,
            names,
            cursor: 0,
        }
    }
}

impl ExecutionPlan for SeqScanExec {
    fn next(&mut self) -> Option<Row> {
        if self.cursor < self.rows.len() {
            let row = self.rows[self.cursor].clone();
            self.cursor += 1;
            Some(row)
        } else {
            None
        }
    }

    fn column_names(&self) -> Vec<String> {
        self.names.clone()
    }

    fn reset(&mut self) {
        self.cursor = 0;
    }
}

/// 过滤算子 - 对子计划输出应用谓词
pub struct FilterExec {
    input: Box<dyn ExecutionPlan>,
    predicate: Expr,
}

impl FilterExec {
    /// 创建过滤算子
    pub fn new(input: Box<dyn ExecutionPlan>, predicate: Expr) -> Self {
        Self { input, predicate }
    }
}

impl ExecutionPlan for FilterExec {
    fn next(&mut self) -> Option<Row> {
        let names = self.input.column_names();
        loop {
            let row = self.input.next()?;
            let ctx = make_row_context(&names, &row);
            match ExprEvaluator::eval(&self.predicate, &ctx) {
                Ok(Value::Bool(true)) => return Some(row),
                _ => continue,
            }
        }
    }

    fn column_names(&self) -> Vec<String> {
        self.input.column_names()
    }

    fn reset(&mut self) {
        self.input.reset();
    }
}

/// 投影算子 - 对每行求值表达式列表
pub struct ProjectionExec {
    input: Box<dyn ExecutionPlan>,
    exprs: Vec<Expr>,
    output_names: Vec<String>,
}

impl ProjectionExec {
    /// 创建投影算子
    pub fn new(input: Box<dyn ExecutionPlan>, exprs: Vec<Expr>, output_names: Vec<String>) -> Self {
        Self {
            input,
            exprs,
            output_names,
        }
    }
}

impl ExecutionPlan for ProjectionExec {
    fn next(&mut self) -> Option<Row> {
        let row = self.input.next()?;
        let input_names = self.input.column_names();
        let ctx = make_row_context(&input_names, &row);
        let out: Row = self
            .exprs
            .iter()
            .map(|e| ExprEvaluator::eval(e, &ctx).unwrap_or(Value::Null))
            .collect();
        Some(out)
    }

    fn column_names(&self) -> Vec<String> {
        self.output_names.clone()
    }

    fn reset(&mut self) {
        self.input.reset();
    }
}

/// LIMIT 算子
pub struct LimitExec {
    input: Box<dyn ExecutionPlan>,
    limit: usize,
    offset: usize,
    emitted: usize,
    skipped: usize,
}

impl LimitExec {
    /// 创建 LIMIT 算子
    pub fn new(input: Box<dyn ExecutionPlan>, limit: usize, offset: usize) -> Self {
        Self {
            input,
            limit,
            offset,
            emitted: 0,
            skipped: 0,
        }
    }
}

impl ExecutionPlan for LimitExec {
    fn next(&mut self) -> Option<Row> {
        while self.skipped < self.offset {
            self.input.next()?;
            self.skipped += 1;
        }
        if self.emitted >= self.limit {
            return None;
        }
        let row = self.input.next()?;
        self.emitted += 1;
        Some(row)
    }

    fn column_names(&self) -> Vec<String> {
        self.input.column_names()
    }

    fn reset(&mut self) {
        self.input.reset();
        self.emitted = 0;
        self.skipped = 0;
    }
}

/// 排序算子 - 物化全部输入后排序输出
pub struct SortExec {
    order_by: Vec<OrderByExpr>,
    sorted_rows: Vec<Row>,
    names: Vec<String>,
    cursor: usize,
    initialized: bool,
    input: Box<dyn ExecutionPlan>,
}

impl SortExec {
    /// 创建排序算子
    pub fn new(input: Box<dyn ExecutionPlan>, order_by: Vec<OrderByExpr>) -> Self {
        let names = input.column_names();
        Self {
            order_by,
            sorted_rows: Vec::new(),
            names,
            cursor: 0,
            initialized: false,
            input,
        }
    }

    fn materialize(&mut self) {
        let mut rows = collect(self.input.as_mut());
        let names = self.names.clone();
        let order_by = self.order_by.clone();

        let mut keyed: Vec<(Vec<Value>, Row)> = rows
            .drain(..)
            .map(|row| {
                let ctx = make_row_context(&names, &row);
                let keys: Vec<Value> = order_by
                    .iter()
                    .map(|ob| ExprEvaluator::eval(&ob.expr, &ctx).unwrap_or(Value::Null))
                    .collect();
                (keys, row)
            })
            .collect();

        keyed.sort_by(|(ki, _), (kj, _)| {
            for (k, ob) in order_by.iter().enumerate() {
                let vi = &ki[k];
                let vj = &kj[k];
                let is_null_i = matches!(vi, Value::Null);
                let is_null_j = matches!(vj, Value::Null);
                let ord = if is_null_i || is_null_j {
                    if is_null_i && is_null_j {
                        std::cmp::Ordering::Equal
                    } else if is_null_i {
                        if ob.nulls_first {
                            std::cmp::Ordering::Less
                        } else {
                            std::cmp::Ordering::Greater
                        }
                    } else if ob.nulls_first {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Less
                    }
                } else {
                    compare_values(vi, vj)
                };
                let ord = if ob.asc {
                    ord
                } else {
                    ord.reverse()
                };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        });

        self.sorted_rows = keyed.into_iter().map(|(_, row)| row).collect();
        self.initialized = true;
    }
}

impl ExecutionPlan for SortExec {
    fn next(&mut self) -> Option<Row> {
        if !self.initialized {
            self.materialize();
        }
        if self.cursor < self.sorted_rows.len() {
            let row = self.sorted_rows[self.cursor].clone();
            self.cursor += 1;
            Some(row)
        } else {
            None
        }
    }

    fn column_names(&self) -> Vec<String> {
        self.names.clone()
    }

    fn reset(&mut self) {
        self.input.reset();
        self.initialized = false;
        self.sorted_rows.clear();
        self.cursor = 0;
    }
}

/// 空结果集
pub struct EmptyExec {
    names: Vec<String>,
}

impl EmptyExec {
    /// 创建空结果集
    pub fn new(names: Vec<String>) -> Self {
        Self { names }
    }
}

impl ExecutionPlan for EmptyExec {
    fn next(&mut self) -> Option<Row> {
        None
    }

    fn column_names(&self) -> Vec<String> {
        self.names.clone()
    }

    fn reset(&mut self) {}
}

/// 物化算子 - 缓存子计划全部输出，支持多次迭代
pub struct MaterializeExec {
    cached: Vec<Row>,
    names: Vec<String>,
    cursor: usize,
    materialized: bool,
    input: Box<dyn ExecutionPlan>,
}

impl MaterializeExec {
    /// 创建物化算子
    pub fn new(input: Box<dyn ExecutionPlan>) -> Self {
        let names = input.column_names();
        Self {
            cached: Vec::new(),
            names,
            cursor: 0,
            materialized: false,
            input,
        }
    }
}

impl ExecutionPlan for MaterializeExec {
    fn next(&mut self) -> Option<Row> {
        if !self.materialized {
            self.cached = collect(self.input.as_mut());
            self.materialized = true;
        }
        if self.cursor < self.cached.len() {
            let row = self.cached[self.cursor].clone();
            self.cursor += 1;
            Some(row)
        } else {
            None
        }
    }

    fn column_names(&self) -> Vec<String> {
        self.names.clone()
    }

    fn reset(&mut self) {
        self.cursor = 0;
    }
}

// =====================================================================
//  LogicalPlan -> 迭代器树
// =====================================================================

/// 从 LogicalPlan 构建迭代器执行树
///
/// 需要外部提供表数据（`table_data`：表名 -> (行数据, 列名)）。
/// 不支持的节点返回 EmptyExec。
pub fn build_iter_plan(
    plan: &LogicalPlan,
    table_data: &std::collections::HashMap<String, (Vec<Row>, Vec<String>)>,
) -> Box<dyn ExecutionPlan> {
    match plan {
        LogicalPlan::Scan { table, schema, .. } => {
            let key = table.name.to_lowercase();
            if let Some((rows, names)) = table_data.get(&key) {
                Box::new(SeqScanExec::new(rows.clone(), names.clone()))
            } else {
                let names: Vec<String> = schema.columns.iter().map(|c| c.name.clone()).collect();
                Box::new(EmptyExec::new(names))
            }
        }
        LogicalPlan::Filter { predicate, input } => {
            let child = build_iter_plan(input, table_data);
            Box::new(FilterExec::new(child, predicate.clone()))
        }
        LogicalPlan::Projection {
            exprs,
            output_names,
            input,
        } => {
            let child = build_iter_plan(input, table_data);
            let proj_exprs: Vec<Expr> = exprs.iter().map(|(e, _)| e.clone()).collect();
            Box::new(ProjectionExec::new(child, proj_exprs, output_names.clone()))
        }
        LogicalPlan::Limit {
            limit,
            offset,
            input,
        } => {
            let child = build_iter_plan(input, table_data);
            let lim = eval_const_int(limit).unwrap_or(usize::MAX);
            let off = eval_const_int(offset).unwrap_or(0);
            Box::new(LimitExec::new(child, lim, off))
        }
        LogicalPlan::Sort { order_by, input } => {
            let child = build_iter_plan(input, table_data);
            Box::new(SortExec::new(child, order_by.clone()))
        }
        LogicalPlan::Distinct { input } => {
            let mut child = build_iter_plan(input, table_data);
            let names = child.column_names();
            let rows = collect(child.as_mut());
            let mut seen = std::collections::HashSet::new();
            let unique: Vec<Row> = rows
                .into_iter()
                .filter(|r| seen.insert(format!("{r:?}")))
                .collect();
            Box::new(SeqScanExec::new(unique, names))
        }
        _ => Box::new(EmptyExec::new(infer_column_names(plan))),
    }
}

/// 从常量表达式求值 usize
fn eval_const_int(expr: &Option<Expr>) -> Option<usize> {
    match expr {
        Some(Expr::Literal(Value::Int64(n))) if *n >= 0 => Some(*n as usize),
        _ => None,
    }
}

/// 推断 LogicalPlan 输出列名
pub fn infer_column_names(plan: &LogicalPlan) -> Vec<String> {
    match plan {
        LogicalPlan::Scan { schema, .. } => schema.columns.iter().map(|c| c.name.clone()).collect(),
        LogicalPlan::IndexScan { schema, .. } => {
            schema.columns.iter().map(|c| c.name.clone()).collect()
        }
        LogicalPlan::Projection { output_names, .. } => output_names.clone(),
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Distinct { input, .. } => infer_column_names(input),
        _ => Vec::new(),
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Expr;
    use crate::plan::TableSchema;

    fn sample_data() -> (Vec<Row>, Vec<String>) {
        let names = vec!["id".to_string(), "name".to_string(), "age".to_string()];
        let rows = vec![
            vec![
                Value::Int64(1),
                Value::Text("Alice".into()),
                Value::Int64(30),
            ],
            vec![Value::Int64(2), Value::Text("Bob".into()), Value::Int64(25)],
            vec![
                Value::Int64(3),
                Value::Text("Carol".into()),
                Value::Int64(35),
            ],
            vec![
                Value::Int64(4),
                Value::Text("Dave".into()),
                Value::Int64(28),
            ],
        ];
        (rows, names)
    }

    #[test]
    fn seq_scan_returns_all_rows() {
        let (rows, names) = sample_data();
        let mut scan = SeqScanExec::new(rows.clone(), names);
        let out = collect(&mut scan);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0][1], Value::Text("Alice".into()));
    }

    #[test]
    fn filter_selects_matching_rows() {
        let (rows, names) = sample_data();
        let scan = SeqScanExec::new(rows, names);
        let pred = Expr::BinaryOp {
            left: Box::new(Expr::Identifier(vec!["age".into()])),
            op: crate::ast::BinaryOp::Gt,
            right: Box::new(Expr::Literal(Value::Int64(28))),
        };
        let mut filter = FilterExec::new(Box::new(scan), pred);
        let out = collect(&mut filter);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn projection_evaluates_exprs() {
        let (rows, names) = sample_data();
        let scan = SeqScanExec::new(rows, names);
        let mut proj = ProjectionExec::new(
            Box::new(scan),
            vec![Expr::Identifier(vec!["name".into()])],
            vec!["name".to_string()],
        );
        let out = collect(&mut proj);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], vec![Value::Text("Alice".into())]);
    }

    #[test]
    fn limit_restricts_output() {
        let (rows, names) = sample_data();
        let scan = SeqScanExec::new(rows, names);
        let mut lim = LimitExec::new(Box::new(scan), 2, 1);
        let out = collect(&mut lim);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0][1], Value::Text("Bob".into()));
    }

    #[test]
    fn sort_orders_by_desc() {
        let (rows, names) = sample_data();
        let scan = SeqScanExec::new(rows, names);
        let order = vec![OrderByExpr {
            expr: Expr::Identifier(vec!["age".into()]),
            asc: false,
            nulls_first: true,
        }];
        let mut sort = SortExec::new(Box::new(scan), order);
        let out = collect(&mut sort);
        assert_eq!(out[0][2], Value::Int64(35));
        assert_eq!(out[3][2], Value::Int64(25));
    }

    #[test]
    fn empty_exec_returns_nothing() {
        let mut e = EmptyExec::new(vec!["x".into()]);
        assert!(e.next().is_none());
        assert_eq!(e.column_names(), vec!["x".to_string()]);
    }

    #[test]
    fn materialize_supports_reiteration() {
        let (rows, names) = sample_data();
        let scan = SeqScanExec::new(rows, names);
        let mut mat = MaterializeExec::new(Box::new(scan));
        let first = collect(&mut mat);
        mat.reset();
        let second = collect(&mut mat);
        assert_eq!(first.len(), second.len());
        assert_eq!(first, second);
    }

    #[test]
    fn build_iter_plan_scan_filter() {
        let (rows, names) = sample_data();
        let mut table_data = std::collections::HashMap::new();
        table_data.insert("users".to_string(), (rows, names.clone()));

        let schema = TableSchema {
            name: crate::ast::TableName {
                schema: None,
                name: "users".into(),
            },
            columns: vec![
                crate::ast::ColumnDefinition {
                    name: "id".into(),
                    data_type: szrsql_types::value::ColumnType::Int64,
                    not_null: true,
                    primary_key: false,
                    unique: false,
                    default: None,
                    check: None,
                    references: None,
                    enum_values: None,
                    custom_type_name: None,
                    generated: None,
                    comment: None,
                    auto_increment: false,
                },
                crate::ast::ColumnDefinition {
                    name: "name".into(),
                    data_type: szrsql_types::value::ColumnType::Text,
                    not_null: true,
                    primary_key: false,
                    unique: false,
                    default: None,
                    check: None,
                    references: None,
                    enum_values: None,
                    custom_type_name: None,
                    generated: None,
                    comment: None,
                    auto_increment: false,
                },
                crate::ast::ColumnDefinition {
                    name: "age".into(),
                    data_type: szrsql_types::value::ColumnType::Int64,
                    not_null: true,
                    primary_key: false,
                    unique: false,
                    default: None,
                    check: None,
                    references: None,
                    enum_values: None,
                    custom_type_name: None,
                    generated: None,
                    comment: None,
                    auto_increment: false,
                },
            ],
        };

        let plan = LogicalPlan::Filter {
            predicate: Expr::BinaryOp {
                left: Box::new(Expr::Identifier(vec!["age".into()])),
                op: crate::ast::BinaryOp::Gt,
                right: Box::new(Expr::Literal(Value::Int64(28))),
            },
            input: Box::new(LogicalPlan::Scan {
                table: crate::ast::TableName {
                    schema: None,
                    name: "users".into(),
                },
                alias: None,
                schema,
            }),
        };

        let mut exec = build_iter_plan(&plan, &table_data);
        let out = collect(exec.as_mut());
        assert_eq!(out.len(), 2);
    }
}
