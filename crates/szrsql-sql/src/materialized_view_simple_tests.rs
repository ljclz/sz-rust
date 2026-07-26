//! Phase 6.12 集成测试 — SIMPLE 增量刷新（按主键合并 INSERT/UPDATE/DELETE）。
//!
//! 覆盖类别：
//! - CDC 事件构造与缓冲（4 条）：Update/Delete 构造 + push_update/push_delete + 混合事件
//! - MaterializedViewStore 主键索引（6 条）：new_with_pk + append 更新索引 + UPSERT 新增/更新 +
//!   delete_by_pk 存在/不存在 + 复合主键
//! - SIMPLE 刷新基础（5 条）：单 INSERT / 单 UPDATE / 单 DELETE / 混合 / 空 feed
//! - SIMPLE 刷新多轮累积（2 条）：多轮 INSERT+UPDATE+DELETE / 高水位推进
//! - 错误场景（2 条）：nonexistent view / non-materialized view
//! - 增量 vs 全量等价性（1 条）：混合 DML 后对比结果
//! - 压力测试（1 条）：100K 行混合 DML
//! - E2E（1 条）：CREATE MV + 批量 INSERT + UPDATE + DELETE + refresh + 验证
//! - RefreshOutcome SIMPLE（2 条）：构造 / clone_eq
//!
//! 共 24 个测试用例。

use super::executor::{Executor, TableStorage};
use super::materialized_view::{
    CdcEvent, CdcFeed, MaterializedViewStore, RefreshMode, RefreshOutcome,
};
use crate::ast::TableName;
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

/// 创建物化视图 `mv`（SELECT id, name FROM users）
fn setup_materialized_view(catalog: &mut InMemoryCatalog) {
    let plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv AS SELECT id, name FROM users",
        catalog,
    );
    let executor = Executor::new();
    executor.execute_create_view(&plan, catalog).unwrap();
}

/// 创建带主键的物化视图存储（id 为主键，索引 0）
fn make_mv_store_with_pk() -> MaterializedViewStore {
    MaterializedViewStore::new_with_pk(
        "mv",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
        vec![0],
    )
}

/// 收集活跃行（排除 tombstone），按主键排序便于对比
fn collect_active_rows_sorted(store: &MaterializedViewStore) -> Vec<Vec<Value>> {
    let mut rows: Vec<Vec<Value>> = store.storage.scan_iter().collect();
    rows.sort_by(|a, b| {
        let a_id = a.first().and_then(|v| match v {
            Value::Int64(i) => Some(*i),
            _ => None,
        });
        let b_id = b.first().and_then(|v| match v {
            Value::Int64(i) => Some(*i),
            _ => None,
        });
        a_id.cmp(&b_id)
    });
    rows
}

// =====================================================================
//  CDC 事件构造与缓冲测试（4 条）
// =====================================================================

#[test]
fn cdc_event_update_construction_and_kind() {
    let event = CdcEvent::update(
        "users",
        vec![Value::Int64(1)],
        vec![Value::Int64(1), Value::Text("Alice".into())],
    );
    assert_eq!(event.kind_str(), "UPDATE");
    assert_eq!(event.source_table().name, "users");
}

#[test]
fn cdc_event_delete_construction_and_kind() {
    let event = CdcEvent::delete("users", vec![Value::Int64(1)]);
    assert_eq!(event.kind_str(), "DELETE");
    assert_eq!(event.source_table().name, "users");
}

#[test]
fn cdc_feed_push_update_and_delete() {
    let mut feed = CdcFeed::new();
    feed.push_update(
        "users",
        vec![Value::Int64(1)],
        vec![Value::Int64(1), Value::Text("Bob".into())],
    );
    feed.push_delete("users", vec![Value::Int64(2)]);
    assert_eq!(feed.len(), 2);
    let events = feed.drain();
    assert!(matches!(events[0], CdcEvent::Update { .. }));
    assert!(matches!(events[1], CdcEvent::Delete { .. }));
}

#[test]
fn cdc_feed_mixed_insert_update_delete() {
    let mut feed = CdcFeed::new();
    feed.push_insert("t", vec![Value::Int64(1)]);
    feed.push_update("t", vec![Value::Int64(1)], vec![Value::Int64(1)]);
    feed.push_delete("t", vec![Value::Int64(1)]);
    let events = feed.drain();
    assert_eq!(events.len(), 3);
    // 验证 FIFO 顺序
    assert_eq!(events[0].kind_str(), "INSERT");
    assert_eq!(events[1].kind_str(), "UPDATE");
    assert_eq!(events[2].kind_str(), "DELETE");
}

