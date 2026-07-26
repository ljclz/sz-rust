//! Phase 6.13 集成测试 — AGGREGATE 增量刷新（SUM/COUNT/AVG/MIN/MAX 递增/递减）。
//!
//! 覆盖类别：
//! - AggregateFunction / AggregateSpec 单元（3 条）：Display/supports_decrement + Spec 构造
//! - AggregateState 单元（5 条）：SUM/COUNT/AVG/MIN/MAX 的 INSERT 递增 + DELETE 递减
//! - MaterializedViewStore 聚合（5 条）：new_with_aggregates + apply_aggregate_insert +
//!   apply_aggregate_delete + clear 重置 + has_aggregates
//! - CDC DeleteWithRow 构造（2 条）：delete_with_row 构造 + push_delete_with_row
//! - AGGREGATE 刷新基础（5 条）：单 INSERT / 单 DELETE / 混合 / 空 feed / Update 退化为 INSERT
//! - AGGREGATE 刷新多轮累积（1 条）：3 轮 INSERT+DELETE
//! - 错误场景（3 条）：nonexistent view / non-materialized view / 无聚合规格
//! - 增量 vs 全量等价性（1 条）：100 行 INSERT + 50 DELETE，对比全量重算
//! - 压力测试（1 条）：100K 行 INSERT + 50K DELETE
//! - RefreshOutcome AGGREGATE（2 条）：构造 / clone_eq
//!
//! 共 28 个测试用例。

use super::executor::{Executor, TableStorage};
use super::materialized_view::{
    AggregateFunction, AggregateSpec, AggregateState, CdcEvent, CdcFeed, MaterializedViewStore,
    RefreshMode, RefreshOutcome,
};
use crate::ast::TableName;
use crate::parser::parse_one;
use crate::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建带 `orders` 表的 catalog（id INT PK, amount FLOAT8, status TEXT）
fn make_catalog_with_orders() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    let plan = plan_sql(
        "CREATE TABLE orders (id INT PRIMARY KEY, amount DOUBLE PRECISION, status TEXT)",
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

/// 创建物化视图 `mv_agg`（SELECT id, amount, status FROM orders）
fn setup_materialized_view(catalog: &mut InMemoryCatalog) {
    let plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv_agg AS SELECT id, amount, status FROM orders",
        catalog,
    );
    let executor = Executor::new();
    executor.execute_create_view(&plan, catalog).unwrap();
}

/// 创建带 SUM(amount) + COUNT(amount) + AVG(amount) 聚合的物化视图存储
///
/// 列布局：[sum_amount, count_amount, avg_amount]
/// - sum_amount（输出列 0）= SUM(amount)，源列 1（amount）
/// - count_amount（输出列 1）= COUNT(amount)，源列 1（amount）
/// - avg_amount（输出列 2）= AVG(amount)，源列 1（amount）
fn make_mv_store_with_sum_count_avg() -> MaterializedViewStore {
    MaterializedViewStore::new_with_aggregates(
        "mv_agg",
        vec![
            ("sum_amount", ColumnType::Float64),
            ("count_amount", ColumnType::Int64),
            ("avg_amount", ColumnType::Float64),
        ],
        vec![
            AggregateSpec::new(AggregateFunction::Sum, 1, 0),
            AggregateSpec::new(AggregateFunction::Count, 1, 1),
            AggregateSpec::new(AggregateFunction::Avg, 1, 2),
        ],
    )
}

/// 创建带 MIN(amount) + MAX(amount) 聚合的物化视图存储
///
/// 列布局：[min_amount, max_amount]
/// - min_amount（输出列 0）= MIN(amount)，源列 1（amount）
/// - max_amount（输出列 1）= MAX(amount)，源列 1（amount）
fn make_mv_store_with_min_max() -> MaterializedViewStore {
    MaterializedViewStore::new_with_aggregates(
        "mv_agg",
        vec![
            ("min_amount", ColumnType::Float64),
            ("max_amount", ColumnType::Float64),
        ],
        vec![
            AggregateSpec::new(AggregateFunction::Min, 1, 0),
            AggregateSpec::new(AggregateFunction::Max, 1, 1),
        ],
    )
}

/// 构造一行 orders 数据（id, amount, status）
fn make_order_row(id: i64, amount: f64, status: &str) -> Vec<Value> {
    vec![
        Value::Int64(id),
        Value::Float64(amount),
        Value::Text(status.into()),
    ]
}

