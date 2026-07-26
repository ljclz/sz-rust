//! Phase 3.34 单元测试 — SHOW/SET 命令。
//!
//! 覆盖类别：
//! - Parser（6）：SHOW TABLES、SHOW CREATE TABLE、SHOW variable、SET NAMES 'charset'、
//!   SET NAMES 'charset' COLLATE 'collation'、SET variable = value
//! - Plan（5）：ShowTables、ShowCreateTable、SetNames、SetVariable、ShowVariable 计划生成
//! - Executor SHOW TABLES（3）：空 catalog、多表按名排序、单表
//! - Executor SHOW CREATE TABLE（2）：DDL 渲染含 NOT NULL、多列 DDL
//! - Executor SET NAMES（2）：仅 charset、charset + collation
//! - Executor SET variable（3）：字符串值、整数值、覆盖既有值
//! - Executor SHOW variable（3）：已设置字符串、已设置整数、未设置返回空
//! - 端到端（2）：进度表场景一（SHOW TABLES）、进度表场景二（SET NAMES utf8mb4 + SHOW statement_timeout）
//! - 多语句解析集成（1）：SET NAMES + SHOW TABLES + SET variable 混合解析
//!
//! 共 27 个测试用例。

use crate::ast::*;
use crate::executor::{Executor, SessionState};
use crate::parser::{parse_one, parse_sql};
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 解析 SQL 并断言成功
fn must_parse(sql: &str) -> Statement {
    match parse_one(sql) {
        Ok(stmt) => stmt,
        Err(e) => panic!("parse failed for SQL: {sql}\nerror: {e:?}"),
    }
}

/// 解析 + 规划，返回 LogicalPlan
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = must_parse(sql);
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).unwrap_or_else(|e| {
        panic!("plan failed for SQL: {sql}\nerror: {e:?}");
    })
}

/// 构造测试 catalog：含 users(id INT8 NOT NULL, name TEXT) 和 orders(id INT8) 两张表
fn make_test_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    catalog.add_simple_table("orders", vec![("id", ColumnType::Int64)]);
    catalog
}

/// 构造测试 catalog（含 NOT NULL 列）：
/// t (id INT8 NOT NULL, name TEXT, age INT8 NOT NULL)
fn make_test_catalog_with_not_null() -> InMemoryCatalog {
    use crate::plan::TableSchema;
    let mut catalog = InMemoryCatalog::new();
    let cols = vec![
        {
            let mut c = ColumnDefinition::new("id", ColumnType::Int64);
            c.not_null = true;
            c
        },
        ColumnDefinition::new("name", ColumnType::Text),
        {
            let mut c = ColumnDefinition::new("age", ColumnType::Int64);
            c.not_null = true;
            c
        },
    ];
    catalog.add_table(TableSchema {
        name: TableName::new("t"),
        columns: cols,
    });
    catalog
}

// =====================================================================
//  Parser 测试（6）
// =====================================================================

#[test]
fn test_parse_show_tables() {
    let stmt = must_parse("SHOW TABLES");
    match stmt {
        Statement::ShowTables => {}
        other => panic!("expected ShowTables, got {other:?}"),
    }
}

#[test]
fn test_parse_show_create_table() {
    let stmt = must_parse("SHOW CREATE TABLE users");
    match stmt {
        Statement::ShowCreateTable { name } => {
            assert_eq!(name.qualified_name(), "users");
        }
        other => panic!("expected ShowCreateTable, got {other:?}"),
    }
}

#[test]
fn test_parse_show_variable() {
    let stmt = must_parse("SHOW statement_timeout");
    match stmt {
        Statement::ShowVariable { variable } => {
            assert_eq!(variable, "statement_timeout");
        }
        other => panic!("expected ShowVariable, got {other:?}"),
    }
}

#[test]
fn test_parse_set_names_charset_only() {
    let stmt = must_parse("SET NAMES 'utf8mb4'");
    match stmt {
        Statement::SetNames { charset, collation } => {
            assert_eq!(charset, "utf8mb4");
            assert!(collation.is_none());
        }
        other => panic!("expected SetNames, got {other:?}"),
    }
}

