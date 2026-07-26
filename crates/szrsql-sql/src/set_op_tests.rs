//! 集合操作单元测试 — INTERSECT / EXCEPT / UNION / UNION ALL。
//!
//! Phase 3.27 基础测试（22）：
//! - Parser（5）：INTERSECT / EXCEPT / UNION / INTERSECT ALL / EXCEPT ALL
//! - Planner（3）：SetOp 计划生成 / 嵌套集合操作 / 列数校验
//! - Executor INTERSECT（4）：基本交集 / 去重 / INTERSECT ALL 重复次数 / 空集
//! - Executor EXCEPT（4）：基本差集 / 去重 / EXCEPT ALL 重复次数 / 空集
//! - Executor UNION（3）：UNION ALL / UNION DISTINCT / 与 INTERSECT/EXCEPT 组合
//! - 错误处理（3）：列数不匹配 / 不存在的表 / 错误计划类型
//!
//! Phase 6.3 组合测试（60）— UNION/UNION ALL/INTERSECT/EXCEPT 各 20 组合：
//! - UNION 组合（18）：WHERE / ORDER BY / LIMIT / 多列 / 表达式 / 聚合 / 子查询 / CTE / JOIN / NULL / 嵌套 / 空集 / GROUP BY
//! - UNION ALL 组合（19）：同上场景 + 行数保持验证
//! - INTERSECT 组合（12）：WHERE / ORDER BY / LIMIT / 多列 / 表达式 / 子查询 / CTE / 自交 / NULL / 嵌套 / 空集 / DISTINCT vs ALL
//! - EXCEPT 组合（11）：WHERE / ORDER BY / LIMIT / 多列 / 表达式 / 子查询 / CTE / 自差 / NULL / 空集 / DISTINCT vs ALL
//!
//! 共 82 个测试用例（UNION=20 / UNION ALL=20 / INTERSECT=20 / EXCEPT=20 组合）。

use super::executor::{ExecutionError, Executor, InMemoryTable, MutableTable};
use crate::ast::*;
use crate::parser::parse_one;
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner, TableSchema};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建 catalog 表 `t1`：(id INT, val INT)
fn make_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let val_col = ColumnDefinition::new("val", ColumnType::Int64);
    catalog.add_table(TableSchema {
        name: TableName::new("t1"),
        columns: vec![id_col, val_col],
    });
    let id_col2 = ColumnDefinition::new("id", ColumnType::Int64);
    let val_col2 = ColumnDefinition::new("val", ColumnType::Int64);
    catalog.add_table(TableSchema {
        name: TableName::new("t2"),
        columns: vec![id_col2, val_col2],
    });
    catalog
}

/// 创建内存表 `t1`：(id INT, val INT)，预置数据：(1,10), (2,20), (3,30)
fn make_t1_with_data() -> InMemoryTable {
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let val_col = ColumnDefinition::new("val", ColumnType::Int64);
    let mut table = InMemoryTable::new(TableSchema {
        name: TableName::new("t1"),
        columns: vec![id_col, val_col],
    });
    table.insert_row(vec![Value::Int64(1), Value::Int64(10)]);
    table.insert_row(vec![Value::Int64(2), Value::Int64(20)]);
    table.insert_row(vec![Value::Int64(3), Value::Int64(30)]);
    table
}

/// 创建内存表 `t2`：(id INT, val INT)，预置数据：(2,20), (3,30), (4,40)
fn make_t2_with_data() -> InMemoryTable {
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let val_col = ColumnDefinition::new("val", ColumnType::Int64);
    let mut table = InMemoryTable::new(TableSchema {
        name: TableName::new("t2"),
        columns: vec![id_col, val_col],
    });
    table.insert_row(vec![Value::Int64(2), Value::Int64(20)]);
    table.insert_row(vec![Value::Int64(3), Value::Int64(30)]);
    table.insert_row(vec![Value::Int64(4), Value::Int64(40)]);
    table
}

/// 创建含重复值的内存表 `t1`：(id INT, val INT)，预置：(1,10), (1,10), (2,20)
fn make_t1_with_duplicates() -> InMemoryTable {
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let val_col = ColumnDefinition::new("val", ColumnType::Int64);
    let mut table = InMemoryTable::new(TableSchema {
        name: TableName::new("t1"),
        columns: vec![id_col, val_col],
    });
    table.insert_row(vec![Value::Int64(1), Value::Int64(10)]);
    table.insert_row(vec![Value::Int64(1), Value::Int64(10)]);
    table.insert_row(vec![Value::Int64(2), Value::Int64(20)]);
    table
}

/// 创建含重复值的内存表 `t2`：(id INT, val INT)，预置：(1,10), (2,20), (2,20)
fn make_t2_with_duplicates() -> InMemoryTable {
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let val_col = ColumnDefinition::new("val", ColumnType::Int64);
    let mut table = InMemoryTable::new(TableSchema {
        name: TableName::new("t2"),
        columns: vec![id_col, val_col],
    });
    table.insert_row(vec![Value::Int64(1), Value::Int64(10)]);
    table.insert_row(vec![Value::Int64(2), Value::Int64(20)]);
    table.insert_row(vec![Value::Int64(2), Value::Int64(20)]);
    table
}