// =====================================================================
//  AggregateFunction / AggregateSpec 单元测试（3 条）
// =====================================================================

#[test]
fn aggregate_function_display() {
    assert_eq!(format!("{}", AggregateFunction::Sum), "SUM");
    assert_eq!(format!("{}", AggregateFunction::Count), "COUNT");
    assert_eq!(format!("{}", AggregateFunction::Avg), "AVG");
    assert_eq!(format!("{}", AggregateFunction::Min), "MIN");
    assert_eq!(format!("{}", AggregateFunction::Max), "MAX");
}

#[test]
fn aggregate_function_supports_decrement() {
    assert!(AggregateFunction::Sum.supports_decrement());
    assert!(AggregateFunction::Count.supports_decrement());
    assert!(AggregateFunction::Avg.supports_decrement());
    assert!(!AggregateFunction::Min.supports_decrement());
    assert!(!AggregateFunction::Max.supports_decrement());
}

#[test]
fn aggregate_spec_new() {
    let spec = AggregateSpec::new(AggregateFunction::Sum, 2, 3);
    assert_eq!(spec.function, AggregateFunction::Sum);
    assert_eq!(spec.source_column, 2);
    assert_eq!(spec.output_column, 3);
}

// =====================================================================
//  AggregateState 单元测试（5 条）
// =====================================================================

#[test]
fn aggregate_state_sum_insert_and_delete() {
    let mut state = AggregateState::new();
    state.apply_insert(AggregateFunction::Sum, &Value::Int64(10));
    state.apply_insert(AggregateFunction::Sum, &Value::Float64(20.5));
    // sum = 30.5
    assert_eq!(
        state.current_value(AggregateFunction::Sum),
        Value::Float64(30.5)
    );

    state.apply_delete(AggregateFunction::Sum, &Value::Int64(10));
    // sum = 20.5
    assert_eq!(
        state.current_value(AggregateFunction::Sum),
        Value::Float64(20.5)
    );
}

#[test]
fn aggregate_state_count_insert_and_delete() {
    let mut state = AggregateState::new();
    state.apply_insert(AggregateFunction::Count, &Value::Int64(1));
    state.apply_insert(AggregateFunction::Count, &Value::Int64(2));
    state.apply_insert(AggregateFunction::Count, &Value::Int64(3));
    assert_eq!(
        state.current_value(AggregateFunction::Count),
        Value::Int64(3)
    );

    state.apply_delete(AggregateFunction::Count, &Value::Int64(2));
    assert_eq!(
        state.current_value(AggregateFunction::Count),
        Value::Int64(2)
    );
}

#[test]
fn aggregate_state_avg_insert_and_delete() {
    let mut state = AggregateState::new();
    state.apply_insert(AggregateFunction::Avg, &Value::Int64(10));
    state.apply_insert(AggregateFunction::Avg, &Value::Int64(20));
    // avg = (10+20)/2 = 15.0
    assert_eq!(
        state.current_value(AggregateFunction::Avg),
        Value::Float64(15.0)
    );

    state.apply_delete(AggregateFunction::Avg, &Value::Int64(10));
    // avg = 20/1 = 20.0
    assert_eq!(
        state.current_value(AggregateFunction::Avg),
        Value::Float64(20.0)
    );

    state.apply_delete(AggregateFunction::Avg, &Value::Int64(20));
    // count = 0 → NULL
    assert_eq!(state.current_value(AggregateFunction::Avg), Value::Null);
}

#[test]
fn aggregate_state_min_insert_only() {
    let mut state = AggregateState::new();
    state.apply_insert(AggregateFunction::Min, &Value::Int64(30));
    state.apply_insert(AggregateFunction::Min, &Value::Int64(10));
    state.apply_insert(AggregateFunction::Min, &Value::Int64(20));
    assert_eq!(
        state.current_value(AggregateFunction::Min),
        Value::Int64(10)
    );

    // MIN 不支持递减
    let ok = state.apply_delete(AggregateFunction::Min, &Value::Int64(10));
    assert!(!ok);
    // 状态不变
    assert_eq!(
        state.current_value(AggregateFunction::Min),
        Value::Int64(10)
    );
}