#[test]
fn test_parse_set_names_charset_and_collation() {
    let stmt = must_parse("SET NAMES utf8mb4 COLLATE utf8mb4_unicode_ci");
    match stmt {
        Statement::SetNames { charset, collation } => {
            assert_eq!(charset, "utf8mb4");
            assert_eq!(collation.as_deref(), Some("utf8mb4_unicode_ci"));
        }
        other => panic!("expected SetNames, got {other:?}"),
    }
}

#[test]
fn test_parse_set_variable() {
    let stmt = must_parse("SET statement_timeout = 5000");
    match stmt {
        Statement::SetVariable { variable, value } => {
            assert_eq!(variable, "statement_timeout");
            // value 应为 Int64(5000)
            match value {
                Expr::Literal(Value::Int64(n)) => assert_eq!(n, 5000),
                other => panic!("expected Int64 literal, got {other:?}"),
            }
        }
        other => panic!("expected SetVariable, got {other:?}"),
    }
}

// =====================================================================
//  Plan 测试（5）
// =====================================================================

#[test]
fn test_plan_show_tables() {
    let catalog = make_test_catalog();
    let plan = plan_sql("SHOW TABLES", &catalog);
    match plan {
        LogicalPlan::ShowTables => {}
        other => panic!("expected ShowTables plan, got {other:?}"),
    }
}

#[test]
fn test_plan_show_create_table() {
    let catalog = make_test_catalog();
    let plan = plan_sql("SHOW CREATE TABLE users", &catalog);
    match plan {
        LogicalPlan::ShowCreateTable { name } => {
            assert_eq!(name.qualified_name(), "users");
        }
        other => panic!("expected ShowCreateTable plan, got {other:?}"),
    }
}

#[test]
fn test_plan_show_create_table_unknown_table_errors() {
    let catalog = make_test_catalog();
    let stmt = must_parse("SHOW CREATE TABLE nonexistent");
    let planner = Planner::new(&catalog);
    let result = planner.plan_statement(stmt);
    match result {
        Err(crate::plan::PlanError::TableNotFound(name)) => {
            assert_eq!(name, "nonexistent");
        }
        other => panic!("expected TableNotFound error, got {other:?}"),
    }
}

#[test]
fn test_plan_set_names() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql("SET NAMES 'utf8mb4'", &catalog);
    match plan {
        LogicalPlan::SetNames { charset, collation } => {
            assert_eq!(charset, "utf8mb4");
            assert!(collation.is_none());
        }
        other => panic!("expected SetNames plan, got {other:?}"),
    }
}

#[test]
fn test_plan_set_variable() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql("SET statement_timeout = 5000", &catalog);
    match plan {
        LogicalPlan::SetVariable { variable, value } => {
            assert_eq!(variable, "statement_timeout");
            match value {
                Expr::Literal(Value::Int64(n)) => assert_eq!(n, 5000),
                other => panic!("expected Int64 literal, got {other:?}"),
            }
        }
        other => panic!("expected SetVariable plan, got {other:?}"),
    }
}

// =====================================================================
//  Executor: SHOW TABLES 测试（3）
// =====================================================================

#[test]
fn test_execute_show_tables_empty_catalog() {
    let catalog = InMemoryCatalog::new();
    let executor = Executor::new().with_catalog(&catalog);
    let rows = executor
        .execute_show_tables()
        .expect("SHOW TABLES succeeds");
    assert!(rows.is_empty(), "empty catalog should yield no rows");
}

#[test]
fn test_execute_show_tables_sorted() {
    let catalog = make_test_catalog();
    let executor = Executor::new().with_catalog(&catalog);
    let rows = executor
        .execute_show_tables()
        .expect("SHOW TABLES succeeds");
    // 应返回 2 行，每行单列 Text
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].len(), 1);
    assert_eq!(rows[1].len(), 1);
    // 排序后应为 orders, users
    match (&rows[0][0], &rows[1][0]) {
        (Value::Text(a), Value::Text(b)) => {
            assert_eq!(a, "orders");
            assert_eq!(b, "users");
        }
        other => panic!("expected Text values, got {other:?}"),
    }
}

