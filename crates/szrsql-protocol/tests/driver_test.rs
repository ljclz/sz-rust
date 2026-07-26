//! Phase 4.10 多语言驱动验证 — Python (asyncpg) / Go (pgx) / Node.js (node-postgres) / Rust (sqlx) 各执行 CRUD + 事务。
//!
//! 完整覆盖进度表 Phase 4.10 验收标准：
//! > Python (asyncpg) / Go (pgx) / Node.js (node-postgres) / Rust (sqlx) 各执行 CRUD + 事务
//! > 4 种驱动全部通过
//!
//! # 与 Phase 4.9 的差异
//!
//! - Phase 4.9：50 条共享 SQL 语句（覆盖 DDL/DML/SELECT/聚合/事务等广度）
//! - Phase 4.10：聚焦 CRUD 工作流 + 事务语义验证（深度）
//!
//! # CRUD + 事务工作流
//!
//! 每个驱动执行完全相同的 12 步工作流：
//! 1. CREATE TABLE driver_test (id BIGINT PRIMARY KEY, name TEXT, value INT)
//! 2. INSERT INTO driver_test VALUES (1, 'alice', 100)
//! 3. INSERT INTO driver_test VALUES (2, 'bob', 200)
//! 4. SELECT COUNT(*) → 期望 2
//! 5. UPDATE driver_test SET value = 150 WHERE id = 1
//! 6. SELECT value WHERE id = 1 → 期望 150
//! 7. DELETE FROM driver_test WHERE id = 2
//! 8. SELECT COUNT(*) → 期望 1
//! 9. BEGIN
//! 10. INSERT INTO driver_test VALUES (3, 'charlie', 300)
//! 11. ROLLBACK
//! 12. SELECT COUNT(*) → 期望 1（ROLLBACK 后仍为 1）
//!
//! # CI 矩阵环境
//!
//! CI 环境需安装：
//! - Python 3.8+ 与 `asyncpg` 包（`pip install asyncpg`）
//! - Go 1.20+ 与 `pgx` 模块（`go get github.com/jackc/pgx/v5`）
//! - Node.js 18+ 与 `pg` 包（`npm install pg`）
//! - Rust 1.78+ 与 `sqlx` 0.8+（作为 dev-dependency 已配置）

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use szrsql_protocol::pgwire::server::{PgwireConfig, PgwireServer};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

// =====================================================================
//  共享 CRUD + 事务工作流（12 步）
// =====================================================================

/// CRUD 工作流的 SQL 语句列表（按执行顺序）。
///
/// 每条 SQL 都是自包含的，不依赖前一条的返回值（客户端自行验证结果）。
const WORKFLOW_SQL: &[&str] = &[
    // 1. CREATE TABLE
    "DROP TABLE IF EXISTS driver_test",
    "CREATE TABLE driver_test (id BIGINT PRIMARY KEY, name TEXT, value INT)",
    // 2-3. INSERT
    "INSERT INTO driver_test VALUES (1, 'alice', 100)",
    "INSERT INTO driver_test VALUES (2, 'bob', 200)",
    // 4. SELECT COUNT → 2
    "SELECT COUNT(*) FROM driver_test",
    // 5. UPDATE
    "UPDATE driver_test SET value = 150 WHERE id = 1",
    // 6. SELECT value → 150
    "SELECT value FROM driver_test WHERE id = 1",
    // 7. DELETE
    "DELETE FROM driver_test WHERE id = 2",
    // 8. SELECT COUNT → 1
    "SELECT COUNT(*) FROM driver_test",
    // 9-11. 事务 ROLLBACK
    "BEGIN",
    "INSERT INTO driver_test VALUES (3, 'charlie', 300)",
    "ROLLBACK",
    // 12. SELECT COUNT → 1
    "SELECT COUNT(*) FROM driver_test",
];

// =====================================================================
//  辅助函数（与 pgwire_compat.rs 一致的设计）
// =====================================================================

