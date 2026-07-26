//! GROUPING SETS / CUBE / ROLLUP — Phase 6.25
//!
//! 提供 PG 风格的多组聚合（Multi-Group Aggregation）功能：
//!
//! - **GROUPING SETS**：显式指定多组聚合（`GROUP BY GROUPING SETS ((a, b), (a), ())`）
//! - **CUBE**：所有子集组合（`GROUP BY CUBE (a, b)` → 4 组聚合）
//! - **ROLLUP**：层级聚合（`GROUP BY ROLLUP (a, b)` → 3 组聚合）
//! - **GROUPING / GROUPING_ID**：标识列是否被聚合（PG 兼容）
//!
//! # 设计
//!
//! - **GroupingSet 枚举**：描述分组集规范（Simple/Rollup/Cube/GroupingSets）
//! - **expand()**：将规范展开为简单分组列表 `Vec<Vec<usize>>`
//! - **aggregate_grouping_sets()**：对每个分组独立聚合，UNION 结果
//! - **辅助聚合函数**：count_star / sum_int64 / count_col / min_value / max_value
//! - **GROUPING / GROUPING_ID**：返回位掩码标识列的聚合状态
//!
//! # 与 PG 的关系
//!
//! - PG 8.4+ 支持 GROUPING SETS / CUBE / ROLLUP
//! - `ROLLUP (a, b, c)` ≡ `GROUPING SETS ((a, b, c), (a, b), (a), ())`
//! - `CUBE (a, b)` ≡ `GROUPING SETS ((a, b), (a), (b), ())`
//! - `GROUPING(col)` 返回 1（列被聚合/填 NULL）或 0（列参与分组）
//! - `GROUPING_ID(col1, col2, ...)` 返回位掩码（高位在前，PG 语义）
//! - 不在当前分组中的列在输出中填充 NULL
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：未集成到 SQL 解析路径，仅提供程序化 API
//! - **无自动去重**：不同分组集可能产生相同输出行（PG 也不去重，除非用 DISTINCT）
//! - **无 HAVING 过滤**：调用方需自行过滤结果
//! - **无 ORDER BY**：输出顺序按分组集展开顺序，再按分组键哈希序（非确定性）
//! - **聚合函数有限**：仅提供常用辅助函数，调用方可传入任意 `Fn(&[Row]) -> Vec<Value>`

use crate::executor::{ExecutionError, Row};
use std::collections::HashMap;
use szrsql_types::value::Value;

/// 聚合函数闭包类型
pub type AggFn = Box<dyn Fn(&[Row]) -> Vec<Value> + Send>;

// =====================================================================
//  错误类型
// =====================================================================

/// 分组集错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GroupingSetsError {
    /// 分组列为空
    #[error("group column list is empty")]
    EmptyGroupColumns,
    /// 列索引越界
    #[error("column index out of range: {0}")]
    ColumnIndexOutOfRange(usize),
    /// 分组集规范引用了不在 group_col_indices 中的列
    #[error("grouping set references column {0} not in group_col_indices")]
    ColumnNotInGroup(usize),
    /// 空分组集（GroupingSets 变体为空 Vec）
    #[error("grouping sets specification is empty")]
    EmptyGroupingSet,
}

impl From<GroupingSetsError> for ExecutionError {
    fn from(e: GroupingSetsError) -> Self {
        ExecutionError::EvalError(format!("GROUPING SETS error: {e}"))
    }
}

// =====================================================================
//  GroupingSet — 分组集规范
// =====================================================================

/// 分组集规范
///
/// 描述多组聚合的分组方式。列索引引用输入行的列位置。
///
/// # PG 一致性
///
/// - `Simple(cols)` ≡ `GROUP BY cols`
/// - `Rollup(cols)` ≡ `GROUP BY ROLLUP(cols)` — 层级聚合
/// - `Cube(cols)` ≡ `GROUP BY CUBE(cols)` — 所有子集
/// - `GroupingSets(sets)` ≡ `GROUP BY GROUPING SETS(sets)` — 显式指定
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupingSet {
    /// 简单分组（等价于普通 GROUP BY）
    Simple(Vec<usize>),
    /// ROLLUP 层级聚合
    ///
    /// `Rollup([a, b, c])` → `[(a,b,c), (a,b), (a), ()]`
    Rollup(Vec<usize>),
    /// CUBE 所有子集
    ///
    /// `Cube([a, b])` → `[(a,b), (a), (b), ()]`
    Cube(Vec<usize>),
    /// 显式分组集
    ///
    /// `GroupingSets([(a,b), (a), ()])` → 3 组聚合
    GroupingSets(Vec<Vec<usize>>),
}

impl GroupingSet {
    /// 展开为简单分组列表
    ///
    /// 返回 `Vec<Vec<usize>>`，每个内层 Vec 是一组列索引（可为空 = 总计）。
    ///
    /// # 展开顺序
    ///
    /// - `Simple` → 单元素列表
    /// - `Rollup` → 从最具体到最宽泛（`(a,b,c) → (a,b) → (a) → ()`）
    /// - `Cube` → 从全集到空集，按二进制位反转序（高位优先）
    /// - `GroupingSets` → 原序
    pub fn expand(&self) -> Vec<Vec<usize>> {
        match self {
            Self::Simple(cols) => vec![cols.clone()],
            Self::Rollup(cols) => {
                // ROLLUP(a, b, c) → (a,b,c), (a,b), (a), ()
                let mut result = Vec::with_capacity(cols.len() + 1);
                for end in (0..=cols.len()).rev() {
                    result.push(cols[..end].to_vec());
                }
                result
            }
            Self::Cube(cols) => {
                // CUBE(a, b) → (a,b), (a), (b), () — 按二进制掩码降序（高位对应靠前列）
                let n = cols.len();
                let mut result = Vec::with_capacity(1usize << n);
                for mask in (0..(1u32 << n)).rev() {
                    let subset: Vec<usize> = (0..n)
                        .filter(|i| (mask >> (n - 1 - i)) & 1 == 1)
                        .map(|i| cols[i])
                        .collect();
                    result.push(subset);
                }
                result
            }
            Self::GroupingSets(sets) => sets.clone(),
        }
    }
}