#[test]
fn test_execute_show_tables_single() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table("solotbl", vec![("id", ColumnType::Int64)]);
    let executor = Executor::new().with_catalog(&catalog);
    let rows = executor
        .execute_show_tables()
        .expect("SHOW TABLES succeeds");
    assert_eq!(rows.len(), 1);
    match &rows[0][0] {
        Value::Text(name) => assert_eq!(name, "solotbl"),
        other => panic!("expected Text(solotbl), got {other:?}"),
    }
}

// =====================================================================
//  Executor: SHOW CREATE TABLE 测试（2）
// =====================================================================

#[test]
fn test_execute_show_create_table_with_not_null() {
    let catalog = make_test_catalog_with_not_null();
    let executor = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("SHOW CREATE TABLE t", &catalog);
    let rows = executor
        .execute_show_create_table(&plan)
        .expect("SHOW CREATE TABLE succeeds");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 2);
    match (&rows[0][0], &rows[0][1]) {
        (Value::Text(name), Value::Text(ddl)) => {
            assert_eq!(name, "t");
            // DDL 应包含 CREATE TABLE、列名、NOT NULL
            assert!(ddl.starts_with("CREATE TABLE t ("));
            assert!(ddl.contains("id INT8 NOT NULL"));
            assert!(ddl.contains("name TEXT"));
            // name 列无 NOT NULL，因此 "name TEXT," 后不应出现 NOT NULL
            assert!(
                !ddl.contains("name TEXT NOT NULL"),
                "name column should not be NOT NULL: {ddl}"
            );
            assert!(ddl.contains("age INT8 NOT NULL"));
        }
        other => panic!("expected (Text, Text), got {other:?}"),
    }
}

#[test]
fn test_execute_show_create_table_multi_columns() {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "multi",
        vec![
            ("id", ColumnType::Int64),
            ("score", ColumnType::Float64),
            ("data", ColumnType::Text),
            ("flag", ColumnType::Bool),
        ],
    );
    let executor = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("SHOW CREATE TABLE multi", &catalog);
    let rows = executor
        .execute_show_create_table(&plan)
        .expect("SHOW CREATE TABLE succeeds");
    assert_eq!(rows.len(), 1);
    if let Value::Text(ddl) = &rows[0][1] {
        assert!(ddl.contains("id INT8"));
        assert!(ddl.contains("score FLOAT8"));
        assert!(ddl.contains("data TEXT"));
        assert!(ddl.contains("flag BOOLEAN"));
    } else {
        panic!("expected Text DDL");
    }
}

// =====================================================================
//  Executor: SET NAMES 测试（2）
// =====================================================================

#[test]
fn test_execute_set_names_charset_only() {
    let catalog = InMemoryCatalog::new();
    let executor = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("SET NAMES 'utf8mb4'", &catalog);
    let mut session = SessionState::new();
    let rows = executor
        .execute_set_names(&plan, &mut session)
        .expect("SET NAMES succeeds");
    assert!(rows.is_empty(), "SET NAMES should produce no rows");
    // 验证 SessionState 已写入 names_charset
    match session.get("names_charset") {
        Some(Value::Text(s)) => assert_eq!(s, "utf8mb4"),
        other => panic!("expected names_charset=utf8mb4, got {other:?}"),
    }
    // 未设置 collation
    assert!(
        session.get("names_collation").is_none(),
        "collation should not be set"
    );
}

#[test]
fn test_execute_set_names_charset_and_collation() {
    let catalog = InMemoryCatalog::new();
    let executor = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("SET NAMES utf8mb4 COLLATE utf8mb4_unicode_ci", &catalog);
    let mut session = SessionState::new();
    executor
        .execute_set_names(&plan, &mut session)
        .expect("SET NAMES succeeds");
    match session.get("names_charset") {
        Some(Value::Text(s)) => assert_eq!(s, "utf8mb4"),
        other => panic!("expected names_charset=utf8mb4, got {other:?}"),
    }
    match session.get("names_collation") {
        Some(Value::Text(s)) => assert_eq!(s, "utf8mb4_unicode_ci"),
        other => panic!("expected names_collation=utf8mb4_unicode_ci, got {other:?}"),
    }
}