/// 检查命令是否可用（通过 `--version` 探针）。
async fn check_command_available(cmd: &str) -> bool {
    let mut probe = Command::new(cmd);
    probe
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match tokio::time::timeout(Duration::from_secs(5), probe.status()).await {
        Ok(Ok(status)) => status.success(),
        _ => false,
    }
}

/// 检查 Python 模块是否可用。
async fn check_python_module(module: &str) -> bool {
    let mut probe = Command::new("python");
    probe
        .arg("-c")
        .arg(format!("import {module}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match tokio::time::timeout(Duration::from_secs(10), probe.status()).await {
        Ok(Ok(status)) => status.success(),
        _ => false,
    }
}

/// 检查 Node.js 模块是否可用。
async fn check_node_module(module: &str) -> bool {
    let mut probe = Command::new("node");
    probe
        .arg("-e")
        .arg(format!("require('{module}')"))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match tokio::time::timeout(Duration::from_secs(10), probe.status()).await {
        Ok(Ok(status)) => status.success(),
        _ => false,
    }
}

/// 检查 Go 模块是否可用（通过 `go list` 探针）。
async fn check_go_module(module: &str) -> bool {
    let mut probe = Command::new("go");
    probe
        .arg("list")
        .arg("-m")
        .arg(module)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match tokio::time::timeout(Duration::from_secs(15), probe.status()).await {
        Ok(Ok(status)) => status.success(),
        _ => false,
    }
}

/// 查找空闲端口（从 start 开始递增扫描）。
async fn find_free_port(start: u16) -> u16 {
    for port in start..start.saturating_add(100) {
        match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(_) => return port,
            Err(_) => continue,
        }
    }
    panic!("找不到空闲端口（范围 {start}..{}）", start + 100);
}

/// 启动测试服务器（trust 认证模式，随机端口）。
async fn spawn_test_server(port: u16) -> tokio::task::JoinHandle<()> {
    let config = PgwireConfig::new()
        .with_port(port)
        .with_server_version("15.0-szrsql-driver-test");
    let server = PgwireServer::new(config);
    tokio::spawn(async move {
        let _ = server.serve().await;
    })
}

/// 等待服务器就绪（最多 5s）。
async fn wait_for_server(port: u16) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if std::time::Instant::now() > deadline {
            panic!("服务器在 5s 内未就绪（端口 {port}）");
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 生成唯一的临时文件路径（不创建文件）。
fn unique_temp_path(prefix: &str, ext: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let filename = format!("{prefix}_{pid}_{nanos}.{ext}");
    std::env::temp_dir().join(filename)
}

/// 异步写入临时文件。
async fn write_temp_file(path: &PathBuf, content: &str) {
    let mut file = tokio::fs::File::create(path)
        .await
        .unwrap_or_else(|e| panic!("无法创建临时文件 {path:?}: {e}"));
    file.write_all(content.as_bytes())
        .await
        .unwrap_or_else(|e| panic!("无法写入临时文件 {path:?}: {e}"));
    file.flush().await.ok();
}

/// 运行子进程并附加 60s 超时保护，返回 (exit_code, stdout, stderr)。
async fn run_with_timeout(name: &str, mut cmd: Command) -> (Option<i32>, String, String) {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("无法启动 {name} 子进程: {e}"));

    match tokio::time::timeout(Duration::from_secs(60), child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            (output.status.code(), stdout, stderr)
        }
        Ok(Err(e)) => panic!("{name} 子进程等待失败: {e}"),
        Err(_) => {
            panic!("{name} 子进程超时（60s）")
        }
    }
}

