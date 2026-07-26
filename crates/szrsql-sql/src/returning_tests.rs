//! Phase 3.21 单元测试 — RETURNING 子句。
//!
//! 覆盖类别：
//! - Parser（3 条）：INSERT/UPDATE/DELETE RETURNING 解析
//! - Planner（3 条）：returning 字段透传到 LogicalPlan
//! - INSERT RETURNING（6 条）：通配符 / 单列 / 多列 / 表达式 / 别名 / 多行 VALUES
//! - UPDATE RETURNING（5 条）：通配符 / 单列 / 多列 / 表达式 / WHERE 过滤
//! - DELETE RETURNING（5 条）：通配符 / 单列 / 多列 / 表达式 / WHERE 过滤
//! - 端到端（3 条）：PG 示例 INSERT RETURNING * / UPDATE RETURNING id, x / DELETE RETURNING id
//! - 无 RETURNING（2 条）：INSERT/UPDATE/DELETE 无 RETURNING 时 returning_rows 为空
//!
//! 共 27 个测试用例。

use super::executor::{DmlResult, Executor, InMemoryTable, MutableTable, TableStorage};
use crate::ast::*;
use crate::parser::parse_one;
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner, TableSchema};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建带主键 `id` 的 catalog 表
fn make_catalog_with_pk_table() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let mut id_col = ColumnDefinition::new("id", ColumnType::Int64);
    id_col.primary_key = true;
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    catalog.add_table(TableSchema {
        name: TableName::new("users"),
        columns: vec![id_col, name_col],
    });
    catalog
}

/// 创建带主键 `id` 的内存表（与 catalog schema 对齐）
fn make_pk_table() -> InMemoryTable {
    let mut id_col = ColumnDefinition::new("id", ColumnType::Int64);
    id_col.primary_key = true;
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    InMemoryTable::new(TableSchema {
        name: TableName::new("users"),
        columns: vec![id_col, name_col],
    })
}

/// SQL → AST → LogicalPlan（断言成功）
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).expect("plan failed")
}

// =====================================================================
//  Parser 测试（3 条）
// =====================================================================

#[test]
fn test_returning_parser_01_insert() {
    let stmt = parse_one("INSERT INTO t VALUES (1, 'a') RETURNING *").unwrap();
    match stmt {
        Statement::Insert {
            returning: Some(items),
            ..
        } => {
            assert_eq!(items.len(), 1);
            assert!(matches!(items[0], SelectItem::Wildcard));
        }
        other => panic!("expected Insert with returning, got {other:?}"),
    }
}

#[test]
fn test_returning_parser_02_update() {
    let stmt = parse_one("UPDATE t SET x = 1 WHERE id = 1 RETURNING id, x").unwrap();
    match stmt {
        Statement::Update {
            returning: Some(items),
            ..
        } => {
            assert_eq!(items.len(), 2);
        }
        other => panic!("expected Update with returning, got {other:?}"),
    }
}

#[test]
fn test_returning_parser_03_delete() {
    let stmt = parse_one("DELETE FROM t WHERE id = 1 RETURNING id").unwrap();
    match stmt {
        Statement::Delete {
            returning: Some(items),
            ..
        } => {
            assert_eq!(items.len(), 1);
        }
        other => panic!("expected Delete with returning, got {other:?}"),
    }
}

// =====================================================================
//  Planner 测试（3 条）
// =====================================================================

#[test]
fn test_returning_planner_01_insert_returning() {
    let catalog = make_catalog_with_pk_table();
    let plan = plan_sql("INSERT INTO users VALUES (1, 'a') RETURNING *", &catalog);
    match plan {
        LogicalPlan::Insert {
            returning: Some(_), ..
        } => { /* OK */ }
        other => panic!("expected Insert with returning=Some, got {other:?}"),
    }
}

#[test]
fn test_returning_planner_02_update_returning() {
    let catalog = make_catalog_with_pk_table();
    let plan = plan_sql(
        "UPDATE users SET name = 'b' WHERE id = 1 RETURNING id, name",
        &catalog,
    );
    match plan {
        LogicalPlan::Update {
            returning: Some(_), ..
        } => { /* OK */ }
        other => panic!("expected Update with returning=Some, got {other:?}"),
    }
}

#[test]
fn test_returning_planner_03_delete_returning() {
    let catalog = make_catalog_with_pk_table();
    let plan = plan_sql("DELETE FROM users WHERE id = 1 RETURNING id", &catalog);
    match plan {
        LogicalPlan::Delete {
            returning: Some(_), ..
        } => { /* OK */ }
        other => panic!("expected Delete with returning=Some, got {other:?}"),
    }
}

