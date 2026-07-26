//! Phase 6.15 集成测试 — 物化视图查询重写。
//!
//! 覆盖类别：
//! - 物化视图路由基础（3 条）：SELECT * FROM mv → MaterializedViewScan → 结果与源表一致
//! - EXPLAIN 格式化（3 条）：MV 查询显示 MaterializedViewScan / 源表显示 SeqScan / Projection+Filter 组合
//! - 源表路由不匹配（2 条）：SELECT * FROM orders → SeqScan → 走源表
//! - 物化视图 + 投影/过滤（2 条）：SELECT id FROM mv / SELECT * FROM mv WHERE ...
//! - 普通视图展开（2 条）：CREATE VIEW v AS ... → 展开为 Projection+Scan
//! - MV 未注册存储（1 条）：SELECT * FROM mv → TableNotFound
//! - 空 MV 存储（1 条）：MV 存储无数据 → 0 行
//! - 多物化视图（1 条）：两个 MV 同源，分别路由
//! - format_plan 多节点（2 条）：Aggregate / Sort 节点格式化
//!
//! 共 17 个测试用例。

use super::executor::{Executor, InMemoryTable};
use super::materialized_view::MaterializedViewStore;
use crate::ast::TableName;
use crate::parser::parse_one;
use crate::plan::{format_plan, InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::{ColumnType, Value};

// =====================================================================
//  辅助函数
// =====================================================================

/// 创建带 `orders` 表的 catalog（id INT PK, amount DOUBLE PRECISION, status TEXT）
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

/// 创建并注册一个物化视图 `mv`，查询为 `SELECT id, amount, status FROM orders`
fn setup_materialized_view(catalog: &mut InMemoryCatalog) {
    let plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv AS SELECT id, amount, status FROM orders",
        catalog,
    );
    let executor = Executor::new();
    executor.execute_create_view(&plan, catalog).unwrap();
}

/// 创建并注册一个物化视图 `mv2`，查询为 `SELECT id FROM orders`
fn setup_materialized_view_mv2(catalog: &mut InMemoryCatalog) {
    let plan = plan_sql(
        "CREATE MATERIALIZED VIEW mv2 AS SELECT id FROM orders",
        catalog,
    );
    let executor = Executor::new();
    executor.execute_create_view(&plan, catalog).unwrap();
}

/// 创建并注册一个普通视图 `v`，查询为 `SELECT id, amount FROM orders`
fn setup_regular_view(catalog: &mut InMemoryCatalog) {
    let plan = plan_sql("CREATE VIEW v AS SELECT id, amount FROM orders", catalog);
    let executor = Executor::new();
    executor.execute_create_view(&plan, catalog).unwrap();
}

/// 创建带 3 列（id, amount, status）的物化视图存储，并填充数据
fn make_filled_mv_store() -> MaterializedViewStore {
    let mut store = MaterializedViewStore::new(
        "mv",
        vec![
            ("id", ColumnType::Int64),
            ("amount", ColumnType::Float64),
            ("status", ColumnType::Text),
        ],
    );
    store.append_row(vec![
        Value::Int64(1),
        Value::Float64(10.0),
        Value::Text("paid".into()),
    ]);
    store.append_row(vec![
        Value::Int64(2),
        Value::Float64(20.0),
        Value::Text("pending".into()),
    ]);
    store.append_row(vec![
        Value::Int64(3),
        Value::Float64(30.0),
        Value::Text("paid".into()),
    ]);
    store
}

/// 创建带 1 列（id）的物化视图存储（用于 mv2）
fn make_filled_mv2_store() -> MaterializedViewStore {
    let mut store = MaterializedViewStore::new("mv2", vec![("id", ColumnType::Int64)]);
    store.append_row(vec![Value::Int64(1)]);
    store.append_row(vec![Value::Int64(2)]);
    store
}

/// 创建并填充源表 `orders`（3 行数据）
fn make_filled_orders_table() -> InMemoryTable {
    let mut table = InMemoryTable::with_columns(
        "orders",
        vec![
            ("id", ColumnType::Int64),
            ("amount", ColumnType::Float64),
            ("status", ColumnType::Text),
        ],
    );
    table.insert(vec![
        Value::Int64(1),
        Value::Float64(10.0),
        Value::Text("paid".into()),
    ]);
    table.insert(vec![
        Value::Int64(2),
        Value::Float64(20.0),
        Value::Text("pending".into()),
    ]);
    table.insert(vec![
        Value::Int64(3),
        Value::Float64(30.0),
        Value::Text("paid".into()),
    ]);
    table
}

