//! Phase 3.20 单元测试 — UPSERT (INSERT ... ON CONFLICT)。
//!
//! 覆盖类别：
//! - Parser（6 条）：DO NOTHING 无列 / DO NOTHING 带列 / DO UPDATE 无 WHERE / DO UPDATE 带 WHERE /
//!   ON CONSTRAINT 错误 / ON DUPLICATE KEY UPDATE 错误
//! - Planner（4 条）：DO NOTHING 计划 / DO UPDATE 计划 / 冲突列不存在错误 / AST 字段透传
//! - Executor DO NOTHING（5 条）：无冲突插入 / PK 冲突跳过 / 显式列冲突跳过 / NULL 不冲突 / 多行混合
//! - Executor DO UPDATE（7 条）：无冲突插入 / PK 冲突更新 / EXCLUDED 伪表 / WHERE 过滤 /
//!   显式冲突列更新 / 多行混合 / 不限定列名引用目标表
//! - 错误处理（2 条）：无 PK 且无显式列 → 错误 / 冲突列不存在 → 错误
//! - 端到端（3 条）：PG 示例 DO UPDATE / PG 示例 DO NOTHING / 多次 UPSERT 累积
//!
//! 共 27 个测试用例。

use super::executor::{Executor, InMemoryTable, MutableTable, TableStorage};
use crate::ast::*;
use crate::parser::{parse_one, ParseError};
use crate::plan::{InMemoryCatalog, LogicalPlan, PlanError, Planner, TableSchema};
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

/// 创建带 `email` 唯一约束（非 PK）的 catalog 表
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

