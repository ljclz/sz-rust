//! 临时表（Phase 3.28）测试套件。
//!
//! 覆盖：
//! - Parser（5）：CREATE TEMPORARY TABLE 基础/ON COMMIT DELETE ROWS/PRESERVE ROWS/DROP/IF NOT EXISTS
//! - Planner（3）：temporary 标志传递、on_commit 传递、普通表 temporary=false
//! - Executor 基础（4）：创建+查询、创建+插入+查询、IF NOT EXISTS、重复创建报错
//! - Executor 会话隔离（3）：不同执行器互不可见、会话结束清理、临时表遮蔽普通表
//! - Executor ON COMMIT（3）：DELETE ROWS、PRESERVE ROWS、DROP
//! - Executor DML（2）：INSERT+UPDATE+DELETE、DROP TEMPORARY TABLE
//! - 错误处理（2）：non-temporary plan 调用 create_table_from_plan、错误 plan 类型
//!
//! # 借用模式
//!
//! 由于 `Executor` 持有 `&'a TempTableStore` 只读引用（用于 Scan 读取路径），
//! 而 DML 操作需要 `&mut TempTableStore`，测试中使用以下模式：
//! - **DDL（CREATE/DROP/ON COMMIT）**：直接调用 `TempTableStore` 方法，不经过 Executor
//! - **DML（INSERT/UPDATE/DELETE）**：使用未绑定 temp_store 的 `Executor::new()`，
//!   通过 `temp_store.get_mut()` 获取 `&mut InMemoryTable` 后作为参数传入
//! - **SELECT**：使用 `Executor::new().with_temp_store(&temp_store)` 绑定后执行

