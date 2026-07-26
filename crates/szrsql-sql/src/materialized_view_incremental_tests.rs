//! Phase 6.11 集成测试 — INSERT_ONLY 增量刷新（CDC 模式）。
//!
//! 覆盖类别：
//! - CDC 单事件：单行 INSERT → CDC 捕获 → 增量刷新 → MV 追加 1 行
//! - CDC 批量：1000 行 INSERT → 增量刷新 → MV 追加 1000 行
//! - 多次刷新：INSERT → refresh → INSERT → refresh → 累积正确
//! - 空刷新：无 CDC 事件 → refresh → no-op（0 行追加）
//! - 高水位：源表 INSERT 后高水位正确推进
//! - 错误场景：视图不存在 / 非物化视图
//! - 增量 vs 全量等价：增量刷新结果 == 全量刷新结果
//! - Stress 测试：10万行 INSERT → 增量刷新 → 结果正确（性能合理）
//!
//! 共 14 个测试用例。

use super::executor::Executor;
use super::materialized_view::{
    CdcEvent, CdcFeed, MaterializedViewStore, RefreshMode, RefreshOutcome,
};
use crate::ast::{Statement, TableName};
use crate::parser::parse_one;
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

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

/// 创建并注册一个物化视图 `mv`，查询为 `SELECT id FROM users`
fn setup_materialized_view(catalog: &mut InMemoryCatalog) {
    let plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv AS SELECT id FROM users",
        catalog,
    );
    let executor = Executor::new();
    executor.execute_create_view(&plan, catalog).unwrap();
}

// =====================================================================
//  CDC 单事件测试
// =====================================================================

#[test]
fn incremental_refresh_single_insert_appends_one_row() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let executor = Executor::new();

    let mut mv_store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
    let mut cdc_feed = CdcFeed::new();

    // 源表 INSERT 1 行 → CDC 捕获（投影后只有 id 列）
    cdc_feed.push_insert("users", vec![Value::Int64(1)]);

    let view_name = TableName::new("mv");
    let source_table = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            1_000_000,
        )
        .unwrap();

    assert_eq!(outcome.rows_appended, 1);
    assert_eq!(outcome.total_rows, 1);
    assert_eq!(outcome.mode, RefreshMode::InsertOnly);
    assert_eq!(mv_store.row_count(), 1);
    assert_eq!(mv_store.rows()[0][0], Value::Int64(1));
    assert!(cdc_feed.is_empty());
}

#[test]
fn incremental_refresh_updates_refresh_state() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let executor = Executor::new();

    let mut mv_store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
    let mut cdc_feed = CdcFeed::new();
    cdc_feed.push_insert("users", vec![Value::Int64(42)]);

    let view_name = TableName::new("mv");
    let source_table = TableName::new("users");
    let _outcome = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            1_700_000_000,
        )
        .unwrap();

    assert!(mv_store.refresh_state.initialized);
    assert_eq!(mv_store.refresh_state.last_row_count, 1);
    assert_eq!(mv_store.refresh_state.last_refresh_timestamp, 1_700_000_000);
    assert_eq!(mv_store.refresh_state.mode, RefreshMode::InsertOnly);
}

// =====================================================================
//  CDC 批量测试
// =====================================================================

#[test]
fn incremental_refresh_batch_1000_inserts() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let executor = Executor::new();

    let mut mv_store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
    let mut cdc_feed = CdcFeed::new();

    // 批量推送 1000 行
    let rows: Vec<Vec<Value>> = (1..=1000).map(|i| vec![Value::Int64(i)]).collect();
    cdc_feed.push_inserts("users", rows);

    let view_name = TableName::new("mv");
    let source_table = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            2_000_000,
        )
        .unwrap();

    assert_eq!(outcome.rows_appended, 1000);
    assert_eq!(outcome.total_rows, 1000);
    assert_eq!(mv_store.row_count(), 1000);
    // 验证第 1 行与第 1000 行
    assert_eq!(mv_store.rows()[0][0], Value::Int64(1));
    assert_eq!(mv_store.rows()[999][0], Value::Int64(1000));
}

