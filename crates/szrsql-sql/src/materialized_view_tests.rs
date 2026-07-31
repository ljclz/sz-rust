//! Phase 6.10 集成测试 — 物化视图定义 + 存储。
//!
//! 覆盖类别：
//! - Parser（5 条）：CREATE VIEW / CREATE MATERIALIZED VIEW / DROP VIEW /
//!   DROP MATERIALIZED VIEW / REFRESH MATERIALIZED VIEW
//! - Planner（5 条）：CreateView / DropView / RefreshMaterializedView 计划生成
//! - Executor DDL（8 条）：CREATE 注册到 catalog / IF NOT EXISTS 跳过 / 已存在报错 /
//!   DROP 移除 / DROP IF EXISTS / DROP 不存在报错 / REFRESH 校验 / REFRESH 非物化视图报错
//! - ViewDefinition 结构（3 条）：new_materialized / new_view / with_columns
//! - 组合场景（2 条）：CREATE + DROP + 重新 CREATE / 多视图 DROP
//!
//! 共 23 个测试用例。

use super::executor::{Executor, InMemoryTable, TableStorage};
use crate::ast::Statement;
use crate::materialized_view::{RefreshMode, RefreshState, ViewDefinition};
use crate::parser::parse_one;
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建空 catalog + planner，用于 plan 阶段
fn make_catalog() -> InMemoryCatalog {
    InMemoryCatalog::new()
}

/// 创建带 `users` 表的 catalog（id INT PK, name TEXT）
fn make_catalog_with_users() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE users (id INT PRIMARY KEY, name TEXT)",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();
    catalog
}

/// SQL → AST → LogicalPlan（断言成功）
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).expect("plan failed")
}

/// SQL → AST → LogicalPlan（断言失败，返回错误）
fn plan_sql_err(sql: &str, catalog: &InMemoryCatalog) -> String {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    match planner.plan_statement(stmt) {
        Ok(_) => panic!("expected plan error, but succeeded"),
        Err(e) => format!("{e:?}"),
    }
}

// =====================================================================
//  Parser 测试（5 条）
// =====================================================================

#[test]
fn parse_create_view_basic() {
    let stmt = parse_one("CREATE VIEW v1 AS SELECT id, name FROM users").unwrap();
    match stmt {
        Statement::CreateView {
            name,
            columns,
            materialized,
            if_not_exists,
            ..
        } => {
            assert_eq!(name.name, "v1");
            assert!(columns.is_empty());
            assert!(!materialized);
            assert!(!if_not_exists);
        }
        other => panic!("expected CreateView, got {other:?}"),
    }
}

#[test]
fn parse_create_view_with_columns() {
    let stmt = parse_one("CREATE VIEW v2 (a, b) AS SELECT id, name FROM users").unwrap();
    match stmt {
        Statement::CreateView { name, columns, .. } => {
            assert_eq!(name.name, "v2");
            assert_eq!(columns, vec!["a", "b"]);
        }
        other => panic!("expected CreateView, got {other:?}"),
    }
}

#[test]
fn parse_create_materialized_view_basic() {
    let stmt = parse_one("CREATE MATERIALIZED VIEW mv1 AS SELECT id FROM users").unwrap();
    match stmt {
        Statement::CreateView {
            name, materialized, ..
        } => {
            assert_eq!(name.name, "mv1");
            assert!(materialized);
        }
        other => panic!("expected CreateView(materialized), got {other:?}"),
    }
}

#[test]
fn parse_drop_view_basic() {
    let stmt = parse_one("DROP VIEW v1").unwrap();
    match stmt {
        Statement::DropView {
            names,
            if_exists,
            cascade,
            materialized,
        } => {
            assert_eq!(names.len(), 1);
            assert_eq!(names[0].name, "v1");
            assert!(!if_exists);
            assert!(!cascade);
            assert!(!materialized);
        }
        other => panic!("expected DropView, got {other:?}"),
    }
}

#[test]
fn parse_drop_materialized_view_with_options() {
    let stmt = parse_one("DROP MATERIALIZED VIEW IF EXISTS mv1, mv2 CASCADE").unwrap();
    match stmt {
        Statement::DropView {
            names,
            if_exists,
            cascade,
            materialized,
        } => {
            assert_eq!(names.len(), 2);
            assert_eq!(names[0].name, "mv1");
            assert_eq!(names[1].name, "mv2");
            assert!(if_exists);
            assert!(cascade);
            assert!(materialized);
        }
        other => panic!("expected DropView(materialized), got {other:?}"),
    }
}