use crate::ast::{OnCommitAction, Statement, TableName};
use crate::executor::{Executor, TableStorage, TempTableStore};
use crate::parser::parse_one;
use crate::plan::{Catalog, InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 解析 SQL，失败时 panic
fn must_parse(sql: &str) -> Statement {
    match parse_one(sql) {
        Ok(stmt) => stmt,
        Err(e) => panic!("parse failed for SQL: {sql}\nerror: {e:?}"),
    }
}

/// 解析 + 规划（使用指定 catalog），返回 LogicalPlan
fn plan_sql_with_catalog(sql: &str, catalog: &InMemoryCatalog) -> LogicalPlan {
    let stmt = must_parse(sql);
    let planner = Planner::new(catalog);
    planner.plan_statement(stmt).unwrap_or_else(|e| {
        panic!("plan failed for SQL: {sql}\nerror: {e:?}");
    })
}

/// 将 Vec<Row> 按第一列排序（便于断言）
///
/// 因 `Value` 未实现 `Ord`，这里使用简化的比较逻辑：仅处理 Int64 / Float64 / Text。
fn sort_rows_by_first(rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    use std::cmp::Ordering;
    let mut sorted = rows;
    sorted.sort_by(|a, b| {
        let a_val = a.first().cloned().unwrap_or(Value::Null);
        let b_val = b.first().cloned().unwrap_or(Value::Null);
        match (&a_val, &b_val) {
            (Value::Int64(x), Value::Int64(y)) => x.cmp(y),
            (Value::Float64(x), Value::Float64(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
            (Value::Int64(x), Value::Float64(y)) => {
                (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
            }
            (Value::Float64(x), Value::Int64(y)) => {
                x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
            }
            (Value::Text(x), Value::Text(y)) => x.cmp(y),
            _ => format!("{a_val:?}").cmp(&format!("{b_val:?}")),
        }
    });
    sorted
}

/// 在 temp_store 上执行 INSERT（使用临时 Executor，不绑定 temp_store）
fn exec_insert_on_temp(
    temp_store: &mut TempTableStore,
    catalog: &InMemoryCatalog,
    sql: &str,
) -> usize {
    let plan = plan_sql_with_catalog(sql, catalog);
    let exec = Executor::new();
    let table = temp_store
        .get_mut(extract_table_name(sql))
        .expect("temp table should exist");
    exec.execute_insert(&plan, table)
        .expect("insert should succeed")
        .affected_rows
}

/// 在 temp_store 上执行 UPDATE
fn exec_update_on_temp(
    temp_store: &mut TempTableStore,
    catalog: &InMemoryCatalog,
    sql: &str,
    table_name: &str,
) -> usize {
    let plan = plan_sql_with_catalog(sql, catalog);
    let exec = Executor::new();
    let table = temp_store
        .get_mut(table_name)
        .expect("temp table should exist");
    exec.execute_update(&plan, table)
        .expect("update should succeed")
        .affected_rows
}

/// 在 temp_store 上执行 DELETE
fn exec_delete_on_temp(
    temp_store: &mut TempTableStore,
    catalog: &InMemoryCatalog,
    sql: &str,
    table_name: &str,
) -> usize {
    let plan = plan_sql_with_catalog(sql, catalog);
    let exec = Executor::new();
    let table = temp_store
        .get_mut(table_name)
        .expect("temp table should exist");
    exec.execute_delete(&plan, table)
        .expect("delete should succeed")
        .affected_rows
}

/// 从 INSERT INTO <name> ... 中提取表名（简化实现）
fn extract_table_name(sql: &str) -> &str {
    let lower = sql.to_lowercase();
    if let Some(pos) = lower.find("insert into ") {
        let rest = &sql[pos + 12..];
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '(')
            .unwrap_or(rest.len());
        &rest[..end]
    } else {
        panic!("cannot extract table name from SQL: {sql}");
    }
}

// =====================================================================
//  Parser 测试（5 条）
// =====================================================================

#[test]
fn test_parse_temp_table_basic() {
    let stmt = must_parse("CREATE TEMPORARY TABLE tmp (id INT)");
    match stmt {
        Statement::CreateTable {
            name,
            temporary,
            on_commit,
            ..
        } => {
            assert_eq!(name.qualified_name(), "tmp");
            assert!(temporary, "temporary should be true");
            assert_eq!(on_commit, None, "on_commit should be None by default");
        }
        other => panic!("expected CreateTable, got {other:?}"),
    }
}

#[test]
fn test_parse_temp_table_on_commit_delete_rows() {
    let stmt = must_parse("CREATE TEMPORARY TABLE tmp (id INT) ON COMMIT DELETE ROWS");
    match stmt {
        Statement::CreateTable {
            on_commit: Some(OnCommitAction::DeleteRows),
            temporary: true,
            ..
        } => {}
        other => panic!("expected CreateTable with DeleteRows, got {other:?}"),
    }
}

#[test]
fn test_parse_temp_table_on_commit_preserve_rows() {
    let stmt = must_parse("CREATE TEMPORARY TABLE tmp (id INT) ON COMMIT PRESERVE ROWS");
    match stmt {
        Statement::CreateTable {
            on_commit: Some(OnCommitAction::PreserveRows),
            temporary: true,
            ..
        } => {}
        other => panic!("expected CreateTable with PreserveRows, got {other:?}"),
    }
}

#[test]
fn test_parse_temp_table_on_commit_drop() {
    let stmt = must_parse("CREATE TEMPORARY TABLE tmp (id INT) ON COMMIT DROP");
    match stmt {
        Statement::CreateTable {
            on_commit: Some(OnCommitAction::Drop),
            temporary: true,
            ..
        } => {}
        other => panic!("expected CreateTable with Drop, got {other:?}"),
    }
}

#[test]
fn test_parse_temp_table_if_not_exists() {
    let stmt = must_parse("CREATE TEMPORARY TABLE IF NOT EXISTS tmp (id INT)");
    match stmt {
        Statement::CreateTable {
            if_not_exists: true,
            temporary: true,
            ..
        } => {}
        other => panic!("expected CreateTable with if_not_exists=true, got {other:?}"),
    }
}

// =====================================================================
//  Planner 测试（3 条）
// =====================================================================

#[test]
fn test_plan_temp_table_temporary_flag() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql_with_catalog("CREATE TEMPORARY TABLE tmp (id INT)", &catalog);
    match plan {
        LogicalPlan::CreateTable {
            temporary,
            on_commit,
            ..
        } => {
            assert!(temporary);
            assert_eq!(on_commit, None);
        }
        other => panic!("expected CreateTable plan, got {other:?}"),
    }
}

#[test]
fn test_plan_temp_table_on_commit_flag() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql_with_catalog(
        "CREATE TEMPORARY TABLE tmp (id INT) ON COMMIT DELETE ROWS",
        &catalog,
    );
    match plan {
        LogicalPlan::CreateTable {
            temporary,
            on_commit: Some(OnCommitAction::DeleteRows),
            ..
        } => {
            assert!(temporary);
        }
        other => panic!("expected CreateTable plan, got {other:?}"),
    }
}

#[test]
fn test_plan_regular_table_not_temporary() {
    let catalog = InMemoryCatalog::new();
    let plan = plan_sql_with_catalog("CREATE TABLE regular_t (id INT)", &catalog);
    match plan {
        LogicalPlan::CreateTable {
            temporary: false,
            on_commit: None,
            ..
        } => {}
        other => panic!("expected CreateTable plan with temporary=false, got {other:?}"),
    }
}

// =====================================================================
//  Executor 基础测试（4 条）
// =====================================================================

#[test]
fn test_exec_temp_table_create_and_select() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    // CREATE TEMPORARY TABLE
    let plan = plan_sql_with_catalog("CREATE TEMPORARY TABLE tmp (id INT)", &catalog);
    temp_store
        .create_table_from_plan(&plan, &mut catalog)
        .expect("create temp table should succeed");

    // 临时表存在
    assert!(temp_store.exists("tmp"));
    assert!(temp_store.exists("TMP")); // 大小写不敏感

    // 查询空临时表（绑定 temp_store 后 SELECT）
    let select_plan = plan_sql_with_catalog("SELECT * FROM tmp", &catalog);
    let exec = Executor::new().with_temp_store(&temp_store);
    let rows = exec.execute(&select_plan).expect("select should succeed");
    assert_eq!(rows.len(), 0, "temp table should be empty");
}

