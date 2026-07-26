//! PG 18 性能对标 — 简化版 TPC-B 工作负载
//!
//! 对应 `docs/全面排查汇总报告.md` P0-2 任务。
//!
//! # 工作负载
//!
//! 1. **INSERT 吞吐**：N 行顺序插入（N=1K/10K/100K）
//! 2. **SELECT 延迟**：全表扫描、WHERE 等值、COUNT(*)
//! 3. **UPDATE 吞吐**：N 行随机更新
//! 4. **DELETE 吞吐**：N 行顺序删除
//!
//! # 指标
//!
//! - TPS（每秒事务数）
//! - 平均延迟（ms）
//! - P95/P99 延迟（ms）
//! - 相对 PG 18 的性能比
//!
//! # 用法
//!
//! ```bash
//! cargo test -p szrsql-shadow --test bench_pgbench -- --nocapture --ignored --test-threads=1
//! ```

use std::time::{Duration, Instant};

use szrsql_shadow::{
    recorder::{Recorder, TrafficEntry},
    replay::{ReplayConfig, ShadowReplay},
    report::ShadowReport,
};
use szrsql_types::value::ColumnType;
use tempfile::NamedTempFile;

/// 尝试连接 PG 18，失败则跳过测试
fn try_pg_url() -> Option<String> {
    let url = "postgresql://postgres:postgres@127.0.0.1:5432/postgres";
    match postgres::Client::connect(url, postgres::NoTls) {
        Ok(_) => Some(url.to_string()),
        Err(e) => {
            eprintln!("[bench] 跳过：无法连接 PG 18 ({e})");
            None
        }
    }
}

/// 生成 INSERT 流量
fn gen_insert_entries(n: usize) -> Vec<TrafficEntry> {
    (1..=n)
        .map(|i| {
            TrafficEntry::new(
                "bench",
                format!("INSERT INTO t (id, name) VALUES ({i}, 'name_{i}')"),
            )
        })
        .collect()
}

/// 生成 SELECT 流量
fn gen_select_entries(n: usize) -> Vec<TrafficEntry> {
    let mut entries = Vec::with_capacity(n);
    entries.push(TrafficEntry::new("bench", "SELECT COUNT(*) FROM t"));
    entries.push(TrafficEntry::new(
        "bench",
        "SELECT id, name FROM t ORDER BY id",
    ));
    for i in 1..=n.min(100) {
        entries.push(TrafficEntry::new(
            "bench",
            format!("SELECT id, name FROM t WHERE id = {i}"),
        ));
    }
    entries
}

/// 生成 UPDATE 流量
fn gen_update_entries(n: usize) -> Vec<TrafficEntry> {
    (1..=n)
        .map(|i| {
            TrafficEntry::new(
                "bench",
                format!("UPDATE t SET name = 'updated_{i}' WHERE id = {i}"),
            )
        })
        .collect()
}

/// 生成 DELETE 流量
fn gen_delete_entries(n: usize) -> Vec<TrafficEntry> {
    (1..=n)
        .map(|i| TrafficEntry::new("bench", format!("DELETE FROM t WHERE id = {i}")))
        .collect()
}

/// 运行基准测试（szrsql + PG 18 差分执行），返回报告
fn run_bench(
    pg_url: &str,
    schema_suffix: &str,
    entries: Vec<TrafficEntry>,
) -> ShadowReport {
    let jsonl_file = NamedTempFile::new().unwrap();
    Recorder::save_to_jsonl(&entries, jsonl_file.path()).unwrap();

    let config = ReplayConfig {
        pg_url: pg_url.to_string(),
        pg_schema: format!("szrsql_bench_{schema_suffix}"),
        skip_sz_errors: true,
    };
    let replay = ShadowReplay::new(config);
    let columns = vec![
        ("id", ColumnType::Int64),
        ("name", ColumnType::Text),
    ];
    let results = replay
        .replay_from_jsonl(jsonl_file.path(), "t", columns)
        .expect("replay failed");
    ShadowReport::from_results(&results)
}

/// 计算 TPS（每秒事务数）
fn calc_tps(count: usize, duration: Duration) -> f64 {
    let secs = duration.as_secs_f64();
    if secs > 0.0 {
        count as f64 / secs
    } else {
        0.0
    }
}

// =====================================================================
//  基准测试用例
// =====================================================================

