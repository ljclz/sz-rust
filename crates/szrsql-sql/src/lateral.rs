//! LATERAL 连接 — Phase 6.24
//!
//! 提供 PG 风格的 LATERAL 连接功能：
//!
//! - **CROSS JOIN LATERAL**：对左表每行求值子查询，产生笛卡尔积
//! - **INNER JOIN LATERAL**：带 ON 条件的 LATERAL 连接
//! - **LEFT JOIN LATERAL**：左表全保留，无匹配时右列填 NULL
//!
//! # 设计
//!
//! - **LateralSubquery trait**：子查询求值接口，接受左表行返回右表行集
//! - **ClosureSubquery**：闭包实现，便于测试和集成
//! - **LateralJoin**：连接执行器，对左表每行求值子查询并按连接类型组合
//! - **OnPredicate**：ON 条件谓词（可选），对组合行进行过滤
//!
//! # 与 PG 的关系
//!
//! - PG 9.3+ 支持 LATERAL
//! - LATERAL 允许子查询引用 FROM 子句中前序表的列（相关子查询）
//! - `CROSS JOIN LATERAL (sub) AS sub` — 子查询必须返回 ≥0 行
//! - `INNER JOIN LATERAL (sub) AS sub ON cond` — ON 条件过滤
//! - `LEFT JOIN LATERAL (sub) AS sub ON cond` — 左表全保留
//! - PG 常见用法：`SELECT * FROM t1, LATERAL (SELECT * FROM t2 WHERE t2.id=t1.id LIMIT 1) sub`
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：未集成到 SQL 解析路径，仅提供程序化 API
//! - **子查询为同步求值**：每行求值一次子查询（无批量优化）
//! - **无 JOIN 下推**：不支持将 JOIN 下推到子查询内部
//! - **ON 条件为简单谓词**：不支持复杂表达式（需调用方预处理）
//! - **无嵌套 LATERAL**：不支持多层 LATERAL 链（但可通过闭包嵌套实现）

use crate::executor::{ExecutionError, Row};
use szrsql_types::value::Value;

// =====================================================================
//  错误类型
// =====================================================================

/// LATERAL 连接错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LateralError {
    /// 子查询求值失败
    #[error("lateral subquery evaluation failed: {0}")]
    SubqueryFailed(String),
    /// 列数不匹配
    #[error("column count mismatch: expected {expected}, got {actual}")]
    ColumnCountMismatch { expected: usize, actual: usize },
    /// ON 条件求值失败
    #[error("ON predicate evaluation failed: {0}")]
    PredicateFailed(String),
    /// 无效参数
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl From<LateralError> for ExecutionError {
    fn from(e: LateralError) -> Self {
        ExecutionError::EvalError(format!("LATERAL error: {e}"))
    }
}

// =====================================================================
//  连接类型
// =====================================================================

/// LATERAL 连接类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LateralJoinType {
    /// `CROSS JOIN LATERAL` — 对左表每行求值子查询，产生笛卡尔积
    ///
    /// 子查询返回 0 行时，该左行不产生输出（与 CROSS JOIN 语义一致）。
    Cross,
    /// `INNER JOIN LATERAL ... ON cond` — 带条件的 LATERAL 连接
    ///
    /// 组合行需满足 ON 条件才输出。
    Inner,
    /// `LEFT JOIN LATERAL ... ON cond` — 左表全保留
    ///
    /// 子查询无匹配或 ON 条件不满足时，右列填 NULL。
    Left,
}

impl LateralJoinType {
    /// 是否要求 ON 条件
    pub fn requires_predicate(self) -> bool {
        matches!(self, Self::Inner | Self::Left)
    }
}

// =====================================================================
//  ON 条件谓词
// =====================================================================

/// ON 条件谓词 — 对组合行（左+右）进行过滤
///
/// 接受组合行引用，返回是否满足条件。
pub type OnPredicate = Box<dyn Fn(&Row) -> bool>;

// =====================================================================
//  LATERAL 子查询 trait
// =====================================================================