#[test]
fn test_exec_temp_table_create_insert_select() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    // CREATE
    let create_plan =
        plan_sql_with_catalog("CREATE TEMPORARY TABLE tmp (id INT, val INT)", &catalog);
    temp_store
        .create_table_from_plan(&create_plan, &mut catalog)
        .expect("create temp table should succeed");

    // INSERT（使用未绑定 temp_store 的 executor）
    let count = exec_insert_on_temp(
        &mut temp_store,
        &catalog,
        "INSERT INTO tmp VALUES (1, 10), (2, 20)",
    );
    assert_eq!(count, 2);

    // SELECT（绑定 temp_store）
    let select_plan = plan_sql_with_catalog("SELECT * FROM tmp", &catalog);
    let exec = Executor::new().with_temp_store(&temp_store);
    let rows = exec.execute(&select_plan).expect("select should succeed");
    assert_eq!(rows.len(), 2);
    let sorted = sort_rows_by_first(rows);
    assert_eq!(sorted[0][0], Value::Int64(1));
    assert_eq!(sorted[0][1], Value::Int64(10));
    assert_eq!(sorted[1][0], Value::Int64(2));
    assert_eq!(sorted[1][1], Value::Int64(20));
}

#[test]
fn test_exec_temp_table_if_not_exists() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    let plan1 = plan_sql_with_catalog("CREATE TEMPORARY TABLE tmp (id INT)", &catalog);
    temp_store
        .create_table_from_plan(&plan1, &mut catalog)
        .expect("first create should succeed");

    // 再次创建（IF NOT EXISTS）— 应静默返回
    let plan2 = plan_sql_with_catalog(
        "CREATE TEMPORARY TABLE IF NOT EXISTS tmp (id INT)",
        &catalog,
    );
    temp_store
        .create_table_from_plan(&plan2, &mut catalog)
        .expect("second create with IF NOT EXISTS should succeed");
}

#[test]
fn test_exec_temp_table_duplicate_create_error() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    let plan1 = plan_sql_with_catalog("CREATE TEMPORARY TABLE tmp (id INT)", &catalog);
    temp_store
        .create_table_from_plan(&plan1, &mut catalog)
        .expect("first create should succeed");

    // 再次创建（不带 IF NOT EXISTS）— 应报错
    let plan2 = plan_sql_with_catalog("CREATE TEMPORARY TABLE tmp (id INT)", &catalog);
    let result = temp_store.create_table_from_plan(&plan2, &mut catalog);
    assert!(result.is_err(), "duplicate create should fail");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("already exists"),
        "error should mention 'already exists', got: {err_msg}"
    );
}

// =====================================================================
//  Executor 会话隔离测试（3 条）
// =====================================================================

