//! Phase 6.14 集成测试 — GROUP_AGGREGATE 分组聚合增量刷新。
//!
//! 覆盖类别：
//! - MaterializedViewStore 分组聚合基础（5 条）：new_with_group_aggregates + has_group_aggregates +
//!   apply_group_aggregate_insert 新分组 + 同分组递增 + apply_group_aggregate_delete 递减
//! - GROUP_AGGREGATE 刷新基础（5 条）：单 INSERT / 多分组 INSERT / 混合 INSERT+DELETE / 空 feed /
//!   Update 退化为 INSERT
//! - GROUP_AGGREGATE 多轮累积（1 条）：3 轮 INSERT+DELETE + 高水位推进
//! - 错误场景（3 条）：nonexistent view / non-materialized view / 无分组聚合规格
//! - 增量 vs 全量等价性（1 条）：100 行 INSERT + 50 DELETE，对比全量重算
//! - 1000 分组压力测试（1 条）：源表分 1000 个分组 INSERT → 每组预聚合独立更新 →
//!   每组的 SUM/COUNT/AVG 正确
//! - clear 重置（1 条）：clear 后分组状态归零
//! - RefreshOutcome GROUP_AGGREGATE（2 条）：构造 / clone_eq
//!
//! 共 19 个测试用例。

use super::executor::{Executor, TableStorage};
use super::materialized_view::{
    AggregateFunction, AggregateSpec, CdcEvent, CdcFeed, MaterializedViewStore, RefreshMode,
    RefreshOutcome,
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

/// 创建物化视图 `mv_group_agg`（SELECT id, amount, status FROM orders）
fn setup_materialized_view(catalog: &mut InMemoryCatalog) {
    let plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv_group_agg AS SELECT id, amount, status FROM orders",
        catalog,
    );
    let executor = Executor::new();
    executor.execute_create_view(&plan, catalog).unwrap();
}

/// 创建带 SUM(amount) + COUNT(amount) + AVG(amount) 分组聚合的物化视图存储
///
/// 列布局：[status, sum_amount, count_amount, avg_amount]
/// - status（输出列 0）= 分组列，源列 2（status）
/// - sum_amount（输出列 1）= SUM(amount)，源列 1（amount）
/// - count_amount（输出列 2）= COUNT(amount)，源列 1（amount）
/// - avg_amount（输出列 3）= AVG(amount)，源列 1（amount）
fn make_mv_store_with_group_sum_count_avg() -> MaterializedViewStore {
    MaterializedViewStore::new_with_group_aggregates(
        "mv_group_agg",
        vec![
            ("status", ColumnType::Text),
            ("sum_amount", ColumnType::Float64),
            ("count_amount", ColumnType::Int64),
            ("avg_amount", ColumnType::Float64),
        ],
        vec![2], // group_column_indices: status 在源行中的索引
        vec![0], // group_output_indices: status 在存储表中的索引
        vec![
            AggregateSpec::new(AggregateFunction::Sum, 1, 1),
            AggregateSpec::new(AggregateFunction::Count, 1, 2),
            AggregateSpec::new(AggregateFunction::Avg, 1, 3),
        ],
    )
}

