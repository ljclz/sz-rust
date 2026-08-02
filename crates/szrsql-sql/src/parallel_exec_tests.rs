//! P2-18 查询并行化集成测试
//!
//! 验证 Executor 的并行执行路径：
//!
//! - 并行排序（ORDER BY）：行数 ≥ 阈值时并行计算排序键
//! - 并行 GROUP BY 聚合：分组数 ≥ 阈值时并行计算每组聚合值
//! - 阈值控制（`with_parallel_threshold(0)` 强制串行）
//! - 并行与串行结果一致性回归（大数据量）

use szrsql_types::value::{ColumnType, Value};

use crate::ast::TableName;
use crate::executor::{Executor, InMemoryTable};
use crate::plan::{InMemoryCatalog, Planner, TableSchema};

// ---------------------------------------------------------------------------
//  测试工具
// ---------------------------------------------------------------------------

/// 从 Value 中提取 i64（测试辅助，失败则 panic）
fn as_i64(v: &Value) -> i64 {
    if let Value::Int64(n) = v {
        *n
    } else {
        panic!("expected Int64, got {v:?}")
    }
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

/// 构建 catalog + 行存表，填充 N 行测试数据（用于 GROUP BY 并行测试）
///
/// 数据规律：
/// - id = i + 1
/// - group_key = i % group_count（控制分组数）
/// - amount = (i + 1) * 10
fn build_table_with_groups(
    row_count: usize,
    group_count: usize,
) -> (InMemoryCatalog, InMemoryTable) {
    let schema = TableSchema {
        name: TableName::new("sales"),
        columns: vec![
            crate::ast::ColumnDefinition::new("id", ColumnType::Int64),
            crate::ast::ColumnDefinition::new("group_key", ColumnType::Int64),
            crate::ast::ColumnDefinition::new("amount", ColumnType::Int64),
        ],
    };

    let mut table = InMemoryTable::new(schema.clone());
    for i in 0..row_count {
        table.insert(vec![
            Value::Int64((i + 1) as i64),
            Value::Int64((i % group_count) as i64),
            Value::Int64((i + 1) as i64 * 10),
        ]);
    }

    let mut catalog = InMemoryCatalog::new();
    catalog.add_table(schema);
    (catalog, table)
}

/// 构建一个简单双列行存表（用于排序测试）
fn build_two_col_table(name: &str, rows: &[(i64, i64)]) -> (InMemoryCatalog, InMemoryTable) {
    let schema = TableSchema {
        name: TableName::new(name),
        columns: vec![
            crate::ast::ColumnDefinition::new("a", ColumnType::Int64),
            crate::ast::ColumnDefinition::new("b", ColumnType::Int64),
        ],
    };
    let mut table = InMemoryTable::new(schema.clone());
    for &(a, b) in rows {
        table.insert(vec![Value::Int64(a), Value::Int64(b)]);
    }
    let mut catalog = InMemoryCatalog::new();
    catalog.add_table(schema);
    (catalog, table)
}

// ---------------------------------------------------------------------------
//  并行排序测试
// ---------------------------------------------------------------------------

#[test]
fn test_p2_18_parallel_sort_correctness() {
    // 验证：并行排序（行数 ≥ 阈值）结果与串行一致
    let data: Vec<(i64, i64)> = (0..15_000)
        .map(|i| (i as i64, (15_000 - i) as i64)) // val 逆序
        .collect();
    let (catalog, table) = build_two_col_table("nums", &data);

    let mut exec = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(10_000);
    exec.register_table(&table);

    let plan = plan_sql("SELECT a, b FROM nums ORDER BY b ASC", &catalog);
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), 15_000);
    // b 应按升序排列：1, 2, 3, ...
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row[1], Value::Int64((i + 1) as i64), "row {i}");
    }
}

