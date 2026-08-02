//! P2-15 向量化执行引擎集成测试
//!
//! 验证列存扫描（`ColumnarScan`）与 batch-mode SIMD 聚合快速路径：
//!
//! - 列存表注册后，`ColumnarScan` 节点正确物化为行
//! - 无 GROUP BY 聚合走 `ColumnarTable::aggregate()` 快速路径
//! - 结果与行存路径一致（正确性回归）
//! - 大数据量（>100K 行）下快速路径正常工作

use szrsql_storage::columnar::{
    ColumnSchema, ColumnSpec, ColumnVector, ColumnarBatch, ColumnarTable, ColumnarType, NullBitmap,
};
use szrsql_types::value::{ColumnType, Value};

use crate::ast::TableName;
use crate::executor::{Executor, InMemoryTable};
use crate::plan::{InMemoryCatalog, Planner, TableSchema};

// ---------------------------------------------------------------------------
//  测试工具
// ---------------------------------------------------------------------------

/// 构造列存测试 schema：id INT, value INT, score FLOAT, name TEXT, active BOOL
fn make_columnar_schema() -> ColumnSchema {
    ColumnSchema::from_columns(vec![
        ColumnSpec::new("id", ColumnarType::Int64),
        ColumnSpec::new("value", ColumnarType::Int64),
        ColumnSpec::new("score", ColumnarType::Float64),
        ColumnSpec::new("name", ColumnarType::Text),
        ColumnSpec::new("active", ColumnarType::Bool),
    ])
}

/// 构造对应的 SQL 层 TableSchema（用于 catalog 注册）
fn make_table_schema() -> TableSchema {
    TableSchema {
        name: TableName::new("sensor_data"),
        columns: vec![
            crate::ast::ColumnDefinition::new("id", ColumnType::Int64),
            crate::ast::ColumnDefinition::new("value", ColumnType::Int64),
            crate::ast::ColumnDefinition::new("score", ColumnType::Float64),
            crate::ast::ColumnDefinition::new("name", ColumnType::Text),
            crate::ast::ColumnDefinition::new("active", ColumnType::Text),
        ],
    }
}

/// 构建 catalog + 列存表，填充 N 行测试数据
///
/// 数据规律（便于断言）：
/// - id = i + 1
/// - value = (i + 1) * 10
/// - score = (i + 1) as f64 * 1.5
/// - name = "sensor_{i+1}"
/// - active = i % 2 == 0
fn build_catalog_and_columnar_table(row_count: usize) -> (InMemoryCatalog, ColumnarTable) {
    let mut catalog = InMemoryCatalog::new();
    let schema = make_table_schema();
    catalog.add_table(schema.clone());

    let col_schema = make_columnar_schema();
    let mut col_table = ColumnarTable::new("sensor_data", col_schema.clone());

    // 按 DEFAULT_BATCH_SIZE 分批填充
    let batch_size = szrsql_storage::columnar::DEFAULT_BATCH_SIZE;
    for batch_start in (0..row_count).step_by(batch_size) {
        let end = (batch_start + batch_size).min(row_count);
        let n = end - batch_start;

        let ids: Vec<i64> = (batch_start..end).map(|i| (i + 1) as i64).collect();
        let values: Vec<i64> = (batch_start..end).map(|i| (i + 1) as i64 * 10).collect();
        let scores: Vec<f64> = (batch_start..end).map(|i| (i + 1) as f64 * 1.5).collect();
        let names: Vec<String> = (batch_start..end)
            .map(|i| format!("sensor_{}", i + 1))
            .collect();
        let actives: Vec<bool> = (batch_start..end).map(|i| i % 2 == 0).collect();

        let mut batch = ColumnarBatch::new(col_schema.clone());
        batch
            .set_column(0, ColumnVector::from_int64_slice(&ids))
            .unwrap();
        batch
            .set_column(1, ColumnVector::from_int64_slice(&values))
            .unwrap();
        batch
            .set_column(2, ColumnVector::from_float64_slice(&scores))
            .unwrap();
        batch
            .set_column(
                3,
                ColumnVector::Text {
                    data: names,
                    null_bitmap: NullBitmap::new(n),
                },
            )
            .unwrap();
        batch
            .set_column(
                4,
                ColumnVector::Bool {
                    data: actives,
                    null_bitmap: NullBitmap::new(n),
                },
            )
            .unwrap();
        batch.set_row_count(n);
        col_table.append_batch(batch).unwrap();
    }

    (catalog, col_table)
}