/// 创建带 MIN(amount) + MAX(amount) 分组聚合的物化视图存储
///
/// 列布局：[status, min_amount, max_amount]
fn make_mv_store_with_group_min_max() -> MaterializedViewStore {
    MaterializedViewStore::new_with_group_aggregates(
        "mv_group_agg",
        vec![
            ("status", ColumnType::Text),
            ("min_amount", ColumnType::Float64),
            ("max_amount", ColumnType::Float64),
        ],
        vec![2], // group_column_indices
        vec![0], // group_output_indices
        vec![
            AggregateSpec::new(AggregateFunction::Min, 1, 1),
            AggregateSpec::new(AggregateFunction::Max, 1, 2),
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

/// 按分组列（status）查找存储行，返回该分组的聚合值
fn find_group_row(store: &MaterializedViewStore, status: &str) -> Option<Vec<Value>> {
    for row in store.storage.scan_iter() {
        if let Some(Value::Text(s)) = row.first() {
            if s == status {
                return Some(row);
            }
        }
    }
    None
}

// =====================================================================
//  MaterializedViewStore 分组聚合基础测试（5 条）
// =====================================================================

#[test]
fn mv_store_group_agg_new_with_group_aggregates() {
    let store = make_mv_store_with_group_sum_count_avg();
    assert!(store.has_group_aggregates());
    assert_eq!(store.group_count(), 0);
    assert_eq!(store.group_column_indices(), &[2]);
    assert_eq!(store.group_output_indices(), &[0]);
    assert_eq!(store.aggregate_specs().len(), 3);
    assert_eq!(store.active_row_count(), 0);
}

#[test]
fn mv_store_group_agg_apply_insert_creates_new_group() {
    let mut store = make_mv_store_with_group_sum_count_avg();
    let row = make_order_row(1, 100.0, "paid");
    let is_new = store.apply_group_aggregate_insert(&row);
    assert!(is_new);
    assert_eq!(store.group_count(), 1);
    assert_eq!(store.active_row_count(), 1);

    // 验证分组行
    let group_row = find_group_row(&store, "paid").expect("group row should exist");
    assert_eq!(group_row[0], Value::Text("paid".into())); // status
    assert_eq!(group_row[1], Value::Float64(100.0)); // sum
    assert_eq!(group_row[2], Value::Int64(1)); // count
    assert_eq!(group_row[3], Value::Float64(100.0)); // avg
}

#[test]
fn mv_store_group_agg_apply_insert_same_group_accumulates() {
    let mut store = make_mv_store_with_group_sum_count_avg();
    // 同一分组 "paid" 插入 3 行
    store.apply_group_aggregate_insert(&make_order_row(1, 100.0, "paid"));
    let is_new_2 = store.apply_group_aggregate_insert(&make_order_row(2, 200.0, "paid"));
    let is_new_3 = store.apply_group_aggregate_insert(&make_order_row(3, 300.0, "paid"));
    assert!(!is_new_2);
    assert!(!is_new_3);
    assert_eq!(store.group_count(), 1);
    assert_eq!(store.active_row_count(), 1);

    // sum = 600, count = 3, avg = 200
    let group_row = find_group_row(&store, "paid").expect("group row");
    assert_eq!(group_row[1], Value::Float64(600.0));
    assert_eq!(group_row[2], Value::Int64(3));
    assert_eq!(group_row[3], Value::Float64(200.0));
}

#[test]
fn mv_store_group_agg_apply_insert_multiple_groups() {
    let mut store = make_mv_store_with_group_sum_count_avg();
    store.apply_group_aggregate_insert(&make_order_row(1, 100.0, "paid"));
    store.apply_group_aggregate_insert(&make_order_row(2, 50.0, "pending"));
    store.apply_group_aggregate_insert(&make_order_row(3, 200.0, "paid"));
    assert_eq!(store.group_count(), 2);
    assert_eq!(store.active_row_count(), 2);

    // "paid" 组: sum=300, count=2, avg=150
    let paid_row = find_group_row(&store, "paid").expect("paid group");
    assert_eq!(paid_row[1], Value::Float64(300.0));
    assert_eq!(paid_row[2], Value::Int64(2));
    assert_eq!(paid_row[3], Value::Float64(150.0));

    // "pending" 组: sum=50, count=1, avg=50
    let pending_row = find_group_row(&store, "pending").expect("pending group");
    assert_eq!(pending_row[1], Value::Float64(50.0));
    assert_eq!(pending_row[2], Value::Int64(1));
    assert_eq!(pending_row[3], Value::Float64(50.0));
}

#[test]
fn mv_store_group_agg_apply_delete_decrements() {
    let mut store = make_mv_store_with_group_sum_count_avg();
    store.apply_group_aggregate_insert(&make_order_row(1, 100.0, "paid"));
    store.apply_group_aggregate_insert(&make_order_row(2, 200.0, "paid"));
    store.apply_group_aggregate_insert(&make_order_row(3, 300.0, "paid"));
    // sum=600, count=3

    // DELETE id=2 (amount=200)
    let ok = store.apply_group_aggregate_delete(&make_order_row(2, 200.0, "paid"));
    assert!(ok);
    // sum=400, count=2, avg=200
    let group_row = find_group_row(&store, "paid").expect("group row");
    assert_eq!(group_row[1], Value::Float64(400.0));
    assert_eq!(group_row[2], Value::Int64(2));
    assert_eq!(group_row[3], Value::Float64(200.0));
}

#[test]
fn mv_store_group_agg_apply_delete_nonexistent_group_returns_false() {
    let mut store = make_mv_store_with_group_sum_count_avg();
    store.apply_group_aggregate_insert(&make_order_row(1, 100.0, "paid"));
    // DELETE 不存在的分组
    let ok = store.apply_group_aggregate_delete(&make_order_row(2, 200.0, "shipped"));
    assert!(!ok);
    // 原分组不变
    let group_row = find_group_row(&store, "paid").expect("group row");
    assert_eq!(group_row[1], Value::Float64(100.0));
    assert_eq!(group_row[2], Value::Int64(1));
}

#[test]
fn mv_store_group_agg_min_max_delete_returns_false() {
    let mut store = make_mv_store_with_group_min_max();
    store.apply_group_aggregate_insert(&make_order_row(1, 100.0, "paid"));
    store.apply_group_aggregate_insert(&make_order_row(2, 50.0, "paid"));
    store.apply_group_aggregate_insert(&make_order_row(3, 200.0, "paid"));
    // min=50, max=200

    let group_row = find_group_row(&store, "paid").expect("group row");
    assert_eq!(group_row[1], Value::Float64(50.0)); // min
    assert_eq!(group_row[2], Value::Float64(200.0)); // max

    // MIN/MAX DELETE 返回 false
    let ok = store.apply_group_aggregate_delete(&make_order_row(2, 50.0, "paid"));
    assert!(!ok);
    // 状态不变（MIN/MAX 无法递减）
    let group_row2 = find_group_row(&store, "paid").expect("group row");
    assert_eq!(group_row2[1], Value::Float64(50.0));
    assert_eq!(group_row2[2], Value::Float64(200.0));
}

// =====================================================================
//  GROUP_AGGREGATE 刷新基础测试（5 条）
// =====================================================================

#[test]
fn group_aggregate_refresh_single_insert() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_group_sum_count_avg();
    let mut feed = CdcFeed::new();
    feed.push_insert("orders", make_order_row(1, 100.0, "paid"));

    let executor = Executor::new();
    let view_name = TableName::new("mv_group_agg");
    let source = TableName::new("orders");
    let outcome = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 1);
    assert_eq!(outcome.rows_removed, 0);
    assert_eq!(outcome.rows_updated, 0); // decrements_failed
    assert_eq!(outcome.mode, RefreshMode::GroupAggregate);
    assert_eq!(outcome.total_rows, 1); // 1 group
    assert_eq!(store.group_count(), 1);
}

#[test]
fn group_aggregate_refresh_multiple_groups_insert() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_group_sum_count_avg();
    let mut feed = CdcFeed::new();
    feed.push_insert("orders", make_order_row(1, 100.0, "paid"));
    feed.push_insert("orders", make_order_row(2, 200.0, "paid"));
    feed.push_insert("orders", make_order_row(3, 50.0, "pending"));
    feed.push_insert("orders", make_order_row(4, 150.0, "shipped"));

    let executor = Executor::new();
    let view_name = TableName::new("mv_group_agg");
    let source = TableName::new("orders");
    let outcome = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 4);
    assert_eq!(outcome.total_rows, 3); // 3 groups
    assert_eq!(store.group_count(), 3);

    // paid: sum=300, count=2, avg=150
    let paid = find_group_row(&store, "paid").expect("paid");
    assert_eq!(paid[1], Value::Float64(300.0));
    assert_eq!(paid[2], Value::Int64(2));
    assert_eq!(paid[3], Value::Float64(150.0));

    // pending: sum=50, count=1, avg=50
    let pending = find_group_row(&store, "pending").expect("pending");
    assert_eq!(pending[1], Value::Float64(50.0));
    assert_eq!(pending[2], Value::Int64(1));
    assert_eq!(pending[3], Value::Float64(50.0));

    // shipped: sum=150, count=1, avg=150
    let shipped = find_group_row(&store, "shipped").expect("shipped");
    assert_eq!(shipped[1], Value::Float64(150.0));
    assert_eq!(shipped[2], Value::Int64(1));
    assert_eq!(shipped[3], Value::Float64(150.0));
}

