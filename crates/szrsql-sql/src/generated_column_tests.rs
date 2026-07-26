//! Phase 6.18 集成测试 — 生成列（Generated Columns）。
//!
//! 覆盖类别：
//! - 基本生成列（3 条）：CREATE TABLE + INSERT + 验证生成值；多行 INSERT；SELECT 查询
//! - UPDATE 重计算（2 条）：UPDATE 基列 → 生成列自动更新；多列 UPDATE
//! - 链式生成列（2 条）：生成列引用另一个生成列；三层链式
//! - CHECK 约束引用生成列（1 条）：CHECK 条件包含生成列
//! - 拒绝显式插入生成列（3 条）：显式列模式、无列模式、DEFAULT VALUES
//! - 拒绝 UPDATE 生成列（2 条）：直接 SET 生成列、SET 多列含生成列
//! - 字符串表达式（1 条）：生成列使用字符串函数
//! - INSERT SELECT（1 条）：INSERT ... SELECT ... 带生成列
//! - 多个生成列（1 条）：同一表多个生成列
//!
//! 共 16 个测试用例。

use super::executor::{Executor, InMemoryTable, TableStorage};
use crate::ast::TableName;
use crate::parser::parse_one;
use crate::plan::{Catalog, InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::Value;

// =====================================================================
//  辅助函数
// =====================================================================

/// 解析并规划 SQL（断言成功）
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = parse_one(sql).expect("parse failed");
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).expect("plan failed")
}

/// 创建带生成列的 catalog（t 表：x INT, y INT GENERATED ALWAYS AS (x * 2) STORED）
fn make_catalog_basic() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (x INT, y INT GENERATED ALWAYS AS (x * 2) STORED)",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();
    catalog
}

/// 创建带生成列 + CHECK 约束的 catalog
fn make_catalog_with_check() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (x INT, y INT GENERATED ALWAYS AS (x * 2) STORED, CHECK (y > 0))",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();
    catalog
}

/// 创建带链式生成列的 catalog（x INT, y GENERATED AS (x+1), z GENERATED AS (y*2)）
fn make_catalog_chain() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (x INT, y INT GENERATED ALWAYS AS (x + 1) STORED, z INT GENERATED ALWAYS AS (y * 2) STORED)",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();
    catalog
}

/// 从 catalog 获取表 schema 并创建 InMemoryTable
fn make_table_from_catalog(catalog: &InMemoryCatalog, name: &str) -> InMemoryTable {
    let schema = catalog
        .get_table(&TableName::new(name))
        .expect("table not found in catalog");
    InMemoryTable::new(schema)
}

// =====================================================================
//  基本生成列测试（3 条）
// =====================================================================

#[test]
fn generated_basic_insert_and_select() {
    let catalog = make_catalog_basic();
    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT x=5 → y 应自动计算为 10
    let insert_plan = plan_sql("INSERT INTO t (x) VALUES (5)", &catalog);
    let exec = Executor::new();
    let result = exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);

    // 验证生成列值
    let row = table.get_row(0).unwrap();
    assert_eq!(row[0], Value::Int64(5)); // x
    assert_eq!(row[1], Value::Int64(10)); // y = x * 2 = 10
}

#[test]
fn generated_multiple_rows_insert() {
    let catalog = make_catalog_basic();
    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT 多行
    let insert_plan = plan_sql(
        "INSERT INTO t (x) VALUES (1), (2), (3), (10), (100)",
        &catalog,
    );
    let exec = Executor::new();
    let result = exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 5);

    // 验证每行的生成列值
    assert_eq!(table.get_row(0).unwrap()[1], Value::Int64(2)); // 1*2
    assert_eq!(table.get_row(1).unwrap()[1], Value::Int64(4)); // 2*2
    assert_eq!(table.get_row(2).unwrap()[1], Value::Int64(6)); // 3*2
    assert_eq!(table.get_row(3).unwrap()[1], Value::Int64(20)); // 10*2
    assert_eq!(table.get_row(4).unwrap()[1], Value::Int64(200)); // 100*2
}

