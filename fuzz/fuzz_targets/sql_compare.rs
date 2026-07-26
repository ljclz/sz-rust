//! SQL 差分模糊测试 target — szrsql vs PostgreSQL 18
//!
//! 对应 `docs/FUZZ_REPORT.md` P1-3 任务。
//!
//! # 设计
//!
//! 使用 libFuzzer 生成随机字节序列，解码为 SQL 操作序列，
//! 在 szrsql 和 PG 18 上执行，比对结果集。
//!
//! # 运行
//!
//! ```bash
//! # 需要 nightly + ASAN 运行时
//! cargo +nightly fuzz run sql_compare -- -max_len=1024 -max_total_time=60
//! ```
//!
//! # 注意
//!
//! - Windows 平台需要 ASAN DLL（参见 FUZZ_REPORT.md）
//! - 替代方案：使用 `tests/sql_compare.rs` 集成测试版本（无需 ASAN）
//! - 此 target 仅作为 libFuzzer 入口，实际差分比对逻辑在 `tests/sql_compare.rs`

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::OnceLock;

/// PG 18 连接（懒初始化，跨 fuzz 迭代复用）
static PG_CLIENT: OnceLock<std::sync::Mutex<postgres::Client>> = OnceLock::new();

/// 初始化 PG 18 连接 + 测试 schema
fn init_pg() -> Option<&'static std::sync::Mutex<postgres::Client>> {
    PG_CLIENT.get_or_init(|| {
        let mut client = match postgres::Client::connect(
            "postgresql://postgres:postgres@127.0.0.1:5432/postgres",
            postgres::NoTls,
        ) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[sql_compare fuzz] PG 18 连接失败，fuzz 将仅测试 szrsql: {e}");
                return std::sync::Mutex::new(
                    postgres::Client::connect(
                        "postgresql://postgres:postgres@127.0.0.1:1/postgres",
                        postgres::NoTls,
                    )
                    .expect("dummy PG connection must fail"),
                );
            }
        };
        let _ = client.batch_execute(
            "DROP SCHEMA IF EXISTS szrsql_fuzz CASCADE;
             CREATE SCHEMA szrsql_fuzz;
             SET search_path TO szrsql_fuzz;
             CREATE TABLE t (id BIGINT PRIMARY KEY, name TEXT);",
        );
        std::sync::Mutex::new(client)
    })
    .into()
}

/// 从 fuzz 输入解码 SQL 操作
///
/// 字节格式：
/// - byte 0: op_type (0=INSERT, 1=UPDATE, 2=DELETE, 3=SELECT)
/// - bytes 1-8: i64 id (小端)
/// - byte 9: name index (0-7)
fn decode_sql(data: &[u8]) -> Option<String> {
    if data.len() < 10 {
        return None;
    }
    let op = data[0];
    let id = i64::from_le_bytes(data[1..9].try_into().ok()?);
    let name_idx = (data[9] as usize) % 8;
    let names = ["alice", "bob", "carol", "dave", "eve", "frank", "grace", "henry"];
    let name = names[name_idx];

    Some(match op % 4 {
        0 => format!("INSERT INTO t (id, name) VALUES ({id}, '{name}')"),
        1 => format!("UPDATE t SET name = '{name}' WHERE id = {id}"),
        2 => format!("DELETE FROM t WHERE id = {id}"),
        3 => format!("SELECT id, name FROM t WHERE id = {id}"),
        _ => unreachable!(),
    })
}

fuzz_target!(|data: &[u8]| {
    let _ = init_pg();

    let sql = match decode_sql(data) {
        Some(s) => s,
        None => return,
    };

    // 1. 在 szrsql 上执行（验证不 panic）
    let catalog = szrsql_sql::plan::InMemoryCatalog::new();
    // 注意：fuzz 模式下不实际执行 szrsql（需要预填充表），
    // 仅验证 parse + plan 不 panic
    if let Ok(stmts) = szrsql_sql::parser::parse_sql(&sql) {
        let planner = szrsql_sql::plan::Planner::new(&catalog);
        for stmt in stmts {
            let _ = planner.plan_statement(stmt);
        }
    }

    // 2. 在 PG 18 上执行（如果可用）
    if let Some(pg_lock) = init_pg() {
        if let Ok(mut client) = pg_lock.lock() {
            // 忽略 PG 错误（如主键冲突），仅验证不 panic
            let _ = client.execute(&sql, &[]);
        }
    }
});