/// 校验子进程输出：exit code 非 0 或 stderr 含 ERROR/FATAL 即判定失败。
fn validate_driver_output(driver: &str, exit_code: Option<i32>, stderr: &str) {
    let exit = exit_code.unwrap_or(-1);
    assert_eq!(
        exit, 0,
        "{driver} 测试失败：exit code = {exit}\nstderr:\n{stderr}"
    );
    let stderr_upper = stderr.to_uppercase();
    assert!(
        !stderr_upper.contains("ERROR") && !stderr_upper.contains("FATAL"),
        "{driver} 测试失败：stderr 含错误\nstderr:\n{stderr}"
    );
}

// =====================================================================
//  测试 1：Python asyncpg 驱动
// =====================================================================

#[tokio::test]
async fn test_driver_asyncpg() {
    if !check_command_available("python").await {
        eprintln!("SKIP test_driver_asyncpg: python 未安装");
        return;
    }
    if !check_python_module("asyncpg").await {
        eprintln!("SKIP test_driver_asyncpg: asyncpg 模块未安装（pip install asyncpg）");
        return;
    }

    let port = find_free_port(16700).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    // 生成 Python 脚本：执行 12 步 CRUD + 事务工作流，并在关键步骤验证结果
    let script = format!(
        r#"import asyncio
import asyncpg
import sys

SQL_WORKFLOW = {sql_list:?}

# 关键步骤的期望值（步骤索引从 0 开始）
# 步骤 4 (SELECT COUNT(*) → 2)
# 步骤 6 (SELECT value WHERE id=1 → 150)
# 步骤 8 (SELECT COUNT(*) → 1)
# 步骤 12 (SELECT COUNT(*) → 1, ROLLBACK 后)

async def main():
    conn = await asyncpg.connect(
        host='127.0.0.1',
        port={port},
        user='test_user',
        database='test_db',
    )
    for i, sql in enumerate(SQL_WORKFLOW, 1):
        try:
            stripped = sql.strip().upper()
            if stripped.startswith('SELECT') or stripped.startswith('WITH'):
                rows = await conn.fetch(sql)
                # 关键步骤验证
                if i == 5:  # SELECT COUNT(*) FROM driver_test (after 2 INSERTs)
                    count = rows[0][0]
                    if count != 2:
                        raise Exception(f'STEP {{i}}: expected COUNT=2, got {{count!r}}')
                elif i == 7:  # SELECT value FROM driver_test WHERE id=1 (after UPDATE)
                    value = rows[0][0]
                    if value != 150:
                        raise Exception(f'STEP {{i}}: expected value=150, got {{value!r}}')
                elif i == 9:  # SELECT COUNT(*) (after DELETE)
                    count = rows[0][0]
                    if count != 1:
                        raise Exception(f'STEP {{i}}: expected COUNT=1, got {{count!r}}')
                elif i == 13:  # SELECT COUNT(*) (after ROLLBACK)
                    count = rows[0][0]
                    if count != 1:
                        raise Exception(f'STEP {{i}}: expected COUNT=1 after ROLLBACK, got {{count!r}}')
            else:
                await conn.execute(sql)
        except Exception as e:
            print(f'STEP {{i}} FAILED: {{sql}}\nERROR: {{e}}', file=sys.stderr)
            await conn.close()
            sys.exit(1)
    await conn.close()
    print(f'All {{len(SQL_WORKFLOW)}} steps passed')

asyncio.run(main())
"#,
        sql_list = WORKFLOW_SQL,
        port = port,
    );

    let script_path = unique_temp_path("driver_asyncpg", "py");
    write_temp_file(&script_path, &script).await;

    let mut cmd = Command::new("python");
    cmd.arg(&script_path);

    let (exit_code, _stdout, stderr) = run_with_timeout("asyncpg", cmd).await;
    let _ = std::fs::remove_file(&script_path);
    validate_driver_output("asyncpg", exit_code, &stderr);
    eprintln!("PASS test_driver_asyncpg: 12 步 CRUD + 事务工作流全部通过");
}

// =====================================================================
//  测试 2：Go pgx 驱动
// =====================================================================