#[test]
fn aggregate_state_max_insert_only() {
    let mut state = AggregateState::new();
    state.apply_insert(AggregateFunction::Max, &Value::Int64(10));
    state.apply_insert(AggregateFunction::Max, &Value::Int64(50));
    state.apply_insert(AggregateFunction::Max, &Value::Int64(20));
    assert_eq!(
        state.current_value(AggregateFunction::Max),
        Value::Int64(50)
    );

    // MAX 不支持递减
    let ok = state.apply_delete(AggregateFunction::Max, &Value::Int64(50));
    assert!(!ok);
    assert_eq!(
        state.current_value(AggregateFunction::Max),
        Value::Int64(50)
    );
}

// =====================================================================
//  MaterializedViewStore 聚合测试（5 条）
// =====================================================================

#[test]
fn mv_store_new_with_aggregates_initializes() {
    let store = make_mv_store_with_sum_count_avg();
    assert!(store.has_aggregates());
    assert_eq!(store.aggregate_specs().len(), 3);
    assert_eq!(store.aggregate_states().len(), 3);
    // 聚合行已初始化（1 行，全 NULL）
    assert_eq!(store.active_row_count(), 1);
    assert!(store.aggregate_row_id().is_some());
}

#[test]
fn mv_store_apply_aggregate_insert_updates_row() {
    let mut store = make_mv_store_with_sum_count_avg();
    // INSERT (id=1, amount=10.0, status='paid')
    store.apply_aggregate_insert(&make_order_row(1, 10.0, "paid"));
    // INSERT (id=2, amount=20.0, status='pending')
    store.apply_aggregate_insert(&make_order_row(2, 20.0, "pending"));

    // sum = 30.0, count = 2, avg = 15.0
    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    assert_eq!(row[0], Value::Float64(30.0)); // sum
    assert_eq!(row[1], Value::Int64(2)); // count
    assert_eq!(row[2], Value::Float64(15.0)); // avg
}

#[test]
fn mv_store_apply_aggregate_delete_decrements() {
    let mut store = make_mv_store_with_sum_count_avg();
    store.apply_aggregate_insert(&make_order_row(1, 10.0, "paid"));
    store.apply_aggregate_insert(&make_order_row(2, 20.0, "pending"));

    // DELETE (id=1, amount=10.0, status='paid')
    let ok = store.apply_aggregate_delete(&make_order_row(1, 10.0, "paid"));
    assert!(ok); // SUM/COUNT/AVG 都支持递减

    // sum = 20.0, count = 1, avg = 20.0
    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    assert_eq!(row[0], Value::Float64(20.0));
    assert_eq!(row[1], Value::Int64(1));
    assert_eq!(row[2], Value::Float64(20.0));
}

#[test]
fn mv_store_clear_resets_aggregate_state() {
    let mut store = make_mv_store_with_sum_count_avg();
    store.apply_aggregate_insert(&make_order_row(1, 10.0, "paid"));
    assert_eq!(store.aggregate_states()[0].sum, 10.0);

    store.clear();
    // 聚合状态已重置
    assert_eq!(store.aggregate_states()[0].sum, 0.0);
    assert_eq!(store.aggregate_states()[0].count, 0);
    // 聚合行已重建（1 行，全 NULL）
    assert_eq!(store.active_row_count(), 1);
    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    assert_eq!(row[0], Value::Null);
}

#[test]
fn mv_store_min_max_delete_returns_false() {
    let mut store = make_mv_store_with_min_max();
    store.apply_aggregate_insert(&make_order_row(1, 10.0, "paid"));
    store.apply_aggregate_insert(&make_order_row(2, 20.0, "pending"));

    // MIN/MAX 的 DELETE 返回 false
    let ok = store.apply_aggregate_delete(&make_order_row(1, 10.0, "paid"));
    assert!(!ok);
    // MIN/MAX 状态不变
    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    assert_eq!(row[0], Value::Float64(10.0)); // min
    assert_eq!(row[1], Value::Float64(20.0)); // max
}

// =====================================================================
//  CDC DeleteWithRow 构造测试（2 条）
// =====================================================================

#[test]
fn cdc_event_delete_with_row_construction() {
    let event = CdcEvent::delete_with_row(
        "orders",
        vec![Value::Int64(1)],
        vec![
            Value::Int64(1),
            Value::Float64(10.0),
            Value::Text("paid".into()),
        ],
    );
    assert_eq!(event.kind_str(), "DELETE");
    match &event {
        CdcEvent::Delete {
            source_table,
            pk,
            row: Some(old_row),
        } => {
            assert_eq!(source_table.name, "orders");
            assert_eq!(pk, &vec![Value::Int64(1)]);
            assert_eq!(old_row.len(), 3);
        }
        _ => panic!("expected Delete with row=Some, got {event:?}"),
    }
}