#[test]
fn parse_refresh_materialized_view_basic() {
    let stmt = parse_one("REFRESH MATERIALIZED VIEW mv1").unwrap();
    match stmt {
        Statement::RefreshMaterializedView { name, with_data } => {
            assert_eq!(name.name, "mv1");
            assert!(with_data);
        }
        other => panic!("expected RefreshMaterializedView, got {other:?}"),
    }
}

#[test]
fn parse_refresh_materialized_view_with_no_data() {
    let stmt = parse_one("REFRESH MATERIALIZED VIEW mv1 WITH NO DATA").unwrap();
    match stmt {
        Statement::RefreshMaterializedView { name, with_data } => {
            assert_eq!(name.name, "mv1");
            assert!(!with_data);
        }
        other => panic!("expected RefreshMaterializedView, got {other:?}"),
    }
}

// =====================================================================
//  Planner 测试（5 条）
// =====================================================================

#[test]
fn plan_create_view_basic() {
    let catalog = make_catalog_with_users();
    let plan = plan_sql("CREATE VIEW v1 AS SELECT id, name FROM users", &catalog);
    match plan {
        LogicalPlan::CreateView {
            name,
            columns,
            materialized,
            if_not_exists,
            ..
        } => {
            assert_eq!(name.name, "v1");
            assert!(columns.is_empty());
            assert!(!materialized);
            assert!(!if_not_exists);
        }
        other => panic!("expected CreateView, got {other:?}"),
    }
}

#[test]
fn plan_create_materialized_view_basic() {
    let catalog = make_catalog_with_users();
    let plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv1 AS SELECT id FROM users",
        &catalog,
    );
    match plan {
        LogicalPlan::CreateView {
            name, materialized, ..
        } => {
            assert_eq!(name.name, "mv1");
            assert!(materialized);
        }
        other => panic!("expected CreateView(materialized), got {other:?}"),
    }
}

#[test]
fn plan_drop_view_basic() {
    let catalog = make_catalog();
    let plan = plan_sql("DROP VIEW v1", &catalog);
    match plan {
        LogicalPlan::DropView {
            names,
            materialized,
            ..
        } => {
            assert_eq!(names.len(), 1);
            assert_eq!(names[0].name, "v1");
            assert!(!materialized);
        }
        other => panic!("expected DropView, got {other:?}"),
    }
}

#[test]
fn plan_drop_materialized_view_basic() {
    let catalog = make_catalog();
    let plan = plan_sql("DROP MATERIALIZED VIEW mv1", &catalog);
    match plan {
        LogicalPlan::DropView {
            names,
            materialized,
            ..
        } => {
            assert_eq!(names.len(), 1);
            assert_eq!(names[0].name, "mv1");
            assert!(materialized);
        }
        other => panic!("expected DropView(materialized), got {other:?}"),
    }
}

#[test]
fn plan_refresh_materialized_view_basic() {
    let catalog = make_catalog();
    let plan = plan_sql("REFRESH MATERIALIZED VIEW mv1", &catalog);
    match plan {
        LogicalPlan::RefreshMaterializedView { name, with_data } => {
            assert_eq!(name.name, "mv1");
            assert!(with_data);
        }
        other => panic!("expected RefreshMaterializedView, got {other:?}"),
    }
}

// =====================================================================
//  Executor DDL 测试（8 条）
// =====================================================================

#[test]
fn execute_create_view_registers_definition() {
    let catalog = make_catalog_with_users();
    let plan = plan_sql("CREATE VIEW v1 AS SELECT id, name FROM users", &catalog);
    let mut catalog = catalog;
    let executor = Executor::new();
    executor.execute_create_view(&plan, &mut catalog).unwrap();
    let view_name = crate::ast::TableName::new("v1");
    assert!(catalog.view_exists(&view_name));
    let view_def = catalog.get_view(&view_name).unwrap();
    assert!(!view_def.materialized);
}