#[test]
fn generated_select_query() {
    let catalog = make_catalog_basic();
    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT 数据
    let insert_plan = plan_sql("INSERT INTO t (x) VALUES (7)", &catalog);
    let exec = Executor::new();
    exec.execute_insert(&insert_plan, &mut table).unwrap();

    // SELECT 查询（需注册表到 executor）
    let select_plan = plan_sql("SELECT x, y FROM t", &catalog);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let result = exec.execute(&select_plan).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0][0], Value::Int64(7)); // x
    assert_eq!(result[0][1], Value::Int64(14)); // y = 7*2
}

// =====================================================================
//  UPDATE 重计算测试（2 条）
// =====================================================================

#[test]
fn generated_update_recomputes() {
    let catalog = make_catalog_basic();
    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT x=5 → y=10
    let insert_plan = plan_sql("INSERT INTO t (x) VALUES (5)", &catalog);
    let exec = Executor::new();
    exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(table.get_row(0).unwrap()[1], Value::Int64(10));

    // UPDATE x=6 → y 应自动重计算为 12
    let update_plan = plan_sql("UPDATE t SET x = 6 WHERE x = 5", &catalog);
    let result = exec.execute_update(&update_plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);

    let row = table.get_row(0).unwrap();
    assert_eq!(row[0], Value::Int64(6)); // x
    assert_eq!(row[1], Value::Int64(12)); // y = 6*2 = 12
}

#[test]
fn generated_update_all_rows_recomputes() {
    let catalog = make_catalog_basic();
    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT 多行
    let insert_plan = plan_sql("INSERT INTO t (x) VALUES (1), (2), (3)", &catalog);
    let exec = Executor::new();
    exec.execute_insert(&insert_plan, &mut table).unwrap();

    // UPDATE 全表 x = x + 10
    let update_plan = plan_sql("UPDATE t SET x = x + 10", &catalog);
    let result = exec.execute_update(&update_plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 3);

    // 验证生成列值
    assert_eq!(table.get_row(0).unwrap()[1], Value::Int64(22)); // (1+10)*2
    assert_eq!(table.get_row(1).unwrap()[1], Value::Int64(24)); // (2+10)*2
    assert_eq!(table.get_row(2).unwrap()[1], Value::Int64(26)); // (3+10)*2
}

// =====================================================================
//  链式生成列测试（2 条）
// =====================================================================

#[test]
fn generated_chain_dependency() {
    // x INT, y = x + 1 (generated), z = y * 2 (generated, 引用 y)
    let catalog = make_catalog_chain();
    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT x=5 → y=6, z=12
    let insert_plan = plan_sql("INSERT INTO t (x) VALUES (5)", &catalog);
    let exec = Executor::new();
    exec.execute_insert(&insert_plan, &mut table).unwrap();

    let row = table.get_row(0).unwrap();
    assert_eq!(row[0], Value::Int64(5)); // x
    assert_eq!(row[1], Value::Int64(6)); // y = x+1 = 6
    assert_eq!(row[2], Value::Int64(12)); // z = y*2 = 12

    // UPDATE x=10 → y=11, z=22
    let update_plan = plan_sql("UPDATE t SET x = 10", &catalog);
    exec.execute_update(&update_plan, &mut table).unwrap();

    let row = table.get_row(0).unwrap();
    assert_eq!(row[0], Value::Int64(10)); // x
    assert_eq!(row[1], Value::Int64(11)); // y = 10+1
    assert_eq!(row[2], Value::Int64(22)); // z = 11*2
}