#[test]
fn cdc_feed_push_delete_with_row() {
    let mut feed = CdcFeed::new();
    feed.push_delete_with_row(
        "orders",
        vec![Value::Int64(1)],
        vec![
            Value::Int64(1),
            Value::Float64(10.0),
            Value::Text("paid".into()),
        ],
    );
    assert_eq!(feed.len(), 1);
    let events = feed.drain();
    assert!(matches!(events[0], CdcEvent::Delete { row: Some(_), .. }));
}

// =====================================================================
//  AGGREGATE 刷新基础测试（5 条）
// =====================================================================

#[test]
fn aggregate_refresh_single_insert() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_sum_count_avg();
    let mut feed = CdcFeed::new();
    feed.push_insert("orders", make_order_row(1, 10.0, "paid"));

    let executor = Executor::new();
    let view_name = TableName::new("mv_agg");
    let source = TableName::new("orders");
    let outcome = executor
        .refresh_materialized_view_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 1);
    assert_eq!(outcome.rows_removed, 0);
    assert_eq!(outcome.rows_updated, 0); // decrements_failed
    assert_eq!(outcome.mode, RefreshMode::Aggregate);
    assert_eq!(outcome.total_rows, 1); // 聚合行

    // 验证聚合值
    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    assert_eq!(row[0], Value::Float64(10.0)); // sum
    assert_eq!(row[1], Value::Int64(1)); // count
    assert_eq!(row[2], Value::Float64(10.0)); // avg
}

#[test]
fn aggregate_refresh_single_delete_with_row() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_sum_count_avg();
    // 预填充：INSERT (id=1, amount=10.0)
    store.apply_aggregate_insert(&make_order_row(1, 10.0, "paid"));

    let mut feed = CdcFeed::new();
    feed.push_delete_with_row(
        "orders",
        vec![Value::Int64(1)],
        make_order_row(1, 10.0, "paid"),
    );

    let executor = Executor::new();
    let view_name = TableName::new("mv_agg");
    let source = TableName::new("orders");
    let outcome = executor
        .refresh_materialized_view_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 0);
    assert_eq!(outcome.rows_removed, 1);
    assert_eq!(outcome.rows_updated, 0); // SUM/COUNT/AVG 都能递减

    // sum = 0.0, count = 0, avg = NULL
    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    assert_eq!(row[0], Value::Float64(0.0));
    assert_eq!(row[1], Value::Int64(0));
    assert_eq!(row[2], Value::Null);
}

#[test]
fn aggregate_refresh_mixed_insert_delete() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_sum_count_avg();

    let mut feed = CdcFeed::new();
    // INSERT 3 行：10, 20, 30
    feed.push_insert("orders", make_order_row(1, 10.0, "a"));
    feed.push_insert("orders", make_order_row(2, 20.0, "b"));
    feed.push_insert("orders", make_order_row(3, 30.0, "c"));
    // DELETE 1 行：20
    feed.push_delete_with_row(
        "orders",
        vec![Value::Int64(2)],
        make_order_row(2, 20.0, "b"),
    );

    let executor = Executor::new();
    let view_name = TableName::new("mv_agg");
    let source = TableName::new("orders");
    let outcome = executor
        .refresh_materialized_view_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 3);
    assert_eq!(outcome.rows_removed, 1);
    assert_eq!(outcome.rows_updated, 0);

    // sum = 10 + 30 = 40, count = 2, avg = 20.0
    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    assert_eq!(row[0], Value::Float64(40.0));
    assert_eq!(row[1], Value::Int64(2));
    assert_eq!(row[2], Value::Float64(20.0));
}

#[test]
fn aggregate_refresh_empty_feed_is_noop() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_sum_count_avg();
    store.apply_aggregate_insert(&make_order_row(1, 10.0, "a"));

    let mut feed = CdcFeed::new();
    let executor = Executor::new();
    let view_name = TableName::new("mv_agg");
    let source = TableName::new("orders");
    let outcome = executor
        .refresh_materialized_view_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 0);
    assert_eq!(outcome.rows_removed, 0);
    assert_eq!(outcome.total_rows, 1);

    // 聚合值不变
    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    assert_eq!(row[0], Value::Float64(10.0));
}