/// LATERAL 子查询求值接口
///
/// 对每个左表行求值子查询，返回匹配的右表行集。
/// 子查询可引用左表行的列（LATERAL 语义）。
pub trait LateralSubquery: Send {
    /// 对左表行求值子查询
    ///
    /// - `left_row` — 左表行（子查询可引用其列）
    ///
    /// 返回匹配的右表行集（可为空）。
    fn evaluate(&self, left_row: &Row) -> Result<Vec<Row>, LateralError>;

    /// 右表列数（用于 LEFT JOIN 生成 NULL 填充行）
    fn right_width(&self) -> usize;
}

// =====================================================================
//  ClosureSubquery — 闭包实现
// =====================================================================

/// 闭包实现的 LATERAL 子查询
///
/// 包装一个闭包 `Fn(&Row) -> Vec<Row>` 和右表列数。
pub struct ClosureSubquery {
    evaluator: LateralEvaluator,
    width: usize,
}

/// 子查询求值器闭包类型
pub type LateralEvaluator = Box<dyn Fn(&Row) -> Vec<Row> + Send>;

impl ClosureSubquery {
    /// 创建闭包子查询
    ///
    /// - `evaluator` — 求值闭包（接受左表行，返回右表行集）
    /// - `width` — 右表列数
    pub fn new<F>(evaluator: F, width: usize) -> Self
    where
        F: Fn(&Row) -> Vec<Row> + Send + 'static,
    {
        Self {
            evaluator: Box::new(evaluator),
            width,
        }
    }
}

impl LateralSubquery for ClosureSubquery {
    fn evaluate(&self, left_row: &Row) -> Result<Vec<Row>, LateralError> {
        let rows = (self.evaluator)(left_row);
        // 校验列数
        for row in &rows {
            if row.len() != self.width {
                return Err(LateralError::ColumnCountMismatch {
                    expected: self.width,
                    actual: row.len(),
                });
            }
        }
        Ok(rows)
    }

    fn right_width(&self) -> usize {
        self.width
    }
}

// =====================================================================
//  LateralJoin — 连接执行器
// =====================================================================

/// LATERAL 连接执行器
///
/// 对左表每行求值子查询，按连接类型组合结果。
///
/// # 用法
///
/// ```ignore
/// use szrsql_sql::lateral::*;
///
/// let left_rows = vec![vec![Value::Int64(1)], vec![Value::Int64(2)]];
/// let subquery = ClosureSubquery::new(|left| {
///     // 子查询引用左表行：SELECT * FROM t2 WHERE t2.id = left.id
///     let left_id = &left[0];
///     vec![vec![left_id.clone(), Value::Text("match".to_string())]]
/// }, 2);
///
/// let join = LateralJoin::new(left_rows, Box::new(subquery), LateralJoinType::Cross);
/// let result = join.execute(None).unwrap();
/// // result = [
/// //   [Int64(1), Int64(1), Text("match")],
/// //   [Int64(2), Int64(2), Text("match")],
/// // ]
/// ```
pub struct LateralJoin {
    left_rows: Vec<Row>,
    subquery: Box<dyn LateralSubquery>,
    join_type: LateralJoinType,
}

impl LateralJoin {
    /// 创建 LATERAL 连接
    ///
    /// - `left_rows` — 左表行集
    /// - `subquery` — LATERAL 子查询
    /// - `join_type` — 连接类型
    pub fn new(
        left_rows: Vec<Row>,
        subquery: Box<dyn LateralSubquery>,
        join_type: LateralJoinType,
    ) -> Self {
        Self {
            left_rows,
            subquery,
            join_type,
        }
    }