#[test]
fn test_p2_18_parallel_sort_multi_key() {
    // 多列 ORDER BY 并行排序
    let data: Vec<(i64, i64)> = (0..12_000)
        .map(|i| ((i % 100) as i64, (100 - (i % 100)) as i64))
        .collect();
    let (catalog, table) = build_two_col_table("pairs", &data);

    let mut exec = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(10_000);
    exec.register_table(&table);

    let plan = plan_sql("SELECT a, b FROM pairs ORDER BY a ASC, b DESC", &catalog);
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), 12_000);
    // 验证排序：a 升序，同 a 内 b 降序
    let mut prev_a = -1i64;
    let mut prev_b = i64::MAX;
    for (i, row) in rows.iter().enumerate() {
        let a = as_i64(&row[0]);
        let b = as_i64(&row[1]);
        if a < prev_a {
            panic!("row {i}: a={a} < prev_a={prev_a}");
        }
        if a == prev_a && b > prev_b {
            panic!("row {i}: b={b} > prev_b={prev_b} within same a");
        }
        prev_a = a;
        prev_b = b;
    }
}

#[test]
fn test_p2_18_parallel_sort_below_threshold_serial() {
    // 行数 < 阈值 → 串行执行，结果正确
    let data: Vec<(i64, i64)> = (0..100).map(|i| ((100 - i) as i64, 0)).collect();
    let (catalog, table) = build_two_col_table("small", &data);

    let mut exec = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(10_000);
    exec.register_table(&table);

    let plan = plan_sql("SELECT a FROM small ORDER BY a ASC", &catalog);
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), 100);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row[0], Value::Int64((i + 1) as i64));
    }
}

// ---------------------------------------------------------------------------
//  并行 GROUP BY 聚合测试
// ---------------------------------------------------------------------------

#[test]
fn test_p2_18_parallel_group_by_count() {
    // 分组数 ≥ 阈值 → 并行计算每组 COUNT(*)
    let row_count = 100_000;
    let group_count = 15_000; // ≥ 默认阈值 10K
    let (catalog, table) = build_table_with_groups(row_count, group_count);

    let mut exec = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(10_000);
    exec.register_table(&table);

    let plan = plan_sql(
        "SELECT group_key, COUNT(*) FROM sales GROUP BY group_key",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), group_count);

    // 验证总数 = row_count
    let total: i64 = rows.iter().map(|r| as_i64(&r[1])).sum();
    assert_eq!(total, row_count as i64);
}

#[test]
fn test_p2_18_parallel_group_by_sum() {
    // 并行 SUM 聚合，验证结果与串行一致
    let row_count = 50_000;
    let group_count = 12_000; // ≥ 默认阈值
    let (catalog, table) = build_table_with_groups(row_count, group_count);

    let mut exec = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(10_000);
    exec.register_table(&table);

    let plan = plan_sql(
        "SELECT group_key, SUM(amount) FROM sales GROUP BY group_key ORDER BY group_key",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), group_count);

    // 验证每组 SUM：group_key = g 的行是 i where i % group_count == g
    // amount = (i+1) * 10
    for row in &rows {
        let g = as_i64(&row[0]);
        let sum_val = as_i64(&row[1]);

        // 计算期望值
        let mut expected: i64 = 0;
        let mut i = g as usize;
        while i < row_count {
            expected += (i + 1) as i64 * 10;
            i += group_count;
        }
        assert_eq!(
            sum_val, expected,
            "group_key={g}: expected SUM={expected}, got {sum_val}"
        );
    }
}

#[test]
fn test_p2_18_parallel_group_by_below_threshold_serial() {
    // 分组数 < 阈值 → 串行执行
    let row_count = 1000;
    let group_count = 5; // << 默认阈值
    let (catalog, table) = build_table_with_groups(row_count, group_count);

    let mut exec = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(10_000);
    exec.register_table(&table);

    let plan = plan_sql(
        "SELECT group_key, COUNT(*), SUM(amount) FROM sales GROUP BY group_key",
        &catalog,
    );
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), group_count);
    let total_count: i64 = rows.iter().map(|r| as_i64(&r[1])).sum();
    assert_eq!(total_count, row_count as i64);
}

