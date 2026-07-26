//! Phase 3.25 单元测试 — REPLACE INTO（MySQL 扩展）。
//!
//! 覆盖类别：
//! - Parser（5）：基本 REPLACE / 显式列 / 多行 VALUES / 大小写不敏感 / 替代 INSERT 语法不混淆
//! - Planner（4）：基本计划生成 / 表不存在 / 列不存在 / 列数不匹配
//! - Executor PK（4）：无冲突插入 / PK 冲突替换 / 多行混合 / 连续两次 REPLACE
//! - Executor UNIQUE（2）：UNIQUE 冲突替换 / 显式列 UNIQUE 冲突
//! - Executor SELECT 源（1）：REPLACE INTO ... SELECT ...
//! - 错误处理（2）：无 PK 且无 UNIQUE → 错误 / 错误计划类型
//!
//! 共 18 个测试用例。

use super::executor::{ExecutionError, Executor, InMemoryTable, MutableTable, TableStorage};
use crate::ast::*;
use crate::parser::parse_one;
use crate::plan::{
    InMemoryCatalog, InsertSourcePlan, LogicalPlan, PlanError, Planner, TableSchema,
};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建带主键 `id` 的 catalog 表 `users`：(id INT PK, name TEXT)
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

/// 创建带主键 `id` 的内存表 `users`：(id INT PK, name TEXT)
fn make_pk_table() -> InMemoryTable {
    let mut id_col = ColumnDefinition::new("id", ColumnType::Int64);
    id_col.primary_key = true;
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    InMemoryTable::new(TableSchema {
        name: TableName::new("users"),
        columns: vec![id_col, name_col],
    })
}

/// 创建带 `email` UNIQUE 约束（非 PK）的 catalog 表 `users`：(id INT, email TEXT UNIQUE, name TEXT)
fn make_catalog_with_unique_email() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let mut email_col = ColumnDefinition::new("email", ColumnType::Text);
    email_col.unique = true;
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    catalog.add_table(TableSchema {
        name: TableName::new("users"),
        columns: vec![id_col, email_col, name_col],
    });
    catalog
}

/// 创建带 `email` UNIQUE 约束的内存表 `users`：(id INT, email TEXT UNIQUE, name TEXT)
fn make_unique_email_table() -> InMemoryTable {
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let mut email_col = ColumnDefinition::new("email", ColumnType::Text);
    email_col.unique = true;
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    InMemoryTable::new(TableSchema {
        name: TableName::new("users"),
        columns: vec![id_col, email_col, name_col],
    })
}

/// 创建无约束的 catalog 表 `users`：(id INT, name TEXT)
fn make_catalog_with_no_constraint_table() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    catalog.add_table(TableSchema {
        name: TableName::new("users"),
        columns: vec![id_col, name_col],
    });
    catalog
}

/// 创建无约束的内存表 `users`：(id INT, name TEXT)
fn make_no_constraint_table() -> InMemoryTable {
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
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

/// SQL → AST → LogicalPlan（断言失败，返回错误）
fn plan_sql_err(sql: &str, catalog: &InMemoryCatalog) -> PlanError {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmt)
        .expect_err("expected plan error")
}

/// 收集 users 表所有行，按 id 排序后返回 (id, name) 对
fn collect_pk_sorted(table: &InMemoryTable) -> Vec<(i64, String)> {
    let mut rows: Vec<(i64, String)> = table
        .scan_iter()
        .map(|r| match (&r[0], &r[1]) {
            (Value::Int64(a), Value::Text(b)) => (*a, b.clone()),
            (Value::Int64(a), Value::Null) => (*a, String::new()),
            _ => panic!("expected (Int64, Text), got {:?}", r),
        })
        .collect();
    rows.sort_by_key(|(a, _)| *a);
    rows
}

/// 收集 users 表所有行（含 email），按 id 排序后返回 (id, email, name) 三元组
fn collect_unique_sorted(table: &InMemoryTable) -> Vec<(i64, String, String)> {
    let mut rows: Vec<(i64, String, String)> = table
        .scan_iter()
        .map(|r| match (&r[0], &r[1], &r[2]) {
            (Value::Int64(a), Value::Text(b), Value::Text(c)) => (*a, b.clone(), c.clone()),
            (Value::Int64(a), Value::Text(b), Value::Null) => (*a, b.clone(), String::new()),
            _ => panic!("expected (Int64, Text, Text/Null), got {:?}", r),
        })
        .collect();
    rows.sort_by_key(|(a, _, _)| *a);
    rows
}

