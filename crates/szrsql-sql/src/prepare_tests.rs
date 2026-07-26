//! Phase 3.26 单元测试 — PREPARE / EXECUTE / DEALLOCATE。
//!
//! 覆盖类别：
//! - Parser（5）：PREPARE 无参 / PREPARE 带参类型 / PREPARE 多参类型 / EXECUTE / DEALLOCATE
//! - Planner（4）：PREPARE / EXECUTE / DEALLOCATE name / DEALLOCATE ALL
//! - Executor PREPARE（2）：基本存储 / 同名覆盖
//! - Executor EXECUTE（4）：无参执行 / 单参执行 / 多参执行 / 多次 EXECUTE 不同参数
//! - Executor DEALLOCATE（3）：DEALLOCATE name / DEALLOCATE ALL / DEALLOCATE 不存在报错
//! - 错误处理（4）：EXECUTE 不存在 / DEALLOCATE 后再 EXECUTE 报错 / 参数越界 / $0 无效
//!
//! 共 22 个测试用例。

use super::executor::{
    ExecutionError, Executor, InMemoryTable, MutableTable, PreparedStatementStore,
};
use crate::ast::*;
use crate::parser::{parse_one, ParseError};
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner, TableSchema};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建带主键 `id` 的 catalog 表 `users`：(id INT PK, name TEXT)
fn make_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let mut id_col = ColumnDefinition::new("id", ColumnType::Int64);
    id_col.primary_key = true;
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    let age_col = ColumnDefinition::new("age", ColumnType::Int64);
    catalog.add_table(TableSchema {
        name: TableName::new("users"),
        columns: vec![id_col, name_col, age_col],
    });
    catalog
}

/// 创建带主键 `id` 的内存表 `users`：(id INT PK, name TEXT, age INT)
fn make_users_table() -> InMemoryTable {
    let mut id_col = ColumnDefinition::new("id", ColumnType::Int64);
    id_col.primary_key = true;
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    let age_col = ColumnDefinition::new("age", ColumnType::Int64);
    InMemoryTable::new(TableSchema {
        name: TableName::new("users"),
        columns: vec![id_col, name_col, age_col],
    })
}

/// 插入 3 行测试数据：(1, 'alice', 30), (42, 'bob', 25), (99, 'carol', 40)
fn make_users_table_with_data() -> InMemoryTable {
    let mut table = make_users_table();
    table.insert_row(vec![
        Value::Int64(1),
        Value::Text("alice".into()),
        Value::Int64(30),
    ]);
    table.insert_row(vec![
        Value::Int64(42),
        Value::Text("bob".into()),
        Value::Int64(25),
    ]);
    table.insert_row(vec![
        Value::Int64(99),
        Value::Text("carol".into()),
        Value::Int64(40),
    ]);
    table
}

/// SQL → AST → LogicalPlan（断言成功）
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).expect("plan failed")
}

/// SQL → AST → LogicalPlan（断言解析失败，返回错误）
fn parse_sql_err(sql: &str) -> ParseError {
    parse_one(sql).expect_err("expected parse error")
}

// =====================================================================
//  Parser 测试（5）
// =====================================================================

#[test]
fn test_prepare_parser_01_no_params() {
    let sql = "PREPARE p AS SELECT * FROM users";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Prepare {
            name,
            parameter_types,
            statement,
        } => {
            assert_eq!(name, "p");
            assert!(parameter_types.is_empty());
            assert!(
                matches!(*statement, Statement::Select(_)),
                "expected Select, got {:?}",
                *statement
            );
        }
        other => panic!("expected Prepare, got {other:?}"),
    }
}

#[test]
fn test_prepare_parser_02_with_param_type() {
    let sql = "PREPARE p (int) AS SELECT * FROM users WHERE id = $1";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Prepare {
            name,
            parameter_types,
            ..
        } => {
            assert_eq!(name, "p");
            assert_eq!(parameter_types.len(), 1);
            assert_eq!(parameter_types[0], ColumnType::Int64);
        }
        other => panic!("expected Prepare, got {other:?}"),
    }
}

#[test]
fn test_prepare_parser_03_multi_param_types() {
    let sql = "PREPARE p (int, text) AS SELECT * FROM users WHERE id = $1 AND name = $2";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Prepare {
            name,
            parameter_types,
            ..
        } => {
            assert_eq!(name, "p");
            assert_eq!(parameter_types.len(), 2);
            assert_eq!(parameter_types[0], ColumnType::Int64);
            assert_eq!(parameter_types[1], ColumnType::Text);
        }
        other => panic!("expected Prepare, got {other:?}"),
    }
}

