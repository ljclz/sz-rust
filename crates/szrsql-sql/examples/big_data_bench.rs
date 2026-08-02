//! SzRSQL 大数据量基准测试
//!
//! 测试 SzRSQL 内存执行器在不同数据规模下的性能表现。
//! 测试维度：
//! 1. INSERT 吞吐（rows/sec）
//! 2. SELECT 全表扫描延迟
//! 3. SELECT + WHERE 过滤延迟
//! 4. SELECT + ORDER BY 延迟
//! 5. DELETE 吞吐
//! 6. UPDATE 吞吐
//!
//! 数据规模：1K / 10K / 100K / 1M / 10M 行
//!
//! 用法：cargo run -p szrsql-sql --example big_data_bench --release
//!
//! 注意：10M 行测试会占用约 2GB 内存。如需跳过 10M 测试，
//! 设置环境变量 `SZRSQL_BENCH_SKIP_10M=1`。

use std::time::Instant;

use szrsql_sql::executor::{Executor, InMemoryTable, Row};
use szrsql_sql::parser::parse_sql;
use szrsql_sql::plan::{Catalog, InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::{ColumnType, Value};

/// 测试数据规模（1K / 10K / 100K / 1M / 10M）
const SIZES: &[usize] = &[1_000, 10_000, 100_000, 1_000_000, 10_000_000];

/// 跳过 10M 行测试的环境变量名（避免内存不足）
const ENV_SKIP_10M: &str = "SZRSQL_BENCH_SKIP_10M";

/// 生成 N 行测试数据
fn generate_rows(n: usize) -> Vec<Row> {
    (0..n)
        .map(|i| {
            vec![
                Value::Int64(i as i64),
                Value::Text(format!("user_{i}")),
                Value::Int64(20 + (i % 50) as i64),
                Value::Text(format!("user_{i}@example.com")),
            ]
        })
        .collect()
}

/// 构建测试表
fn make_bench_table() -> InMemoryTable {
    InMemoryTable::with_columns(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
            ("email", ColumnType::Text),
        ],
    )
}

/// 构建带 users 表的 catalog
fn make_catalog() -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(
        "users",
        vec![
            ("id", ColumnType::Int64),
            ("name", ColumnType::Text),
            ("age", ColumnType::Int64),
            ("email", ColumnType::Text),
        ],
    );
    catalog
}

/// SQL → AST → LogicalPlan
fn plan_sql(sql: &str, catalog: &dyn Catalog) -> LogicalPlan {
    let stmts = parse_sql(sql).expect("parse failed");
    assert_eq!(stmts.len(), 1, "expected exactly 1 statement");
    let planner = Planner::new(catalog);
    planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .expect("plan failed")
}

fn fmt_duration(secs: f64) -> String {
    if secs < 0.001 {
        format!("{:.2} μs", secs * 1_000_000.0)
    } else if secs < 1.0 {
        format!("{:.2} ms", secs * 1_000.0)
    } else {
        format!("{:.2} s", secs)
    }
}

fn fmt_throughput(rows: usize, secs: f64) -> String {
    if secs == 0.0 {
        return "N/A".to_string();
    }
    let rps = rows as f64 / secs;
    if rps >= 1_000_000.0 {
        format!("{:.2} M rows/s", rps / 1_000_000.0)
    } else if rps >= 1_000.0 {
        format!("{:.2} K rows/s", rps / 1_000.0)
    } else {
        format!("{:.2} rows/s", rps)
    }
}

/// 测试 INSERT 吞吐
fn bench_insert(n: usize) -> (f64, usize) {
    let rows = generate_rows(n);
    let start = Instant::now();
    let mut table = make_bench_table();
    let inserted = table.bulk_insert(rows);
    let elapsed = start.elapsed().as_secs_f64();
    (elapsed, inserted)
}

/// 测试 SELECT * 全表扫描
fn bench_select_all(n: usize) -> f64 {
    let rows = generate_rows(n);
    let mut table = make_bench_table();
    table.bulk_insert(rows);
    let catalog = make_catalog();
    let plan = plan_sql("SELECT * FROM users", &catalog);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let start = Instant::now();
    let result = exec.execute(&plan).expect("execute failed");
    let elapsed = start.elapsed().as_secs_f64();
    assert_eq!(result.len(), n, "SELECT * should return all rows");
    elapsed
}

/// 测试 SELECT WHERE 过滤
fn bench_select_where(n: usize) -> f64 {
    let rows = generate_rows(n);
    let mut table = make_bench_table();
    table.bulk_insert(rows);
    let catalog = make_catalog();
    let plan = plan_sql("SELECT * FROM users WHERE age > 50", &catalog);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let start = Instant::now();
    let result = exec.execute(&plan).expect("execute failed");
    let elapsed = start.elapsed().as_secs_f64();
    let expected = (0..n).filter(|i| 20 + (i % 50) > 50).count();
    assert_eq!(result.len(), expected, "WHERE filter result count mismatch");
    elapsed
}