// =====================================================================
//  Parser 测试（5）
// =====================================================================

#[test]
fn test_replace_parser_01_basic_values() {
    let sql = "REPLACE INTO users VALUES (1, 'alice')";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Replace {
            table,
            columns,
            source,
        } => {
            assert_eq!(table, TableName::new("users"));
            assert_eq!(columns, None);
            match source {
                InsertSource::Values(rows) => {
                    assert_eq!(rows.len(), 1);
                    assert_eq!(rows[0].len(), 2);
                }
                other => panic!("expected Values source, got {other:?}"),
            }
        }
        other => panic!("expected Replace, got {other:?}"),
    }
}

#[test]
fn test_replace_parser_02_explicit_columns() {
    let sql = "REPLACE INTO users (id, name) VALUES (1, 'bob')";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Replace {
            columns, source, ..
        } => {
            assert_eq!(columns, Some(vec!["id".to_string(), "name".to_string()]));
            match source {
                InsertSource::Values(rows) => {
                    assert_eq!(rows.len(), 1);
                    assert_eq!(rows[0].len(), 2);
                }
                other => panic!("expected Values, got {other:?}"),
            }
        }
        other => panic!("expected Replace, got {other:?}"),
    }
}

#[test]
fn test_replace_parser_03_multiple_rows() {
    let sql = "REPLACE INTO users (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')";
    let stmt = parse_one(sql).unwrap();
    match stmt {
        Statement::Replace { source, .. } => match source {
            InsertSource::Values(rows) => {
                assert_eq!(rows.len(), 3);
            }
            other => panic!("expected Values, got {other:?}"),
        },
        other => panic!("expected Replace, got {other:?}"),
    }
}

#[test]
fn test_replace_parser_04_case_insensitive() {
    // 小写 `replace into` 也应被识别（MySqlDialect 大小写不敏感）
    let sql = "replace into users values (1, 'a')";
    let stmt = parse_one(sql).unwrap();
    assert!(matches!(stmt, Statement::Replace { .. }));
}

#[test]
fn test_replace_parser_05_not_confused_with_insert() {
    // 普通 INSERT 不应被识别为 REPLACE
    let sql = "INSERT INTO users VALUES (1, 'a')";
    let stmt = parse_one(sql).unwrap();
    assert!(matches!(stmt, Statement::Insert { .. }));
    assert!(!matches!(stmt, Statement::Replace { .. }));
}

// =====================================================================
//  Planner 测试（4）
// =====================================================================

#[test]
fn test_replace_plan_01_basic() {
    let catalog = make_catalog_with_pk_table();
    let plan = plan_sql("REPLACE INTO users VALUES (1, 'alice')", &catalog);
    match plan {
        LogicalPlan::Replace {
            table,
            schema,
            columns,
            source,
        } => {
            assert_eq!(table, TableName::new("users"));
            assert_eq!(schema.columns.len(), 2);
            assert!(schema.columns[0].primary_key);
            assert_eq!(columns, None);
            match source {
                InsertSourcePlan::Values(rows) => assert_eq!(rows.len(), 1),
                other => panic!("expected Values, got {other:?}"),
            }
        }
        other => panic!("expected Replace plan, got {other:?}"),
    }
}

#[test]
fn test_replace_plan_02_table_not_found() {
    let catalog = InMemoryCatalog::new();
    let err = plan_sql_err("REPLACE INTO missing VALUES (1, 'a')", &catalog);
    assert!(matches!(err, PlanError::TableNotFound(_)));
}

#[test]
fn test_replace_plan_03_column_not_found() {
    let catalog = make_catalog_with_pk_table();
    let err = plan_sql_err(
        "REPLACE INTO users (id, no_such_col) VALUES (1, 'a')",
        &catalog,
    );
    assert!(matches!(err, PlanError::ColumnNotFound(_)));
}

#[test]
fn test_replace_plan_04_column_count_mismatch() {
    let catalog = make_catalog_with_pk_table();
    // 指定 2 列但 VALUES 仅 1 个值
    let err = plan_sql_err("REPLACE INTO users (id, name) VALUES (1)", &catalog);
    assert!(matches!(err, PlanError::InvalidExpression(_)));
}