// =====================================================================
//  INSERT RETURNING 测试（6 条）
// =====================================================================

#[test]
fn test_returning_insert_01_wildcard() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();

    let plan = plan_sql("INSERT INTO users VALUES (1, 'a') RETURNING *", &catalog);
    let exec = Executor::new();
    let result: DmlResult = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(
        result.returning_rows[0],
        vec![Value::Int64(1), Value::Text("a".into())]
    );
}

#[test]
fn test_returning_insert_02_single_column() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();

    let plan = plan_sql("INSERT INTO users VALUES (1, 'a') RETURNING id", &catalog);
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(result.returning_rows[0], vec![Value::Int64(1)]);
}

#[test]
fn test_returning_insert_03_multiple_columns() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();

    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a') RETURNING id, name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(
        result.returning_rows[0],
        vec![Value::Int64(1), Value::Text("a".into())]
    );
}

#[test]
fn test_returning_insert_04_expression() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();

    // RETURNING id + 1 — 表达式求值
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a') RETURNING id + 1",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(result.returning_rows[0], vec![Value::Int64(2)]);
}

#[test]
fn test_returning_insert_05_alias() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();

    // RETURNING id AS user_id — 别名（结果行忽略别名，仅返回值）
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a') RETURNING id AS user_id",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(result.returning_rows[0], vec![Value::Int64(1)]);
}

#[test]
fn test_returning_insert_06_multi_rows() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();

    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a'), (2, 'b'), (3, 'c') RETURNING id, name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 3);
    assert_eq!(result.returning_rows.len(), 3);
    assert_eq!(
        result.returning_rows[0],
        vec![Value::Int64(1), Value::Text("a".into())]
    );
    assert_eq!(
        result.returning_rows[1],
        vec![Value::Int64(2), Value::Text("b".into())]
    );
    assert_eq!(
        result.returning_rows[2],
        vec![Value::Int64(3), Value::Text("c".into())]
    );
}

// =====================================================================
//  UPDATE RETURNING 测试（5 条）
// =====================================================================

#[test]
fn test_returning_update_01_wildcard() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("old".into())]);

    let plan = plan_sql(
        "UPDATE users SET name = 'new' WHERE id = 1 RETURNING *",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    // UPDATE RETURNING 返回更新后的新行
    assert_eq!(
        result.returning_rows[0],
        vec![Value::Int64(1), Value::Text("new".into())]
    );
}

#[test]
fn test_returning_update_02_single_column() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("old".into())]);

    let plan = plan_sql(
        "UPDATE users SET name = 'new' WHERE id = 1 RETURNING name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(result.returning_rows[0], vec![Value::Text("new".into())]);
}

#[test]
fn test_returning_update_03_multiple_columns() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("old".into())]);

    let plan = plan_sql(
        "UPDATE users SET name = 'new' WHERE id = 1 RETURNING id, name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(
        result.returning_rows[0],
        vec![Value::Int64(1), Value::Text("new".into())]
    );
}

#[test]
fn test_returning_update_04_expression() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("old".into())]);

    // RETURNING id * 2 — 表达式
    let plan = plan_sql(
        "UPDATE users SET name = 'new' WHERE id = 1 RETURNING id * 2",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(result.returning_rows[0], vec![Value::Int64(2)]);
}

#[test]
fn test_returning_update_05_where_filter() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);
    table.insert_row(vec![Value::Int64(2), Value::Text("b".into())]);
    table.insert_row(vec![Value::Int64(3), Value::Text("c".into())]);

    // 只更新 id >= 2 的行，RETURNING 返回 2 行
    let plan = plan_sql(
        "UPDATE users SET name = 'X' WHERE id >= 2 RETURNING id",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 2);
    assert_eq!(result.returning_rows.len(), 2);
    assert_eq!(result.returning_rows[0], vec![Value::Int64(2)]);
    assert_eq!(result.returning_rows[1], vec![Value::Int64(3)]);
}

// =====================================================================
//  DELETE RETURNING 测试（5 条）
// =====================================================================

#[test]
fn test_returning_delete_01_wildcard() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);

    let plan = plan_sql("DELETE FROM users WHERE id = 1 RETURNING *", &catalog);
    let exec = Executor::new();
    let result = exec.execute_delete(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    // DELETE RETURNING 返回被删除的旧行
    assert_eq!(
        result.returning_rows[0],
        vec![Value::Int64(1), Value::Text("a".into())]
    );
    // 行已被删除
    assert_eq!(table.row_count(), 0);
}