// =====================================================================
//  多次刷新测试
// =====================================================================

#[test]
fn incremental_refresh_multiple_rounds_accumulate() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let executor = Executor::new();

    let mut mv_store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
    let view_name = TableName::new("mv");
    let source_table = TableName::new("users");

    // 第 1 轮：INSERT 3 行 → refresh
    let mut cdc_feed = CdcFeed::new();
    cdc_feed.push_inserts(
        "users",
        vec![
            vec![Value::Int64(1)],
            vec![Value::Int64(2)],
            vec![Value::Int64(3)],
        ],
    );
    let outcome1 = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            1_000,
        )
        .unwrap();
    assert_eq!(outcome1.rows_appended, 3);
    assert_eq!(outcome1.total_rows, 3);

    // 第 2 轮：INSERT 2 行 → refresh
    cdc_feed.push_inserts("users", vec![vec![Value::Int64(4)], vec![Value::Int64(5)]]);
    let outcome2 = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            2_000,
        )
        .unwrap();
    assert_eq!(outcome2.rows_appended, 2);
    assert_eq!(outcome2.total_rows, 5);

    // 第 3 轮：INSERT 0 行 → refresh（no-op）
    let outcome3 = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            3_000,
        )
        .unwrap();
    assert_eq!(outcome3.rows_appended, 0);
    assert_eq!(outcome3.total_rows, 5);

    // 验证累积结果
    assert_eq!(mv_store.row_count(), 5);
    for (i, row) in mv_store.rows().iter().enumerate() {
        assert_eq!(row[0], Value::Int64((i + 1) as i64));
    }
}

// =====================================================================
//  空刷新测试
// =====================================================================

#[test]
fn incremental_refresh_empty_feed_is_noop() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let executor = Executor::new();

    let mut mv_store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
    let mut cdc_feed = CdcFeed::new();
    // 不推送任何事件

    let view_name = TableName::new("mv");
    let source_table = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            5_000,
        )
        .unwrap();

    assert_eq!(outcome.rows_appended, 0);
    assert_eq!(outcome.total_rows, 0);
    assert_eq!(mv_store.row_count(), 0);
    // refresh_state 仍被更新（initialized=true, last_row_count=0）
    assert!(mv_store.refresh_state.initialized);
}

// =====================================================================
//  高水位测试
// =====================================================================

#[test]
fn incremental_refresh_advances_high_water_mark() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let executor = Executor::new();

    let mut mv_store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
    let view_name = TableName::new("mv");
    let source_table = TableName::new("users");

    // 初始高水位 == 0
    assert_eq!(mv_store.high_water_mark(&source_table), 0);

    // 第 1 轮：INSERT 10 行
    let mut cdc_feed = CdcFeed::new();
    let rows: Vec<Vec<Value>> = (1..=10).map(|i| vec![Value::Int64(i)]).collect();
    cdc_feed.push_inserts("users", rows);
    executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            1_000,
        )
        .unwrap();
    assert_eq!(mv_store.high_water_mark(&source_table), 10);

    // 第 2 轮：INSERT 20 行
    let rows: Vec<Vec<Value>> = (11..=30).map(|i| vec![Value::Int64(i)]).collect();
    cdc_feed.push_inserts("users", rows);
    executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            2_000,
        )
        .unwrap();
    assert_eq!(mv_store.high_water_mark(&source_table), 30);
    assert_eq!(mv_store.row_count(), 30);
}

// =====================================================================
//  错误场景测试
// =====================================================================

#[test]
fn incremental_refresh_nonexistent_view_errors() {
    let catalog = make_catalog_with_users();
    let executor = Executor::new();

    let mut mv_store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
    let mut cdc_feed = CdcFeed::new();
    cdc_feed.push_insert("users", vec![Value::Int64(1)]);

    let view_name = TableName::new("nonexistent");
    let source_table = TableName::new("users");
    let result = executor.refresh_materialized_view_incremental(
        &view_name,
        &catalog,
        &mut mv_store,
        &mut cdc_feed,
        &source_table,
        1_000,
    );

    assert!(result.is_err());
    let err = result.unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("does not exist"),
        "expected 'does not exist' in: {msg}"
    );
}