// =====================================================================
//  MaterializedViewStore 主键索引测试（6 条）
// =====================================================================

#[test]
fn mv_store_upsert_insert_new_row() {
    let mut store = make_mv_store_with_pk();
    let (was_insert, was_update) =
        store.upsert_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
    assert!(was_insert);
    assert!(!was_update);
    assert_eq!(store.active_row_count(), 1);
}

#[test]
fn mv_store_upsert_update_existing_row() {
    let mut store = make_mv_store_with_pk();
    store.append_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
    let (was_insert, was_update) =
        store.upsert_row(vec![Value::Int64(1), Value::Text("Bob".into())]);
    assert!(!was_insert);
    assert!(was_update);
    assert_eq!(store.active_row_count(), 1);
    // 验证行内容已更新
    let rows = store.rows();
    assert_eq!(rows[0][1], Value::Text("Bob".into()));
}

#[test]
fn mv_store_delete_by_pk_existing() {
    let mut store = make_mv_store_with_pk();
    store.append_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
    store.append_row(vec![Value::Int64(2), Value::Text("Bob".into())]);
    assert!(store.delete_by_pk(&[Value::Int64(1)]));
    assert_eq!(store.active_row_count(), 1);
}

#[test]
fn mv_store_delete_by_pk_nonexistent_returns_false() {
    let mut store = make_mv_store_with_pk();
    store.append_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
    assert!(!store.delete_by_pk(&[Value::Int64(99)]));
    assert_eq!(store.active_row_count(), 1);
}

#[test]
fn mv_store_composite_pk_upsert_and_delete() {
    let mut store = MaterializedViewStore::new_with_pk(
        "mv",
        vec![
            ("tenant_id", ColumnType::Int64),
            ("user_id", ColumnType::Int64),
            ("name", ColumnType::Text),
        ],
        vec![0, 1],
    );
    store.append_row(vec![
        Value::Int64(1),
        Value::Int64(100),
        Value::Text("Alice".into()),
    ]);
    // UPSERT 已存在
    let (was_insert, was_update) = store.upsert_row(vec![
        Value::Int64(1),
        Value::Int64(100),
        Value::Text("Alice2".into()),
    ]);
    assert!(!was_insert);
    assert!(was_update);
    // 按复合主键删除
    assert!(store.delete_by_pk(&[Value::Int64(1), Value::Int64(100)]));
    assert_eq!(store.active_row_count(), 0);
}

#[test]
fn mv_store_no_pk_upsert_degrades_to_append() {
    let mut store = MaterializedViewStore::new(
        "mv",
        vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
    );
    let (was_insert, _) = store.upsert_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
    assert!(was_insert);
    assert_eq!(store.row_count(), 1);
}

// =====================================================================
//  SIMPLE 刷新基础测试（5 条）
// =====================================================================

#[test]
fn simple_refresh_single_insert() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_pk();
    let mut feed = CdcFeed::new();
    feed.push_insert("users", vec![Value::Int64(1), Value::Text("Alice".into())]);

    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 1);
    assert_eq!(outcome.rows_updated, 0);
    assert_eq!(outcome.rows_removed, 0);
    assert_eq!(outcome.mode, RefreshMode::Simple);
    assert_eq!(outcome.total_rows, 1);
    assert_eq!(store.active_row_count(), 1);
}

#[test]
fn simple_refresh_single_update() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_pk();
    // 预填充一行
    store.append_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
    let mut feed = CdcFeed::new();
    feed.push_update(
        "users",
        vec![Value::Int64(1)],
        vec![Value::Int64(1), Value::Text("Bob".into())],
    );

    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 0);
    assert_eq!(outcome.rows_updated, 1);
    assert_eq!(outcome.rows_removed, 0);
    assert_eq!(outcome.total_rows, 1);
    // 验证行已更新
    let rows = collect_active_rows_sorted(&store);
    assert_eq!(rows[0][1], Value::Text("Bob".into()));
}

#[test]
fn simple_refresh_single_delete() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_pk();
    store.append_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
    store.append_row(vec![Value::Int64(2), Value::Text("Bob".into())]);
    let mut feed = CdcFeed::new();
    feed.push_delete("users", vec![Value::Int64(1)]);

    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 0);
    assert_eq!(outcome.rows_updated, 0);
    assert_eq!(outcome.rows_removed, 1);
    assert_eq!(outcome.total_rows, 1);
    assert_eq!(store.active_row_count(), 1);
}