#[test]
fn test_execute_parser_01_basic() {
    let sql = "EXECUTE p(42)";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Execute { name, parameters } => {
            assert_eq!(name, "p");
            assert_eq!(parameters.len(), 1);
            assert!(
                matches!(&parameters[0], Expr::Literal(Value::Int64(42))),
                "expected Literal(Int64(42)), got {:?}",
                parameters[0]
            );
        }
        other => panic!("expected Execute, got {other:?}"),
    }
}

#[test]
fn test_deallocate_parser_01_basic() {
    // DEALLOCATE name
    let stmt = parse_one("DEALLOCATE p").unwrap();
    match stmt {
        Statement::Deallocate { name } => assert_eq!(name, Some("p".to_string())),
        other => panic!("expected Deallocate, got {other:?}"),
    }

    // DEALLOCATE PREPARE name
    let stmt = parse_one("DEALLOCATE PREPARE p").unwrap();
    match stmt {
        Statement::Deallocate { name } => assert_eq!(name, Some("p".to_string())),
        other => panic!("expected Deallocate, got {other:?}"),
    }

    // DEALLOCATE ALL
    let stmt = parse_one("DEALLOCATE ALL").unwrap();
    match stmt {
        Statement::Deallocate { name } => assert_eq!(name, None),
        other => panic!("expected Deallocate, got {other:?}"),
    }
}

// =====================================================================
//  Planner 测试（4）
// =====================================================================

#[test]
fn test_prepare_planner_01_basic() {
    let catalog = make_catalog();
    let plan = plan_sql(
        "PREPARE p (int) AS SELECT * FROM users WHERE id = $1",
        &catalog,
    );
    match plan {
        LogicalPlan::Prepare {
            name,
            parameter_types,
            ..
        } => {
            assert_eq!(name, "p");
            assert_eq!(parameter_types.len(), 1);
        }
        other => panic!("expected Prepare, got {other:?}"),
    }
}

#[test]
fn test_execute_planner_01_basic() {
    let catalog = make_catalog();
    let plan = plan_sql("EXECUTE p(42)", &catalog);
    match plan {
        LogicalPlan::Execute { name, parameters } => {
            assert_eq!(name, "p");
            assert_eq!(parameters.len(), 1);
        }
        other => panic!("expected Execute, got {other:?}"),
    }
}

#[test]
fn test_deallocate_planner_01_name() {
    let catalog = make_catalog();
    let plan = plan_sql("DEALLOCATE p", &catalog);
    match plan {
        LogicalPlan::Deallocate { name } => assert_eq!(name, Some("p".to_string())),
        other => panic!("expected Deallocate, got {other:?}"),
    }
}

#[test]
fn test_deallocate_planner_02_all() {
    let catalog = make_catalog();
    let plan = plan_sql("DEALLOCATE ALL", &catalog);
    match plan {
        LogicalPlan::Deallocate { name } => assert_eq!(name, None),
        other => panic!("expected Deallocate, got {other:?}"),
    }
}

// =====================================================================
//  Executor PREPARE 测试（2）
// =====================================================================

#[test]
fn test_executor_prepare_01_basic_storage() {
    let catalog = make_catalog();
    let prepare_plan = plan_sql(
        "PREPARE p (int) AS SELECT * FROM users WHERE id = $1",
        &catalog,
    );
    let exec = Executor::new();
    let mut store = PreparedStatementStore::new();

    exec.execute_prepare(&prepare_plan, &mut store).unwrap();
    assert!(store.exists("p"));
    assert_eq!(store.len(), 1);

    let (stmt, param_types) = store.get("p").expect("p should exist");
    assert_eq!(param_types.len(), 1);
    assert!(matches!(stmt, Statement::Select(_)));
}

#[test]
fn test_executor_prepare_02_overwrite() {
    let catalog = make_catalog();
    let exec = Executor::new();
    let mut store = PreparedStatementStore::new();

    // 第一次 PREPARE
    let p1 = plan_sql("PREPARE p AS SELECT * FROM users", &catalog);
    exec.execute_prepare(&p1, &mut store).unwrap();
    assert_eq!(store.len(), 1);

    // 同名第二次 PREPARE（覆盖）
    let p2 = plan_sql(
        "PREPARE p (int) AS SELECT * FROM users WHERE id = $1",
        &catalog,
    );
    exec.execute_prepare(&p2, &mut store).unwrap();
    assert_eq!(store.len(), 1);

    // 验证使用的是第二个定义
    let (stmt, param_types) = store.get("p").expect("p should exist");
    assert_eq!(
        param_types.len(),
        1,
        "should use the second definition with 1 param type"
    );
    match stmt {
        Statement::Select(s) => {
            // 第二个定义有 WHERE 子句
            assert!(
                s.where_clause.is_some(),
                "second definition should have WHERE clause"
            );
        }
        other => panic!("expected Select, got {other:?}"),
    }
}