/// 用 planner 解析 SQL 并生成 LogicalPlan
fn plan_sql(sql: &str, catalog: &InMemoryCatalog) -> crate::plan::LogicalPlan {
    use crate::parser::parse_sql;
    let stmt = parse_sql(sql).unwrap_or_else(|e| panic!("parse failed for SQL: {sql}\n{e:?}"));
    let stmt = stmt
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("empty statement: {sql}"));
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmt)
        .unwrap_or_else(|e| panic!("plan failed for SQL: {sql}\nerror: {e:?}"))
}

// ---------------------------------------------------------------------------
//  列存扫描基础测试
// ---------------------------------------------------------------------------

#[test]
fn test_p2_15_columnar_scan_small_batch() {
    // 小批量（< DEFAULT_BATCH_SIZE）：单 batch 物化
    let (catalog, col_table) = build_catalog_and_columnar_table(500);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    // 直接执行 ColumnarScan（绕过 planner，手动构造计划）
    let schema = make_table_schema();
    let plan = crate::plan::LogicalPlan::ColumnarScan {
        table: TableName::new("sensor_data"),
        alias: None,
        schema,
    };

    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 500, "columnar scan should return 500 rows");

    // 验证数据规律
    assert_eq!(rows[0][0], Value::Int64(1));
    assert_eq!(rows[0][1], Value::Int64(10));
    if let Value::Float64(v) = rows[0][2] {
        assert!((v - 1.5).abs() < 1e-9);
    } else {
        panic!("expected Float64, got {:?}", rows[0][2]);
    }
    assert_eq!(rows[499][0], Value::Int64(500));
    assert_eq!(rows[499][1], Value::Int64(5000));
}

#[test]
fn test_p2_15_columnar_scan_multi_batch() {
    // 多 batch（> DEFAULT_BATCH_SIZE）：验证跨 batch 合并
    let (catalog, col_table) = build_catalog_and_columnar_table(5000);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    let schema = make_table_schema();
    let plan = crate::plan::LogicalPlan::ColumnarScan {
        table: TableName::new("sensor_data"),
        alias: None,
        schema,
    };

    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 5000, "columnar scan should return 5000 rows");
    // 最后一行
    assert_eq!(rows[4999][0], Value::Int64(5000));
    assert_eq!(rows[4999][1], Value::Int64(50000));
}

#[test]
fn test_p2_15_columnar_scan_empty_table() {
    // 空列存表
    let schema = make_columnar_schema();
    let col_table = ColumnarTable::new("empty_table", schema);

    let mut catalog = InMemoryCatalog::new();
    catalog.add_table(make_table_schema());

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("empty_table", &col_table);

    let plan = crate::plan::LogicalPlan::ColumnarScan {
        table: TableName::new("empty_table"),
        alias: None,
        schema: make_table_schema(),
    };

    let rows = exec.execute(&plan).unwrap();
    assert!(rows.is_empty(), "empty columnar table should return 0 rows");
}

#[test]
fn test_p2_15_columnar_scan_with_projection_and_filter() {
    // ColumnarScan 上叠加 Projection + Filter（走行存表达式求值）
    let (catalog, col_table) = build_catalog_and_columnar_table(100);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    // SELECT id, value FROM sensor_data WHERE id <= 10
    let plan = plan_sql("SELECT id, value FROM sensor_data WHERE id <= 10", &catalog);
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), 10);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row[0], Value::Int64((i + 1) as i64));
        assert_eq!(row[1], Value::Int64((i + 1) as i64 * 10));
    }
}

// ---------------------------------------------------------------------------
//  列存聚合快速路径测试
// ---------------------------------------------------------------------------

#[test]
fn test_p2_15_columnar_aggregate_count_star() {
    // COUNT(*) 快速路径 → table.row_count()
    let (catalog, col_table) = build_catalog_and_columnar_table(3000);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    let plan = plan_sql("SELECT COUNT(*) FROM sensor_data", &catalog);
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(3000));
}