#[test]
fn aggregate_refresh_update_degrades_to_insert() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_sum_count_avg();

    let mut feed = CdcFeed::new();
    // UPDATE 视为 INSERT（聚合值偏高）
    feed.push_update(
        "orders",
        vec![Value::Int64(1)],
        make_order_row(1, 10.0, "paid"),
    );

    let executor = Executor::new();
    let view_name = TableName::new("mv_agg");
    let source = TableName::new("orders");
    let outcome = executor
        .refresh_materialized_view_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 1); // UPDATE 退化为 INSERT
    assert_eq!(outcome.rows_removed, 0);

    // sum = 10.0
    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    assert_eq!(row[0], Value::Float64(10.0));
}

// =====================================================================
//  AGGREGATE 刷新多轮累积测试（1 条）
// =====================================================================

#[test]
fn aggregate_refresh_multiple_rounds_accumulate() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_sum_count_avg();
    let executor = Executor::new();
    let view_name = TableName::new("mv_agg");
    let source = TableName::new("orders");

    // 第一轮：INSERT 10, 20, 30
    let mut feed1 = CdcFeed::new();
    feed1.push_insert("orders", make_order_row(1, 10.0, "a"));
    feed1.push_insert("orders", make_order_row(2, 20.0, "b"));
    feed1.push_insert("orders", make_order_row(3, 30.0, "c"));
    let o1 = executor
        .refresh_materialized_view_aggregate(
            &view_name, &catalog, &mut store, &mut feed1, &source, 1000,
        )
        .unwrap();
    assert_eq!(o1.rows_appended, 3);
    // sum = 60, count = 3, avg = 20

    // 第二轮：DELETE 20
    let mut feed2 = CdcFeed::new();
    feed2.push_delete_with_row(
        "orders",
        vec![Value::Int64(2)],
        make_order_row(2, 20.0, "b"),
    );
    let o2 = executor
        .refresh_materialized_view_aggregate(
            &view_name, &catalog, &mut store, &mut feed2, &source, 2000,
        )
        .unwrap();
    assert_eq!(o2.rows_appended, 0);
    assert_eq!(o2.rows_removed, 1);
    // sum = 40, count = 2, avg = 20

    // 第三轮：INSERT 40
    let mut feed3 = CdcFeed::new();
    feed3.push_insert("orders", make_order_row(4, 40.0, "d"));
    let o3 = executor
        .refresh_materialized_view_aggregate(
            &view_name, &catalog, &mut store, &mut feed3, &source, 3000,
        )
        .unwrap();
    assert_eq!(o3.rows_appended, 1);
    // sum = 80, count = 3, avg = 80/3 ≈ 26.67

    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    assert_eq!(row[0], Value::Float64(80.0));
    assert_eq!(row[1], Value::Int64(3));
    match &row[2] {
        Value::Float64(v) => assert!((v - 80.0 / 3.0).abs() < 1e-9),
        other => panic!("expected Float64 avg, got {other:?}"),
    }

    // 高水位推进：3 + 1 + 1 = 5
    assert_eq!(store.high_water_mark(&source), 5);
}

// =====================================================================
//  错误场景测试（3 条）
// =====================================================================

#[test]
fn aggregate_refresh_nonexistent_view_returns_error() {
    let catalog = make_catalog_with_orders();
    let mut store = make_mv_store_with_sum_count_avg();
    let mut feed = CdcFeed::new();
    let executor = Executor::new();
    let view_name = TableName::new("nonexistent");
    let source = TableName::new("orders");
    let result = executor.refresh_materialized_view_aggregate(
        &view_name, &catalog, &mut store, &mut feed, &source, 1000,
    );
    assert!(result.is_err());
}

#[test]
fn aggregate_refresh_non_materialized_view_returns_error() {
    let mut catalog = make_catalog_with_orders();
    // 创建普通视图（非物化）
    let plan = plan_sql("CREATE VIEW v AS SELECT id, amount FROM orders", &catalog);
    let executor = Executor::new();
    executor.execute_create_view(&plan, &mut catalog).unwrap();

    let mut store = make_mv_store_with_sum_count_avg();
    let mut feed = CdcFeed::new();
    let view_name = TableName::new("v");
    let source = TableName::new("orders");
    let result = executor.refresh_materialized_view_aggregate(
        &view_name, &catalog, &mut store, &mut feed, &source, 1000,
    );
    assert!(result.is_err());
}