// =====================================================================
//  Executor EXECUTE 测试（4）
// =====================================================================

#[test]
fn test_executor_execute_01_select_no_params() {
    let catalog = make_catalog();
    let table = make_users_table_with_data();

    let mut exec = Executor::new();
    exec.register_table(&table);

    let mut store = PreparedStatementStore::new();
    let prepare_plan = plan_sql("PREPARE p AS SELECT * FROM users", &catalog);
    exec.execute_prepare(&prepare_plan, &mut store).unwrap();

    let execute_plan = plan_sql("EXECUTE p", &catalog);
    let rows = exec
        .execute_execute(&execute_plan, &store, &catalog)
        .unwrap();
    assert_eq!(rows.len(), 3);
}

#[test]
fn test_executor_execute_02_select_with_param() {
    let catalog = make_catalog();
    let table = make_users_table_with_data();

    let mut exec = Executor::new();
    exec.register_table(&table);

    let mut store = PreparedStatementStore::new();
    let prepare_plan = plan_sql(
        "PREPARE p (int) AS SELECT * FROM users WHERE id = $1",
        &catalog,
    );
    exec.execute_prepare(&prepare_plan, &mut store).unwrap();

    // EXECUTE p(42) → 应返回 id=42 的行
    let execute_plan = plan_sql("EXECUTE p(42)", &catalog);
    let rows = exec
        .execute_execute(&execute_plan, &store, &catalog)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(42));
    assert_eq!(rows[0][1], Value::Text("bob".into()));
}

#[test]
fn test_executor_execute_03_select_multi_params() {
    let catalog = make_catalog();
    let table = make_users_table_with_data();

    let mut exec = Executor::new();
    exec.register_table(&table);

    let mut store = PreparedStatementStore::new();
    let prepare_plan = plan_sql(
        "PREPARE p (int, text) AS SELECT * FROM users WHERE id = $1 AND name = $2",
        &catalog,
    );
    exec.execute_prepare(&prepare_plan, &mut store).unwrap();

    // EXECUTE p(42, 'bob') → 应返回 id=42 AND name='bob' 的行
    let execute_plan = plan_sql("EXECUTE p(42, 'bob')", &catalog);
    let rows = exec
        .execute_execute(&execute_plan, &store, &catalog)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(42));

    // EXECUTE p(42, 'alice') → id=42 但 name≠'alice' → 0 行
    let execute_plan = plan_sql("EXECUTE p(42, 'alice')", &catalog);
    let rows = exec
        .execute_execute(&execute_plan, &store, &catalog)
        .unwrap();
    assert_eq!(rows.len(), 0);
}

#[test]
fn test_executor_execute_04_multiple_executes_different_params() {
    let catalog = make_catalog();
    let table = make_users_table_with_data();

    let mut exec = Executor::new();
    exec.register_table(&table);

    let mut store = PreparedStatementStore::new();
    let prepare_plan = plan_sql(
        "PREPARE p (int) AS SELECT * FROM users WHERE id = $1",
        &catalog,
    );
    exec.execute_prepare(&prepare_plan, &mut store).unwrap();

    // EXECUTE p(42)
    let execute_plan = plan_sql("EXECUTE p(42)", &catalog);
    let rows = exec
        .execute_execute(&execute_plan, &store, &catalog)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(42));

    // EXECUTE p(99)
    let execute_plan = plan_sql("EXECUTE p(99)", &catalog);
    let rows = exec
        .execute_execute(&execute_plan, &store, &catalog)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(99));

    // EXECUTE p(1)
    let execute_plan = plan_sql("EXECUTE p(1)", &catalog);
    let rows = exec
        .execute_execute(&execute_plan, &store, &catalog)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(1));

    // EXECUTE p(1000) → 无匹配
    let execute_plan = plan_sql("EXECUTE p(1000)", &catalog);
    let rows = exec
        .execute_execute(&execute_plan, &store, &catalog)
        .unwrap();
    assert_eq!(rows.len(), 0);
}

// =====================================================================
//  Executor DEALLOCATE 测试（3）
// =====================================================================

#[test]
fn test_executor_deallocate_01_name() {
    let catalog = make_catalog();
    let exec = Executor::new();
    let mut store = PreparedStatementStore::new();

    let prepare_plan = plan_sql(
        "PREPARE p (int) AS SELECT * FROM users WHERE id = $1",
        &catalog,
    );
    exec.execute_prepare(&prepare_plan, &mut store).unwrap();
    assert!(store.exists("p"));

    let deallocate_plan = plan_sql("DEALLOCATE p", &catalog);
    exec.execute_deallocate(&deallocate_plan, &mut store)
        .unwrap();
    assert!(!store.exists("p"));
    assert_eq!(store.len(), 0);
}