#[test]
fn test_p2_15_columnar_aggregate_sum_int64() {
    // SUM(value) 快速路径 → ColumnarTable::aggregate(Sum, "value")
    let (catalog, col_table) = build_catalog_and_columnar_table(1000);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    let plan = plan_sql("SELECT SUM(value) FROM sensor_data", &catalog);
    let rows = exec.execute(&plan).unwrap();

    // value = (i+1) * 10, i=0..999 → sum = 10 * (1+2+...+1000) = 10 * 500500 = 5005000
    assert_eq!(rows[0][0], Value::Int64(5_005_000));
}

#[test]
fn test_p2_15_columnar_aggregate_avg_int64() {
    // AVG(id) 快速路径 → ColumnarTable::aggregate(Avg, "id")
    let (catalog, col_table) = build_catalog_and_columnar_table(100);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    let plan = plan_sql("SELECT AVG(id) FROM sensor_data", &catalog);
    let rows = exec.execute(&plan).unwrap();

    // AVG(1..100) = 50.5
    if let Value::Float64(v) = rows[0][0] {
        assert!((v - 50.5).abs() < 1e-9, "expected 50.5, got {v}");
    } else {
        panic!("expected Float64, got {:?}", rows[0][0]);
    }
}

#[test]
fn test_p2_15_columnar_aggregate_min_max() {
    // MIN / MAX 快速路径
    let (catalog, col_table) = build_catalog_and_columnar_table(5000);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    let plan = plan_sql(
        "SELECT MIN(id), MAX(id), MIN(value), MAX(value) FROM sensor_data",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(1)); // MIN(id)
    assert_eq!(rows[0][1], Value::Int64(5000)); // MAX(id)
    assert_eq!(rows[0][2], Value::Int64(10)); // MIN(value) = 1*10
    assert_eq!(rows[0][3], Value::Int64(50000)); // MAX(value) = 5000*10
}

#[test]
fn test_p2_15_columnar_aggregate_count_column() {
    // COUNT(id) 快速路径
    let (catalog, col_table) = build_catalog_and_columnar_table(2500);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    let plan = plan_sql("SELECT COUNT(id) FROM sensor_data", &catalog);
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows[0][0], Value::Int64(2500));
}

#[test]
fn test_p2_15_columnar_aggregate_multi_aggs_single_query() {
    // 多聚合函数单次查询：COUNT(*) + SUM(value) + AVG(score)
    let (catalog, col_table) = build_catalog_and_columnar_table(100);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    let plan = plan_sql(
        "SELECT COUNT(*), SUM(value), AVG(score) FROM sensor_data",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(100)); // COUNT(*)
    assert_eq!(rows[0][1], Value::Int64(50_500)); // SUM(value) = 10 * 5050
                                                  // AVG(score) = AVG(1.5, 3.0, ..., 150.0) = (1.5 + 150.0) / 2 = 75.75
    if let Value::Float64(v) = rows[0][2] {
        assert!((v - 75.75).abs() < 1e-6, "expected 75.75, got {v}");
    } else {
        panic!("expected Float64, got {:?}", rows[0][2]);
    }
}

#[test]
fn test_p2_15_columnar_aggregate_large_scale() {
    // 大规模（>100K 行）快速路径正确性
    let (catalog, col_table) = build_catalog_and_columnar_table(200_000);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    let plan = plan_sql(
        "SELECT COUNT(*), SUM(id), AVG(id) FROM sensor_data",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(200_000)); // COUNT(*)
                                                   // SUM(1..200000) = 200000 * 200001 / 2 = 20_000_100_000
    assert_eq!(rows[0][1], Value::Int64(20_000_100_000));
    // AVG(1..200000) = (1 + 200000) / 2 = 100000.5
    if let Value::Float64(v) = rows[0][2] {
        assert!((v - 100_000.5).abs() < 1e-6, "expected 100000.5, got {v}");
    } else {
        panic!("expected Float64, got {:?}", rows[0][2]);
    }
}

// ---------------------------------------------------------------------------
//  快速路径退化测试（应回退到行存路径）
// ---------------------------------------------------------------------------