#[test]
fn generated_chain_three_levels() {
    // 三层链式：x, y=x+1, z=y*2, w=z-1
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (x INT, y INT GENERATED ALWAYS AS (x + 1) STORED, z INT GENERATED ALWAYS AS (y * 2) STORED, w INT GENERATED ALWAYS AS (z - 1) STORED)",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT x=10 → y=11, z=22, w=21
    let insert_plan = plan_sql("INSERT INTO t (x) VALUES (10)", &catalog);
    let exec = Executor::new();
    exec.execute_insert(&insert_plan, &mut table).unwrap();

    let row = table.get_row(0).unwrap();
    assert_eq!(row[0], Value::Int64(10)); // x
    assert_eq!(row[1], Value::Int64(11)); // y = 10+1
    assert_eq!(row[2], Value::Int64(22)); // z = 11*2
    assert_eq!(row[3], Value::Int64(21)); // w = 22-1
}

// =====================================================================
//  CHECK 约束引用生成列测试（1 条）
// =====================================================================

#[test]
fn generated_check_constraint_references_generated() {
    let catalog = make_catalog_with_check();
    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT x=5 → y=10, CHECK (y > 0) 通过
    let insert_plan = plan_sql("INSERT INTO t (x) VALUES (5)", &catalog);
    let exec = Executor::new().with_catalog(&catalog);
    let result = exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);
    assert_eq!(table.get_row(0).unwrap()[1], Value::Int64(10));

    // INSERT x=-1 → y=-2, CHECK (y > 0) 失败
    let insert_plan = plan_sql("INSERT INTO t (x) VALUES (-1)", &catalog);
    let result = exec.execute_insert(&insert_plan, &mut table);
    assert!(result.is_err(), "CHECK constraint should reject y=-2");
}

// =====================================================================
//  拒绝显式插入生成列测试（3 条）
// =====================================================================

#[test]
fn generated_reject_explicit_insert_with_column_list() {
    let catalog = make_catalog_basic();

    // INSERT INTO t (x, y) VALUES (5, 10) → 应拒绝（y 是生成列）
    let result = std::panic::catch_unwind(|| {
        let _plan = plan_sql("INSERT INTO t (x, y) VALUES (5, 10)", &catalog);
    });
    assert!(
        result.is_err(),
        "should reject INSERT with explicit generated column"
    );
}

#[test]
fn generated_reject_insert_without_column_list() {
    let catalog = make_catalog_basic();

    // INSERT INTO t VALUES (5, 10) → 应拒绝（表含生成列，必须显式列）
    let result = std::panic::catch_unwind(|| {
        let _plan = plan_sql("INSERT INTO t VALUES (5, 10)", &catalog);
    });
    assert!(
        result.is_err(),
        "should reject INSERT without column list when table has generated columns"
    );
}

#[test]
fn generated_default_values_works() {
    let catalog = make_catalog_basic();
    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT INTO t DEFAULT VALUES → x=NULL, y=NULL*2=NULL
    let insert_plan = plan_sql("INSERT INTO t DEFAULT VALUES", &catalog);
    let exec = Executor::new();
    let result = exec.execute_insert(&insert_plan, &mut table).unwrap();
    assert_eq!(result.affected_rows, 1);

    let row = table.get_row(0).unwrap();
    assert_eq!(row[0], Value::Null); // x = NULL
    assert_eq!(row[1], Value::Null); // y = NULL * 2 = NULL
}

// =====================================================================
//  拒绝 UPDATE 生成列测试（2 条）
// =====================================================================

#[test]
fn generated_reject_update_generated_column() {
    let catalog = make_catalog_basic();

    // UPDATE t SET y = 20 → 应拒绝（y 是生成列）
    let result = std::panic::catch_unwind(|| {
        let _plan = plan_sql("UPDATE t SET y = 20", &catalog);
    });
    assert!(
        result.is_err(),
        "should reject UPDATE SET on generated column"
    );
}

#[test]
fn generated_reject_update_multiple_including_generated() {
    let catalog = make_catalog_basic();

    // UPDATE t SET x = 5, y = 20 → 应拒绝（y 是生成列）
    let result = std::panic::catch_unwind(|| {
        let _plan = plan_sql("UPDATE t SET x = 5, y = 20", &catalog);
    });
    assert!(
        result.is_err(),
        "should reject UPDATE SET including generated column"
    );
}

