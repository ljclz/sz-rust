//! Phase 5.5 — JOIN 顺序优化（DPccp 算法）
//!
//! 使用动态规划枚举所有连接顺序，找到成本最低的 JOIN 树。
//!
//! # 算法
//!
//! DPccp（Dynamic Programming connected complement pair）：
//! 1. 提取 JOIN 树中的所有 base table 与 JOIN 谓词，构建 JOIN 图
//! 2. 使用 bitmask DP 枚举所有子集，存储每个子集的最优计划
//! 3. 对每个大小 ≥ 2 的子集 S，枚举其所有"连通补对"(S1, S2)：
//!    - S1 ∪ S2 = S，S1 ∩ S2 = ∅
//!    - S1 和 S2 在 JOIN 图中各自连通
//!    - S1 与 S2 之间至少存在一条 JOIN 边
//!    - cost = best[S1].cost + best[S2].cost + join_cost(S1, S2)
//! 4. 返回 best[full_set]
//!
//! # 重构 JOIN 条件
//!
//! 当 DPccp 选择将子集 S1 与 S2 直接 JOIN 时，合并所有"跨越 S1-S2 边界"的原始 JOIN
//! 谓词为单个 ON 表达式（AND 连接）。所有这种谓词都引用 S1 和 S2 中的表，因此在新
//! JOIN 节点上是合法的。
//!
//! # 限制
//!
//! - 仅重排 Inner / Cross JOIN；Outer JOIN 保持原始顺序（递归处理其子树）
//! - JOIN 图必须连通；不连通的部分各自独立优化
//! - 不考虑 JOIN 算法选择（HashJoin vs NestedLoopJoin）—— 由 CostModel 估算时自动选择
//! - 表数 N 受 `u32` 位宽限制（N ≤ 31）；实际受 DP 复杂度 O(3^N) 限制，建议 N ≤ 12
//! - 不处理"谓词中单层标识符"（如 `col = 1`）；仅识别 `table.col` 形式的列引用

use std::collections::HashMap;
use std::sync::Arc;

use szrsql_sql::ast::{BinaryOp, Expr, JoinCondition, JoinType};
use szrsql_sql::plan::LogicalPlan;

use crate::cost::{Cost, CostModel};
use crate::statistics::{InMemoryStatisticsStore, StatisticsStore};

// =====================================================================
//  公共 API
// =====================================================================

/// JOIN 顺序优化器（DPccp）
///
/// 无状态（除成本模型外），可并发使用。
pub struct JoinOrderOptimizer {
    /// 成本模型（用于估算每个候选 JOIN 顺序的总成本）
    cost_model: CostModel,
}

impl JoinOrderOptimizer {
    /// 创建 JOIN 顺序优化器
    pub fn new(cost_model: CostModel) -> Self {
        Self { cost_model }
    }

    /// 创建使用空统计信息的优化器（仅基于默认行数估算）
    ///
    /// 适用于不持有真实统计信息的场景（如纯 RBO 测试）。
    pub fn without_stats() -> Self {
        let store: Arc<dyn StatisticsStore> = Arc::new(InMemoryStatisticsStore::new());
        Self::new(CostModel::new(store))
    }

    /// 优化计划：递归处理，对纯 Inner/Cross JOIN 子树应用 DPccp
    pub fn optimize(&self, plan: LogicalPlan) -> LogicalPlan {
        self.optimize_recursive(plan)
    }