#[test]
fn group_aggregate_refresh_mixed_insert_delete() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_group_sum_count_avg();
    let mut feed = CdcFeed::new();
    feed.push_insert("orders", make_order_row(1, 100.0, "paid"));
    feed.push_insert("orders", make_order_row(2, 200.0, "paid"));
    feed.push_insert("orders", make_order_row(3, 50.0, "pending"));
    // DELETE id=1 (amount=100, status=paid)
    feed.push_delete_with_row(
        "orders",
        vec![Value::Int64(1)],
        make_order_row(1, 100.0, "paid"),
    );

    let executor = Executor::new();
    let view_name = TableName::new("mv_group_agg");
    let source = TableName::new("orders");
    let outcome = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 3); // 3 INSERTs
    assert_eq!(outcome.rows_removed, 1); // 1 DELETE
    assert_eq!(outcome.rows_updated, 0); // no decrement failures
    assert_eq!(outcome.total_rows, 2); // 2 groups

    // paid: sum=200 (300-100), count=1 (2-1), avg=200
    let paid = find_group_row(&store, "paid").expect("paid");
    assert_eq!(paid[1], Value::Float64(200.0));
    assert_eq!(paid[2], Value::Int64(1));
    assert_eq!(paid[3], Value::Float64(200.0));

    // pending: sum=50, count=1, avg=50
    let pending = find_group_row(&store, "pending").expect("pending");
    assert_eq!(pending[1], Value::Float64(50.0));
    assert_eq!(pending[2], Value::Int64(1));
}