// =====================================================================
//  物化视图路由基础测试（3 条）
// =====================================================================

#[test]
fn mv_query_rewrite_select_all_routes_to_mv_storage() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    let mv_store = make_filled_mv_store();

    // 规划 SELECT * FROM mv
    let plan = plan_sql("SELECT * FROM mv", &catalog);

    // SELECT * 会被规划为 Projection(MaterializedViewScan)，验证内部是 MaterializedViewScan
    let text = format_plan(&plan);
    assert!(
        text.contains("MaterializedViewScan: mv"),
        "expected MaterializedViewScan in plan, got: {text}"
    );

    // 执行查询
    let mut executor = Executor::new();
    executor.register_materialized_view_store("mv", &mv_store.storage);
    let result = executor.execute(&plan).unwrap();

    // 验证结果与 MV 存储一致（3 行）
    assert_eq!(result.len(), 3);
    assert_eq!(result[0][0], Value::Int64(1));
    assert_eq!(result[0][1], Value::Float64(10.0));
    assert_eq!(result[0][2], Value::Text("paid".into()));
    assert_eq!(result[2][0], Value::Int64(3));
}

#[test]
fn mv_query_rewrite_results_match_source_table() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);

    let orders_table = make_filled_orders_table();
    let mv_store = make_filled_mv_store();

    let mut executor = Executor::new();
    executor.register_table(&orders_table);
    executor.register_materialized_view_store("mv", &mv_store.storage);

    // 查询源表
    let source_plan = plan_sql("SELECT * FROM orders", &catalog);
    let source_result = executor.execute(&source_plan).unwrap();

    // 查询物化视图
    let mv_plan = plan_sql("SELECT * FROM mv", &catalog);
    let mv_result = executor.execute(&mv_plan).unwrap();

    // 结果应一致（行数和值）
    assert_eq!(source_result.len(), mv_result.len());
    for (src_row, mv_row) in source_result.iter().zip(mv_result.iter()) {
        assert_eq!(src_row, mv_row);
    }
}

#[test]
fn mv_query_rewrite_plan_has_correct_schema() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);

    let plan = plan_sql("SELECT * FROM mv", &catalog);

    // SELECT * 会被规划为 Projection(MaterializedViewScan)
    // 验证内部 MaterializedViewScan 的 schema
    if let LogicalPlan::Projection { input, .. } = &plan {
        if let LogicalPlan::MaterializedViewScan { name, schema, .. } = input.as_ref() {
            assert_eq!(name.name, "mv");
            // Schema 应有 3 列：id, amount, status
            assert_eq!(schema.columns.len(), 3);
            assert_eq!(schema.columns[0].name, "id");
            assert_eq!(schema.columns[1].name, "amount");
            assert_eq!(schema.columns[2].name, "status");
        } else {
            panic!("expected MaterializedViewScan inside Projection, got {input:?}");
        }
    } else {
        panic!("expected Projection wrapping MaterializedViewScan, got {plan:?}");
    }
}

// =====================================================================
//  EXPLAIN 格式化测试（3 条）
// =====================================================================

#[test]
fn explain_mv_query_shows_materialized_view_scan() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);

    let plan = plan_sql("SELECT * FROM mv", &catalog);
    let text = format_plan(&plan);

    assert!(
        text.contains("MaterializedViewScan: mv"),
        "expected 'MaterializedViewScan: mv' in output, got: {text}"
    );
}

#[test]
fn explain_source_table_query_shows_seq_scan() {
    let catalog = make_catalog_with_orders();

    let plan = plan_sql("SELECT * FROM orders", &catalog);
    let text = format_plan(&plan);

    assert!(
        text.contains("SeqScan: orders"),
        "expected 'SeqScan: orders' in output, got: {text}"
    );
    assert!(
        !text.contains("MaterializedViewScan"),
        "should not contain MaterializedViewScan for source table query"
    );
}

#[test]
fn explain_mv_query_with_filter_and_projection() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);

    let plan = plan_sql("SELECT id FROM mv WHERE id > 1", &catalog);
    let text = format_plan(&plan);

    // 应包含 MaterializedViewScan
    assert!(
        text.contains("MaterializedViewScan: mv"),
        "expected 'MaterializedViewScan: mv' in output, got: {text}"
    );
    // 应包含 Projection 和 Filter
    assert!(
        text.contains("Projection"),
        "expected 'Projection' in output, got: {text}"
    );
    assert!(
        text.contains("Filter"),
        "expected 'Filter' in output, got: {text}"
    );
}

