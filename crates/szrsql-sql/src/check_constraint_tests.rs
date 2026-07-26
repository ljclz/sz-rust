//! Phase 3.30 单元测试 — CHECK 约束运行时校验。
//!
//! 覆盖类别：
//! - Catalog CHECK 注册（4）：列级 CHECK、表级 CHECK、具名 CHECK、add_check_constraint API
//! - INSERT 校验（5）：合法插入、非法插入拒绝、NULL 通过、DEFAULT VALUES 通过、无 catalog 不校验
//! - UPDATE 校验（3）：合法更新、非法更新拒绝、非 CHECK 列更新不校验
//! - 多 CHECK 约束（2）：多个 CHECK 同时校验、任一失败即拒绝
//! - 复合表达式 CHECK（2）：AND 复合、BETWEEN 复合
//! - 边界情况（2）：CHECK 引用不存在列报错、CHECK 求值返回非布尔值
//!
//! 共 18 个测试用例。

use crate::ast::*;
use crate::executor::{ExecutionError, Executor, InMemoryTable, TableStorage};
use crate::parser::parse_one;
use crate::plan::{Catalog, CheckConstraint, InMemoryCatalog, LogicalPlan, Planner};
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

/// 创建带列级 CHECK 的表 + catalog
///
/// 表 `t (id INT PRIMARY KEY, x INT CHECK (x > 0))`
fn make_check_table_setup() -> (InMemoryCatalog, InMemoryTable) {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (id INT PRIMARY KEY, x INT CHECK (x > 0))",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("x", ColumnType::Int64)],
    );
    (catalog, table)
}

/// 创建带表级 CHECK 的表 + catalog
///
/// 表 `t (id INT PRIMARY KEY, x INT, y INT, CHECK (x + y > 0))`
fn make_table_level_check_setup() -> (InMemoryCatalog, InMemoryTable) {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (id INT PRIMARY KEY, x INT, y INT, CHECK (x + y > 0))",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let table = InMemoryTable::with_columns(
        "t",
        vec![
            ("id", ColumnType::Int64),
            ("x", ColumnType::Int64),
            ("y", ColumnType::Int64),
        ],
    );
    (catalog, table)
}

// =====================================================================
//  Catalog CHECK 注册测试（4）
// =====================================================================

#[test]
fn test_check_register_column_level() {
    // 列级 CHECK：`x INT CHECK (x > 0)`
    let (catalog, _) = make_check_table_setup();

    let checks = catalog.get_check_constraints(&TableName::new("t"));
    assert_eq!(checks.len(), 1);
    assert!(checks[0].name.is_none(), "列级 CHECK 应为无名");
}

#[test]
fn test_check_register_table_level() {
    // 表级 CHECK：`CHECK (x + y > 0)`
    let (catalog, _) = make_table_level_check_setup();

    let checks = catalog.get_check_constraints(&TableName::new("t"));
    assert_eq!(checks.len(), 1);
    assert!(checks[0].name.is_none(), "未指定名时表级 CHECK 也为无名");
}

#[test]
fn test_check_register_named() {
    // 具名 CHECK：`CONSTRAINT chk_x CHECK (x > 0)`
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (id INT PRIMARY KEY, x INT, CONSTRAINT chk_x CHECK (x > 0))",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let checks = catalog.get_check_constraints(&TableName::new("t"));
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].name.as_deref(), Some("chk_x"));
}

#[test]
fn test_check_add_constraint_api() {
    // 直接使用 add_check_constraint API
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table("t", vec![("id", ColumnType::Int64)]);

    // 构造一个简单的 CHECK 表达式：id > 0
    let expr = Expr::BinaryOp {
        left: Box::new(Expr::Identifier(vec!["id".to_string()])),
        op: BinaryOp::Gt,
        right: Box::new(Expr::Literal(Value::Int64(0))),
    };
    catalog.add_check_constraint(
        &TableName::new("t"),
        CheckConstraint::with_name("ck_id", expr),
    );

    let checks = catalog.get_check_constraints(&TableName::new("t"));
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].name.as_deref(), Some("ck_id"));
}

// =====================================================================
//  INSERT 校验测试（5）
// =====================================================================

#[test]
fn test_check_insert_valid() {
    // 列级 CHECK(x > 0)，INSERT x=5 → 通过
    let (catalog, mut table) = make_check_table_setup();

    let exec = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("INSERT INTO t VALUES (1, 5)", &catalog);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 1);
}

#[test]
fn test_check_insert_invalid_rejected() {
    // 列级 CHECK(x > 0)，INSERT x=-1 → 拒绝（CheckViolation）
    let (catalog, mut table) = make_check_table_setup();

    let exec = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("INSERT INTO t VALUES (1, -1)", &catalog);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::CheckViolation(_)));
    assert_eq!(table.row_count(), 0, "失败行不应插入");
}