#[test]
fn group_aggregate_refresh_empty_feed_is_noop() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_group_sum_count_avg();
    // 预填充一个分组
    store.apply_group_aggregate_insert(&make_order_row(1, 100.0, "paid"));
    let mut feed = CdcFeed::new();

    let executor = Executor::new();
    let view_name = TableName::new("mv_group_agg");
    let source = TableName::new("orders");
    let outcome = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 0);
    assert_eq!(outcome.rows_removed, 0);
    assert_eq!(outcome.rows_updated, 0);
    assert_eq!(outcome.total_rows, 1); // still 1 group
    assert_eq!(store.group_count(), 1);
}

#[test]
fn group_aggregate_refresh_update_degrades_to_insert() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_group_sum_count_avg();
    let mut feed = CdcFeed::new();
    // UPDATE 视为 INSERT（新行），不递减旧行
    feed.push_update(
        "orders",
        vec![Value::Int64(1)],
        make_order_row(1, 100.0, "paid"),
    );

    let executor = Executor::new();
    let view_name = TableName::new("mv_group_agg");
    let source = TableName::new("orders");
    let outcome = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 1); // UPDATE counted as INSERT
    assert_eq!(outcome.total_rows, 1);
    // paid: sum=100, count=1, avg=100
    let paid = find_group_row(&store, "paid").expect("paid");
    assert_eq!(paid[1], Value::Float64(100.0));
    assert_eq!(paid[2], Value::Int64(1));
    assert_eq!(paid[3], Value::Float64(100.0));
}

// =====================================================================
//  GROUP_AGGREGATE 多轮累积测试（1 条）
// =====================================================================