// =====================================================================
//  源表路由不匹配测试（2 条）
// =====================================================================

#[test]
fn source_table_query_routes_to_seq_scan() {
    let catalog = make_catalog_with_orders();
    let orders_table = make_filled_orders_table();

    let mut executor = Executor::new();
    executor.register_table(&orders_table);

    let plan = plan_sql("SELECT * FROM orders", &catalog);

    // SELECT * 会被规划为 Projection(Scan)，验证内部是 Scan 而非 MaterializedViewScan
    let text = format_plan(&plan);
    assert!(
        text.contains("SeqScan: orders"),
        "expected SeqScan in plan, got: {text}"
    );
    assert!(
        !text.contains("MaterializedViewScan"),
        "should not contain MaterializedViewScan for source table query"
    );

    let result = executor.execute(&plan).unwrap();
    assert_eq!(result.len(), 3);
}

#[test]
fn source_table_query_with_filter_routes_to_seq_scan() {
    let catalog = make_catalog_with_orders();
    let orders_table = make_filled_orders_table();

    let mut executor = Executor::new();
    executor.register_table(&orders_table);

    let plan = plan_sql("SELECT * FROM orders WHERE status = 'paid'", &catalog);

    // 验证计划包含 Scan（源表），不包含 MaterializedViewScan
    let text = format_plan(&plan);
    assert!(text.contains("SeqScan: orders"));
    assert!(!text.contains("MaterializedViewScan"));

    let result = executor.execute(&plan).unwrap();
    // status='paid' 有 2 行（id=1 和 id=3）
    assert_eq!(result.len(), 2);
}

// =====================================================================
//  物化视图 + 投影/过滤测试（2 条）
// =====================================================================

#[test]
fn mv_query_with_projection_routes_correctly() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);

    let mv_store = make_filled_mv_store();
    let mut executor = Executor::new();
    executor.register_materialized_view_store("mv", &mv_store.storage);

    // SELECT id FROM mv → 应路由到 MV 存储
    let plan = plan_sql("SELECT id FROM mv", &catalog);
    let text = format_plan(&plan);
    assert!(text.contains("MaterializedViewScan: mv"));

    let result = executor.execute(&plan).unwrap();
    assert_eq!(result.len(), 3);
    // 每行只有 1 列（id）
    assert_eq!(result[0].len(), 1);
    assert_eq!(result[0][0], Value::Int64(1));
    assert_eq!(result[2][0], Value::Int64(3));
}

#[test]
fn mv_query_with_filter_routes_correctly() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);

    let mv_store = make_filled_mv_store();
    let mut executor = Executor::new();
    executor.register_materialized_view_store("mv", &mv_store.storage);

    // SELECT * FROM mv WHERE amount > 15 → 应路由到 MV 存储
    let plan = plan_sql("SELECT * FROM mv WHERE amount > 15", &catalog);
    let text = format_plan(&plan);
    assert!(text.contains("MaterializedViewScan: mv"));

    let result = executor.execute(&plan).unwrap();
    // amount > 15: id=2 (20.0) 和 id=3 (30.0)
    assert_eq!(result.len(), 2);
    assert_eq!(result[0][0], Value::Int64(2));
    assert_eq!(result[1][0], Value::Int64(3));
}

// =====================================================================
//  普通视图展开测试（2 条）
// =====================================================================

#[test]
fn regular_view_query_expands_to_source_table_scan() {
    let mut catalog = make_catalog_with_orders();
    setup_regular_view(&mut catalog);

    let orders_table = make_filled_orders_table();
    let mut executor = Executor::new();
    executor.register_table(&orders_table);

    // SELECT * FROM v → 应展开为 Projection + Scan(orders)
    let plan = plan_sql("SELECT * FROM v", &catalog);
    let text = format_plan(&plan);

    // 普通视图不路由到 MV 存储
    assert!(
        !text.contains("MaterializedViewScan"),
        "regular view should not route to MaterializedViewScan"
    );
    // 应展开为源表 Scan
    assert!(
        text.contains("SeqScan: orders"),
        "regular view should expand to source table scan, got: {text}"
    );

    let result = executor.execute(&plan).unwrap();
    // v 查询为 SELECT id, amount FROM orders → 2 列 × 3 行
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].len(), 2); // id, amount
    assert_eq!(result[0][0], Value::Int64(1));
    assert_eq!(result[0][1], Value::Float64(10.0));
}