#[test]
fn test_returning_delete_02_single_column() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);

    let plan = plan_sql("DELETE FROM users WHERE id = 1 RETURNING id", &catalog);
    let exec = Executor::new();
    let result = exec.execute_delete(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(result.returning_rows[0], vec![Value::Int64(1)]);
}

#[test]
fn test_returning_delete_03_multiple_columns() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);

    let plan = plan_sql(
        "DELETE FROM users WHERE id = 1 RETURNING id, name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_delete(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(
        result.returning_rows[0],
        vec![Value::Int64(1), Value::Text("a".into())]
    );
}

#[test]
fn test_returning_delete_04_expression() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(5), Value::Text("a".into())]);

    // RETURNING id + 10 — 表达式
    let plan = plan_sql("DELETE FROM users WHERE id = 5 RETURNING id + 10", &catalog);
    let exec = Executor::new();
    let result = exec.execute_delete(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(result.returning_rows[0], vec![Value::Int64(15)]);
}

#[test]
fn test_returning_delete_05_where_filter() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);
    table.insert_row(vec![Value::Int64(2), Value::Text("b".into())]);
    table.insert_row(vec![Value::Int64(3), Value::Text("c".into())]);

    // 删除 id >= 2 的行，RETURNING 返回 2 行
    let plan = plan_sql("DELETE FROM users WHERE id >= 2 RETURNING id", &catalog);
    let exec = Executor::new();
    let result = exec.execute_delete(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 2);
    assert_eq!(result.returning_rows.len(), 2);
    assert_eq!(result.returning_rows[0], vec![Value::Int64(2)]);
    assert_eq!(result.returning_rows[1], vec![Value::Int64(3)]);
    // 剩余 1 行
    assert_eq!(table.row_count(), 1);
}

// =====================================================================
//  端到端测试（3 条）— PG 标准示例
// =====================================================================

#[test]
fn test_returning_e2e_01_pg_example_insert() {
    // PG 标准示例：INSERT INTO t VALUES (1) RETURNING * → 返回刚插入的行
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();

    let plan = plan_sql("INSERT INTO users VALUES (1, 'a') RETURNING *", &catalog);
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(
        result.returning_rows[0],
        vec![Value::Int64(1), Value::Text("a".into())]
    );
    // 验证行已插入
    assert_eq!(table.row_count(), 1);
}

#[test]
fn test_returning_e2e_02_pg_example_update() {
    // PG 标准示例：UPDATE t SET x=1 WHERE id=1 RETURNING id, x → 返回更新后的行
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("old".into())]);

    let plan = plan_sql(
        "UPDATE users SET name = 'new' WHERE id = 1 RETURNING id, name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_update(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    // 返回更新后的新值
    assert_eq!(
        result.returning_rows[0],
        vec![Value::Int64(1), Value::Text("new".into())]
    );
}

#[test]
fn test_returning_e2e_03_pg_example_delete() {
    // PG 标准示例：DELETE FROM t WHERE id=1 RETURNING id → 返回被删除的行
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);

    let plan = plan_sql("DELETE FROM users WHERE id = 1 RETURNING id", &catalog);
    let exec = Executor::new();
    let result = exec.execute_delete(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(result.returning_rows.len(), 1);
    assert_eq!(result.returning_rows[0], vec![Value::Int64(1)]);
    // 行已被删除
    assert_eq!(table.row_count(), 0);
}

// =====================================================================
//  无 RETURNING 测试（2 条）— 验证向后兼容
// =====================================================================

#[test]
fn test_returning_absent_01_insert_no_returning() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();

    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert!(result.returning_rows.is_empty());
}

#[test]
fn test_returning_absent_02_update_delete_no_returning() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);

    let exec = Executor::new();

    let update_plan = plan_sql("UPDATE users SET name = 'b' WHERE id = 1", &catalog);
    let update_result = exec.execute_update(&update_plan, &mut table).unwrap();
    assert_eq!(update_result.affected_rows, 1);
    assert!(update_result.returning_rows.is_empty());

    let delete_plan = plan_sql("DELETE FROM users WHERE id = 1", &catalog);
    let delete_result = exec.execute_delete(&delete_plan, &mut table).unwrap();
    assert_eq!(delete_result.affected_rows, 1);
    assert!(delete_result.returning_rows.is_empty());
}