#[test]
fn group_aggregate_refresh_multiple_rounds_accumulate() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_group_sum_count_avg();
    let executor = Executor::new();
    let view_name = TableName::new("mv_group_agg");
    let source = TableName::new("orders");

    // 第一轮：INSERT paid 100, pending 50
    let mut feed1 = CdcFeed::new();
    feed1.push_insert("orders", make_order_row(1, 100.0, "paid"));
    feed1.push_insert("orders", make_order_row(2, 50.0, "pending"));
    let o1 = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed1, &source, 1000,
        )
        .unwrap();
    assert_eq!(o1.rows_appended, 2);
    assert_eq!(o1.total_rows, 2);

    // 第二轮：INSERT paid 200, DELETE paid 100
    let mut feed2 = CdcFeed::new();
    feed2.push_insert("orders", make_order_row(3, 200.0, "paid"));
    feed2.push_delete_with_row(
        "orders",
        vec![Value::Int64(1)],
        make_order_row(1, 100.0, "paid"),
    );
    let o2 = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed2, &source, 2000,
        )
        .unwrap();
    assert_eq!(o2.rows_appended, 1);
    assert_eq!(o2.rows_removed, 1);
    assert_eq!(o2.total_rows, 2);

    // paid: sum=200 (100+200-100), count=1 (2-1), avg=200
    let paid = find_group_row(&store, "paid").expect("paid");
    assert_eq!(paid[1], Value::Float64(200.0));
    assert_eq!(paid[2], Value::Int64(1));
    assert_eq!(paid[3], Value::Float64(200.0));

    // pending: sum=50, count=1
    let pending = find_group_row(&store, "pending").expect("pending");
    assert_eq!(pending[1], Value::Float64(50.0));
    assert_eq!(pending[2], Value::Int64(1));

    // 第三轮：INSERT shipped 300
    let mut feed3 = CdcFeed::new();
    feed3.push_insert("orders", make_order_row(4, 300.0, "shipped"));
    let o3 = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed3, &source, 3000,
        )
        .unwrap();
    assert_eq!(o3.rows_appended, 1);
    assert_eq!(o3.total_rows, 3);

    // 高水位推进验证
    assert_eq!(store.high_water_mark(&source), 2 + 2 + 1); // 5 events total
}

// =====================================================================
//  错误场景测试（3 条）
// =====================================================================

#[test]
fn group_aggregate_refresh_nonexistent_view_returns_error() {
    let catalog = make_catalog_with_orders();
    let mut store = make_mv_store_with_group_sum_count_avg();
    let mut feed = CdcFeed::new();
    feed.push_insert("orders", make_order_row(1, 100.0, "paid"));

    let executor = Executor::new();
    let view_name = TableName::new("nonexistent_mv");
    let source = TableName::new("orders");
    let result = executor.refresh_materialized_view_group_aggregate(
        &view_name, &catalog, &mut store, &mut feed, &source, 1000,
    );
    assert!(result.is_err());
}

#[test]
fn group_aggregate_refresh_non_materialized_view_returns_error() {
    let mut catalog = make_catalog_with_orders();
    // 创建普通视图（非物化）
    let plan = plan_sql("CREATE VIEW v_normal AS SELECT * FROM orders", &catalog);
    let executor = Executor::new();
    executor.execute_create_view(&plan, &mut catalog).unwrap();

    let mut store = make_mv_store_with_group_sum_count_avg();
    let mut feed = CdcFeed::new();
    feed.push_insert("orders", make_order_row(1, 100.0, "paid"));

    let view_name = TableName::new("v_normal");
    let source = TableName::new("orders");
    let result = executor.refresh_materialized_view_group_aggregate(
        &view_name, &catalog, &mut store, &mut feed, &source, 1000,
    );
    assert!(result.is_err());
}

#[test]
fn group_aggregate_refresh_no_group_aggregates_returns_error() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    // 使用普通构造（无分组聚合规格）
    let mut store = MaterializedViewStore::new("mv_group_agg", vec![("status", ColumnType::Text)]);
    let mut feed = CdcFeed::new();
    feed.push_insert("orders", make_order_row(1, 100.0, "paid"));

    let executor = Executor::new();
    let view_name = TableName::new("mv_group_agg");
    let source = TableName::new("orders");
    let result = executor.refresh_materialized_view_group_aggregate(
        &view_name, &catalog, &mut store, &mut feed, &source, 1000,
    );
    assert!(result.is_err());
}

// =====================================================================
//  增量 vs 全量等价性测试（1 条）
// =====================================================================