#[test]
fn regular_view_definition_not_materialized() {
    let mut catalog = make_catalog_with_orders();
    setup_regular_view(&mut catalog);

    let view_name = TableName::new("v");
    let view_def = catalog.get_view(&view_name).unwrap();
    assert!(!view_def.materialized);
}

// =====================================================================
//  MV 未注册存储测试（1 条）
// =====================================================================

#[test]
fn mv_query_without_registered_storage_returns_error() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);

    // 不注册 MV 存储到执行器
    let executor = Executor::new();

    let plan = plan_sql("SELECT * FROM mv", &catalog);
    let result = executor.execute(&plan);

    // 应返回 TableNotFound 错误
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("mv") || err_msg.contains("not found"),
        "expected TableNotFound error containing 'mv', got: {err_msg}"
    );
}

// =====================================================================
//  空 MV 存储测试（1 条）
// =====================================================================

#[test]
fn mv_query_with_empty_storage_returns_zero_rows() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);

    // 创建空 MV 存储（不填充数据）
    let mv_store = MaterializedViewStore::new(
        "mv",
        vec![
            ("id", ColumnType::Int64),
            ("amount", ColumnType::Float64),
            ("status", ColumnType::Text),
        ],
    );

    let mut executor = Executor::new();
    executor.register_materialized_view_store("mv", &mv_store.storage);

    let plan = plan_sql("SELECT * FROM mv", &catalog);
    let result = executor.execute(&plan).unwrap();
    assert_eq!(result.len(), 0);
}

// =====================================================================
//  多物化视图测试（1 条）
// =====================================================================

#[test]
fn multiple_mvs_route_independently() {
    let mut catalog = make_catalog_with_orders();
    setup_materialized_view(&mut catalog);
    setup_materialized_view_mv2(&mut catalog);

    let mv_store = make_filled_mv_store();
    let mv2_store = make_filled_mv2_store();

    let mut executor = Executor::new();
    executor.register_materialized_view_store("mv", &mv_store.storage);
    executor.register_materialized_view_store("mv2", &mv2_store.storage);

    // 查询 mv → 3 列 × 3 行
    let plan_mv = plan_sql("SELECT * FROM mv", &catalog);
    let text_mv = format_plan(&plan_mv);
    assert!(
        text_mv.contains("MaterializedViewScan: mv"),
        "expected MaterializedViewScan for mv, got: {text_mv}"
    );
    let result_mv = executor.execute(&plan_mv).unwrap();
    assert_eq!(result_mv.len(), 3);
    assert_eq!(result_mv[0].len(), 3);

    // 查询 mv2 → 1 列 × 2 行
    let plan_mv2 = plan_sql("SELECT * FROM mv2", &catalog);
    let text_mv2 = format_plan(&plan_mv2);
    assert!(
        text_mv2.contains("MaterializedViewScan: mv2"),
        "expected MaterializedViewScan for mv2, got: {text_mv2}"
    );
    let result_mv2 = executor.execute(&plan_mv2).unwrap();
    assert_eq!(result_mv2.len(), 2);
    assert_eq!(result_mv2[0].len(), 1);
}

// =====================================================================
//  format_plan 多节点测试（2 条）
// =====================================================================

#[test]
fn format_plan_aggregate_node() {
    let catalog = make_catalog_with_orders();
    // SELECT status, COUNT(*) FROM orders GROUP BY status
    let plan = plan_sql(
        "SELECT status, COUNT(*) FROM orders GROUP BY status",
        &catalog,
    );
    let text = format_plan(&plan);

    assert!(
        text.contains("Aggregate"),
        "expected 'Aggregate' in output, got: {text}"
    );
    assert!(
        text.contains("SeqScan: orders"),
        "expected 'SeqScan: orders' in output, got: {text}"
    );
}

#[test]
fn format_plan_sort_and_limit_nodes() {
    let catalog = make_catalog_with_orders();
    let plan = plan_sql("SELECT * FROM orders ORDER BY id DESC LIMIT 2", &catalog);
    let text = format_plan(&plan);

    assert!(
        text.contains("Sort"),
        "expected 'Sort' in output, got: {text}"
    );
    assert!(
        text.contains("Limit"),
        "expected 'Limit' in output, got: {text}"
    );
    assert!(
        text.contains("SeqScan: orders"),
        "expected 'SeqScan: orders' in output, got: {text}"
    );
}
