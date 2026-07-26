//! Phase 4.9 协议兼容性 CI 矩阵 — psql / Python asyncpg / Node.js pg / JDBC 各 50 条 SQL 验证。
//!
//! 完整覆盖进度表 Phase 4.9 验收标准：
//! > CI 中自动执行 psql / JDBC / Python asyncpg / Node.js pg 各 50 条 SQL 验证
//! > 4 种客户端全部通过
//!
//! # 设计
//!
//! - **共享 SQL 列表**：50 条 SQL 语句覆盖 DDL/DML/SELECT/聚合/事务/表达式/类型/函数
//! - **每客户端独立测试函数**：`test_compat_psql` / `test_compat_asyncpg` /
//!   `test_compat_node_pg` / `test_compat_jdbc`
//! - **缺失工具优雅跳过**：通过 `--version` / 模块导入探针检测可用性；缺失时 `eprintln!`
//!   并 `return`（不 panic），CI 环境安装对应工具后自动启用
//! - **统一启动测试服务器**：复用 `PgwireServer` + `PgwireConfig::Trust` 认证模式
//! - **超时保护**：每客户端子进程最长 60s，防止挂起
//! - **错误检测**：检查子进程 exit code 非 0 或 stderr 含 `ERROR`/`FATAL` 即判定失败
//!
//! # CI 矩阵环境
//!
//! CI 环境需安装：
//! - PostgreSQL 客户端工具（提供 `psql`）
//! - Python 3.8+ 与 `asyncpg` 包（`pip install asyncpg`）
//! - Node.js 18+ 与 `pg` 包（`npm install pg`）
//! - Java 11+ 与 PostgreSQL JDBC 驱动（通过 `PG_JDBC_JAR` 环境变量指定 JAR 路径）

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use szrsql_protocol::pgwire::server::{PgwireConfig, PgwireServer};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::process::Command;

// =====================================================================
//  共享 SQL 列表（50 条）
// =====================================================================
//
// 选择原则：
// - 覆盖 DDL（CREATE/DROP TABLE）
// - 覆盖 DML（INSERT/UPDATE/DELETE）
// - 覆盖 SELECT（WHERE/LIKE/IN/IS NULL/DISTINCT/GROUP BY/聚合）
// - 覆盖事务（BEGIN/COMMIT/ROLLBACK）
// - 覆盖表达式（算术/字符串连接/函数）
// - 避免使用 Executor 不支持的 `ORDER BY`（Sort plan node 未实现）
// - 所有语句应在 4 种客户端下行为一致

const SQL_STATEMENTS: &[&str] = &[
    // 1-2: DDL
    "DROP TABLE IF EXISTS compat_test",
    "CREATE TABLE compat_test (id BIGINT PRIMARY KEY, name TEXT, age INT, salary FLOAT, active BOOL)",
    // 3-12: 批量 INSERT（10 行）
    "INSERT INTO compat_test VALUES (1, 'Alice', 30, 50000.00, true)",
    "INSERT INTO compat_test VALUES (2, 'Bob', 25, 45000.00, false)",
    "INSERT INTO compat_test VALUES (3, 'Charlie', 35, 60000.00, true)",
    "INSERT INTO compat_test VALUES (4, 'David', 28, 48000.00, true)",
    "INSERT INTO compat_test VALUES (5, 'Eve', 40, 70000.00, false)",
    "INSERT INTO compat_test VALUES (6, 'Frank', 32, 55000.00, true)",
    "INSERT INTO compat_test VALUES (7, 'Grace', 29, 52000.00, true)",
    "INSERT INTO compat_test VALUES (8, 'Heidi', 45, 80000.00, false)",
    "INSERT INTO compat_test VALUES (9, 'Ivan', 27, 47000.00, true)",
    "INSERT INTO compat_test VALUES (10, 'Judy', 33, 58000.00, true)",
    // 13-20: 简单查询
    "SELECT COUNT(*) FROM compat_test",
    "SELECT * FROM compat_test WHERE id = 1",
    "SELECT * FROM compat_test WHERE age > 30",
    "SELECT * FROM compat_test WHERE active = true",
    "SELECT name, age FROM compat_test WHERE age >= 25 AND age <= 35",
    "SELECT name FROM compat_test WHERE name LIKE 'A%'",
    "SELECT name FROM compat_test WHERE age IN (25, 30, 35)",
    "SELECT name FROM compat_test WHERE age IS NOT NULL",
    // 21-25: 聚合与去重
    "SELECT DISTINCT active FROM compat_test",
    "SELECT COUNT(*), active FROM compat_test GROUP BY active",
    "SELECT SUM(age) FROM compat_test",
    "SELECT AVG(age) FROM compat_test",
    "SELECT MIN(age), MAX(age) FROM compat_test",
    // 26-31: UPDATE / DELETE
    "UPDATE compat_test SET age = 31 WHERE id = 1",
    "SELECT age FROM compat_test WHERE id = 1",
    "DELETE FROM compat_test WHERE id = 2",
    "SELECT COUNT(*) FROM compat_test",
    "INSERT INTO compat_test VALUES (100, 'Test', 99, 9999.99, false)",
    "SELECT * FROM compat_test WHERE id = 100",
    // 32-36: 事务 ROLLBACK
    "BEGIN",
    "INSERT INTO compat_test VALUES (101, 'Tx1', 50, 5000.00, true)",
    "SELECT COUNT(*) FROM compat_test",
    "ROLLBACK",
    "SELECT COUNT(*) FROM compat_test",
    // 37-40: 事务 COMMIT
    "BEGIN",
    "INSERT INTO compat_test VALUES (102, 'Tx2', 51, 5100.00, true)",
    "COMMIT",
    "SELECT COUNT(*) FROM compat_test",
    // 41-47: 表达式与函数
    "SELECT 1 + 1",
    "SELECT 'hello' || ' world'",
    "SELECT 10 * 5",
    "SELECT 100 / 4",
    "SELECT LENGTH('abcde')",
    "SELECT UPPER('hello')",
    "SELECT LOWER('HELLO')",
    // 48-50: 收尾 DML + 验证
    "UPDATE compat_test SET salary = salary * 1.1 WHERE active = true",
    "DELETE FROM compat_test WHERE age > 90",
    "SELECT COUNT(*) FROM compat_test",
];