// =====================================================================
//  Executor PK 测试（4）
// =====================================================================

#[test]
fn test_replace_exec_01_no_conflict_insert() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    // 预置一行 (1, 'alice')
    table.insert_row(vec![Value::Int64(1), Value::Text("alice".into())]);

    let plan = plan_sql("REPLACE INTO users VALUES (2, 'bob')", &catalog);
    let exec = Executor::new();
    let result = exec.execute_replace(&plan, &mut table).unwrap();

    // 无冲突 → 受影响 1
    assert_eq!(result.affected_rows, 1);
    let rows = collect_pk_sorted(&table);
    assert_eq!(rows, vec![(1, "alice".into()), (2, "bob".into())]);
}

#[test]
fn test_replace_exec_02_pk_conflict_replace() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    // 预置一行 (1, 'alice')
    table.insert_row(vec![Value::Int64(1), Value::Text("alice".into())]);

    let plan = plan_sql("REPLACE INTO users VALUES (1, 'alice_new')", &catalog);
    let exec = Executor::new();
    let result = exec.execute_replace(&plan, &mut table).unwrap();

    // PK 冲突 → DELETE+INSERT → 受影响 2
    assert_eq!(result.affected_rows, 2);
    let rows = collect_pk_sorted(&table);
    assert_eq!(rows, vec![(1, "alice_new".into())]);
}

#[test]
fn test_replace_exec_03_multi_row_mixed() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    // 预置 (1, 'a'), (2, 'b')
    table.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);
    table.insert_row(vec![Value::Int64(2), Value::Text("b".into())]);

    // REPLACE 多行：(1, 'a_new') 冲突，(3, 'c') 无冲突
    let plan = plan_sql("REPLACE INTO users VALUES (1, 'a_new'), (3, 'c')", &catalog);
    let exec = Executor::new();
    let result = exec.execute_replace(&plan, &mut table).unwrap();

    // (1, 'a_new') → 2，(3, 'c') → 1，总计 3
    assert_eq!(result.affected_rows, 3);
    let rows = collect_pk_sorted(&table);
    assert_eq!(
        rows,
        vec![(1, "a_new".into()), (2, "b".into()), (3, "c".into())]
    );
}

#[test]
fn test_replace_exec_04_sequential_replaces() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    let exec = Executor::new();

    // 第一次：插入 (1, 'a')
    let plan = plan_sql("REPLACE INTO users VALUES (1, 'a')", &catalog);
    let r1 = exec.execute_replace(&plan, &mut table).unwrap();
    assert_eq!(r1.affected_rows, 1);

    // 第二次：替换 (1, 'b')
    let plan = plan_sql("REPLACE INTO users VALUES (1, 'b')", &catalog);
    let r2 = exec.execute_replace(&plan, &mut table).unwrap();
    assert_eq!(r2.affected_rows, 2);

    // 第三次：替换 (1, 'c')
    let plan = plan_sql("REPLACE INTO users VALUES (1, 'c')", &catalog);
    let r3 = exec.execute_replace(&plan, &mut table).unwrap();
    assert_eq!(r3.affected_rows, 2);

    let rows = collect_pk_sorted(&table);
    assert_eq!(rows, vec![(1, "c".into())]);
}

// =====================================================================
//  Executor UNIQUE 测试（2）
// =====================================================================

