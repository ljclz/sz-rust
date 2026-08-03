//! Phase 5.3/5.4 — RBO 优化规则
//!
//! # Phase 5.3 谓词下推（Predicate Pushdown）
//!
//! 将 Filter 谓词尽可能下推到靠近数据源（Scan）的位置，减少上游算子处理的数据量。
//!
//! # Phase 5.4 投影裁剪（Projection Pruning）
//!
//! 分析整棵计划树，收集每个 Scan 节点被上层算子引用的列，裁剪 Scan.schema 仅保留
//! 所需列。配合执行器按 schema 投影列，减少上游算子处理的列数与内存占用。
//!
//! ## 算法
//!
//! 1. **第一遍**：递归遍历计划树，收集所有"双层标识符"列引用（`table.col`），
//!    按表名小写分组存储到 `HashMap<String, HashSet<String>>`
//! 2. **第二遍**：递归遍历计划树，对每个 Scan 节点：
//!    - 按表名/别名查找所需列集合
//!    - 若找到且列数 < 原 schema 列数，裁剪 schema 仅保留所需列（保持原列顺序）
//!    - 若未找到（如 `SELECT *` 全展开为单层标识符），保留原 schema
//!
//! ## 限制
//!
//! - 仅裁剪"双层标识符"引用的列；单层标识符（如 `SELECT c0 FROM t`）不裁剪
//! - 不处理表达式中的列引用（如 `SELECT a.c0 + b.c1`）—— 实际上会处理，因 collect_column_refs_grouped 递归遍历
//! - 不修改 Scan 之外的节点结构
//! - 配合 executor.rs::execute_scan 按 schema 投影列

use std::collections::{HashMap, HashSet};

use szrsql_sql::ast::{BinaryOp, Expr, JoinCondition, JoinType, Select, SelectItem, Statement};
use szrsql_sql::plan::{IndexDefinition, LogicalPlan, Planner};
use szrsql_types::value::Value;

// =====================================================================
//  PredicatePushdown
// =====================================================================

/// 谓词下推规则应用器
pub struct PredicatePushdown;

impl PredicatePushdown {
    /// 应用谓词下推规则
    ///
    /// 递归自底向上：先处理子节点，再处理当前节点。
    pub fn apply(plan: LogicalPlan) -> LogicalPlan {
        Self::apply_recursive(plan)
    }