#[test]
fn group_aggregate_incremental_equals_full_recompute() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let executor = Executor::new();
    let view_name = TableName::new("mv_group_agg");
    let source = TableName::new("orders");

    // 增量刷新：100 行 INSERT + 50 行 DELETE，分布在 3 个分组
    let mut store = MaterializedViewStore::new_with_group_aggregates(
        "mv_group_agg",
        vec![
            ("status", ColumnType::Text),
            ("sum_amount", ColumnType::Float64),
            ("count_amount", ColumnType::Int64),
            ("avg_amount", ColumnType::Float64),
        ],
        vec![2],
        vec![0],
        vec![
            AggregateSpec::new(AggregateFunction::Sum, 1, 1),
            AggregateSpec::new(AggregateFunction::Count, 1, 2),
            AggregateSpec::new(AggregateFunction::Avg, 1, 3),
        ],
    );

    let statuses = ["paid", "pending", "shipped"];
    let mut feed = CdcFeed::new();
    let mut inserted_rows: Vec<(i64, f64, String)> = Vec::new();
    for i in 0..100i64 {
        let status = statuses[(i as usize) % 3];
        let amount = (i as f64) * 1.5 + 10.0;
        feed.push_insert("orders", make_order_row(i, amount, status));
        inserted_rows.push((i, amount, status.to_string()));
    }
    // DELETE 前 50 行
    for (id, amount, status) in inserted_rows.iter().take(50) {
        feed.push_delete_with_row(
            "orders",
            vec![Value::Int64(*id)],
            make_order_row(*id, *amount, status),
        );
    }
    let outcome = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 100);
    assert_eq!(outcome.rows_removed, 50);

    // 全量重算：遍历剩余 50 行，按分组计算期望值
    let mut expected: std::collections::HashMap<String, (f64, i64)> =
        std::collections::HashMap::new();
    for (_id, amount, status) in inserted_rows.iter().skip(50) {
        let entry = expected.entry(status.clone()).or_insert((0.0, 0));
        entry.0 += *amount;
        entry.1 += 1;
    }

    // 验证每个分组的 SUM/COUNT/AVG
    for (status, (sum, count)) in &expected {
        let row =
            find_group_row(&store, status).unwrap_or_else(|| panic!("group {status} should exist"));
        assert_eq!(row[1], Value::Float64(*sum), "SUM mismatch for {status}");
        assert_eq!(row[2], Value::Int64(*count), "COUNT mismatch for {status}");
        let avg = if *count > 0 {
            sum / *count as f64
        } else {
            0.0
        };
        assert_eq!(row[3], Value::Float64(avg), "AVG mismatch for {status}");
    }
    assert_eq!(store.group_count(), 3);
}

// =====================================================================
//  1000 分组压力测试（1 条）— Phase 6.14 验收标准
// =====================================================================

#[test]
fn group_aggregate_1000_groups_stress_test() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mut store = make_mv_store_with_group_sum_count_avg();
    let executor = Executor::new();
    let view_name = TableName::new("mv_group_agg");
    let source = TableName::new("orders");

    // 1000 个分组（group_000 ~ group_999），每组 10 行 INSERT
    let mut feed = CdcFeed::new();
    for g in 0..1000 {
        let status = format!("group_{g:03}");
        for i in 0..10 {
            let id = (g * 10 + i) as i64;
            let amount = (g as f64) * 100.0 + (i as f64) * 10.0;
            feed.push_insert("orders", make_order_row(id, amount, &status));
        }
    }
    let outcome = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed, &source, 1000,
        )
        .unwrap();
    assert_eq!(outcome.rows_appended, 10000); // 1000 groups × 10 rows
    assert_eq!(outcome.total_rows, 1000); // 1000 groups
    assert_eq!(store.group_count(), 1000);

    // 验证部分分组的 SUM/COUNT/AVG 正确性
    // group_000: amounts = 0,10,20,...,90 → sum=450, count=10, avg=45
    let g0 = find_group_row(&store, "group_000").expect("group_000");
    assert_eq!(g0[1], Value::Float64(450.0));
    assert_eq!(g0[2], Value::Int64(10));
    assert_eq!(g0[3], Value::Float64(45.0));

    // group_500: amounts = 50000,50010,...,50090 → sum=500450, count=10, avg=50045
    let g500 = find_group_row(&store, "group_500").expect("group_500");
    assert_eq!(g500[1], Value::Float64(500450.0));
    assert_eq!(g500[2], Value::Int64(10));
    assert_eq!(g500[3], Value::Float64(50045.0));

    // group_999: amounts = 99900,99910,...,99990 → sum=999450, count=10, avg=99945
    let g999 = find_group_row(&store, "group_999").expect("group_999");
    assert_eq!(g999[1], Value::Float64(999450.0));
    assert_eq!(g999[2], Value::Int64(10));
    assert_eq!(g999[3], Value::Float64(99945.0));

    // 第二轮：每组 DELETE 前 5 行，验证递减
    let mut feed2 = CdcFeed::new();
    for g in 0..1000 {
        let status = format!("group_{g:03}");
        for i in 0..5 {
            let id = (g * 10 + i) as i64;
            let amount = (g as f64) * 100.0 + (i as f64) * 10.0;
            feed2.push_delete_with_row(
                "orders",
                vec![Value::Int64(id)],
                make_order_row(id, amount, &status),
            );
        }
    }
    let outcome2 = executor
        .refresh_materialized_view_group_aggregate(
            &view_name, &catalog, &mut store, &mut feed2, &source, 2000,
        )
        .unwrap();
    assert_eq!(outcome2.rows_removed, 5000); // 1000 groups × 5 DELETEs
    assert_eq!(outcome2.total_rows, 1000);

    // group_000 after DELETE: amounts = 50,60,70,80,90 → sum=350, count=5, avg=70
    let g0_after = find_group_row(&store, "group_000").expect("group_000 after");
    assert_eq!(g0_after[1], Value::Float64(350.0));
    assert_eq!(g0_after[2], Value::Int64(5));
    assert_eq!(g0_after[3], Value::Float64(70.0));

    // group_999 after DELETE: amounts = 99950,99960,99970,99980,99990 → sum=499850, count=5, avg=99970
    let g999_after = find_group_row(&store, "group_999").expect("group_999 after");
    assert_eq!(g999_after[1], Value::Float64(499850.0));
    assert_eq!(g999_after[2], Value::Int64(5));
    assert_eq!(g999_after[3], Value::Float64(99970.0));
}