#[test]
fn test_exec_temp_table_session_isolation() {
    // 会话 A 创建临时表
    let mut catalog_a = InMemoryCatalog::new();
    let mut temp_store_a = TempTableStore::new();
    let plan_a = plan_sql_with_catalog("CREATE TEMPORARY TABLE tmp (id INT)", &catalog_a);
    temp_store_a
        .create_table_from_plan(&plan_a, &mut catalog_a)
        .expect("create in session A should succeed");

    // 会话 B 不应有 tmp 表
    let catalog_b = InMemoryCatalog::new();
    let temp_store_b = TempTableStore::new();
    assert!(
        !temp_store_b.exists("tmp"),
        "session B should not see session A's temp table"
    );

    // 会话 B 查询 tmp 应失败（表不存在）
    let plan_b = Planner::new(&catalog_b).plan_statement(must_parse("SELECT * FROM tmp"));
    assert!(
        plan_b.is_err(),
        "plan in session B should fail (table not found)"
    );
}

#[test]
fn test_exec_temp_table_session_end_cleanup() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();
    let plan = plan_sql_with_catalog("CREATE TEMPORARY TABLE tmp (id INT)", &catalog);
    temp_store
        .create_table_from_plan(&plan, &mut catalog)
        .expect("create should succeed");
    assert!(temp_store.exists("tmp"));

    // 模拟会话断开
    temp_store.clear();
    assert!(
        !temp_store.exists("tmp"),
        "temp table should be cleaned up after session end"
    );
    assert_eq!(temp_store.len(), 0, "no temp tables should remain");
}

#[test]
fn test_exec_temp_table_shadows_regular_table() {
    use crate::executor::InMemoryTable;

    // 创建普通表 t1（注册到 catalog 和 executor）
    let mut catalog = InMemoryCatalog::new();
    let regular_table = InMemoryTable::with_columns(
        "t1",
        vec![("id", ColumnType::Int64), ("val", ColumnType::Int64)],
    );
    catalog.add_table(regular_table.schema().clone());

    let mut temp_store = TempTableStore::new();

    // 创建同名临时表 t1（遮蔽普通表）
    let create_temp =
        plan_sql_with_catalog("CREATE TEMPORARY TABLE t1 (id INT, val INT)", &catalog);
    temp_store
        .create_table_from_plan(&create_temp, &mut catalog)
        .expect("create temp table should succeed");

    // 向临时表 t1 插入数据 (1, 200)
    exec_insert_on_temp(&mut temp_store, &catalog, "INSERT INTO t1 VALUES (1, 200)");

    // SELECT * FROM t1 — 应返回临时表的数据 (1, 200)
    // executor 同时绑定普通表和 temp_store，temp_store 优先
    let select_plan = plan_sql_with_catalog("SELECT * FROM t1", &catalog);
    let rows = {
        let mut exec = Executor::new();
        exec.register_table(&regular_table);
        exec.set_temp_store(&temp_store);
        exec.execute(&select_plan).expect("select should succeed")
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(1));
    assert_eq!(
        rows[0][1],
        Value::Int64(200),
        "temp table should shadow regular table"
    );

    // 删除临时表后，应能看到普通表
    temp_store.drop_table("t1");
    let rows_after_drop = {
        let mut exec = Executor::new();
        exec.register_table(&regular_table);
        exec.set_temp_store(&temp_store);
        exec.execute(&select_plan)
            .expect("select after drop should succeed")
    };
    // 普通表是空的
    assert_eq!(
        rows_after_drop.len(),
        0,
        "after dropping temp table, regular table should be visible"
    );
}

// =====================================================================
//  Executor ON COMMIT 测试（3 条）
// =====================================================================

#[test]
fn test_exec_temp_table_on_commit_delete_rows() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    let create_plan = plan_sql_with_catalog(
        "CREATE TEMPORARY TABLE tmp (id INT) ON COMMIT DELETE ROWS",
        &catalog,
    );
    temp_store
        .create_table_from_plan(&create_plan, &mut catalog)
        .expect("create should succeed");

    // 插入数据
    exec_insert_on_temp(
        &mut temp_store,
        &catalog,
        "INSERT INTO tmp VALUES (1), (2), (3)",
    );

    // COMMIT — 应清空数据
    let dropped = temp_store
        .on_commit(&mut catalog)
        .expect("on_commit should succeed");
    assert_eq!(dropped.len(), 0, "no tables should be dropped");

    // 验证数据被清空
    let select_plan = plan_sql_with_catalog("SELECT * FROM tmp", &catalog);
    let exec = Executor::new().with_temp_store(&temp_store);
    let rows = exec.execute(&select_plan).expect("select should succeed");
    assert_eq!(
        rows.len(),
        0,
        "data should be cleared after ON COMMIT DELETE ROWS"
    );

    // 表结构应仍存在
    assert!(
        temp_store.exists("tmp"),
        "table structure should still exist after DELETE ROWS"
    );
}