#[test]
fn incremental_refresh_non_materialized_view_errors() {
    let mut catalog = make_catalog_with_users();
    // 创建普通视图（非物化）
    let plan = plan_sql("CREATE VIEW v1 AS SELECT id FROM users", &catalog);
    let executor = Executor::new();
    executor.execute_create_view(&plan, &mut catalog).unwrap();

    let mut mv_store = MaterializedViewStore::new("v1", vec![("id", ColumnType::Int64)]);
    let mut cdc_feed = CdcFeed::new();
    cdc_feed.push_insert("users", vec![Value::Int64(1)]);

    let view_name = TableName::new("v1");
    let source_table = TableName::new("users");
    let result = executor.refresh_materialized_view_incremental(
        &view_name,
        &catalog,
        &mut mv_store,
        &mut cdc_feed,
        &source_table,
        1_000,
    );

    assert!(result.is_err());
    let err = result.unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("not a materialized view"),
        "expected 'not a materialized view' in: {msg}"
    );
}

// =====================================================================
//  增量 vs 全量等价测试
// =====================================================================

#[test]
fn incremental_equals_full_refresh_result() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let executor = Executor::new();

    // 准备源表数据：100 行
    let source_rows: Vec<Vec<Value>> = (1..=100)
        .map(|i| vec![Value::Int64(i), Value::Text(format!("user{i}"))])
        .collect();

    // === 增量刷新路径 ===
    let mut mv_incremental = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
    let mut cdc_feed = CdcFeed::new();
    // 投影：源表 (id, name) → 物化视图 (id)
    for row in &source_rows {
        cdc_feed.push_insert("users", vec![row[0].clone()]);
    }
    let view_name = TableName::new("mv");
    let source_table = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_incremental,
            &mut cdc_feed,
            &source_table,
            1_000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 100);

    // === 全量刷新路径 ===
    // 模拟全量刷新：直接 append 所有投影行
    let mut mv_full = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
    let projected: Vec<Vec<Value>> = source_rows.iter().map(|r| vec![r[0].clone()]).collect();
    mv_full.append_rows(projected);

    // 验证两者一致
    assert_eq!(mv_incremental.row_count(), mv_full.row_count());
    for (i, (inc_row, full_row)) in mv_incremental
        .rows()
        .iter()
        .zip(mv_full.rows().iter())
        .enumerate()
    {
        assert_eq!(inc_row, full_row, "row {i} mismatch");
    }
}

// =====================================================================
//  RefreshOutcome 与 CDC 事件类型测试
// =====================================================================

#[test]
fn refresh_outcome_insert_only_construction() {
    let outcome = RefreshOutcome::insert_only(42, 100);
    assert_eq!(outcome.rows_appended, 42);
    assert_eq!(outcome.rows_removed, 0);
    assert_eq!(outcome.mode, RefreshMode::InsertOnly);
    assert_eq!(outcome.total_rows, 100);
}

#[test]
fn cdc_event_insert_kind_str() {
    let event = CdcEvent::insert("users", vec![Value::Int64(1)]);
    assert_eq!(event.kind_str(), "INSERT");
}

#[test]
fn cdc_feed_drain_clears_buffer() {
    let mut feed = CdcFeed::new();
    feed.push_insert("t", vec![Value::Int64(1)]);
    feed.push_insert("t", vec![Value::Int64(2)]);
    feed.push_insert("t", vec![Value::Int64(3)]);
    let events = feed.drain();
    assert_eq!(events.len(), 3);
    assert!(feed.is_empty());
    // 再次 drain 返回空
    let events2 = feed.drain();
    assert!(events2.is_empty());
}

// =====================================================================
//  Stress 测试（10 万行）
// =====================================================================