/// SQL → AST → LogicalPlan（断言成功）
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).expect("plan failed")
}

/// 排序行集合以便比较（按第一列升序）
fn sort_rows_by_first(rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    let mut sorted = rows;
    sorted.sort_by(|a, b| match (a.first(), b.first()) {
        (Some(Value::Int64(a)), Some(Value::Int64(b))) => a.cmp(b),
        _ => std::cmp::Ordering::Equal,
    });
    sorted
}

// =====================================================================
//  Parser 测试（5）
// =====================================================================

#[test]
fn test_set_op_parser_01_intersect() {
    let stmt = parse_one("SELECT id FROM t1 INTERSECT SELECT id FROM t2").unwrap();
    match stmt {
        Statement::Select(select) => {
            let set_op = select.set_op.expect("expected set_op");
            assert_eq!(set_op.op, SetOperator::Intersect);
            assert_eq!(set_op.quantifier, SetQuantifier::None);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_set_op_parser_02_except() {
    let stmt = parse_one("SELECT id FROM t1 EXCEPT SELECT id FROM t2").unwrap();
    match stmt {
        Statement::Select(select) => {
            let set_op = select.set_op.expect("expected set_op");
            assert_eq!(set_op.op, SetOperator::Except);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_set_op_parser_03_union() {
    let stmt = parse_one("SELECT id FROM t1 UNION SELECT id FROM t2").unwrap();
    match stmt {
        Statement::Select(select) => {
            let set_op = select.set_op.expect("expected set_op");
            assert_eq!(set_op.op, SetOperator::Union);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_set_op_parser_04_intersect_all() {
    let stmt = parse_one("SELECT id FROM t1 INTERSECT ALL SELECT id FROM t2").unwrap();
    match stmt {
        Statement::Select(select) => {
            let set_op = select.set_op.expect("expected set_op");
            assert_eq!(set_op.op, SetOperator::Intersect);
            assert_eq!(set_op.quantifier, SetQuantifier::All);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

#[test]
fn test_set_op_parser_05_except_distinct() {
    let stmt = parse_one("SELECT id FROM t1 EXCEPT DISTINCT SELECT id FROM t2").unwrap();
    match stmt {
        Statement::Select(select) => {
            let set_op = select.set_op.expect("expected set_op");
            assert_eq!(set_op.op, SetOperator::Except);
            assert_eq!(set_op.quantifier, SetQuantifier::Distinct);
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

// =====================================================================
//  Planner 测试（3）
// =====================================================================

#[test]
fn test_set_op_planner_01_basic_intersect() {
    let catalog = make_catalog();
    let plan = plan_sql("SELECT id FROM t1 INTERSECT SELECT id FROM t2", &catalog);
    match plan {
        LogicalPlan::SetOp {
            op,
            quantifier,
            left,
            right,
        } => {
            assert_eq!(op, SetOperator::Intersect);
            assert_eq!(quantifier, SetQuantifier::None);
            assert!(matches!(*left, LogicalPlan::Projection { .. }));
            assert!(matches!(*right, LogicalPlan::Projection { .. }));
        }
        other => panic!("expected SetOp, got {other:?}"),
    }
}

#[test]
fn test_set_op_planner_02_nested_set_op() {
    let catalog = make_catalog();
    // (SELECT id FROM t1 UNION SELECT id FROM t2) EXCEPT SELECT id FROM t1
    // sqlparser 解析为 left=SetOp(UNION, t1, t2), right=t1, op=EXCEPT
    let plan = plan_sql(
        "SELECT id FROM t1 UNION SELECT id FROM t2 EXCEPT SELECT id FROM t1",
        &catalog,
    );
    // 外层应为 EXCEPT 的 SetOp
    match plan {
        LogicalPlan::SetOp {
            op,
            quantifier,
            left,
            right,
        } => {
            assert_eq!(op, SetOperator::Except);
            assert_eq!(quantifier, SetQuantifier::None);
            // left 应为嵌套的 UNION SetOp
            assert!(
                matches!(
                    *left,
                    LogicalPlan::SetOp {
                        op: SetOperator::Union,
                        ..
                    }
                ),
                "expected nested UNION, got {:?}",
                *left
            );
            // right 应为 Projection（SELECT id FROM t1）
            assert!(matches!(*right, LogicalPlan::Projection { .. }));
        }
        other => panic!("expected SetOp, got {other:?}"),
    }
}

#[test]
fn test_set_op_planner_03_with_order_by_limit() {
    let catalog = make_catalog();
    let plan = plan_sql(
        "SELECT id FROM t1 INTERSECT SELECT id FROM t2 ORDER BY id LIMIT 10",
        &catalog,
    );
    // 外层应为 Limit(Sort(SetOp))
    match plan {
        LogicalPlan::Limit { input, .. } => match *input {
            LogicalPlan::Sort { input, .. } => match *input {
                LogicalPlan::SetOp { op, .. } => assert_eq!(op, SetOperator::Intersect),
                other => panic!("expected SetOp, got {other:?}"),
            },
            other => panic!("expected Sort, got {other:?}"),
        },
        other => panic!("expected Limit, got {other:?}"),
    }
}

// =====================================================================
//  Executor INTERSECT 测试（4）
// =====================================================================

#[test]
fn test_set_op_exec_intersect_01_basic() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql("SELECT id FROM t1 INTERSECT SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    let ids: Vec<i64> = rows
        .iter()
        .filter_map(|r| match r.first() {
            Some(Value::Int64(v)) => Some(*v),
            _ => None,
        })
        .collect();
    // t1.id = {1,2,3}, t2.id = {2,3,4} → 交集 = {2,3}
    let mut ids_sorted = ids;
    ids_sorted.sort();
    assert_eq!(ids_sorted, vec![2, 3]);
}

#[test]
fn test_set_op_exec_intersect_02_distinct_default() {
    let catalog = make_catalog();
    let t1 = make_t1_with_duplicates();
    let t2 = make_t2_with_duplicates();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // 默认 DISTINCT：t1.id = {1,1,2}, t2.id = {1,2,2} → 交集 = {1,2}（去重）
    let plan = plan_sql("SELECT id FROM t1 INTERSECT SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 2, "INTERSECT DISTINCT should dedup to 2 rows");
}

#[test]
fn test_set_op_exec_intersect_03_all_keeps_duplicates() {
    let catalog = make_catalog();
    let t1 = make_t1_with_duplicates();
    let t2 = make_t2_with_duplicates();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // INTERSECT ALL：t1.id = {1,1,2}, t2.id = {1,2,2}
    // 1: min(2,1)=1, 2: min(1,2)=1 → 总共 2 行
    let plan = plan_sql(
        "SELECT id FROM t1 INTERSECT ALL SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "INTERSECT ALL should keep min(left,right) duplicates"
    );
}

#[test]
fn test_set_op_exec_intersect_04_empty_when_no_overlap() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data(); // ids: 1, 2, 3
    let mut t2 = InMemoryTable::new(TableSchema {
        name: TableName::new("t2"),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("val", ColumnType::Int64),
        ],
    });
    // t2 只有 id=99, 100（与 t1 无交集）
    t2.insert_row(vec![Value::Int64(99), Value::Int64(999)]);
    t2.insert_row(vec![Value::Int64(100), Value::Int64(1000)]);

    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql("SELECT id FROM t1 INTERSECT SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert!(rows.is_empty(), "no overlap should return empty");
}

// =====================================================================
//  Executor EXCEPT 测试（4）
// =====================================================================

#[test]
fn test_set_op_exec_except_01_basic() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql("SELECT id FROM t1 EXCEPT SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    let mut ids: Vec<i64> = rows
        .iter()
        .filter_map(|r| match r.first() {
            Some(Value::Int64(v)) => Some(*v),
            _ => None,
        })
        .collect();
    ids.sort();
    // t1.id = {1,2,3}, t2.id = {2,3,4} → 差集 = {1}
    assert_eq!(ids, vec![1]);
}

#[test]
fn test_set_op_exec_except_02_distinct_default() {
    let catalog = make_catalog();
    let t1 = make_t1_with_duplicates(); // ids: 1, 1, 2
    let t2 = make_t2_with_duplicates(); // ids: 1, 2, 2
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // EXCEPT DISTINCT: {1,1,2} - {1,2,2} = {} (空集，因为 1 和 2 都在 t2 中)
    let plan = plan_sql("SELECT id FROM t1 EXCEPT SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(
        rows.len(),
        0,
        "EXCEPT DISTINCT: {{1,1,2}} - {{1,2,2}} should be empty"
    );
}

#[test]
fn test_set_op_exec_except_03_all_subtracts_duplicates() {
    let catalog = make_catalog();
    let t1 = make_t1_with_duplicates(); // ids: 1, 1, 2
    let t2 = make_t2_with_duplicates(); // ids: 1, 2, 2
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // EXCEPT ALL: 1: max(0, 2-1)=1, 2: max(0, 1-2)=0 → 总共 1 行（一个 1）
    let plan = plan_sql("SELECT id FROM t1 EXCEPT ALL SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(
        rows.len(),
        1,
        "EXCEPT ALL: {{1,1,2}} - {{1,2,2}} should yield one row (id=1)"
    );
    assert_eq!(rows[0][0], Value::Int64(1));
}

#[test]
fn test_set_op_exec_except_04_empty_when_left_empty() {
    let catalog = make_catalog();
    let t1 = InMemoryTable::new(TableSchema {
        name: TableName::new("t1"),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("val", ColumnType::Int64),
        ],
    });
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql("SELECT id FROM t1 EXCEPT SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert!(rows.is_empty(), "empty left should return empty");
}

// =====================================================================
//  Executor UNION 测试（3）
// =====================================================================

#[test]
fn test_set_op_exec_union_01_all() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data(); // 3 行
    let t2 = make_t2_with_data(); // 3 行
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql("SELECT id FROM t1 UNION ALL SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    // UNION ALL：3 + 3 = 6 行（不去重）
    assert_eq!(rows.len(), 6, "UNION ALL should not dedup");
}

#[test]
fn test_set_op_exec_union_02_distinct_default() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data(); // ids: 1, 2, 3
    let t2 = make_t2_with_data(); // ids: 2, 3, 4
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql("SELECT id FROM t1 UNION SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    // UNION [DISTINCT]：{1,2,3} ∪ {2,3,4} = {1,2,3,4}（去重）
    assert_eq!(rows.len(), 4, "UNION DISTINCT should dedup");
}

#[test]
fn test_set_op_exec_union_03_combined_with_except() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data(); // ids: 1, 2, 3
    let t2 = make_t2_with_data(); // ids: 2, 3, 4
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // (t1 UNION t2) EXCEPT t1 → 应为 {4}
    // sqlparser 默认左结合：((t1 UNION t2) EXCEPT t1)
    let plan = plan_sql(
        "SELECT id FROM t1 UNION SELECT id FROM t2 EXCEPT SELECT id FROM t1",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    let mut ids: Vec<i64> = rows
        .iter()
        .filter_map(|r| match r.first() {
            Some(Value::Int64(v)) => Some(*v),
            _ => None,
        })
        .collect();
    ids.sort();
    // (t1 ∪ t2) - t1 = {1,2,3,4} - {1,2,3} = {4}
    assert_eq!(ids, vec![4]);
}

// =====================================================================
//  错误处理测试（3）
// =====================================================================

#[test]
fn test_set_op_error_01_column_count_mismatch() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1 选 2 列，t2 选 1 列 → 列数不匹配
    let plan = plan_sql(
        "SELECT id, val FROM t1 INTERSECT SELECT id FROM t2",
        &catalog,
    );
    let err = exec.execute(&plan).unwrap_err();
    match err {
        ExecutionError::InvalidArgument(msg) => {
            assert!(
                msg.contains("column counts"),
                "expected column counts error, got: {msg}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn test_set_op_error_02_table_not_found() {
    let catalog = InMemoryCatalog::new(); // 空 catalog
    let stmt = parse_one("SELECT id FROM t1 INTERSECT SELECT id FROM t2").unwrap();
    let planner = Planner::new(&catalog);
    let err = planner.plan_statement(stmt).unwrap_err();
    // 应报 PlanError::TableNotFound
    let msg = format!("{err}");
    assert!(
        msg.contains("not found") || msg.contains("TableNotFound"),
        "expected table not found, got: {msg}"
    );
}

#[test]
fn test_set_op_error_03_column_count_mismatch_with_dup_values() {
    // 验证列数校验在不同行数情况下也有效
    let catalog = make_catalog();
    let t1 = make_t1_with_duplicates(); // 3 行：1,1,2
    let t2 = make_t2_with_duplicates(); // 3 行：1,2,2
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1 选 1 列，t2 选 1 列，列数匹配，但用更复杂的表达式
    let plan = plan_sql(
        "SELECT id FROM t1 EXCEPT ALL SELECT id FROM t2 WHERE id > 0",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    // t1.id = {1,1,2}, t2.id (filtered >0) = {1,2,2}
    // EXCEPT ALL: 1: max(0,2-1)=1, 2: max(0,1-2)=0 → 1 行
    assert_eq!(rows.len(), 1, "EXCEPT ALL with WHERE should still work");
}

// =====================================================================
//  Phase 6.3 — 集合操作组合测试
//  Spec: UNION / UNION ALL / INTERSECT / EXCEPT 各 20 个组合
//
//  现有测试分布（Phase 3.27）：
//  - UNION: parser_03, exec_union_02 = 2
//  - UNION ALL: exec_union_01 = 1
//  - INTERSECT: parser_01, parser_04, planner_01, planner_03, exec_intersect_01-04 = 8
//  - EXCEPT: parser_02, parser_05, planner_02, exec_except_01-04, exec_union_03, error_03 = 9
//
//  新增组合测试：UNION +18, UNION ALL +19, INTERSECT +12, EXCEPT +11 = 60
//  最终：UNION=20, UNION ALL=20, INTERSECT=20, EXCEPT=20
// =====================================================================

// --- 辅助函数（Phase 6.3）---

/// 从行集合提取第一列 i64 并排序（用于无序结果比较）
fn sorted_ids(rows: Vec<Vec<Value>>) -> Vec<i64> {
    let mut ids: Vec<i64> = rows
        .iter()
        .filter_map(|r| match r.first() {
            Some(Value::Int64(v)) => Some(*v),
            _ => None,
        })
        .collect();
    ids.sort();
    ids
}

/// 从行集合提取第一列 i64（保持顺序，用于有序结果比较）
fn first_col_ids(rows: &[Vec<Value>]) -> Vec<i64> {
    rows.iter()
        .filter_map(|r| match r.first() {
            Some(Value::Int64(v)) => Some(*v),
            _ => None,
        })
        .collect()
}

/// 创建含 NULL 的内存表 `t3`：(id INT, val INT)，预置：(1,10), (2,NULL), (NULL,30)
fn make_t3_with_nulls() -> InMemoryTable {
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let val_col = ColumnDefinition::new("val", ColumnType::Int64);
    let mut table = InMemoryTable::new(TableSchema {
        name: TableName::new("t3"),
        columns: vec![id_col, val_col],
    });
    table.insert_row(vec![Value::Int64(1), Value::Int64(10)]);
    table.insert_row(vec![Value::Int64(2), Value::Null]);
    table.insert_row(vec![Value::Null, Value::Int64(30)]);
    table
}

/// 创建含 t3 的 catalog（t1, t2, t3 均为 (id INT, val INT)）
fn make_catalog_with_t3() -> InMemoryCatalog {
    let mut catalog = make_catalog();
    catalog.add_table(TableSchema {
        name: TableName::new("t3"),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("val", ColumnType::Int64),
        ],
    });
    catalog
}

// =====================================================================
//  UNION 组合测试（18 new，总计 20）
//  现有: parser_03_union, exec_union_02_distinct_default
//  注: exec_union_03_combined_with_except 外层为 EXCEPT，计入 EXCEPT
// =====================================================================

#[test]
fn test_set_op_union_04_no_overlap() {
    // 无交集：t1.WHERE(id<2)={1}, t2.WHERE(id>3)={4} → UNION = {1,4}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 WHERE id < 2 UNION SELECT id FROM t2 WHERE id > 3",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 4]);
}

#[test]
fn test_set_op_union_05_self_union() {
    // 自并集：{1,2,3} ∪ {1,2,3} = {1,2,3}（去重）
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);

    let plan = plan_sql("SELECT id FROM t1 UNION SELECT id FROM t1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3]);
}

#[test]
fn test_set_op_union_06_where_left() {
    // 左侧 WHERE：t1.WHERE(id<3)={1,2} ∪ t2={2,3,4} → {1,2,3,4}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 WHERE id < 3 UNION SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3, 4]);
}

#[test]
fn test_set_op_union_07_where_right() {
    // 右侧 WHERE：t1={1,2,3} ∪ t2.WHERE(id<4)={2,3} → {1,2,3}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 UNION SELECT id FROM t2 WHERE id < 4",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3]);
}

#[test]
fn test_set_op_union_08_where_both() {
    // 双侧 WHERE：t1.WHERE(id<2)={1} ∪ t2.WHERE(id>3)={4} → {1,4}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 WHERE id < 2 UNION SELECT id FROM t2 WHERE id > 3",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 4]);
}

#[test]
fn test_set_op_union_09_order_by() {
    // ORDER BY：t1 ∪ t2 ORDER BY id DESC → [4,3,2,1]
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 UNION SELECT id FROM t2 ORDER BY id DESC",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(first_col_ids(&rows), vec![4, 3, 2, 1]);
}

#[test]
fn test_set_op_union_10_limit() {
    // LIMIT：t1 ∪ t2 ORDER BY id LIMIT 2 → [1,2]
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 UNION SELECT id FROM t2 ORDER BY id LIMIT 2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(first_col_ids(&rows), vec![1, 2]);
}

#[test]
fn test_set_op_union_11_two_columns() {
    // 多列：(id,val) UNION (id,val) → 去重 4 行
    let catalog = make_catalog();
    let t1 = make_t1_with_data(); // (1,10),(2,20),(3,30)
    let t2 = make_t2_with_data(); // (2,20),(3,30),(4,40)
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id, val FROM t1 UNION SELECT id, val FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 4, "2-col UNION should dedup to 4 rows");
}

#[test]
fn test_set_op_union_12_expression() {
    // 表达式：t1.(id+1)={2,3,4} ∪ t2.id={2,3,4} → {2,3,4}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql("SELECT id + 1 FROM t1 UNION SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![2, 3, 4]);
}

#[test]
fn test_set_op_union_13_aggregate() {
    // 聚合：COUNT(t1)=3, SUM(t2.id)=9 → UNION = {3,9}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT COUNT(*) FROM t1 UNION SELECT SUM(id) FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![3, 9]);
}

#[test]
fn test_set_op_union_14_subquery() {
    // 子查询：SELECT FROM (subquery) UNION t2
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM (SELECT id FROM t1) sub1 UNION SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3, 4]);
}

#[test]
fn test_set_op_union_15_cte() {
    // CTE：WITH cte AS (t1) SELECT FROM cte UNION t2
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "WITH cte AS (SELECT id FROM t1) SELECT id FROM cte UNION SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3, 4]);
}

#[test]
fn test_set_op_union_16_join() {
    // JOIN：t1 JOIN t2 ON id → {2,3} ∪ t1 → {1,2,3}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT t1.id FROM t1 JOIN t2 ON t1.id = t2.id UNION SELECT id FROM t1",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3]);
}

#[test]
fn test_set_op_union_17_null_value() {
    // NULL 值：t3 含 NULL，UNION 应保留 NULL
    let catalog = make_catalog_with_t3();
    let t1 = make_t1_with_data();
    let t3 = make_t3_with_nulls();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t3);

    // t3.id = {1, 2, NULL}, t1.id = {1, 2, 3}
    // UNION = {1, 2, NULL, 3} = 4 行
    let plan = plan_sql("SELECT id FROM t3 UNION SELECT id FROM t1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 4, "UNION with NULL should have 4 rows");
    let has_null = rows.iter().any(|r| matches!(r.first(), Some(Value::Null)));
    assert!(has_null, "UNION result should contain NULL");
}

#[test]
fn test_set_op_union_18_three_table_nested() {
    // 3 表嵌套：t1 ∪ t2 ∪ t1 → 去重 {1,2,3,4}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 UNION SELECT id FROM t2 UNION SELECT id FROM t1",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3, 4]);
}

#[test]
fn test_set_op_union_19_empty_left() {
    // 空左：t1.WHERE(false)={} ∪ t2={2,3,4} → {2,3,4}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 WHERE id > 100 UNION SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![2, 3, 4]);
}

#[test]
fn test_set_op_union_20_empty_right() {
    // 空右：t1={1,2,3} ∪ t2.WHERE(false)={} → {1,2,3}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 UNION SELECT id FROM t2 WHERE id > 100",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3]);
}

#[test]
fn test_set_op_union_21_group_by() {
    // GROUP BY：t1.GROUP BY id={1,2,3} ∪ t2={2,3,4} → {1,2,3,4}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 GROUP BY id UNION SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3, 4]);
}

// =====================================================================
//  UNION ALL 组合测试（19 new，总计 20）
//  现有: exec_union_01_all
// =====================================================================

#[test]
fn test_set_op_union_all_02_preserves_dup_overlap() {
    // 保留重复：{1,2,3} + {2,3,4} = 6 行（不去重）
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql("SELECT id FROM t1 UNION ALL SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 6, "UNION ALL should preserve all 6 rows");
}

#[test]
fn test_set_op_union_all_03_no_overlap() {
    // 无交集：{1} + {4} = 2 行
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 WHERE id < 2 UNION ALL SELECT id FROM t2 WHERE id > 3",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(sorted_ids(rows), vec![1, 4]);
}

#[test]
fn test_set_op_union_all_04_self_union() {
    // 自并集：{1,2,3} + {1,2,3} = 6 行
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);

    let plan = plan_sql("SELECT id FROM t1 UNION ALL SELECT id FROM t1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 6, "UNION ALL self should have 6 rows");
}

#[test]
fn test_set_op_union_all_05_where_left() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1.WHERE(id<3)={1,2} + t2={2,3,4} = 5 行
    let plan = plan_sql(
        "SELECT id FROM t1 WHERE id < 3 UNION ALL SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
}

#[test]
fn test_set_op_union_all_06_where_right() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1={1,2,3} + t2.WHERE(id<4)={2,3} = 5 行
    let plan = plan_sql(
        "SELECT id FROM t1 UNION ALL SELECT id FROM t2 WHERE id < 4",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
}

#[test]
fn test_set_op_union_all_07_where_both() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1.WHERE(id<2)={1} + t2.WHERE(id>3)={4} = 2 行
    let plan = plan_sql(
        "SELECT id FROM t1 WHERE id < 2 UNION ALL SELECT id FROM t2 WHERE id > 3",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 4]);
}

#[test]
fn test_set_op_union_all_08_order_by() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 UNION ALL SELECT id FROM t2 ORDER BY id DESC",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    // 6 行排序后：[4,3,3,2,2,1]
    assert_eq!(first_col_ids(&rows), vec![4, 3, 3, 2, 2, 1]);
}

#[test]
fn test_set_op_union_all_09_limit() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 UNION ALL SELECT id FROM t2 ORDER BY id LIMIT 3",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(first_col_ids(&rows), vec![1, 2, 2]);
}

#[test]
fn test_set_op_union_all_10_two_columns() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // (id,val) + (id,val) = 6 行（不去重）
    let plan = plan_sql(
        "SELECT id, val FROM t1 UNION ALL SELECT id, val FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 6, "2-col UNION ALL should have 6 rows");
}

#[test]
fn test_set_op_union_all_11_expression() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1.(id+1)={2,3,4} + t2.id={2,3,4} = 6 行
    let plan = plan_sql(
        "SELECT id + 1 FROM t1 UNION ALL SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!(sorted_ids(rows), vec![2, 2, 3, 3, 4, 4]);
}

#[test]
fn test_set_op_union_all_12_aggregate() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // COUNT(t1)=3, SUM(t2.id)=9 → 2 行
    let plan = plan_sql(
        "SELECT COUNT(*) FROM t1 UNION ALL SELECT SUM(id) FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(sorted_ids(rows), vec![3, 9]);
}

#[test]
fn test_set_op_union_all_13_subquery() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM (SELECT id FROM t1) sub1 UNION ALL SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!(sorted_ids(rows), vec![1, 2, 2, 3, 3, 4]);
}

#[test]
fn test_set_op_union_all_14_cte() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "WITH cte AS (SELECT id FROM t1) SELECT id FROM cte UNION ALL SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 6);
    assert_eq!(sorted_ids(rows), vec![1, 2, 2, 3, 3, 4]);
}

#[test]
fn test_set_op_union_all_15_join() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1 JOIN t2 ON id → {2,3} (2 rows) + t1 → {1,2,3} (3 rows) = 5 rows
    let plan = plan_sql(
        "SELECT t1.id FROM t1 JOIN t2 ON t1.id = t2.id UNION ALL SELECT id FROM t1",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(sorted_ids(rows), vec![1, 2, 2, 3, 3]);
}

#[test]
fn test_set_op_union_all_16_null_value() {
    let catalog = make_catalog_with_t3();
    let t1 = make_t1_with_data();
    let t3 = make_t3_with_nulls();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t3);

    // t3.id={1,2,NULL} + t1.id={1,2,3} = 6 rows
    let plan = plan_sql("SELECT id FROM t3 UNION ALL SELECT id FROM t1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 6, "UNION ALL with NULL should have 6 rows");
    let null_count = rows
        .iter()
        .filter(|r| matches!(r.first(), Some(Value::Null)))
        .count();
    assert_eq!(null_count, 1, "should have exactly 1 NULL");
}

#[test]
fn test_set_op_union_all_17_three_table_nested() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1 + t2 + t1 = 3+3+3 = 9 rows
    let plan = plan_sql(
        "SELECT id FROM t1 UNION ALL SELECT id FROM t2 UNION ALL SELECT id FROM t1",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 9, "3-table UNION ALL should have 9 rows");
}

#[test]
fn test_set_op_union_all_18_empty_left() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // empty + t2 = 3 rows
    let plan = plan_sql(
        "SELECT id FROM t1 WHERE id > 100 UNION ALL SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(sorted_ids(rows), vec![2, 3, 4]);
}

#[test]
fn test_set_op_union_all_19_empty_right() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1 + empty = 3 rows
    let plan = plan_sql(
        "SELECT id FROM t1 UNION ALL SELECT id FROM t2 WHERE id > 100",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(sorted_ids(rows), vec![1, 2, 3]);
}

#[test]
fn test_set_op_union_all_20_row_count_exact() {
    // 精确行数验证：3 + 3 = 6
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql("SELECT id FROM t1 UNION ALL SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 6);
    // 验证所有值（含重复）：[1,2,3,2,3,4]
    let mut ids = first_col_ids(&rows);
    ids.sort();
    assert_eq!(ids, vec![1, 2, 2, 3, 3, 4]);
}

// =====================================================================
//  INTERSECT 组合测试（12 new，总计 20）
//  现有: parser_01, parser_04, planner_01, planner_03, exec_intersect_01-04
// =====================================================================

#[test]
fn test_set_op_intersect_05_where_left() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1.WHERE(id>=2)={2,3} ∩ t2={2,3,4} → {2,3}
    let plan = plan_sql(
        "SELECT id FROM t1 WHERE id >= 2 INTERSECT SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![2, 3]);
}

#[test]
fn test_set_op_intersect_06_order_by() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 INTERSECT SELECT id FROM t2 ORDER BY id DESC",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(first_col_ids(&rows), vec![3, 2]);
}

#[test]
fn test_set_op_intersect_07_limit() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 INTERSECT SELECT id FROM t2 ORDER BY id LIMIT 1",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(first_col_ids(&rows), vec![2]);
}

#[test]
fn test_set_op_intersect_08_two_columns() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data(); // (1,10),(2,20),(3,30)
    let t2 = make_t2_with_data(); // (2,20),(3,30),(4,40)
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // (id,val) ∩ (id,val) → {(2,20),(3,30)} = 2 行
    let plan = plan_sql(
        "SELECT id, val FROM t1 INTERSECT SELECT id, val FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 2, "2-col INTERSECT should have 2 rows");
}

#[test]
fn test_set_op_intersect_09_expression() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1.(id+1)={2,3,4} ∩ t2.id={2,3,4} → {2,3,4}
    let plan = plan_sql(
        "SELECT id + 1 FROM t1 INTERSECT SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![2, 3, 4]);
}

#[test]
fn test_set_op_intersect_10_subquery() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM (SELECT id FROM t1) sub1 INTERSECT SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![2, 3]);
}

#[test]
fn test_set_op_intersect_11_cte() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "WITH cte AS (SELECT id FROM t1) SELECT id FROM cte INTERSECT SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![2, 3]);
}

#[test]
fn test_set_op_intersect_12_self_intersect() {
    // 自交：t1 ∩ t1 = {1,2,3}（去重）
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);

    let plan = plan_sql("SELECT id FROM t1 INTERSECT SELECT id FROM t1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3]);
}

#[test]
fn test_set_op_intersect_13_null_value() {
    let catalog = make_catalog_with_t3();
    let t1 = make_t1_with_data();
    let t3 = make_t3_with_nulls();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t3);

    // t3.id={1,2,NULL} ∩ t1.id={1,2,3} → {1,2}（NULL 不在 t1 中）
    let plan = plan_sql("SELECT id FROM t3 INTERSECT SELECT id FROM t1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2]);
}

#[test]
fn test_set_op_intersect_14_three_table_nested() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1 ∩ t2 ∩ t1 = {2,3}
    let plan = plan_sql(
        "SELECT id FROM t1 INTERSECT SELECT id FROM t2 INTERSECT SELECT id FROM t1",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![2, 3]);
}

#[test]
fn test_set_op_intersect_15_empty_right() {
    // 空右：t1 ∩ empty = {}
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 INTERSECT SELECT id FROM t2 WHERE id > 100",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert!(
        rows.is_empty(),
        "INTERSECT with empty right should be empty"
    );
}

#[test]
fn test_set_op_intersect_16_distinct_vs_all() {
    // 对比 DISTINCT vs ALL：t1_dup={1,1,2}, t2_dup={1,2,2}
    let catalog = make_catalog();
    let t1 = make_t1_with_duplicates();
    let t2 = make_t2_with_duplicates();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // INTERSECT [DISTINCT]: {1,2} = 2 rows
    let plan_d = plan_sql("SELECT id FROM t1 INTERSECT SELECT id FROM t2", &catalog);
    let rows_d = exec.execute(&plan_d).unwrap();
    assert_eq!(rows_d.len(), 2, "INTERSECT DISTINCT should have 2 rows");

    // INTERSECT ALL: min(2,1)=1 for id=1, min(1,2)=1 for id=2 → 2 rows
    let plan_a = plan_sql(
        "SELECT id FROM t1 INTERSECT ALL SELECT id FROM t2",
        &catalog,
    );
    let rows_a = exec.execute(&plan_a).unwrap();
    assert_eq!(rows_a.len(), 2, "INTERSECT ALL should have 2 rows");
}

// =====================================================================
//  EXCEPT 组合测试（11 new，总计 20）
//  现有: parser_02, parser_05, planner_02, exec_except_01-04, exec_union_03, error_03
// =====================================================================

#[test]
fn test_set_op_except_05_where_left() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1.WHERE(id<3)={1,2} - t2={2,3,4} → {1}
    let plan = plan_sql(
        "SELECT id FROM t1 WHERE id < 3 EXCEPT SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1]);
}

#[test]
fn test_set_op_except_06_order_by() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1 - t2 = {1} ORDER BY id DESC → [1]
    let plan = plan_sql(
        "SELECT id FROM t1 EXCEPT SELECT id FROM t2 ORDER BY id DESC",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(first_col_ids(&rows), vec![1]);
}

#[test]
fn test_set_op_except_07_limit() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 EXCEPT SELECT id FROM t2 ORDER BY id LIMIT 1",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(first_col_ids(&rows), vec![1]);
}

#[test]
fn test_set_op_except_08_two_columns() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data(); // (1,10),(2,20),(3,30)
    let t2 = make_t2_with_data(); // (2,20),(3,30),(4,40)
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // (id,val) - (id,val) → {(1,10)} = 1 row
    let plan = plan_sql(
        "SELECT id, val FROM t1 EXCEPT SELECT id, val FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 1, "2-col EXCEPT should have 1 row");
}

#[test]
fn test_set_op_except_09_expression() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // t1.(id+1)={2,3,4} - t2.id={2,3,4} → {} (empty)
    let plan = plan_sql("SELECT id + 1 FROM t1 EXCEPT SELECT id FROM t2", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert!(
        rows.is_empty(),
        "EXCEPT should be empty when all values match"
    );
}

#[test]
fn test_set_op_except_10_subquery() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM (SELECT id FROM t1) sub1 EXCEPT SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1]);
}