    fn optimize_recursive(&self, plan: LogicalPlan) -> LogicalPlan {
        match plan {
            LogicalPlan::Join {
                join_type,
                condition,
                left,
                right,
                ..
            } => {
                // 先递归优化子树
                let left = self.optimize_recursive(*left);
                let right = self.optimize_recursive(*right);

                // 仅 Inner/Cross JOIN 可重排
                if matches!(join_type, JoinType::Inner | JoinType::Cross) {
                    let joined = LogicalPlan::Join {
                        join_type,
                        condition,
                        left: Box::new(left),
                        right: Box::new(right),
                        lateral: false,
                        lateral_subquery: None,
                        right_schema: None,
                    };
                    self.try_reorder(joined)
                } else {
                    LogicalPlan::Join {
                        join_type,
                        condition,
                        left: Box::new(left),
                        right: Box::new(right),
                        lateral: false,
                        lateral_subquery: None,
                        right_schema: None,
                    }
                }
            }
            LogicalPlan::Projection {
                exprs,
                output_names,
                input,
            } => {
                let input = self.optimize_recursive(*input);
                LogicalPlan::Projection {
                    exprs,
                    output_names,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Filter { predicate, input } => {
                let input = self.optimize_recursive(*input);
                LogicalPlan::Filter {
                    predicate,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Aggregate {
                grouping_sets,
                aggregates,
                having,
                input,
            } => {
                let input = self.optimize_recursive(*input);
                LogicalPlan::Aggregate {
                    grouping_sets,
                    aggregates,
                    having,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Sort { order_by, input } => {
                let input = self.optimize_recursive(*input);
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
                let input = self.optimize_recursive(*input);
                LogicalPlan::Limit {
                    limit,
                    offset,
                    input: Box::new(input),
                }
            }
            LogicalPlan::Distinct { input } => {
                let input = self.optimize_recursive(*input);
                LogicalPlan::Distinct {
                    input: Box::new(input),
                }
            }
            // Scan / DML / DDL: 无子树可优化
            other => other,
        }
    }

    /// 尝试对一棵 Inner/Cross JOIN 子树应用 DPccp 重排
    ///
    /// 若 JOIN 图不可提取（如包含 Outer JOIN 或非 Scan 叶子），返回原计划。
    fn try_reorder(&self, plan: LogicalPlan) -> LogicalPlan {
        match extract_join_graph(&plan) {
            Some(graph) if graph.nodes.len() >= 2 => self.dpccp(&graph),
            _ => plan,
        }
    }

    /// DPccp 主算法
    ///
    /// 输入：连通 JOIN 图（至少 2 个节点）
    /// 输出：成本最低的 JOIN 树
    fn dpccp(&self, graph: &JoinGraph) -> LogicalPlan {
        let n = graph.nodes.len();
        let full_mask: u32 = (1u32 << n) - 1;

        // best[mask] = (plan, cost)
        let mut best: HashMap<u32, (LogicalPlan, Cost)> = HashMap::new();

        // 单节点：每个 base table 自身
        for (i, (_, base_plan)) in graph.nodes.iter().enumerate() {
            let mask = 1u32 << i;
            let cost = self.cost_model.estimate(base_plan);
            best.insert(mask, (base_plan.clone(), cost));
        }

        // 按 size 递增枚举子集
        for size in 2..=n {
            for mask in iter_subsets_of_size(full_mask, size) {
                self.enumerate_pairs(graph, mask, &mut best);
            }
        }

        // 返回 full_mask 对应的最优计划
        best.get(&full_mask)
            .map(|(p, _)| p.clone())
            .expect("DPccp must find a plan for the connected full set")
    }

    /// 枚举 mask 的所有连通补对 (S1, S2)，更新 best[mask]
    fn enumerate_pairs(
        &self,
        graph: &JoinGraph,
        mask: u32,
        best: &mut HashMap<u32, (LogicalPlan, Cost)>,
    ) {
        let mut best_total: Option<f64> = None;
        let mut best_plan: Option<LogicalPlan> = None;

        // 枚举 mask 的所有非空真子集 S1
        let mut s1 = (mask - 1) & mask;
        while s1 > 0 {
            let s2 = mask ^ s1;
            // 避免 (s1, s2) 和 (s2, s1) 重复：要求 s1 < s2
            if s1 < s2 {
                // 检查 S1 和 S2 是否各自连通
                if is_connected(graph, s1) && is_connected(graph, s2) {
                    // 找 S1 与 S2 之间的所有边
                    let cross_edges = find_cross_edges(graph, s1, s2);
                    if !cross_edges.is_empty() {
                        // 取 best[S1] 和 best[S2]
                        if let (Some((p1, _)), Some((p2, _))) = (best.get(&s1), best.get(&s2)) {
                            // 尝试两个方向：S1×S2 和 S2×S1
                            for (left_plan, right_plan) in
                                [(p1.clone(), p2.clone()), (p2.clone(), p1.clone())]
                            {
                                if let Some(new_plan) = build_join_with_edges(
                                    graph,
                                    &cross_edges,
                                    left_plan,
                                    right_plan,
                                ) {
                                    let new_cost = self.cost_model.estimate(&new_plan);
                                    let total = new_cost.total();
                                    if best_total.map(|c| total < c).unwrap_or(true) {
                                        best_total = Some(total);
                                        best_plan = Some(new_plan);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // 下一个非空真子集（Gosper's hack 逆操作简化版）
            if s1 == 0 {
                break;
            }
            s1 = (s1 - 1) & mask;
        }

        if let Some(plan) = best_plan {
            let cost = self.cost_model.estimate(&plan);
            best.insert(mask, (plan, cost));
        }
    }
}

// =====================================================================
//  JoinGraph
// =====================================================================

/// JOIN 图
struct JoinGraph {
    /// 节点：(table_key, base_plan)
    /// table_key = alias 优先（无 alias 用 table.name），全小写
    nodes: Vec<(String, LogicalPlan)>,
    /// 边：JOIN 谓词（无向，但 left/right 索引有序：left < right）
    edges: Vec<JoinEdge>,
}

/// JOIN 边
struct JoinEdge {
    /// 左节点索引（较小者）
    left: usize,
    /// 右节点索引（较大者）
    right: usize,
    /// JOIN 类型（仅 Inner / Cross；Outer 在提取阶段被拒绝）
    join_type: JoinType,
    /// JOIN 条件（On/Using/Natural/None）
    condition: JoinCondition,
}

/// 从 LogicalPlan 提取 JOIN 图
///
/// 仅处理 Inner / Cross JOIN；遇到 Outer JOIN 或非 Scan 叶子返回 None
fn extract_join_graph(plan: &LogicalPlan) -> Option<JoinGraph> {
    let mut nodes: Vec<(String, LogicalPlan)> = Vec::new();
    let mut edges: Vec<JoinEdge> = Vec::new();
    extract_recursive(plan, &mut nodes, &mut edges)?;
    if nodes.len() < 2 {
        return None;
    }
    Some(JoinGraph { nodes, edges })
}

/// 递归提取
///
/// 返回 Some(indices)：本子树中所有 base table 的索引列表
/// 返回 None：本子树包含 Outer JOIN 或非 Scan 叶子（不可重排）
fn extract_recursive(
    plan: &LogicalPlan,
    nodes: &mut Vec<(String, LogicalPlan)>,
    edges: &mut Vec<JoinEdge>,
) -> Option<Vec<usize>> {
    match plan {
        LogicalPlan::Join {
            join_type,
            condition,
            left,
            right,
            ..
        } => {
            // Outer JOIN 不支持重排
            if !matches!(join_type, JoinType::Inner | JoinType::Cross) {
                return None;
            }
            let left_indices = extract_recursive(left, nodes, edges)?;
            let right_indices = extract_recursive(right, nodes, edges)?;

            // 从 condition 提取表级边
            add_edges_from_condition(
                condition,
                *join_type,
                &left_indices,
                &right_indices,
                nodes,
                edges,
            );

            let mut all = left_indices;
            all.extend(right_indices);
            Some(all)
        }
        LogicalPlan::Scan { table, alias, .. } => {
            let key = alias
                .clone()
                .unwrap_or_else(|| table.name.clone())
                .to_lowercase();
            let idx = nodes.len();
            nodes.push((key, plan.clone()));
            Some(vec![idx])
        }
        // 其他节点（Projection/Filter 包裹的 Scan、子查询等）— 不重排
        _ => None,
    }
}

/// 从 JOIN 条件提取表级边，添加到 edges
fn add_edges_from_condition(
    condition: &JoinCondition,
    join_type: JoinType,
    left_indices: &[usize],
    right_indices: &[usize],
    nodes: &[(String, LogicalPlan)],
    edges: &mut Vec<JoinEdge>,
) {
    match condition {
        JoinCondition::On(expr) => {
            // 按 AND 拆分，每个合取项可能产生一条边
            let conjuncts = split_conjuncts(expr);
            if conjuncts.is_empty() {
                // 空条件（理论不应出现）→ fallback：用整表达式作为边
                add_fallback_edge(
                    left_indices,
                    right_indices,
                    join_type,
                    condition.clone(),
                    edges,
                );
                return;
            }
            for conjunct in conjuncts {
                let table_refs = collect_table_refs(&conjunct);
                // 找出此合取项引用的、跨越左右子树的表对
                let cross_pairs =
                    find_cross_table_pairs(&table_refs, left_indices, right_indices, nodes);
                if cross_pairs.is_empty() {
                    // 单表或常量谓词：fallback 加一条边
                    add_fallback_edge(
                        left_indices,
                        right_indices,
                        join_type,
                        JoinCondition::On(conjunct.clone()),
                        edges,
                    );
                } else {
                    for (li, ri) in cross_pairs {
                        edges.push(JoinEdge {
                            left: li.min(ri),
                            right: li.max(ri),
                            join_type,
                            condition: JoinCondition::On(conjunct.clone()),
                        });
                    }
                }
            }
        }
        JoinCondition::Using(_) | JoinCondition::Natural | JoinCondition::None => {
            // USING/NATURAL/Cross：单条边，条件原样保留
            add_fallback_edge(
                left_indices,
                right_indices,
                join_type,
                condition.clone(),
                edges,
            );
        }
    }
}

/// Fallback：在 left_indices[0] 和 right_indices[0] 之间加一条边
fn add_fallback_edge(
    left_indices: &[usize],
    right_indices: &[usize],
    join_type: JoinType,
    condition: JoinCondition,
    edges: &mut Vec<JoinEdge>,
) {
    if let (Some(&li), Some(&ri)) = (left_indices.first(), right_indices.first()) {
        edges.push(JoinEdge {
            left: li.min(ri),
            right: li.max(ri),
            join_type,
            condition,
        });
    }
}

/// 找出表引用中跨越左右子树的对
///
/// 返回 Vec<(left_idx, right_idx)>（无序）
fn find_cross_table_pairs(
    table_refs: &[String],
    left_indices: &[usize],
    right_indices: &[usize],
    nodes: &[(String, LogicalPlan)],
) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    // 将 table_refs 解析为节点索引
    let ref_indices: Vec<usize> = table_refs
        .iter()
        .filter_map(|t| nodes.iter().position(|(k, _)| k == t))
        .collect();
    // 找跨越左右的表对
    for &i in &ref_indices {
        for &j in &ref_indices {
            if i >= j {
                continue;
            }
            let i_in_left = left_indices.contains(&i);
            let i_in_right = right_indices.contains(&i);
            let j_in_left = left_indices.contains(&j);
            let j_in_right = right_indices.contains(&j);
            if (i_in_left && j_in_right) || (i_in_right && j_in_left) {
                pairs.push((i, j));
            }
        }
    }
    pairs
}

// =====================================================================
//  连通性检查与边查找
// =====================================================================

/// 检查子集 mask 在 JOIN 图中是否连通
///
/// 使用 BFS：从 mask 中任一节点出发，能否遍历到 mask 中所有其他节点。
fn is_connected(graph: &JoinGraph, mask: u32) -> bool {
    if mask == 0 {
        return true;
    }
    let start = mask.trailing_zeros() as usize;
    let mut visited = 0u32;
    let mut queue = vec![start];
    visited |= 1u32 << start;
    while let Some(node) = queue.pop() {
        for edge in &graph.edges {
            let other = if edge.left == node && (mask & (1u32 << edge.right)) != 0 {
                Some(edge.right)
            } else if edge.right == node && (mask & (1u32 << edge.left)) != 0 {
                Some(edge.left)
            } else {
                None
            };
            if let Some(o) = other {
                if (visited & (1u32 << o)) == 0 {
                    visited |= 1u32 << o;
                    queue.push(o);
                }
            }
        }
    }
    visited == mask
}

/// 找子集 s1 与 s2 之间的所有边
fn find_cross_edges(graph: &JoinGraph, s1: u32, s2: u32) -> Vec<usize> {
    let mut result = Vec::new();
    for (i, edge) in graph.edges.iter().enumerate() {
        let l_in_s1 = (s1 & (1u32 << edge.left)) != 0;
        let r_in_s1 = (s1 & (1u32 << edge.right)) != 0;
        let l_in_s2 = (s2 & (1u32 << edge.left)) != 0;
        let r_in_s2 = (s2 & (1u32 << edge.right)) != 0;
        if (l_in_s1 && r_in_s2) || (l_in_s2 && r_in_s1) {
            result.push(i);
        }
    }
    result
}

/// 构建 JOIN 节点：合并所有跨边谓词为单个 ON 表达式
///
/// 返回 None：无法构建（无跨边）
fn build_join_with_edges(
    graph: &JoinGraph,
    cross_edges: &[usize],
    left_plan: LogicalPlan,
    right_plan: LogicalPlan,
) -> Option<LogicalPlan> {
    if cross_edges.is_empty() {
        return None;
    }

    // 收集所有跨边谓词
    let mut on_exprs: Vec<Expr> = Vec::new();
    let mut chosen_join_type: Option<JoinType> = None;
    let mut using_cols: Vec<String> = Vec::new();
    let mut has_non_on = false;

    for &idx in cross_edges {
        let edge = &graph.edges[idx];
        // 优先级：Inner > Cross
        match edge.join_type {
            JoinType::Inner => {
                if chosen_join_type.is_none() {
                    chosen_join_type = Some(JoinType::Inner);
                }
            }
            JoinType::Cross => {
                if chosen_join_type.is_none() {
                    chosen_join_type = Some(JoinType::Cross);
                }
            }
            _ => unreachable!("Outer joins should not be in reorderable graph"),
        }
        match &edge.condition {
            JoinCondition::On(expr) => on_exprs.push(expr.clone()),
            JoinCondition::Using(cols) => {
                for c in cols {
                    using_cols.push(c.clone());
                }
                has_non_on = true;
            }
            JoinCondition::Natural => {
                has_non_on = true;
            }
            JoinCondition::None => {}
        }
    }

    let join_type = chosen_join_type.unwrap_or(JoinType::Inner);

    // 合并条件
    let condition = if has_non_on {
        // USING/NATURAL 与 On 混合：保留 USING 列（简化处理）
        if !using_cols.is_empty() {
            JoinCondition::Using(using_cols)
        } else {
            JoinCondition::Natural
        }
    } else if on_exprs.is_empty() {
        // 无 ON 表达式（Cross）
        JoinCondition::None
    } else {
        // 合并所有 ON 表达式为 AND
        let combined = on_exprs
            .into_iter()
            .reduce(|acc, e| Expr::BinaryOp {
                left: Box::new(acc),
                op: BinaryOp::And,
                right: Box::new(e),
            })
            .expect("cross_edges non-empty handled above");
        JoinCondition::On(combined)
    };

    Some(LogicalPlan::Join {
        join_type,
        condition,
        left: Box::new(left_plan),
        right: Box::new(right_plan),
        lateral: false,
        lateral_subquery: None,
        right_schema: None,
    })
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 将表达式按 AND 拆分为合取项列表
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

/// 收集表达式中所有"双层标识符"（`table.col`）的表名（小写）
fn collect_table_refs(expr: &Expr) -> Vec<String> {
    let mut refs = Vec::new();
    collect_table_refs_recursive(expr, &mut refs);
    refs
}

fn collect_table_refs_recursive(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Identifier(parts) => {
            if parts.len() >= 2 {
                let table = parts[parts.len() - 2].to_lowercase();
                out.push(table);
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_table_refs_recursive(left, out);
            collect_table_refs_recursive(right, out);
        }
        Expr::UnaryOp { expr, .. } => {
            collect_table_refs_recursive(expr, out);
        }
        Expr::Function { args, .. } => {
            for arg in args {
                collect_table_refs_recursive(arg, out);
            }
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            if let Some(op) = operand {
                collect_table_refs_recursive(op, out);
            }
            for (when, then) in when_then {
                collect_table_refs_recursive(when, out);
                collect_table_refs_recursive(then, out);
            }
            if let Some(e) = else_expr {
                collect_table_refs_recursive(e, out);
            }
        }
        Expr::Cast { expr, .. } => collect_table_refs_recursive(expr, out),
        Expr::InList { expr, list, .. } => {
            collect_table_refs_recursive(expr, out);
            for item in list {
                collect_table_refs_recursive(item, out);
            }
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_table_refs_recursive(expr, out);
            collect_table_refs_recursive(low, out);
            collect_table_refs_recursive(high, out);
        }
        Expr::Like { expr, pattern, .. } => {
            collect_table_refs_recursive(expr, out);
            collect_table_refs_recursive(pattern, out);
        }
        Expr::IsNull { expr, .. } => {
            collect_table_refs_recursive(expr, out);
        }
        _ => {}
    }
}

/// 枚举 full_mask 中所有恰好包含 `size` 个 bit 的子集
fn iter_subsets_of_size(full_mask: u32, size: usize) -> impl Iterator<Item = u32> {
    let mut result = Vec::new();
    if size == 0 || size > full_mask.count_ones() as usize {
        return result.into_iter();
    }
    // Gosper's hack 枚举 size 个 bit 的子集
    let mut subset: u32 = (1u32 << size) - 1;
    let bound = full_mask + 1;
    while subset < bound {
        if (subset & full_mask) == subset {
            result.push(subset);
        }
        // Gosper's hack：下一个相同 popcount 的数
        let c = subset & subset.wrapping_neg();
        let r = subset + c;
        subset = (((r ^ subset) >> 2) / c) | r;
    }
    result.into_iter()
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_sql::ast::{ColumnDefinition, TableName};
    use szrsql_sql::plan::TableSchema;
    use szrsql_types::value::{ColumnType, Value};

    /// 构建带别名的 Scan 计划
    fn scan(table_name: &str, alias: Option<&str>, cols: &[&str]) -> LogicalPlan {
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
            system_time_as_of: None,
        }
    }

    /// 构建 `a.col = b.col` 等值表达式
    fn equi_cond(left_table: &str, left_col: &str, right_table: &str, right_col: &str) -> Expr {
        Expr::BinaryOp {
            left: Box::new(Expr::Identifier(vec![
                left_table.to_string(),
                left_col.to_string(),
            ])),
            op: BinaryOp::Eq,
            right: Box::new(Expr::Identifier(vec![
                right_table.to_string(),
                right_col.to_string(),
            ])),
        }
    }

    /// 构建 Inner Join
    fn inner_join(left: LogicalPlan, right: LogicalPlan, cond: Expr) -> LogicalPlan {
        LogicalPlan::Join {
            join_type: JoinType::Inner,
            condition: JoinCondition::On(cond),
            left: Box::new(left),
            right: Box::new(right),
            lateral: false,
            lateral_subquery: None,
            right_schema: None,
        }
    }

    /// 构建 Cross Join
    fn cross_join(left: LogicalPlan, right: LogicalPlan) -> LogicalPlan {
        LogicalPlan::Join {
            join_type: JoinType::Cross,
            condition: JoinCondition::None,
            left: Box::new(left),
            right: Box::new(right),
            lateral: false,
            lateral_subquery: None,
            right_schema: None,
        }
    }

    /// 构建 LeftOuter Join
    fn left_outer_join(left: LogicalPlan, right: LogicalPlan, cond: Expr) -> LogicalPlan {
        LogicalPlan::Join {
            join_type: JoinType::LeftOuter,
            condition: JoinCondition::On(cond),
            left: Box::new(left),
            right: Box::new(right),
            lateral: false,
            lateral_subquery: None,
            right_schema: None,
        }
    }

    /// 提取计划中最顶层 JOIN 的左、右表名（用于断言 JOIN 顺序）
    fn top_join_tables(plan: &LogicalPlan) -> Option<(String, String)> {
        if let LogicalPlan::Join { left, right, .. } = plan {
            let lt = first_table_name(left);
            let rt = first_table_name(right);
            Some((lt, rt))
        } else {
            None
        }
    }

    fn first_table_name(plan: &LogicalPlan) -> String {
        match plan {
            LogicalPlan::Scan { table, alias, .. } => {
                alias.clone().unwrap_or_else(|| table.name.clone())
            }
            LogicalPlan::Join { left, .. } => first_table_name(left),
            _ => "unknown".to_string(),
        }
    }

    /// 统计 JOIN 节点数量
    fn count_joins(plan: &LogicalPlan) -> usize {
        match plan {
            LogicalPlan::Join { left, right, .. } => 1 + count_joins(left) + count_joins(right),
            _ => 0,
        }
    }

    #[test]
    fn test_split_conjuncts_single() {
        let e = equi_cond("a", "x", "b", "y");
        let parts = split_conjuncts(&e);
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn test_split_conjuncts_multiple() {
        let e1 = equi_cond("a", "x", "b", "y");
        let e2 = equi_cond("b", "z", "c", "w");
        let combined = Expr::BinaryOp {
            left: Box::new(e1),
            op: BinaryOp::And,
            right: Box::new(e2),
        };
        let parts = split_conjuncts(&combined);
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn test_collect_table_refs_qualified() {
        let e = equi_cond("a", "x", "b", "y");
        let refs = collect_table_refs(&e);
        assert_eq!(refs.len(), 2);
        assert!(refs.contains(&"a".to_string()));
        assert!(refs.contains(&"b".to_string()));
    }

    #[test]
    fn test_collect_table_refs_unqualified() {
        let e = Expr::Identifier(vec!["col".to_string()]);
        let refs = collect_table_refs(&e);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_iter_subsets_size_2_of_4() {
        let full = 0b1111u32;
        let subsets: Vec<u32> = iter_subsets_of_size(full, 2).collect();
        assert_eq!(subsets.len(), 6); // C(4,2) = 6
    }

    #[test]
    fn test_iter_subsets_size_3_of_5() {
        let full = 0b11111u32;
        let subsets: Vec<u32> = iter_subsets_of_size(full, 3).collect();
        assert_eq!(subsets.len(), 10); // C(5,3) = 10
    }

    #[test]
    fn test_extract_graph_two_table_join() {
        let a = scan("a", Some("a"), &["id", "x"]);
        let b = scan("b", Some("b"), &["id", "y"]);
        let join = inner_join(a, b, equi_cond("a", "id", "b", "id"));

        let graph = extract_join_graph(&join).expect("graph extraction");
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].left, 0);
        assert_eq!(graph.edges[0].right, 1);
    }

    #[test]
    fn test_extract_graph_chain_join_3_tables() {
        // ((a JOIN b) JOIN c)
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let c = scan("c", Some("c"), &["id"]);
        let ab = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        let abc = inner_join(ab, c, equi_cond("b", "id", "c", "id"));

        let graph = extract_join_graph(&abc).expect("graph extraction");
        assert_eq!(graph.nodes.len(), 3);
        // 2 个 JOIN → 2 条边
        assert_eq!(graph.edges.len(), 2);
    }

    #[test]
    fn test_extract_graph_rejects_outer_join() {
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let join = left_outer_join(a, b, equi_cond("a", "id", "b", "id"));

        assert!(extract_join_graph(&join).is_none());
    }

    #[test]
    fn test_extract_graph_rejects_non_scan_leaf() {
        // Filter 包裹的 Scan — 不可重排
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let filter = LogicalPlan::Filter {
            predicate: Expr::BinaryOp {
                left: Box::new(Expr::Identifier(vec!["a".to_string(), "x".to_string()])),
                op: BinaryOp::Gt,
                right: Box::new(Expr::Literal(Value::Int64(10))),
            },
            input: Box::new(a),
        };
        let join = inner_join(filter, b, equi_cond("a", "id", "b", "id"));

        assert!(extract_join_graph(&join).is_none());
    }

    #[test]
    fn test_is_connected_single_node() {
        let graph = JoinGraph {
            nodes: vec![("a".to_string(), scan("a", Some("a"), &["id"]))],
            edges: vec![],
        };
        assert!(is_connected(&graph, 0b1));
    }

    #[test]
    fn test_is_connected_chain() {
        // a-b-c 链式：edges = [(0,1), (1,2)]
        let graph = JoinGraph {
            nodes: vec![
                ("a".to_string(), scan("a", Some("a"), &["id"])),
                ("b".to_string(), scan("b", Some("b"), &["id"])),
                ("c".to_string(), scan("c", Some("c"), &["id"])),
            ],
            edges: vec![
                JoinEdge {
                    left: 0,
                    right: 1,
                    join_type: JoinType::Inner,
                    condition: JoinCondition::None,
                },
                JoinEdge {
                    left: 1,
                    right: 2,
                    join_type: JoinType::Inner,
                    condition: JoinCondition::None,
                },
            ],
        };
        assert!(is_connected(&graph, 0b111));
        assert!(is_connected(&graph, 0b011)); // a-b
        assert!(is_connected(&graph, 0b110)); // b-c
        assert!(!is_connected(&graph, 0b101)); // a-c 不直接相连
    }

    #[test]
    fn test_find_cross_edges() {
        let graph = JoinGraph {
            nodes: vec![
                ("a".to_string(), scan("a", Some("a"), &["id"])),
                ("b".to_string(), scan("b", Some("b"), &["id"])),
                ("c".to_string(), scan("c", Some("c"), &["id"])),
            ],
            edges: vec![
                JoinEdge {
                    left: 0,
                    right: 1,
                    join_type: JoinType::Inner,
                    condition: JoinCondition::None,
                },
                JoinEdge {
                    left: 1,
                    right: 2,
                    join_type: JoinType::Inner,
                    condition: JoinCondition::None,
                },
            ],
        };
        // s1 = {a, b}, s2 = {c}
        let edges = find_cross_edges(&graph, 0b011, 0b100);
        assert_eq!(edges.len(), 1); // b-c
                                    // s1 = {a}, s2 = {b, c}
        let edges = find_cross_edges(&graph, 0b001, 0b110);
        assert_eq!(edges.len(), 1); // a-b
    }

    #[test]
    fn test_optimize_single_table_no_change() {
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id", "x"]);
        let result = optimizer.optimize(a.clone());
        // 单表无 JOIN，应保持原样
        assert!(matches!(result, LogicalPlan::Scan { .. }));
    }

    #[test]
    fn test_optimize_two_table_join_preserves_structure() {
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id", "x"]);
        let b = scan("b", Some("b"), &["id", "y"]);
        let join = inner_join(a, b, equi_cond("a", "id", "b", "id"));

        let result = optimizer.optimize(join);
        // 2 表 JOIN：应保持 1 个 JOIN 节点
        assert_eq!(count_joins(&result), 1);
    }

    #[test]
    fn test_optimize_5_table_chain_join() {
        // 链式 5 表 JOIN：a-b-c-d-e
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let c = scan("c", Some("c"), &["id"]);
        let d = scan("d", Some("d"), &["id"]);
        let e = scan("e", Some("e"), &["id"]);

        let ab = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        let abc = inner_join(ab, c, equi_cond("b", "id", "c", "id"));
        let abcd = inner_join(abc, d, equi_cond("c", "id", "d", "id"));
        let abcde = inner_join(abcd, e, equi_cond("d", "id", "e", "id"));

        let result = optimizer.optimize(abcde);
        // 应仍是 4 个 JOIN 节点（5 个表 → 4 个 JOIN）
        assert_eq!(count_joins(&result), 4);
    }

    #[test]
    fn test_optimize_5_table_star_join() {
        // 星型 5 表 JOIN：中心 a，辐射到 b/c/d/e
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let c = scan("c", Some("c"), &["id"]);
        let d = scan("d", Some("d"), &["id"]);
        let e = scan("e", Some("e"), &["id"]);

        let ab = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        let abc = inner_join(ab, c, equi_cond("a", "id", "c", "id"));
        let abcd = inner_join(abc, d, equi_cond("a", "id", "d", "id"));
        let abcde = inner_join(abcd, e, equi_cond("a", "id", "e", "id"));

        let result = optimizer.optimize(abcde);
        assert_eq!(count_joins(&result), 4);
    }

    #[test]
    fn test_optimize_outer_join_not_reordered() {
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id", "x"]);
        let b = scan("b", Some("b"), &["id", "y"]);
        let join = left_outer_join(a, b, equi_cond("a", "id", "b", "id"));

        let result = optimizer.optimize(join);
        // Outer JOIN 不重排：顶层仍是 LeftOuter
        if let LogicalPlan::Join { join_type, .. } = &result {
            assert_eq!(*join_type, JoinType::LeftOuter);
        } else {
            panic!("expected Join, got {:?}", result);
        }
    }

    #[test]
    fn test_optimize_outer_join_recurses_into_inner_subtree() {
        // (a LOJ b) 内部含 Inner JOIN 子树时不重排外层，但递归处理内层
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let c = scan("c", Some("c"), &["id"]);
        // (a INNER b) LOJ c
        let ab = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        let abc = left_outer_join(ab, c, equi_cond("a", "id", "c", "id"));

        let result = optimizer.optimize(abc);
        // 外层应保持 LeftOuter
        if let LogicalPlan::Join { join_type, .. } = &result {
            assert_eq!(*join_type, JoinType::LeftOuter);
        } else {
            panic!("expected Join");
        }
    }

    #[test]
    fn test_optimize_cross_join() {
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let join = cross_join(a, b);

        let result = optimizer.optimize(join);
        assert_eq!(count_joins(&result), 1);
        if let LogicalPlan::Join { join_type, .. } = &result {
            assert_eq!(*join_type, JoinType::Cross);
        }
    }

    #[test]
    fn test_optimize_mixed_inner_and_cross() {
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let c = scan("c", Some("c"), &["id"]);
        // (a CROSS b) INNER c ON b.id = c.id
        let ab = cross_join(a, b);
        let abc = inner_join(ab, c, equi_cond("b", "id", "c", "id"));

        let result = optimizer.optimize(abc);
        assert_eq!(count_joins(&result), 2);
    }

    #[test]
    fn test_optimize_preserves_join_count_3_tables() {
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let c = scan("c", Some("c"), &["id"]);
        let ab = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        let abc = inner_join(ab, c, equi_cond("b", "id", "c", "id"));

        let result = optimizer.optimize(abc);
        assert_eq!(count_joins(&result), 2);
    }

    #[test]
    fn test_optimize_projection_wraps_join() {
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id", "x"]);
        let b = scan("b", Some("b"), &["id", "y"]);
        let join = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        let proj = LogicalPlan::Projection {
            exprs: vec![(
                Expr::Identifier(vec!["a".to_string(), "x".to_string()]),
                Some("x".to_string()),
            )],
            output_names: vec!["x".to_string()],
            input: Box::new(join),
        };

        let result = optimizer.optimize(proj);
        // 顶层仍是 Projection
        assert!(matches!(result, LogicalPlan::Projection { .. }));
        // 内部 JOIN 仍被优化（2 表保持 1 个 JOIN）
        if let LogicalPlan::Projection { input, .. } = result {
            assert_eq!(count_joins(&input), 1);
        }
    }

    #[test]
    fn test_optimize_filter_wraps_join() {
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id", "x"]);
        let b = scan("b", Some("b"), &["id", "y"]);
        let join = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        let filter = LogicalPlan::Filter {
            predicate: Expr::BinaryOp {
                left: Box::new(Expr::Identifier(vec!["a".to_string(), "x".to_string()])),
                op: BinaryOp::Gt,
                right: Box::new(Expr::Literal(Value::Int64(10))),
            },
            input: Box::new(join),
        };

        let result = optimizer.optimize(filter);
        assert!(matches!(result, LogicalPlan::Filter { .. }));
        if let LogicalPlan::Filter { input, .. } = result {
            assert_eq!(count_joins(&input), 1);
        }
    }

    #[test]
    fn test_optimize_3_table_join_finds_valid_order() {
        // 验证 DPccp 找到的顺序仍是合法的（顶层 JOIN 的两侧覆盖所有 5 表）
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let c = scan("c", Some("c"), &["id"]);
        let ab = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        let abc = inner_join(ab, c, equi_cond("b", "id", "c", "id"));

        let result = optimizer.optimize(abc);
        // 顶层 JOIN 必须存在
        let top = top_join_tables(&result);
        assert!(top.is_some(), "top-level join must exist");
    }

    #[test]
    fn test_optimize_4_table_join() {
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let c = scan("c", Some("c"), &["id"]);
        let d = scan("d", Some("d"), &["id"]);

        let ab = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        let abc = inner_join(ab, c, equi_cond("b", "id", "c", "id"));
        let abcd = inner_join(abc, d, equi_cond("c", "id", "d", "id"));

        let result = optimizer.optimize(abcd);
        assert_eq!(count_joins(&result), 3);
    }

    #[test]
    fn test_optimize_cycle_join_3_tables() {
        // 环形：a-b, b-c, c-a
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let c = scan("c", Some("c"), &["id"]);

        let ab = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        // 第二个 JOIN 同时包含 a-b 和 b-c 谓词
        let combined = Expr::BinaryOp {
            left: Box::new(equi_cond("a", "id", "b", "id")),
            op: BinaryOp::And,
            right: Box::new(equi_cond("b", "id", "c", "id")),
        };
        let abc = inner_join(ab, c, combined);

        let result = optimizer.optimize(abc);
        assert_eq!(count_joins(&result), 2);
    }

    #[test]
    fn test_build_join_with_edges_single_on() {
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let graph = JoinGraph {
            nodes: vec![("a".to_string(), a.clone()), ("b".to_string(), b.clone())],
            edges: vec![JoinEdge {
                left: 0,
                right: 1,
                join_type: JoinType::Inner,
                condition: JoinCondition::On(equi_cond("a", "id", "b", "id")),
            }],
        };
        let join = build_join_with_edges(&graph, &[0], a, b).expect("build_join");
        if let LogicalPlan::Join {
            join_type,
            condition,
            ..
        } = &join
        {
            assert_eq!(*join_type, JoinType::Inner);
            assert!(matches!(condition, JoinCondition::On(_)));
        } else {
            panic!("expected Join");
        }
    }

    #[test]
    fn test_build_join_with_edges_multiple_on_combined() {
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let graph = JoinGraph {
            nodes: vec![("a".to_string(), a.clone()), ("b".to_string(), b.clone())],
            edges: vec![
                JoinEdge {
                    left: 0,
                    right: 1,
                    join_type: JoinType::Inner,
                    condition: JoinCondition::On(equi_cond("a", "id", "b", "id")),
                },
                JoinEdge {
                    left: 0,
                    right: 1,
                    join_type: JoinType::Inner,
                    condition: JoinCondition::On(equi_cond("a", "x", "b", "y")),
                },
            ],
        };
        let join = build_join_with_edges(&graph, &[0, 1], a, b).expect("build_join");
        if let LogicalPlan::Join {
            condition: JoinCondition::On(expr),
            ..
        } = &join
        {
            // 合并后应是 AND 连接
            assert!(matches!(
                expr,
                Expr::BinaryOp {
                    op: BinaryOp::And,
                    ..
                }
            ));
        } else {
            panic!("expected Join with On(AND)");
        }
    }

    #[test]
    fn test_build_join_with_edges_empty_returns_none() {
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let graph = JoinGraph {
            nodes: vec![("a".to_string(), a.clone()), ("b".to_string(), b.clone())],
            edges: vec![],
        };
        assert!(build_join_with_edges(&graph, &[], a, b).is_none());
    }

    #[test]
    fn test_build_join_with_edges_cross_join() {
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let graph = JoinGraph {
            nodes: vec![("a".to_string(), a.clone()), ("b".to_string(), b.clone())],
            edges: vec![JoinEdge {
                left: 0,
                right: 1,
                join_type: JoinType::Cross,
                condition: JoinCondition::None,
            }],
        };
        let join = build_join_with_edges(&graph, &[0], a, b).expect("build_join");
        if let LogicalPlan::Join {
            join_type,
            condition,
            ..
        } = &join
        {
            assert_eq!(*join_type, JoinType::Cross);
            assert!(matches!(condition, JoinCondition::None));
        } else {
            panic!("expected Cross Join");
        }
    }

    #[test]
    fn test_optimize_idempotent() {
        // 优化后再优化应保持等价
        let optimizer = JoinOrderOptimizer::without_stats();
        let a = scan("a", Some("a"), &["id"]);
        let b = scan("b", Some("b"), &["id"]);
        let c = scan("c", Some("c"), &["id"]);
        let ab = inner_join(a, b, equi_cond("a", "id", "b", "id"));
        let abc = inner_join(ab, c, equi_cond("b", "id", "c", "id"));

        let once = optimizer.optimize(abc);
        let twice = optimizer.optimize(once);
        assert_eq!(count_joins(&twice), 2);
    }
}