#[tokio::test]
async fn test_driver_pgx() {
    if !check_command_available("go").await {
        eprintln!("SKIP test_driver_pgx: go 未安装");
        return;
    }
    if !check_go_module("github.com/jackc/pgx/v5").await {
        eprintln!("SKIP test_driver_pgx: pgx 模块未安装（go get github.com/jackc/pgx/v5）");
        return;
    }

    let port = find_free_port(16800).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    // 生成 Go 脚本：使用 pgx 执行 12 步 CRUD + 事务工作流
    let mut sql_array = String::from("var sqlWorkflow = []string{\n");
    for sql in WORKFLOW_SQL {
        let escaped = sql.replace('\\', "\\\\").replace('"', "\\\"");
        sql_array.push_str(&format!("    \"{escaped}\",\n"));
    }
    sql_array.push_str("}\n");

    let script = format!(
        r#"package main

import (
    "context"
    "fmt"
    "os"
    "github.com/jackc/pgx/v5"
)

{sql_array}

func main() {{
    ctx := context.Background()
    url := fmt.Sprintf("postgres://test_user@127.0.0.1:{port}/test_db?sslmode=disable")
    conn, err := pgx.Connect(ctx, url)
    if err != nil {{
        fmt.Fprintf(os.Stderr, "CONNECT FAILED: %v\n", err)
        os.Exit(1)
    }}
    defer conn.Close(ctx)

    for i, sql := range sqlWorkflow {{
        step := i + 1
        stripped := sql
        // 判断是否为 SELECT
        if len(stripped) >= 6 && (stripped[:6] == "SELECT" || stripped[:6] == "select") {{
            rows, err := conn.Query(ctx, sql)
            if err != nil {{
                fmt.Fprintf(os.Stderr, "STEP %d FAILED: %s\nERROR: %v\n", step, sql, err)
                os.Exit(1)
            }}
            // 读取所有行
            cols := rows.FieldDescriptions()
            collected := [][]interface{{}}{{}}
            for rows.Next() {{
                vals, err := rows.Values()
                if err != nil {{
                    rows.Close()
                    fmt.Fprintf(os.Stderr, "STEP %d ROW VALUES FAILED: %v\n", step, err)
                    os.Exit(1)
                }}
                collected = append(collected, vals)
            }}
            rows.Close()
            // 关键步骤验证
            if step == 5 {{ // SELECT COUNT(*) → 2
                if len(collected) == 0 || len(cols) == 0 {{
                    fmt.Fprintf(os.Stderr, "STEP %d: no rows\n", step)
                    os.Exit(1)
                }}
                count, ok := collected[0][0].(int64)
                if !ok || count != 2 {{
                    fmt.Fprintf(os.Stderr, "STEP %d: expected COUNT=2, got %v\n", step, collected[0][0])
                    os.Exit(1)
                }}
            }} else if step == 7 {{ // SELECT value → 150
                if len(collected) == 0 {{
                    fmt.Fprintf(os.Stderr, "STEP %d: no rows\n", step)
                    os.Exit(1)
                }}
                value, ok := collected[0][0].(int64)
                if !ok || value != 150 {{
                    fmt.Fprintf(os.Stderr, "STEP %d: expected value=150, got %v\n", step, collected[0][0])
                    os.Exit(1)
                }}
            }} else if step == 9 {{ // SELECT COUNT(*) → 1
                count, ok := collected[0][0].(int64)
                if !ok || count != 1 {{
                    fmt.Fprintf(os.Stderr, "STEP %d: expected COUNT=1, got %v\n", step, collected[0][0])
                    os.Exit(1)
                }}
            }} else if step == 13 {{ // SELECT COUNT(*) → 1 (after ROLLBACK)
                count, ok := collected[0][0].(int64)
                if !ok || count != 1 {{
                    fmt.Fprintf(os.Stderr, "STEP %d: expected COUNT=1 after ROLLBACK, got %v\n", step, collected[0][0])
                    os.Exit(1)
                }}
            }}
        }} else {{
            _, err := conn.Exec(ctx, sql)
            if err != nil {{
                fmt.Fprintf(os.Stderr, "STEP %d FAILED: %s\nERROR: %v\n", step, sql, err)
                os.Exit(1)
            }}
        }}
    }}
    fmt.Printf("All %d steps passed\n", len(sqlWorkflow))
}}
"#,
        sql_array = sql_array,
        port = port,
    );

    let script_path = unique_temp_path("driver_pgx", "go");
    write_temp_file(&script_path, &script).await;

    let mut cmd = Command::new("go");
    cmd.arg("run").arg(&script_path);

    let (exit_code, _stdout, stderr) = run_with_timeout("pgx", cmd).await;
    let _ = std::fs::remove_file(&script_path);
    validate_driver_output("pgx", exit_code, &stderr);
    eprintln!("PASS test_driver_pgx: 12 步 CRUD + 事务工作流全部通过");
}