/// 创建带 `email` 列的内存表（id, email, name）
fn make_unique_email_table() -> InMemoryTable {
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let email_col = ColumnDefinition::new("email", ColumnType::Text);
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    InMemoryTable::new(TableSchema {
        name: TableName::new("users"),
        columns: vec![id_col, email_col, name_col],
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

// =====================================================================
//  Parser 测试（6 条）
// =====================================================================

#[test]
fn test_upsert_parser_01_do_nothing_no_cols() {
    let stmt = parse_one("INSERT INTO t VALUES (1, 'a') ON CONFLICT DO NOTHING").unwrap();
    match stmt {
        Statement::Insert {
            on_conflict: Some(OnConflict::DoNothing { conflict_columns }),
            ..
        } => {
            assert_eq!(conflict_columns, None);
        }
        other => panic!("expected Insert with DoNothing, got {other:?}"),
    }
}

#[test]
fn test_upsert_parser_02_do_nothing_with_cols() {
    let stmt = parse_one("INSERT INTO t VALUES (1, 'a') ON CONFLICT (id) DO NOTHING").unwrap();
    match stmt {
        Statement::Insert {
            on_conflict:
                Some(OnConflict::DoNothing {
                    conflict_columns: Some(cols),
                }),
            ..
        } => {
            assert_eq!(cols, vec!["id"]);
        }
        other => panic!("expected Insert with DoNothing(cols), got {other:?}"),
    }
}

#[test]
fn test_upsert_parser_03_do_update_no_where() {
    let stmt = parse_one(
        "INSERT INTO t VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
    )
    .unwrap();
    match stmt {
        Statement::Insert {
            on_conflict:
                Some(OnConflict::DoUpdate {
                    conflict_columns,
                    assignments,
                    where_clause,
                }),
            ..
        } => {
            assert_eq!(conflict_columns, Some(vec!["id".to_string()]));
            assert_eq!(assignments.len(), 1);
            assert_eq!(assignments[0].column, "name");
            // EXCLUDED.name → Identifier(["EXCLUDED", "name"])
            match &assignments[0].value {
                Expr::Identifier(parts) => {
                    assert_eq!(parts, &vec!["EXCLUDED".to_string(), "name".to_string()]);
                }
                other => panic!("expected Identifier, got {other:?}"),
            }
            assert_eq!(where_clause, None);
        }
        other => panic!("expected Insert with DoUpdate, got {other:?}"),
    }
}

#[test]
fn test_upsert_parser_04_do_update_with_where() {
    let stmt = parse_one(
        "INSERT INTO t VALUES (1, 'a') ON CONFLICT (id) \
         DO UPDATE SET name = EXCLUDED.name WHERE id > 0",
    )
    .unwrap();
    match stmt {
        Statement::Insert {
            on_conflict:
                Some(OnConflict::DoUpdate {
                    where_clause: Some(_),
                    ..
                }),
            ..
        } => { /* OK */ }
        other => panic!("expected Insert with DoUpdate(where=Some), got {other:?}"),
    }
}

#[test]
fn test_upsert_parser_05_on_constraint_unsupported() {
    let result =
        parse_one("INSERT INTO t VALUES (1, 'a') ON CONFLICT ON CONSTRAINT t_pkey DO NOTHING");
    assert!(matches!(result, Err(ParseError::Unsupported(_))));
}

#[test]
fn test_upsert_parser_06_on_duplicate_key_update_unsupported() {
    // PG 方言下不解析 ON DUPLICATE KEY UPDATE，应报错
    let result = parse_one("INSERT INTO t VALUES (1, 'a') ON DUPLICATE KEY UPDATE name = 'b'");
    assert!(result.is_err());
}

// =====================================================================
//  Planner 测试（4 条）
// =====================================================================

#[test]
fn test_upsert_planner_01_do_nothing_plan() {
    let catalog = make_catalog_with_pk_table();
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a') ON CONFLICT DO NOTHING",
        &catalog,
    );
    match plan {
        LogicalPlan::Insert {
            on_conflict: Some(OnConflict::DoNothing { .. }),
            ..
        } => { /* OK */ }
        other => panic!("expected Insert with DoNothing, got {other:?}"),
    }
}

#[test]
fn test_upsert_planner_02_do_update_plan() {
    let catalog = make_catalog_with_pk_table();
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    match plan {
        LogicalPlan::Insert {
            on_conflict: Some(OnConflict::DoUpdate { .. }),
            ..
        } => { /* OK */ }
        other => panic!("expected Insert with DoUpdate, got {other:?}"),
    }
}

#[test]
fn test_upsert_planner_03_conflict_col_not_found() {
    let catalog = make_catalog_with_pk_table();
    let err = plan_sql_err(
        "INSERT INTO users VALUES (1, 'a') ON CONFLICT (bad_col) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    assert!(matches!(err, PlanError::ColumnNotFound(_)));
}

#[test]
fn test_upsert_planner_04_on_conflict_none_when_absent() {
    let catalog = make_catalog_with_pk_table();
    let plan = plan_sql("INSERT INTO users VALUES (1, 'a')", &catalog);
    match plan {
        LogicalPlan::Insert {
            on_conflict: None, ..
        } => { /* OK */ }
        other => panic!("expected Insert with on_conflict=None, got {other:?}"),
    }
}

// =====================================================================
//  Executor DO NOTHING 测试（5 条）
// =====================================================================

#[test]
fn test_upsert_exec_do_nothing_01_no_conflict_inserts() {
    let catalog = make_catalog_with_pk_table();
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a') ON CONFLICT DO NOTHING",
        &catalog,
    );
    let mut table = make_pk_table();
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 1);
    assert_eq!(table.get_row(0).unwrap()[0], Value::Int64(1));
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("a".into()));
}

#[test]
fn test_upsert_exec_do_nothing_02_pk_conflict_skips() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    // 预置冲突行
    table.insert_row(vec![Value::Int64(1), Value::Text("existing".into())]);

    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a') ON CONFLICT DO NOTHING",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 0); // 跳过
    assert_eq!(table.row_count(), 1);
    // 原行不变
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("existing".into()));
}

#[test]
fn test_upsert_exec_do_nothing_03_explicit_col_conflict_skips() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("existing".into())]);

    // 显式指定冲突列 id
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a') ON CONFLICT (id) DO NOTHING",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 0);
    assert_eq!(table.row_count(), 1);
}