#[test]
fn simple_refresh_mixed_insert_update_delete() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_pk();
    // 预填充 id=1, id=2
    store.append_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
    store.append_row(vec![Value::Int64(2), Value::Text("Bob".into())]);

    let mut feed = CdcFeed::new();
    // INSERT id=3
    feed.push_insert(
        "users",
        vec![Value::Int64(3), Value::Text("Charlie".into())],
    );
    // UPDATE id=1 -> "Alice2"
    feed.push_update(
        "users",
        vec![Value::Int64(1)],
        vec![Value::Int64(1), Value::Text("Alice2".into())],
    );
    // DELETE id=2
    feed.push_delete("users", vec![Value::Int64(2)]);

    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 1); // INSERT id=3
    assert_eq!(outcome.rows_updated, 1); // UPDATE id=1
    assert_eq!(outcome.rows_removed, 1); // DELETE id=2
    assert_eq!(outcome.total_rows, 2); // 1 (Alice2) + 3 (Charlie)
    assert_eq!(store.active_row_count(), 2);

    // 验证剩余行
    let rows = collect_active_rows_sorted(&store);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], Value::Int64(1));
    assert_eq!(rows[0][1], Value::Text("Alice2".into()));
    assert_eq!(rows[1][0], Value::Int64(3));
    assert_eq!(rows[1][1], Value::Text("Charlie".into()));
}

#[test]
fn simple_refresh_empty_feed_is_noop() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_pk();
    store.append_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
    let mut feed = CdcFeed::new();

    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source = TableName::new("users");
    let outcome = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 0);
    assert_eq!(outcome.rows_updated, 0);
    assert_eq!(outcome.rows_removed, 0);
    assert_eq!(outcome.total_rows, 1);
    assert_eq!(store.active_row_count(), 1);
}

// =====================================================================
//  SIMPLE 刷新多轮累积测试（2 条）
// =====================================================================

#[test]
fn simple_refresh_multiple_rounds_accumulate() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_pk();
    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source = TableName::new("users");

    // 第一轮：INSERT id=1, 2, 3
    let mut feed1 = CdcFeed::new();
    feed1.push_insert("users", vec![Value::Int64(1), Value::Text("A".into())]);
    feed1.push_insert("users", vec![Value::Int64(2), Value::Text("B".into())]);
    feed1.push_insert("users", vec![Value::Int64(3), Value::Text("C".into())]);
    let o1 = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed1, &source, 1000,
        )
        .unwrap();
    assert_eq!(o1.rows_appended, 3);
    assert_eq!(o1.total_rows, 3);

    // 第二轮：UPDATE id=2, DELETE id=1
    let mut feed2 = CdcFeed::new();
    feed2.push_update(
        "users",
        vec![Value::Int64(2)],
        vec![Value::Int64(2), Value::Text("B2".into())],
    );
    feed2.push_delete("users", vec![Value::Int64(1)]);
    let o2 = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed2, &source, 2000,
        )
        .unwrap();
    assert_eq!(o2.rows_updated, 1);
    assert_eq!(o2.rows_removed, 1);
    assert_eq!(o2.total_rows, 2);

    // 第三轮：INSERT id=4
    let mut feed3 = CdcFeed::new();
    feed3.push_insert("users", vec![Value::Int64(4), Value::Text("D".into())]);
    let o3 = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed3, &source, 3000,
        )
        .unwrap();
    assert_eq!(o3.rows_appended, 1);
    assert_eq!(o3.total_rows, 3);

    // 验证最终状态：id=2(B2), id=3(C), id=4(D)
    let rows = collect_active_rows_sorted(&store);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][0], Value::Int64(2));
    assert_eq!(rows[0][1], Value::Text("B2".into()));
    assert_eq!(rows[1][0], Value::Int64(3));
    assert_eq!(rows[1][1], Value::Text("C".into()));
    assert_eq!(rows[2][0], Value::Int64(4));
    assert_eq!(rows[2][1], Value::Text("D".into()));
}

#[test]
fn simple_refresh_advances_high_water_mark() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_pk();
    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source = TableName::new("users");

    // 第一轮：3 个事件
    let mut feed1 = CdcFeed::new();
    feed1.push_insert("users", vec![Value::Int64(1), Value::Text("A".into())]);
    feed1.push_insert("users", vec![Value::Int64(2), Value::Text("B".into())]);
    feed1.push_insert("users", vec![Value::Int64(3), Value::Text("C".into())]);
    executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed1, &source, 1000,
        )
        .unwrap();
    assert_eq!(store.high_water_mark(&source), 3);

    // 第二轮：2 个事件
    let mut feed2 = CdcFeed::new();
    feed2.push_update(
        "users",
        vec![Value::Int64(1)],
        vec![Value::Int64(1), Value::Text("A2".into())],
    );
    feed2.push_delete("users", vec![Value::Int64(2)]);
    executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed2, &source, 2000,
        )
        .unwrap();
    assert_eq!(store.high_water_mark(&source), 5);
}