// =====================================================================
//  多组聚合
// =====================================================================

/// 对输入行执行多组聚合
///
/// # 参数
///
/// - `rows` — 输入行
/// - `group_set` — 分组集规范
/// - `group_col_indices` — 分组列在输入行中的索引（决定输出顺序；必须包含 group_set 引用的所有列）
/// - `agg_fn` — 聚合函数，接受一组行返回聚合值（多个聚合返回多个值）
///
/// # 返回
///
/// 每行格式：`[group_col_1, group_col_2, ..., agg_1, agg_2, ...]`
/// 不在当前分组中的列填充 `Value::Null`。
///
/// # PG 语义
///
/// - 对每个展开的分组，按分组键分区输入行
/// - 每个分区计算聚合，输出一行
/// - 空输入行 → 空输出（无分区）
/// - 同一分组集的不同分区产生不同输出行
///
/// # 错误
///
/// - `EmptyGroupColumns` — group_col_indices 为空
/// - `EmptyGroupingSet` — GroupingSets 变体为空 Vec
/// - `ColumnNotInGroup` — group_set 引用的列不在 group_col_indices 中
/// - `ColumnIndexOutOfRange` — 列索引超过输入行宽度
pub fn aggregate_grouping_sets(
    rows: &[Row],
    group_set: &GroupingSet,
    group_col_indices: &[usize],
    agg_fn: &dyn Fn(&[Row]) -> Vec<Value>,
) -> Result<Vec<Row>, GroupingSetsError> {
    // 校验
    if group_col_indices.is_empty() {
        return Err(GroupingSetsError::EmptyGroupColumns);
    }
    if let GroupingSet::GroupingSets(sets) = group_set {
        if sets.is_empty() {
            return Err(GroupingSetsError::EmptyGroupingSet);
        }
    }

    // 校验列索引越界（基于第一行宽度；空行时跳过）
    if let Some(first) = rows.first() {
        let width = first.len();
        for &idx in group_col_indices {
            if idx >= width {
                return Err(GroupingSetsError::ColumnIndexOutOfRange(idx));
            }
        }
    }

    // 校验 group_set 引用的列都在 group_col_indices 中
    for group in group_set.expand() {
        for &col in &group {
            if !group_col_indices.contains(&col) {
                return Err(GroupingSetsError::ColumnNotInGroup(col));
            }
        }
    }

    let groups = group_set.expand();
    let mut result: Vec<Row> = Vec::new();

    for group in &groups {
        // 按 group 键分区（Value 未实现 Hash/Eq，使用字符串键作为 HashMap 键）
        let mut partitions: HashMap<String, (Vec<Value>, Vec<Row>)> = HashMap::new();
        for row in rows {
            let key: Vec<Value> = group.iter().map(|&i| row[i].clone()).collect();
            let hash_key = make_partition_key(&key);
            partitions
                .entry(hash_key)
                .or_insert_with(|| (key.clone(), Vec::new()))
                .1
                .push(row.clone());
        }

        // 对每个分区计算聚合
        for (key, partition_rows) in partitions.into_values() {
            let agg_values = agg_fn(&partition_rows);
            let mut out_row: Row = Vec::with_capacity(group_col_indices.len() + agg_values.len());
            // 填充分组列：在当前 group 中的列取值，否则 NULL
            for &col_idx in group_col_indices {
                if let Some(pos) = group.iter().position(|&c| c == col_idx) {
                    out_row.push(key[pos].clone());
                } else {
                    out_row.push(Value::Null);
                }
            }
            out_row.extend(agg_values);
            result.push(out_row);
        }
    }

    Ok(result)
}

/// 生成分区键的哈希字符串（Value 未实现 Hash，使用 Debug 表示）
fn make_partition_key(values: &[Value]) -> String {
    values
        .iter()
        .map(|v| format!("{v:?}"))
        .collect::<Vec<_>>()
        .join("\x00")
}

// =====================================================================
//  GROUPING / GROUPING_ID 函数
// =====================================================================

/// GROUPING 函数 — 返回单列是否被聚合
///
/// PG 语义：`GROUPING(col)` 返回 1（列被聚合/填 NULL）或 0（列参与分组）。
///
/// # 参数
///
/// - `col_idx` — 要检查的列在 group_col_indices 中的位置
/// - `current_group` — 当前分组（group_set.expand() 的某一项）
/// - `group_col_indices` — 全部分组列
///
/// # 返回
///
/// `Value::Int64(1)` 或 `Value::Int64(0)`
pub fn grouping(col_idx: usize, current_group: &[usize], group_col_indices: &[usize]) -> Value {
    // col_idx 是 group_col_indices 中的位置
    if col_idx >= group_col_indices.len() {
        return Value::Int64(1);
    }
    let actual_col = group_col_indices[col_idx];
    if current_group.contains(&actual_col) {
        Value::Int64(0)
    } else {
        Value::Int64(1)
    }
}