    fn apply_recursive(plan: LogicalPlan) -> LogicalPlan {
        match plan {
            LogicalPlan::Filter { predicate, input } => {
                // 先递归处理子节点
                let input = Self::apply_recursive(*input);
                // 然后尝试下推当前谓词
                Self::pushdown_filter(predicate, input)
            }
            LogicalPlan::Projection {
                exprs,
                output_names,
                input,
            } => {
                let input = Self::apply_recursive(*input);
                LogicalPlan::Projection {
                    exprs,
                    output_names,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
            } => {
                let left = Self::apply_recursive(*left);
                let right = Self::apply_recursive(*right);
                LogicalPlan::Join {
                    join_type,
                    condition,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            }
            LogicalPlan::Aggregate {
                grouping_sets,
                aggregates,
                having,
                input,
            } => {
                let input = Self::apply_recursive(*input);
                LogicalPlan::Aggregate {
                    grouping_sets,
                    aggregates,
                    having,
                    input: Box::new(input),
                }
            }
            // Phase 6.2: Window 节点 — 递归优化 input 子计划
            LogicalPlan::Window {
                window_funcs,
                input,
            } => {
                let input = Self::apply_recursive(*input);
                LogicalPlan::Window {
                    window_funcs,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Sort { order_by, input } => {
                let input = Self::apply_recursive(*input);
                LogicalPlan::Sort {
                    order_by,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Limit {
                limit,
                offset,
                input,
            } => {
                let input = Self::apply_recursive(*input);
                LogicalPlan::Limit {
                    limit,
                    offset,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Distinct { input } => {
                let input = Self::apply_recursive(*input);
                LogicalPlan::Distinct {
                    input: Box::new(input),
                }
            }
            // 其他节点（Scan、DML、DDL）无子查询需要下推
            other => other,
        }
    }

    /// 将谓词下推到子计划中
    ///
    /// 根据子计划类型选择下推策略：
    /// - Join：按表名拆分谓词，分别下推
    /// - Projection：穿透（简化版，不重写列引用）
    /// - Sort/Limit/Distinct：穿透
    /// - Filter：合并
    /// - 其他：保持 Filter 不变
    fn pushdown_filter(predicate: Expr, input: LogicalPlan) -> LogicalPlan {
        match input {
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
            } => Self::pushdown_to_join(predicate, join_type, condition, left, right),
            LogicalPlan::Sort { order_by, input } => {
                let inner = Self::pushdown_filter(predicate, *input);
                LogicalPlan::Sort {
                    order_by,
                    input: Box::new(inner),
                }
            }
            LogicalPlan::Distinct { input } => {
                let inner = Self::pushdown_filter(predicate, *input);
                LogicalPlan::Distinct {
                    input: Box::new(inner),
                }
            }
            LogicalPlan::Filter {
                predicate: existing,
                input,
            } => {
                // 合并两个 Filter：AND 连接
                let combined = Expr::BinaryOp {
                    left: Box::new(existing),
                    op: BinaryOp::And,
                    right: Box::new(predicate),
                };
                Self::pushdown_filter(combined, *input)
            }
            // Projection/Aggregate/Scan/Limit/Join 子节点未匹配时保持 Filter
            other => LogicalPlan::Filter {
                predicate,
                input: Box::new(other),
            },
        }
    }

    /// 将谓词下推到 Join
    ///
    /// 拆分谓词为：
    /// - `left_preds`：仅引用左表列的谓词
    /// - `right_preds`：仅引用右表列的谓词
    /// - `remaining`：引用两侧列的谓词（保留在 Join 上方）
    ///
    /// Outer Join 限制：
    /// - LeftOuter：右侧谓词不下推
    /// - RightOuter：左侧谓词不下推
    /// - FullOuter：两侧都不下推
    fn pushdown_to_join(
        predicate: Expr,
        join_type: JoinType,
        condition: szrsql_sql::ast::JoinCondition,
        left: Box<LogicalPlan>,
        right: Box<LogicalPlan>,
    ) -> LogicalPlan {
        // 收集左右表的别名/表名
        let left_tables = collect_table_aliases(&left);
        let right_tables = collect_table_aliases(&right);

        // 拆分谓词
        let predicates = split_conjuncts(&predicate);
        let mut left_preds = Vec::new();
        let mut right_preds = Vec::new();
        let mut remaining = Vec::new();

        for pred in predicates {
            let refs = collect_column_refs(&pred);
            let refs_left = refs.iter().all(|t| left_tables.contains(t));
            let refs_right = refs.iter().all(|t| right_tables.contains(t));

            if refs.is_empty() {
                // 无列引用（常量谓词）→ 下推到左侧
                left_preds.push(pred);
            } else if refs_left {
                // 仅引用左表
                if matches!(join_type, JoinType::RightOuter | JoinType::FullOuter) {
                    // Outer Join 限制：RightOuter 左侧不下推，FullOuter 两侧都不下推
                    remaining.push(pred);
                } else {
                    left_preds.push(pred);
                }
            } else if refs_right {
                // 仅引用右表
                if matches!(join_type, JoinType::LeftOuter | JoinType::FullOuter) {
                    // Outer Join 限制：LeftOuter 右侧不下推，FullOuter 两侧都不下推
                    remaining.push(pred);
                } else {
                    right_preds.push(pred);
                }
            } else {
                // 引用两侧列 → 保留
                remaining.push(pred);
            }
        }

        // 构建下推后的左子树
        let new_left = if left_preds.is_empty() {
            *left
        } else {
            let combined = combine_conjuncts(left_preds);
            LogicalPlan::Filter {
                predicate: combined,
                input: left,
            }
        };

        // 构建下推后的右子树
        let new_right = if right_preds.is_empty() {
            *right
        } else {
            let combined = combine_conjuncts(right_preds);
            LogicalPlan::Filter {
                predicate: combined,
                input: right,
            }
        };

        // 构建新的 Join
        let new_join = LogicalPlan::Join {
            join_type,
            condition,
            left: Box::new(new_left),
            right: Box::new(new_right),
        };

        // 若有剩余谓词，包裹 Filter
        if remaining.is_empty() {
            new_join
        } else {
            let combined = combine_conjuncts(remaining);
            LogicalPlan::Filter {
                predicate: combined,
                input: Box::new(new_join),
            }
        }
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 收集计划中所有表的别名/表名（用于谓词引用解析）
fn collect_table_aliases(plan: &LogicalPlan) -> Vec<String> {
    let mut aliases = Vec::new();
    collect_aliases_recursive(plan, &mut aliases);
    aliases
}

fn collect_aliases_recursive(plan: &LogicalPlan, out: &mut Vec<String>) {
    match plan {
        LogicalPlan::Scan { table, alias, .. } => {
            // 表名（全小写）
            out.push(table.name.to_lowercase());
            if let Some(a) = alias {
                out.push(a.to_lowercase());
            }
        }
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Projection { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Distinct { input, .. }
        | LogicalPlan::Aggregate { input, .. } => {
            collect_aliases_recursive(input, out);
        }
        LogicalPlan::Join { left, right, .. } => {
            collect_aliases_recursive(left, out);
            collect_aliases_recursive(right, out);
        }
        _ => {}
    }
}

/// 收集表达式中所有列引用的表名（前缀）
///
/// 对于 `col`（无前缀）返回空字符串（无法判断归属）
/// 对于 `table.col` 返回 `table`（小写）
fn collect_column_refs(expr: &Expr) -> Vec<String> {
    let mut refs = Vec::new();
    collect_column_refs_recursive(expr, &mut refs);
    refs
}

fn collect_column_refs_recursive(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Identifier(parts) => {
            if parts.len() >= 2 {
                // table.col → 取倒数第二部分作为表名
                let table = parts[parts.len() - 2].to_lowercase();
                out.push(table);
            }
            // 单层标识符（col）无法判断归属，忽略
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_column_refs_recursive(left, out);
            collect_column_refs_recursive(right, out);
        }
        Expr::UnaryOp { expr, .. } => {
            collect_column_refs_recursive(expr, out);
        }
        Expr::Function { args, .. } => {
            for arg in args {
                collect_column_refs_recursive(arg, out);
            }
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            if let Some(op) = operand {
                collect_column_refs_recursive(op, out);
            }
            for (when, then) in when_then {
                collect_column_refs_recursive(when, out);
                collect_column_refs_recursive(then, out);
            }
            if let Some(e) = else_expr {
                collect_column_refs_recursive(e, out);
            }
        }
        Expr::Cast { expr, .. } => collect_column_refs_recursive(expr, out),
        Expr::InList { expr, list, .. } => {
            collect_column_refs_recursive(expr, out);
            for item in list {
                collect_column_refs_recursive(item, out);
            }
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_column_refs_recursive(expr, out);
            collect_column_refs_recursive(low, out);
            collect_column_refs_recursive(high, out);
        }
        Expr::Like { expr, pattern, .. } => {
            collect_column_refs_recursive(expr, out);
            collect_column_refs_recursive(pattern, out);
        }
        Expr::IsNull { expr, .. } => {
            collect_column_refs_recursive(expr, out);
        }
        // 其他表达式类型不涉及列引用
        _ => {}
    }
}

/// 将谓词按 AND 拆分为列表
///
/// `a AND b AND c` → `[a, b, c]`
fn split_conjuncts(expr: &Expr) -> Vec<Expr> {
    let mut result = Vec::new();
    split_conjuncts_recursive(expr, &mut result);
    result
}

fn split_conjuncts_recursive(expr: &Expr, out: &mut Vec<Expr>) {
    match expr {
        Expr::BinaryOp {
            left,
            op: BinaryOp::And,
            right,
        } => {
            split_conjuncts_recursive(left, out);
            split_conjuncts_recursive(right, out);
        }
        other => out.push(other.clone()),
    }
}

/// 将谓词列表用 AND 连接为单个表达式
///
/// `[]` → 返回 `Expr::Literal(Bool(true))`（恒真谓词，不应被调用）
/// `[a]` → `a`
/// `[a, b, c]` → `a AND (b AND c)`
fn combine_conjuncts(preds: Vec<Expr>) -> Expr {
    if preds.is_empty() {
        return Expr::Literal(szrsql_types::value::Value::Bool(true));
    }
    if preds.len() == 1 {
        return preds.into_iter().next().unwrap();
    }
    let mut iter = preds.into_iter();
    let mut acc = iter.next().unwrap();
    for pred in iter {
        acc = Expr::BinaryOp {
            left: Box::new(acc),
            op: BinaryOp::And,
            right: Box::new(pred),
        };
    }
    acc
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_sql::ast::{ColumnDefinition, Expr as AExpr, JoinCondition, OrderByExpr, TableName};
    use szrsql_sql::plan::{LogicalPlan, TableSchema};
    use szrsql_types::value::{ColumnType, Value};

    /// 构建 Scan 计划
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

    /// 构建带别名的 Scan 计划
    fn build_scan_plan_with_alias(table_name: &str, alias: &str, num_cols: usize) -> LogicalPlan {
        let mut columns = Vec::with_capacity(num_cols);
        for i in 0..num_cols {
            columns.push(ColumnDefinition::new(format!("c{i}"), ColumnType::Int64));
        }
        LogicalPlan::Scan {
            table: TableName::new(table_name),
            alias: Some(alias.to_string()),
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

    /// 构建 Inner Join 计划
    fn build_inner_join(left: LogicalPlan, right: LogicalPlan, condition: AExpr) -> LogicalPlan {
        LogicalPlan::Join {
            join_type: JoinType::Inner,
            condition: JoinCondition::On(condition),
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// 构建 LeftOuter Join 计划
    fn build_left_outer_join(
        left: LogicalPlan,
        right: LogicalPlan,
        condition: AExpr,
    ) -> LogicalPlan {
        LogicalPlan::Join {
            join_type: JoinType::LeftOuter,
            condition: JoinCondition::On(condition),
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// 构建列引用表达式 `table.col`
    fn col_ref(table: &str, col: &str) -> AExpr {
        AExpr::Identifier(vec![table.into(), col.into()])
    }

    /// 构建字面量
    fn lit_int(n: i64) -> AExpr {
        AExpr::Literal(Value::Int64(n))
    }

    /// 构建等值谓词 `table.col = literal`
    fn eq_pred(table: &str, col: &str, n: i64) -> AExpr {
        AExpr::BinaryOp {
            left: Box::new(col_ref(table, col)),
            op: BinaryOp::Eq,
            right: Box::new(lit_int(n)),
        }
    }

    /// 构建范围谓词 `table.col > literal`
    fn gt_pred(table: &str, col: &str, n: i64) -> AExpr {
        AExpr::BinaryOp {
            left: Box::new(col_ref(table, col)),
            op: BinaryOp::Gt,
            right: Box::new(lit_int(n)),
        }
    }

    /// 构建等值 JOIN 条件 `left.col = right.col`
    fn equi_join_cond(
        left_table: &str,
        left_col: &str,
        right_table: &str,
        right_col: &str,
    ) -> AExpr {
        AExpr::BinaryOp {
            left: Box::new(col_ref(left_table, left_col)),
            op: BinaryOp::Eq,
            right: Box::new(col_ref(right_table, right_col)),
        }
    }

    /// 构建两表 JOIN（a JOIN b ON a.id=b.id）
    fn build_two_table_join() -> LogicalPlan {
        let a = build_scan_plan_with_alias("a", "a", 3);
        let b = build_scan_plan_with_alias("b", "b", 3);
        build_inner_join(a, b, equi_join_cond("a", "id", "b", "id"))
    }

    /// 断言计划中存在 Filter 节点
    fn count_filters(plan: &LogicalPlan) -> usize {
        let mut count = 0;
        count_filters_recursive(plan, &mut count);
        count
    }

    fn count_filters_recursive(plan: &LogicalPlan, count: &mut usize) {
        match plan {
            LogicalPlan::Filter { input, .. } => {
                *count += 1;
                count_filters_recursive(input, count);
            }
            LogicalPlan::Projection { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Distinct { input, .. }
            | LogicalPlan::Aggregate { input, .. } => {
                count_filters_recursive(input, count);
            }
            LogicalPlan::Join { left, right, .. } => {
                count_filters_recursive(left, count);
                count_filters_recursive(right, count);
            }
            _ => {}
        }
    }

    /// 检查 Filter 是否在指定表名的 Scan 上方
    fn is_filter_above_scan(plan: &LogicalPlan, table_name: &str) -> bool {
        if let LogicalPlan::Filter { input, .. } = plan {
            if let LogicalPlan::Scan { table, .. } = input.as_ref() {
                return table.name.eq_ignore_ascii_case(table_name);
            }
        }
        false
    }

    /// 获取 Join 左子树
    fn get_join_left(plan: &LogicalPlan) -> Option<&LogicalPlan> {
        if let LogicalPlan::Join { left, .. } = plan {
            Some(left.as_ref())
        } else {
            None
        }
    }

    /// 获取 Join 右子树
    fn get_join_right(plan: &LogicalPlan) -> Option<&LogicalPlan> {
        if let LogicalPlan::Join { right, .. } = plan {
            Some(right.as_ref())
        } else {
            None
        }
    }

    #[test]
    fn test_pushdown_filter_to_join_inner() {
        // SELECT * FROM a JOIN b ON a.id=b.id WHERE a.x > 10
        // 预期：谓词下推到 a 表 Scan 上方
        let join = build_two_table_join();
        let filter = build_filter_plan(gt_pred("a", "x", 10), join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Join（谓词已下推）
        assert!(matches!(optimized, LogicalPlan::Join { .. }));
        // 左子树（a 表）应有 Filter
        let left = get_join_left(&optimized).unwrap();
        assert!(matches!(left, LogicalPlan::Filter { .. }));
        assert!(is_filter_above_scan(left, "a"));
        // 右子树（b 表）应无 Filter
        let right = get_join_right(&optimized).unwrap();
        assert!(matches!(right, LogicalPlan::Scan { .. }));
        // 总 Filter 数应为 1
        assert_eq!(count_filters(&optimized), 1);
    }

    #[test]
    fn test_pushdown_filter_to_join_both_sides() {
        // SELECT * FROM a JOIN b ON a.id=b.id WHERE a.x > 10 AND b.y = 5
        // 预期：a.x > 10 下推到 a，b.y = 5 下推到 b
        let join = build_two_table_join();
        let pred = AExpr::BinaryOp {
            left: Box::new(gt_pred("a", "x", 10)),
            op: BinaryOp::And,
            right: Box::new(eq_pred("b", "y", 5)),
        };
        let filter = build_filter_plan(pred, join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Join
        assert!(matches!(optimized, LogicalPlan::Join { .. }));
        // 左右子树都应有 Filter
        let left = get_join_left(&optimized).unwrap();
        assert!(matches!(left, LogicalPlan::Filter { .. }));
        let right = get_join_right(&optimized).unwrap();
        assert!(matches!(right, LogicalPlan::Filter { .. }));
        // 总 Filter 数应为 2
        assert_eq!(count_filters(&optimized), 2);
    }

    #[test]
    fn test_pushdown_filter_join_condition_remaining() {
        // SELECT * FROM a JOIN b ON a.id=b.id WHERE a.x > b.y
        // 预期：谓词引用两侧列，保留在 Join 上方的 Filter 中
        let join = build_two_table_join();
        let pred = AExpr::BinaryOp {
            left: Box::new(col_ref("a", "x")),
            op: BinaryOp::Gt,
            right: Box::new(col_ref("b", "y")),
        };
        let filter = build_filter_plan(pred, join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Filter（谓词未下推）
        assert!(matches!(optimized, LogicalPlan::Filter { .. }));
        // Filter 下方是 Join
        if let LogicalPlan::Filter { input, .. } = &optimized {
            assert!(matches!(input.as_ref(), LogicalPlan::Join { .. }));
        }
        // 左右子树应无 Filter
        if let LogicalPlan::Filter { input, .. } = &optimized {
            if let LogicalPlan::Join { left, right, .. } = input.as_ref() {
                assert!(matches!(left.as_ref(), LogicalPlan::Scan { .. }));
                assert!(matches!(right.as_ref(), LogicalPlan::Scan { .. }));
            }
        }
        // 总 Filter 数应为 1（顶部）
        assert_eq!(count_filters(&optimized), 1);
    }

    #[test]
    fn test_pushdown_filter_left_outer_join_restricts_right() {
        // SELECT * FROM a LEFT JOIN b ON a.id=b.id WHERE b.y = 5
        // 预期：LeftOuter 时右侧谓词不下推（避免过滤掉 a 表的空值匹配）
        let a = build_scan_plan_with_alias("a", "a", 3);
        let b = build_scan_plan_with_alias("b", "b", 3);
        let join = build_left_outer_join(a, b, equi_join_cond("a", "id", "b", "id"));
        let filter = build_filter_plan(eq_pred("b", "y", 5), join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Filter（谓词未下推）
        assert!(matches!(optimized, LogicalPlan::Filter { .. }));
        // 右子树（b 表）应无 Filter
        if let LogicalPlan::Filter { input, .. } = &optimized {
            if let LogicalPlan::Join { right, .. } = input.as_ref() {
                assert!(matches!(right.as_ref(), LogicalPlan::Scan { .. }));
            }
        }
    }

    #[test]
    fn test_pushdown_filter_left_outer_join_allows_left() {
        // SELECT * FROM a LEFT JOIN b ON a.id=b.id WHERE a.x > 10
        // 预期：LeftOuter 时左侧谓词可下推
        let a = build_scan_plan_with_alias("a", "a", 3);
        let b = build_scan_plan_with_alias("b", "b", 3);
        let join = build_left_outer_join(a, b, equi_join_cond("a", "id", "b", "id"));
        let filter = build_filter_plan(gt_pred("a", "x", 10), join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Join（谓词已下推到左侧）
        assert!(matches!(optimized, LogicalPlan::Join { .. }));
        let left = get_join_left(&optimized).unwrap();
        assert!(matches!(left, LogicalPlan::Filter { .. }));
    }

    #[test]
    fn test_pushdown_filter_through_sort() {
        // SELECT * FROM t WHERE c0 > 5 ORDER BY c0
        // 预期：Filter 穿透 Sort，下推到 Scan 上方
        let scan = build_scan_plan("t", 2);
        let sort = LogicalPlan::Sort {
            order_by: vec![OrderByExpr {
                expr: AExpr::Identifier(vec!["c0".into()]),
                asc: true,
                nulls_first: false,
            }],
            input: Box::new(scan),
        };
        let pred = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["c0".into()])),
            op: BinaryOp::Gt,
            right: Box::new(lit_int(5)),
        };
        let filter = build_filter_plan(pred, sort);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Sort
        assert!(matches!(optimized, LogicalPlan::Sort { .. }));
        // Sort 下方应为 Filter
        if let LogicalPlan::Sort { input, .. } = &optimized {
            assert!(matches!(input.as_ref(), LogicalPlan::Filter { .. }));
        }
    }

    #[test]
    fn test_pushdown_filter_through_distinct() {
        // SELECT DISTINCT * FROM t WHERE c0 > 5
        let scan = build_scan_plan("t", 2);
        let distinct = LogicalPlan::Distinct {
            input: Box::new(scan),
        };
        let pred = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["c0".into()])),
            op: BinaryOp::Gt,
            right: Box::new(lit_int(5)),
        };
        let filter = build_filter_plan(pred, distinct);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Distinct
        assert!(matches!(optimized, LogicalPlan::Distinct { .. }));
        // Distinct 下方应为 Filter
        if let LogicalPlan::Distinct { input, .. } = &optimized {
            assert!(matches!(input.as_ref(), LogicalPlan::Filter { .. }));
        }
    }

    #[test]
    fn test_merge_consecutive_filters() {
        // Filter(a > 5, Filter(b < 10, Scan))
        // 预期：合并为 Filter(a > 5 AND b < 10, Scan)
        let scan = build_scan_plan("t", 2);
        let inner_filter = build_filter_plan(
            AExpr::BinaryOp {
                left: Box::new(col_ref("t", "c1")),
                op: BinaryOp::Lt,
                right: Box::new(lit_int(10)),
            },
            scan,
        );
        let outer_filter = build_filter_plan(
            AExpr::BinaryOp {
                left: Box::new(col_ref("t", "c0")),
                op: BinaryOp::Gt,
                right: Box::new(lit_int(5)),
            },
            inner_filter,
        );

        let optimized = PredicatePushdown::apply(outer_filter);

        // 应合并为单个 Filter
        assert!(matches!(optimized, LogicalPlan::Filter { .. }));
        assert_eq!(count_filters(&optimized), 1);
        // Filter 下方直接是 Scan
        if let LogicalPlan::Filter { input, .. } = &optimized {
            assert!(matches!(input.as_ref(), LogicalPlan::Scan { .. }));
        }
    }

    #[test]
    fn test_split_conjuncts_single() {
        let pred = eq_pred("a", "x", 5);
        let conjuncts = split_conjuncts(&pred);
        assert_eq!(conjuncts.len(), 1);
    }

    #[test]
    fn test_split_conjuncts_multiple() {
        // a.x = 5 AND b.y > 10 AND a.z < 100
        let pred = AExpr::BinaryOp {
            left: Box::new(AExpr::BinaryOp {
                left: Box::new(eq_pred("a", "x", 5)),
                op: BinaryOp::And,
                right: Box::new(gt_pred("b", "y", 10)),
            }),
            op: BinaryOp::And,
            right: Box::new(AExpr::BinaryOp {
                left: Box::new(col_ref("a", "z")),
                op: BinaryOp::Lt,
                right: Box::new(lit_int(100)),
            }),
        };
        let conjuncts = split_conjuncts(&pred);
        assert_eq!(conjuncts.len(), 3);
    }

    #[test]
    fn test_combine_conjuncts_empty() {
        let combined = combine_conjuncts(vec![]);
        // 空列表返回恒真
        assert!(matches!(combined, AExpr::Literal(Value::Bool(true))));
    }

    #[test]
    fn test_combine_conjuncts_single() {
        let pred = eq_pred("a", "x", 5);
        let combined = combine_conjuncts(vec![pred.clone()]);
        assert_eq!(combined, pred);
    }

    #[test]
    fn test_combine_conjuncts_multiple() {
        let p1 = eq_pred("a", "x", 5);
        let p2 = gt_pred("b", "y", 10);
        let combined = combine_conjuncts(vec![p1, p2]);
        // 应为 AND 连接
        assert!(matches!(
            combined,
            AExpr::BinaryOp {
                op: BinaryOp::And,
                ..
            }
        ));
    }

    #[test]
    fn test_collect_column_refs_qualified() {
        // a.x = b.y
        let expr = AExpr::BinaryOp {
            left: Box::new(col_ref("a", "x")),
            op: BinaryOp::Eq,
            right: Box::new(col_ref("b", "y")),
        };
        let refs = collect_column_refs(&expr);
        assert!(refs.contains(&"a".to_string()));
        assert!(refs.contains(&"b".to_string()));
    }

    #[test]
    fn test_collect_column_refs_unqualified() {
        // c0 = 5（无表前缀）
        let expr = eq_pred("a", "x", 5);
        // 注：eq_pred 使用 col_ref，有表前缀
        let refs = collect_column_refs(&expr);
        assert!(refs.contains(&"a".to_string()));
    }

    #[test]
    fn test_collect_table_aliases_scan() {
        let plan = build_scan_plan_with_alias("users", "u", 3);
        let aliases = collect_table_aliases(&plan);
        assert!(aliases.contains(&"users".to_string()));
        assert!(aliases.contains(&"u".to_string()));
    }

    #[test]
    fn test_collect_table_aliases_join() {
        let a = build_scan_plan_with_alias("a", "a", 3);
        let b = build_scan_plan_with_alias("b", "b", 3);
        let join = build_inner_join(a, b, equi_join_cond("a", "id", "b", "id"));
        let aliases = collect_table_aliases(&join);
        assert!(aliases.contains(&"a".to_string()));
        assert!(aliases.contains(&"b".to_string()));
    }

    #[test]
    fn test_pushdown_no_change_for_scan() {
        // Filter(c0 > 5, Scan) → 无下推空间
        let scan = build_scan_plan("t", 2);
        let pred = AExpr::BinaryOp {
            left: Box::new(col_ref("t", "c0")),
            op: BinaryOp::Gt,
            right: Box::new(lit_int(5)),
        };
        let filter = build_filter_plan(pred, scan);

        let optimized = PredicatePushdown::apply(filter);

        // 应保持 Filter 结构
        assert!(matches!(optimized, LogicalPlan::Filter { .. }));
        assert_eq!(count_filters(&optimized), 1);
    }

    #[test]
    fn test_pushdown_constant_predicate() {
        // SELECT * FROM a JOIN b ON a.id=b.id WHERE 1 = 1
        // 预期：常量谓词下推到左侧（不影响正确性，因常量对所有行成立）
        let join = build_two_table_join();
        let pred = AExpr::BinaryOp {
            left: Box::new(lit_int(1)),
            op: BinaryOp::Eq,
            right: Box::new(lit_int(1)),
        };
        let filter = build_filter_plan(pred, join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Join（谓词已下推）
        assert!(matches!(optimized, LogicalPlan::Join { .. }));
    }

    #[test]
    fn test_pushdown_complex_predicate() {
        // SELECT * FROM a JOIN b ON a.id=b.id
        // WHERE a.x > 10 AND (b.y = 5 OR a.z < 100)
        // a.x > 10 可下推到 a；(b.y = 5 OR a.z < 100) 引用两侧列，保留
        let join = build_two_table_join();
        let or_pred = AExpr::BinaryOp {
            left: Box::new(eq_pred("b", "y", 5)),
            op: BinaryOp::Or,
            right: Box::new(AExpr::BinaryOp {
                left: Box::new(col_ref("a", "z")),
                op: BinaryOp::Lt,
                right: Box::new(lit_int(100)),
            }),
        };
        let pred = AExpr::BinaryOp {
            left: Box::new(gt_pred("a", "x", 10)),
            op: BinaryOp::And,
            right: Box::new(or_pred),
        };
        let filter = build_filter_plan(pred, join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Filter（OR 部分保留）
        assert!(matches!(optimized, LogicalPlan::Filter { .. }));
        // 左子树（a 表）应有 Filter（a.x > 10 下推）
        if let LogicalPlan::Filter { input, .. } = &optimized {
            if let LogicalPlan::Join { left, .. } = input.as_ref() {
                assert!(matches!(left.as_ref(), LogicalPlan::Filter { .. }));
            }
        }
    }

    #[test]
    fn test_pushdown_nested_join() {
        // SELECT * FROM (a JOIN b) JOIN c
        // WHERE a.x > 10 AND c.z = 5
        let a = build_scan_plan_with_alias("a", "a", 3);
        let b = build_scan_plan_with_alias("b", "b", 3);
        let ab_join = build_inner_join(a, b, equi_join_cond("a", "id", "b", "id"));
        let c = build_scan_plan_with_alias("c", "c", 3);
        let abc_join = build_inner_join(ab_join, c, equi_join_cond("a", "id", "c", "id"));

        let pred = AExpr::BinaryOp {
            left: Box::new(gt_pred("a", "x", 10)),
            op: BinaryOp::And,
            right: Box::new(eq_pred("c", "z", 5)),
        };
        let filter = build_filter_plan(pred, abc_join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Join
        assert!(matches!(optimized, LogicalPlan::Join { .. }));
        // c.z = 5 应下推到 c 表
        let right = get_join_right(&optimized).unwrap();
        assert!(matches!(right, LogicalPlan::Filter { .. }));
    }

    #[test]
    fn test_pushdown_preserves_join_condition() {
        // 验证下推后 JOIN 条件保持不变
        let join = build_two_table_join();
        let filter = build_filter_plan(gt_pred("a", "x", 10), join);

        let optimized = PredicatePushdown::apply(filter);

        if let LogicalPlan::Join { condition, .. } = &optimized {
            // 验证 JOIN 条件仍为 On(a.id = b.id)
            assert!(matches!(condition, JoinCondition::On(_)));
        } else {
            panic!("Expected Join after pushdown");
        }
    }

    #[test]
    fn test_pushdown_full_outer_join_no_pushdown() {
        // SELECT * FROM a FULL JOIN b ON a.id=b.id WHERE a.x > 10 AND b.y = 5
        // 预期：FullOuter 时两侧都不下推
        let a = build_scan_plan_with_alias("a", "a", 3);
        let b = build_scan_plan_with_alias("b", "b", 3);
        let join = LogicalPlan::Join {
            join_type: JoinType::FullOuter,
            condition: JoinCondition::On(equi_join_cond("a", "id", "b", "id")),
            left: Box::new(a),
            right: Box::new(b),
        };
        let pred = AExpr::BinaryOp {
            left: Box::new(gt_pred("a", "x", 10)),
            op: BinaryOp::And,
            right: Box::new(eq_pred("b", "y", 5)),
        };
        let filter = build_filter_plan(pred, join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Filter（所有谓词保留）
        assert!(matches!(optimized, LogicalPlan::Filter { .. }));
        // 左右子树应无 Filter
        if let LogicalPlan::Filter { input, .. } = &optimized {
            if let LogicalPlan::Join { left, right, .. } = input.as_ref() {
                assert!(matches!(left.as_ref(), LogicalPlan::Scan { .. }));
                assert!(matches!(right.as_ref(), LogicalPlan::Scan { .. }));
            }
        }
    }

    #[test]
    fn test_pushdown_right_outer_join_restricts_left() {
        // SELECT * FROM a RIGHT JOIN b ON a.id=b.id WHERE a.x > 10
        // 预期：RightOuter 时左侧谓词不下推
        let a = build_scan_plan_with_alias("a", "a", 3);
        let b = build_scan_plan_with_alias("b", "b", 3);
        let join = LogicalPlan::Join {
            join_type: JoinType::RightOuter,
            condition: JoinCondition::On(equi_join_cond("a", "id", "b", "id")),
            left: Box::new(a),
            right: Box::new(b),
        };
        let filter = build_filter_plan(gt_pred("a", "x", 10), join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Filter
        assert!(matches!(optimized, LogicalPlan::Filter { .. }));
        // 左子树（a 表）应无 Filter
        if let LogicalPlan::Filter { input, .. } = &optimized {
            if let LogicalPlan::Join { left, .. } = input.as_ref() {
                assert!(matches!(left.as_ref(), LogicalPlan::Scan { .. }));
            }
        }
    }

    #[test]
    fn test_pushdown_right_outer_join_allows_right() {
        // SELECT * FROM a RIGHT JOIN b ON a.id=b.id WHERE b.y = 5
        // 预期：RightOuter 时右侧谓词可下推
        let a = build_scan_plan_with_alias("a", "a", 3);
        let b = build_scan_plan_with_alias("b", "b", 3);
        let join = LogicalPlan::Join {
            join_type: JoinType::RightOuter,
            condition: JoinCondition::On(equi_join_cond("a", "id", "b", "id")),
            left: Box::new(a),
            right: Box::new(b),
        };
        let filter = build_filter_plan(eq_pred("b", "y", 5), join);

        let optimized = PredicatePushdown::apply(filter);

        // 顶层应为 Join
        assert!(matches!(optimized, LogicalPlan::Join { .. }));
        let right = get_join_right(&optimized).unwrap();
        assert!(matches!(right, LogicalPlan::Filter { .. }));
    }
}

// =====================================================================
//  ProjectionPruning — Phase 5.4
// =====================================================================

/// 投影裁剪规则应用器
///
/// 分析计划树，收集每个 Scan 节点被上层算子引用的列，裁剪 Scan.schema 仅保留所需列。
///
/// # 算法
///
/// 1. 第一遍：递归遍历计划树，收集所有"双层标识符"列引用（`table.col`），按表名小写分组
/// 2. 第二遍：递归遍历计划树，对每个 Scan 节点按表名/别名查找所需列，裁剪 schema
///
/// # 限制
///
/// - 仅裁剪"双层标识符"引用的列；单层标识符（如 `SELECT c0 FROM t`）不裁剪
/// - 配合 executor.rs::execute_scan 按 schema 投影列
pub struct ProjectionPruning;

impl ProjectionPruning {
    /// 分析计划树，返回每个 Scan 表的所需列名列表（按字母升序）
    ///
    /// 仅收集"双层标识符"列引用（`table.col`），单层标识符（`col`）不参与裁剪。
    pub fn analyze(plan: &LogicalPlan) -> HashMap<String, Vec<String>> {
        let mut table_cols: HashMap<String, HashSet<String>> = HashMap::new();
        collect_required_columns(plan, &mut table_cols);
        // 转换为 Vec<String> 并按字母升序排列（便于测试断言）
        table_cols
            .into_iter()
            .map(|(k, v)| {
                let mut cols: Vec<String> = v.into_iter().collect();
                cols.sort();
                (k, cols)
            })
            .collect()
    }

    /// 应用投影裁剪规则，返回裁剪后的计划
    ///
    /// 对每个 Scan 节点，按表名/别名查找所需列，裁剪 schema 仅保留所需列。
    /// 若所需列集合为空或包含所有列，则不裁剪。
    pub fn apply(plan: LogicalPlan) -> LogicalPlan {
        let table_cols = Self::analyze(&plan);
        Self::apply_recursive(plan, &table_cols)
    }

    fn apply_recursive(
        plan: LogicalPlan,
        table_cols: &HashMap<String, Vec<String>>,
    ) -> LogicalPlan {
        match plan {
            LogicalPlan::Scan {
                table,
                alias,
                schema,
            } => {
                // 按别名优先、表名其次查找所需列
                let key = alias
                    .clone()
                    .unwrap_or_else(|| table.name.clone())
                    .to_lowercase();
                if let Some(required) = table_cols.get(&key) {
                    if !required.is_empty() && required.len() < schema.columns.len() {
                        // 裁剪 schema：仅保留所需列，保持原列顺序
                        let required_set: HashSet<&str> =
                            required.iter().map(|s| s.as_str()).collect();
                        let pruned_columns = schema
                            .columns
                            .into_iter()
                            .filter(|c| required_set.contains(c.name.to_lowercase().as_str()))
                            .collect();
                        return LogicalPlan::Scan {
                            table,
                            alias,
                            schema: szrsql_sql::plan::TableSchema {
                                name: schema.name,
                                columns: pruned_columns,
                            },
                        };
                    }
                }
                LogicalPlan::Scan {
                    table,
                    alias,
                    schema,
                }
            }
            LogicalPlan::Projection {
                exprs,
                output_names,
                input,
            } => {
                let input = Self::apply_recursive(*input, table_cols);
                LogicalPlan::Projection {
                    exprs,
                    output_names,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Filter { predicate, input } => {
                let input = Self::apply_recursive(*input, table_cols);
                LogicalPlan::Filter {
                    predicate,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
            } => {
                let left = Self::apply_recursive(*left, table_cols);
                let right = Self::apply_recursive(*right, table_cols);
                LogicalPlan::Join {
                    join_type,
                    condition,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            }
            LogicalPlan::Aggregate {
                grouping_sets,
                aggregates,
                having,
                input,
            } => {
                let input = Self::apply_recursive(*input, table_cols);
                LogicalPlan::Aggregate {
                    grouping_sets,
                    aggregates,
                    having,
                    input: Box::new(input),
                }
            }
            // Phase 6.2: Window — 递归列裁剪 input 子计划
            LogicalPlan::Window {
                window_funcs,
                input,
            } => {
                let input = Self::apply_recursive(*input, table_cols);
                LogicalPlan::Window {
                    window_funcs,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Sort { order_by, input } => {
                let input = Self::apply_recursive(*input, table_cols);
                LogicalPlan::Sort {
                    order_by,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Limit {
                limit,
                offset,
                input,
            } => {
                let input = Self::apply_recursive(*input, table_cols);
                LogicalPlan::Limit {
                    limit,
                    offset,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Distinct { input } => {
                let input = Self::apply_recursive(*input, table_cols);
                LogicalPlan::Distinct {
                    input: Box::new(input),
                }
            }
            // 其他节点（DML/DDL）不递归
            other => other,
        }
    }
}

// =====================================================================
//  投影裁剪辅助函数
// =====================================================================

/// 递归遍历计划树，收集所有"双层标识符"列引用，按表名小写分组
fn collect_required_columns(plan: &LogicalPlan, out: &mut HashMap<String, HashSet<String>>) {
    match plan {
        LogicalPlan::Projection { exprs, input, .. } => {
            for (expr, _) in exprs {
                collect_expr_column_refs_grouped(expr, out);
            }
            collect_required_columns(input, out);
        }
        LogicalPlan::Filter { predicate, input } => {
            collect_expr_column_refs_grouped(predicate, out);
            collect_required_columns(input, out);
        }
        LogicalPlan::Join {
            condition,
            left,
            right,
            ..
        } => {
            if let szrsql_sql::ast::JoinCondition::On(expr) = condition {
                collect_expr_column_refs_grouped(expr, out);
            }
            collect_required_columns(left, out);
            collect_required_columns(right, out);
        }
        LogicalPlan::Sort { order_by, input } => {
            for ob in order_by {
                collect_expr_column_refs_grouped(&ob.expr, out);
            }
            collect_required_columns(input, out);
        }
        LogicalPlan::Aggregate {
            grouping_sets,
            aggregates,
            having,
            input,
        } => {
            // P3-1: 多分组集 — 收集所有集的列引用
            for set in grouping_sets {
                for expr in set {
                    collect_expr_column_refs_grouped(expr, out);
                }
            }
            for agg in aggregates {
                for arg in &agg.args {
                    collect_expr_column_refs_grouped(arg, out);
                }
            }
            if let Some(h) = having {
                collect_expr_column_refs_grouped(h, out);
            }
            collect_required_columns(input, out);
        }
        LogicalPlan::Limit { input, .. } | LogicalPlan::Distinct { input } => {
            collect_required_columns(input, out);
        }
        _ => {}
    }
}

/// 递归遍历表达式，收集"双层标识符"列引用（`table.col`），按表名小写分组
///
/// 单层标识符（`col`）无法判断归属，不收集。
fn collect_expr_column_refs_grouped(expr: &Expr, out: &mut HashMap<String, HashSet<String>>) {
    match expr {
        Expr::Identifier(parts) => {
            if parts.len() >= 2 {
                let table = parts[parts.len() - 2].to_lowercase();
                let col = parts[parts.len() - 1].to_lowercase();
                out.entry(table).or_default().insert(col);
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_expr_column_refs_grouped(left, out);
            collect_expr_column_refs_grouped(right, out);
        }
        Expr::UnaryOp { expr, .. } => {
            collect_expr_column_refs_grouped(expr, out);
        }
        Expr::Function { args, .. } => {
            for arg in args {
                collect_expr_column_refs_grouped(arg, out);
            }
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            if let Some(op) = operand {
                collect_expr_column_refs_grouped(op, out);
            }
            for (when, then) in when_then {
                collect_expr_column_refs_grouped(when, out);
                collect_expr_column_refs_grouped(then, out);
            }
            if let Some(e) = else_expr {
                collect_expr_column_refs_grouped(e, out);
            }
        }
        Expr::Cast { expr, .. } => collect_expr_column_refs_grouped(expr, out),
        Expr::InList { expr, list, .. } => {
            collect_expr_column_refs_grouped(expr, out);
            for item in list {
                collect_expr_column_refs_grouped(item, out);
            }
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_expr_column_refs_grouped(expr, out);
            collect_expr_column_refs_grouped(low, out);
            collect_expr_column_refs_grouped(high, out);
        }
        Expr::Like { expr, pattern, .. } => {
            collect_expr_column_refs_grouped(expr, out);
            collect_expr_column_refs_grouped(pattern, out);
        }
        Expr::IsNull { expr, .. } => {
            collect_expr_column_refs_grouped(expr, out);
        }
        _ => {}
    }
}

// =====================================================================
//  SubqueryFlattening — Phase 5.6
// =====================================================================

/// 子查询展平规则应用器 — Phase 5.6
///
/// 将 `WHERE expr IN (SELECT ...)` 展平为 SemiJoin，
/// `WHERE expr NOT IN (SELECT ...)` 展平为 AntiJoin，
/// `WHERE EXISTS (SELECT ... WHERE outer.col = inner.col)` 展平为 SemiJoin，
/// `WHERE NOT EXISTS (...)` 展平为 AntiJoin。
///
/// # 算法
///
/// 递归遍历计划树，对每个 `Filter` 节点：
/// 1. 将谓词按 AND 拆分为合取项
/// 2. 对每个合取项尝试展平：
///    - `InSubquery { expr, subquery, negated }`：
///      - 子查询必须输出单列且为简单列引用
///      - 规划子查询 → `sub_plan`
///      - 构建 JOIN 条件 `expr = subquery_col`
///      - `negated=false` → SemiJoin，`negated=true` → AntiJoin
///    - `Exists { subquery, negated }`：
///      - 必须为相关子查询（WHERE 引用外层表列）
///      - 从子查询 WHERE 提取相关谓词（引用外层表的合取项）作为 JOIN 条件
///      - 剩余谓词保留在子查询内
///      - `negated=false` → SemiJoin，`negated=true` → AntiJoin
/// 3. 展平成功的合取项从谓词中移除，替换为 JOIN
/// 4. 若所有合取项都展平，Filter 被完全替换；否则保留剩余谓词的 Filter
///
/// # 限制
///
/// - IN 子查询的投影必须为简单列引用（`col` 或 `table.col`）
/// - EXISTS 仅处理相关子查询；不相关 EXISTS 不展平（常量求值，留待后续优化）
/// - 不处理 SELECT 列表中的子查询（仅 WHERE 子句）
/// - 不处理嵌套子查询（子查询中再含子查询）
/// - 不处理含 GROUP BY/HAVING/LIMIT/ORDER BY/DISTINCT 的子查询（仅简单 SELECT ... FROM ... WHERE ...）
/// - 相关子查询的解相关仅提取等值/范围谓词作为 JOIN 条件，不实现完整的 Apply 算子
pub struct SubqueryFlattening<'a, 'c> {
    /// 规划器引用（用于将子查询 AST 转换为 LogicalPlan）
    planner: &'a Planner<'c>,
}

impl<'a, 'c> SubqueryFlattening<'a, 'c> {
    /// 创建子查询展平规则应用器
    pub fn new(planner: &'a Planner<'c>) -> Self {
        Self { planner }
    }

    /// 应用子查询展平规则
    pub fn apply(&self, plan: LogicalPlan) -> LogicalPlan {
        self.apply_recursive(plan)
    }

    /// 递归处理计划树
    fn apply_recursive(&self, plan: LogicalPlan) -> LogicalPlan {
        match plan {
            LogicalPlan::Filter { predicate, input } => {
                let input = self.apply_recursive(*input);
                self.flatten_filter(predicate, input)
            }
            LogicalPlan::Projection {
                exprs,
                output_names,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Projection {
                    exprs,
                    output_names,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
            } => {
                let left = self.apply_recursive(*left);
                let right = self.apply_recursive(*right);
                LogicalPlan::Join {
                    join_type,
                    condition,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            }
            LogicalPlan::Aggregate {
                grouping_sets,
                aggregates,
                having,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Aggregate {
                    grouping_sets,
                    aggregates,
                    having,
                    input: Box::new(input),
                }
            }
            // Phase 6.2: 窗口函数节点 — 递归处理 input
            LogicalPlan::Window {
                window_funcs,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Window {
                    window_funcs,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Sort { order_by, input } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Sort {
                    order_by,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Limit {
                limit,
                offset,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Limit {
                    limit,
                    offset,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Distinct { input } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Distinct {
                    input: Box::new(input),
                }
            }
            // 其他节点（Scan、DML、DDL）无 Filter 需要展平
            other => other,
        }
    }

    /// 处理 Filter 节点：尝试展平谓词中的子查询
    fn flatten_filter(&self, predicate: Expr, input: LogicalPlan) -> LogicalPlan {
        let conjuncts = split_conjuncts(&predicate);
        let mut remaining = Vec::new();
        let mut current_plan = input;

        for conjunct in conjuncts {
            if let Some(new_plan) = self.try_flatten_conjunct(&conjunct, current_plan.clone()) {
                current_plan = new_plan;
            } else {
                remaining.push(conjunct);
            }
        }

        if remaining.is_empty() {
            current_plan
        } else {
            LogicalPlan::Filter {
                predicate: combine_conjuncts(remaining),
                input: Box::new(current_plan),
            }
        }
    }

    /// 尝试展平单个合取项
    ///
    /// 返回 `Some(new_plan)` 表示展平成功；`None` 表示无法展平。
    fn try_flatten_conjunct(&self, conjunct: &Expr, input: LogicalPlan) -> Option<LogicalPlan> {
        match conjunct {
            Expr::InSubquery {
                expr,
                subquery,
                negated,
            } => self.flatten_in_subquery(expr, subquery, *negated, input),
            Expr::Exists { subquery, negated } => self.flatten_exists(subquery, *negated, input),
            _ => None,
        }
    }

    /// 展平 IN/NOT IN 子查询
    ///
    /// `expr IN (SELECT col FROM ...)` → `SemiJoin(input, sub_plan, ON expr = col)`
    /// `expr NOT IN (SELECT col FROM ...)` → `AntiJoin(input, sub_plan, ON expr = col)`
    fn flatten_in_subquery(
        &self,
        expr: &Expr,
        subquery: &Select,
        negated: bool,
        input: LogicalPlan,
    ) -> Option<LogicalPlan> {
        // 子查询限制：无集合操作、无 GROUP BY/HAVING、无 LIMIT/OFFSET、无 ORDER BY、无 DISTINCT
        if !Self::is_simple_subquery(subquery) {
            return None;
        }

        // 子查询投影必须为单列且为简单列引用
        if subquery.projection.len() != 1 {
            return None;
        }
        let sub_col_expr = extract_simple_column_from_select_item(&subquery.projection[0])?;

        // 规划子查询
        let sub_plan = self
            .planner
            .plan_statement(Statement::Select(Box::new(subquery.clone())))
            .ok()?;

        // 构建 JOIN 条件：expr = sub_col
        let condition = JoinCondition::On(Expr::BinaryOp {
            left: Box::new(expr.clone()),
            op: BinaryOp::Eq,
            right: Box::new(sub_col_expr),
        });

        let join_type = if negated {
            JoinType::Anti
        } else {
            JoinType::Semi
        };

        Some(LogicalPlan::Join {
            join_type,
            condition,
            left: Box::new(input),
            right: Box::new(sub_plan),
        })
    }

    /// 展平 EXISTS/NOT EXISTS 子查询（仅相关子查询）
    ///
    /// `EXISTS (SELECT ... FROM inner WHERE inner.col = outer.col AND ...)` →
    /// `SemiJoin(input, sub_plan_without_correlation, ON inner.col = outer.col)`
    fn flatten_exists(
        &self,
        subquery: &Select,
        negated: bool,
        input: LogicalPlan,
    ) -> Option<LogicalPlan> {
        // 子查询限制
        if !Self::is_simple_subquery(subquery) {
            return None;
        }

        // 收集外层计划的表名/别名
        let outer_tables = collect_table_aliases(&input);
        if outer_tables.is_empty() {
            return None;
        }

        // 从子查询 WHERE 提取相关谓词
        let where_clause = subquery.where_clause.as_ref()?;
        let conjuncts = split_conjuncts(where_clause);

        let mut correlation_preds = Vec::new();
        let mut remaining_preds = Vec::new();
        for conjunct in conjuncts {
            let refs = collect_column_refs(&conjunct);
            // 检查是否引用外层表（任一引用）
            let refs_outer = refs.iter().any(|t| outer_tables.contains(t));
            if refs_outer {
                correlation_preds.push(conjunct);
            } else {
                remaining_preds.push(conjunct);
            }
        }

        // 必须有相关谓词才展平（不相关 EXISTS 不处理）
        if correlation_preds.is_empty() {
            return None;
        }

        // 构建去相关后的子查询（WHERE 保留剩余谓词）
        let mut sub_select = subquery.clone();
        sub_select.where_clause = if remaining_preds.is_empty() {
            None
        } else {
            Some(combine_conjuncts(remaining_preds))
        };

        let sub_plan = self
            .planner
            .plan_statement(Statement::Select(Box::new(sub_select)))
            .ok()?;

        // JOIN 条件 = 所有相关谓词的 AND
        let condition = JoinCondition::On(combine_conjuncts(correlation_preds));

        let join_type = if negated {
            JoinType::Anti
        } else {
            JoinType::Semi
        };

        Some(LogicalPlan::Join {
            join_type,
            condition,
            left: Box::new(input),
            right: Box::new(sub_plan),
        })
    }

    /// 判断子查询是否为"简单子查询"（可展平）
    ///
    /// 简单子查询：无集合操作、无 GROUP BY/HAVING、无 LIMIT/OFFSET、无 ORDER BY、无 DISTINCT
    fn is_simple_subquery(subquery: &Select) -> bool {
        subquery.set_op.is_none()
            && subquery.group_by.is_empty()
            && subquery.having.is_none()
            && subquery.limit.is_none()
            && subquery.offset.is_none()
            && subquery.order_by.is_empty()
            && !subquery.distinct
    }
}

/// 从 SelectItem 提取简单列引用
///
/// - `UnnamedExpr(Identifier(["col"]))` → `Identifier(["col"])`
/// - `UnnamedExpr(Identifier(["table", "col"]))` → `Identifier(["table", "col"])`
/// - `ExprWithAlias { expr: Identifier(...), alias }` → `Identifier(...)`（忽略别名）
/// - 其他（Wildcard/表达式）→ None
fn extract_simple_column_from_select_item(item: &SelectItem) -> Option<Expr> {
    match item {
        SelectItem::UnnamedExpr(expr) | SelectItem::ExprWithAlias { expr, .. } => {
            if let Expr::Identifier(parts) = expr {
                if !parts.is_empty() {
                    return Some(Expr::Identifier(parts.clone()));
                }
            }
            None
        }
        SelectItem::Wildcard | SelectItem::QualifiedWildcard(_) => None,
    }
}

// =====================================================================
//  ProjectionPruning 单元测试
// =====================================================================

#[cfg(test)]
mod projection_pruning_tests {
    use super::*;
    use szrsql_sql::ast::{ColumnDefinition, Expr as AExpr, JoinCondition, OrderByExpr, TableName};
    use szrsql_sql::plan::{LogicalPlan, TableSchema};
    use szrsql_types::value::ColumnType;

    /// 构建 Scan 计划（指定列名列表）
    fn build_scan_with_columns(
        table_name: &str,
        alias: Option<&str>,
        cols: &[&str],
    ) -> LogicalPlan {
        let columns: Vec<ColumnDefinition> = cols
            .iter()
            .map(|c| ColumnDefinition::new(*c, ColumnType::Int64))
            .collect();
        LogicalPlan::Scan {
            table: TableName::new(table_name),
            alias: alias.map(|s| s.to_string()),
            schema: TableSchema {
                name: TableName::new(table_name),
                columns,
            },
        }
    }

    /// 构建 Projection 计划
    fn build_projection(
        exprs: Vec<AExpr>,
        output_names: Vec<String>,
        input: LogicalPlan,
    ) -> LogicalPlan {
        let paired: Vec<(AExpr, Option<String>)> = exprs
            .into_iter()
            .zip(output_names)
            .map(|(e, n)| (e, Some(n)))
            .collect();
        let output_names: Vec<String> = paired.iter().map(|(_, n)| n.clone().unwrap()).collect();
        LogicalPlan::Projection {
            exprs: paired,
            output_names,
            input: Box::new(input),
        }
    }

    /// 构建 Inner Join
    fn build_join(left: LogicalPlan, right: LogicalPlan, cond: AExpr) -> LogicalPlan {
        LogicalPlan::Join {
            join_type: JoinType::Inner,
            condition: JoinCondition::On(cond),
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// 构建 `table.col` 列引用
    fn col(table: &str, col: &str) -> AExpr {
        AExpr::Identifier(vec![table.into(), col.into()])
    }

    /// 构建 `table.col = table2.col2` 等值条件
    fn equi_cond(lt: &str, lc: &str, rt: &str, rc: &str) -> AExpr {
        AExpr::BinaryOp {
            left: Box::new(col(lt, lc)),
            op: BinaryOp::Eq,
            right: Box::new(col(rt, rc)),
        }
    }

    /// 获取 Scan 的列数
    fn scan_column_count(plan: &LogicalPlan) -> Option<usize> {
        fn find_scan(plan: &LogicalPlan) -> Option<&TableSchema> {
            match plan {
                LogicalPlan::Scan { schema, .. } => Some(schema),
                LogicalPlan::Filter { input, .. }
                | LogicalPlan::Projection { input, .. }
                | LogicalPlan::Sort { input, .. }
                | LogicalPlan::Limit { input, .. }
                | LogicalPlan::Distinct { input, .. }
                | LogicalPlan::Aggregate { input, .. } => find_scan(input),
                LogicalPlan::Join { left, right, .. } => {
                    find_scan(left).or_else(|| find_scan(right))
                }
                _ => None,
            }
        }
        find_scan(plan).map(|s| s.columns.len())
    }

    #[test]
    fn test_analyze_single_table_qualified() {
        // SELECT a.c0, a.c2 FROM a (a 有 c0/c1/c2/c3 四列)
        // 预期：所需列 = {c0, c2}
        let scan = build_scan_with_columns("a", Some("a"), &["c0", "c1", "c2", "c3"]);
        let proj = build_projection(
            vec![col("a", "c0"), col("a", "c2")],
            vec!["c0".to_string(), "c2".to_string()],
            scan,
        );

        let result = ProjectionPruning::analyze(&proj);
        let expected: Vec<String> = vec!["c0".to_string(), "c2".to_string()];
        assert_eq!(result.get("a").cloned(), Some(expected));
    }

    #[test]
    fn test_analyze_join_predicate_column_refs() {
        // SELECT a.name FROM a JOIN b ON a.id=b.id
        // 预期：a 表所需列 = {id, name}；b 表所需列 = {id}
        let a = build_scan_with_columns("a", Some("a"), &["id", "name", "age"]);
        let b = build_scan_with_columns("b", Some("b"), &["id", "value", "ts"]);
        let join = build_join(a, b, equi_cond("a", "id", "b", "id"));
        let proj = build_projection(vec![col("a", "name")], vec!["name".to_string()], join);

        let result = ProjectionPruning::analyze(&proj);
        let expected_a: Vec<String> = vec!["id".to_string(), "name".to_string()];
        let expected_b: Vec<String> = vec!["id".to_string()];
        assert_eq!(result.get("a").cloned(), Some(expected_a));
        assert_eq!(result.get("b").cloned(), Some(expected_b));
    }

    #[test]
    fn test_analyze_unqualified_no_collection() {
        // SELECT c0 FROM a (无表前缀)
        // 预期：不收集（单层标识符无法判断归属）
        let scan = build_scan_with_columns("a", None, &["c0", "c1"]);
        let proj = build_projection(
            vec![AExpr::Identifier(vec!["c0".into()])],
            vec!["c0".to_string()],
            scan,
        );

        let result = ProjectionPruning::analyze(&proj);
        assert!(result.is_empty());
    }

    #[test]
    fn test_analyze_filter_predicate() {
        // SELECT a.c0 FROM a WHERE a.c1 > 10
        // 预期：a 表所需列 = {c0, c1}
        let scan = build_scan_with_columns("a", Some("a"), &["c0", "c1", "c2"]);
        let filter = LogicalPlan::Filter {
            predicate: AExpr::BinaryOp {
                left: Box::new(col("a", "c1")),
                op: BinaryOp::Gt,
                right: Box::new(AExpr::Literal(szrsql_types::value::Value::Int64(10))),
            },
            input: Box::new(scan),
        };
        let proj = build_projection(vec![col("a", "c0")], vec!["c0".to_string()], filter);

        let result = ProjectionPruning::analyze(&proj);
        let expected: Vec<String> = vec!["c0".to_string(), "c1".to_string()];
        assert_eq!(result.get("a").cloned(), Some(expected));
    }

    #[test]
    fn test_analyze_sort_key() {
        // SELECT a.c0 FROM a ORDER BY a.c2
        let scan = build_scan_with_columns("a", Some("a"), &["c0", "c1", "c2"]);
        let sort = LogicalPlan::Sort {
            order_by: vec![OrderByExpr {
                expr: col("a", "c2"),
                asc: true,
                nulls_first: false,
            }],
            input: Box::new(scan),
        };
        let proj = build_projection(vec![col("a", "c0")], vec!["c0".to_string()], sort);

        let result = ProjectionPruning::analyze(&proj);
        let expected: Vec<String> = vec!["c0".to_string(), "c2".to_string()];
        assert_eq!(result.get("a").cloned(), Some(expected));
    }

    #[test]
    fn test_analyze_aggregate_group_by() {
        // SELECT a.g, COUNT(*) FROM a GROUP BY a.g
        let scan = build_scan_with_columns("a", Some("a"), &["g", "v1", "v2"]);
        let agg = LogicalPlan::Aggregate {
            grouping_sets: vec![vec![col("a", "g")]],
            aggregates: vec![szrsql_sql::plan::AggregateExpr {
                func_name: "count".to_string(),
                distinct: false,
                args: vec![],
                alias: Some("count".to_string()),
            }],
            having: None,
            input: Box::new(scan),
        };

        let result = ProjectionPruning::analyze(&agg);
        let expected: Vec<String> = vec!["g".to_string()];
        assert_eq!(result.get("a").cloned(), Some(expected));
    }

    #[test]
    fn test_analyze_aggregate_having() {
        // SELECT a.g, COUNT(*) FROM a GROUP BY a.g HAVING a.g > 5
        let scan = build_scan_with_columns("a", Some("a"), &["g", "v1"]);
        let agg = LogicalPlan::Aggregate {
            grouping_sets: vec![vec![col("a", "g")]],
            aggregates: vec![szrsql_sql::plan::AggregateExpr {
                func_name: "count".to_string(),
                distinct: false,
                args: vec![],
                alias: Some("count".to_string()),
            }],
            having: Some(AExpr::BinaryOp {
                left: Box::new(col("a", "g")),
                op: BinaryOp::Gt,
                right: Box::new(AExpr::Literal(szrsql_types::value::Value::Int64(5))),
            }),
            input: Box::new(scan),
        };

        let result = ProjectionPruning::analyze(&agg);
        let expected: Vec<String> = vec!["g".to_string()];
        assert_eq!(result.get("a").cloned(), Some(expected));
    }

    #[test]
    fn test_apply_prunes_single_table() {
        // SELECT a.c0, a.c2 FROM a (a 有 c0/c1/c2/c3 四列)
        // 预期：裁剪后 Scan schema 只剩 c0, c2
        let scan = build_scan_with_columns("a", Some("a"), &["c0", "c1", "c2", "c3"]);
        let proj = build_projection(
            vec![col("a", "c0"), col("a", "c2")],
            vec!["c0".to_string(), "c2".to_string()],
            scan,
        );

        let optimized = ProjectionPruning::apply(proj);
        assert_eq!(scan_column_count(&optimized), Some(2));
    }

    #[test]
    fn test_apply_prunes_join_both_sides() {
        // SELECT a.name FROM a JOIN b ON a.id=b.id
        // 预期：a 裁剪为 {id, name}；b 裁剪为 {id}
        let a = build_scan_with_columns("a", Some("a"), &["id", "name", "age"]);
        let b = build_scan_with_columns("b", Some("b"), &["id", "value", "ts"]);
        let join = build_join(a, b, equi_cond("a", "id", "b", "id"));
        let proj = build_projection(vec![col("a", "name")], vec!["name".to_string()], join);

        let optimized = ProjectionPruning::apply(proj);
        // 验证顶层结构未变（仍是 Projection）
        assert!(matches!(optimized, LogicalPlan::Projection { .. }));
        // 验证整体 Scan 总列数 = 2(a) + 1(b) = 3
        // 由于 scan_column_count 只返回第一个 Scan，需手动遍历
        fn count_all_scan_cols(plan: &LogicalPlan) -> usize {
            match plan {
                LogicalPlan::Scan { schema, .. } => schema.columns.len(),
                LogicalPlan::Filter { input, .. }
                | LogicalPlan::Projection { input, .. }
                | LogicalPlan::Sort { input, .. }
                | LogicalPlan::Limit { input, .. }
                | LogicalPlan::Distinct { input, .. }
                | LogicalPlan::Aggregate { input, .. } => count_all_scan_cols(input),
                LogicalPlan::Join { left, right, .. } => {
                    count_all_scan_cols(left) + count_all_scan_cols(right)
                }
                _ => 0,
            }
        }
        assert_eq!(count_all_scan_cols(&optimized), 3);
    }

    #[test]
    fn test_apply_no_prune_when_all_columns_referenced() {
        // SELECT a.c0, a.c1 FROM a (a 有 c0/c1 两列)
        // 预期：所有列都被引用，不裁剪
        let scan = build_scan_with_columns("a", Some("a"), &["c0", "c1"]);
        let proj = build_projection(
            vec![col("a", "c0"), col("a", "c1")],
            vec!["c0".to_string(), "c1".to_string()],
            scan,
        );

        let optimized = ProjectionPruning::apply(proj);
        assert_eq!(scan_column_count(&optimized), Some(2));
    }

    #[test]
    fn test_apply_no_prune_for_unqualified_refs() {
        // SELECT c0 FROM a (无表前缀)
        // 预期：不裁剪（单层标识符无法判断归属）
        let scan = build_scan_with_columns("a", None, &["c0", "c1", "c2"]);
        let proj = build_projection(
            vec![AExpr::Identifier(vec!["c0".into()])],
            vec!["c0".to_string()],
            scan,
        );

        let optimized = ProjectionPruning::apply(proj);
        assert_eq!(scan_column_count(&optimized), Some(3));
    }

    #[test]
    fn test_apply_preserves_column_order() {
        // SELECT a.c2, a.c0 FROM a (a 有 c0/c1/c2/c3)
        // 预期：裁剪后列顺序仍为 c0, c2（按 schema 原顺序）
        let scan = build_scan_with_columns("a", Some("a"), &["c0", "c1", "c2", "c3"]);
        let proj = build_projection(
            vec![col("a", "c2"), col("a", "c0")],
            vec!["c2".to_string(), "c0".to_string()],
            scan,
        );

        let optimized = ProjectionPruning::apply(proj);
        // 验证裁剪后列顺序为 c0, c2
        fn get_scan_columns(plan: &LogicalPlan) -> Vec<String> {
            fn find_scan(plan: &LogicalPlan) -> Option<&TableSchema> {
                match plan {
                    LogicalPlan::Scan { schema, .. } => Some(schema),
                    LogicalPlan::Filter { input, .. }
                    | LogicalPlan::Projection { input, .. }
                    | LogicalPlan::Sort { input, .. }
                    | LogicalPlan::Limit { input, .. }
                    | LogicalPlan::Distinct { input, .. }
                    | LogicalPlan::Aggregate { input, .. } => find_scan(input),
                    LogicalPlan::Join { left, right, .. } => {
                        find_scan(left).or_else(|| find_scan(right))
                    }
                    _ => None,
                }
            }
            find_scan(plan)
                .map(|s| s.columns.iter().map(|c| c.name.clone()).collect())
                .unwrap_or_default()
        }
        let cols = get_scan_columns(&optimized);
        assert_eq!(cols, vec!["c0".to_string(), "c2".to_string()]);
    }

    #[test]
    fn test_apply_uses_alias_as_key() {
        // SELECT t.c0 FROM a AS t (表名 a，别名 t)
        // 预期：按别名 t 查找所需列，裁剪 a 表
        let scan = build_scan_with_columns("a", Some("t"), &["c0", "c1", "c2"]);
        let proj = build_projection(vec![col("t", "c0")], vec!["c0".to_string()], scan);

        let optimized = ProjectionPruning::apply(proj);
        assert_eq!(scan_column_count(&optimized), Some(1));
    }

    #[test]
    fn test_apply_uses_table_name_when_no_alias() {
        // SELECT a.c0 FROM a (无别名)
        // 预期：按表名 a 查找所需列，裁剪 a 表
        let scan = build_scan_with_columns("a", None, &["c0", "c1", "c2"]);
        let proj = build_projection(vec![col("a", "c0")], vec!["c0".to_string()], scan);

        let optimized = ProjectionPruning::apply(proj);
        assert_eq!(scan_column_count(&optimized), Some(1));
    }

    #[test]
    fn test_apply_nested_join() {
        // SELECT a.c0 FROM (a JOIN b ON a.id=b.id) JOIN c ON a.id=c.id
        // 预期：a 裁剪为 {c0, id}；b 裁剪为 {id}；c 裁剪为 {id}
        let a = build_scan_with_columns("a", Some("a"), &["id", "c0", "c1"]);
        let b = build_scan_with_columns("b", Some("b"), &["id", "c2", "c3"]);
        let ab_join = build_join(a, b, equi_cond("a", "id", "b", "id"));
        let c = build_scan_with_columns("c", Some("c"), &["id", "c4", "c5"]);
        let abc_join = build_join(ab_join, c, equi_cond("a", "id", "c", "id"));
        let proj = build_projection(vec![col("a", "c0")], vec!["c0".to_string()], abc_join);

        let optimized = ProjectionPruning::apply(proj);
        // 验证总 Scan 列数 = 2(a) + 1(b) + 1(c) = 4
        fn count_all_scan_cols(plan: &LogicalPlan) -> usize {
            match plan {
                LogicalPlan::Scan { schema, .. } => schema.columns.len(),
                LogicalPlan::Filter { input, .. }
                | LogicalPlan::Projection { input, .. }
                | LogicalPlan::Sort { input, .. }
                | LogicalPlan::Limit { input, .. }
                | LogicalPlan::Distinct { input, .. }
                | LogicalPlan::Aggregate { input, .. } => count_all_scan_cols(input),
                LogicalPlan::Join { left, right, .. } => {
                    count_all_scan_cols(left) + count_all_scan_cols(right)
                }
                _ => 0,
            }
        }
        assert_eq!(count_all_scan_cols(&optimized), 4);
    }

    #[test]
    fn test_apply_function_args() {
        // SELECT SUM(a.c0 + a.c1) FROM a
        // 预期：a 裁剪为 {c0, c1}
        let scan = build_scan_with_columns("a", Some("a"), &["c0", "c1", "c2"]);
        let agg = LogicalPlan::Aggregate {
            grouping_sets: vec![vec![]],
            aggregates: vec![szrsql_sql::plan::AggregateExpr {
                func_name: "sum".to_string(),
                distinct: false,
                args: vec![AExpr::BinaryOp {
                    left: Box::new(col("a", "c0")),
                    op: BinaryOp::Plus,
                    right: Box::new(col("a", "c1")),
                }],
                alias: Some("sum".to_string()),
            }],
            having: None,
            input: Box::new(scan),
        };

        let optimized = ProjectionPruning::apply(agg);
        assert_eq!(scan_column_count(&optimized), Some(2));
    }

    #[test]
    fn test_apply_case_expression() {
        // SELECT CASE WHEN a.c0 > 0 THEN a.c1 ELSE a.c2 END FROM a
        let scan = build_scan_with_columns("a", Some("a"), &["c0", "c1", "c2", "c3"]);
        let case_expr = AExpr::Case {
            operand: None,
            when_then: vec![(
                AExpr::BinaryOp {
                    left: Box::new(col("a", "c0")),
                    op: BinaryOp::Gt,
                    right: Box::new(AExpr::Literal(szrsql_types::value::Value::Int64(0))),
                },
                col("a", "c1"),
            )],
            else_expr: Some(Box::new(col("a", "c2"))),
        };
        let proj = build_projection(vec![case_expr], vec!["case".to_string()], scan);

        let optimized = ProjectionPruning::apply(proj);
        assert_eq!(scan_column_count(&optimized), Some(3));
    }

    #[test]
    fn test_apply_in_list() {
        // SELECT a.c0 FROM a WHERE a.c1 IN (1, 2, 3)
        let scan = build_scan_with_columns("a", Some("a"), &["c0", "c1", "c2"]);
        let filter = LogicalPlan::Filter {
            predicate: AExpr::InList {
                expr: Box::new(col("a", "c1")),
                list: vec![
                    AExpr::Literal(szrsql_types::value::Value::Int64(1)),
                    AExpr::Literal(szrsql_types::value::Value::Int64(2)),
                    AExpr::Literal(szrsql_types::value::Value::Int64(3)),
                ],
                negated: false,
            },
            input: Box::new(scan),
        };
        let proj = build_projection(vec![col("a", "c0")], vec!["c0".to_string()], filter);

        let optimized = ProjectionPruning::apply(proj);
        assert_eq!(scan_column_count(&optimized), Some(2));
    }
}

// =====================================================================
//  SubqueryFlattening 单元测试 — Phase 5.6
// =====================================================================

#[cfg(test)]
mod subquery_flattening_tests {
    use super::*;
    use szrsql_sql::ast::{
        ColumnDefinition, Expr as AExpr, Select, SelectItem, TableAlias, TableFactor, TableName,
        TableWithJoins,
    };
    use szrsql_sql::plan::{InMemoryCatalog, TableSchema};
    use szrsql_types::value::{ColumnType, Value};

    /// 构建测试用 catalog：表 a(id, x) 和 b(id, y)
    fn build_catalog() -> InMemoryCatalog {
        let mut catalog = InMemoryCatalog::new();
        catalog.add_simple_table(
            "a",
            vec![("id", ColumnType::Int64), ("x", ColumnType::Int64)],
        );
        catalog.add_simple_table(
            "b",
            vec![("id", ColumnType::Int64), ("y", ColumnType::Int64)],
        );
        catalog.add_simple_table(
            "c",
            vec![("id", ColumnType::Int64), ("z", ColumnType::Int64)],
        );
        catalog
    }

    /// 构建 Scan 计划
    fn build_scan(table_name: &str, cols: &[&str]) -> LogicalPlan {
        let columns: Vec<ColumnDefinition> = cols
            .iter()
            .map(|c| ColumnDefinition::new(*c, ColumnType::Int64))
            .collect();
        LogicalPlan::Scan {
            table: TableName::new(table_name),
            alias: Some(table_name.to_string()),
            schema: TableSchema {
                name: TableName::new(table_name),
                columns,
            },
        }
    }

    /// 构建简单 SELECT 子查询：SELECT col FROM table [WHERE pred]
    fn build_simple_subquery(table: &str, col: &str, where_pred: Option<AExpr>) -> Select {
        Select {
            with: None,
            distinct: false,
            projection: vec![SelectItem::UnnamedExpr(AExpr::Identifier(vec![
                table.to_string(),
                col.to_string(),
            ]))],
            from: vec![TableWithJoins {
                relation: TableFactor::Table {
                    name: TableName::new(table),
                    alias: Some(TableAlias::new(table)),
                },
                joins: vec![],
            }],
            where_clause: where_pred,
            group_by: vec![],
            having: None,
            order_by: vec![],
            limit: None,
            offset: None,
            set_op: None,
            grouping_sets: None,
        }
    }

    /// 构建 IN 子查询谓词 `outer.col IN (SELECT sub_col FROM sub_table)`
    fn build_in_subquery_pred(
        outer_table: &str,
        outer_col: &str,
        sub_table: &str,
        sub_col: &str,
        negated: bool,
    ) -> AExpr {
        AExpr::InSubquery {
            expr: Box::new(AExpr::Identifier(vec![
                outer_table.to_string(),
                outer_col.to_string(),
            ])),
            subquery: Box::new(build_simple_subquery(sub_table, sub_col, None)),
            negated,
        }
    }

    /// 构建 EXISTS 子查询谓词 `EXISTS (SELECT * FROM sub WHERE sub.col = outer.col)`
    fn build_exists_pred(
        outer_table: &str,
        outer_col: &str,
        sub_table: &str,
        sub_col: &str,
        negated: bool,
    ) -> AExpr {
        // SELECT sub_col FROM sub WHERE sub.sub_col = outer.outer_col
        let pred = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec![
                sub_table.to_string(),
                sub_col.to_string(),
            ])),
            op: BinaryOp::Eq,
            right: Box::new(AExpr::Identifier(vec![
                outer_table.to_string(),
                outer_col.to_string(),
            ])),
        };
        AExpr::Exists {
            subquery: Box::new(build_simple_subquery(sub_table, sub_col, Some(pred))),
            negated,
        }
    }

    /// 获取计划中的 SemiJoin/AntiJoin 数量
    fn count_semijoin_anti(plan: &LogicalPlan) -> (usize, usize) {
        let mut semi = 0;
        let mut anti = 0;
        count_semijoin_anti_recursive(plan, &mut semi, &mut anti);
        (semi, anti)
    }

    fn count_semijoin_anti_recursive(plan: &LogicalPlan, semi: &mut usize, anti: &mut usize) {
        match plan {
            LogicalPlan::Join {
                join_type,
                left,
                right,
                ..
            } => {
                match join_type {
                    JoinType::Semi => *semi += 1,
                    JoinType::Anti => *anti += 1,
                    _ => {}
                }
                count_semijoin_anti_recursive(left, semi, anti);
                count_semijoin_anti_recursive(right, semi, anti);
            }
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Projection { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Distinct { input, .. }
            | LogicalPlan::Aggregate { input, .. } => {
                count_semijoin_anti_recursive(input, semi, anti);
            }
            _ => {}
        }
    }

    /// 统计计划中 InSubquery/Exists 表达式数量（在 Filter 谓词中）
    fn count_subquery_exprs(plan: &LogicalPlan) -> usize {
        let mut count = 0;
        count_subquery_exprs_recursive(plan, &mut count);
        count
    }

    fn count_subquery_exprs_recursive(plan: &LogicalPlan, count: &mut usize) {
        match plan {
            LogicalPlan::Filter { predicate, input } => {
                count_subquery_in_expr(predicate, count);
                count_subquery_exprs_recursive(input, count);
            }
            LogicalPlan::Projection { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Distinct { input, .. }
            | LogicalPlan::Aggregate { input, .. } => {
                count_subquery_exprs_recursive(input, count);
            }
            LogicalPlan::Join { left, right, .. } => {
                count_subquery_exprs_recursive(left, count);
                count_subquery_exprs_recursive(right, count);
            }
            _ => {}
        }
    }

    fn count_subquery_in_expr(expr: &AExpr, count: &mut usize) {
        match expr {
            AExpr::InSubquery { .. } | AExpr::Exists { .. } | AExpr::Subquery(_) => *count += 1,
            AExpr::BinaryOp { left, right, .. } => {
                count_subquery_in_expr(left, count);
                count_subquery_in_expr(right, count);
            }
            AExpr::UnaryOp { expr, .. } => count_subquery_in_expr(expr, count),
            _ => {}
        }
    }

    // -----------------------------------------------------------------
    //  IN 子查询展平测试
    // -----------------------------------------------------------------

    #[test]
    fn test_flatten_in_subquery_to_semi_join() {
        // SELECT * FROM a WHERE a.id IN (SELECT id FROM b)
        // 预期：展平为 SemiJoin(Scan(a), Scan(b), ON a.id = b.id)
        let catalog = build_catalog();
        let planner = Planner::new(&catalog);
        let flattener = SubqueryFlattening::new(&planner);

        let scan_a = build_scan("a", &["id", "x"]);
        let filter = LogicalPlan::Filter {
            predicate: build_in_subquery_pred("a", "id", "b", "id", false),
            input: Box::new(scan_a),
        };

        let optimized = flattener.apply(filter);

        // 顶层应为 SemiJoin
        if let LogicalPlan::Join { join_type, .. } = &optimized {
            assert_eq!(*join_type, JoinType::Semi);
        } else {
            panic!("expected SemiJoin, got {:?}", optimized);
        }
        // 无残留子查询表达式
        assert_eq!(count_subquery_exprs(&optimized), 0);
        let (semi, anti) = count_semijoin_anti(&optimized);
        assert_eq!(semi, 1);
        assert_eq!(anti, 0);
    }

    #[test]
    fn test_flatten_not_in_subquery_to_anti_join() {
        // SELECT * FROM a WHERE a.id NOT IN (SELECT id FROM b)
        // 预期：展平为 AntiJoin
        let catalog = build_catalog();
        let planner = Planner::new(&catalog);
        let flattener = SubqueryFlattening::new(&planner);

        let scan_a = build_scan("a", &["id", "x"]);
        let filter = LogicalPlan::Filter {
            predicate: build_in_subquery_pred("a", "id", "b", "id", true),
            input: Box::new(scan_a),
        };

        let optimized = flattener.apply(filter);

        if let LogicalPlan::Join { join_type, .. } = &optimized {
            assert_eq!(*join_type, JoinType::Anti);
        } else {
            panic!("expected AntiJoin, got {:?}", optimized);
        }
        assert_eq!(count_subquery_exprs(&optimized), 0);
        let (semi, anti) = count_semijoin_anti(&optimized);
        assert_eq!(semi, 0);
        assert_eq!(anti, 1);
    }

    #[test]
    fn test_flatten_in_subquery_with_extra_predicate() {
        // SELECT * FROM a WHERE a.x > 10 AND a.id IN (SELECT id FROM b)
        // 预期：保留 a.x > 10 的 Filter，子查询展平为 SemiJoin
        let catalog = build_catalog();
        let planner = Planner::new(&catalog);
        let flattener = SubqueryFlattening::new(&planner);

        let scan_a = build_scan("a", &["id", "x"]);
        let extra_pred = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["a".to_string(), "x".to_string()])),
            op: BinaryOp::Gt,
            right: Box::new(AExpr::Literal(Value::Int64(10))),
        };
        let combined = AExpr::BinaryOp {
            left: Box::new(extra_pred),
            op: BinaryOp::And,
            right: Box::new(build_in_subquery_pred("a", "id", "b", "id", false)),
        };
        let filter = LogicalPlan::Filter {
            predicate: combined,
            input: Box::new(scan_a),
        };

        let optimized = flattener.apply(filter);

        // 顶层应为 Filter（保留 a.x > 10），下方为 SemiJoin
        if let LogicalPlan::Filter { input, .. } = &optimized {
            if let LogicalPlan::Join { join_type, .. } = input.as_ref() {
                assert_eq!(*join_type, JoinType::Semi);
            } else {
                panic!("expected SemiJoin below Filter");
            }
        } else {
            panic!("expected Filter at top");
        }
        assert_eq!(count_subquery_exprs(&optimized), 0);
    }

    // -----------------------------------------------------------------
    //  EXISTS 子查询展平测试
    // -----------------------------------------------------------------

    #[test]
    fn test_flatten_exists_to_semi_join() {
        // SELECT * FROM a WHERE EXISTS (SELECT id FROM b WHERE b.id = a.id)
        // 预期：展平为 SemiJoin(Scan(a), Scan(b), ON b.id = a.id)
        let catalog = build_catalog();
        let planner = Planner::new(&catalog);
        let flattener = SubqueryFlattening::new(&planner);

        let scan_a = build_scan("a", &["id", "x"]);
        let filter = LogicalPlan::Filter {
            predicate: build_exists_pred("a", "id", "b", "id", false),
            input: Box::new(scan_a),
        };

        let optimized = flattener.apply(filter);

        if let LogicalPlan::Join { join_type, .. } = &optimized {
            assert_eq!(*join_type, JoinType::Semi);
        } else {
            panic!("expected SemiJoin, got {:?}", optimized);
        }
        assert_eq!(count_subquery_exprs(&optimized), 0);
        let (semi, anti) = count_semijoin_anti(&optimized);
        assert_eq!(semi, 1);
        assert_eq!(anti, 0);
    }

    #[test]
    fn test_flatten_not_exists_to_anti_join() {
        // SELECT * FROM a WHERE NOT EXISTS (SELECT id FROM b WHERE b.id = a.id)
        // 预期：展平为 AntiJoin
        let catalog = build_catalog();
        let planner = Planner::new(&catalog);
        let flattener = SubqueryFlattening::new(&planner);

        let scan_a = build_scan("a", &["id", "x"]);
        let filter = LogicalPlan::Filter {
            predicate: build_exists_pred("a", "id", "b", "id", true),
            input: Box::new(scan_a),
        };

        let optimized = flattener.apply(filter);

        if let LogicalPlan::Join { join_type, .. } = &optimized {
            assert_eq!(*join_type, JoinType::Anti);
        } else {
            panic!("expected AntiJoin, got {:?}", optimized);
        }
        assert_eq!(count_subquery_exprs(&optimized), 0);
        let (semi, anti) = count_semijoin_anti(&optimized);
        assert_eq!(semi, 0);
        assert_eq!(anti, 1);
    }

    #[test]
    fn test_flatten_exists_with_remaining_predicate() {
        // SELECT * FROM a WHERE EXISTS (SELECT id FROM b WHERE b.id = a.id AND b.y > 5)
        // 预期：SemiJoin(Scan(a), Filter(b.y > 5, Scan(b)), ON b.id = a.id)
        let catalog = build_catalog();
        let planner = Planner::new(&catalog);
        let flattener = SubqueryFlattening::new(&planner);

        // 子查询 WHERE: b.id = a.id AND b.y > 5
        let correlation = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["b".to_string(), "id".to_string()])),
            op: BinaryOp::Eq,
            right: Box::new(AExpr::Identifier(vec!["a".to_string(), "id".to_string()])),
        };
        let inner_pred = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["b".to_string(), "y".to_string()])),
            op: BinaryOp::Gt,
            right: Box::new(AExpr::Literal(Value::Int64(5))),
        };
        let combined_where = AExpr::BinaryOp {
            left: Box::new(correlation),
            op: BinaryOp::And,
            right: Box::new(inner_pred),
        };
        let subquery = Select {
            with: None,
            distinct: false,
            projection: vec![SelectItem::UnnamedExpr(AExpr::Identifier(vec![
                "b".to_string(),
                "id".to_string(),
            ]))],
            from: vec![TableWithJoins {
                relation: TableFactor::Table {
                    name: TableName::new("b"),
                    alias: Some(TableAlias::new("b")),
                },
                joins: vec![],
            }],
            where_clause: Some(combined_where),
            group_by: vec![],
            having: None,
            order_by: vec![],
            limit: None,
            offset: None,
            set_op: None,
            grouping_sets: None,
        };
        let exists_pred = AExpr::Exists {
            subquery: Box::new(subquery),
            negated: false,
        };

        let scan_a = build_scan("a", &["id", "x"]);
        let filter = LogicalPlan::Filter {
            predicate: exists_pred,
            input: Box::new(scan_a),
        };

        let optimized = flattener.apply(filter);

        // 顶层应为 SemiJoin
        if let LogicalPlan::Join {
            join_type, right, ..
        } = &optimized
        {
            assert_eq!(*join_type, JoinType::Semi);
            // 右子树应含 Filter（b.y > 5）：可能是 Projection 包 Filter 或直接 Filter
            let right_has_filter = match right.as_ref() {
                LogicalPlan::Filter { .. } => true,
                LogicalPlan::Projection { input, .. } => {
                    matches!(input.as_ref(), LogicalPlan::Filter { .. })
                }
                _ => false,
            };
            assert!(right_has_filter, "expected Filter in right subtree");
        } else {
            panic!("expected SemiJoin, got {:?}", optimized);
        }
        assert_eq!(count_subquery_exprs(&optimized), 0);
    }

    // -----------------------------------------------------------------
    //  不展平场景测试
    // -----------------------------------------------------------------

    #[test]
    fn test_no_flatten_uncorrelated_exists() {
        // SELECT * FROM a WHERE EXISTS (SELECT id FROM b WHERE b.y > 5)
        // 不相关 EXISTS（无外层表引用）→ 不展平
        let catalog = build_catalog();
        let planner = Planner::new(&catalog);
        let flattener = SubqueryFlattening::new(&planner);

        let inner_pred = AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["b".to_string(), "y".to_string()])),
            op: BinaryOp::Gt,
            right: Box::new(AExpr::Literal(Value::Int64(5))),
        };
        let subquery = build_simple_subquery("b", "id", Some(inner_pred));
        let exists_pred = AExpr::Exists {
            subquery: Box::new(subquery),
            negated: false,
        };

        let scan_a = build_scan("a", &["id", "x"]);
        let filter = LogicalPlan::Filter {
            predicate: exists_pred,
            input: Box::new(scan_a),
        };

        let optimized = flattener.apply(filter);

        // 应保持 Filter 不变
        assert!(matches!(optimized, LogicalPlan::Filter { .. }));
        assert_eq!(count_subquery_exprs(&optimized), 1);
        let (semi, anti) = count_semijoin_anti(&optimized);
        assert_eq!(semi, 0);
        assert_eq!(anti, 0);
    }

    #[test]
    fn test_no_flatten_non_simple_subquery() {
        // SELECT * FROM a WHERE a.id IN (SELECT id FROM b LIMIT 10)
        // 子查询含 LIMIT → 不展平
        let catalog = build_catalog();
        let planner = Planner::new(&catalog);
        let flattener = SubqueryFlattening::new(&planner);

        let mut subquery = build_simple_subquery("b", "id", None);
        subquery.limit = Some(AExpr::Literal(Value::Int64(10)));
        let in_pred = AExpr::InSubquery {
            expr: Box::new(AExpr::Identifier(vec!["a".to_string(), "id".to_string()])),
            subquery: Box::new(subquery),
            negated: false,
        };

        let scan_a = build_scan("a", &["id", "x"]);
        let filter = LogicalPlan::Filter {
            predicate: in_pred,
            input: Box::new(scan_a),
        };

        let optimized = flattener.apply(filter);

        // 应保持 Filter 不变
        assert!(matches!(optimized, LogicalPlan::Filter { .. }));
        assert_eq!(count_subquery_exprs(&optimized), 1);
    }

    // -----------------------------------------------------------------
    //  递归与嵌套测试
    // -----------------------------------------------------------------

    #[test]
    fn test_flatten_in_subquery_inside_projection() {
        // SELECT * FROM (SELECT * FROM a WHERE a.id IN (SELECT id FROM b))
        // 外层 Projection 包裹含子查询的 Filter → 递归展平
        let catalog = build_catalog();
        let planner = Planner::new(&catalog);
        let flattener = SubqueryFlattening::new(&planner);

        let scan_a = build_scan("a", &["id", "x"]);
        let filter = LogicalPlan::Filter {
            predicate: build_in_subquery_pred("a", "id", "b", "id", false),
            input: Box::new(scan_a),
        };
        let proj = LogicalPlan::Projection {
            exprs: vec![
                (
                    AExpr::Identifier(vec!["a".to_string(), "id".to_string()]),
                    Some("id".to_string()),
                ),
                (
                    AExpr::Identifier(vec!["a".to_string(), "x".to_string()]),
                    Some("x".to_string()),
                ),
            ],
            output_names: vec!["id".to_string(), "x".to_string()],
            input: Box::new(filter),
        };

        let optimized = flattener.apply(proj);

        // 顶层 Projection，下方 SemiJoin
        if let LogicalPlan::Projection { input, .. } = &optimized {
            if let LogicalPlan::Join { join_type, .. } = input.as_ref() {
                assert_eq!(*join_type, JoinType::Semi);
            } else {
                panic!("expected SemiJoin below Projection");
            }
        } else {
            panic!("expected Projection at top");
        }
        assert_eq!(count_subquery_exprs(&optimized), 0);
    }

    #[test]
    fn test_flatten_multiple_in_subqueries() {
        // SELECT * FROM a WHERE a.id IN (SELECT id FROM b) AND a.x IN (SELECT id FROM c)
        // 两个 IN 子查询 → 两个 SemiJoin 嵌套
        let catalog = build_catalog();
        let planner = Planner::new(&catalog);
        let flattener = SubqueryFlattening::new(&planner);

        let scan_a = build_scan("a", &["id", "x"]);
        let combined = AExpr::BinaryOp {
            left: Box::new(build_in_subquery_pred("a", "id", "b", "id", false)),
            op: BinaryOp::And,
            right: Box::new(build_in_subquery_pred("a", "x", "c", "id", false)),
        };
        let filter = LogicalPlan::Filter {
            predicate: combined,
            input: Box::new(scan_a),
        };

        let optimized = flattener.apply(filter);

        // 应有 2 个 SemiJoin
        let (semi, anti) = count_semijoin_anti(&optimized);
        assert_eq!(semi, 2);
        assert_eq!(anti, 0);
        assert_eq!(count_subquery_exprs(&optimized), 0);
    }

    // -----------------------------------------------------------------
    //  辅助函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_extract_simple_column_from_select_item() {
        // UnnamedExpr(Identifier(["col"]))
        let item = SelectItem::UnnamedExpr(AExpr::Identifier(vec!["col".to_string()]));
        let result = extract_simple_column_from_select_item(&item);
        assert!(result.is_some());

        // UnnamedExpr(Identifier(["table", "col"]))
        let item = SelectItem::UnnamedExpr(AExpr::Identifier(vec![
            "table".to_string(),
            "col".to_string(),
        ]));
        let result = extract_simple_column_from_select_item(&item);
        assert!(result.is_some());

        // ExprWithAlias
        let item = SelectItem::ExprWithAlias {
            expr: AExpr::Identifier(vec!["col".to_string()]),
            alias: "c".to_string(),
        };
        let result = extract_simple_column_from_select_item(&item);
        assert!(result.is_some());

        // Wildcard → None
        let result = extract_simple_column_from_select_item(&SelectItem::Wildcard);
        assert!(result.is_none());

        // 表达式 → None
        let item = SelectItem::UnnamedExpr(AExpr::BinaryOp {
            left: Box::new(AExpr::Identifier(vec!["a".to_string()])),
            op: BinaryOp::Plus,
            right: Box::new(AExpr::Identifier(vec!["b".to_string()])),
        });
        let result = extract_simple_column_from_select_item(&item);
        assert!(result.is_none());
    }

    #[test]
    fn test_is_simple_subquery() {
        // 简单 SELECT
        let simple = build_simple_subquery("b", "id", None);
        assert!(SubqueryFlattening::is_simple_subquery(&simple));

        // 含 DISTINCT
        let mut with_distinct = build_simple_subquery("b", "id", None);
        with_distinct.distinct = true;
        assert!(!SubqueryFlattening::is_simple_subquery(&with_distinct));

        // 含 LIMIT
        let mut with_limit = build_simple_subquery("b", "id", None);
        with_limit.limit = Some(AExpr::Literal(Value::Int64(10)));
        assert!(!SubqueryFlattening::is_simple_subquery(&with_limit));

        // 含 GROUP BY
        let mut with_group = build_simple_subquery("b", "id", None);
        with_group.group_by = vec![AExpr::Identifier(vec!["id".to_string()])];
        assert!(!SubqueryFlattening::is_simple_subquery(&with_group));
    }
}

// =====================================================================
//  IndexSelection — Phase 5.7 索引选择
// =====================================================================

/// 索引选择规则 — Phase 5.7
///
/// 在 `Filter { predicate, input: Scan }` 模式下，根据 Catalog 中的索引定义，
/// 选择合适的索引将 SeqScan 替换为 IndexScan。
///
/// # 选择策略
///
/// 1. 提取谓词中所有 `col OP literal` 形式的合取项
/// 2. 查询 Catalog 获取表的所有索引
/// 3. 对每个索引按"最左前缀匹配"计算匹配列数：
///    - 索引首列必须出现在谓词的等值条件中（否则该索引不可用）
///    - 后续列按顺序匹配等值条件，每个匹配列 +1
/// 4. 选择匹配列数最多的索引；若多个索引匹配列数相同，选 UNIQUE 优先
/// 5. 若最佳匹配列数 = 0（无任何索引首列在等值条件中），保持 SeqScan
///
/// # 限制
///
/// - 仅识别 `col = literal` 形式作为等值条件；表达式（如 `f(x) = 1`）不识别
/// - 复合索引要求严格最左前缀匹配
/// - 仅识别 i64 字面量（受 `InMemoryBTreeIndex` 限制）
/// - 不考虑索引选择性差异（如不同列的 NDV），仅按匹配列数决策
pub struct IndexSelection<'a, 'c> {
    /// Catalog 引用（用于查询表的索引列表）
    catalog: &'a dyn szrsql_sql::plan::Catalog,
    /// 隐藏 Planner 引用（保持与 SubqueryFlattening 一致的生命周期风格）
    _planner: std::marker::PhantomData<&'c Planner<'c>>,
}

impl<'a, 'c> IndexSelection<'a, 'c> {
    /// 创建索引选择规则应用器
    pub fn new(catalog: &'a dyn szrsql_sql::plan::Catalog) -> Self {
        Self {
            catalog,
            _planner: std::marker::PhantomData,
        }
    }

    /// 应用索引选择规则
    pub fn apply(&self, plan: LogicalPlan) -> LogicalPlan {
        self.apply_recursive(plan)
    }

    fn apply_recursive(&self, plan: LogicalPlan) -> LogicalPlan {
        match plan {
            LogicalPlan::Filter { predicate, input } => {
                let input = self.apply_recursive(*input);
                self.try_replace_with_index_scan(predicate, input)
            }
            LogicalPlan::Projection {
                exprs,
                output_names,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Projection {
                    exprs,
                    output_names,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
            } => {
                let left = self.apply_recursive(*left);
                let right = self.apply_recursive(*right);
                LogicalPlan::Join {
                    join_type,
                    condition,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            }
            LogicalPlan::Aggregate {
                grouping_sets,
                aggregates,
                having,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Aggregate {
                    grouping_sets,
                    aggregates,
                    having,
                    input: Box::new(input),
                }
            }
            // Phase 6.2: 窗口函数节点 — 递归处理 input
            LogicalPlan::Window {
                window_funcs,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Window {
                    window_funcs,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Sort { order_by, input } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Sort {
                    order_by,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Limit {
                limit,
                offset,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Limit {
                    limit,
                    offset,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Distinct { input } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Distinct {
                    input: Box::new(input),
                }
            }
            // Scan / IndexScan / DML / DDL 不变
            other => other,
        }
    }

    /// 尝试用 IndexScan 替换 `Filter { input: Scan }` 模式
    ///
    /// 若 Scan 表上有合适索引，且谓词中包含索引列的访问条件，则产生 IndexScan。
    /// IndexScan 节点的 `predicate` 包含完整 Filter 谓词（执行器先按索引访问路径查找，
    /// 再对结果应用完整谓词作残余过滤）。
    fn try_replace_with_index_scan(&self, predicate: Expr, input: LogicalPlan) -> LogicalPlan {
        let (table, alias, schema) = match &input {
            LogicalPlan::Scan {
                table,
                alias,
                schema,
            } => (table.clone(), alias.clone(), schema.clone()),
            _ => {
                // input 不是 Scan → 保持 Filter
                return LogicalPlan::Filter {
                    predicate,
                    input: Box::new(input),
                };
            }
        };

        let indexes = self.catalog.list_indexes(&table);
        if indexes.is_empty() {
            return LogicalPlan::Filter {
                predicate,
                input: Box::new(input),
            };
        }

        let eq_cols = collect_eq_columns(&predicate);
        if eq_cols.is_empty() {
            return LogicalPlan::Filter {
                predicate,
                input: Box::new(input),
            };
        }

        let best = choose_best_index(&indexes, &eq_cols);
        match best {
            Some(idx) => {
                let index_columns: Vec<String> =
                    idx.column_names().into_iter().map(String::from).collect();
                LogicalPlan::IndexScan {
                    table,
                    alias,
                    schema,
                    index_name: idx.name.clone(),
                    index_columns,
                    predicate,
                }
            }
            None => LogicalPlan::Filter {
                predicate,
                input: Box::new(input),
            },
        }
    }
}

/// 收集谓词中所有等值条件涉及的列名（大小写不敏感）
///
/// 形式：`col = literal` 或 `literal = col`，其中 literal 为 i64 字面量。
/// AND 连接的多个等值条件全部收集。
fn collect_eq_columns(predicate: &Expr) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut conjuncts = Vec::new();
    collect_and_conjuncts_ref(predicate, &mut conjuncts);
    for conjunct in conjuncts {
        if let Expr::BinaryOp {
            left,
            op: BinaryOp::Eq,
            right,
        } = conjunct
        {
            if let Expr::Identifier(parts) = left.as_ref() {
                if let Some(last) = parts.last() {
                    if is_i64_literal(right.as_ref()) {
                        out.insert(last.to_lowercase());
                    }
                }
            } else if let Expr::Identifier(parts) = right.as_ref() {
                if let Some(last) = parts.last() {
                    if is_i64_literal(left.as_ref()) {
                        out.insert(last.to_lowercase());
                    }
                }
            }
        }
    }
    out
}

fn collect_and_conjuncts_ref<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::BinaryOp {
        left,
        op: BinaryOp::And,
        right,
    } = expr
    {
        collect_and_conjuncts_ref(left.as_ref(), out);
        collect_and_conjuncts_ref(right.as_ref(), out);
    } else {
        out.push(expr);
    }
}

fn is_i64_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Literal(Value::Int64(_)))
}

/// 选择最佳索引：最左前缀匹配列数最多者；列数相同选 UNIQUE
///
/// 最左前缀规则：
/// - 索引首列必须在等值条件集合中（否则该索引匹配列数 = 0）
/// - 后续列按声明顺序逐个检查是否在等值条件集合中，遇第一个不匹配则停止
fn choose_best_index<'a>(
    indexes: &'a [IndexDefinition],
    eq_cols: &HashSet<String>,
) -> Option<&'a IndexDefinition> {
    let mut best: Option<&IndexDefinition> = None;
    let mut best_match_count = 0;
    for idx in indexes {
        let mut count = 0;
        for col in &idx.columns {
            if eq_cols.contains(&col.column.to_lowercase()) {
                count += 1;
            } else {
                break; // 最左前缀：遇不匹配则停止
            }
        }
        if count == 0 {
            continue;
        }
        let is_better = match best {
            None => true,
            Some(b) => {
                count > best_match_count || (count == best_match_count && idx.unique && !b.unique)
            }
        };
        if is_better {
            best = Some(idx);
            best_match_count = count;
        }
    }
    best
}

// =====================================================================
//  IndexSelection 单元测试 — Phase 5.7
// =====================================================================

#[cfg(test)]
mod index_selection_tests {
    use super::*;
    use szrsql_sql::ast::{ColumnDefinition, Expr as AExpr, IndexColumn, TableName};
    use szrsql_sql::plan::{Catalog, InMemoryCatalog, TableSchema};
    use szrsql_types::value::ColumnType;

    /// 构建测试用 catalog：表 t(id, age, name) + 多种索引
    fn build_catalog_with_indexes() -> InMemoryCatalog {
        let mut catalog = InMemoryCatalog::new();
        let schema = TableSchema {
            name: TableName::new("t"),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("age", ColumnType::Int64),
                ColumnDefinition::new("name", ColumnType::Text),
            ],
        };
        catalog.add_table(schema);

        // idx_id：单列 id 索引
        catalog.add_index(IndexDefinition::new(
            "idx_id",
            TableName::new("t"),
            vec![IndexColumn::new("id")],
        ));
        // idx_age：单列 age 索引
        catalog.add_index(IndexDefinition::new(
            "idx_age",
            TableName::new("t"),
            vec![IndexColumn::new("age")],
        ));
        // idx_id_age：复合索引 (id, age)
        catalog.add_index(IndexDefinition::new(
            "idx_id_age",
            TableName::new("t"),
            vec![IndexColumn::new("id"), IndexColumn::new("age")],
        ));
        // idx_unique_id：UNIQUE 索引 (id)
        catalog.add_index(IndexDefinition::new_unique(
            "idx_unique_id",
            TableName::new("t"),
            vec![IndexColumn::new("id")],
        ));
        catalog
    }

    fn build_scan_t() -> LogicalPlan {
        LogicalPlan::Scan {
            table: TableName::new("t"),
            alias: None,
            schema: TableSchema {
                name: TableName::new("t"),
                columns: vec![
                    ColumnDefinition::new("id", ColumnType::Int64),
                    ColumnDefinition::new("age", ColumnType::Int64),
                    ColumnDefinition::new("name", ColumnType::Text),
                ],
            },
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

    fn and(left: AExpr, right: AExpr) -> AExpr {
        AExpr::BinaryOp {
            left: Box::new(left),
            op: BinaryOp::And,
            right: Box::new(right),
        }
    }

    #[test]
    fn test_select_index_for_eq_predicate_on_indexed_column() {
        // WHERE id = 5 → 选择 idx_id 或 idx_unique_id（UNIQUE 优先）
        let catalog = build_catalog_with_indexes();
        let selector = IndexSelection::new(&catalog);
        let scan = build_scan_t();
        let filter = LogicalPlan::Filter {
            predicate: make_eq("id", 5),
            input: Box::new(scan),
        };
        let result = selector.apply(filter);
        match result {
            LogicalPlan::IndexScan {
                index_name,
                index_columns,
                ..
            } => {
                // UNIQUE 优先于普通 idx_id
                assert_eq!(index_name, "idx_unique_id");
                assert_eq!(index_columns, vec!["id".to_string()]);
            }
            _ => panic!("expected IndexScan, got {:?}", result),
        }
    }

    #[test]
    fn test_select_composite_index_when_multiple_eq() {
        // WHERE id = 5 AND age = 18 → 选择 idx_id_age（匹配 2 列）
        let catalog = build_catalog_with_indexes();
        let selector = IndexSelection::new(&catalog);
        let scan = build_scan_t();
        let pred = and(make_eq("id", 5), make_eq("age", 18));
        let filter = LogicalPlan::Filter {
            predicate: pred,
            input: Box::new(scan),
        };
        let result = selector.apply(filter);
        match result {
            LogicalPlan::IndexScan {
                index_name,
                index_columns,
                ..
            } => {
                assert_eq!(index_name, "idx_id_age");
                assert_eq!(index_columns, vec!["id".to_string(), "age".to_string()]);
            }
            _ => panic!("expected IndexScan, got {:?}", result),
        }
    }

    #[test]
    fn test_no_index_when_predicate_has_no_eq() {
        // WHERE id > 5（仅范围条件）→ 不选择索引，保持 Filter(Scan)
        // 因为索引选择要求索引首列必须出现在等值条件中
        let catalog = build_catalog_with_indexes();
        let selector = IndexSelection::new(&catalog);
        let scan = build_scan_t();
        let filter = LogicalPlan::Filter {
            predicate: make_gt("id", 5),
            input: Box::new(scan),
        };
        let result = selector.apply(filter);
        assert!(
            matches!(result, LogicalPlan::Filter { .. }),
            "expected Filter when no eq predicate, got {:?}",
            result
        );
    }

    #[test]
    fn test_no_index_when_no_indexes_registered() {
        // 表 t 上无索引 → 保持 Filter(Scan)
        let mut catalog = InMemoryCatalog::new();
        catalog.add_table(TableSchema {
            name: TableName::new("t"),
            columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
        });
        let selector = IndexSelection::new(&catalog);
        let scan = LogicalPlan::Scan {
            table: TableName::new("t"),
            alias: None,
            schema: TableSchema {
                name: TableName::new("t"),
                columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
            },
        };
        let filter = LogicalPlan::Filter {
            predicate: make_eq("id", 5),
            input: Box::new(scan),
        };
        let result = selector.apply(filter);
        assert!(matches!(result, LogicalPlan::Filter { .. }));
    }

    #[test]
    fn test_recursive_apply_through_projection() {
        // SELECT * FROM (SELECT * FROM t WHERE id = 5) → Projection 包裹 IndexScan
        let catalog = build_catalog_with_indexes();
        let selector = IndexSelection::new(&catalog);
        let scan = build_scan_t();
        let filter = LogicalPlan::Filter {
            predicate: make_eq("id", 5),
            input: Box::new(scan),
        };
        let proj = LogicalPlan::Projection {
            exprs: vec![(
                AExpr::Identifier(vec!["id".to_string()]),
                Some("id".to_string()),
            )],
            output_names: vec!["id".to_string()],
            input: Box::new(filter),
        };
        let result = selector.apply(proj);
        match result {
            LogicalPlan::Projection { input, .. } => {
                assert!(matches!(input.as_ref(), LogicalPlan::IndexScan { .. }));
            }
            _ => panic!("expected Projection, got {:?}", result),
        }
    }

    #[test]
    fn test_filter_not_on_scan_kept() {
        // Filter 包裹 Projection（非 Scan）→ 保持 Filter 不变
        let catalog = build_catalog_with_indexes();
        let selector = IndexSelection::new(&catalog);
        let scan = build_scan_t();
        let inner_proj = LogicalPlan::Projection {
            exprs: vec![(
                AExpr::Identifier(vec!["id".to_string()]),
                Some("id".to_string()),
            )],
            output_names: vec!["id".to_string()],
            input: Box::new(scan),
        };
        let filter = LogicalPlan::Filter {
            predicate: make_eq("id", 5),
            input: Box::new(inner_proj),
        };
        let result = selector.apply(filter);
        assert!(matches!(result, LogicalPlan::Filter { .. }));
    }

    #[test]
    fn test_collect_eq_columns_basic() {
        let pred = and(make_eq("id", 5), make_eq("age", 18));
        let cols = collect_eq_columns(&pred);
        assert!(cols.contains("id"));
        assert!(cols.contains("age"));
        assert_eq!(cols.len(), 2);
    }

    #[test]
    fn test_collect_eq_columns_with_non_eq_conjuncts() {
        // id = 5 AND age > 18 → 仅 id 入集合
        let pred = and(make_eq("id", 5), make_gt("age", 18));
        let cols = collect_eq_columns(&pred);
        assert!(cols.contains("id"));
        assert!(!cols.contains("age"));
        assert_eq!(cols.len(), 1);
    }

    #[test]
    fn test_choose_best_index_prefers_composite() {
        let catalog = build_catalog_with_indexes();
        let indexes = catalog.list_indexes(&TableName::new("t"));
        let mut eq_cols = HashSet::new();
        eq_cols.insert("id".to_string());
        eq_cols.insert("age".to_string());
        let best = choose_best_index(&indexes, &eq_cols).unwrap();
        assert_eq!(best.name, "idx_id_age");
    }

    #[test]
    fn test_choose_best_index_prefers_unique_on_tie() {
        let catalog = build_catalog_with_indexes();
        let indexes = catalog.list_indexes(&TableName::new("t"));
        let mut eq_cols = HashSet::new();
        eq_cols.insert("id".to_string());
        let best = choose_best_index(&indexes, &eq_cols).unwrap();
        // idx_id 和 idx_unique_id 都匹配 1 列，选 UNIQUE
        assert_eq!(best.name, "idx_unique_id");
    }

    #[test]
    fn test_choose_best_index_returns_none_when_no_match() {
        let catalog = build_catalog_with_indexes();
        let indexes = catalog.list_indexes(&TableName::new("t"));
        let eq_cols = HashSet::new(); // 空集合
        let best = choose_best_index(&indexes, &eq_cols);
        assert!(best.is_none());
    }

    #[test]
    fn test_choose_best_index_leftmost_prefix() {
        // 索引 (id, age) 中，age 单独出现不能使用该复合索引
        let catalog = build_catalog_with_indexes();
        let indexes = catalog.list_indexes(&TableName::new("t"));
        let mut eq_cols = HashSet::new();
        eq_cols.insert("age".to_string());
        // idx_age 可用（1 列匹配），idx_id_age 不可用（首列 id 不在等值集合）
        let best = choose_best_index(&indexes, &eq_cols).unwrap();
        assert_eq!(best.name, "idx_age");
    }

    #[test]
    fn test_in_memory_catalog_add_and_remove_index() {
        let mut catalog = InMemoryCatalog::new();
        catalog.add_table(TableSchema {
            name: TableName::new("t"),
            columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
        });
        catalog.add_index(IndexDefinition::new(
            "idx_id",
            TableName::new("t"),
            vec![IndexColumn::new("id")],
        ));
        let indexes = catalog.list_indexes(&TableName::new("t"));
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].name, "idx_id");

        // 重复添加同名索引 → 替换
        catalog.add_index(IndexDefinition::new_unique(
            "idx_id",
            TableName::new("t"),
            vec![IndexColumn::new("id")],
        ));
        let indexes = catalog.list_indexes(&TableName::new("t"));
        assert_eq!(indexes.len(), 1);
        assert!(indexes[0].unique);

        // 删除索引
        let removed = catalog.remove_index("idx_id");
        assert!(removed.is_some());
        assert!(removed.unwrap().unique);
        let indexes = catalog.list_indexes(&TableName::new("t"));
        assert!(indexes.is_empty());
    }

    #[test]
    fn test_catalog_list_indexes_empty_for_unknown_table() {
        let catalog = InMemoryCatalog::new();
        let indexes = catalog.list_indexes(&TableName::new("nonexistent"));
        assert!(indexes.is_empty());
    }

    #[test]
    fn test_catalog_list_indexes_case_insensitive() {
        let mut catalog = InMemoryCatalog::new();
        catalog.add_table(TableSchema {
            name: TableName::new("MyTable"),
            columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
        });
        catalog.add_index(IndexDefinition::new(
            "idx_id",
            TableName::new("MyTable"),
            vec![IndexColumn::new("id")],
        ));
        // 大小写不敏感查询
        let indexes = catalog.list_indexes(&TableName::new("MYTABLE"));
        assert_eq!(indexes.len(), 1);
        let indexes = catalog.list_indexes(&TableName::new("mytable"));
        assert_eq!(indexes.len(), 1);
    }
}

// =====================================================================
//  CommonSubexpressionElimination — Phase 5.8 CSE 公共子表达式消除
// =====================================================================

/// 公共子表达式消除（CSE）规则 — Phase 5.8
///
/// 检测计划树中重复出现的相同子树（仅叶子节点 `Scan`/`IndexScan`），
/// 第一次出现包装为 `Shared`，后续出现替换为 `MemoRef`，由执行器缓存结果复用。
///
/// # 算法
///
/// 1. **Phase 1（collect_leaf_fingerprints）**：递归遍历原始 plan，对每个
///    `Scan`/`IndexScan` 叶子节点计算结构指纹（基于 `Debug` 字符串的哈希），
///    统计每个指纹出现次数
/// 2. **Phase 2（apply_recursive）**：递归处理 plan：
///    - 对 `Filter`/`Projection`/`Join`/`Aggregate`/`Sort`/`Limit`/`Distinct`/`SetOp`
///      节点先递归处理子节点
///    - 对 `Scan`/`IndexScan` 叶子节点：若指纹出现次数 > 1，按"首次 Shared / 后续 MemoRef"
///      规则替换
///
/// # 限制
///
/// - **仅叶子节点 CSE**：中间节点（Filter/Projection 等）的 CSE 需要复杂的等价性证明
///   （因递归处理后子节点已被包装为 Shared/MemoRef，会改变父节点指纹），本阶段不实现
/// - **指纹基于 `Debug` 字符串哈希**：理论上存在哈希碰撞风险，但实际碰撞概率极低
///   （u64 哈希空间）；后续可替换为更精细的结构化哈希
/// - **不处理 DML/DDL**：DML（Insert/Update/Delete/Replace/Merge）和 DDL 节点不参与 CSE
/// - **CSE 安全节点白名单**：仅 Scan/IndexScan/Filter/Projection/Join/Aggregate/Sort/
///   Limit/Distinct/SetOp/Empty/Dual 参与 CSE；其他节点（DML/DDL/Copy/Listen 等）不变
pub struct CommonSubexpressionElimination;

impl CommonSubexpressionElimination {
    /// 应用 CSE 规则
    pub fn apply(plan: LogicalPlan) -> LogicalPlan {
        // Phase 1: 收集叶子节点指纹
        let mut counts: HashMap<u64, usize> = HashMap::new();
        collect_leaf_fingerprints(&plan, &mut counts);

        // Phase 2: 递归替换
        let mut next_id: u64 = 1;
        let mut memo: HashMap<u64, u64> = HashMap::new();
        let mut wrapped: HashSet<u64> = HashSet::new();
        Self::apply_recursive(plan, &counts, &mut next_id, &mut memo, &mut wrapped)
    }

    fn apply_recursive(
        plan: LogicalPlan,
        counts: &HashMap<u64, usize>,
        next_id: &mut u64,
        memo: &mut HashMap<u64, u64>,
        wrapped: &mut HashSet<u64>,
    ) -> LogicalPlan {
        match plan {
            LogicalPlan::Filter { predicate, input } => {
                let input = Self::apply_recursive(*input, counts, next_id, memo, wrapped);
                LogicalPlan::Filter {
                    predicate,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Projection {
                exprs,
                output_names,
                input,
            } => {
                let input = Self::apply_recursive(*input, counts, next_id, memo, wrapped);
                LogicalPlan::Projection {
                    exprs,
                    output_names,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
            } => {
                let left = Self::apply_recursive(*left, counts, next_id, memo, wrapped);
                let right = Self::apply_recursive(*right, counts, next_id, memo, wrapped);
                LogicalPlan::Join {
                    join_type,
                    condition,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            }
            LogicalPlan::Aggregate {
                grouping_sets,
                aggregates,
                having,
                input,
            } => {
                let input = Self::apply_recursive(*input, counts, next_id, memo, wrapped);
                LogicalPlan::Aggregate {
                    grouping_sets,
                    aggregates,
                    having,
                    input: Box::new(input),
                }
            }
            // Phase 6.2: 窗口函数节点 — 递归处理 input
            LogicalPlan::Window {
                window_funcs,
                input,
            } => {
                let input = Self::apply_recursive(*input, counts, next_id, memo, wrapped);
                LogicalPlan::Window {
                    window_funcs,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Sort { order_by, input } => {
                let input = Self::apply_recursive(*input, counts, next_id, memo, wrapped);
                LogicalPlan::Sort {
                    order_by,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Limit {
                limit,
                offset,
                input,
            } => {
                let input = Self::apply_recursive(*input, counts, next_id, memo, wrapped);
                LogicalPlan::Limit {
                    limit,
                    offset,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Distinct { input } => {
                let input = Self::apply_recursive(*input, counts, next_id, memo, wrapped);
                LogicalPlan::Distinct {
                    input: Box::new(input),
                }
            }
            LogicalPlan::SetOp {
                op,
                quantifier,
                left,
                right,
            } => {
                let left = Self::apply_recursive(*left, counts, next_id, memo, wrapped);
                let right = Self::apply_recursive(*right, counts, next_id, memo, wrapped);
                LogicalPlan::SetOp {
                    op,
                    quantifier,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            }
            // 叶子节点：Scan/IndexScan — CSE 替换目标
            leaf @ (LogicalPlan::Scan { .. } | LogicalPlan::IndexScan { .. }) => {
                let fp = fingerprint_plan(&leaf);
                let count = counts.get(&fp).copied().unwrap_or(0);
                if count > 1 {
                    if wrapped.contains(&fp) {
                        // 后续出现 → MemoRef
                        let id = memo[&fp];
                        let schema = leaf_schema(&leaf);
                        LogicalPlan::MemoRef { id, schema }
                    } else {
                        // 首次出现 → Shared
                        let id = *next_id;
                        *next_id += 1;
                        memo.insert(fp, id);
                        wrapped.insert(fp);
                        LogicalPlan::Shared {
                            id,
                            plan: Box::new(leaf),
                        }
                    }
                } else {
                    leaf
                }
            }
            // DML/DDL/其他节点不变
            other => other,
        }
    }
}

/// 递归收集叶子节点（Scan/IndexScan）的指纹计数
fn collect_leaf_fingerprints(plan: &LogicalPlan, counts: &mut HashMap<u64, usize>) {
    match plan {
        LogicalPlan::Scan { .. } | LogicalPlan::IndexScan { .. } => {
            let fp = fingerprint_plan(plan);
            *counts.entry(fp).or_insert(0) += 1;
        }
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Projection { input, .. }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Distinct { input, .. }
        | LogicalPlan::Aggregate { input, .. }
        | LogicalPlan::Window { input, .. } => {
            collect_leaf_fingerprints(input, counts);
        }
        LogicalPlan::Join { left, right, .. } | LogicalPlan::SetOp { left, right, .. } => {
            collect_leaf_fingerprints(left, counts);
            collect_leaf_fingerprints(right, counts);
        }
        LogicalPlan::Shared { plan, .. } => collect_leaf_fingerprints(plan, counts),
        // DML/DDL/MemoRef/Empty/Dual/其他 — 不递归
        _ => {}
    }
}

/// 计算计划节点的结构指纹（基于 `Debug` 字符串的哈希）
fn fingerprint_plan(plan: &LogicalPlan) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    format!("{:?}", plan).hash(&mut hasher);
    hasher.finish()
}

/// 提取叶子节点的 schema（用于 MemoRef 节点）
fn leaf_schema(plan: &LogicalPlan) -> szrsql_sql::plan::TableSchema {
    match plan {
        LogicalPlan::Scan { schema, .. } | LogicalPlan::IndexScan { schema, .. } => schema.clone(),
        _ => panic!("leaf_schema only valid for Scan/IndexScan, got {:?}", plan),
    }
}

// =====================================================================
//  HtapColumnarRewrite — P2-15 HTAP 列存路由
// =====================================================================

/// HTAP 列存重写规则。
///
/// 当 Catalog 报告某表存在列存副本（`has_columnar_store == true`）时，
/// 将该表的 `Scan` 节点替换为 `ColumnarScan`，使执行器走列存 batch-mode 路径：
///
/// - 纯聚合查询（Aggregate 无 GROUP BY / HAVING）→ 列存 SIMD 快速路径，
///   跳过行材料化，直接在 `ColumnarBatch` 上计算 SUM/AVG/COUNT/MIN/MAX
/// - 其他查询 → 列存全表扫描 + 行材料化（与 `execute_scan` 回退路径等价，
///   但显式走列存，避免行存查找开销）
///
/// DML 节点（Insert/Update/Delete/Replace）不递归，保证其内部 Scan
/// 仍走行存 MVCC 可见性路径。
pub struct HtapColumnarRewrite<'a> {
    catalog: &'a dyn szrsql_sql::plan::Catalog,
}

impl<'a> HtapColumnarRewrite<'a> {
    /// 创建列存重写规则应用器
    pub fn new(catalog: &'a dyn szrsql_sql::plan::Catalog) -> Self {
        Self { catalog }
    }

    /// 应用列存重写规则
    pub fn apply(&self, plan: LogicalPlan) -> LogicalPlan {
        self.apply_recursive(plan)
    }

    fn apply_recursive(&self, plan: LogicalPlan) -> LogicalPlan {
        match plan {
            LogicalPlan::Filter { predicate, input } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Filter {
                    predicate,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Projection {
                exprs,
                output_names,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Projection {
                    exprs,
                    output_names,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
            } => {
                let left = self.apply_recursive(*left);
                let right = self.apply_recursive(*right);
                LogicalPlan::Join {
                    join_type,
                    condition,
                    left: Box::new(left),
                    right: Box::new(right),
                }
            }
            LogicalPlan::Aggregate {
                grouping_sets,
                aggregates,
                having,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Aggregate {
                    grouping_sets,
                    aggregates,
                    having,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Window {
                window_funcs,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Window {
                    window_funcs,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Sort { order_by, input } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Sort {
                    order_by,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Limit {
                limit,
                offset,
                input,
            } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Limit {
                    limit,
                    offset,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Distinct { input } => {
                let input = self.apply_recursive(*input);
                LogicalPlan::Distinct {
                    input: Box::new(input),
                }
            }
            LogicalPlan::SetOp {
                left,
                right,
                op,
                quantifier,
            } => {
                let left = self.apply_recursive(*left);
                let right = self.apply_recursive(*right);
                LogicalPlan::SetOp {
                    left: Box::new(left),
                    right: Box::new(right),
                    op,
                    quantifier,
                }
            }
            LogicalPlan::Scan {
                table,
                alias,
                schema,
            } => {
                // HTAP 路由决策：表有列存副本 → 走 ColumnarScan
                if self.catalog.has_columnar_store(&table) {
                    LogicalPlan::ColumnarScan {
                        table,
                        alias,
                        schema,
                    }
                } else {
                    LogicalPlan::Scan {
                        table,
                        alias,
                        schema,
                    }
                }
            }
            // IndexScan / MaterializedViewScan / Empty / Dual / Shared / MemoRef 不变
            // DML（Insert/Update/Delete/Replace）不递归：保证内部 Scan 走行存 MVCC 路径
            other => other,
        }
    }
}

#[cfg(test)]
mod cse_tests {
    use super::*;
    use szrsql_sql::ast::{
        ColumnDefinition, Expr as AExpr, OrderByExpr, SetOperator, SetQuantifier, TableName,
    };
    use szrsql_sql::plan::{LogicalPlan, TableSchema};
    use szrsql_types::value::{ColumnType, Value};

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

    fn schema_u() -> TableSchema {
        TableSchema {
            name: TableName::new("u"),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("age", ColumnType::Int64),
            ],
        }
    }

    /// 构造 Scan(t)
    fn scan_t() -> LogicalPlan {
        LogicalPlan::Scan {
            table: TableName::new("t"),
            alias: None,
            schema: schema_t(),
        }
    }

    /// 构造 Scan(u)
    fn scan_u() -> LogicalPlan {
        LogicalPlan::Scan {
            table: TableName::new("u"),
            alias: None,
            schema: schema_u(),
        }
    }

    /// 构造 Scan(t) with alias
    fn scan_t_aliased(alias: &str) -> LogicalPlan {
        LogicalPlan::Scan {
            table: TableName::new("t"),
            alias: Some(alias.to_string()),
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

    fn inner_join(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Join {
            join_type: JoinType::Inner,
            condition: JoinCondition::On(make_eq("id", 0)),
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    fn filter(pred: AExpr, input: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Filter {
            predicate: pred,
            input: Box::new(input),
        }
    }

    fn projection(input: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Projection {
            exprs: vec![(AExpr::Identifier(vec!["id".to_string()]), None)],
            output_names: vec!["id".to_string()],
            input: Box::new(input),
        }
    }

    #[test]
    fn test_cse_detects_duplicate_scan_in_join() {
        // Join(t, t) → 第一个 t 包装为 Shared，第二个 t 替换为 MemoRef
        let plan = inner_join(scan_t(), scan_t());
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::Join { left, right, .. } => {
                assert!(
                    matches!(left.as_ref(), LogicalPlan::Shared { .. }),
                    "left should be Shared, got {:?}",
                    left
                );
                assert!(
                    matches!(right.as_ref(), LogicalPlan::MemoRef { .. }),
                    "right should be MemoRef, got {:?}",
                    right
                );
                // 验证 id 一致
                let left_id = match left.as_ref() {
                    LogicalPlan::Shared { id, .. } => *id,
                    _ => unreachable!(),
                };
                let right_id = match right.as_ref() {
                    LogicalPlan::MemoRef { id, .. } => *id,
                    _ => unreachable!(),
                };
                assert_eq!(left_id, right_id, "Shared id and MemoRef id must match");
            }
            _ => panic!("expected Join, got {:?}", result),
        }
    }

    #[test]
    fn test_cse_no_change_when_no_duplicate() {
        // Join(t, u) → 无重复 → 计划不变
        let plan = inner_join(scan_t(), scan_u());
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::Join { left, right, .. } => {
                assert!(matches!(left.as_ref(), LogicalPlan::Scan { .. }));
                assert!(matches!(right.as_ref(), LogicalPlan::Scan { .. }));
            }
            _ => panic!("expected Join, got {:?}", result),
        }
    }

    #[test]
    fn test_cse_no_change_for_single_scan() {
        // 单个 Scan → 无重复 → 不变
        let plan = scan_t();
        let result = CommonSubexpressionElimination::apply(plan);
        assert!(matches!(result, LogicalPlan::Scan { .. }));
    }

    #[test]
    fn test_cse_skips_dml() {
        // DML 节点不参与 CSE（这里用 Insert 测试）
        let plan = LogicalPlan::Insert {
            table: TableName::new("t"),
            schema: schema_t(),
            columns: None,
            source: szrsql_sql::plan::InsertSourcePlan::DefaultValues,
            on_conflict: None,
            returning: None,
        };
        let result = CommonSubexpressionElimination::apply(plan);
        assert!(matches!(result, LogicalPlan::Insert { .. }));
    }

    #[test]
    fn test_cse_recursive_through_projection() {
        // Projection(Join(t, t)) → 内部 Join 的 Scan 被替换
        let plan = projection(inner_join(scan_t(), scan_t()));
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::Projection { input, .. } => match input.as_ref() {
                LogicalPlan::Join { left, right, .. } => {
                    assert!(matches!(left.as_ref(), LogicalPlan::Shared { .. }));
                    assert!(matches!(right.as_ref(), LogicalPlan::MemoRef { .. }));
                }
                _ => panic!("expected Join, got {:?}", input),
            },
            _ => panic!("expected Projection, got {:?}", result),
        }
    }

    #[test]
    fn test_cse_recursive_through_filter() {
        // Filter(Join(t, t)) → 内部 Join 的 Scan 被替换
        let plan = filter(make_eq("id", 1), inner_join(scan_t(), scan_t()));
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::Filter { input, .. } => match input.as_ref() {
                LogicalPlan::Join { left, right, .. } => {
                    assert!(matches!(left.as_ref(), LogicalPlan::Shared { .. }));
                    assert!(matches!(right.as_ref(), LogicalPlan::MemoRef { .. }));
                }
                _ => panic!("expected Join, got {:?}", input),
            },
            _ => panic!("expected Filter, got {:?}", result),
        }
    }

    #[test]
    fn test_cse_recursive_through_aggregate() {
        // Aggregate(Join(t, t)) → 内部 Join 的 Scan 被替换
        let plan = LogicalPlan::Aggregate {
            grouping_sets: vec![vec![AExpr::Identifier(vec!["id".to_string()])]],
            aggregates: vec![],
            having: None,
            input: Box::new(inner_join(scan_t(), scan_t())),
        };
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::Aggregate { input, .. } => match input.as_ref() {
                LogicalPlan::Join { left, right, .. } => {
                    assert!(matches!(left.as_ref(), LogicalPlan::Shared { .. }));
                    assert!(matches!(right.as_ref(), LogicalPlan::MemoRef { .. }));
                }
                _ => panic!("expected Join, got {:?}", input),
            },
            _ => panic!("expected Aggregate, got {:?}", result),
        }
    }

    #[test]
    fn test_cse_recursive_through_nested_join() {
        // Join(Join(t, t), t) → 三处 t，第一个 Shared，后两个 MemoRef
        let plan = inner_join(inner_join(scan_t(), scan_t()), scan_t());
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::Join { left, right, .. } => {
                // 外层 right 应该是 MemoRef
                assert!(
                    matches!(right.as_ref(), LogicalPlan::MemoRef { .. }),
                    "outer right should be MemoRef, got {:?}",
                    right
                );
                // 外层 left 是内层 Join
                match left.as_ref() {
                    LogicalPlan::Join { left, right, .. } => {
                        assert!(matches!(left.as_ref(), LogicalPlan::Shared { .. }));
                        assert!(matches!(right.as_ref(), LogicalPlan::MemoRef { .. }));
                    }
                    _ => panic!("expected inner Join, got {:?}", left),
                }
            }
            _ => panic!("expected outer Join, got {:?}", result),
        }
    }

    #[test]
    fn test_cse_id_consistency() {
        let plan = inner_join(scan_t(), scan_t());
        let result = CommonSubexpressionElimination::apply(plan);
        if let LogicalPlan::Join { left, right, .. } = result {
            let left_id = match left.as_ref() {
                LogicalPlan::Shared { id, .. } => *id,
                _ => unreachable!(),
            };
            let right_id = match right.as_ref() {
                LogicalPlan::MemoRef { id, .. } => *id,
                _ => unreachable!(),
            };
            assert_eq!(left_id, right_id);
        }
    }

    #[test]
    fn test_cse_multiple_distinct_duplicates() {
        // Join(Join(t, t), Join(u, u)) → 两组重复
        let plan = inner_join(
            inner_join(scan_t(), scan_t()),
            inner_join(scan_u(), scan_u()),
        );
        let result = CommonSubexpressionElimination::apply(plan);
        if let LogicalPlan::Join { left, right, .. } = result {
            // 内层 Join(t, t)
            match left.as_ref() {
                LogicalPlan::Join { left, right, .. } => {
                    assert!(matches!(left.as_ref(), LogicalPlan::Shared { .. }));
                    assert!(matches!(right.as_ref(), LogicalPlan::MemoRef { .. }));
                    // 验证 t 的 id 一致
                    let l_id = match left.as_ref() {
                        LogicalPlan::Shared { id, .. } => *id,
                        _ => unreachable!(),
                    };
                    let r_id = match right.as_ref() {
                        LogicalPlan::MemoRef { id, .. } => *id,
                        _ => unreachable!(),
                    };
                    assert_eq!(l_id, r_id);
                }
                _ => panic!("expected inner Join(t,t), got {:?}", left),
            }
            // 内层 Join(u, u)
            match right.as_ref() {
                LogicalPlan::Join { left, right, .. } => {
                    assert!(matches!(left.as_ref(), LogicalPlan::Shared { .. }));
                    assert!(matches!(right.as_ref(), LogicalPlan::MemoRef { .. }));
                    // u 的 id 应该不同于 t 的 id
                    let l_id = match left.as_ref() {
                        LogicalPlan::Shared { id, .. } => *id,
                        _ => unreachable!(),
                    };
                    let r_id = match right.as_ref() {
                        LogicalPlan::MemoRef { id, .. } => *id,
                        _ => unreachable!(),
                    };
                    assert_eq!(l_id, r_id);
                    // 不同指纹应有不同 id
                    assert_ne!(l_id, 0);
                }
                _ => panic!("expected inner Join(u,u), got {:?}", right),
            }
        }
    }

    #[test]
    fn test_cse_three_duplicates() {
        // Join(Join(t, t), t) → 三处 t，第一个 Shared，后两个 MemoRef，所有 id 一致
        let plan = inner_join(inner_join(scan_t(), scan_t()), scan_t());
        let result = CommonSubexpressionElimination::apply(plan);
        let mut shared_ids = Vec::new();
        let mut memo_ids = Vec::new();
        collect_shared_and_memo_ids(&result, &mut shared_ids, &mut memo_ids);
        assert_eq!(
            shared_ids.len(),
            1,
            "expected 1 Shared, got {}",
            shared_ids.len()
        );
        assert_eq!(
            memo_ids.len(),
            2,
            "expected 2 MemoRef, got {}",
            memo_ids.len()
        );
        // 所有 id 一致
        let shared_id = shared_ids[0];
        for mid in &memo_ids {
            assert_eq!(*mid, shared_id, "MemoRef id must match Shared id");
        }
    }

    fn collect_shared_and_memo_ids(plan: &LogicalPlan, shared: &mut Vec<u64>, memo: &mut Vec<u64>) {
        match plan {
            LogicalPlan::Shared { id, plan } => {
                shared.push(*id);
                collect_shared_and_memo_ids(plan, shared, memo);
            }
            LogicalPlan::MemoRef { id, .. } => {
                memo.push(*id);
            }
            LogicalPlan::Filter { input, .. }
            | LogicalPlan::Projection { input, .. }
            | LogicalPlan::Sort { input, .. }
            | LogicalPlan::Limit { input, .. }
            | LogicalPlan::Distinct { input, .. }
            | LogicalPlan::Aggregate { input, .. } => {
                collect_shared_and_memo_ids(input, shared, memo);
            }
            LogicalPlan::Join { left, right, .. } | LogicalPlan::SetOp { left, right, .. } => {
                collect_shared_and_memo_ids(left, shared, memo);
                collect_shared_and_memo_ids(right, shared, memo);
            }
            _ => {}
        }
    }

    #[test]
    fn test_cse_indexscan_also_supported() {
        // Join(IndexScan(t), IndexScan(t)) → 同样 CSE
        let index_scan = LogicalPlan::IndexScan {
            table: TableName::new("t"),
            alias: None,
            schema: schema_t(),
            index_name: "idx_id".to_string(),
            index_columns: vec!["id".to_string()],
            predicate: make_eq("id", 5),
        };
        let plan = inner_join(index_scan.clone(), index_scan);
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::Join { left, right, .. } => {
                assert!(matches!(left.as_ref(), LogicalPlan::Shared { .. }));
                assert!(matches!(right.as_ref(), LogicalPlan::MemoRef { .. }));
            }
            _ => panic!("expected Join, got {:?}", result),
        }
    }

    #[test]
    fn test_cse_mixed_scan_and_indexscan() {
        // Join(Scan(t), IndexScan(t)) → 不同指纹 → 不替换
        let scan = scan_t();
        let index_scan = LogicalPlan::IndexScan {
            table: TableName::new("t"),
            alias: None,
            schema: schema_t(),
            index_name: "idx_id".to_string(),
            index_columns: vec!["id".to_string()],
            predicate: make_eq("id", 5),
        };
        let plan = inner_join(scan, index_scan);
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::Join { left, right, .. } => {
                assert!(matches!(left.as_ref(), LogicalPlan::Scan { .. }));
                assert!(matches!(right.as_ref(), LogicalPlan::IndexScan { .. }));
            }
            _ => panic!("expected Join, got {:?}", result),
        }
    }

    #[test]
    fn test_cse_preserves_other_node_structure() {
        // Filter(Join(t, t)) → Filter 结构保留，内部 Join 子树被替换
        let plan = filter(make_eq("id", 1), inner_join(scan_t(), scan_t()));
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::Filter { predicate, input } => {
                // 谓词保留
                assert_eq!(predicate, make_eq("id", 1));
                // input 是 Join
                assert!(matches!(input.as_ref(), LogicalPlan::Join { .. }));
            }
            _ => panic!("expected Filter, got {:?}", result),
        }
    }

    #[test]
    fn test_cse_distinguishes_aliased_scans() {
        // Scan(t) vs Scan(t) with alias 'x' → 不同指纹 → 不替换
        let plan = inner_join(scan_t(), scan_t_aliased("x"));
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::Join { left, right, .. } => {
                assert!(matches!(left.as_ref(), LogicalPlan::Scan { .. }));
                assert!(matches!(right.as_ref(), LogicalPlan::Scan { .. }));
            }
            _ => panic!("expected Join, got {:?}", result),
        }
    }

    #[test]
    fn test_fingerprint_plan_identical_plans() {
        let p1 = scan_t();
        let p2 = scan_t();
        assert_eq!(fingerprint_plan(&p1), fingerprint_plan(&p2));
    }

    #[test]
    fn test_fingerprint_plan_different_plans() {
        let p1 = scan_t();
        let p2 = scan_u();
        assert_ne!(fingerprint_plan(&p1), fingerprint_plan(&p2));
    }

    #[test]
    fn test_cse_through_setop() {
        // SetOp(Union, t, t) → 内部 Scan 被替换
        let plan = LogicalPlan::SetOp {
            op: SetOperator::Union,
            quantifier: SetQuantifier::Distinct,
            left: Box::new(scan_t()),
            right: Box::new(scan_t()),
        };
        let result = CommonSubexpressionElimination::apply(plan);
        match result {
            LogicalPlan::SetOp { left, right, .. } => {
                assert!(matches!(left.as_ref(), LogicalPlan::Shared { .. }));
                assert!(matches!(right.as_ref(), LogicalPlan::MemoRef { .. }));
            }
            _ => panic!("expected SetOp, got {:?}", result),
        }
    }

    #[test]
    fn test_cse_through_sort_limit_distinct() {
        // Sort(Limit(Distinct(Join(t, t)))) → 内部 Join 的 Scan 被替换
        let join = inner_join(scan_t(), scan_t());
        let distinct = LogicalPlan::Distinct {
            input: Box::new(join),
        };
        let limit = LogicalPlan::Limit {
            limit: None,
            offset: None,
            input: Box::new(distinct),
        };
        let sort = LogicalPlan::Sort {
            order_by: vec![OrderByExpr {
                expr: AExpr::Identifier(vec!["id".to_string()]),
                asc: true,
                nulls_first: false,
            }],
            input: Box::new(limit),
        };
        let result = CommonSubexpressionElimination::apply(sort);
        // 递归到最内层 Join
        fn find_join(plan: &LogicalPlan) -> Option<(&LogicalPlan, &LogicalPlan)> {
            match plan {
                LogicalPlan::Join { left, right, .. } => Some((left, right)),
                LogicalPlan::Sort { input, .. }
                | LogicalPlan::Limit { input, .. }
                | LogicalPlan::Distinct { input, .. }
                | LogicalPlan::Filter { input, .. }
                | LogicalPlan::Projection { input, .. }
                | LogicalPlan::Aggregate { input, .. } => find_join(input),
                LogicalPlan::Shared { plan, .. } => find_join(plan),
                _ => None,
            }
        }
        match &result {
            LogicalPlan::Sort { input, .. } => match find_join(input.as_ref()) {
                Some((left, right)) => {
                    assert!(matches!(left, LogicalPlan::Shared { .. }));
                    assert!(matches!(right, LogicalPlan::MemoRef { .. }));
                }
                None => panic!("no Join found in Sort subtree"),
            },
            _ => panic!("expected Sort, got {:?}", result),
        }
    }

    // ==================================================================
    //  集成测试：同一子查询出现两次 → 只执行一次并共享结果 — Phase 5.8
    // ==================================================================

    /// 端到端验证 CSE 后的 Shared/MemoRef 计划能被执行器正确执行，
    /// 且结果与未优化的原计划一致（结果正确性）。
    ///
    /// 计划结构：SetOp(UnionAll, Scan(t), Scan(t))
    /// - 未优化：两次扫描 t，结果 6 行（3 + 3）
    /// - CSE 后：Shared(Scan(t)) + MemoRef，第一次执行物化到 memo_cache，
    ///   第二次直接读缓存，结果仍为 6 行
    #[test]
    fn test_cse_integration_shared_memo_ref_executes_correctly() {
        use szrsql_sql::executor::{Executor, InMemoryTable};

        // 1. 构造测试表 t(id, name) 并插入 3 行
        let mut t = InMemoryTable::with_columns(
            "t",
            vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
        );
        t.insert(vec![Value::Int64(1), Value::Text("alice".to_string())]);
        t.insert(vec![Value::Int64(2), Value::Text("bob".to_string())]);
        t.insert(vec![Value::Int64(3), Value::Text("carol".to_string())]);

        // 2. 构造 SetOp(UnionAll, Scan(t), Scan(t)) — 两个相同子树
        let plan = LogicalPlan::SetOp {
            op: SetOperator::Union,
            quantifier: SetQuantifier::All,
            left: Box::new(scan_t()),
            right: Box::new(scan_t()),
        };

        // 3. 应用 CSE：第一个 Scan(t) 包装为 Shared，第二个替换为 MemoRef
        let optimized = CommonSubexpressionElimination::apply(plan);
        match &optimized {
            LogicalPlan::SetOp { left, right, .. } => {
                assert!(
                    matches!(left.as_ref(), LogicalPlan::Shared { .. }),
                    "left should be Shared after CSE, got {:?}",
                    left
                );
                assert!(
                    matches!(right.as_ref(), LogicalPlan::MemoRef { .. }),
                    "right should be MemoRef after CSE, got {:?}",
                    right
                );
            }
            _ => panic!("expected SetOp after CSE, got {:?}", optimized),
        }

        // 4. 执行 CSE 优化后的计划
        let mut exec = Executor::new();
        exec.register_table(&t);
        let optimized_rows = exec.execute(&optimized).expect("CSE 优化计划执行失败");

        // 5. 执行未优化计划作为对照
        let plan_unopt = LogicalPlan::SetOp {
            op: SetOperator::Union,
            quantifier: SetQuantifier::All,
            left: Box::new(scan_t()),
            right: Box::new(scan_t()),
        };
        let mut exec2 = Executor::new();
        exec2.register_table(&t);
        let unoptimized_rows = exec2.execute(&plan_unopt).expect("原计划执行失败");

        // 6. 结果一致性校验：CSE 后行数与未优化一致
        assert_eq!(
            optimized_rows.len(),
            6,
            "CSE 后应有 6 行（3+3 UNION ALL），实际 {}",
            optimized_rows.len()
        );
        assert_eq!(
            optimized_rows.len(),
            unoptimized_rows.len(),
            "CSE 优化前后行数应一致"
        );

        // 7. 内容一致性：每行的 id 都应在 {1,2,3} 中（每个 id 出现两次）
        let mut id_counts: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
        for row in &optimized_rows {
            match &row[0] {
                Value::Int64(id) => *id_counts.entry(*id).or_insert(0) += 1,
                _ => panic!("第一列应为 Int64，实际 {:?}", row[0]),
            }
        }
        assert_eq!(id_counts.remove(&1), Some(2), "id=1 应出现 2 次");
        assert_eq!(id_counts.remove(&2), Some(2), "id=2 应出现 2 次");
        assert_eq!(id_counts.remove(&3), Some(2), "id=3 应出现 2 次");
        assert!(id_counts.is_empty(), "不应有其他 id：{:?}", id_counts);
    }
}

// =====================================================================
//  HtapColumnarRewrite 端到端测试 — P2-15
// =====================================================================

#[cfg(test)]
mod htap_rewrite_tests {
    use super::HtapColumnarRewrite;
    use szrsql_sql::ast::{ColumnDefinition, TableName};
    use szrsql_sql::executor::Executor;
    use szrsql_sql::parser::parse_sql;
    use szrsql_sql::plan::LogicalPlan;
    use szrsql_sql::plan::{InMemoryCatalog, Planner, TableSchema};
    use szrsql_storage::columnar::{
        ColumnSchema, ColumnSpec, ColumnVector, ColumnarBatch, ColumnarTable, ColumnarType,
        NullBitmap,
    };
    use szrsql_types::value::{ColumnType, Value};

    /// 验证 `HtapColumnarRewrite` 将列存表的 `Scan` 改写为 `ColumnarScan`，
    /// 并通过执行器产出正确聚合结果。
    #[test]
    fn test_htap_rewrite_columnar_scan_and_aggregate() {
        // 1. 建 catalog + 表 schema
        let mut catalog = InMemoryCatalog::new();
        let schema = TableSchema {
            name: TableName::new("sensor_data"),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("value", ColumnType::Int64),
                ColumnDefinition::new("score", ColumnType::Float64),
            ],
        };
        catalog.add_table(schema.clone());

        // 2. 构造列存表并填充 100 行
        let col_schema = ColumnSchema::from_columns(vec![
            ColumnSpec::new("id", ColumnarType::Int64),
            ColumnSpec::new("value", ColumnarType::Int64),
            ColumnSpec::new("score", ColumnarType::Float64),
        ]);
        let mut col_table = ColumnarTable::new("sensor_data", col_schema.clone());
        let n = 100usize;
        let ids: Vec<i64> = (1..=n as i64).collect();
        let values: Vec<i64> = (1..=n as i64).map(|i| i * 10).collect();
        let scores: Vec<f64> = (0..n).map(|i| (i as f64) * 1.5).collect();
        let batch = ColumnarBatch::from_columns(
            col_schema,
            vec![
                ColumnVector::Int64 {
                    data: ids,
                    null_bitmap: NullBitmap::new(n),
                },
                ColumnVector::Int64 {
                    data: values,
                    null_bitmap: NullBitmap::new(n),
                },
                ColumnVector::Float64 {
                    data: scores,
                    null_bitmap: NullBitmap::new(n),
                },
            ],
        )
        .unwrap();
        col_table.append_batch(batch).unwrap();

        // 3. 注册列存标记
        catalog.register_columnar_table("sensor_data");

        // 4. 规划查询
        let planner = Planner::new(&catalog);
        let stmts = parse_sql("SELECT SUM(value), AVG(score) FROM sensor_data").unwrap();
        let raw_plan = planner
            .plan_statement(stmts.into_iter().next().unwrap())
            .unwrap();

        // 5. 原始计划：Projection { Aggregate { Scan } }
        assert!(
            matches!(
                &raw_plan,
                LogicalPlan::Projection { input, .. }
                    if matches!(input.as_ref(), LogicalPlan::Aggregate { input, .. }
                        if matches!(input.as_ref(), LogicalPlan::Scan { .. }))
            ),
            "raw plan should be Projection{{Aggregate{{Scan}}}}, got: {:?}",
            raw_plan
        );

        // 6. 应用 HTAP 列存重写
        let rewritten = HtapColumnarRewrite::new(&catalog).apply(raw_plan);

        // 7. 重写后：Projection { Aggregate { ColumnarScan } }
        assert!(
            matches!(
                &rewritten,
                LogicalPlan::Projection { input, .. }
                    if matches!(input.as_ref(), LogicalPlan::Aggregate { input, .. }
                        if matches!(input.as_ref(), LogicalPlan::ColumnarScan { .. }))
            ),
            "rewritten plan should be Projection{{Aggregate{{ColumnarScan}}}}, got: {:?}",
            rewritten
        );

        // 8. 执行重写计划
        let mut exec = Executor::new()
            .with_catalog(&catalog)
            .with_sql_functions_from_catalog(&catalog);
        exec.register_columnar_table("sensor_data", &col_table);
        let rows = exec.execute(&rewritten).unwrap();

        assert_eq!(rows.len(), 1, "聚合结果应为 1 行");
        assert_eq!(rows[0].len(), 2, "应有 2 个聚合列");

        // SUM(value) = 10 * (1+2+...+100) = 50500
        assert_eq!(rows[0][0], Value::Int64(50_500));

        // AVG(score) = 1.5 * (0+1+...+99) / 100 = 74.25
        if let Value::Float64(avg) = rows[0][1] {
            assert!(
                (avg - 74.25).abs() < 1e-9,
                "AVG(score) 应为 74.25，实际 {}",
                avg
            );
        } else {
            panic!("AVG(score) 应为 Float64，实际 {:?}", rows[0][1]);
        }
    }

    /// 未注册列存的表不应被改写
    #[test]
    fn test_htap_rewrite_skips_non_columnar_table() {
        let mut catalog = InMemoryCatalog::new();
        // 表必须在 catalog 中，否则 plan_statement 报 TableNotFound
        catalog.add_table(TableSchema {
            name: TableName::new("sensor_data"),
            columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
        });

        let planner = Planner::new(&catalog);
        let stmts = parse_sql("SELECT * FROM sensor_data").unwrap();
        let raw_plan = planner
            .plan_statement(stmts.into_iter().next().unwrap())
            .unwrap();

        let rewritten = HtapColumnarRewrite::new(&catalog).apply(raw_plan);
        // 非列存表：内层 Scan 不应被改写为 ColumnarScan
        // （planner 可能在外层包 Projection，检查最内层叶子节点即可）
        fn has_columnar_scan(plan: &LogicalPlan) -> bool {
            match plan {
                LogicalPlan::ColumnarScan { .. } => true,
                LogicalPlan::Projection { input, .. }
                | LogicalPlan::Filter { input, .. }
                | LogicalPlan::Aggregate { input, .. } => has_columnar_scan(input),
                _ => false,
            }
        }
        assert!(
            !has_columnar_scan(&rewritten),
            "非列存表不应出现 ColumnarScan，got: {:?}",
            rewritten
        );
    }
}