#[test]
fn test_exec_temp_table_on_commit_preserve_rows() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    let create_plan = plan_sql_with_catalog(
        "CREATE TEMPORARY TABLE tmp (id INT) ON COMMIT PRESERVE ROWS",
        &catalog,
    );
    temp_store
        .create_table_from_plan(&create_plan, &mut catalog)
        .expect("create should succeed");

    // 插入数据
    exec_insert_on_temp(&mut temp_store, &catalog, "INSERT INTO tmp VALUES (1), (2)");

    // COMMIT — 应保留数据
    let dropped = temp_store
        .on_commit(&mut catalog)
        .expect("on_commit should succeed");
    assert_eq!(dropped.len(), 0);

    // 验证数据仍存在
    let select_plan = plan_sql_with_catalog("SELECT * FROM tmp", &catalog);
    let exec = Executor::new().with_temp_store(&temp_store);
    let rows = exec.execute(&select_plan).expect("select should succeed");
    assert_eq!(
        rows.len(),
        2,
        "data should be preserved after ON COMMIT PRESERVE ROWS"
    );
}

#[test]
fn test_exec_temp_table_on_commit_drop() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    let create_plan = plan_sql_with_catalog(
        "CREATE TEMPORARY TABLE tmp (id INT) ON COMMIT DROP",
        &catalog,
    );
    temp_store
        .create_table_from_plan(&create_plan, &mut catalog)
        .expect("create should succeed");

    // 插入数据
    exec_insert_on_temp(&mut temp_store, &catalog, "INSERT INTO tmp VALUES (1)");

    // COMMIT — 应删除临时表
    let dropped = temp_store
        .on_commit(&mut catalog)
        .expect("on_commit should succeed");
    assert_eq!(dropped.len(), 1, "one table should be dropped");
    assert_eq!(dropped[0], "tmp");

    // 表应不再存在
    assert!(
        !temp_store.exists("tmp"),
        "temp table should be dropped after ON COMMIT DROP"
    );

    // catalog 中也应不再有该表
    assert!(
        !catalog.table_exists(&TableName::new("tmp")),
        "catalog should not have tmp after ON COMMIT DROP"
    );
}

// =====================================================================
//  Executor DML 测试（2 条）
// =====================================================================

#[test]
fn test_exec_temp_table_update_and_delete() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    let create_plan =
        plan_sql_with_catalog("CREATE TEMPORARY TABLE tmp (id INT, val INT)", &catalog);
    temp_store
        .create_table_from_plan(&create_plan, &mut catalog)
        .expect("create should succeed");

    // 插入数据
    exec_insert_on_temp(
        &mut temp_store,
        &catalog,
        "INSERT INTO tmp VALUES (1, 10), (2, 20), (3, 30)",
    );

    // UPDATE tmp SET val = 999 WHERE id = 2
    let updated = exec_update_on_temp(
        &mut temp_store,
        &catalog,
        "UPDATE tmp SET val = 999 WHERE id = 2",
        "tmp",
    );
    assert_eq!(updated, 1);

    // DELETE FROM tmp WHERE id = 3
    let deleted = exec_delete_on_temp(
        &mut temp_store,
        &catalog,
        "DELETE FROM tmp WHERE id = 3",
        "tmp",
    );
    assert_eq!(deleted, 1);

    // SELECT * FROM tmp — 应返回 (1, 10) 和 (2, 999)
    let select_plan = plan_sql_with_catalog("SELECT * FROM tmp", &catalog);
    let exec = Executor::new().with_temp_store(&temp_store);
    let rows = exec.execute(&select_plan).expect("select should succeed");
    assert_eq!(rows.len(), 2);
    let sorted = sort_rows_by_first(rows);
    assert_eq!(sorted[0][0], Value::Int64(1));
    assert_eq!(sorted[0][1], Value::Int64(10));
    assert_eq!(sorted[1][0], Value::Int64(2));
    assert_eq!(sorted[1][1], Value::Int64(999));
}