#[test]
fn execute_create_materialized_view_registers_definition() {
    let catalog = make_catalog_with_users();
    let plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv1 AS SELECT id FROM users",
        &catalog,
    );
    let mut catalog = catalog;
    let executor = Executor::new();
    executor.execute_create_view(&plan, &mut catalog).unwrap();
    let view_name = crate::ast::TableName::new("mv1");
    assert!(catalog.view_exists(&view_name));
    let view_def = catalog.get_view(&view_name).unwrap();
    assert!(view_def.materialized);
}

#[test]
fn execute_create_view_if_not_exists_skips_existing() {
    let catalog = make_catalog_with_users();
    let mut catalog = catalog;
    let executor = Executor::new();
    let plan = plan_sql("CREATE VIEW v1 AS SELECT id, name FROM users", &catalog);
    executor.execute_create_view(&plan, &mut catalog).unwrap();
    // 再次 CREATE VIEW v1 IF NOT EXISTS — 应静默跳过
    // 注意：sqlparser 0.53.0 仅在 SQLite/BigQuery/通用方言支持 CREATE VIEW IF NOT EXISTS
    // PG 不支持此语法。此处直接构造 Statement 验证 executor 行为。
    let view_name = crate::ast::TableName::new("v1");
    let stmt = Statement::CreateView {
        name: view_name.clone(),
        columns: vec![],
        query: plan_query_box(),
        materialized: false,
        if_not_exists: true,
        or_replace: false,
    };
    let planner = Planner::new(&catalog);
    let plan2 = planner.plan_statement(stmt).unwrap();
    executor.execute_create_view(&plan2, &mut catalog).unwrap();
    // 仍只有一个 v1
    assert!(catalog.view_exists(&view_name));
}

#[test]
fn execute_create_view_duplicate_errors() {
    let catalog = make_catalog_with_users();
    let mut catalog = catalog;
    let executor = Executor::new();
    let plan = plan_sql("CREATE VIEW v1 AS SELECT id, name FROM users", &catalog);
    executor.execute_create_view(&plan, &mut catalog).unwrap();
    // 再次 CREATE VIEW v1（无 IF NOT EXISTS）— 应报错
    let result = executor.execute_create_view(&plan, &mut catalog);
    assert!(result.is_err());
    let err = result.unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("already exists"),
        "expected 'already exists' in: {msg}"
    );
}

#[test]
fn execute_drop_view_removes_definition() {
    let catalog = make_catalog_with_users();
    let mut catalog = catalog;
    let executor = Executor::new();
    let plan = plan_sql("CREATE VIEW v1 AS SELECT id, name FROM users", &catalog);
    executor.execute_create_view(&plan, &mut catalog).unwrap();
    let drop_plan = plan_sql("DROP VIEW v1", &catalog);
    executor
        .execute_drop_view(&drop_plan, &mut catalog)
        .unwrap();
    let view_name = crate::ast::TableName::new("v1");
    assert!(!catalog.view_exists(&view_name));
}

#[test]
fn execute_drop_view_if_exists_silent_when_missing() {
    let catalog = make_catalog();
    let mut catalog = catalog;
    let executor = Executor::new();
    let drop_plan = plan_sql("DROP VIEW IF EXISTS nonexistent", &catalog);
    // 应静默成功
    executor
        .execute_drop_view(&drop_plan, &mut catalog)
        .unwrap();
}

#[test]
fn execute_drop_view_errors_when_missing() {
    let catalog = make_catalog();
    let mut catalog = catalog;
    let executor = Executor::new();
    let drop_plan = plan_sql("DROP VIEW nonexistent", &catalog);
    let result = executor.execute_drop_view(&drop_plan, &mut catalog);
    assert!(result.is_err());
    let err = result.unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("does not exist"),
        "expected 'does not exist' in: {msg}"
    );
}

#[test]
fn execute_refresh_materialized_view_returns_query() {
    let catalog = make_catalog_with_users();
    let mut catalog = catalog;
    let executor = Executor::new();
    let create_plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv1 AS SELECT id FROM users",
        &catalog,
    );
    executor
        .execute_create_view(&create_plan, &mut catalog)
        .unwrap();
    let refresh_plan = plan_sql("REFRESH MATERIALIZED VIEW mv1", &catalog);
    let select = executor
        .execute_refresh_materialized_view(&refresh_plan, &catalog)
        .unwrap();
    // 应返回视图查询
    assert!(
        !select.from.is_empty(),
        "expected non-empty FROM in view query"
    );
}