/// 测试 1：1K 行 INSERT 吞吐对标
#[test]
#[ignore = "性能测试默认跳过，使用 --ignored 运行"]
fn bench_insert_1k() {
    let pg_url = match try_pg_url() {
        Some(u) => u,
        None => return,
    };

    let entries = gen_insert_entries(1_000);
    let start = Instant::now();
    let report = run_bench(&pg_url, "insert_1k", entries);
    let duration = start.elapsed();

    println!("\n=== INSERT 1K 吞吐测试 ===");
    println!("{}", report.to_markdown());
    println!("总耗时：{:.2}s", duration.as_secs_f64());
    println!(
        "PG 18 TPS：{:.0}",
        calc_tps(report.total, Duration::from_secs_f64(report.pg_p50_ms / 1000.0 * report.total as f64))
    );

    // 验证：至少 90% 应匹配
    assert!(
        report.match_rate >= 0.9,
        "匹配率应 ≥ 90%：{}",
        report.match_rate
    );
    assert_eq!(report.pg_errors, 0, "PG 18 不应有错误");
}

/// 测试 2：10K 行 INSERT 吞吐对标
#[test]
#[ignore = "性能测试默认跳过，使用 --ignored 运行"]
fn bench_insert_10k() {
    let pg_url = match try_pg_url() {
        Some(u) => u,
        None => return,
    };

    let entries = gen_insert_entries(10_000);
    let report = run_bench(&pg_url, "insert_10k", entries);

    println!("\n=== INSERT 10K 吞吐测试 ===");
    println!("{}", report.to_markdown());

    assert!(report.match_rate >= 0.9);
    assert_eq!(report.pg_errors, 0);
}

/// 测试 3：100K 行 INSERT 吞吐对标
#[test]
#[ignore = "性能测试默认跳过，使用 --ignored 运行（耗时较长）"]
fn bench_insert_100k() {
    let pg_url = match try_pg_url() {
        Some(u) => u,
        None => return,
    };

    let entries = gen_insert_entries(100_000);
    let report = run_bench(&pg_url, "insert_100k", entries);

    println!("\n=== INSERT 100K 吞吐测试 ===");
    println!("{}", report.to_markdown());

    assert!(report.match_rate >= 0.9);
    assert_eq!(report.pg_errors, 0);
}

/// 测试 4：SELECT 延迟对标（先填充 1K 数据，再执行 102 条 SELECT）
#[test]
#[ignore = "性能测试默认跳过，使用 --ignored 运行"]
fn bench_select_1k() {
    let pg_url = match try_pg_url() {
        Some(u) => u,
        None => return,
    };

    // 先填充 1K 数据
    let mut entries = gen_insert_entries(1_000);
    // 再追加 SELECT
    entries.extend(gen_select_entries(100));

    let report = run_bench(&pg_url, "select_1k", entries);

    println!("\n=== SELECT 1K 延迟测试 ===");
    println!("{}", report.to_markdown());

    // 至少 INSERT 部分应匹配
    assert!(report.match_rate >= 0.85);
    assert_eq!(report.pg_errors, 0);
}

/// 测试 5：UPDATE 吞吐对标（先填充 1K，再更新 1K）
#[test]
#[ignore = "性能测试默认跳过，使用 --ignored 运行"]
fn bench_update_1k() {
    let pg_url = match try_pg_url() {
        Some(u) => u,
        None => return,
    };

    let mut entries = gen_insert_entries(1_000);
    entries.extend(gen_update_entries(1_000));

    let report = run_bench(&pg_url, "update_1k", entries);

    println!("\n=== UPDATE 1K 吞吐测试 ===");
    println!("{}", report.to_markdown());

    assert!(report.match_rate >= 0.9);
    assert_eq!(report.pg_errors, 0);
}

/// 测试 6：DELETE 吞吐对标（先填充 1K，再删除 1K）
#[test]
#[ignore = "性能测试默认跳过，使用 --ignored 运行"]
fn bench_delete_1k() {
    let pg_url = match try_pg_url() {
        Some(u) => u,
        None => return,
    };

    let mut entries = gen_insert_entries(1_000);
    entries.extend(gen_delete_entries(1_000));

    let report = run_bench(&pg_url, "delete_1k", entries);

    println!("\n=== DELETE 1K 吞吐测试 ===");
    println!("{}", report.to_markdown());

    assert!(report.match_rate >= 0.9);
    assert_eq!(report.pg_errors, 0);
}

/// 测试 7：综合工作负载（混合 INSERT + SELECT + UPDATE + DELETE）
#[test]
#[ignore = "性能测试默认跳过，使用 --ignored 运行"]
fn bench_mixed_workload() {
    let pg_url = match try_pg_url() {
        Some(u) => u,
        None => return,
    };

    let mut entries = Vec::new();
    // 100 INSERT
    entries.extend(gen_insert_entries(100));
    // 50 SELECT
    entries.extend(gen_select_entries(50));
    // 100 UPDATE
    entries.extend(gen_update_entries(100));
    // 50 DELETE
    entries.extend(gen_delete_entries(50));

    let report = run_bench(&pg_url, "mixed", entries);

    println!("\n=== 综合工作负载测试 ===");
    println!("{}", report.to_markdown());

    assert!(report.match_rate >= 0.9);
    assert_eq!(report.pg_errors, 0);
}