#[test]
fn test_exec_temp_table_drop_explicit() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    let create_plan = plan_sql_with_catalog("CREATE TEMPORARY TABLE tmp (id INT)", &catalog);
    temp_store
        .create_table_from_plan(&create_plan, &mut catalog)
        .expect("create should succeed");
    assert!(temp_store.exists("tmp"));

    // 从 catalog 中也移除（模拟完整 DROP TABLE 语义）
    catalog.remove_table(&TableName::new("tmp"));

    // 显式删除临时表
    let dropped = temp_store.drop_table("tmp");
    assert!(
        dropped,
        "drop_table should return true for existing temp table"
    );

    // 再次删除应返回 false
    let dropped_again = temp_store.drop_table("tmp");
    assert!(
        !dropped_again,
        "drop_table should return false for non-existent temp table"
    );

    // 查询应失败（plan 阶段）
    let plan_result = Planner::new(&catalog).plan_statement(must_parse("SELECT * FROM tmp"));
    assert!(
        plan_result.is_err(),
        "plan should fail after temp table is dropped and removed from catalog"
    );
}

// =====================================================================
//  错误处理测试（2 条）
// =====================================================================

#[test]
fn test_exec_temp_table_error_non_temporary_plan() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    // 普通 CREATE TABLE（非临时表）
    let plan = plan_sql_with_catalog("CREATE TABLE regular_t (id INT)", &catalog);
    let result = temp_store.create_table_from_plan(&plan, &mut catalog);
    assert!(result.is_err(), "should fail for non-temporary plan");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("temporary=false"),
        "error should mention temporary=false, got: {err_msg}"
    );
}

#[test]
fn test_exec_temp_table_error_wrong_plan_type() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    // 传入 SELECT 计划
    let plan = plan_sql_with_catalog("SELECT 1", &catalog);
    let result = temp_store.create_table_from_plan(&plan, &mut catalog);
    assert!(result.is_err(), "should fail for non-CreateTable plan");
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("expected CreateTable plan"),
        "error should mention expected CreateTable plan, got: {err_msg}"
    );
}

// =====================================================================
//  完整工作流测试（1 条）
// =====================================================================

#[test]
fn test_exec_temp_table_full_workflow() {
    let mut catalog = InMemoryCatalog::new();
    let mut temp_store = TempTableStore::new();

    // 1. 创建临时表
    let create_plan = plan_sql_with_catalog(
        "CREATE TEMPORARY TABLE session_cache (key TEXT, value INT) ON COMMIT DELETE ROWS",
        &catalog,
    );
    temp_store
        .create_table_from_plan(&create_plan, &mut catalog)
        .expect("create should succeed");

    // 2. 插入数据
    exec_insert_on_temp(
        &mut temp_store,
        &catalog,
        "INSERT INTO session_cache VALUES ('a', 1), ('b', 2), ('c', 3)",
    );

    // 3. 查询验证
    let select_plan = plan_sql_with_catalog("SELECT * FROM session_cache", &catalog);
    {
        let exec = Executor::new().with_temp_store(&temp_store);
        let rows = exec.execute(&select_plan).expect("select should succeed");
        assert_eq!(rows.len(), 3);
    }

    // 4. 聚合查询
    let agg_plan = plan_sql_with_catalog("SELECT COUNT(*) FROM session_cache", &catalog);
    {
        let exec = Executor::new().with_temp_store(&temp_store);
        let agg_rows = exec.execute(&agg_plan).expect("aggregate should succeed");
        assert_eq!(agg_rows.len(), 1);
        assert_eq!(agg_rows[0][0], Value::Int64(3));
    }

    // 5. COMMIT — 清空数据
    temp_store
        .on_commit(&mut catalog)
        .expect("on_commit should succeed");

    // 6. 验证数据被清空但表结构存在
    {
        let exec = Executor::new().with_temp_store(&temp_store);
        let rows_after = exec
            .execute(&select_plan)
            .expect("select after commit should succeed");
        assert_eq!(rows_after.len(), 0, "data should be cleared");
    }
    assert!(
        temp_store.exists("session_cache"),
        "table structure should still exist"
    );

    // 7. 再次插入数据
    exec_insert_on_temp(
        &mut temp_store,
        &catalog,
        "INSERT INTO session_cache VALUES ('x', 100)",
    );

    // 8. 会话结束 — 清理所有临时表
    temp_store.clear();
    assert!(!temp_store.exists("session_cache"));
    assert_eq!(temp_store.len(), 0);
}