/// 测试 SELECT ORDER BY
fn bench_select_order_by(n: usize) -> f64 {
    let rows = generate_rows(n);
    let mut table = make_bench_table();
    table.bulk_insert(rows);
    let catalog = make_catalog();
    let plan = plan_sql("SELECT * FROM users ORDER BY age DESC, id ASC", &catalog);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let start = Instant::now();
    let result = exec.execute(&plan).expect("execute failed");
    let elapsed = start.elapsed().as_secs_f64();
    assert_eq!(result.len(), n, "ORDER BY should return all rows");
    elapsed
}

/// 测试 SELECT COUNT(*)
fn bench_select_count(n: usize) -> f64 {
    let rows = generate_rows(n);
    let mut table = make_bench_table();
    table.bulk_insert(rows);
    let catalog = make_catalog();
    let plan = plan_sql("SELECT COUNT(*) FROM users", &catalog);
    let mut exec = Executor::new();
    exec.register_table(&table);
    let start = Instant::now();
    let result = exec.execute(&plan).expect("execute failed");
    let elapsed = start.elapsed().as_secs_f64();
    assert_eq!(result.len(), 1, "COUNT should return 1 row");
    elapsed
}

/// 测试 UPDATE 全表吞吐
fn bench_update(n: usize) -> f64 {
    let rows = generate_rows(n);
    let mut table = make_bench_table();
    table.bulk_insert(rows);
    let catalog = make_catalog();
    let plan = plan_sql("UPDATE users SET age = age + 1", &catalog);
    let exec = Executor::new();
    let start = Instant::now();
    exec.execute_update(&plan, &mut table)
        .expect("execute failed");
    start.elapsed().as_secs_f64()
}

/// 测试 DELETE 全表吞吐
fn bench_delete(n: usize) -> f64 {
    let rows = generate_rows(n);
    let mut table = make_bench_table();
    table.bulk_insert(rows);
    let catalog = make_catalog();
    let plan = plan_sql("DELETE FROM users", &catalog);
    let exec = Executor::new();
    let start = Instant::now();
    exec.execute_delete(&plan, &mut table)
        .expect("execute failed");
    start.elapsed().as_secs_f64()
}

/// 判断是否应跳过指定规模（用于避免 10M 行测试在低内存机器上 OOM）
fn should_skip(size: usize) -> bool {
    if size == 10_000_000 {
        return std::env::var(ENV_SKIP_10M)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    }
    false
}

fn main() {
    println!("========================================");
    println!("SzRSQL 大数据量基准测试");
    println!("========================================");
    println!();

    // 读取操作表
    println!("[读取操作]");
    println!(
        "{:<12} {:<12} {:<12} {:<12} {:<12}",
        "数据规模", "SELECT *", "WHERE", "ORDER BY", "COUNT(*)"
    );
    println!("{:-<70}", "");

    for &size in SIZES {
        if should_skip(size) {
            println!(
                "{:<12} {:<12} {:<12} {:<12} {:<12}",
                format!("{} 行", format_num(size)),
                "SKIPPED",
                "-",
                "-",
                "-"
            );
            println!();
            continue;
        }
        let select_all_t = bench_select_all(size);
        let where_t = bench_select_where(size);
        let order_t = bench_select_order_by(size);
        let count_t = bench_select_count(size);

        println!(
            "{:<12} {:<12} {:<12} {:<12} {:<12}",
            format!("{} 行", format_num(size)),
            fmt_duration(select_all_t),
            fmt_duration(where_t),
            fmt_duration(order_t),
            fmt_duration(count_t),
        );

        println!(
            "  吞吐:    {}          {}          {}          {}",
            fmt_throughput(size, select_all_t),
            fmt_throughput(size, where_t),
            fmt_throughput(size, order_t),
            fmt_throughput(size, count_t),
        );
        println!();
    }

    // 写入操作表
    println!("[写入操作]");
    println!(
        "{:<12} {:<12} {:<12} {:<12}",
        "数据规模", "INSERT", "UPDATE", "DELETE"
    );
    println!("{:-<60}", "");

    for &size in SIZES {
        if should_skip(size) {
            println!(
                "{:<12} {:<12} {:<12} {:<12}",
                format!("{} 行", format_num(size)),
                "SKIPPED",
                "-",
                "-"
            );
            println!();
            continue;
        }
        let (insert_t, inserted) = bench_insert(size);
        let update_t = bench_update(size);
        let delete_t = bench_delete(size);

        println!(
            "{:<12} {:<12} {:<12} {:<12}",
            format!("{} 行", format_num(size)),
            fmt_duration(insert_t),
            fmt_duration(update_t),
            fmt_duration(delete_t),
        );

        println!(
            "  吞吐:    {}          {}          {}",
            fmt_throughput(inserted, insert_t),
            fmt_throughput(size, update_t),
            fmt_throughput(size, delete_t),
        );
        println!();
    }

    println!("========================================");
    println!("测试完成");
    println!("========================================");
}

fn format_num(n: usize) -> String {
    if n >= 1_000_000 {
        format!("{}M", n / 1_000_000)
    } else if n >= 1_000 {
        format!("{}K", n / 1_000)
    } else {
        n.to_string()
    }
}