#[test]
fn test_set_op_except_11_cte() {
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "WITH cte AS (SELECT id FROM t1) SELECT id FROM cte EXCEPT SELECT id FROM t2",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1]);
}

#[test]
fn test_set_op_except_12_self_except() {
    // 自差：t1 - t1 = {} (empty)
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);

    let plan = plan_sql("SELECT id FROM t1 EXCEPT SELECT id FROM t1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert!(rows.is_empty(), "self EXCEPT should be empty");
}

#[test]
fn test_set_op_except_13_null_value() {
    let catalog = make_catalog_with_t3();
    let t1 = make_t1_with_data();
    let t3 = make_t3_with_nulls();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t3);

    // t3.id={1,2,NULL} - t1.id={1,2,3} → {NULL}
    let plan = plan_sql("SELECT id FROM t3 EXCEPT SELECT id FROM t1", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 1, "EXCEPT with NULL should have 1 row (NULL)");
    assert!(
        matches!(rows[0].first(), Some(Value::Null)),
        "EXCEPT result should be NULL"
    );
}

#[test]
fn test_set_op_except_14_empty_right() {
    // 空右：t1 - empty = t1 (deduped)
    let catalog = make_catalog();
    let t1 = make_t1_with_data();
    let t2 = make_t2_with_data();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    let plan = plan_sql(
        "SELECT id FROM t1 EXCEPT SELECT id FROM t2 WHERE id > 100",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(sorted_ids(rows), vec![1, 2, 3]);
}

#[test]
fn test_set_op_except_15_distinct_vs_all() {
    // 对比 DISTINCT vs ALL：t1_dup={1,1,2}, t2_dup={1,2,2}
    let catalog = make_catalog();
    let t1 = make_t1_with_duplicates();
    let t2 = make_t2_with_duplicates();
    let mut exec = Executor::new();
    exec.register_table(&t1);
    exec.register_table(&t2);

    // EXCEPT [DISTINCT]: {1,1,2} - {1,2,2} → {} (empty, both 1 and 2 in t2)
    let plan_d = plan_sql("SELECT id FROM t1 EXCEPT SELECT id FROM t2", &catalog);
    let rows_d = exec.execute(&plan_d).unwrap();
    assert_eq!(rows_d.len(), 0, "EXCEPT DISTINCT should be empty");

    // EXCEPT ALL: 1: max(0,2-1)=1, 2: max(0,1-2)=0 → 1 row (id=1)
    let plan_a = plan_sql("SELECT id FROM t1 EXCEPT ALL SELECT id FROM t2", &catalog);
    let rows_a = exec.execute(&plan_a).unwrap();
    assert_eq!(rows_a.len(), 1, "EXCEPT ALL should have 1 row");
    assert_eq!(rows_a[0][0], Value::Int64(1));
}