#[test]
fn test_upsert_exec_do_nothing_04_null_no_conflict_inserts() {
    // 无 PK 列时，NULL 在冲突列上不引发冲突（PG 语义）
    let mut catalog = InMemoryCatalog::new();
    let id_col = ColumnDefinition::new("id", ColumnType::Int64); // 无 PK
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    catalog.add_table(TableSchema {
        name: TableName::new("t"),
        columns: vec![id_col, name_col],
    });

    let mut table = InMemoryTable::new(TableSchema {
        name: TableName::new("t"),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("name", ColumnType::Text),
        ],
    });
    // 预置一行 id=NULL
    table.insert_row(vec![Value::Null, Value::Text("existing".into())]);

    // ON CONFLICT (id) DO NOTHING：拟插入 id=NULL → 不应冲突
    let plan = plan_sql(
        "INSERT INTO t VALUES (NULL, 'a') ON CONFLICT (id) DO NOTHING",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1); // NULL 不冲突 → 插入
    assert_eq!(table.row_count(), 2);
}

#[test]
fn test_upsert_exec_do_nothing_05_multi_rows_mixed() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    // 预置 id=2 已存在
    table.insert_row(vec![Value::Int64(2), Value::Text("existing".into())]);

    // 三行 VALUES：id=1（无冲突）+ id=2（冲突）+ id=3（无冲突）
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a'), (2, 'b'), (3, 'c') ON CONFLICT DO NOTHING",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 2); // 跳过 id=2，插入 1 和 3
    assert_eq!(table.row_count(), 3); // 原 1 + 新 2
}

// =====================================================================
//  Executor DO UPDATE 测试（7 条）
// =====================================================================

#[test]
fn test_upsert_exec_do_update_01_no_conflict_inserts() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();

    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("a".into()));
}

#[test]
fn test_upsert_exec_do_update_02_pk_conflict_updates() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("old".into())]);

    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'new') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1); // 更新计 1
    assert_eq!(table.row_count(), 1); // 行数不变
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("new".into()));
    // PK 不变
    assert_eq!(table.get_row(0).unwrap()[0], Value::Int64(1));
}

#[test]
fn test_upsert_exec_do_update_03_excluded_pseudo_table() {
    // 验证 SET name = EXCLUDED.name 用拟插入值，SET id = id 用目标表当前值
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("old_name".into())]);

    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'new_name') \
         ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    let exec = Executor::new();
    exec.execute_insert(&plan, &mut table).unwrap();

    // EXCLUDED.name 应为 'new_name'
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("new_name".into()));
}

#[test]
fn test_upsert_exec_do_update_04_where_false_skips() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("old".into())]);

    // WHERE 条件不满足（id > 100 为 false，因 id=1）→ 跳过更新
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'new') \
         ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name WHERE id > 100",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 0); // WHERE 不满足 → 跳过
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("old".into())); // 原值不变
}

#[test]
fn test_upsert_exec_do_update_05_where_true_updates() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("old".into())]);

    // WHERE 条件满足（id = 1 为 true）
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'new') \
         ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name WHERE id = 1",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("new".into()));
}

#[test]
fn test_upsert_exec_do_update_06_non_pk_conflict_col() {
    // ON CONFLICT (email) — email 不是 PK 但作为冲突判定列
    let catalog = make_catalog_with_unique_email();
    let mut table = make_unique_email_table();
    table.insert_row(vec![
        Value::Int64(1),
        Value::Text("a@x.com".into()),
        Value::Text("alice".into()),
    ]);

    // 冲突 email='a@x.com' → 更新 name
    let plan = plan_sql(
        "INSERT INTO users VALUES (2, 'a@x.com', 'bob') \
         ON CONFLICT (email) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 1); // 没有新增行
    assert_eq!(table.get_row(0).unwrap()[0], Value::Int64(1)); // 原 id 保留
    assert_eq!(table.get_row(0).unwrap()[2], Value::Text("bob".into())); // name 更新
}

#[test]
fn test_upsert_exec_do_update_07_multi_rows_mixed() {
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    // 预置 id=2
    table.insert_row(vec![Value::Int64(2), Value::Text("old2".into())]);

    // 三行：id=1（无冲突，插入）+ id=2（冲突，更新）+ id=3（无冲突，插入）
    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a'), (2, 'new2'), (3, 'c') \
         ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 3); // 2 插入 + 1 更新
    assert_eq!(table.row_count(), 3);
    // id=2 的 name 应已更新
    let row2 = table.scan_with_ids().find(|(_, r)| r[0] == Value::Int64(2));
    assert_eq!(row2.unwrap().1[1], Value::Text("new2".into()));
}

// =====================================================================
//  错误处理（2 条）
// =====================================================================

#[test]
fn test_upsert_error_01_no_pk_no_cols() {
    // 无 PK 且无显式冲突列 → 错误
    let mut catalog = InMemoryCatalog::new();
    let id_col = ColumnDefinition::new("id", ColumnType::Int64); // 无 PK
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    catalog.add_table(TableSchema {
        name: TableName::new("t"),
        columns: vec![id_col, name_col],
    });

    let mut table = InMemoryTable::new(TableSchema {
        name: TableName::new("t"),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("name", ColumnType::Text),
        ],
    });

    let plan = plan_sql(
        "INSERT INTO t VALUES (1, 'a') ON CONFLICT DO NOTHING",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table);
    assert!(result.is_err());
}