// =====================================================================
//  Executor: SET variable 测试（3）
// =====================================================================

#[test]
fn test_execute_set_variable_string_value() {
    let catalog = InMemoryCatalog::new();
    let executor = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("SET search_path = 'public'", &catalog);
    let mut session = SessionState::new();
    let rows = executor
        .execute_set_variable(&plan, &mut session)
        .expect("SET variable succeeds");
    assert!(rows.is_empty(), "SET variable should produce no rows");
    match session.get("search_path") {
        Some(Value::Text(s)) => assert_eq!(s, "public"),
        other => panic!("expected search_path=public, got {other:?}"),
    }
}

#[test]
fn test_execute_set_variable_integer_value() {
    let catalog = InMemoryCatalog::new();
    let executor = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("SET statement_timeout = 5000", &catalog);
    let mut session = SessionState::new();
    executor
        .execute_set_variable(&plan, &mut session)
        .expect("SET variable succeeds");
    match session.get("statement_timeout") {
        Some(Value::Int64(n)) => assert_eq!(*n, 5000),
        other => panic!("expected statement_timeout=5000, got {other:?}"),
    }
}

#[test]
fn test_execute_set_variable_overwrite() {
    let catalog = InMemoryCatalog::new();
    let executor = Executor::new().with_catalog(&catalog);
    let mut session = SessionState::new();
    // 第一次设置
    let plan1 = plan_sql("SET statement_timeout = 5000", &catalog);
    executor
        .execute_set_variable(&plan1, &mut session)
        .expect("first SET succeeds");
    // 第二次覆盖
    let plan2 = plan_sql("SET statement_timeout = 10000", &catalog);
    executor
        .execute_set_variable(&plan2, &mut session)
        .expect("second SET succeeds");
    match session.get("statement_timeout") {
        Some(Value::Int64(n)) => assert_eq!(*n, 10000, "value should be overwritten"),
        other => panic!("expected statement_timeout=10000, got {other:?}"),
    }
    // 变量总数应为 1（覆盖，不新增）
    assert_eq!(
        session.len(),
        1,
        "session should contain exactly 1 variable"
    );
}

// =====================================================================
//  Executor: SHOW variable 测试（3）
// =====================================================================

#[test]
fn test_execute_show_variable_string_value() {
    let catalog = InMemoryCatalog::new();
    let executor = Executor::new().with_catalog(&catalog);
    let mut session = SessionState::new();
    session.set("search_path", Value::Text("public".into()));
    let plan = LogicalPlan::ShowVariable {
        variable: "search_path".into(),
    };
    let rows = executor
        .execute_show_variable(&plan, &session)
        .expect("SHOW variable succeeds");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 1);
    match &rows[0][0] {
        Value::Text(s) => assert_eq!(s, "public"),
        other => panic!("expected Text(public), got {other:?}"),
    }
}

#[test]
fn test_execute_show_variable_integer_value() {
    let catalog = InMemoryCatalog::new();
    let executor = Executor::new().with_catalog(&catalog);
    let mut session = SessionState::new();
    session.set("statement_timeout", Value::Int64(5000));
    let plan = LogicalPlan::ShowVariable {
        variable: "statement_timeout".into(),
    };
    let rows = executor
        .execute_show_variable(&plan, &session)
        .expect("SHOW variable succeeds");
    assert_eq!(rows.len(), 1);
    match &rows[0][0] {
        // 整数通过 value_to_text 转换为文本
        Value::Text(s) => assert_eq!(s, "5000"),
        other => panic!("expected Text(5000), got {other:?}"),
    }
}

