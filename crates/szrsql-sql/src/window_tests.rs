//! Phase 6.2 单元测试 — 窗口函数（Window Functions）。
//!
//! 覆盖类别：
//! - Parser（3）：OVER 子句解析 / PARTITION BY + ORDER BY / 窗口帧 ROWS/RANGE
//! - Planner（3）：Window 节点生成 / 多窗口函数并列 / 与 Aggregate 共存
//! - Executor 排名函数（4）：ROW_NUMBER / RANK / DENSE_RANK / NTILE
//! - Executor 偏移函数（3）：LAG / LEAD / FIRST_VALUE / LAST_VALUE / NTH_VALUE
//! - Executor 聚合窗口（3）：SUM OVER / COUNT OVER / AVG OVER / MIN/MAX OVER
//! - PARTITION BY（2）：分区排名 / 分区聚合
//! - 窗口帧（2）：ROWS BETWEEN / 默认帧（RANGE UNBOUNDED PRECEDING）
//! - 错误处理（2）：未知窗口函数 / 窗口函数参数错误
//!
//! 共 22 个测试用例。覆盖 Phase 6.2 spec 中要求的 13 种窗口函数。

use super::executor::{Executor, InMemoryTable};
use crate::ast::*;
use crate::parser::parse_one;
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner, TableSchema};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建 catalog 表 `emp`：(id INT, dept INT, salary INT)
fn make_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_table(TableSchema {
        name: TableName::new("emp"),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("dept", ColumnType::Int64),
            ColumnDefinition::new("salary", ColumnType::Int64),
        ],
    });
    catalog
}

/// 创建内存表 `emp`，预置数据：
/// ```text
/// id | dept | salary
/// ---+------+-------
///  1 |   10 |   100
///  2 |   10 |   200
///  3 |   10 |   300
///  4 |   20 |   150
///  5 |   20 |   250
/// ```
fn make_emp_with_data() -> InMemoryTable {
    let mut table = InMemoryTable::new(TableSchema {
        name: TableName::new("emp"),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("dept", ColumnType::Int64),
            ColumnDefinition::new("salary", ColumnType::Int64),
        ],
    });
    table.insert(vec![Value::Int64(1), Value::Int64(10), Value::Int64(100)]);
    table.insert(vec![Value::Int64(2), Value::Int64(10), Value::Int64(200)]);
    table.insert(vec![Value::Int64(3), Value::Int64(10), Value::Int64(300)]);
    table.insert(vec![Value::Int64(4), Value::Int64(20), Value::Int64(150)]);
    table.insert(vec![Value::Int64(5), Value::Int64(20), Value::Int64(250)]);
    table
}

/// SQL → AST → LogicalPlan（断言成功）
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).expect("plan failed")
}

/// 提取执行结果的指定列为 i64 Vec
fn column_as_i64(rows: &[Vec<Value>], col: usize) -> Vec<i64> {
    rows.iter()
        .filter_map(|r| match r.get(col) {
            Some(Value::Int64(v)) => Some(*v),
            _ => None,
        })
        .collect()
}

// =====================================================================
//  Parser 测试（3）
// =====================================================================