#[test]
fn aggregate_refresh_no_aggregate_specs_returns_error() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    // 使用无聚合规格的 store
    let mut store = MaterializedViewStore::new("mv_agg", vec![("sum_amount", ColumnType::Float64)]);
    let mut feed = CdcFeed::new();
    let executor = Executor::new();
    let view_name = TableName::new("mv_agg");
    let source = TableName::new("orders");
    let result = executor.refresh_materialized_view_aggregate(
        &view_name, &catalog, &mut store, &mut feed, &source, 1000,
    );
    assert!(result.is_err());
}

// =====================================================================
//  增量 vs 全量等价性测试（1 条）
// =====================================================================

#[test]
fn aggregate_refresh_vs_full_equivalence_100_rows() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_sum_count_avg();
    let executor = Executor::new();
    let view_name = TableName::new("mv_agg");
    let source = TableName::new("orders");

    // 增量刷新：INSERT 100 行（amount = 1.0..100.0）
    let mut feed = CdcFeed::new();
    for i in 1..=100 {
        feed.push_insert("orders", make_order_row(i, i as f64, "a"));
    }
    // DELETE 50 行（amount = 1.0..50.0）
    for i in 1..=50 {
        feed.push_delete_with_row(
            "orders",
            vec![Value::Int64(i)],
            make_order_row(i, i as f64, "a"),
        );
    }
    let outcome = executor
        .refresh_materialized_view_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 100);
    assert_eq!(outcome.rows_removed, 50);

    // 全量重算：sum = 51+52+...+100 = (51+100)*50/2 = 3775
    // count = 50
    // avg = 3775 / 50 = 75.5
    let expected_sum: f64 = (51..=100).map(|i| i as f64).sum();
    let expected_count = 50;
    let expected_avg = expected_sum / expected_count as f64;

    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    match &row[0] {
        Value::Float64(v) => assert!((v - expected_sum).abs() < 1e-6),
        other => panic!("expected Float64 sum, got {other:?}"),
    }
    assert_eq!(row[1], Value::Int64(expected_count));
    match &row[2] {
        Value::Float64(v) => assert!((v - expected_avg).abs() < 1e-6),
        other => panic!("expected Float64 avg, got {other:?}"),
    }
}

// =====================================================================
//  压力测试（1 条）
// =====================================================================

#[test]
fn aggregate_refresh_stress_100k_insert_50k_delete() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_sum_count_avg();
    let executor = Executor::new();
    let view_name = TableName::new("mv_agg");
    let source = TableName::new("orders");

    // INSERT 100K 行（amount = 1.0..100000.0）
    let mut feed = CdcFeed::new();
    for i in 1..=100_000 {
        feed.push_insert("orders", make_order_row(i, i as f64, "a"));
    }
    // DELETE 50K 行（amount = 1.0..50000.0）
    for i in 1..=50_000 {
        feed.push_delete_with_row(
            "orders",
            vec![Value::Int64(i)],
            make_order_row(i, i as f64, "a"),
        );
    }
    let outcome = executor
        .refresh_materialized_view_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 100_000);
    assert_eq!(outcome.rows_removed, 50_000);
    assert_eq!(outcome.rows_updated, 0); // SUM/COUNT/AVG 都递减成功

    // 全量重算：sum = 50001+...+100000 = (50001+100000)*50000/2 = 3750025000
    let expected_sum: f64 = (50_001..=100_000).map(|i| i as f64).sum();
    let expected_count = 50_000;

    let row_id = store.aggregate_row_id().unwrap();
    let row = store.storage.get_row(row_id).unwrap();
    match &row[0] {
        Value::Float64(v) => assert!((v - expected_sum).abs() < 1e-3),
        other => panic!("expected Float64 sum, got {other:?}"),
    }
    assert_eq!(row[1], Value::Int64(expected_count));
}

// =====================================================================
//  RefreshOutcome AGGREGATE 测试（2 条）
// =====================================================================

#[test]
fn refresh_outcome_aggregate_construction() {
    let outcome = RefreshOutcome::aggregate(100, 50, 2, 1);
    assert_eq!(outcome.rows_appended, 100);
    assert_eq!(outcome.rows_removed, 50);
    assert_eq!(outcome.rows_updated, 2); // decrements_failed
    assert_eq!(outcome.mode, RefreshMode::Aggregate);
    assert_eq!(outcome.total_rows, 1);
}

#[test]
fn refresh_outcome_aggregate_clone_eq() {
    let outcome = RefreshOutcome::aggregate(10, 5, 1, 1);
    let cloned = outcome.clone();
    assert_eq!(outcome, cloned);
}