    /// 执行 LATERAL 连接
    ///
    /// - `on_predicate` — ON 条件谓词（Inner/Left 需要；Cross 可选，None 时不过滤）
    ///
    /// 返回组合行集（左列 + 右列）。
    pub fn execute(self, on_predicate: Option<OnPredicate>) -> Result<Vec<Row>, LateralError> {
        // 校验：Inner/Left 需要谓词
        if self.join_type.requires_predicate() && on_predicate.is_none() {
            return Err(LateralError::InvalidArgument(format!(
                "{:?} join requires an ON predicate",
                self.join_type
            )));
        }

        let right_width = self.subquery.right_width();
        let mut result: Vec<Row> = Vec::new();

        for left_row in &self.left_rows {
            let right_rows = self.subquery.evaluate(left_row)?;

            let mut matched = false;
            for right_row in right_rows {
                let mut combined = left_row.clone();
                combined.extend(right_row);

                // ON 条件过滤
                let passes = match &on_predicate {
                    Some(pred) => pred(&combined),
                    None => true,
                };

                if passes {
                    result.push(combined);
                    matched = true;
                }
            }

            // LEFT JOIN：无匹配时输出左行 + NULL 填充
            if self.join_type == LateralJoinType::Left && !matched {
                let mut combined = left_row.clone();
                combined.extend(std::iter::repeat_n(Value::Null, right_width));
                result.push(combined);
            }
        }

        Ok(result)
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建 NULL 填充行（用于 LEFT JOIN 无匹配时）
pub fn null_row(width: usize) -> Row {
    std::iter::repeat_n(Value::Null, width).collect()
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  测试辅助
    // -----------------------------------------------------------------

    /// 创建左表行：(id,)
    fn left_row(id: i64) -> Row {
        vec![Value::Int64(id)]
    }

    /// 创建右表行：(id, name)
    fn right_row(id: i64, name: &str) -> Row {
        vec![Value::Int64(id), Value::Text(name.to_string())]
    }

    /// 创建组合行：(left_id, right_id, name)
    fn combined_row(left_id: i64, right_id: i64, name: &str) -> Row {
        vec![
            Value::Int64(left_id),
            Value::Int64(right_id),
            Value::Text(name.to_string()),
        ]
    }

    /// 创建按 id 匹配的子查询（返回匹配的右表行）
    fn make_matching_subquery(right_data: Vec<Row>) -> ClosureSubquery {
        ClosureSubquery::new(
            move |left| {
                let left_id = &left[0];
                right_data
                    .iter()
                    .filter(|r| &r[0] == left_id)
                    .cloned()
                    .collect()
            },
            2,
        )
    }

    /// 创建 ON 谓词：combined[1]（right_id） > 0
    fn make_positive_right_id_predicate() -> OnPredicate {
        Box::new(|combined: &Row| match combined.get(1) {
            Some(Value::Int64(n)) => *n > 0,
            _ => false,
        })
    }

    // =================================================================
    //  LateralJoinType 测试
    // =================================================================

    #[test]
    fn test_join_type_requires_predicate() {
        assert!(!LateralJoinType::Cross.requires_predicate());
        assert!(LateralJoinType::Inner.requires_predicate());
        assert!(LateralJoinType::Left.requires_predicate());
    }

    // =================================================================
    //  ClosureSubquery 测试
    // =================================================================

    #[test]
    fn test_closure_subquery_evaluate() {
        let sub = ClosureSubquery::new(
            |left| {
                let id = &left[0];
                vec![vec![id.clone(), Value::Text("match".to_string())]]
            },
            2,
        );
        let result = sub.evaluate(&left_row(1)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0], Value::Int64(1));
        assert_eq!(result[0][1], Value::Text("match".to_string()));
    }

    #[test]
    fn test_closure_subquery_right_width() {
        let sub = ClosureSubquery::new(|_| vec![], 3);
        assert_eq!(sub.right_width(), 3);
    }

    #[test]
    fn test_closure_subquery_column_mismatch() {
        let sub = ClosureSubquery::new(|_| vec![vec![Value::Int64(1)]], 2);
        let err = sub.evaluate(&left_row(1)).unwrap_err();
        assert!(matches!(
            err,
            LateralError::ColumnCountMismatch {
                expected: 2,
                actual: 1
            }
        ));
    }

    #[test]
    fn test_closure_subquery_empty_result() {
        let sub = ClosureSubquery::new(|_| vec![], 2);
        let result = sub.evaluate(&left_row(1)).unwrap();
        assert!(result.is_empty());
    }

    // =================================================================
    //  CROSS JOIN LATERAL 测试
    // =================================================================

    #[test]
    fn test_cross_join_lateral_basic() {
        let left = vec![left_row(1), left_row(2)];
        let sub = make_matching_subquery(vec![right_row(1, "alice"), right_row(2, "bob")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Cross);
        let result = join.execute(None).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], combined_row(1, 1, "alice"));
        assert_eq!(result[1], combined_row(2, 2, "bob"));
    }