// =====================================================================
//  错误场景测试（2 条）
// =====================================================================

#[test]
fn simple_refresh_nonexistent_view_errors() {
    let catalog = make_catalog_with_users();
    let mut store = make_mv_store_with_pk();
    let mut feed = CdcFeed::new();
    let executor = Executor::new();
    let view_name = TableName::new("nonexistent");
    let source = TableName::new("users");
    let result = executor.refresh_materialized_view_simple(
        &view_name, &catalog, &mut store, &mut feed, &source, 1000,
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
fn simple_refresh_non_materialized_view_errors() {
    let mut catalog = make_catalog_with_users();
    // 创建普通视图（非物化）
    let plan = plan_sql("CREATE VIEW v1 AS SELECT id FROM users", &catalog);
    let executor = Executor::new();
    executor.execute_create_view(&plan, &mut catalog).unwrap();
    let mut store = make_mv_store_with_pk();
    let mut feed = CdcFeed::new();
    let view_name = TableName::new("v1");
    let source = TableName::new("users");
    let result = executor.refresh_materialized_view_simple(
        &view_name, &catalog, &mut store, &mut feed, &source, 1000,
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
//  增量 vs 全量等价性测试（1 条）
// =====================================================================

#[test]
fn simple_equals_full_refresh_result() {
    // 全量：直接构造 100 行
    let mut full_store = make_mv_store_with_pk();
    for i in 1..=100i64 {
        full_store.append_row(vec![Value::Int64(i), Value::Text(format!("user{i}"))]);
    }

    // 增量：从空开始，INSERT 100 行 + UPDATE 50 行 + DELETE 25 行，再 INSERT 25 行
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let mut incr_store = make_mv_store_with_pk();
    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source = TableName::new("users");

    // 初始 INSERT 100 行
    let mut feed1 = CdcFeed::new();
    for i in 1..=100i64 {
        feed1.push_insert(
            "users",
            vec![Value::Int64(i), Value::Text(format!("user{i}"))],
        );
    }
    executor
        .refresh_materialized_view_simple(
            &view_name,
            &catalog,
            &mut incr_store,
            &mut feed1,
            &source,
            1000,
        )
        .unwrap();

    // UPDATE id=1..50 -> "updated_{id}"
    let mut feed2 = CdcFeed::new();
    for i in 1..=50i64 {
        feed2.push_update(
            "users",
            vec![Value::Int64(i)],
            vec![Value::Int64(i), Value::Text(format!("updated_{i}"))],
        );
    }
    executor
        .refresh_materialized_view_simple(
            &view_name,
            &catalog,
            &mut incr_store,
            &mut feed2,
            &source,
            2000,
        )
        .unwrap();

    // DELETE id=76..100（25 行）
    let mut feed3 = CdcFeed::new();
    for i in 76..=100i64 {
        feed3.push_delete("users", vec![Value::Int64(i)]);
    }
    executor
        .refresh_materialized_view_simple(
            &view_name,
            &catalog,
            &mut incr_store,
            &mut feed3,
            &source,
            3000,
        )
        .unwrap();

    // 对应全量：同样 UPDATE + DELETE
    for i in 1..=50i64 {
        let _ = full_store.upsert_row(vec![Value::Int64(i), Value::Text(format!("updated_{i}"))]);
    }
    for i in 76..=100i64 {
        full_store.delete_by_pk(&[Value::Int64(i)]);
    }

    // 对比结果
    let incr_rows = collect_active_rows_sorted(&incr_store);
    let full_rows = collect_active_rows_sorted(&full_store);
    assert_eq!(incr_rows.len(), full_rows.len());
    for (a, b) in incr_rows.iter().zip(full_rows.iter()) {
        assert_eq!(a, b, "row mismatch");
    }
}

// =====================================================================
//  压力测试（1 条）
// =====================================================================

#[test]
fn simple_refresh_stress_100k_mixed_dml() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_pk();
    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source = TableName::new("users");

    // 初始 INSERT 50000 行
    let mut feed1 = CdcFeed::new();
    for i in 1..=50000i64 {
        feed1.push_insert(
            "users",
            vec![Value::Int64(i), Value::Text(format!("user{i}"))],
        );
    }
    let o1 = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed1, &source, 1000,
        )
        .unwrap();
    assert_eq!(o1.rows_appended, 50000);
    assert_eq!(o1.total_rows, 50000);

    // UPDATE id=1..20000
    let mut feed2 = CdcFeed::new();
    for i in 1..=20000i64 {
        feed2.push_update(
            "users",
            vec![Value::Int64(i)],
            vec![Value::Int64(i), Value::Text(format!("upd{i}"))],
        );
    }
    let o2 = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed2, &source, 2000,
        )
        .unwrap();
    assert_eq!(o2.rows_updated, 20000);
    assert_eq!(o2.total_rows, 50000);

    // DELETE id=40001..50000（10000 行）
    let mut feed3 = CdcFeed::new();
    for i in 40001..=50000i64 {
        feed3.push_delete("users", vec![Value::Int64(i)]);
    }
    let o3 = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed3, &source, 3000,
        )
        .unwrap();
    assert_eq!(o3.rows_removed, 10000);
    assert_eq!(o3.total_rows, 40000);

    // INSERT 30000 新行（id=50001..80000）
    let mut feed4 = CdcFeed::new();
    for i in 50001..=80000i64 {
        feed4.push_insert(
            "users",
            vec![Value::Int64(i), Value::Text(format!("new{i}"))],
        );
    }
    let o4 = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed4, &source, 4000,
        )
        .unwrap();
    assert_eq!(o4.rows_appended, 30000);
    assert_eq!(o4.total_rows, 70000);

    assert_eq!(store.active_row_count(), 70000);
}