#[test]
fn test_p2_15_columnar_aggregate_with_group_by_fallback() {
    // GROUP BY 不走快速路径，退化为行存聚合（仍通过列存扫描获取行）
    let (catalog, col_table) = build_catalog_and_columnar_table(100);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    // SELECT active, COUNT(*) FROM sensor_data GROUP BY active
    let plan = plan_sql(
        "SELECT active, COUNT(*) FROM sensor_data GROUP BY active",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();

    // active = i % 2 == 0, i=0..99 → 50 true, 50 false
    assert_eq!(rows.len(), 2);
    let total: i64 = rows
        .iter()
        .map(|r| {
            if let Value::Int64(n) = r[1] {
                n
            } else {
                panic!("expected Int64, got {:?}", r[1]);
            }
        })
        .sum();
    assert_eq!(total, 100);
}

#[test]
fn test_p2_15_columnar_aggregate_distinct_fallback() {
    // COUNT(DISTINCT ...) 不走快速路径
    let (catalog, col_table) = build_catalog_and_columnar_table(100);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    // 所有 id 唯一 → COUNT(DISTINCT id) = 100
    let plan = plan_sql("SELECT COUNT(DISTINCT id) FROM sensor_data", &catalog);
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows[0][0], Value::Int64(100));
}

#[test]
fn test_p2_15_columnar_aggregate_having_fallback() {
    // HAVING 不走快速路径
    let (catalog, col_table) = build_catalog_and_columnar_table(100);

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_columnar_table("sensor_data", &col_table);

    // COUNT(*) = 100 > 50 → 1 行
    let plan = plan_sql(
        "SELECT COUNT(*) FROM sensor_data HAVING COUNT(*) > 50",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int64(100));
}

// ---------------------------------------------------------------------------
//  列存 + 行存一致性回归测试
// ---------------------------------------------------------------------------

#[test]
fn test_p2_15_columnar_vs_row_aggregate_consistency() {
    // 同一份数据同时注册到列存表和行存表，
    // 验证聚合结果一致。
    let row_count = 10_000;
    let (catalog, col_table) = build_catalog_and_columnar_table(row_count);

    // 构建对应的行存表
    let schema = make_table_schema();
    let mut row_table = InMemoryTable::new(schema.clone());
    for i in 0..row_count {
        let row = vec![
            Value::Int64((i + 1) as i64),
            Value::Int64((i + 1) as i64 * 10),
            Value::Float64((i + 1) as f64 * 1.5),
            Value::Text(format!("sensor_{i}")),
            Value::Text(if i % 2 == 0 {
                "true".to_string()
            } else {
                "false".to_string()
            }),
        ];
        row_table.insert(row);
    }

    let mut exec = Executor::new().with_catalog(&catalog);
    exec.register_table(&row_table);
    exec.register_columnar_table("sensor_data", &col_table);

    let queries = [
        "SELECT COUNT(*) FROM sensor_data",
        "SELECT SUM(id) FROM sensor_data",
        "SELECT AVG(value) FROM sensor_data",
        "SELECT MIN(id), MAX(id), MIN(value), MAX(value) FROM sensor_data",
        "SELECT COUNT(*), SUM(id), AVG(score) FROM sensor_data",
    ];

    // 空列存表用于临时取消注册（替换为无数据的空表）
    let empty_col_table = build_empty_columnar_table();

    for sql in queries {
        let plan = plan_sql(sql, &catalog);
        let col_rows = exec.execute(&plan).unwrap();

        // 取消列存注册，走纯行存路径
        exec.register_columnar_table("sensor_data", &empty_col_table);
        let row_rows = exec.execute(&plan).unwrap();

        // 恢复列存注册
        exec.register_columnar_table("sensor_data", &col_table);

        assert_eq!(
            col_rows.len(),
            row_rows.len(),
            "row count mismatch for query: {sql}"
        );
        for (col_row, row_row) in col_rows.iter().zip(row_rows.iter()) {
            assert_eq!(
                col_row.len(),
                row_row.len(),
                "col count mismatch for query: {sql}"
            );
            for (c, r) in col_row.iter().zip(row_row.iter()) {
                assert_eq!(c, r, "value mismatch for query: {sql}");
            }
        }
    }
}

fn build_empty_columnar_table() -> ColumnarTable {
    ColumnarTable::new("sensor_data", make_columnar_schema())
}