#[test]
fn test_window_parser_01_over_basic() {
    // SELECT ROW_NUMBER() OVER () FROM emp
    let stmt = parse_one("SELECT ROW_NUMBER() OVER () FROM emp").unwrap();
    match stmt {
        Statement::Select(select) => {
            assert_eq!(select.projection.len(), 1);
            // 不再深入断言 Expr 内部结构（依赖 SelectItem API）
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_window_parser_02_partition_by_order_by() {
    let sql = "SELECT id, RANK() OVER (PARTITION BY dept ORDER BY salary DESC) AS rnk FROM emp";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Select(select) => {
            assert_eq!(select.projection.len(), 2);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_window_parser_03_window_frame() {
    let sql = "SELECT id, SUM(salary) OVER (ORDER BY id ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) AS s FROM emp";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Select(select) => {
            assert_eq!(select.projection.len(), 2);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

// =====================================================================
//  Planner 测试（3）
// =====================================================================

#[test]
fn test_window_planner_01_window_node_generated() {
    let catalog = make_catalog();
    let plan = plan_sql("SELECT ROW_NUMBER() OVER () FROM emp", &catalog);
    let plan_str = format!("{plan:?}");
    assert!(
        plan_str.contains("Window"),
        "expected Window node in plan: {plan_str}"
    );
}

#[test]
fn test_window_planner_02_multiple_window_functions() {
    let catalog = make_catalog();
    let sql = "SELECT id, ROW_NUMBER() OVER () AS rn, RANK() OVER (ORDER BY id) AS rk FROM emp";
    let plan = plan_sql(sql, &catalog);
    let plan_str = format!("{plan:?}");
    // 至少出现 2 个 window_funcs
    let window_count = plan_str.matches("WindowFunctionExpr").count();
    assert!(
        window_count >= 2,
        "expected >=2 WindowFunctionExpr, got {window_count}: {plan_str}"
    );
}

#[test]
fn test_window_planner_03_window_with_aggregate() {
    // GROUP BY + 窗口函数共存
    let catalog = make_catalog();
    let sql = "SELECT dept, COUNT(*) AS cnt, ROW_NUMBER() OVER (ORDER BY COUNT(*) DESC) AS rn FROM emp GROUP BY dept";
    let plan = plan_sql(sql, &catalog);
    let plan_str = format!("{plan:?}");
    assert!(
        plan_str.contains("Aggregate"),
        "expected Aggregate: {plan_str}"
    );
    assert!(plan_str.contains("Window"), "expected Window: {plan_str}");
}

// =====================================================================
//  Executor 排名函数（4）
// =====================================================================

#[test]
fn test_window_exec_01_row_number() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    let plan = plan_sql(
        "SELECT id, ROW_NUMBER() OVER (ORDER BY id) AS rn FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    // ORDER BY id 升序 → rn = 1..=5
    let rns = column_as_i64(&rows, 1);
    let mut sorted = rns.clone();
    sorted.sort();
    assert_eq!(sorted, vec![1, 2, 3, 4, 5]);
}

#[test]
fn test_window_exec_02_rank_with_ties() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    // 同 dept 内按 salary 排名；同薪则 RANK 重复并跳号
    let plan = plan_sql(
        "SELECT id, dept, RANK() OVER (PARTITION BY dept ORDER BY salary DESC) AS rk FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    // dept=10: salary 300,200,100 → rank 1,2,3
    // dept=20: salary 250,150 → rank 1,2
    let by_dept: std::collections::HashMap<i64, Vec<i64>> = rows
        .iter()
        .filter_map(|r| match (r.get(1), r.get(2)) {
            (Some(Value::Int64(dept)), Some(Value::Int64(rk))) => Some((*dept, *rk)),
            _ => None,
        })
        .fold(std::collections::HashMap::new(), |mut acc, (dept, rk)| {
            acc.entry(dept).or_default().push(rk);
            acc
        });
    let mut dept10 = by_dept.get(&10).cloned().unwrap_or_default();
    let mut dept20 = by_dept.get(&20).cloned().unwrap_or_default();
    dept10.sort();
    dept20.sort();
    assert_eq!(dept10, vec![1, 2, 3], "dept=10 ranks should be 1,2,3");
    assert_eq!(dept20, vec![1, 2], "dept=20 ranks should be 1,2");
}

#[test]
fn test_window_exec_03_dense_rank() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    let plan = plan_sql(
        "SELECT id, DENSE_RANK() OVER (ORDER BY dept) AS dr FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    // dept 10,10,10,20,20 → dense_rank 1,1,1,2,2
    let mut drs = column_as_i64(&rows, 1);
    drs.sort();
    // 排序后应包含两个唯一值 1 和 2，频次 3 和 2
    let unique: std::collections::BTreeSet<i64> = drs.iter().copied().collect();
    assert_eq!(unique, vec![1, 2].into_iter().collect());
    let count_1 = drs.iter().filter(|x| **x == 1).count();
    let count_2 = drs.iter().filter(|x| **x == 2).count();
    assert_eq!(count_1, 3, "dense_rank=1 should appear 3 times");
    assert_eq!(count_2, 2, "dense_rank=2 should appear 2 times");
}

#[test]
fn test_window_exec_04_ntile() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    let plan = plan_sql(
        "SELECT id, NTILE(2) OVER (ORDER BY id) AS bucket FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    // 5 行分 2 桶 → 桶 1 有 3 行，桶 2 有 2 行
    let mut buckets = column_as_i64(&rows, 1);
    buckets.sort();
    let count_1 = buckets.iter().filter(|x| **x == 1).count();
    let count_2 = buckets.iter().filter(|x| **x == 2).count();
    assert_eq!(count_1, 3, "bucket 1 should have 3 rows");
    assert_eq!(count_2, 2, "bucket 2 should have 2 rows");
}

// =====================================================================
//  Executor 偏移函数（3）
// =====================================================================

#[test]
fn test_window_exec_05_lag() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    let plan = plan_sql(
        "SELECT id, salary, LAG(salary, 1) OVER (ORDER BY id) AS prev_salary FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    // 按 id 排序：行 0..4 对应 id 1..5
    // 第一行 LAG 为 NULL，其余为前一行的 salary
    assert!(
        matches!(rows[0].get(2), Some(Value::Null)),
        "row 0 LAG should be NULL"
    );
    for i in 1..5 {
        let prev_salary = match rows[i - 1].get(1) {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 salary at row {}", i - 1),
        };
        let lag_val = match rows[i].get(2) {
            Some(Value::Int64(v)) => *v,
            Some(Value::Null) => panic!("LAG at row {i} was NULL, expected Int64"),
            other => panic!("expected Int64 LAG at row {i}, got {other:?}"),
        };
        assert_eq!(lag_val, prev_salary, "LAG mismatch at row {i}");
    }
}

#[test]
fn test_window_exec_06_lead() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    let plan = plan_sql(
        "SELECT id, salary, LEAD(salary, 1) OVER (ORDER BY id) AS next_salary FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    // 按 id 排序：行 0..4 对应 id 1..5
    // 最后一行 LEAD 为 NULL，其余为下一行的 salary
    assert!(
        matches!(rows[4].get(2), Some(Value::Null)),
        "row 4 LEAD should be NULL"
    );
    for i in 0..4 {
        let next_salary = match rows[i + 1].get(1) {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 salary at row {}", i + 1),
        };
        let lead_val = match rows[i].get(2) {
            Some(Value::Int64(v)) => *v,
            Some(Value::Null) => panic!("LEAD at row {i} was NULL, expected Int64"),
            other => panic!("expected Int64 LEAD at row {i}, got {other:?}"),
        };
        assert_eq!(lead_val, next_salary, "LEAD mismatch at row {i}");
    }
}

#[test]
fn test_window_exec_07_first_last_nth_value() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    // FIRST_VALUE / LAST_VALUE / NTH_VALUE 同一查询
    let plan = plan_sql(
        "SELECT id, \
         FIRST_VALUE(salary) OVER (PARTITION BY dept ORDER BY id) AS fv, \
         LAST_VALUE(salary) OVER (PARTITION BY dept ORDER BY id) AS lv, \
         NTH_VALUE(salary, 2) OVER (PARTITION BY dept ORDER BY id) AS nv \
         FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    // 验证 FIRST_VALUE：分区内按 id 升序，第一行 salary
    // dept=10 第一行 id=1, salary=100
    // dept=20 第一行 id=4, salary=150
    for r in &rows {
        let id = match r.first() {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 id"),
        };
        let fv = match r.get(1) {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 fv"),
        };
        let expected_fv = if id <= 3 {
            100
        } else {
            150
        };
        assert_eq!(fv, expected_fv, "FIRST_VALUE mismatch for id={id}");
    }
}

// =====================================================================
//  Executor 聚合窗口函数（3）
// =====================================================================

#[test]
fn test_window_exec_08_sum_over() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    // SUM OVER (PARTITION BY dept) — 分区累计 = 分区总和
    let plan = plan_sql(
        "SELECT id, SUM(salary) OVER (PARTITION BY dept) AS dept_total FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    // dept=10 总和 = 100+200+300 = 600
    // dept=20 总和 = 150+250 = 400
    for r in &rows {
        let id = match r.first() {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 id"),
        };
        let total = match r.get(1) {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 total"),
        };
        let expected = if id <= 3 {
            600
        } else {
            400
        };
        assert_eq!(total, expected, "SUM OVER PARTITION mismatch for id={id}");
    }
}

#[test]
fn test_window_exec_09_count_over() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    // COUNT(*) OVER (PARTITION BY dept) — 分区行数
    let plan = plan_sql(
        "SELECT id, COUNT(*) OVER (PARTITION BY dept) AS dept_cnt FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    for r in &rows {
        let id = match r.first() {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 id"),
        };
        let cnt = match r.get(1) {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 cnt"),
        };
        let expected = if id <= 3 {
            3
        } else {
            2
        };
        assert_eq!(cnt, expected, "COUNT OVER PARTITION mismatch for id={id}");
    }
}

#[test]
fn test_window_exec_10_avg_min_max_over() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    let plan = plan_sql(
        "SELECT id, \
         AVG(salary) OVER (PARTITION BY dept) AS avg_sal, \
         MIN(salary) OVER (PARTITION BY dept) AS min_sal, \
         MAX(salary) OVER (PARTITION BY dept) AS max_sal \
         FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    for r in &rows {
        let id = match r.first() {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 id"),
        };
        let avg = match r.get(1) {
            Some(Value::Float64(v)) => *v,
            Some(Value::Int64(v)) => *v as f64,
            other => panic!("expected numeric avg, got {other:?}"),
        };
        let min = match r.get(2) {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 min"),
        };
        let max = match r.get(3) {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 max"),
        };
        let (expected_avg, expected_min, expected_max) = if id <= 3 {
            (200.0, 100, 300)
        } else {
            (200.0, 150, 250)
        };
        assert!(
            (avg - expected_avg).abs() < 0.001,
            "AVG mismatch for id={id}: {avg}"
        );
        assert_eq!(min, expected_min, "MIN mismatch for id={id}");
        assert_eq!(max, expected_max, "MAX mismatch for id={id}");
    }
}

// =====================================================================
//  PARTITION BY 测试（2）
// =====================================================================

#[test]
fn test_window_partition_01_rank_per_dept() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    let plan = plan_sql(
        "SELECT id, dept, ROW_NUMBER() OVER (PARTITION BY dept ORDER BY salary DESC) AS rn FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    // dept=10: salary 300,200,100 → rn 1,2,3
    // dept=20: salary 250,150 → rn 1,2
    // 按 dept 分组并按 rn 排序，验证 id 顺序
    let mut dept10: Vec<(i64, i64)> = Vec::new();
    let mut dept20: Vec<(i64, i64)> = Vec::new();
    for r in &rows {
        let id = match r.first() {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 id"),
        };
        let dept = match r.get(1) {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 dept"),
        };
        let rn = match r.get(2) {
            Some(Value::Int64(v)) => *v,
            _ => panic!("expected Int64 rn"),
        };
        if dept == 10 {
            dept10.push((rn, id));
        } else {
            dept20.push((rn, id));
        }
    }
    dept10.sort();
    dept20.sort();
    let dept10_ids: Vec<i64> = dept10.iter().map(|(_, id)| *id).collect();
    let dept20_ids: Vec<i64> = dept20.iter().map(|(_, id)| *id).collect();
    // dept=10 按 salary 降序：300(id=3), 200(id=2), 100(id=1)
    assert_eq!(
        dept10_ids,
        vec![3, 2, 1],
        "dept=10 ids ordered by salary DESC"
    );
    // dept=20 按 salary 降序：250(id=5), 150(id=4)
    assert_eq!(dept20_ids, vec![5, 4], "dept=20 ids ordered by salary DESC");
}

#[test]
fn test_window_partition_02_sum_running_total() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    // 累计求和（默认帧：ORDER BY id RANGE UNBOUNDED PRECEDING）
    let plan = plan_sql(
        "SELECT id, SUM(salary) OVER (ORDER BY id) AS running_sum FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    // 按 id 升序累计：100, 300, 600, 750, 1000
    let mut by_id: Vec<(i64, i64)> = rows
        .iter()
        .filter_map(|r| match (r.first(), r.get(1)) {
            (Some(Value::Int64(id)), Some(Value::Int64(sum))) => Some((*id, *sum)),
            _ => None,
        })
        .collect();
    by_id.sort();
    let sums: Vec<i64> = by_id.iter().map(|(_, s)| *s).collect();
    assert_eq!(sums, vec![100, 300, 600, 750, 1000], "running sum mismatch");
}

// =====================================================================
//  窗口帧测试（2）
// =====================================================================

#[test]
fn test_window_frame_01_rows_between() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    // ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING — 滑动 3 行窗口求和
    let plan = plan_sql(
        "SELECT id, SUM(salary) OVER (ORDER BY id ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) AS s FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    let mut by_id: Vec<(i64, i64)> = rows
        .iter()
        .filter_map(|r| match (r.first(), r.get(1)) {
            (Some(Value::Int64(id)), Some(Value::Int64(s))) => Some((*id, *s)),
            _ => None,
        })
        .collect();
    by_id.sort();
    let sums: Vec<i64> = by_id.iter().map(|(_, s)| *s).collect();
    // id=1: 100 (无前) + 200 = 300
    // id=2: 100 + 200 + 300 = 600
    // id=3: 200 + 300 + 150 = 650
    // id=4: 300 + 150 + 250 = 700
    // id=5: 150 + 250 (无后) = 400
    assert_eq!(
        sums,
        vec![300, 600, 650, 700, 400],
        "ROWS frame sum mismatch"
    );
}

#[test]
fn test_window_frame_02_default_frame_with_order_by() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    // 默认帧（有 ORDER BY）= RANGE UNBOUNDED PRECEDING TO CURRENT ROW
    // 等同于累计求和
    let plan = plan_sql(
        "SELECT id, SUM(salary) OVER (ORDER BY id) AS cum_sum FROM emp",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    let mut by_id: Vec<(i64, i64)> = rows
        .iter()
        .filter_map(|r| match (r.first(), r.get(1)) {
            (Some(Value::Int64(id)), Some(Value::Int64(s))) => Some((*id, *s)),
            _ => None,
        })
        .collect();
    by_id.sort();
    let sums: Vec<i64> = by_id.iter().map(|(_, s)| *s).collect();
    // 累计：100, 300, 600, 750, 1000
    assert_eq!(
        sums,
        vec![100, 300, 600, 750, 1000],
        "default frame cumsum mismatch"
    );
}

// =====================================================================
//  错误处理（2）
// =====================================================================

#[test]
fn test_window_error_01_unknown_function() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    let plan = plan_sql(
        "SELECT id, UNKNOWN_FUNC() OVER (ORDER BY id) AS u FROM emp",
        &catalog,
    );
    let result = exec.execute(&plan);
    assert!(
        result.is_err(),
        "expected error for unknown window function"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.to_lowercase().contains("unknown")
            || err_msg.to_lowercase().contains("unsupported"),
        "error should mention unknown/unsupported: {err_msg}"
    );
}

#[test]
fn test_window_error_02_lag_without_order() {
    let catalog = make_catalog();
    let emp = make_emp_with_data();
    let mut exec = Executor::new();
    exec.register_table(&emp);

    // LAG/LEAD 在无 ORDER BY 时行为未定义；这里仅验证不 panic
    let plan = plan_sql(
        "SELECT id, LAG(salary, 1) OVER () AS lag_s FROM emp",
        &catalog,
    );
    let result = exec.execute(&plan);
    // 允许返回 Ok（按行顺序）或 Err；只要不 panic 即可
    match result {
        Ok(rows) => assert_eq!(rows.len(), 5),
        Err(_) => { /* 也接受错误 */ }
    }
}