#[test]
fn test_check_insert_null_passes() {
    // 列级 CHECK(x > 0)，INSERT x=NULL → 通过（PG 语义：NULL 视为 true）
    let (catalog, mut table) = make_check_table_setup();

    let exec = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("INSERT INTO t (id) VALUES (1)", &catalog);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Null);
}

#[test]
fn test_check_insert_default_values_passes() {
    // 列级 CHECK(x > 0)，INSERT DEFAULT VALUES → x=NULL → 通过
    let (catalog, mut table) = make_check_table_setup();

    let exec = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("INSERT INTO t DEFAULT VALUES", &catalog);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
}

#[test]
fn test_check_insert_without_catalog_no_validation() {
    // 无 catalog 绑定 → 不校验 CHECK，即使违反也成功
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (id INT PRIMARY KEY, x INT CHECK (x > 0))",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("x", ColumnType::Int64)],
    );

    // Executor 未绑定 catalog → 跳过 CHECK 校验
    let exec = Executor::new();
    let plan = plan_sql("INSERT INTO t VALUES (1, -1)", &catalog);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.row_count(), 1);
}

// =====================================================================
//  UPDATE 校验测试（3）
// =====================================================================

#[test]
fn test_check_update_valid() {
    // 列级 CHECK(x > 0)，UPDATE x=10 → 通过
    let (catalog, mut table) = make_check_table_setup();
    table.insert(vec![Value::Int64(1), Value::Int64(5)]);

    let exec = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("UPDATE t SET x = 10 WHERE id = 1", &catalog);
    let result = exec.execute_update(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Int64(10));
}

#[test]
fn test_check_update_invalid_rejected() {
    // 列级 CHECK(x > 0)，UPDATE x=0 → 拒绝（CheckViolation）
    let (catalog, mut table) = make_check_table_setup();
    table.insert(vec![Value::Int64(1), Value::Int64(5)]);

    let exec = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("UPDATE t SET x = 0 WHERE id = 1", &catalog);
    let err = exec.execute_update(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::CheckViolation(_)));
    // 行未被更新：x 仍为 5
    assert_eq!(table.get_row(0).unwrap()[1], Value::Int64(5));
}

#[test]
fn test_check_update_non_check_column_no_validation() {
    // 列级 CHECK(x > 0)，UPDATE id（非 CHECK 列）→ 不触发 CHECK 校验，应成功
    let (catalog, mut table) = make_check_table_setup();
    table.insert(vec![Value::Int64(1), Value::Int64(5)]);

    let exec = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("UPDATE t SET id = 100 WHERE id = 1", &catalog);
    let result = exec.execute_update(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.get_row(0).unwrap()[0], Value::Int64(100));
    // x 未变，仍为 5
    assert_eq!(table.get_row(0).unwrap()[1], Value::Int64(5));
}

// =====================================================================
//  多 CHECK 约束测试（2）
// =====================================================================

#[test]
fn test_check_multiple_constraints_all_pass() {
    // 多个 CHECK：x > 0 AND y < 100，两个都满足 → 通过
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (id INT PRIMARY KEY, x INT CHECK (x > 0), y INT CHECK (y < 100))",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![
            ("id", ColumnType::Int64),
            ("x", ColumnType::Int64),
            ("y", ColumnType::Int64),
        ],
    );

    let exec = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("INSERT INTO t VALUES (1, 5, 50)", &catalog);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);

    // 验证 catalog 中有两个 CHECK
    let checks = catalog.get_check_constraints(&TableName::new("t"));
    assert_eq!(checks.len(), 2);
}

#[test]
fn test_check_multiple_constraints_any_fail_rejects() {
    // 多个 CHECK：x > 0 AND y < 100，y=200 违反第二个 → 拒绝
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (id INT PRIMARY KEY, x INT CHECK (x > 0), y INT CHECK (y < 100))",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![
            ("id", ColumnType::Int64),
            ("x", ColumnType::Int64),
            ("y", ColumnType::Int64),
        ],
    );

    let exec = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("INSERT INTO t VALUES (1, 5, 200)", &catalog);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::CheckViolation(_)));
    assert_eq!(table.row_count(), 0);
}

// =====================================================================
//  复合表达式 CHECK 测试（2）
// =====================================================================