// =====================================================================
//  服务器启动辅助（与 pgwire_integration.rs 一致）
// =====================================================================

/// 寻找可用端口：从给定起始端口开始尝试。
async fn find_free_port(start: u16) -> u16 {
    for port in start..start + 50 {
        if tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            return port;
        }
    }
    panic!("no free port found in {start}..{}", start + 50);
}

/// 启动一个测试服务器，返回其监听端口对应的 JoinHandle。
async fn spawn_test_server(port: u16) -> tokio::task::JoinHandle<()> {
    let config = PgwireConfig::new()
        .with_host("127.0.0.1")
        .with_port(port)
        .with_server_version("15.0-szrsql-compat");
    let server = PgwireServer::new(config);
    tokio::spawn(async move {
        let _ = server.serve().await;
    })
}

/// 等待服务器就绪（可连接）。
async fn wait_for_server(port: u16) {
    for _ in 0..100 {
        if TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server did not become ready on port {port}");
}

// =====================================================================
//  客户端工具可用性检测
// =====================================================================

/// 检测命令是否可用（通过 `--version` 探测）。
fn check_command_available(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 检测 Python 模块是否可导入。
async fn check_python_module(module: &str) -> bool {
    Command::new("python")
        .arg("-c")
        .arg(format!("import {module}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 检测 Node.js 模块是否可 require。
async fn check_node_module(module: &str) -> bool {
    Command::new("node")
        .arg("-e")
        .arg(format!("require('{module}')"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

// =====================================================================
//  子进程错误检测
// =====================================================================

/// 检查子进程输出是否符合"成功"标准：
/// - exit code 为 0
/// - stderr 不含 `ERROR` / `FATAL`（不区分大小写）
fn validate_client_output(name: &str, exit_code: Option<i32>, stderr: &str) {
    let exit = exit_code.unwrap_or(-1);
    assert_eq!(
        exit, 0,
        "{name}: 子进程退出码非 0（exit={exit}）\nstderr:\n{stderr}"
    );

    let lower = stderr.to_lowercase();
    for keyword in ["error", "fatal", "traceback", "exception"] {
        let has_keyword = lower.contains(keyword);
        // Python 异常栈里包含 "Error" 字样（如 asyncpg.exceptions.PostgresError），
        // 这是真正的错误；JDBC 抛 SQLException 时 stderr 含 "Exception"。
        if has_keyword {
            panic!("{name}: stderr 含错误关键字 `{keyword}`\nstderr:\n{stderr}");
        }
    }
}

/// 在子进程超时前等待其完成，并返回 (exit_code, stdout, stderr)。
async fn run_with_timeout(name: &str, mut cmd: Command) -> (Option<i32>, String, String) {
    let timeout = Duration::from_secs(60);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            (output.status.code(), stdout, stderr)
        }
        Ok(Err(e)) => {
            panic!("{name}: 启动子进程失败: {e}");
        }
        Err(_) => {
            panic!("{name}: 子进程超时（>{timeout:?}）");
        }
    }
}

// =====================================================================
//  测试 1：psql 客户端
// =====================================================================

/// 使用 `psql -f script.sql` 执行 50 条 SQL，开启 `ON_ERROR_STOP=1`。
#[tokio::test]
async fn test_compat_psql() {
    if !check_command_available("psql") {
        eprintln!("SKIP test_compat_psql: psql 未安装");
        return;
    }

    let port = find_free_port(15700).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    // 写入 SQL 脚本文件
    let mut script = String::new();
    for sql in SQL_STATEMENTS {
        script.push_str(sql);
        script.push(';');
        script.push('\n');
    }
    let script_path = unique_temp_path("psql_compat", "sql");
    write_temp_file(&script_path, &script).await;

    let mut cmd = Command::new("psql");
    cmd.arg("-h")
        .arg("127.0.0.1")
        .arg("-p")
        .arg(port.to_string())
        .arg("-U")
        .arg("test_user")
        .arg("-d")
        .arg("test_db")
        .arg("-v")
        .arg("ON_ERROR_STOP=1")
        .arg("-q") // 安静模式：只输出错误
        .arg("-f")
        .arg(&script_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let (exit_code, _stdout, stderr) = run_with_timeout("psql", cmd).await;
    let _ = std::fs::remove_file(&script_path);
    validate_client_output("psql", exit_code, &stderr);
    eprintln!("PASS test_compat_psql: 50 条 SQL 全部执行成功");
}

// =====================================================================
//  测试 2：Python asyncpg 客户端
// =====================================================================

#[tokio::test]
async fn test_compat_asyncpg() {
    if !check_command_available("python") {
        eprintln!("SKIP test_compat_asyncpg: python 未安装");
        return;
    }
    if !check_python_module("asyncpg").await {
        eprintln!("SKIP test_compat_asyncpg: asyncpg 模块未安装（pip install asyncpg）");
        return;
    }

    let port = find_free_port(15800).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    // 生成 Python 脚本
    let script = format!(
        r#"import asyncio
import asyncpg
import sys

SQL_STATEMENTS = {sql_list:?}

async def main():
    conn = await asyncpg.connect(
        host='127.0.0.1',
        port={port},
        user='test_user',
        database='test_db',
    )
    for i, sql in enumerate(SQL_STATEMENTS, 1):
        try:
            stripped = sql.strip().upper()
            # asyncpg: fetch() 用于 SELECT/WITH；execute() 用于 DML/DDL/事务
            if stripped.startswith('SELECT') or stripped.startswith('WITH'):
                await conn.fetch(sql)
            else:
                await conn.execute(sql)
        except Exception as e:
            print(f'STMT {{i}} FAILED: {{sql}}\nERROR: {{e}}', file=sys.stderr)
            await conn.close()
            sys.exit(1)
    await conn.close()
    print(f'All {{len(SQL_STATEMENTS)}} statements executed successfully')

asyncio.run(main())
"#,
        sql_list = SQL_STATEMENTS,
        port = port,
    );

    let script_path = unique_temp_path("asyncpg_compat", "py");
    write_temp_file(&script_path, &script).await;

    let mut cmd = Command::new("python");
    cmd.arg(&script_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let (exit_code, _stdout, stderr) = run_with_timeout("asyncpg", cmd).await;
    let _ = std::fs::remove_file(&script_path);
    validate_client_output("asyncpg", exit_code, &stderr);
    eprintln!("PASS test_compat_asyncpg: 50 条 SQL 全部执行成功");
}

// =====================================================================
//  测试 3：Node.js pg 客户端
// =====================================================================

#[tokio::test]
async fn test_compat_node_pg() {
    if !check_command_available("node") {
        eprintln!("SKIP test_compat_node_pg: node 未安装");
        return;
    }
    if !check_node_module("pg").await {
        eprintln!("SKIP test_compat_node_pg: pg 模块未安装（npm install pg）");
        return;
    }

    let port = find_free_port(15900).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    // 生成 JS 脚本
    let mut sql_array = String::from("[\n");
    for sql in SQL_STATEMENTS {
        // 转义反斜杠和引号
        let escaped = sql.replace('\\', "\\\\").replace('\'', "\\'");
        sql_array.push_str(&format!("  '{}',\n", escaped));
    }
    sql_array.push(']');

    let script = format!(
        r#"'use strict';
const {{ Client }} = require('pg');

const SQL_STATEMENTS = {sql_array};

async function main() {{
  const client = new Client({{
    host: '127.0.0.1',
    port: {port},
    user: 'test_user',
    database: 'test_db',
  }});
  await client.connect();
  for (let i = 0; i < SQL_STATEMENTS.length; i++) {{
    const sql = SQL_STATEMENTS[i];
    try {{
      await client.query(sql);
    }} catch (e) {{
      console.error(`STMT ${{i + 1}} FAILED: ${{sql}}`);
      console.error(`ERROR: ${{e.message}}`);
      await client.end();
      process.exit(1);
    }}
  }}
  await client.end();
  console.log(`All ${{SQL_STATEMENTS.length}} statements executed successfully`);
}}

main().catch(e => {{
  console.error('FATAL:', e);
  process.exit(1);
}});
"#,
        sql_array = sql_array,
        port = port,
    );

    let script_path = unique_temp_path("node_pg_compat", "js");
    write_temp_file(&script_path, &script).await;

    let mut cmd = Command::new("node");
    cmd.arg(&script_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let (exit_code, _stdout, stderr) = run_with_timeout("node_pg", cmd).await;
    let _ = std::fs::remove_file(&script_path);
    validate_client_output("node_pg", exit_code, &stderr);
    eprintln!("PASS test_compat_node_pg: 50 条 SQL 全部执行成功");
}

// =====================================================================
//  测试 4：JDBC 客户端
// =====================================================================

#[tokio::test]
async fn test_compat_jdbc() {
    if !check_command_available("java") {
        eprintln!("SKIP test_compat_jdbc: java 未安装");
        return;
    }
    let jdbc_jar = match std::env::var("PG_JDBC_JAR") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("SKIP test_compat_jdbc: PG_JDBC_JAR 环境变量未设置");
            return;
        }
    };
    if !std::path::Path::new(&jdbc_jar).exists() {
        eprintln!("SKIP test_compat_jdbc: PG_JDBC_JAR 文件不存在: {jdbc_jar}");
        return;
    }

    let port = find_free_port(16000).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    // 生成 Java 单文件源码（Java 11+ 单文件模式：`java --source 11 File.java`）
    let mut sql_array = String::from("{\n");
    for sql in SQL_STATEMENTS {
        // 转义 Java 字符串中的反斜杠和引号
        let escaped = sql.replace('\\', "\\\\").replace('"', "\\\"");
        sql_array.push_str(&format!("        \"{}\",\n", escaped));
    }
    sql_array.push_str("      }");

    let script = format!(
        r#"import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.ResultSet;
import java.sql.Statement;

public class JdbcCompat {{
    public static void main(String[] args) throws Exception {{
        String[] statements = {sql_array};
        String url = "jdbc:postgresql://127.0.0.1:{port}/test_db?user=test_user";

        try (Connection conn = DriverManager.getConnection(url)) {{
            for (int i = 0; i < statements.length; i++) {{
                String sql = statements[i];
                try (Statement stmt = conn.createStatement()) {{
                    boolean hasResultSet = stmt.execute(sql);
                    if (hasResultSet) {{
                        try (ResultSet rs = stmt.getResultSet()) {{
                            while (rs.next()) {{
                                // 消费结果集
                            }}
                        }}
                    }}
                }} catch (Exception e) {{
                    System.err.println("STMT " + (i + 1) + " FAILED: " + sql);
                    System.err.println("ERROR: " + e.getMessage());
                    System.exit(1);
                }}
            }}
            System.out.println("All " + statements.length + " statements executed successfully");
        }}
    }}
}}
"#,
        sql_array = sql_array,
        port = port,
    );

    let script_path = unique_temp_path("JdbcCompat", "java");
    write_temp_file(&script_path, &script).await;

    // 构造 classpath：当前目录 + JDBC JAR（Windows 用 ';'，Unix 用 ':'）
    let sep = if cfg!(windows) {
        ";"
    } else {
        ":"
    };
    let classpath = format!(".{sep}{jdbc_jar}");

    let mut cmd = Command::new("java");
    cmd.arg("--class-path")
        .arg(&classpath)
        .arg(&script_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let (exit_code, _stdout, stderr) = run_with_timeout("jdbc", cmd).await;
    let _ = std::fs::remove_file(&script_path);
    validate_client_output("jdbc", exit_code, &stderr);
    eprintln!("PASS test_compat_jdbc: 50 条 SQL 全部执行成功");
}

// =====================================================================
//  辅助：临时文件
// =====================================================================

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