/// GROUPING_ID 函数 — 返回多列的聚合位掩码
///
/// PG 语义：`GROUPING_ID(col1, col2, ...)` 返回位掩码，高位在前。
/// 例如 `GROUPING_ID(a, b)`：
/// - (a, b) 都在分组中 → 0 (00)
/// - a 在，b 不在 → 1 (01)
/// - a 不在，b 在 → 2 (10)
/// - 都不在 → 3 (11)
///
/// # 参数
///
/// - `col_indices` — 要检查的列在 group_col_indices 中的位置列表
/// - `current_group` — 当前分组
/// - `group_col_indices` — 全部分组列
pub fn grouping_id(
    col_indices: &[usize],
    current_group: &[usize],
    group_col_indices: &[usize],
) -> Value {
    let mut mask: i64 = 0;
    for &pos in col_indices {
        mask <<= 1;
        if pos < group_col_indices.len() {
            let actual_col = group_col_indices[pos];
            if !current_group.contains(&actual_col) {
                mask |= 1;
            }
        } else {
            mask |= 1;
        }
    }
    Value::Int64(mask)
}

// =====================================================================
//  辅助聚合函数
// =====================================================================

/// COUNT(*) — 计算行数
pub fn agg_count_star() -> AggFn {
    Box::new(|rows: &[Row]| vec![Value::Int64(rows.len() as i64)])
}

/// SUM(col_idx) — 对 Int64 列求和（空组返回 NULL）
pub fn agg_sum_int64(col_idx: usize) -> AggFn {
    Box::new(move |rows: &[Row]| {
        let sum: Option<i64> = rows
            .iter()
            .filter_map(|r| match r.get(col_idx) {
                Some(Value::Int64(v)) => Some(*v),
                _ => None,
            })
            .reduce(|a, b| a + b);
        vec![sum.map_or(Value::Null, Value::Int64)]
    })
}

/// COUNT(col_idx) — 计算非 NULL 值的数量
pub fn agg_count_col(col_idx: usize) -> AggFn {
    Box::new(move |rows: &[Row]| {
        let count = rows
            .iter()
            .filter(|r| !matches!(r.get(col_idx), Some(Value::Null) | None))
            .count();
        vec![Value::Int64(count as i64)]
    })
}

/// MIN(col_idx) — 最小值（使用 value_compare；空组返回 NULL）
pub fn agg_min_value(col_idx: usize) -> AggFn {
    Box::new(move |rows: &[Row]| {
        let values: Vec<&Value> = rows
            .iter()
            .filter_map(|r| match r.get(col_idx) {
                Some(Value::Null) | None => None,
                Some(v) => Some(v),
            })
            .collect();
        if values.is_empty() {
            return vec![Value::Null];
        }
        let mut min = values[0];
        for v in &values[1..] {
            if value_compare(v, min) == std::cmp::Ordering::Less {
                min = v;
            }
        }
        vec![min.clone()]
    })
}

/// MAX(col_idx) — 最大值（使用 value_compare；空组返回 NULL）
pub fn agg_max_value(col_idx: usize) -> AggFn {
    Box::new(move |rows: &[Row]| {
        let values: Vec<&Value> = rows
            .iter()
            .filter_map(|r| match r.get(col_idx) {
                Some(Value::Null) | None => None,
                Some(v) => Some(v),
            })
            .collect();
        if values.is_empty() {
            return vec![Value::Null];
        }
        let mut max = values[0];
        for v in &values[1..] {
            if value_compare(v, max) == std::cmp::Ordering::Greater {
                max = v;
            }
        }
        vec![max.clone()]
    })
}

// =====================================================================
//  类型感知的 Value 比较（与 fdw.rs 一致）
// =====================================================================

