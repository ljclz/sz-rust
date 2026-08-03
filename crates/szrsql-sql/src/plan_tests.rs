//! Phase 3.2 单元测试 — Planner 完整覆盖。
//!
//! 覆盖范围：
//! - SELECT（13 条）：投影 / WHERE / JOIN / GROUP BY / ORDER BY / LIMIT / DISTINCT / 子查询
//! - INSERT（11 条）：VALUES / 多行 / 显式列 / DEFAULT / SELECT / 错误场景
//! - UPDATE（11 条）：SET / WHERE / 多列 / 表达式 / 别名 / 错误场景
//! - DELETE（10 条）：全表 / WHERE / 复杂条件 / 别名 / 错误场景
//! - DDL（10 条）：CREATE/DROP TABLE / CREATE/DROP INDEX / 错误场景
//! - 事务控制（6 条）：BEGIN / COMMIT / ROLLBACK / SAVEPOINT / RELEASE / SET TRANSACTION
//! - EXPLAIN（1 条）

use crate::ast::*;
use crate::parser::parse_one;
use crate::plan::{InMemoryCatalog, LogicalPlan, PlanError, Planner};
use szrsql_types::value::ColumnType;

// =====================================================================
//  测试辅助
// =====================================================================

/// 创建测试 catalog：
/// - t1(id INT, name TEXT, age INT)
/// - t2(id INT, t1_id INT, value INT)
/// - users(uid INT, name TEXT)
fn make_catalog() -> InMemoryCatalog {
    let mut cat = InMemoryCatalog::new();
    cat.add_simple_table(
        "t1",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
        ],
    );
    cat.add_simple_table(
        "t2",
        vec![
            ("id", ColumnType::Int64),
            ("t1_id", ColumnType::Int64),
            ("value", ColumnType::Int64),
        ],
    );
    cat.add_simple_table(
        "users",
        vec![("uid", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    cat
}

/// 解析 + 计划一条 SQL
fn plan_one(sql: &str, cat: &InMemoryCatalog) -> Result<LogicalPlan, PlanError> {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(cat);
    planner.plan_statement(stmt)
}

/// 解析 + 计划一条 SQL，断言成功
fn must_plan(sql: &str, cat: &InMemoryCatalog) -> LogicalPlan {
    match plan_one(sql, cat) {
        Ok(p) => p,
        Err(e) => panic!("plan failed for SQL: {sql}\nerror: {e:?}"),
    }
}

/// 解析 + 计划一条 SQL，断言失败并返回错误
fn must_fail(sql: &str, cat: &InMemoryCatalog) -> PlanError {
    match plan_one(sql, cat) {
        Ok(p) => panic!("expected plan failure, got: {p:#?}"),
        Err(e) => e,
    }
}

// =====================================================================
//  SELECT 测试（13 条）
// =====================================================================

#[test]
fn test_select_star() {
    let cat = make_catalog();
    let plan = must_plan("SELECT * FROM t1", &cat);
    match plan {
        LogicalPlan::Projection { input, .. } => {
            assert!(matches!(*input, LogicalPlan::Scan { .. }));
        }
        other => panic!("expected Projection, got {other:#?}"),
    }
}

#[test]
fn test_select_columns() {
    let cat = make_catalog();
    let plan = must_plan("SELECT id, name FROM t1", &cat);
    match plan {
        LogicalPlan::Projection {
            output_names,
            input,
            ..
        } => {
            assert_eq!(output_names, vec!["id", "name"]);
            assert!(matches!(*input, LogicalPlan::Scan { .. }));
        }
        other => panic!("expected Projection, got {other:#?}"),
    }
}

#[test]
fn test_select_with_alias() {
    let cat = make_catalog();
    let plan = must_plan("SELECT id AS user_id, name FROM t1", &cat);
    match plan {
        LogicalPlan::Projection { output_names, .. } => {
            assert_eq!(output_names, vec!["user_id", "name"]);
        }
        other => panic!("expected Projection, got {other:#?}"),
    }
}

#[test]
fn test_select_where() {
    let cat = make_catalog();
    let plan = must_plan("SELECT id FROM t1 WHERE id > 10", &cat);
    match plan {
        LogicalPlan::Projection { input, .. } => {
            assert!(matches!(*input, LogicalPlan::Filter { .. }));
        }
        other => panic!("expected Projection, got {other:#?}"),
    }
}

#[test]
fn test_select_inner_join() {
    let cat = make_catalog();
    let plan = must_plan(
        "SELECT t1.id, t2.value FROM t1 JOIN t2 ON t1.id = t2.t1_id",
        &cat,
    );
    match plan {
        LogicalPlan::Projection { input, .. } => match *input {
            LogicalPlan::Join { join_type, .. } => {
                assert_eq!(join_type, JoinType::Inner);
            }
            other => panic!("expected Join, got {other:#?}"),
        },
        other => panic!("expected Projection, got {other:#?}"),
    }
}

#[test]
fn test_select_left_join() {
    let cat = make_catalog();
    let plan = must_plan(
        "SELECT t1.id FROM t1 LEFT JOIN t2 ON t1.id = t2.t1_id",
        &cat,
    );
    match plan {
        LogicalPlan::Projection { input, .. } => match *input {
            LogicalPlan::Join { join_type, .. } => {
                assert_eq!(join_type, JoinType::LeftOuter);
            }
            other => panic!("expected Join, got {other:#?}"),
        },
        other => panic!("expected Projection, got {other:#?}"),
    }
}

#[test]
fn test_select_group_by_with_aggregate() {
    let cat = make_catalog();
    let plan = must_plan("SELECT id, COUNT(*) FROM t1 GROUP BY id", &cat);
    match plan {
        LogicalPlan::Projection { input, .. } => match *input {
            LogicalPlan::Aggregate {
                grouping_sets,
                aggregates,
                ..
            } => {
                // P3-1: 普通 GROUP BY 包装为单分组集
                assert_eq!(grouping_sets.len(), 1);
                assert_eq!(grouping_sets[0].len(), 1);
                assert_eq!(aggregates.len(), 1);
                assert_eq!(aggregates[0].func_name, "count");
            }
            other => panic!("expected Aggregate, got {other:#?}"),
        },
        other => panic!("expected Projection, got {other:#?}"),
    }
}

#[test]
fn test_select_having() {
    let cat = make_catalog();
    let plan = must_plan(
        "SELECT id, COUNT(*) FROM t1 GROUP BY id HAVING COUNT(*) > 1",
        &cat,
    );
    match plan {
        LogicalPlan::Projection { input, .. } => match *input {
            LogicalPlan::Aggregate { having, .. } => {
                assert!(having.is_some());
            }
            other => panic!("expected Aggregate, got {other:#?}"),
        },
        other => panic!("expected Projection, got {other:#?}"),
    }
}

#[test]
fn test_select_order_by() {
    let cat = make_catalog();
    let plan = must_plan("SELECT id FROM t1 ORDER BY id DESC", &cat);
    match plan {
        LogicalPlan::Sort { order_by, .. } => {
            assert_eq!(order_by.len(), 1);
            assert!(!order_by[0].asc);
        }
        other => panic!("expected Sort, got {other:#?}"),
    }
}

#[test]
fn test_select_limit_offset() {
    let cat = make_catalog();
    let plan = must_plan("SELECT id FROM t1 LIMIT 10 OFFSET 5", &cat);
    match plan {
        LogicalPlan::Limit { limit, offset, .. } => {
            assert!(limit.is_some());
            assert!(offset.is_some());
        }
        other => panic!("expected Limit, got {other:#?}"),
    }
}

#[test]
fn test_select_distinct() {
    let cat = make_catalog();
    let plan = must_plan("SELECT DISTINCT id FROM t1", &cat);
    match plan {
        LogicalPlan::Distinct { input, .. } => {
            assert!(matches!(*input, LogicalPlan::Projection { .. }));
        }
        other => panic!("expected Distinct, got {other:#?}"),
    }
}

#[test]
fn test_select_qualified_wildcard() {
    let cat = make_catalog();
    let plan = must_plan("SELECT t1.* FROM t1", &cat);
    match plan {
        LogicalPlan::Projection { output_names, .. } => {
            assert_eq!(output_names, vec!["id", "name", "age"]);
        }
        other => panic!("expected Projection, got {other:#?}"),
    }
}

#[test]
fn test_select_subquery_in_from() {
    let cat = make_catalog();
    let plan = must_plan("SELECT x FROM (SELECT id AS x FROM t1) AS sub", &cat);
    match plan {
        LogicalPlan::Projection { output_names, .. } => {
            assert_eq!(output_names, vec!["x"]);
        }
        other => panic!("expected Projection, got {other:#?}"),
    }
}

// =====================================================================
//  INSERT 测试（11 条）
// =====================================================================

#[test]
fn test_insert_values_single_row() {
    let cat = make_catalog();
    let plan = must_plan("INSERT INTO t1 VALUES (1, 'a', 20)", &cat);
    match plan {
        LogicalPlan::Insert {
            table,
            columns,
            source,
            ..
        } => {
            assert_eq!(table.name, "t1");
            assert!(columns.is_none());
            match source {
                crate::plan::InsertSourcePlan::Values(rows) => {
                    assert_eq!(rows.len(), 1);
                    assert_eq!(rows[0].len(), 3);
                }
                other => panic!("expected Values, got {other:#?}"),
            }
        }
        other => panic!("expected Insert, got {other:#?}"),
    }
}

#[test]
fn test_insert_with_columns() {
    let cat = make_catalog();
    let plan = must_plan("INSERT INTO t1 (id, name) VALUES (1, 'a')", &cat);
    match plan {
        LogicalPlan::Insert { columns, .. } => {
            assert_eq!(columns, Some(vec!["id".to_string(), "name".to_string()]));
        }
        other => panic!("expected Insert, got {other:#?}"),
    }
}

#[test]
fn test_insert_values_multiple_rows() {
    let cat = make_catalog();
    let plan = must_plan(
        "INSERT INTO t1 (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')",
        &cat,
    );
    match plan {
        LogicalPlan::Insert { source, .. } => match source {
            crate::plan::InsertSourcePlan::Values(rows) => {
                assert_eq!(rows.len(), 3);
            }
            other => panic!("expected Values, got {other:#?}"),
        },
        other => panic!("expected Insert, got {other:#?}"),
    }
}

#[test]
fn test_insert_default_values() {
    let cat = make_catalog();
    let plan = must_plan("INSERT INTO t1 DEFAULT VALUES", &cat);
    match plan {
        LogicalPlan::Insert { source, .. } => {
            assert!(matches!(
                source,
                crate::plan::InsertSourcePlan::DefaultValues
            ));
        }
        other => panic!("expected Insert, got {other:#?}"),
    }
}

#[test]
fn test_insert_select() {
    let cat = make_catalog();
    let plan = must_plan("INSERT INTO t2 (id, t1_id) SELECT id, id FROM t1", &cat);
    match plan {
        LogicalPlan::Insert { source, .. } => {
            assert!(matches!(source, crate::plan::InsertSourcePlan::Select(_)));
        }
        other => panic!("expected Insert, got {other:#?}"),
    }
}

#[test]
fn test_insert_table_not_found() {
    let cat = make_catalog();
    let err = must_fail("INSERT INTO nonexistent VALUES (1)", &cat);
    assert!(matches!(err, PlanError::TableNotFound(_)));
}

#[test]
fn test_insert_column_not_found() {
    let cat = make_catalog();
    let err = must_fail("INSERT INTO t1 (nonexistent_col) VALUES (1)", &cat);
    assert!(matches!(err, PlanError::ColumnNotFound(_)));
}

#[test]
fn test_insert_column_count_mismatch_too_many() {
    let cat = make_catalog();
    let err = must_fail("INSERT INTO t1 (id, name) VALUES (1, 'a', 99)", &cat);
    assert!(matches!(err, PlanError::InvalidExpression(_)));
}

#[test]
fn test_insert_column_count_mismatch_too_few() {
    let cat = make_catalog();
    let err = must_fail("INSERT INTO t1 (id, name, age) VALUES (1, 'a')", &cat);
    assert!(matches!(err, PlanError::InvalidExpression(_)));
}

#[test]
fn test_insert_full_row_no_columns_match_count() {
    let cat = make_catalog();
    // t1 有 3 列，给 3 个值
    let plan = must_plan("INSERT INTO t1 VALUES (1, 'a', 20)", &cat);
    assert!(matches!(plan, LogicalPlan::Insert { .. }));
}

#[test]
fn test_insert_full_row_wrong_count() {
    let cat = make_catalog();
    // t1 有 3 列，给 2 个值
    let err = must_fail("INSERT INTO t1 VALUES (1, 'a')", &cat);
    assert!(matches!(err, PlanError::InvalidExpression(_)));
}

// =====================================================================
//  UPDATE 测试（11 条）
// =====================================================================

#[test]
fn test_update_simple() {
    let cat = make_catalog();
    let plan = must_plan("UPDATE t1 SET name = 'x'", &cat);
    match plan {
        LogicalPlan::Update {
            table,
            assignments,
            source,
            ..
        } => {
            assert_eq!(table.name, "t1");
            assert_eq!(assignments.len(), 1);
            assert_eq!(assignments[0].column, "name");
            assert!(source.is_none()); // 无 WHERE
        }
        other => panic!("expected Update, got {other:#?}"),
    }
}

#[test]
fn test_update_where() {
    let cat = make_catalog();
    let plan = must_plan("UPDATE t1 SET name = 'x' WHERE id = 1", &cat);
    match plan {
        LogicalPlan::Update { source, .. } => {
            assert!(source.is_some());
            match *source.unwrap() {
                LogicalPlan::Filter { input, .. } => {
                    assert!(matches!(*input, LogicalPlan::Scan { .. }));
                }
                other => panic!("expected Filter, got {other:#?}"),
            }
        }
        other => panic!("expected Update, got {other:#?}"),
    }
}

#[test]
fn test_update_multiple_set() {
    let cat = make_catalog();
    let plan = must_plan("UPDATE t1 SET name = 'x', age = 20 WHERE id = 1", &cat);
    match plan {
        LogicalPlan::Update { assignments, .. } => {
            assert_eq!(assignments.len(), 2);
            assert_eq!(assignments[0].column, "name");
            assert_eq!(assignments[1].column, "age");
        }
        other => panic!("expected Update, got {other:#?}"),
    }
}

#[test]
fn test_update_with_expression() {
    let cat = make_catalog();
    let plan = must_plan("UPDATE t1 SET age = age + 1 WHERE id = 1", &cat);
    match plan {
        LogicalPlan::Update { assignments, .. } => {
            assert_eq!(assignments.len(), 1);
            assert!(matches!(
                assignments[0].value,
                Expr::BinaryOp {
                    op: BinaryOp::Plus,
                    ..
                }
            ));
        }
        other => panic!("expected Update, got {other:#?}"),
    }
}

#[test]
fn test_update_with_alias() {
    let cat = make_catalog();
    let plan = must_plan("UPDATE t1 AS a SET name = 'x' WHERE a.id = 1", &cat);
    match plan {
        LogicalPlan::Update { source, .. } => match *source.unwrap() {
            LogicalPlan::Filter { input, .. } => match *input {
                LogicalPlan::Scan { alias, .. } => {
                    assert_eq!(alias, Some("a".to_string()));
                }
                other => panic!("expected Scan, got {other:#?}"),
            },
            other => panic!("expected Filter, got {other:#?}"),
        },
        other => panic!("expected Update, got {other:#?}"),
    }
}

#[test]
fn test_update_complex_where() {
    let cat = make_catalog();
    let plan = must_plan(
        "UPDATE t1 SET name = 'x' WHERE id > 0 AND (age < 100 OR name IS NULL)",
        &cat,
    );
    assert!(matches!(plan, LogicalPlan::Update { .. }));
}

#[test]
fn test_update_subquery_in_where() {
    let cat = make_catalog();
    let plan = must_plan(
        "UPDATE t1 SET name = 'x' WHERE id IN (SELECT t1_id FROM t2)",
        &cat,
    );
    assert!(matches!(plan, LogicalPlan::Update { .. }));
}

#[test]
fn test_update_table_not_found() {
    let cat = make_catalog();
    let err = must_fail("UPDATE nonexistent SET x = 1", &cat);
    assert!(matches!(err, PlanError::TableNotFound(_)));
}

#[test]
fn test_update_column_not_found() {
    let cat = make_catalog();
    let err = must_fail("UPDATE t1 SET nonexistent_col = 1", &cat);
    assert!(matches!(err, PlanError::ColumnNotFound(_)));
}

#[test]
fn test_update_no_where_updates_all() {
    let cat = make_catalog();
    let plan = must_plan("UPDATE t1 SET age = 0", &cat);
    match plan {
        LogicalPlan::Update { source, .. } => {
            assert!(source.is_none()); // 无 WHERE = 全表更新
        }
        other => panic!("expected Update, got {other:#?}"),
    }
}

#[test]
fn test_update_set_with_function() {
    let cat = make_catalog();
    let plan = must_plan("UPDATE t1 SET name = upper(name) WHERE id = 1", &cat);
    match plan {
        LogicalPlan::Update { assignments, .. } => {
            assert!(matches!(assignments[0].value, Expr::Function { .. }));
        }
        other => panic!("expected Update, got {other:#?}"),
    }
}

// =====================================================================
//  DELETE 测试（10 条）
// =====================================================================

#[test]
fn test_delete_all() {
    let cat = make_catalog();
    let plan = must_plan("DELETE FROM t1", &cat);
    match plan {
        LogicalPlan::Delete { table, source, .. } => {
            assert_eq!(table.name, "t1");
            assert!(source.is_none()); // 无 WHERE = 删除全部
        }
        other => panic!("expected Delete, got {other:#?}"),
    }
}

#[test]
fn test_delete_where() {
    let cat = make_catalog();
    let plan = must_plan("DELETE FROM t1 WHERE id = 1", &cat);
    match plan {
        LogicalPlan::Delete { source, .. } => {
            assert!(source.is_some());
            assert!(matches!(*source.unwrap(), LogicalPlan::Filter { .. }));
        }
        other => panic!("expected Delete, got {other:#?}"),
    }
}

#[test]
fn test_delete_with_alias() {
    let cat = make_catalog();
    let plan = must_plan("DELETE FROM t1 AS a WHERE a.id = 1", &cat);
    match plan {
        LogicalPlan::Delete { source, .. } => match *source.unwrap() {
            LogicalPlan::Filter { input, .. } => match *input {
                LogicalPlan::Scan { alias, .. } => {
                    assert_eq!(alias, Some("a".to_string()));
                }
                other => panic!("expected Scan, got {other:#?}"),
            },
            other => panic!("expected Filter, got {other:#?}"),
        },
        other => panic!("expected Delete, got {other:#?}"),
    }
}

#[test]
fn test_delete_complex_where() {
    let cat = make_catalog();
    let plan = must_plan(
        "DELETE FROM t1 WHERE id > 0 AND age < 100 OR name IS NULL",
        &cat,
    );
    assert!(matches!(plan, LogicalPlan::Delete { .. }));
}

#[test]
fn test_delete_with_in_subquery() {
    let cat = make_catalog();
    let plan = must_plan("DELETE FROM t1 WHERE id IN (SELECT t1_id FROM t2)", &cat);
    assert!(matches!(plan, LogicalPlan::Delete { .. }));
}

#[test]
fn test_delete_with_between() {
    let cat = make_catalog();
    let plan = must_plan("DELETE FROM t1 WHERE age BETWEEN 18 AND 65", &cat);
    assert!(matches!(plan, LogicalPlan::Delete { .. }));
}

#[test]
fn test_delete_with_like() {
    let cat = make_catalog();
    let plan = must_plan("DELETE FROM t1 WHERE name LIKE 'prefix%'", &cat);
    assert!(matches!(plan, LogicalPlan::Delete { .. }));
}

#[test]
fn test_delete_with_or_and() {
    let cat = make_catalog();
    let plan = must_plan("DELETE FROM t1 WHERE id = 1 OR id = 2 OR id = 3", &cat);
    assert!(matches!(plan, LogicalPlan::Delete { .. }));
}

#[test]
fn test_delete_table_not_found() {
    let cat = make_catalog();
    let err = must_fail("DELETE FROM nonexistent", &cat);
    assert!(matches!(err, PlanError::TableNotFound(_)));
}

#[test]
fn test_delete_with_not_condition() {
    let cat = make_catalog();
    let plan = must_plan("DELETE FROM t1 WHERE NOT (id = 1)", &cat);
    assert!(matches!(plan, LogicalPlan::Delete { .. }));
}

// =====================================================================
//  DDL 测试（10 条）
// =====================================================================

#[test]
fn test_create_table() {
    let cat = make_catalog();
    let plan = must_plan(
        "CREATE TABLE new_t (id INT PRIMARY KEY, name TEXT NOT NULL)",
        &cat,
    );
    match plan {
        LogicalPlan::CreateTable { name, columns, .. } => {
            assert_eq!(name.name, "new_t");
            assert_eq!(columns.len(), 2);
        }
        other => panic!("expected CreateTable, got {other:#?}"),
    }
}

#[test]
fn test_create_table_if_not_exists() {
    let cat = make_catalog();
    // t1 已存在，但 IF NOT EXISTS 应允许通过
    let plan = must_plan("CREATE TABLE IF NOT EXISTS t1 (id INT)", &cat);
    assert!(matches!(plan, LogicalPlan::CreateTable { .. }));
}

#[test]
fn test_create_table_already_exists() {
    let cat = make_catalog();
    // t1 已存在，无 IF NOT EXISTS 应报错
    let err = must_fail("CREATE TABLE t1 (id INT)", &cat);
    assert!(matches!(err, PlanError::TableAlreadyExists(_)));
}

#[test]
fn test_drop_table() {
    let cat = make_catalog();
    let plan = must_plan("DROP TABLE t1", &cat);
    match plan {
        LogicalPlan::DropTable {
            names,
            if_exists,
            cascade,
        } => {
            assert_eq!(names.len(), 1);
            assert!(!if_exists);
            assert!(!cascade);
        }
        other => panic!("expected DropTable, got {other:#?}"),
    }
}

#[test]
fn test_drop_table_if_exists() {
    let cat = make_catalog();
    let plan = must_plan("DROP TABLE IF EXISTS nonexistent", &cat);
    assert!(matches!(
        plan,
        LogicalPlan::DropTable {
            if_exists: true,
            ..
        }
    ));
}

#[test]
fn test_drop_table_not_found() {
    let cat = make_catalog();
    let err = must_fail("DROP TABLE nonexistent", &cat);
    assert!(matches!(err, PlanError::TableNotFound(_)));
}

#[test]
fn test_drop_table_cascade() {
    let cat = make_catalog();
    let plan = must_plan("DROP TABLE t1 CASCADE", &cat);
    match plan {
        LogicalPlan::DropTable { cascade, .. } => assert!(cascade),
        other => panic!("expected DropTable, got {other:#?}"),
    }
}

#[test]
fn test_create_index() {
    let cat = make_catalog();
    let plan = must_plan("CREATE INDEX idx_name ON t1 (name)", &cat);
    match plan {
        LogicalPlan::CreateIndex {
            name,
            table,
            columns,
            unique,
            ..
        } => {
            assert_eq!(name, Some("idx_name".to_string()));
            assert_eq!(table.name, "t1");
            assert_eq!(columns.len(), 1);
            assert!(!unique);
        }
        other => panic!("expected CreateIndex, got {other:#?}"),
    }
}

#[test]
fn test_create_unique_index() {
    let cat = make_catalog();
    let plan = must_plan("CREATE UNIQUE INDEX idx_id ON t1 (id)", &cat);
    match plan {
        LogicalPlan::CreateIndex { unique, .. } => assert!(unique),
        other => panic!("expected CreateIndex, got {other:#?}"),
    }
}

#[test]
fn test_create_index_table_not_found() {
    let cat = make_catalog();
    let err = must_fail("CREATE INDEX idx ON nonexistent (col)", &cat);
    assert!(matches!(err, PlanError::TableNotFound(_)));
}

#[test]
fn test_drop_index() {
    let cat = make_catalog();
    let plan = must_plan("DROP INDEX idx_name", &cat);
    match plan {
        LogicalPlan::DropIndex { names, if_exists } => {
            assert_eq!(names, vec!["idx_name".to_string()]);
            assert!(!if_exists);
        }
        other => panic!("expected DropIndex, got {other:#?}"),
    }
}

// =====================================================================
//  事务控制测试（6 条）
// =====================================================================

#[test]
fn test_begin() {
    let cat = make_catalog();
    let plan = must_plan("BEGIN", &cat);
    assert!(matches!(plan, LogicalPlan::Empty));
}

#[test]
fn test_commit() {
    let cat = make_catalog();
    let plan = must_plan("COMMIT", &cat);
    assert!(matches!(plan, LogicalPlan::Empty));
}

#[test]
fn test_rollback() {
    let cat = make_catalog();
    let plan = must_plan("ROLLBACK", &cat);
    assert!(matches!(plan, LogicalPlan::Empty));
}

#[test]
fn test_savepoint() {
    let cat = make_catalog();
    let plan = must_plan("SAVEPOINT sp1", &cat);
    assert!(matches!(plan, LogicalPlan::Empty));
}

#[test]
fn test_release_savepoint() {
    let cat = make_catalog();
    let plan = must_plan("RELEASE SAVEPOINT sp1", &cat);
    assert!(matches!(plan, LogicalPlan::Empty));
}

#[test]
fn test_set_transaction() {
    let cat = make_catalog();
    let plan = must_plan("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE", &cat);
    assert!(matches!(plan, LogicalPlan::Empty));
}

// =====================================================================
//  EXPLAIN 测试（1 条）
// =====================================================================

#[test]
fn test_explain() {
    let cat = make_catalog();
    // EXPLAIN 透传内部计划
    let plan = must_plan("EXPLAIN SELECT id FROM t1", &cat);
    // 内部是 SELECT 计划
    assert!(matches!(plan, LogicalPlan::Projection { .. }));
}

// =====================================================================
//  跨语句综合测试（验证 DML 序列）
// =====================================================================

#[test]
fn test_dml_sequence_insert_update_delete() {
    let cat = make_catalog();
    // INSERT → UPDATE → DELETE 序列都能正确计划
    let insert_plan = must_plan("INSERT INTO t1 VALUES (1, 'a', 20)", &cat);
    assert!(matches!(insert_plan, LogicalPlan::Insert { .. }));

    let update_plan = must_plan("UPDATE t1 SET age = 21 WHERE id = 1", &cat);
    assert!(matches!(update_plan, LogicalPlan::Update { .. }));

    let delete_plan = must_plan("DELETE FROM t1 WHERE id = 1", &cat);
    assert!(matches!(delete_plan, LogicalPlan::Delete { .. }));
}

#[test]
fn test_create_select_drop_sequence() {
    let cat = make_catalog();
    // CREATE TABLE → SELECT → DROP TABLE 序列
    let create_plan = must_plan("CREATE TABLE temp_t (id INT)", &cat);
    assert!(matches!(create_plan, LogicalPlan::CreateTable { .. }));

    // 注意：temp_t 不在 catalog 中，所以 SELECT/DROP 会失败
    // 这里只验证已存在的表可以正常工作
    let select_plan = must_plan("SELECT id FROM t1", &cat);
    assert!(matches!(select_plan, LogicalPlan::Projection { .. }));

    let drop_plan = must_plan("DROP TABLE t1", &cat);
    assert!(matches!(drop_plan, LogicalPlan::DropTable { .. }));
}

// =====================================================================
//  错误信息可读性测试
// =====================================================================

#[test]
fn test_table_not_found_error_message() {
    let cat = make_catalog();
    let err = must_fail("SELECT * FROM nonexistent_table", &cat);
    let msg = format!("{err}");
    assert!(msg.contains("nonexistent_table"), "msg = {msg}");
}

#[test]
fn test_column_not_found_error_message() {
    let cat = make_catalog();
    let err = must_fail("INSERT INTO t1 (nonexistent_col) VALUES (1)", &cat);
    let msg = format!("{err}");
    assert!(msg.contains("nonexistent_col"), "msg = {msg}");
}