#[test]
fn test_replace_exec_05_unique_conflict_replace() {
    let catalog = make_catalog_with_unique_email();
    let mut table = make_unique_email_table();
    // 预置 (1, 'a@x.com', 'alice')
    table.insert_row(vec![
        Value::Int64(1),
        Value::Text("a@x.com".into()),
        Value::Text("alice".into()),
    ]);

    // REPLACE 与 email UNIQUE 冲突
    let plan = plan_sql(
        "REPLACE INTO users (id, email, name) VALUES (2, 'a@x.com', 'bob')",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_replace(&plan, &mut table).unwrap();

    // UNIQUE 冲突 → DELETE+INSERT → 受影响 2
    assert_eq!(result.affected_rows, 2);
    let rows = collect_unique_sorted(&table);
    assert_eq!(rows, vec![(2, "a@x.com".into(), "bob".into())]);
}

#[test]
fn test_replace_exec_06_unique_explicit_cols_no_conflict() {
    let catalog = make_catalog_with_unique_email();
    let mut table = make_unique_email_table();

    // 无冲突，使用显式列（id, email, name）
    let plan = plan_sql(
        "REPLACE INTO users (id, email, name) VALUES (1, 'new@x.com', 'new')",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_replace(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    let rows = collect_unique_sorted(&table);
    assert_eq!(rows, vec![(1, "new@x.com".into(), "new".into())]);
}

// =====================================================================
//  Executor SELECT 源测试（1）
// =====================================================================

#[test]
fn test_replace_exec_07_select_source() {
    // 测试 REPLACE INTO ... SELECT ... 语法
    let mut catalog = InMemoryCatalog::new();
    // 注册目标表 t（id INT PK, name TEXT）
    let mut id_col = ColumnDefinition::new("id", ColumnType::Int64);
    id_col.primary_key = true;
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    catalog.add_table(TableSchema {
        name: TableName::new("t"),
        columns: vec![id_col.clone(), name_col.clone()],
    });
    // 注册源表 s（id INT, name TEXT）
    catalog.add_table(TableSchema {
        name: TableName::new("s"),
        columns: vec![id_col, name_col],
    });

    // 创建源表 s 含两行：(1, 'a'), (2, 'b')
    let mut src_schema = TableSchema {
        name: TableName::new("s"),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("name", ColumnType::Text),
        ],
    };
    let _ = &mut src_schema;
    let mut source = InMemoryTable::new(src_schema);
    source.insert_row(vec![Value::Int64(1), Value::Text("a".into())]);
    source.insert_row(vec![Value::Int64(2), Value::Text("b".into())]);

    // 目标表 t 空表
    let mut target_schema = TableSchema {
        name: TableName::new("t"),
        columns: vec![
            {
                let mut c = ColumnDefinition::new("id", ColumnType::Int64);
                c.primary_key = true;
                c
            },
            ColumnDefinition::new("name", ColumnType::Text),
        ],
    };
    let _ = &mut target_schema;
    let mut target = InMemoryTable::new(target_schema);

    let plan = plan_sql("REPLACE INTO t SELECT id, name FROM s", &catalog);

    let mut exec = Executor::new();
    exec.register_table(&source);
    let result = exec.execute_replace(&plan, &mut target).unwrap();

    // 无冲突 → 2 行 INSERT
    assert_eq!(result.affected_rows, 2);
    let rows: Vec<(i64, String)> = {
        let mut rows: Vec<(i64, String)> = target
            .scan_iter()
            .map(|r| match (&r[0], &r[1]) {
                (Value::Int64(a), Value::Text(b)) => (*a, b.clone()),
                _ => panic!("expected (Int64, Text), got {:?}", r),
            })
            .collect();
        rows.sort_by_key(|(a, _)| *a);
        rows
    };
    assert_eq!(rows, vec![(1, "a".into()), (2, "b".into())]);
}

// =====================================================================
//  错误处理测试（2）
// =====================================================================

#[test]
fn test_replace_err_01_no_pk_no_unique() {
    let catalog = make_catalog_with_no_constraint_table();
    let mut table = make_no_constraint_table();

    let plan = plan_sql("REPLACE INTO users VALUES (1, 'a')", &catalog);
    let exec = Executor::new();
    let result = exec.execute_replace(&plan, &mut table);
    assert!(result.is_err());
    match result.unwrap_err() {
        ExecutionError::InvalidArgument(msg) => {
            assert!(
                msg.contains("PRIMARY KEY") || msg.contains("UNIQUE"),
                "expected error about PRIMARY KEY or UNIQUE, got: {msg}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}

#[test]
fn test_replace_err_02_wrong_plan_type() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();

    // 用 SELECT 计划调用 execute_replace 应当报错
    let plan = plan_sql("SELECT 1", &catalog);
    let exec = Executor::new();
    let result = exec.execute_replace(&plan, &mut table);
    assert!(result.is_err());
    match result.unwrap_err() {
        ExecutionError::InvalidArgument(msg) => {
            assert!(
                msg.contains("expected Replace plan"),
                "unexpected msg: {msg}"
            );
        }
        other => panic!("expected InvalidArgument, got {other:?}"),
    }
}