// =====================================================================
//  字符串表达式生成列测试（1 条）
// =====================================================================

#[test]
fn generated_string_expression() {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (name TEXT, greeting TEXT GENERATED ALWAYS AS (name || '!') STORED)",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT name='hello' → greeting='hello!'
    let insert_plan = plan_sql("INSERT INTO t (name) VALUES ('hello')", &catalog);
    let exec = Executor::new();
    exec.execute_insert(&insert_plan, &mut table).unwrap();

    let row = table.get_row(0).unwrap();
    assert_eq!(row[0], Value::Text("hello".into()));
    assert_eq!(row[1], Value::Text("hello!".into())); // name || '!'
}

// =====================================================================
//  INSERT SELECT 测试（1 条）
// =====================================================================

#[test]
fn generated_insert_select() {
    let mut catalog = InMemoryCatalog::new();

    // 创建源表
    let src_plan = plan_sql("CREATE TABLE src (val INT)", &catalog);
    catalog.register_from_create_plan(&src_plan).unwrap();

    // 创建目标表（含生成列）
    let dst_plan = plan_sql(
        "CREATE TABLE dst (x INT, y INT GENERATED ALWAYS AS (x * 3) STORED)",
        &catalog,
    );
    catalog.register_from_create_plan(&dst_plan).unwrap();

    let mut src_table = make_table_from_catalog(&catalog, "src");
    let mut dst_table = make_table_from_catalog(&catalog, "dst");

    // 填充源表
    let exec = Executor::new();
    let insert_plan = plan_sql("INSERT INTO src (val) VALUES (1), (2), (3)", &catalog);
    exec.execute_insert(&insert_plan, &mut src_table).unwrap();

    // INSERT INTO dst (x) SELECT val FROM src
    let insert_select_plan = plan_sql("INSERT INTO dst (x) SELECT val FROM src", &catalog);
    let mut exec = Executor::new();
    exec.register_table(&src_table);
    let result = exec
        .execute_insert(&insert_select_plan, &mut dst_table)
        .unwrap();
    assert_eq!(result.affected_rows, 3);

    // 验证生成列值
    assert_eq!(dst_table.get_row(0).unwrap()[1], Value::Int64(3)); // 1*3
    assert_eq!(dst_table.get_row(1).unwrap()[1], Value::Int64(6)); // 2*3
    assert_eq!(dst_table.get_row(2).unwrap()[1], Value::Int64(9)); // 3*3
}

// =====================================================================
//  多个生成列测试（1 条）
// =====================================================================

#[test]
fn generated_multiple_in_same_table() {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE t (a INT, b INT GENERATED ALWAYS AS (a + 1) STORED, c INT GENERATED ALWAYS AS (a * 10) STORED)",
        &catalog,
    );
    catalog.register_from_create_plan(&plan).unwrap();

    let mut table = make_table_from_catalog(&catalog, "t");

    // INSERT a=5 → b=6, c=50
    let insert_plan = plan_sql("INSERT INTO t (a) VALUES (5)", &catalog);
    let exec = Executor::new();
    exec.execute_insert(&insert_plan, &mut table).unwrap();

    let row = table.get_row(0).unwrap();
    assert_eq!(row[0], Value::Int64(5)); // a
    assert_eq!(row[1], Value::Int64(6)); // b = a+1
    assert_eq!(row[2], Value::Int64(50)); // c = a*10

    // UPDATE a=10 → b=11, c=100
    let update_plan = plan_sql("UPDATE t SET a = 10", &catalog);
    exec.execute_update(&update_plan, &mut table).unwrap();

    let row = table.get_row(0).unwrap();
    assert_eq!(row[0], Value::Int64(10)); // a
    assert_eq!(row[1], Value::Int64(11)); // b = 10+1
    assert_eq!(row[2], Value::Int64(100)); // c = 10*10
}
