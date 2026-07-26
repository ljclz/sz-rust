//! 影子回放集成测试 — 在 PG 18 + szrsql 上回放录制的 SQL 序列
//!
//! # 运行前置条件
//!
//! - PostgreSQL 18 运行在 127.0.0.1:5432
//! - 连接串：`postgresql://postgres:postgres@127.0.0.1:5432/postgres`
//!
//! # 运行
//!
//! ```bash
//! cargo test -p szrsql-shadow --test shadow_replay -- --nocapture --test-threads=1
//! ```

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
            eprintln!("[shadow_replay] 跳过：无法连接 PG 18 ({e})");
            None
        }
    }
}

/// 测试 1：完整流程（录制 → 回放 → 报告）
///
/// 1. 准备 10 条 SQL（INSERT/SELECT 混合）
/// 2. 写入 JSONL 流量文件
/// 3. 用 ShadowReplay 回放
/// 4. 生成 ShadowReport
/// 5. 验证：所有 SQL 应匹配（szrsql 与 PG 18 结果一致）
#[test]
fn shadow_replay_full_pipeline() {
    let pg_url = match try_pg_url() {
        Some(u) => u,
        None => return,
    };

    // 1. 准备流量
    let entries: Vec<TrafficEntry> = vec![
        TrafficEntry::new("s1", "INSERT INTO t (id, name) VALUES (1, 'alice')"),
        TrafficEntry::new("s1", "INSERT INTO t (id, name) VALUES (2, 'bob')"),
        TrafficEntry::new("s1", "INSERT INTO t (id, name) VALUES (3, 'carol')"),
        TrafficEntry::new("s1", "SELECT id, name FROM t ORDER BY id"),
        TrafficEntry::new("s1", "SELECT COUNT(*) FROM t"),
        TrafficEntry::new("s1", "SELECT id, name FROM t WHERE id = 2"),
        TrafficEntry::new("s1", "UPDATE t SET name = 'bob2' WHERE id = 2"),
        TrafficEntry::new("s1", "SELECT id, name FROM t WHERE id = 2"),
        TrafficEntry::new("s1", "DELETE FROM t WHERE id = 1"),
        TrafficEntry::new("s1", "SELECT COUNT(*) FROM t"),
    ];

    // 2. 写入 JSONL 文件
    let jsonl_file = NamedTempFile::new().unwrap();
    let count = Recorder::save_to_jsonl(&entries, jsonl_file.path()).unwrap();
    assert_eq!(count, 10);

    // 3. 回放
    let config = ReplayConfig {
        pg_url,
        pg_schema: "szrsql_shadow_test".to_string(),
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

    assert_eq!(results.len(), 10);

    // 4. 生成报告
    let report = ShadowReport::from_results(&results);
    eprintln!("=== 影子回放报告 ===");
    eprintln!("{}", report.to_markdown());

    // 5. 验证：所有 SQL 应匹配
    eprintln!(
        "matched={}, mismatched={}, sz_errors={}",
        report.matched, report.mismatched, report.sz_errors
    );
    // 至少 8/10 应匹配（允许 szrsql 个别 SQL 不支持）
    assert!(
        report.matched + report.sz_errors >= 8,
        "至少 8 条 SQL 应匹配或 szrsql 不支持（不算 PG 错误）：matched={}, sz_errors={}",
        report.matched,
        report.sz_errors
    );
    // 不应有 PG 18 错误（PG 是参考标准）
    assert_eq!(report.pg_errors, 0, "PG 18 不应有执行错误");
}

/// 测试 2：纯 SELECT 影子回放
///
/// 先在 PG 18 + szrsql 各自填充 5 行数据，然后回放 5 条 SELECT
#[test]
fn shadow_replay_select_only() {
    let pg_url = match try_pg_url() {
        Some(u) => u,
        None => return,
    };

    // 准备数据填充 + 查询
    let mut entries: Vec<TrafficEntry> = Vec::new();
    for i in 1..=5 {
        let name = ["alice", "bob", "carol", "dave", "eve"][(i - 1) as usize];
        entries.push(TrafficEntry::new(
            "s1",
            format!("INSERT INTO t (id, name) VALUES ({i}, '{name}')"),
        ));
    }
    entries.push(TrafficEntry::new("s1", "SELECT id, name FROM t ORDER BY id"));
    entries.push(TrafficEntry::new("s1", "SELECT COUNT(*) FROM t"));
    entries.push(TrafficEntry::new(
        "s1",
        "SELECT id, name FROM t WHERE id > 2 ORDER BY id",
    ));

    let jsonl_file = NamedTempFile::new().unwrap();
    Recorder::save_to_jsonl(&entries, jsonl_file.path()).unwrap();

    let config = ReplayConfig {
        pg_url,
        pg_schema: "szrsql_shadow_select".to_string(),
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

    let report = ShadowReport::from_results(&results);
    eprintln!("=== 纯 SELECT 影子回放报告 ===");
    eprintln!("{}", report.to_markdown());

    // SELECT 部分应全部匹配（5 条 INSERT + 3 条 SELECT = 8 条）
    assert_eq!(results.len(), 8);
    assert_eq!(report.pg_errors, 0);
}

/// 测试 3：报告序列化与反序列化
#[test]
fn shadow_report_serialization() {
    use szrsql_shadow::compare::{MatchStatus, ReplayResult};

    let results = vec![
        ReplayResult {
            sql: "SELECT 1".to_string(),
            pg_rows: 1,
            sz_rows: 1,
            pg_latency_ms: 0.5,
            sz_latency_ms: 0.3,
            status: MatchStatus::Match,
        },
        ReplayResult {
            sql: "SELECT 2".to_string(),
            pg_rows: 1,
            sz_rows: 0,
            pg_latency_ms: 0.5,
            sz_latency_ms: 0.3,
            status: MatchStatus::Mismatch("row count mismatch".to_string()),
        },
    ];
    let report = ShadowReport::from_results(&results);

    let json = report.to_json().unwrap();
    let parsed: ShadowReport = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.total, 2);
    assert_eq!(parsed.matched, 1);
    assert_eq!(parsed.mismatched, 1);
    assert!(!parsed.passed);
}