// =====================================================================
//  clear 重置测试（1 条）
// =====================================================================

#[test]
fn group_aggregate_clear_resets_group_states() {
    let mut store = make_mv_store_with_group_sum_count_avg();
    store.apply_group_aggregate_insert(&make_order_row(1, 100.0, "paid"));
    store.apply_group_aggregate_insert(&make_order_row(2, 200.0, "pending"));
    assert_eq!(store.group_count(), 2);
    assert_eq!(store.active_row_count(), 2);

    store.clear();
    assert_eq!(store.group_count(), 0);
    assert_eq!(store.active_row_count(), 0);
    // 清空后可继续使用
    store.apply_group_aggregate_insert(&make_order_row(3, 300.0, "shipped"));
    assert_eq!(store.group_count(), 1);
    let shipped = find_group_row(&store, "shipped").expect("shipped");
    assert_eq!(shipped[1], Value::Float64(300.0));
    assert_eq!(shipped[2], Value::Int64(1));
}

// =====================================================================
//  RefreshOutcome GROUP_AGGREGATE 测试（2 条）
// =====================================================================

#[test]
fn refresh_outcome_group_aggregate_construction() {
    let outcome = RefreshOutcome::group_aggregate(100, 50, 2, 48);
    assert_eq!(outcome.rows_appended, 100);
    assert_eq!(outcome.rows_removed, 50);
    assert_eq!(outcome.rows_updated, 2); // decrements_failed
    assert_eq!(outcome.mode, RefreshMode::GroupAggregate);
    assert_eq!(outcome.total_rows, 48);
}

#[test]
fn refresh_outcome_group_aggregate_clone_eq() {
    let outcome = RefreshOutcome::group_aggregate(10, 5, 1, 9);
    let cloned = outcome.clone();
    assert_eq!(outcome, cloned);
}

// =====================================================================
//  CDC DeleteWithRow 构造测试（1 条）— 验证 Phase 6.13 构造器在 6.14 场景下可用
// =====================================================================

#[test]
fn cdc_event_delete_with_row_for_group_aggregate() {
    let event = CdcEvent::delete_with_row(
        "orders",
        vec![Value::Int64(1)],
        make_order_row(1, 100.0, "paid"),
    );
    match event {
        CdcEvent::Delete {
            source_table,
            pk,
            row,
        } => {
            assert_eq!(source_table.name, "orders");
            assert_eq!(pk, vec![Value::Int64(1)]);
            let old_row = row.expect("row should be Some");
            assert_eq!(old_row.len(), 3);
            assert_eq!(old_row[0], Value::Int64(1));
            assert_eq!(old_row[1], Value::Float64(100.0));
            assert_eq!(old_row[2], Value::Text("paid".into()));
        }
        _ => panic!("expected Delete with row"),
    }
}