#[test]
fn test_executor_deallocate_02_all() {
    let catalog = make_catalog();
    let exec = Executor::new();
    let mut store = PreparedStatementStore::new();

    // PREPARE 多个语句
    let p1 = plan_sql("PREPARE p1 AS SELECT * FROM users", &catalog);
    let p2 = plan_sql("PREPARE p2 AS SELECT * FROM users", &catalog);
    let p3 = plan_sql("PREPARE p3 AS SELECT * FROM users", &catalog);
    exec.execute_prepare(&p1, &mut store).unwrap();
    exec.execute_prepare(&p2, &mut store).unwrap();
    exec.execute_prepare(&p3, &mut store).unwrap();
    assert_eq!(store.len(), 3);

    // DEALLOCATE ALL
    let deallocate_plan = plan_sql("DEALLOCATE ALL", &catalog);
    exec.execute_deallocate(&deallocate_plan, &mut store)
        .unwrap();
    assert_eq!(store.len(), 0);
    assert!(store.is_empty());
}

#[test]
fn test_executor_deallocate_03_not_found() {
    let catalog = make_catalog();
    let exec = Executor::new();
    let mut store = PreparedStatementStore::new();

    let deallocate_plan = plan_sql("DEALLOCATE nonexistent", &catalog);
    let result = exec.execute_deallocate(&deallocate_plan, &mut store);
    assert!(result.is_err());
    match result.unwrap_err() {
        ExecutionError::InvalidArgument(msg) => {
            assert!(
                msg.contains("does not exist"),
                "expected 'does not exist' in message, got: {msg}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

// =====================================================================
//  错误处理测试（4）
// =====================================================================

#[test]
fn test_error_execute_nonexistent_prepared() {
    let catalog = make_catalog();
    let table = make_users_table_with_data();
    let mut exec = Executor::new();
    exec.register_table(&table);

    let store = PreparedStatementStore::new();
    let execute_plan = plan_sql("EXECUTE nonexistent(42)", &catalog);
    let result = exec.execute_execute(&execute_plan, &store, &catalog);
    assert!(result.is_err());
    match result.unwrap_err() {
        ExecutionError::InvalidArgument(msg) => {
            assert!(
                msg.contains("does not exist"),
                "expected 'does not exist' in message, got: {msg}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn test_error_execute_after_deallocate() {
    let catalog = make_catalog();
    let table = make_users_table_with_data();
    let mut exec = Executor::new();
    exec.register_table(&table);

    let mut store = PreparedStatementStore::new();
    let prepare_plan = plan_sql(
        "PREPARE p (int) AS SELECT * FROM users WHERE id = $1",
        &catalog,
    );
    exec.execute_prepare(&prepare_plan, &mut store).unwrap();

    // EXECUTE 成功
    let execute_plan = plan_sql("EXECUTE p(42)", &catalog);
    let rows = exec
        .execute_execute(&execute_plan, &store, &catalog)
        .unwrap();
    assert_eq!(rows.len(), 1);

    // DEALLOCATE
    let deallocate_plan = plan_sql("DEALLOCATE p", &catalog);
    exec.execute_deallocate(&deallocate_plan, &mut store)
        .unwrap();

    // 再次 EXECUTE → 报错
    let result = exec.execute_execute(&execute_plan, &store, &catalog);
    assert!(result.is_err());
    match result.unwrap_err() {
        ExecutionError::InvalidArgument(msg) => {
            assert!(
                msg.contains("does not exist"),
                "expected 'does not exist' in message, got: {msg}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn test_error_param_out_of_range() {
    let catalog = make_catalog();
    let table = make_users_table_with_data();
    let mut exec = Executor::new();
    exec.register_table(&table);

    let mut store = PreparedStatementStore::new();
    // PREPARE 使用 $1 和 $2
    let prepare_plan = plan_sql(
        "PREPARE p (int, text) AS SELECT * FROM users WHERE id = $1 AND name = $2",
        &catalog,
    );
    exec.execute_prepare(&prepare_plan, &mut store).unwrap();

    // EXECUTE 只提供 1 个参数 → $2 越界 → 报错
    let execute_plan = plan_sql("EXECUTE p(42)", &catalog);
    let result = exec.execute_execute(&execute_plan, &store, &catalog);
    assert!(result.is_err());
    match result.unwrap_err() {
        ExecutionError::InvalidArgument(msg) => {
            assert!(
                msg.contains("out of range") || msg.contains("$2"),
                "expected 'out of range' or '$2' in message, got: {msg}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn test_error_placeholder_zero_invalid() {
    // $0 在解析阶段就应报错（idx 必须是 >= 1）
    let result = parse_one("PREPARE p AS SELECT * FROM users WHERE id = $0");
    assert!(result.is_err());
    let _err = parse_sql_err("PREPARE p AS SELECT * FROM users WHERE id = $0");
}