// =====================================================================
//  测试 3：Node.js node-postgres 驱动
// =====================================================================

#[tokio::test]
async fn test_driver_node_pg() {
    if !check_command_available("node").await {
        eprintln!("SKIP test_driver_node_pg: node 未安装");
        return;
    }
    if !check_node_module("pg").await {
        eprintln!("SKIP test_driver_node_pg: pg 模块未安装（npm install pg）");
        return;
    }

    let port = find_free_port(16900).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    // 生成 JS 脚本：使用 pg.Client 执行 12 步 CRUD + 事务工作流
    let mut sql_array = String::from("[\n");
    for sql in WORKFLOW_SQL {
        let escaped = sql.replace('\\', "\\\\").replace('\'', "\\'");
        sql_array.push_str(&format!("  '{}',\n", escaped));
    }
    sql_array.push_str("]\n");

    let script = format!(
        r#"'use strict';
const {{ Client }} = require('pg');

const SQL_WORKFLOW = {sql_array};

async function main() {{
  const client = new Client({{
    host: '127.0.0.1',
    port: {port},
    user: 'test_user',
    database: 'test_db',
  }});
  await client.connect();

  for (let i = 0; i < SQL_WORKFLOW.length; i++) {{
    const step = i + 1;
    const sql = SQL_WORKFLOW[i];
    try {{
      const stripped = sql.trim().toUpperCase();
      if (stripped.startsWith('SELECT') || stripped.startsWith('WITH')) {{
        const res = await client.query(sql);
        // 关键步骤验证
        if (step === 5) {{ // SELECT COUNT(*) → 2
          const count = parseInt(res.rows[0].count, 10);
          if (count !== 2) {{
            throw new Error(`STEP ${{step}}: expected COUNT=2, got ${{count}}`);
          }}
        }} else if (step === 7) {{ // SELECT value → 150
          const value = parseInt(res.rows[0].value, 10);
          if (value !== 150) {{
            throw new Error(`STEP ${{step}}: expected value=150, got ${{value}}`);
          }}
        }} else if (step === 9) {{ // SELECT COUNT(*) → 1
          const count = parseInt(res.rows[0].count, 10);
          if (count !== 1) {{
            throw new Error(`STEP ${{step}}: expected COUNT=1, got ${{count}}`);
          }}
        }} else if (step === 13) {{ // SELECT COUNT(*) → 1 (after ROLLBACK)
          const count = parseInt(res.rows[0].count, 10);
          if (count !== 1) {{
            throw new Error(`STEP ${{step}}: expected COUNT=1 after ROLLBACK, got ${{count}}`);
          }}
        }}
      }} else {{
        await client.query(sql);
      }}
    }} catch (e) {{
      console.error(`STEP ${{step}} FAILED: ${{sql}}`);
      console.error(`ERROR: ${{e.message}}`);
      await client.end();
      process.exit(1);
    }}
  }}
  await client.end();
  console.log(`All ${{SQL_WORKFLOW.length}} steps passed`);
}}

main().catch(e => {{
  console.error(`UNEXPECTED ERROR: ${{e.message}}`);
  process.exit(1);
}});
"#,
        sql_array = sql_array,
        port = port,
    );

    let script_path = unique_temp_path("driver_node_pg", "js");
    write_temp_file(&script_path, &script).await;

    let mut cmd = Command::new("node");
    cmd.arg(&script_path);

    let (exit_code, _stdout, stderr) = run_with_timeout("node_pg", cmd).await;
    let _ = std::fs::remove_file(&script_path);
    validate_driver_output("node_pg", exit_code, &stderr);
    eprintln!("PASS test_driver_node_pg: 12 步 CRUD + 事务工作流全部通过");
}