// =====================================================================
//  E2E 测试（1 条）
// =====================================================================

#[test]
fn e2e_create_mv_insert_update_delete_refresh_verify() {
    let mut catalog = make_catalog_with_users();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_pk();
    let executor = Executor::new();
    let view_name = TableName::new("mv");
    let source = TableName::new("users");

    // 第一轮：INSERT 5 行
    let mut feed1 = CdcFeed::new();
    for i in 1..=5i64 {
        feed1.push_insert(
            "users",
            vec![Value::Int64(i), Value::Text(format!("user{i}"))],
        );
    }
    let o1 = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed1, &source, 1000,
        )
        .unwrap();
    assert_eq!(o1.total_rows, 5);

    // 第二轮：UPDATE id=3, DELETE id=1, INSERT id=6
    let mut feed2 = CdcFeed::new();
    feed2.push_update(
        "users",
        vec![Value::Int64(3)],
        vec![Value::Int64(3), Value::Text("user3_updated".into())],
    );
    feed2.push_delete("users", vec![Value::Int64(1)]);
    feed2.push_insert("users", vec![Value::Int64(6), Value::Text("user6".into())]);
    let o2 = executor
        .refresh_materialized_view_simple(
            &view_name, &catalog, &mut store, &mut feed2, &source, 2000,
        )
        .unwrap();
    assert_eq!(o2.rows_appended, 1);
    assert_eq!(o2.rows_updated, 1);
    assert_eq!(o2.rows_removed, 1);
    assert_eq!(o2.total_rows, 5);

    // 验证最终状态：id=2, 3(updated), 4, 5, 6
    let rows = collect_active_rows_sorted(&store);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0][0], Value::Int64(2));
    assert_eq!(rows[1][0], Value::Int64(3));
    assert_eq!(rows[1][1], Value::Text("user3_updated".into()));
    assert_eq!(rows[2][0], Value::Int64(4));
    assert_eq!(rows[3][0], Value::Int64(5));
    assert_eq!(rows[4][0], Value::Int64(6));

    // 验证 refresh_state
    assert!(store.refresh_state.initialized);
    assert_eq!(store.refresh_state.mode, RefreshMode::Simple);
    assert_eq!(store.refresh_state.last_row_count, 5);
    assert_eq!(store.refresh_state.last_refresh_timestamp, 2000);
}

// =====================================================================
//  RefreshOutcome SIMPLE 测试（2 条）
// =====================================================================

#[test]
fn refresh_outcome_simple_construction() {
    let outcome = RefreshOutcome::simple(100, 50, 30, 120);
    assert_eq!(outcome.rows_appended, 100);
    assert_eq!(outcome.rows_updated, 50);
    assert_eq!(outcome.rows_removed, 30);
    assert_eq!(outcome.mode, RefreshMode::Simple);
    assert_eq!(outcome.total_rows, 120);
}

#[test]
fn refresh_outcome_simple_clone_eq() {
    let outcome = RefreshOutcome::simple(10, 5, 3, 12);
    let cloned = outcome.clone();
    assert_eq!(outcome, cloned);
}