#[test]
fn test_check_compound_and_expression() {
    // 复合 CHECK：x > 0 AND x < 100
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (id INT PRIMARY KEY, x INT, CHECK (x > 0 AND x < 100))",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("x", ColumnType::Int64)],
    );

    let exec = Executor::new().with_catalog(&catalog);

    // x=50 → 满足两个条件
    let plan = plan_sql("INSERT INTO t VALUES (1, 50)", &catalog);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);

    // x=200 → 违反 x < 100
    let plan = plan_sql("INSERT INTO t VALUES (2, 200)", &catalog);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::CheckViolation(_)));

    // x=-1 → 违反 x > 0
    let plan = plan_sql("INSERT INTO t VALUES (3, -1)", &catalog);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::CheckViolation(_)));

    assert_eq!(table.row_count(), 1, "只有第一条 INSERT 应成功");
}

#[test]
fn test_check_between_expression() {
    // BETWEEN CHECK：x BETWEEN 1 AND 10
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (id INT PRIMARY KEY, x INT, CHECK (x BETWEEN 1 AND 10))",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("x", ColumnType::Int64)],
    );

    let exec = Executor::new().with_catalog(&catalog);

    // x=5 → 在 [1, 10] 范围内 → 通过
    let plan = plan_sql("INSERT INTO t VALUES (1, 5)", &catalog);
    exec.execute_insert(&plan, &mut table).unwrap();

    // x=0 → 不在范围内 → 拒绝
    let plan = plan_sql("INSERT INTO t VALUES (2, 0)", &catalog);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::CheckViolation(_)));

    // x=11 → 不在范围内 → 拒绝
    let plan = plan_sql("INSERT INTO t VALUES (3, 11)", &catalog);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::CheckViolation(_)));

    assert_eq!(table.row_count(), 1);
}

// =====================================================================
//  边界情况测试（2）
// =====================================================================

#[test]
fn test_check_reference_unknown_column_evaluates_to_error() {
    // CHECK 引用不存在的列 → 求值报错 → 包装为 CheckViolation
    let mut catalog = InMemoryCatalog::new();
    // 注意：CHECK (unknown_col > 0) 在解析时不会校验列存在
    let plan = plan_sql(
        "CREATE TABLE t (id INT PRIMARY KEY, x INT, CHECK (unknown_col > 0))",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let mut table = InMemoryTable::with_columns(
        "t",
        vec![("id", ColumnType::Int64), ("x", ColumnType::Int64)],
    );

    let exec = Executor::new().with_catalog(&catalog);
    let plan = plan_sql("INSERT INTO t VALUES (1, 5)", &catalog);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    // 求值时找不到列 → 报 CheckViolation（包装 evaluation error）
    assert!(
        matches!(err, ExecutionError::CheckViolation(_)),
        "expected CheckViolation, got: {err:?}"
    );
}

#[test]
fn test_check_table_level_multi_column() {
    // 表级 CHECK 引用多列：CHECK (x + y > 0)
    let (catalog, mut table) = make_table_level_check_setup();

    let exec = Executor::new().with_catalog(&catalog);

    // x=5, y=10 → x+y=15 > 0 → 通过
    let plan = plan_sql("INSERT INTO t VALUES (1, 5, 10)", &catalog);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);

    // x=-10, y=-5 → x+y=-15 < 0 → 拒绝
    let plan = plan_sql("INSERT INTO t VALUES (2, -10, -5)", &catalog);
    let err = exec.execute_insert(&plan, &mut table).unwrap_err();
    assert!(matches!(err, ExecutionError::CheckViolation(_)));

    // x=5, y=NULL → x+y=NULL > 0 → NULL → 通过（PG 语义）
    let plan = plan_sql("INSERT INTO t VALUES (3, 5, NULL)", &catalog);
    let result = exec.execute_insert(&plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);

    assert_eq!(table.row_count(), 2);
}

// =====================================================================
//  UPDATE 通过 execute_update_with_cascades 校验 CHECK（1）
// =====================================================================

#[test]
fn test_check_update_with_cascades_validates_check() {
    // execute_update_with_cascades 也应校验 CHECK
    let (catalog, mut table) = make_check_table_setup();
    table.insert(vec![Value::Int64(1), Value::Int64(5)]);

    let exec = Executor::new().with_catalog(&catalog);

    // 合法更新：x=20
    let plan = plan_sql("UPDATE t SET x = 20 WHERE id = 1", &catalog);
    let (result, _cascades) = exec
        .execute_update_with_cascades(&plan, &mut table)
        .unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Int64(20));

    // 非法更新：x=0 → 拒绝
    let plan = plan_sql("UPDATE t SET x = 0 WHERE id = 1", &catalog);
    let err = exec
        .execute_update_with_cascades(&plan, &mut table)
        .unwrap_err();
    assert!(matches!(err, ExecutionError::CheckViolation(_)));
    // x 仍为 20
    assert_eq!(table.get_row(0).unwrap()[1], Value::Int64(20));
}