#[test]
fn incremental_refresh_stress_100k_rows() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let executor = Executor::new();

    let mut mv_store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
    let mut cdc_feed = CdcFeed::new();

    // 批量推送 100K 行
    let rows: Vec<Vec<Value>> = (1..=100_000).map(|i| vec![Value::Int64(i)]).collect();
    cdc_feed.push_inserts("users", rows);

    let view_name = TableName::new("mv");
    let source_table = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            9_999_999,
        )
        .unwrap();

    assert_eq!(outcome.rows_appended, 100_000);
    assert_eq!(outcome.total_rows, 100_000);
    assert_eq!(mv_store.row_count(), 100_000);
    // 抽样验证
    assert_eq!(mv_store.rows()[0][0], Value::Int64(1));
    assert_eq!(mv_store.rows()[50_000][0], Value::Int64(50_001));
    assert_eq!(mv_store.rows()[99_999][0], Value::Int64(100_000));
    // 高水位正确推进
    assert_eq!(mv_store.high_water_mark(&source_table), 100_000);
}

// =====================================================================
//  E2E 测试：CREATE MV + INSERT + CDC + 增量刷新 + 验证
// =====================================================================

#[test]
fn e2e_create_mv_insert_cdc_refresh_verify() {
    // 1. 创建 catalog + users 表
    let mut catalog = make_catalog_with_users();
    let executor = Executor::new();

    // 2. CREATE MATERIALIZED VIEW mv AS SELECT id FROM users
    let create_mv_plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv AS SELECT id FROM users",
        &catalog,
    );
    executor
        .execute_create_view(&create_mv_plan, &mut catalog)
        .unwrap();

    // 3. 模拟源表 INSERT 3 行（直接构造 CDC 事件，跳过执行器 INSERT）
    let mut cdc_feed = CdcFeed::new();
    let mut mv_store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);

    // 第 1 行
    cdc_feed.push_insert("users", vec![Value::Int64(1)]);
    // 第 2、3 行
    cdc_feed.push_inserts("users", vec![vec![Value::Int64(2)], vec![Value::Int64(3)]]);

    // 4. 增量刷新
    let view_name = TableName::new("mv");
    let source_table = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            1_700_000_000,
        )
        .unwrap();

    // 5. 验证
    assert_eq!(outcome.rows_appended, 3);
    assert_eq!(mv_store.row_count(), 3);
    let ids: Vec<i64> = mv_store
        .rows()
        .iter()
        .map(|r| match &r[0] {
            Value::Int64(n) => *n,
            _ => panic!("expected Int64"),
        })
        .collect();
    assert_eq!(ids, vec![1, 2, 3]);

    // 6. 再次 INSERT 2 行 + 刷新
    cdc_feed.push_inserts("users", vec![vec![Value::Int64(4)], vec![Value::Int64(5)]]);
    let outcome2 = executor
        .refresh_materialized_view_incremental(
            &view_name,
            &catalog,
            &mut mv_store,
            &mut cdc_feed,
            &source_table,
            1_700_000_001,
        )
        .unwrap();
    assert_eq!(outcome2.rows_appended, 2);
    assert_eq!(outcome2.total_rows, 5);

    // 7. 最终验证
    let ids2: Vec<i64> = mv_store
        .rows()
        .iter()
        .map(|r| match &r[0] {
            Value::Int64(n) => *n,
            _ => panic!("expected Int64"),
        })
        .collect();
    assert_eq!(ids2, vec![1, 2, 3, 4, 5]);

    // 8. catalog 中视图定义仍存在
    assert!(catalog.view_exists(&view_name));
    let view_def = catalog.get_view(&view_name).unwrap();
    assert!(view_def.materialized);
}

// =====================================================================
//  从 Statement 解析验证 CDC 兼容性
// =====================================================================

#[test]
fn cdc_feed_compatible_with_insert_statement_parsing() {
    // 验证：INSERT 语句能解析，CDC 事件能正确表示
    let stmt = parse_one("INSERT INTO users VALUES (1, 'Alice')").unwrap();
    match stmt {
        Statement::Insert { .. } => {
            // 模拟 CDC 捕获：投影后只有 id 列
            let mut feed = CdcFeed::new();
            feed.push_insert("users", vec![Value::Int64(1)]);
            assert_eq!(feed.len(), 1);
        }
        other => panic!("expected Insert, got {other:?}"),
    }
}