// =====================================================================
//  测试 4：Rust sqlx 驱动
// =====================================================================

#[tokio::test]
async fn test_driver_sqlx() {
    let port = find_free_port(17000).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    use sqlx::postgres::PgPoolOptions;
    use sqlx::Row;

    let url = format!("postgres://test_user@127.0.0.1:{port}/test_db");

    // 连接池（强制单连接：szrsql 当前为每连接独立内存态，
    // 多连接池会导致 CREATE TABLE 在 A 连接、INSERT 在 B 连接时找不到表）
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await
        .unwrap_or_else(|e| panic!("sqlx 连接失败: {e}"));

    for (i, sql) in WORKFLOW_SQL.iter().enumerate() {
        let step = i + 1;
        let stripped = sql.trim().to_uppercase();

        let result = if stripped.starts_with("SELECT") || stripped.starts_with("WITH") {
            // SELECT 查询
            sqlx::query(sql).fetch_all(&pool).await
        } else {
            // DDL/DML/事务控制
            sqlx::query(sql).execute(&pool).await.map(|_| Vec::new())
        };

        match result {
            Ok(rows) => {
                // 关键步骤验证
                if step == 5 {
                    // SELECT COUNT(*) FROM driver_test → 期望 2
                    let count: i64 = rows[0]
                        .try_get(0)
                        .unwrap_or_else(|e| panic!("STEP {step}: 读取 COUNT 失败: {e}"));
                    assert_eq!(count, 2, "STEP {step}: expected COUNT=2, got {count}");
                } else if step == 7 {
                    // SELECT value FROM driver_test WHERE id=1 → 期望 150
                    // 注：szrsql 内部将所有整数统一存储为 i64（ColumnType::Int64），
                    // 即使 CREATE TABLE 声明为 INT，OID 仍是 INT8，需按 i64 解码
                    let value: i64 = rows[0]
                        .try_get(0)
                        .unwrap_or_else(|e| panic!("STEP {step}: 读取 value 失败: {e}"));
                    assert_eq!(value, 150, "STEP {step}: expected value=150, got {value}");
                } else if step == 9 {
                    // SELECT COUNT(*) → 期望 1（DELETE 后）
                    let count: i64 = rows[0]
                        .try_get(0)
                        .unwrap_or_else(|e| panic!("STEP {step}: 读取 COUNT 失败: {e}"));
                    assert_eq!(count, 1, "STEP {step}: expected COUNT=1, got {count}");
                } else if step == 13 {
                    // SELECT COUNT(*) → 期望 1（ROLLBACK 后）
                    let count: i64 = rows[0]
                        .try_get(0)
                        .unwrap_or_else(|e| panic!("STEP {step}: 读取 COUNT 失败: {e}"));
                    assert_eq!(
                        count, 1,
                        "STEP {step}: expected COUNT=1 after ROLLBACK, got {count}"
                    );
                }
            }
            Err(e) => {
                panic!("STEP {step} FAILED: {sql}\nERROR: {e}");
            }
        }
    }

    pool.close().await;
    eprintln!("PASS test_driver_sqlx: 12 步 CRUD + 事务工作流全部通过");
}