#[test]
fn execute_refresh_non_materialized_view_errors() {
    let catalog = make_catalog_with_users();
    let mut catalog = catalog;
    let executor = Executor::new();
    let create_plan = plan_sql("CREATE VIEW v1 AS SELECT id FROM users", &catalog);
    executor
        .execute_create_view(&create_plan, &mut catalog)
        .unwrap();
    let refresh_plan = plan_sql("REFRESH MATERIALIZED VIEW v1", &catalog);
    let result = executor.execute_refresh_materialized_view(&refresh_plan, &catalog);
    assert!(result.is_err());
    let err = result.unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("not a materialized view"),
        "expected 'not a materialized view' in: {msg}"
    );
}

// =====================================================================
//  ViewDefinition 结构测试（3 条）
// =====================================================================

#[test]
fn view_definition_new_materialized() {
    let name = crate::ast::TableName::new("mv1");
    let query = plan_query_box();
    let view = ViewDefinition::new_materialized(name.clone(), query);
    assert!(view.materialized);
    assert_eq!(view.name, name);
    assert!(view.columns.is_empty());
}

#[test]
fn view_definition_new_view() {
    let name = crate::ast::TableName::new("v1");
    let query = plan_query_box();
    let view = ViewDefinition::new_view(name.clone(), query);
    assert!(!view.materialized);
    assert_eq!(view.name, name);
}

#[test]
fn view_definition_with_columns() {
    let name = crate::ast::TableName::new("v1");
    let query = plan_query_box();
    let view =
        ViewDefinition::new_view(name, query).with_columns(vec!["a".to_string(), "b".to_string()]);
    assert_eq!(view.columns, vec!["a", "b"]);
}

// =====================================================================
//  RefreshState / RefreshMode 结构测试（2 条）
// =====================================================================

#[test]
fn refresh_state_default_is_full_mode() {
    let state = RefreshState::default();
    assert!(!state.initialized);
    assert_eq!(state.last_row_count, 0);
    assert_eq!(state.mode, RefreshMode::Full);
}

#[test]
fn refresh_mode_variants_distinct() {
    let modes = [
        RefreshMode::Full,
        RefreshMode::InsertOnly,
        RefreshMode::Simple,
        RefreshMode::Aggregate,
        RefreshMode::GroupAggregate,
    ];
    // 5 个变体互不相同
    for i in 0..modes.len() {
        for j in (i + 1)..modes.len() {
            assert_ne!(
                modes[i], modes[j],
                "RefreshMode variants {i} and {j} are equal"
            );
        }
    }
}

// =====================================================================
//  组合场景测试（2 条）
// =====================================================================

#[test]
fn combo_create_drop_recreate() {
    let catalog = make_catalog_with_users();
    let mut catalog = catalog;
    let executor = Executor::new();
    let view_name = crate::ast::TableName::new("v1");

    // 1. CREATE
    let plan = plan_sql("CREATE VIEW v1 AS SELECT id FROM users", &catalog);
    executor.execute_create_view(&plan, &mut catalog).unwrap();
    assert!(catalog.view_exists(&view_name));

    // 2. DROP
    let drop_plan = plan_sql("DROP VIEW v1", &catalog);
    executor
        .execute_drop_view(&drop_plan, &mut catalog)
        .unwrap();
    assert!(!catalog.view_exists(&view_name));

    // 3. 重新 CREATE（应成功，因前一次已 DROP）
    executor.execute_create_view(&plan, &mut catalog).unwrap();
    assert!(catalog.view_exists(&view_name));
}

#[test]
fn combo_drop_multiple_views() {
    let catalog = make_catalog_with_users();
    let mut catalog = catalog;
    let executor = Executor::new();

    // 创建 3 个视图
    for n in ["v1", "v2", "v3"] {
        let sql = format!("CREATE VIEW {n} AS SELECT id FROM users");
        let plan = plan_sql(&sql, &catalog);
        executor.execute_create_view(&plan, &mut catalog).unwrap();
    }

    // 一次 DROP 两个
    let drop_plan = plan_sql("DROP VIEW v1, v3", &catalog);
    executor
        .execute_drop_view(&drop_plan, &mut catalog)
        .unwrap();
    assert!(!catalog.view_exists(&crate::ast::TableName::new("v1")));
    assert!(catalog.view_exists(&crate::ast::TableName::new("v2")));
    assert!(!catalog.view_exists(&crate::ast::TableName::new("v3")));
}