// ---------------------------------------------------------------------------
//  阈值控制测试
// ---------------------------------------------------------------------------

#[test]
fn test_p2_18_parallel_threshold_zero_forces_serial() {
    // with_parallel_threshold(0) → 始终串行执行
    let data: Vec<(i64, i64)> = (0..20_000)
        .map(|i| (i as i64, (20_000 - i) as i64))
        .collect();
    let (catalog, table) = build_two_col_table("big", &data);

    let mut exec = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(0); // 强制串行
    exec.register_table(&table);

    let plan = plan_sql("SELECT a, b FROM big ORDER BY b ASC", &catalog);
    let rows = exec.execute(&plan).unwrap();

    assert_eq!(rows.len(), 20_000);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row[1], Value::Int64((i + 1) as i64));
    }
}

// ---------------------------------------------------------------------------
//  并行 vs 串行一致性回归测试
// ---------------------------------------------------------------------------

#[test]
fn test_p2_18_parallel_vs_serial_sort_consistency() {
    // 同一大数据集，分别用并行和串行执行排序，结果应完全一致
    let data: Vec<(i64, i64)> = (0..25_000)
        .map(|i| ((i * 7 + 13) as i64 % 10_000, i as i64))
        .collect();
    let (catalog, table) = build_two_col_table("data", &data);

    let plan = plan_sql("SELECT a, b FROM data ORDER BY a ASC, b ASC", &catalog);

    // 并行执行
    let mut exec_parallel = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(10_000);
    exec_parallel.register_table(&table);
    let parallel_rows = exec_parallel.execute(&plan).unwrap();

    // 串行执行
    let mut exec_serial = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(0);
    exec_serial.register_table(&table);
    let serial_rows = exec_serial.execute(&plan).unwrap();

    assert_eq!(parallel_rows.len(), serial_rows.len(), "row count mismatch");
    for (i, (p, s)) in parallel_rows.iter().zip(serial_rows.iter()).enumerate() {
        assert_eq!(p, s, "row {i} mismatch between parallel and serial");
    }
}

#[test]
fn test_p2_18_parallel_vs_serial_group_by_consistency() {
    // 同一大数据集，分别用并行和串行执行 GROUP BY，结果应完全一致
    let row_count = 80_000;
    let group_count = 12_000; // ≥ 默认阈值
    let (catalog, table) = build_table_with_groups(row_count, group_count);

    let sql = "SELECT group_key, COUNT(*), SUM(amount), AVG(amount) FROM sales GROUP BY group_key ORDER BY group_key";

    // 并行执行
    let mut exec_parallel = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(10_000);
    exec_parallel.register_table(&table);
    let parallel_rows = exec_parallel.execute(&plan_sql(sql, &catalog)).unwrap();

    // 串行执行
    let mut exec_serial = Executor::new()
        .with_catalog(&catalog)
        .with_parallel_threshold(0);
    exec_serial.register_table(&table);
    let serial_rows = exec_serial.execute(&plan_sql(sql, &catalog)).unwrap();

    assert_eq!(
        parallel_rows.len(),
        serial_rows.len(),
        "group count mismatch"
    );
    for (i, (p, s)) in parallel_rows.iter().zip(serial_rows.iter()).enumerate() {
        assert_eq!(p.len(), s.len(), "col count mismatch at group {i}");
        for (j, (pv, sv)) in p.iter().zip(s.iter()).enumerate() {
            // AVG 是 Float64，允许浮点误差
            if let (Value::Float64(a), Value::Float64(b)) = (pv, sv) {
                assert!(
                    (a - b).abs() < 1e-9,
                    "group {i} col {j}: parallel={a}, serial={b}"
                );
            } else {
                assert_eq!(pv, sv, "group {i} col {j} mismatch");
            }
        }
    }
}