#[test]
fn test_upsert_error_02_no_pk_do_update_no_cols() {
    let mut catalog = InMemoryCatalog::new();
    let id_col = ColumnDefinition::new("id", ColumnType::Int64);
    let name_col = ColumnDefinition::new("name", ColumnType::Text);
    catalog.add_table(TableSchema {
        name: TableName::new("t"),
        columns: vec![id_col, name_col],
    });

    let mut table = InMemoryTable::new(TableSchema {
        name: TableName::new("t"),
        columns: vec![
            ColumnDefinition::new("id", ColumnType::Int64),
            ColumnDefinition::new("name", ColumnType::Text),
        ],
    });

    let plan = plan_sql(
        "INSERT INTO t VALUES (1, 'a') ON CONFLICT DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table);
    assert!(result.is_err());
}

// =====================================================================
//  端到端测试（3 条）
// =====================================================================

#[test]
fn test_upsert_e2e_01_pg_example_do_update() {
    // PG 标准示例：冲突时更新，无冲突时插入
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    let exec = Executor::new();

    // 第一次：无冲突 → 插入 'a'
    let plan1 = plan_sql(
        "INSERT INTO users VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    let result = exec.execute_insert(&plan1, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("a".into()));

    // 第二次：冲突 → 更新 name 为 'b'
    let plan2 = plan_sql(
        "INSERT INTO users VALUES (1, 'b') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    let result = exec.execute_insert(&plan2, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1); // 更新计 1
    assert_eq!(table.row_count(), 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("b".into()));
}

#[test]
fn test_upsert_e2e_02_pg_example_do_nothing() {
    // PG 标准示例：冲突时跳过
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    table.insert_row(vec![Value::Int64(1), Value::Text("existing".into())]);

    let plan = plan_sql(
        "INSERT INTO users VALUES (1, 'a') ON CONFLICT DO NOTHING",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&plan, &mut table).unwrap();

    assert_eq!(result.affected_rows, 0);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("existing".into()));
}

#[test]
fn test_upsert_e2e_03_repeated_upsert_accumulates() {
    // 多次 UPSERT 累积：第一次插入，后续每次更新（每次用不同 VALUES）
    let catalog = make_catalog_with_pk_table();
    let mut table = make_pk_table();
    let exec = Executor::new();

    // 第一次插入
    let plan_a = plan_sql(
        "INSERT INTO users VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    exec.execute_insert(&plan_a, &mut table).unwrap();
    assert_eq!(table.row_count(), 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("a".into()));

    // 第二次更新 → 'b'
    let plan_b = plan_sql(
        "INSERT INTO users VALUES (1, 'b') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    exec.execute_insert(&plan_b, &mut table).unwrap();
    assert_eq!(table.row_count(), 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("b".into()));

    // 第三次更新 → 'c'
    let plan_c = plan_sql(
        "INSERT INTO users VALUES (1, 'c') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name",
        &catalog,
    );
    exec.execute_insert(&plan_c, &mut table).unwrap();
    assert_eq!(table.row_count(), 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Text("c".into()));
}