/// 类型感知的 Value 比较 — 返回 Ordering
///
/// `Value` 未实现 `PartialOrd`，故在此提供本地比较函数。
/// 仅支持常用类型，跨类型比较按 Int64↔Float64 隐式转换，其余按 Debug 字符串排序。
fn value_compare(a: &Value, b: &Value) -> std::cmp::Ordering {
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
        _ => format!("{a:?}").cmp(&format!("{b:?}")),
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    //  辅助函数
    // -----------------------------------------------------------------

    fn make_row_int(vals: &[i64]) -> Row {
        vals.iter().map(|v| Value::Int64(*v)).collect()
    }

    fn make_row_int_text(int_val: i64, text_val: &str) -> Row {
        vec![Value::Int64(int_val), Value::Text(text_val.to_string())]
    }

    fn make_row_int_int_int(a: i64, b: i64, c: i64) -> Row {
        vec![Value::Int64(a), Value::Int64(b), Value::Int64(c)]
    }

    // -----------------------------------------------------------------
    //  GroupingSet::expand
    // -----------------------------------------------------------------

    #[test]
    fn test_expand_simple() {
        let gs = GroupingSet::Simple(vec![0, 1]);
        assert_eq!(gs.expand(), vec![vec![0, 1]]);
    }

    #[test]
    fn test_expand_simple_empty() {
        // Simple(vec![]) → grand total
        let gs = GroupingSet::Simple(vec![]);
        assert_eq!(gs.expand(), vec![Vec::<usize>::new()]);
    }

    #[test]
    fn test_expand_rollup_2_cols() {
        // ROLLUP(a, b) → (a,b), (a), ()
        let gs = GroupingSet::Rollup(vec![0, 1]);
        assert_eq!(gs.expand(), vec![vec![0, 1], vec![0], vec![]]);
    }

    #[test]
    fn test_expand_rollup_3_cols() {
        // ROLLUP(a, b, c) → (a,b,c), (a,b), (a), ()
        let gs = GroupingSet::Rollup(vec![0, 1, 2]);
        assert_eq!(
            gs.expand(),
            vec![vec![0, 1, 2], vec![0, 1], vec![0], vec![]]
        );
    }

    #[test]
    fn test_expand_rollup_1_col() {
        // ROLLUP(a) → (a), ()
        let gs = GroupingSet::Rollup(vec![0]);
        assert_eq!(gs.expand(), vec![vec![0], vec![]]);
    }

    #[test]
    fn test_expand_cube_2_cols() {
        // CUBE(a, b) → (a,b), (a), (b), ()
        let gs = GroupingSet::Cube(vec![0, 1]);
        assert_eq!(gs.expand(), vec![vec![0, 1], vec![0], vec![1], vec![]]);
    }

    #[test]
    fn test_expand_cube_1_col() {
        // CUBE(a) → (a), ()
        let gs = GroupingSet::Cube(vec![0]);
        assert_eq!(gs.expand(), vec![vec![0], vec![]]);
    }

    #[test]
    fn test_expand_cube_3_cols_count() {
        // CUBE(a, b, c) → 2^3 = 8 组
        let gs = GroupingSet::Cube(vec![0, 1, 2]);
        assert_eq!(gs.expand().len(), 8);
    }

    #[test]
    fn test_expand_grouping_sets() {
        let gs = GroupingSet::GroupingSets(vec![vec![0, 1], vec![0], vec![]]);
        assert_eq!(gs.expand(), vec![vec![0, 1], vec![0], vec![]]);
    }

    #[test]
    fn test_expand_grouping_sets_preserves_order() {
        // 顺序应与输入一致
        let gs = GroupingSet::GroupingSets(vec![vec![], vec![0], vec![0, 1]]);
        assert_eq!(gs.expand(), vec![vec![], vec![0], vec![0, 1]]);
    }

    // -----------------------------------------------------------------
    //  aggregate_grouping_sets — Simple
    // -----------------------------------------------------------------

    #[test]
    fn test_aggregate_simple_basic() {
        // SELECT a, COUNT(*) FROM t GROUP BY a
        let rows = vec![
            make_row_int(&[1, 10]),
            make_row_int(&[1, 20]),
            make_row_int(&[2, 30]),
        ];
        let gs = GroupingSet::Simple(vec![0]);
        let result =
            aggregate_grouping_sets(&rows, &gs, &[0], &|r| vec![Value::Int64(r.len() as i64)])
                .unwrap();

        assert_eq!(result.len(), 2); // 两个分组：a=1, a=2
                                     // 验证包含 (1, 2) 和 (2, 1)
        let has_1_2 = result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(2)]);
        let has_2_1 = result
            .iter()
            .any(|r| r == &vec![Value::Int64(2), Value::Int64(1)]);
        assert!(has_1_2, "should contain (1, 2): {:?}", result);
        assert!(has_2_1, "should contain (2, 1): {:?}", result);
    }

    #[test]
    fn test_aggregate_simple_with_sum() {
        // SELECT a, SUM(b) FROM t GROUP BY a
        let rows = vec![
            make_row_int(&[1, 10]),
            make_row_int(&[1, 20]),
            make_row_int(&[2, 30]),
        ];
        let gs = GroupingSet::Simple(vec![0]);
        let sum_fn = agg_sum_int64(1);
        let result = aggregate_grouping_sets(&rows, &gs, &[0], &sum_fn).unwrap();

        assert_eq!(result.len(), 2);
        let has_1_30 = result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(30)]);
        let has_2_30 = result
            .iter()
            .any(|r| r == &vec![Value::Int64(2), Value::Int64(30)]);
        assert!(has_1_30, "should contain (1, 30): {:?}", result);
        assert!(has_2_30, "should contain (2, 30): {:?}", result);
    }

    #[test]
    fn test_aggregate_simple_empty_input() {
        // 空输入 → 空输出
        let rows: Vec<Row> = vec![];
        let gs = GroupingSet::Simple(vec![0]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0], &agg_count_star()).unwrap();
        assert!(result.is_empty());
    }

    // -----------------------------------------------------------------
    //  aggregate_grouping_sets — ROLLUP
    // -----------------------------------------------------------------

    #[test]
    fn test_aggregate_rollup_basic() {
        // SELECT a, COUNT(*) FROM t GROUP BY ROLLUP(a)
        // → (a, count) for each a + (NULL, total_count)
        let rows = vec![
            make_row_int(&[1, 100]),
            make_row_int(&[1, 200]),
            make_row_int(&[2, 300]),
        ];
        let gs = GroupingSet::Rollup(vec![0]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0], &agg_count_star()).unwrap();

        assert_eq!(result.len(), 3); // a=1, a=2, grand total
        let has_1_2 = result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(2)]);
        let has_2_1 = result
            .iter()
            .any(|r| r == &vec![Value::Int64(2), Value::Int64(1)]);
        let has_null_3 = result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Int64(3)]);
        assert!(has_1_2, "should contain (1, 2): {:?}", result);
        assert!(has_2_1, "should contain (2, 1): {:?}", result);
        assert!(has_null_3, "should contain (NULL, 3): {:?}", result);
    }

    #[test]
    fn test_aggregate_rollup_2_cols() {
        // SELECT a, b, COUNT(*) FROM t GROUP BY ROLLUP(a, b)
        // → (a, b, count) + (a, NULL, count) + (NULL, NULL, total)
        let rows = vec![
            make_row_int_int_int(1, 10, 0),
            make_row_int_int_int(1, 10, 0),
            make_row_int_int_int(1, 20, 0),
            make_row_int_int_int(2, 30, 0),
        ];
        let gs = GroupingSet::Rollup(vec![0, 1]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0, 1], &agg_count_star()).unwrap();

        // 展开为 3 组：(a,b), (a), ()
        // (a,b) 分区: (1,10)→2, (1,20)→1, (2,30)→1 = 3 输出行
        // (a) 分区: (1)→3, (2)→1 = 2 输出行
        // () 分区: ()→4 = 1 输出行
        assert_eq!(result.len(), 6);

        // 验证 (1, 10, 2)
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(10), Value::Int64(2)]));
        // 验证 (1, NULL, 3) — a=1 小计
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Null, Value::Int64(3)]));
        // 验证 (NULL, NULL, 4) — 总计
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Null, Value::Int64(4)]));
    }

    // -----------------------------------------------------------------
    //  aggregate_grouping_sets — CUBE
    // -----------------------------------------------------------------

    #[test]
    fn test_aggregate_cube_basic() {
        // SELECT a, b, COUNT(*) FROM t GROUP BY CUBE(a, b)
        // → (a,b) + (a) + (b) + ()
        let rows = vec![
            make_row_int_int_int(1, 10, 0),
            make_row_int_int_int(1, 10, 0),
            make_row_int_int_int(2, 20, 0),
        ];
        let gs = GroupingSet::Cube(vec![0, 1]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0, 1], &agg_count_star()).unwrap();

        // CUBE(a,b) → 4 组
        // (a,b): (1,10)→2, (2,20)→1 = 2 行
        // (a): (1)→2, (2)→1 = 2 行
        // (b): (10)→2, (20)→1 = 2 行
        // (): ()→3 = 1 行
        assert_eq!(result.len(), 7);

        // 验证 (1, 10, 2)
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(10), Value::Int64(2)]));
        // 验证 (1, NULL, 2) — a=1 小计
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Null, Value::Int64(2)]));
        // 验证 (NULL, 10, 2) — b=10 小计
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Int64(10), Value::Int64(2)]));
        // 验证 (NULL, NULL, 3) — 总计
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Null, Value::Int64(3)]));
    }

    #[test]
    fn test_aggregate_cube_1_col() {
        // CUBE(a) ≡ ROLLUP(a) → (a), ()
        let rows = vec![make_row_int(&[1, 10]), make_row_int(&[1, 20])];
        let gs = GroupingSet::Cube(vec![0]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0], &agg_count_star()).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(2)]));
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Int64(2)]));
    }

    // -----------------------------------------------------------------
    //  aggregate_grouping_sets — GROUPING SETS
    // -----------------------------------------------------------------

    #[test]
    fn test_aggregate_grouping_sets_explicit() {
        // SELECT a, COUNT(*) FROM t GROUP BY GROUPING SETS ((a), ())
        // → 部门小计 + 总计
        let rows = vec![
            make_row_int(&[1, 10]),
            make_row_int(&[1, 20]),
            make_row_int(&[2, 30]),
        ];
        let gs = GroupingSet::GroupingSets(vec![vec![0], vec![]]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0], &agg_count_star()).unwrap();

        assert_eq!(result.len(), 3); // a=1, a=2, total
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(2)]));
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(2), Value::Int64(1)]));
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Int64(3)]));
    }

    #[test]
    fn test_aggregate_grouping_sets_multiple_sets() {
        // GROUPING SETS ((a, b), (a), (b), ())
        let rows = vec![
            make_row_int_int_int(1, 10, 0),
            make_row_int_int_int(1, 20, 0),
            make_row_int_int_int(2, 10, 0),
        ];
        let gs = GroupingSet::GroupingSets(vec![vec![0, 1], vec![0], vec![1], vec![]]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0, 1], &agg_count_star()).unwrap();

        // (a,b): (1,10)→1, (1,20)→1, (2,10)→1 = 3 行
        // (a): (1)→2, (2)→1 = 2 行
        // (b): (10)→2, (20)→1 = 2 行
        // (): ()→3 = 1 行
        assert_eq!(result.len(), 8);
    }

    // -----------------------------------------------------------------
    //  GROUPING / GROUPING_ID
    // -----------------------------------------------------------------

    #[test]
    fn test_grouping_in_group() {
        // 列在分组中 → 0
        let group = vec![0, 1];
        let group_cols = vec![0, 1, 2];
        assert_eq!(grouping(0, &group, &group_cols), Value::Int64(0));
        assert_eq!(grouping(1, &group, &group_cols), Value::Int64(0));
    }

    #[test]
    fn test_grouping_not_in_group() {
        // 列不在分组中 → 1
        let group = vec![0, 1];
        let group_cols = vec![0, 1, 2];
        assert_eq!(grouping(2, &group, &group_cols), Value::Int64(1));
    }

    #[test]
    fn test_grouping_empty_group() {
        // 空分组（总计）→ 所有列都返回 1
        let group: Vec<usize> = vec![];
        let group_cols = vec![0, 1];
        assert_eq!(grouping(0, &group, &group_cols), Value::Int64(1));
        assert_eq!(grouping(1, &group, &group_cols), Value::Int64(1));
    }

    #[test]
    fn test_grouping_id_all_in_group() {
        // 所有列都在分组中 → 0
        let group = vec![0, 1];
        let group_cols = vec![0, 1];
        assert_eq!(grouping_id(&[0, 1], &group, &group_cols), Value::Int64(0));
    }

    #[test]
    fn test_grouping_id_none_in_group() {
        // 没有列在分组中 → 3 (0b11)
        let group: Vec<usize> = vec![];
        let group_cols = vec![0, 1];
        assert_eq!(grouping_id(&[0, 1], &group, &group_cols), Value::Int64(3));
    }

    #[test]
    fn test_grouping_id_partial() {
        // a 在分组中, b 不在 → 1 (0b01)
        let group = vec![0];
        let group_cols = vec![0, 1];
        assert_eq!(grouping_id(&[0, 1], &group, &group_cols), Value::Int64(1));

        // a 不在, b 在 → 2 (0b10)
        let group2 = vec![1];
        assert_eq!(grouping_id(&[0, 1], &group2, &group_cols), Value::Int64(2));
    }

    #[test]
    fn test_grouping_id_three_cols() {
        // 3 列：a, b, c 都不在 → 7 (0b111)
        let group: Vec<usize> = vec![];
        let group_cols = vec![0, 1, 2];
        assert_eq!(
            grouping_id(&[0, 1, 2], &group, &group_cols),
            Value::Int64(7)
        );
    }

    // -----------------------------------------------------------------
    //  辅助聚合函数
    // -----------------------------------------------------------------

    #[test]
    fn test_agg_count_star() {
        let rows = vec![make_row_int(&[1]), make_row_int(&[2]), make_row_int(&[3])];
        let f = agg_count_star();
        assert_eq!(f(&rows), vec![Value::Int64(3)]);
        assert_eq!(f(&[]), vec![Value::Int64(0)]);
    }

    #[test]
    fn test_agg_sum_int64() {
        let rows = vec![
            make_row_int(&[1, 10]),
            make_row_int(&[2, 20]),
            make_row_int(&[3, 30]),
        ];
        let f = agg_sum_int64(1);
        assert_eq!(f(&rows), vec![Value::Int64(60)]);
    }

    #[test]
    fn test_agg_sum_int64_with_nulls() {
        // NULL 值被跳过
        let rows = vec![
            vec![Value::Int64(1), Value::Int64(10)],
            vec![Value::Int64(2), Value::Null],
            vec![Value::Int64(3), Value::Int64(30)],
        ];
        let f = agg_sum_int64(1);
        assert_eq!(f(&rows), vec![Value::Int64(40)]); // 10 + 30
    }

    #[test]
    fn test_agg_sum_int64_empty() {
        let f = agg_sum_int64(0);
        assert_eq!(f(&[]), vec![Value::Null]); // 空组返回 NULL
    }

    #[test]
    fn test_agg_sum_int64_all_null() {
        let rows = vec![
            vec![Value::Int64(1), Value::Null],
            vec![Value::Int64(2), Value::Null],
        ];
        let f = agg_sum_int64(1);
        assert_eq!(f(&rows), vec![Value::Null]); // 全 NULL → NULL
    }

    #[test]
    fn test_agg_count_col() {
        let rows = vec![
            vec![Value::Int64(1), Value::Int64(10)],
            vec![Value::Int64(2), Value::Null],
            vec![Value::Int64(3), Value::Int64(30)],
        ];
        let f = agg_count_col(1);
        assert_eq!(f(&rows), vec![Value::Int64(2)]); // 2 个非 NULL
    }

    #[test]
    fn test_agg_count_col_empty() {
        let f = agg_count_col(0);
        assert_eq!(f(&[]), vec![Value::Int64(0)]);
    }

    #[test]
    fn test_agg_min_value() {
        let rows = vec![
            vec![Value::Int64(1), Value::Int64(30)],
            vec![Value::Int64(2), Value::Int64(10)],
            vec![Value::Int64(3), Value::Int64(20)],
        ];
        let f = agg_min_value(1);
        assert_eq!(f(&rows), vec![Value::Int64(10)]);
    }

    #[test]
    fn test_agg_min_value_with_nulls() {
        let rows = vec![
            vec![Value::Int64(1), Value::Null],
            vec![Value::Int64(2), Value::Int64(10)],
            vec![Value::Int64(3), Value::Int64(5)],
        ];
        let f = agg_min_value(1);
        assert_eq!(f(&rows), vec![Value::Int64(5)]);
    }

    #[test]
    fn test_agg_min_value_empty() {
        let f = agg_min_value(0);
        assert_eq!(f(&[]), vec![Value::Null]);
    }

    #[test]
    fn test_agg_max_value() {
        let rows = vec![
            vec![Value::Int64(1), Value::Int64(30)],
            vec![Value::Int64(2), Value::Int64(10)],
            vec![Value::Int64(3), Value::Int64(20)],
        ];
        let f = agg_max_value(1);
        assert_eq!(f(&rows), vec![Value::Int64(30)]);
    }

    #[test]
    fn test_agg_max_value_with_nulls() {
        let rows = vec![
            vec![Value::Int64(1), Value::Null],
            vec![Value::Int64(2), Value::Int64(10)],
            vec![Value::Int64(3), Value::Int64(50)],
        ];
        let f = agg_max_value(1);
        assert_eq!(f(&rows), vec![Value::Int64(50)]);
    }

    #[test]
    fn test_agg_max_value_empty() {
        let f = agg_max_value(0);
        assert_eq!(f(&[]), vec![Value::Null]);
    }

    #[test]
    fn test_agg_min_max_text() {
        let rows = vec![
            make_row_int_text(1, "banana"),
            make_row_int_text(2, "apple"),
            make_row_int_text(3, "cherry"),
        ];
        let min_fn = agg_min_value(1);
        let max_fn = agg_max_value(1);
        assert_eq!(min_fn(&rows), vec![Value::Text("apple".to_string())]);
        assert_eq!(max_fn(&rows), vec![Value::Text("cherry".to_string())]);
    }

    // -----------------------------------------------------------------
    //  错误处理
    // -----------------------------------------------------------------

    #[test]
    fn test_error_empty_group_columns() {
        let rows = vec![make_row_int(&[1])];
        let gs = GroupingSet::Simple(vec![0]);
        let result = aggregate_grouping_sets(&rows, &gs, &[], &agg_count_star());
        assert_eq!(result, Err(GroupingSetsError::EmptyGroupColumns));
    }

    #[test]
    fn test_error_empty_grouping_set() {
        let rows = vec![make_row_int(&[1])];
        let gs = GroupingSet::GroupingSets(vec![]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0], &agg_count_star());
        assert_eq!(result, Err(GroupingSetsError::EmptyGroupingSet));
    }

    #[test]
    fn test_error_column_not_in_group() {
        // group_set 引用列 2，但 group_col_indices 只有 [0, 1]
        let rows = vec![make_row_int_int_int(1, 2, 3)];
        let gs = GroupingSet::Simple(vec![0, 1, 2]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0, 1], &agg_count_star());
        assert_eq!(result, Err(GroupingSetsError::ColumnNotInGroup(2)));
    }

    #[test]
    fn test_error_column_index_out_of_range() {
        let rows = vec![make_row_int(&[1, 2])]; // 宽度 2
        let gs = GroupingSet::Simple(vec![0]);
        let result = aggregate_grouping_sets(&rows, &gs, &[5], &agg_count_star());
        assert_eq!(result, Err(GroupingSetsError::ColumnIndexOutOfRange(5)));
    }

    #[test]
    fn test_error_to_execution_error() {
        let e: ExecutionError = GroupingSetsError::EmptyGroupColumns.into();
        match e {
            ExecutionError::EvalError(msg) => {
                assert!(msg.contains("GROUPING SETS error"));
                assert!(msg.contains("group column list is empty"));
            }
            _ => panic!("expected EvalError"),
        }
    }

    // -----------------------------------------------------------------
    //  E2E 场景
    // -----------------------------------------------------------------

    #[test]
    fn test_e2e_department_subtotal_and_total() {
        // 模拟 PG: SELECT dept, SUM(salary) FROM t GROUP BY GROUPING SETS ((dept), ())
        // 实际数据：dept_id, salary
        let rows: Vec<Row> = vec![
            vec![Value::Int64(1), Value::Int64(100)],
            vec![Value::Int64(1), Value::Int64(200)],
            vec![Value::Int64(2), Value::Int64(300)],
            vec![Value::Int64(2), Value::Int64(400)],
        ];
        let gs = GroupingSet::GroupingSets(vec![vec![0], vec![]]);
        let sum_fn = agg_sum_int64(1);
        let result = aggregate_grouping_sets(&rows, &gs, &[0], &sum_fn).unwrap();

        assert_eq!(result.len(), 3); // dept=1, dept=2, total
                                     // (1, 300)
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(300)]));
        // (2, 700)
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(2), Value::Int64(700)]));
        // (NULL, 1000)
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Int64(1000)]));
    }

    #[test]
    fn test_e2e_rollup_hierarchy() {
        // 模拟 PG: SELECT region, country, SUM(sales) FROM t GROUP BY ROLLUP(region, country)
        let rows: Vec<Row> = vec![
            vec![Value::Int64(1), Value::Int64(10), Value::Int64(100)], // region=1, country=10, sales=100
            vec![Value::Int64(1), Value::Int64(10), Value::Int64(200)],
            vec![Value::Int64(1), Value::Int64(20), Value::Int64(50)],
            vec![Value::Int64(2), Value::Int64(30), Value::Int64(300)],
        ];
        let gs = GroupingSet::Rollup(vec![0, 1]);
        let sum_fn = agg_sum_int64(2);
        let result = aggregate_grouping_sets(&rows, &gs, &[0, 1], &sum_fn).unwrap();

        // ROLLUP(region, country) → (region, country), (region), ()
        // (1, 10, 300), (1, 20, 50), (2, 30, 300) — 3 行
        // (1, NULL, 350), (2, NULL, 300) — 2 行
        // (NULL, NULL, 650) — 1 行
        assert_eq!(result.len(), 6);

        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(10), Value::Int64(300)]));
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Null, Value::Int64(350)]));
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Null, Value::Int64(650)]));
    }

    #[test]
    fn test_e2e_cube_all_subsets() {
        // CUBE(a, b) → 4 组
        let rows: Vec<Row> = vec![
            vec![Value::Int64(1), Value::Int64(10), Value::Int64(100)],
            vec![Value::Int64(1), Value::Int64(20), Value::Int64(200)],
            vec![Value::Int64(2), Value::Int64(10), Value::Int64(50)],
        ];
        let gs = GroupingSet::Cube(vec![0, 1]);
        let sum_fn = agg_sum_int64(2);
        let result = aggregate_grouping_sets(&rows, &gs, &[0, 1], &sum_fn).unwrap();

        // CUBE(a, b) → (a,b), (a), (b), ()
        // (a,b): (1,10)→100, (1,20)→200, (2,10)→50 — 3 行
        // (a): (1)→300, (2)→50 — 2 行
        // (b): (10)→150, (20)→200 — 2 行
        // (): ()→350 — 1 行
        assert_eq!(result.len(), 8);

        // 验证总计
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Null, Value::Int64(350)]));
        // 验证 a=1 小计
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Null, Value::Int64(300)]));
        // 验证 b=10 小计
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Int64(10), Value::Int64(150)]));
    }

    #[test]
    fn test_e2e_multiple_aggregates() {
        // SELECT a, COUNT(*), SUM(b), MIN(b), MAX(b) FROM t GROUP BY a
        let rows: Vec<Row> = vec![
            vec![Value::Int64(1), Value::Int64(10)],
            vec![Value::Int64(1), Value::Int64(20)],
            vec![Value::Int64(1), Value::Int64(5)],
            vec![Value::Int64(2), Value::Int64(100)],
        ];
        let gs = GroupingSet::Simple(vec![0]);

        let count_fn = agg_count_star();
        let sum_fn = agg_sum_int64(1);
        let min_fn = agg_min_value(1);
        let max_fn = agg_max_value(1);

        // 组合多个聚合
        let combined_fn = move |r: &[Row]| -> Vec<Value> {
            let mut result = count_fn(r);
            result.extend(sum_fn(r));
            result.extend(min_fn(r));
            result.extend(max_fn(r));
            result
        };

        let result = aggregate_grouping_sets(&rows, &gs, &[0], &combined_fn).unwrap();
        assert_eq!(result.len(), 2);

        // 找到 a=1 的行
        let row_1 = result
            .iter()
            .find(|r| r[0] == Value::Int64(1))
            .expect("should have a=1");
        // [a=1, count=3, sum=35, min=5, max=20]
        assert_eq!(
            row_1,
            &vec![
                Value::Int64(1),
                Value::Int64(3),  // COUNT(*)
                Value::Int64(35), // SUM
                Value::Int64(5),  // MIN
                Value::Int64(20), // MAX
            ]
        );
    }

    #[test]
    fn test_e2e_grouping_with_rollup() {
        // 验证 GROUPING 函数在 ROLLUP 场景下的行为
        let group_cols = vec![0, 1];
        let groups = GroupingSet::Rollup(vec![0, 1]).expand();

        // 第 0 组: (0, 1) — 都参与分组
        assert_eq!(grouping(0, &groups[0], &group_cols), Value::Int64(0));
        assert_eq!(grouping(1, &groups[0], &group_cols), Value::Int64(0));
        assert_eq!(
            grouping_id(&[0, 1], &groups[0], &group_cols),
            Value::Int64(0)
        );

        // 第 1 组: (0) — 只有 a 参与分组
        assert_eq!(grouping(0, &groups[1], &group_cols), Value::Int64(0));
        assert_eq!(grouping(1, &groups[1], &group_cols), Value::Int64(1));
        assert_eq!(
            grouping_id(&[0, 1], &groups[1], &group_cols),
            Value::Int64(1)
        );

        // 第 2 组: () — 总计
        assert_eq!(grouping(0, &groups[2], &group_cols), Value::Int64(1));
        assert_eq!(grouping(1, &groups[2], &group_cols), Value::Int64(1));
        assert_eq!(
            grouping_id(&[0, 1], &groups[2], &group_cols),
            Value::Int64(3)
        );
    }

    #[test]
    fn test_e2e_single_row_input() {
        // 单行输入
        let rows = vec![make_row_int(&[1, 100])];
        let gs = GroupingSet::Rollup(vec![0]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0], &agg_count_star()).unwrap();
        assert_eq!(result.len(), 2); // (1, 1) + (NULL, 1)
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(1)]));
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Int64(1)]));
    }

    #[test]
    fn test_e2e_all_null_group_key() {
        // 分组键包含 NULL — PG 中 NULL 值视为同一组
        let rows: Vec<Row> = vec![
            vec![Value::Null, Value::Int64(10)],
            vec![Value::Null, Value::Int64(20)],
            vec![Value::Int64(1), Value::Int64(30)],
        ];
        let gs = GroupingSet::Simple(vec![0]);
        let result = aggregate_grouping_sets(&rows, &gs, &[0], &agg_count_star()).unwrap();
        assert_eq!(result.len(), 2); // NULL 组 + a=1 组
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Null, Value::Int64(2)]));
        assert!(result
            .iter()
            .any(|r| r == &vec![Value::Int64(1), Value::Int64(1)]));
    }

    #[test]
    fn test_e2e_float_aggregation() {
        // Float64 列聚合
        let rows: Vec<Row> = vec![
            vec![Value::Int64(1), Value::Float64(1.5)],
            vec![Value::Int64(1), Value::Float64(2.5)],
            vec![Value::Int64(2), Value::Float64(3.0)],
        ];
        let gs = GroupingSet::Simple(vec![0]);
        let min_fn = agg_min_value(1);
        let max_fn = agg_max_value(1);
        let result = aggregate_grouping_sets(&rows, &gs, &[0], &move |r| {
            let mut v = min_fn(r);
            v.extend(max_fn(r));
            v
        })
        .unwrap();

        assert_eq!(result.len(), 2);
        let row_1 = result
            .iter()
            .find(|r| r[0] == Value::Int64(1))
            .expect("should have a=1");
        assert_eq!(row_1[1], Value::Float64(1.5)); // MIN
        assert_eq!(row_1[2], Value::Float64(2.5)); // MAX
    }
}