    #[test]
    fn test_cross_join_lateral_multiple_matches() {
        // 子查询返回多行 → 笛卡尔积
        let left = vec![left_row(1)];
        let sub = make_matching_subquery(vec![
            right_row(1, "alice"),
            right_row(1, "alice2"),
            right_row(2, "bob"), // 不匹配 left_id=1
        ]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Cross);
        let result = join.execute(None).unwrap();

        assert_eq!(result.len(), 2); // 两个 right_id=1 的行
        assert_eq!(result[0], combined_row(1, 1, "alice"));
        assert_eq!(result[1], combined_row(1, 1, "alice2"));
    }

    #[test]
    fn test_cross_join_lateral_no_match_skips_row() {
        // CROSS JOIN：子查询返回空 → 该左行不产生输出
        let left = vec![left_row(1), left_row(99)]; // 99 无匹配
        let sub = make_matching_subquery(vec![right_row(1, "alice")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Cross);
        let result = join.execute(None).unwrap();

        assert_eq!(result.len(), 1); // 只有 left_id=1 产生输出
        assert_eq!(result[0], combined_row(1, 1, "alice"));
    }

    #[test]
    fn test_cross_join_lateral_empty_left() {
        let left: Vec<Row> = vec![];
        let sub = make_matching_subquery(vec![right_row(1, "alice")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Cross);
        let result = join.execute(None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_cross_join_lateral_empty_right() {
        let left = vec![left_row(1)];
        let sub = make_matching_subquery(vec![]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Cross);
        let result = join.execute(None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_cross_join_lateral_with_predicate() {
        // CROSS JOIN with optional predicate (过滤)
        let left = vec![left_row(1), left_row(2)];
        let sub = make_matching_subquery(vec![right_row(1, "alice"), right_row(2, "bob")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Cross);
        // 谓词：只保留 right_id == 1
        let pred: OnPredicate =
            Box::new(|combined| matches!(combined.get(1), Some(Value::Int64(1))));
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], combined_row(1, 1, "alice"));
    }

    // =================================================================
    //  INNER JOIN LATERAL 测试
    // =================================================================

    #[test]
    fn test_inner_join_lateral_basic() {
        let left = vec![left_row(1), left_row(2)];
        let sub = make_matching_subquery(vec![right_row(1, "alice"), right_row(2, "bob")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Inner);
        // ON true
        let pred: OnPredicate = Box::new(|_| true);
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], combined_row(1, 1, "alice"));
        assert_eq!(result[1], combined_row(2, 2, "bob"));
    }

    #[test]
    fn test_inner_join_lateral_with_condition() {
        let left = vec![left_row(1), left_row(2)];
        let sub = make_matching_subquery(vec![right_row(1, "alice"), right_row(2, "bob")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Inner);
        // ON right_id > 1
        let pred: OnPredicate =
            Box::new(|combined| matches!(combined.get(1), Some(Value::Int64(n)) if *n > 1));
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], combined_row(2, 2, "bob"));
    }

    #[test]
    fn test_inner_join_lateral_no_match_skips_row() {
        // INNER JOIN：无匹配（子查询空或 ON 不满足）→ 不输出
        let left = vec![left_row(1), left_row(99)];
        let sub = make_matching_subquery(vec![right_row(1, "alice")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Inner);
        let pred: OnPredicate = Box::new(|_| true);
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], combined_row(1, 1, "alice"));
    }

    #[test]
    fn test_inner_join_lateral_requires_predicate() {
        let left = vec![left_row(1)];
        let sub = make_matching_subquery(vec![right_row(1, "alice")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Inner);
        let err = join.execute(None).unwrap_err();
        assert!(matches!(err, LateralError::InvalidArgument(_)));
    }

    #[test]
    fn test_inner_join_lateral_on_condition_filters_all() {
        // ON 条件过滤掉所有行
        let left = vec![left_row(1)];
        let sub = make_matching_subquery(vec![right_row(1, "alice")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Inner);
        let pred: OnPredicate = Box::new(|_| false);
        let result = join.execute(Some(pred)).unwrap();
        assert!(result.is_empty());
    }

    // =================================================================
    //  LEFT JOIN LATERAL 测试
    // =================================================================

    #[test]
    fn test_left_join_lateral_basic() {
        let left = vec![left_row(1), left_row(2)];
        let sub = make_matching_subquery(vec![right_row(1, "alice"), right_row(2, "bob")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Left);
        let pred: OnPredicate = Box::new(|_| true);
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], combined_row(1, 1, "alice"));
        assert_eq!(result[1], combined_row(2, 2, "bob"));
    }

    #[test]
    fn test_left_join_lateral_no_match_fills_null() {
        // LEFT JOIN：无匹配时输出左行 + NULL
        let left = vec![left_row(1), left_row(99)];
        let sub = make_matching_subquery(vec![right_row(1, "alice")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Left);
        let pred: OnPredicate = Box::new(|_| true);
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], combined_row(1, 1, "alice"));
        // left_id=99 无匹配 → [99, Null, Null]
        assert_eq!(result[1], vec![Value::Int64(99), Value::Null, Value::Null]);
    }

    #[test]
    fn test_left_join_lateral_all_no_match() {
        let left = vec![left_row(1), left_row(2)];
        let sub = make_matching_subquery(vec![right_row(99, "nobody")]); // 无匹配
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Left);
        let pred: OnPredicate = Box::new(|_| true);
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], vec![Value::Int64(1), Value::Null, Value::Null]);
        assert_eq!(result[1], vec![Value::Int64(2), Value::Null, Value::Null]);
    }

    #[test]
    fn test_left_join_lateral_on_condition_filters() {
        // LEFT JOIN with ON：ON 不满足时也填 NULL
        let left = vec![left_row(1)];
        let sub = make_matching_subquery(vec![right_row(1, "alice")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Left);
        // ON right_id > 99 → 永远不满足
        let pred: OnPredicate =
            Box::new(|combined| matches!(combined.get(1), Some(Value::Int64(n)) if *n > 99));
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 1);
        // ON 不满足 → NULL 填充
        assert_eq!(result[0], vec![Value::Int64(1), Value::Null, Value::Null]);
    }

    #[test]
    fn test_left_join_lateral_requires_predicate() {
        let left = vec![left_row(1)];
        let sub = make_matching_subquery(vec![right_row(1, "alice")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Left);
        let err = join.execute(None).unwrap_err();
        assert!(matches!(err, LateralError::InvalidArgument(_)));
    }

    #[test]
    fn test_left_join_lateral_empty_left() {
        let left: Vec<Row> = vec![];
        let sub = make_matching_subquery(vec![right_row(1, "alice")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Left);
        let pred: OnPredicate = Box::new(|_| true);
        let result = join.execute(Some(pred)).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_left_join_lateral_multiple_matches() {
        let left = vec![left_row(1)];
        let sub = make_matching_subquery(vec![right_row(1, "alice"), right_row(1, "alice2")]);
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Left);
        let pred: OnPredicate = Box::new(|_| true);
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 2); // 两个匹配
        assert_eq!(result[0], combined_row(1, 1, "alice"));
        assert_eq!(result[1], combined_row(1, 1, "alice2"));
    }

    // =================================================================
    //  LIMIT 1 场景测试（PG 常见用法）
    // =================================================================

    #[test]
    fn test_lateral_limit_1_pattern() {
        // 模拟：SELECT * FROM t1, LATERAL (SELECT * FROM t2 WHERE t2.id=t1.id LIMIT 1) sub
        let left = vec![left_row(1), left_row(2), left_row(3)];
        let right_data = [
            right_row(1, "first_1"),
            right_row(1, "second_1"), // LIMIT 1 会跳过
            right_row(2, "first_2"),
            // id=3 无匹配
        ];
        // 子查询模拟 LIMIT 1：只返回第一个匹配
        let sub = ClosureSubquery::new(
            move |left| {
                let left_id = &left[0];
                right_data
                    .iter()
                    .filter(|r| &r[0] == left_id)
                    .take(1) // LIMIT 1
                    .cloned()
                    .collect()
            },
            2,
        );
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Cross);
        let result = join.execute(None).unwrap();

        assert_eq!(result.len(), 2); // id=1 和 id=2 各一行，id=3 无匹配
        assert_eq!(result[0], combined_row(1, 1, "first_1"));
        assert_eq!(result[1], combined_row(2, 2, "first_2"));
    }

    #[test]
    fn test_lateral_limit_1_with_left_join() {
        // LEFT JOIN LATERAL + LIMIT 1：无匹配时填 NULL
        let left = vec![left_row(1), left_row(99)];
        let right_data = [right_row(1, "first_1")];
        let sub = ClosureSubquery::new(
            move |left| {
                let left_id = &left[0];
                right_data
                    .iter()
                    .filter(|r| &r[0] == left_id)
                    .take(1)
                    .cloned()
                    .collect()
            },
            2,
        );
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Left);
        let pred: OnPredicate = Box::new(|_| true);
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], combined_row(1, 1, "first_1"));
        assert_eq!(result[1], vec![Value::Int64(99), Value::Null, Value::Null]);
    }

    // =================================================================
    //  辅助函数测试
    // =================================================================

    #[test]
    fn test_null_row() {
        let row = null_row(3);
        assert_eq!(row.len(), 3);
        assert!(row.iter().all(|v| *v == Value::Null));
    }

    #[test]
    fn test_null_row_zero_width() {
        let row = null_row(0);
        assert!(row.is_empty());
    }

    // =================================================================
    //  错误转换测试
    // =================================================================

    #[test]
    fn test_lateral_error_to_execution_error() {
        let err = LateralError::SubqueryFailed("test".to_string());
        let exec_err: ExecutionError = err.into();
        match exec_err {
            ExecutionError::EvalError(msg) => {
                assert!(msg.contains("LATERAL error"));
                assert!(msg.contains("subquery evaluation failed"));
            }
            _ => panic!("expected EvalError"),
        }
    }

    #[test]
    fn test_column_count_mismatch_error() {
        let err = LateralError::ColumnCountMismatch {
            expected: 3,
            actual: 2,
        };
        let msg = format!("{err}");
        assert!(msg.contains("expected 3"));
        assert!(msg.contains("got 2"));
    }

    // =================================================================
    //  端到端场景测试
    // =================================================================

    #[test]
    fn test_e2e_correlated_subquery() {
        // 模拟 PG: SELECT * FROM orders, LATERAL (SELECT * FROM customers WHERE customers.id = orders.cust_id) c
        let orders = vec![
            // order_id, cust_id
            vec![Value::Int64(101), Value::Int64(1)],
            vec![Value::Int64(102), Value::Int64(2)],
            vec![Value::Int64(103), Value::Int64(99)], // 无匹配客户
        ];
        let customers = [
            // cust_id, name
            vec![Value::Int64(1), Value::Text("Alice".to_string())],
            vec![Value::Int64(2), Value::Text("Bob".to_string())],
        ];

        // 子查询引用 orders.cust_id（left[1]）
        let sub = ClosureSubquery::new(
            move |left| {
                let cust_id = &left[1];
                customers
                    .iter()
                    .filter(|c| &c[0] == cust_id)
                    .cloned()
                    .collect()
            },
            2,
        );

        // CROSS JOIN LATERAL
        let join = LateralJoin::new(orders, Box::new(sub), LateralJoinType::Cross);
        let result = join.execute(None).unwrap();

        assert_eq!(result.len(), 2); // order 103 无匹配
                                     // [order_id, cust_id, cust_id, name]
        assert_eq!(
            result[0],
            vec![
                Value::Int64(101),
                Value::Int64(1),
                Value::Int64(1),
                Value::Text("Alice".to_string())
            ]
        );
        assert_eq!(
            result[1],
            vec![
                Value::Int64(102),
                Value::Int64(2),
                Value::Int64(2),
                Value::Text("Bob".to_string())
            ]
        );
    }

    #[test]
    fn test_e2e_left_join_preserves_all_left() {
        // LEFT JOIN LATERAL：所有左行保留
        let orders = vec![
            vec![Value::Int64(101), Value::Int64(1)],
            vec![Value::Int64(102), Value::Int64(2)],
            vec![Value::Int64(103), Value::Int64(99)],
        ];
        let customers = [
            vec![Value::Int64(1), Value::Text("Alice".to_string())],
            vec![Value::Int64(2), Value::Text("Bob".to_string())],
        ];

        let sub = ClosureSubquery::new(
            move |left| {
                let cust_id = &left[1];
                customers
                    .iter()
                    .filter(|c| &c[0] == cust_id)
                    .cloned()
                    .collect()
            },
            2,
        );

        let join = LateralJoin::new(orders, Box::new(sub), LateralJoinType::Left);
        let pred: OnPredicate = Box::new(|_| true);
        let result = join.execute(Some(pred)).unwrap();

        assert_eq!(result.len(), 3); // 全部保留
                                     // order 103 无匹配 → NULL 填充
        assert_eq!(
            result[2],
            vec![
                Value::Int64(103),
                Value::Int64(99),
                Value::Null,
                Value::Null
            ]
        );
    }

    #[test]
    fn test_e2e_aggregate_in_subquery() {
        // 模拟：SELECT * FROM dept, LATERAL (SELECT COUNT(*) AS emp_count FROM emp WHERE emp.dept_id = dept.id) s
        let depts = vec![
            vec![Value::Int64(1), Value::Text("Eng".to_string())],
            vec![Value::Int64(2), Value::Text("Sales".to_string())],
        ];
        let emps = [
            vec![Value::Int64(1)],
            vec![Value::Int64(1)],
            vec![Value::Int64(1)],
            vec![Value::Int64(2)],
            vec![Value::Int64(2)],
        ];

        // 子查询返回聚合结果 (emp_count,)
        let sub = ClosureSubquery::new(
            move |left| {
                let dept_id = &left[0];
                let count = emps.iter().filter(|e| &e[0] == dept_id).count() as i64;
                vec![vec![Value::Int64(count)]]
            },
            1,
        );

        let join = LateralJoin::new(depts, Box::new(sub), LateralJoinType::Cross);
        let result = join.execute(None).unwrap();

        assert_eq!(result.len(), 2);
        // [dept_id, dept_name, emp_count]
        assert_eq!(
            result[0],
            vec![
                Value::Int64(1),
                Value::Text("Eng".to_string()),
                Value::Int64(3)
            ]
        );
        assert_eq!(
            result[1],
            vec![
                Value::Int64(2),
                Value::Text("Sales".to_string()),
                Value::Int64(2)
            ]
        );
    }

    #[test]
    fn test_e2e_cross_join_with_empty_subquery_predicate() {
        // CROSS JOIN 带谓词，子查询返回空
        let left = vec![left_row(1), left_row(2)];
        let sub = make_matching_subquery(vec![]); // 空
        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Cross);
        let pred = make_positive_right_id_predicate();
        let result = join.execute(Some(pred)).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_e2e_subquery_references_multiple_left_columns() {
        // 子查询引用左表多个列
        // 模拟：SELECT * FROM t1, LATERAL (SELECT * FROM t2 WHERE t2.a = t1.a AND t2.b > t1.b) s
        let left = vec![
            // a, b
            vec![Value::Int64(1), Value::Int64(10)],
            vec![Value::Int64(2), Value::Int64(20)],
        ];
        let right_data = [
            // a, c
            vec![Value::Int64(1), Value::Int64(100)],
            vec![Value::Int64(1), Value::Int64(5)], // c < left.b → 被过滤
            vec![Value::Int64(2), Value::Int64(200)],
            vec![Value::Int64(2), Value::Int64(15)], // c < left.b → 被过滤
        ];

        let sub = ClosureSubquery::new(
            move |left| {
                let left_a = &left[0];
                let left_b = &left[1];
                right_data
                    .iter()
                    .filter(|r| {
                        r[0] == *left_a // t2.a = t1.a
                            && match (&r[1], left_b) {
                                (Value::Int64(c), Value::Int64(b)) => c > b,
                                _ => false,
                            } // t2.c > t1.b
                    })
                    .cloned()
                    .collect()
            },
            2,
        );

        let join = LateralJoin::new(left, Box::new(sub), LateralJoinType::Cross);
        let result = join.execute(None).unwrap();

        assert_eq!(result.len(), 2);
        // [a, b, a, c]
        assert_eq!(
            result[0],
            vec![
                Value::Int64(1),
                Value::Int64(10),
                Value::Int64(1),
                Value::Int64(100)
            ]
        );
        assert_eq!(
            result[1],
            vec![
                Value::Int64(2),
                Value::Int64(20),
                Value::Int64(2),
                Value::Int64(200)
            ]
        );
    }
}