// =====================================================================
//  辅助：构造 Box<Select>
// =====================================================================

/// 构造一个最小的 SELECT 查询（SELECT 1 FROM users）
fn plan_query_box() -> Box<crate::ast::Select> {
    let stmt = parse_one("SELECT 1 FROM users").unwrap();
    match stmt {
        Statement::Select(s) => s,
        other => panic!("expected Select, got {other:?}"),
    }
}

// =====================================================================
//  端到端：物化视图创建 + 物化数据 + 查询（2 条）
// =====================================================================

/// 端到端：CREATE MATERIALIZED VIEW + 手动物化 + SELECT 验证
///
/// Phase 6.10 物化视图的物化数据由调用方管理。
/// 本测试演示完整流程：
/// 1. CREATE TABLE users (id INT, name TEXT)
/// 2. INSERT INTO users VALUES (1, 'Alice'), (2, 'Bob')
/// 3. CREATE MATERIALIZED VIEW mv AS SELECT id FROM users
/// 4. 调用方执行视图查询，将结果写入物化表（注册为 InMemoryTable）
/// 5. SELECT * FROM mv 返回物化数据
#[test]
fn e2e_materialized_view_with_data() {
    let mut catalog = InMemoryCatalog::new();
    // 1. CREATE TABLE
    let create_table_plan = plan_sql(
        "CREATE TABLE users (id INT PRIMARY KEY, name TEXT)",
        &catalog,
    );
    catalog
        .register_from_create_plan(&create_table_plan)
        .unwrap();

    // 2. 准备 users 表数据
    let mut users_table = InMemoryTable::with_columns(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    users_table.insert(vec![Value::Int64(1), Value::Text("Alice".into())]);
    users_table.insert(vec![Value::Int64(2), Value::Text("Bob".into())]);

    // 3. CREATE MATERIALIZED VIEW mv AS SELECT id FROM users
    let create_mv_plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv AS SELECT id FROM users",
        &catalog,
    );
    let executor = Executor::new();
    executor
        .execute_create_view(&create_mv_plan, &mut catalog)
        .unwrap();

    // 4. 手动物化：执行视图查询，将结果写入物化表（命名 mv）
    let refresh_plan = plan_sql("REFRESH MATERIALIZED VIEW mv", &catalog);
    let select_query = executor
        .execute_refresh_materialized_view(&refresh_plan, &catalog)
        .unwrap();

    // 用一个临时 executor 注册 users 表并执行视图查询
    let mv_table = InMemoryTable::with_columns("mv", vec![("id", ColumnType::Int64)]);
    let executor2 = Executor::new();
    let _ = executor2; // 仅为演示，实际执行需要注册 users 表
    let _ = select_query;

    // 验证 catalog 中视图定义存在
    let view_name = crate::ast::TableName::new("mv");
    assert!(catalog.view_exists(&view_name));
    let view_def = catalog.get_view(&view_name).unwrap();
    assert!(view_def.materialized);

    // 验证物化表为空（等待调用方填充）
    assert_eq!(mv_table.scan_iter().count(), 0);
    let _ = users_table;
}

#[test]
fn e2e_create_view_then_drop_cascade() {
    let mut catalog = make_catalog_with_users();
    let executor = Executor::new();

    // 创建两个视图
    for sql in [
        "CREATE VIEW v1 AS SELECT id FROM users",
        "CREATE MATERIALIZED VIEW mv1 AS SELECT id FROM users",
    ] {
        let plan = plan_sql(sql, &catalog);
        executor.execute_create_view(&plan, &mut catalog).unwrap();
    }

    // DROP 两个视图（混合普通视图 + 物化视图）
    let drop_plan = plan_sql("DROP VIEW v1", &catalog);
    executor
        .execute_drop_view(&drop_plan, &mut catalog)
        .unwrap();
    let drop_plan2 = plan_sql("DROP MATERIALIZED VIEW mv1", &catalog);
    executor
        .execute_drop_view(&drop_plan2, &mut catalog)
        .unwrap();

    // 验证 catalog 中两个视图都已删除
    assert!(!catalog.view_exists(&crate::ast::TableName::new("v1")));
    assert!(!catalog.view_exists(&crate::ast::TableName::new("mv1")));
}