#[test]
fn test_execute_show_variable_unset_returns_empty() {
    let catalog = InMemoryCatalog::new();
    let executor = Executor::new().with_catalog(&catalog);
    let session = SessionState::new();
    let plan = LogicalPlan::ShowVariable {
        variable: "nonexistent_var".into(),
    };
    let rows = executor
        .execute_show_variable(&plan, &session)
        .expect("SHOW variable succeeds");
    assert_eq!(rows.len(), 1);
    match &rows[0][0] {
        Value::Text(s) => assert!(s.is_empty(), "unset variable should yield empty string"),
        other => panic!("expected empty Text, got {other:?}"),
    }
}

// =====================================================================
//  端到端测试（2）— 进度表验收场景
// =====================================================================

#[test]
fn test_end_to_end_show_tables_after_create() {
    // 场景一：CREATE TABLE → SHOW TABLES → 列出当前库所有表
    let mut catalog = InMemoryCatalog::new();
    // 先 CREATE TABLE users（通过 catalog 直接注册，模拟执行 DDL 后的状态）
    catalog.add_simple_table(
        "users",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    catalog.add_simple_table("orders", vec![("id", ColumnType::Int64)]);

    // 解析 + 规划 + 执行 SHOW TABLES
    let stmts = parse_sql("SHOW TABLES").expect("parse succeeds");
    assert_eq!(stmts.len(), 1);
    let planner = Planner::new(&catalog);
    let plan = planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .unwrap();
    let executor = Executor::new().with_catalog(&catalog);
    match plan {
        LogicalPlan::ShowTables => {
            let rows = executor
                .execute_show_tables()
                .expect("SHOW TABLES succeeds");
            // 验收标准：列出当前库所有表
            assert_eq!(rows.len(), 2, "should list all 2 tables");
            let names: Vec<String> = rows
                .into_iter()
                .filter_map(|r| {
                    if let Value::Text(s) = r.into_iter().next()? {
                        Some(s)
                    } else {
                        None
                    }
                })
                .collect();
            assert!(names.contains(&"users".to_string()));
            assert!(names.contains(&"orders".to_string()));
        }
        other => panic!("expected ShowTables plan, got {other:?}"),
    }
}

#[test]
fn test_end_to_end_set_names_and_show_variable() {
    // 场景二：SET NAMES utf8mb4 + SET statement_timeout = 5000 + SHOW statement_timeout
    let catalog = InMemoryCatalog::new();
    let executor = Executor::new().with_catalog(&catalog);
    let mut session = SessionState::new();

    // 1. SET NAMES utf8mb4（验证会话字符集设置）
    let plan = plan_sql("SET NAMES utf8mb4", &catalog);
    executor
        .execute_set_names(&plan, &mut session)
        .expect("SET NAMES succeeds");
    match session.get("names_charset") {
        Some(Value::Text(s)) => assert_eq!(s, "utf8mb4"),
        other => panic!("expected names_charset=utf8mb4, got {other:?}"),
    }

    // 2. SET statement_timeout = 5000（验证会话语句超时设置）
    let plan = plan_sql("SET statement_timeout = 5000", &catalog);
    executor
        .execute_set_variable(&plan, &mut session)
        .expect("SET statement_timeout succeeds");

    // 3. SHOW statement_timeout（验证读取已设置的变量）
    let plan = plan_sql("SHOW statement_timeout", &catalog);
    let rows = executor
        .execute_show_variable(&plan, &session)
        .expect("SHOW statement_timeout succeeds");
    assert_eq!(rows.len(), 1);
    match &rows[0][0] {
        Value::Text(s) => assert_eq!(s, "5000"),
        other => panic!("expected Text(5000), got {other:?}"),
    }
}

// =====================================================================
//  Executor: 多语句解析集成测试（1）
// =====================================================================

#[test]
fn test_parse_multiple_show_set_statements() {
    // 多语句混合（验证 contains_set_names_statement 路径与 PG 方言共存）
    let sql = "SET NAMES utf8mb4; SHOW TABLES; SET statement_timeout = 5000";
    let stmts = parse_sql(sql).expect("multi-statement parse succeeds");
    assert_eq!(stmts.len(), 3);
    assert!(matches!(stmts[0], Statement::SetNames { .. }));
    assert!(matches!(stmts[1], Statement::ShowTables));
    assert!(matches!(stmts[2], Statement::SetVariable { .. }));
}
