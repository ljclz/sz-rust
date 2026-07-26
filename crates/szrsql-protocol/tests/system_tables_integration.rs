//! Phase 4.7 端到端集成测试 — 元数据查询（pg_catalog + information_schema）。
//!
//! 通过启动实际 TCP 服务器 + 简单查询协议验证 DBeaver / DataGrip 等数据库工具
//! 连接后浏览元数据的标准流程：
//! - `SELECT * FROM pg_tables` — 列出所有表
//! - `SELECT * FROM information_schema.tables` — ANSI SQL 标准表清单
//! - `SELECT * FROM information_schema.columns` — ANSI SQL 标准列清单
//! - `SELECT * FROM pg_catalog.pg_tables` — 带 schema 前缀查询
//! - WHERE 过滤、ORDER BY 排序、LIMIT/OFFSET 分页
//!
//! 完整覆盖进度表 Phase 4.7 验收标准：
//! > DBeaver 连接 → 自动列出数据库/表/列信息（通过 pg_catalog 子集）
//! > DBeaver 表结构显示正确

use std::time::Duration;
use szrsql_protocol::pgwire::{
    message::{
        MSG_COMMAND_COMPLETE, MSG_DATA_ROW, MSG_ERROR_RESPONSE, MSG_READY_FOR_QUERY,
        MSG_ROW_DESCRIPTION,
    },
    server::{PgwireConfig, PgwireServer},
    startup::{encode_startup_message, StartupParams},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// =====================================================================
//  辅助函数（与 pgwire_integration.rs 同样的模式，保持独立性）
// =====================================================================

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

async fn spawn_test_server(port: u16) -> tokio::task::JoinHandle<()> {
    let config = PgwireConfig::new()
        .with_host("127.0.0.1")
        .with_port(port)
        .with_server_version("14.0-test");
    let server = PgwireServer::new(config);
    tokio::spawn(async move {
        let _ = server.serve().await;
    })
}

async fn wait_for_server(port: u16) {
    for _ in 0..50 {
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

async fn read_until_ready_for_query(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await.expect("read should succeed");
        if n == 0 {
            panic!("connection closed before ReadyForQuery");
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_last_message_start(&buf, b'Z') {
            if buf.len() >= pos + 6 {
                let length =
                    i32::from_be_bytes([buf[pos + 1], buf[pos + 2], buf[pos + 3], buf[pos + 4]]);
                if length == 5 && buf.len() >= pos + 1 + length as usize {
                    return buf;
                }
            }
        }
    }
}

fn find_last_message_start(buf: &[u8], msg_type: u8) -> Option<usize> {
    if buf.len() < 6 {
        return None;
    }
    (0..=buf.len() - 6).rev().find(|&i| buf[i] == msg_type)
}

fn parse_message_types(buf: &[u8]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let msg_type = buf[i];
        let msg_len = i32::from_be_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]) as usize;
        types.push(msg_type);
        i += 1 + msg_len;
    }
    types
}

async fn setup_connection(port: u16, user: &str) -> TcpStream {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");
    let params = StartupParams::new().with("user", user);
    let startup_bytes = encode_startup_message(&params);
    stream
        .write_all(&startup_bytes)
        .await
        .expect("write startup");
    stream.flush().await.expect("flush");
    let _ = read_until_ready_for_query(&mut stream).await;
    stream
}

async fn send_query_and_read(stream: &mut TcpStream, sql: &str) -> Vec<u8> {
    let mut query_msg = Vec::new();
    query_msg.push(b'Q');
    query_msg.extend_from_slice(&(sql.len() as i32 + 4 + 1).to_be_bytes());
    query_msg.extend_from_slice(sql.as_bytes());
    query_msg.push(0);
    stream.write_all(&query_msg).await.expect("write query");
    stream.flush().await.expect("flush");
    read_until_ready_for_query(stream).await
}

fn extract_data_rows(buf: &[u8]) -> Vec<Vec<Vec<u8>>> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let msg_type = buf[i];
        let msg_len = i32::from_be_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]) as usize;
        if msg_type == MSG_DATA_ROW {
            let payload = &buf[i + 5..i + 1 + msg_len];
            let col_count = i16::from_be_bytes([payload[0], payload[1]]) as usize;
            let mut cols = Vec::with_capacity(col_count);
            let mut p = 2;
            for _ in 0..col_count {
                let col_len = i32::from_be_bytes([
                    payload[p],
                    payload[p + 1],
                    payload[p + 2],
                    payload[p + 3],
                ]);
                p += 4;
                if col_len < 0 {
                    cols.push(Vec::new());
                } else {
                    let len = col_len as usize;
                    cols.push(payload[p..p + len].to_vec());
                    p += len;
                }
            }
            rows.push(cols);
        }
        i += 1 + msg_len;
    }
    rows
}

fn extract_command_complete_tag(buf: &[u8]) -> Option<String> {
    let mut i = 0;
    while i < buf.len() {
        let msg_type = buf[i];
        let msg_len = i32::from_be_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]) as usize;
        if msg_type == MSG_COMMAND_COMPLETE {
            let payload = &buf[i + 5..i + 1 + msg_len];
            let end = payload
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(payload.len());
            return Some(String::from_utf8_lossy(&payload[..end]).to_string());
        }
        i += 1 + msg_len;
    }
    None
}

/// 从 RowDescription 中提取列名列表
fn extract_column_names(buf: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let msg_type = buf[i];
        let msg_len = i32::from_be_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]) as usize;
        if msg_type == MSG_ROW_DESCRIPTION {
            let payload = &buf[i + 5..i + 1 + msg_len];
            let col_count = i16::from_be_bytes([payload[0], payload[1]]) as usize;
            let mut p = 2;
            for _ in 0..col_count {
                // 列名以 NUL 结尾
                let end = payload[p..]
                    .iter()
                    .position(|&b| b == 0)
                    .expect("column name should be NUL-terminated");
                let name = String::from_utf8_lossy(&payload[p..p + end]).to_string();
                names.push(name);
                p += end + 1;
                // 跳过 table_oid(i32=4) + col_attr(i16=2) + type_oid(u32=4) + type_size(i16=2) + type_mod(i32=4) + format(i16=2) = 18 字节
                p += 18;
            }
            return names;
        }
        i += 1 + msg_len;
    }
    names
}

// =====================================================================
//  端到端测试
// =====================================================================

/// 验收场景 1：SELECT * FROM pg_tables 在空数据库中返回空结果集。
#[tokio::test]
async fn test_e2e_pg_tables_empty_database_returns_empty_result() {
    let port = find_free_port(19032).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "pg_empty_user").await;

    let resp = send_query_and_read(&mut stream, "SELECT * FROM pg_tables").await;
    let types = parse_message_types(&resp);

    // 空数据库：RowDescription + CommandComplete + ReadyForQuery（无 DataRow）
    assert_eq!(
        types,
        vec![
            MSG_ROW_DESCRIPTION,
            MSG_COMMAND_COMPLETE,
            MSG_READY_FOR_QUERY,
        ]
    );
    // 列名应为 pg_tables 的 4 列
    let cols = extract_column_names(&resp);
    assert_eq!(cols.len(), 4);
    assert!(cols.iter().any(|c| c.eq_ignore_ascii_case("tablename")));
}

/// 验收场景 2：CREATE TABLE 后 SELECT * FROM pg_tables 返回该表。
#[tokio::test]
async fn test_e2e_pg_tables_after_create_returns_row() {
    let port = find_free_port(19132).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "pg_create_user").await;

    // 准备数据
    send_query_and_read(&mut stream, "CREATE TABLE users (id BIGINT, name TEXT)").await;

    // 查询 pg_tables
    let resp = send_query_and_read(&mut stream, "SELECT * FROM pg_tables").await;
    let types = parse_message_types(&resp);
    assert_eq!(
        types,
        vec![
            MSG_ROW_DESCRIPTION,
            MSG_DATA_ROW,
            MSG_COMMAND_COMPLETE,
            MSG_READY_FOR_QUERY,
        ]
    );

    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 1, "should have 1 table");
    // pg_tables 列顺序：schemaname, tablename, tableowner, hasindexes
    assert_eq!(rows[0].len(), 4);
    // schemaname
    assert_eq!(&rows[0][0], b"public");
    // tablename
    assert_eq!(&rows[0][1], b"users");
    // hasindexes 是 Int64 类型：0 = 无索引，1 = 有索引
    assert!(
        &rows[0][3] == b"0" || &rows[0][3] == b"1",
        "hasindexes should be '0' or '1'"
    );
    assert_eq!(
        extract_command_complete_tag(&resp).as_deref(),
        Some("SELECT 1")
    );
}

/// 验收场景 3：pg_catalog.pg_tables 带 schema 前缀查询。
#[tokio::test]
async fn test_e2e_pg_catalog_schema_prefix_works() {
    let port = find_free_port(19232).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "pg_cat_user").await;

    send_query_and_read(&mut stream, "CREATE TABLE t1 (id BIGINT)").await;

    let resp = send_query_and_read(&mut stream, "SELECT * FROM pg_catalog.pg_tables").await;
    let types = parse_message_types(&resp);
    assert_eq!(
        types,
        vec![
            MSG_ROW_DESCRIPTION,
            MSG_DATA_ROW,
            MSG_COMMAND_COMPLETE,
            MSG_READY_FOR_QUERY,
        ]
    );

    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 1);
    assert_eq!(&rows[0][1], b"t1");
}

/// 验收场景 4：information_schema.tables 返回正确的元数据。
#[tokio::test]
async fn test_e2e_information_schema_tables_returns_metadata() {
    let port = find_free_port(19332).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "info_tables_user").await;

    send_query_and_read(&mut stream, "CREATE TABLE foo (id BIGINT)").await;
    send_query_and_read(&mut stream, "CREATE TABLE bar (id BIGINT)").await;

    let resp = send_query_and_read(&mut stream, "SELECT * FROM information_schema.tables").await;
    let types = parse_message_types(&resp);
    assert_eq!(
        types,
        vec![
            MSG_ROW_DESCRIPTION,
            MSG_DATA_ROW,
            MSG_DATA_ROW,
            MSG_COMMAND_COMPLETE,
            MSG_READY_FOR_QUERY,
        ]
    );

    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 2, "should have 2 tables");
    // information_schema.tables 列：TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE
    for row in &rows {
        assert_eq!(row.len(), 4);
        // TABLE_SCHEMA = "public"
        assert_eq!(&row[1], b"public");
        // TABLE_TYPE = "BASE TABLE"
        assert_eq!(&row[3], b"BASE TABLE");
    }
}

/// 验收场景 5：information_schema.columns 返回正确的列元数据。
#[tokio::test]
async fn test_e2e_information_schema_columns_returns_column_metadata() {
    let port = find_free_port(19432).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "info_cols_user").await;

    send_query_and_read(
        &mut stream,
        "CREATE TABLE typed (id BIGINT, name TEXT, score DOUBLE PRECISION)",
    )
    .await;

    let resp = send_query_and_read(&mut stream, "SELECT * FROM information_schema.columns").await;
    let types = parse_message_types(&resp);
    assert_eq!(
        types,
        vec![
            MSG_ROW_DESCRIPTION,
            MSG_DATA_ROW,
            MSG_DATA_ROW,
            MSG_DATA_ROW,
            MSG_COMMAND_COMPLETE,
            MSG_READY_FOR_QUERY,
        ]
    );

    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 3, "should have 3 columns");

    // 收集所有列名（COLUMN_NAME 是第 4 列，索引 3）
    let col_names: Vec<String> = rows
        .iter()
        .map(|r| String::from_utf8_lossy(&r[3]).to_string())
        .collect();
    assert!(col_names.contains(&"id".to_string()));
    assert!(col_names.contains(&"name".to_string()));
    assert!(col_names.contains(&"score".to_string()));
}

/// 验收场景 6：WHERE 过滤 — 只返回匹配的表。
#[tokio::test]
async fn test_e2e_pg_tables_with_where_filter() {
    let port = find_free_port(19532).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "where_user").await;

    send_query_and_read(&mut stream, "CREATE TABLE alpha (id BIGINT)").await;
    send_query_and_read(&mut stream, "CREATE TABLE beta (id BIGINT)").await;

    // WHERE tablename = 'alpha'
    let resp = send_query_and_read(
        &mut stream,
        "SELECT * FROM pg_tables WHERE tablename = 'alpha'",
    )
    .await;
    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 1, "WHERE should filter to 1 row");
    assert_eq!(&rows[0][1], b"alpha");
}

/// 验收场景 7：WHERE 大小写不敏感。
#[tokio::test]
async fn test_e2e_pg_tables_where_case_insensitive() {
    let port = find_free_port(19632).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "case_user").await;

    send_query_and_read(&mut stream, "CREATE TABLE MyTable (id BIGINT)").await;

    // 使用大写列名 TABLENAME
    let resp = send_query_and_read(
        &mut stream,
        "SELECT * FROM pg_tables WHERE TABLENAME = 'MyTable'",
    )
    .await;
    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 1, "case-insensitive WHERE should match");
    assert_eq!(&rows[0][1], b"MyTable");
}

/// 验收场景 8：ORDER BY 排序。
#[tokio::test]
async fn test_e2e_pg_tables_order_by_tablename() {
    let port = find_free_port(19732).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "order_user").await;

    send_query_and_read(&mut stream, "CREATE TABLE zebra (id BIGINT)").await;
    send_query_and_read(&mut stream, "CREATE TABLE apple (id BIGINT)").await;
    send_query_and_read(&mut stream, "CREATE TABLE mango (id BIGINT)").await;

    let resp = send_query_and_read(
        &mut stream,
        "SELECT * FROM pg_tables ORDER BY tablename ASC",
    )
    .await;
    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 3);
    let names: Vec<String> = rows
        .iter()
        .map(|r| String::from_utf8_lossy(&r[1]).to_string())
        .collect();
    assert_eq!(
        names,
        vec![
            "apple".to_string(),
            "mango".to_string(),
            "zebra".to_string()
        ],
        "ORDER BY should sort alphabetically"
    );
}

/// 验收场景 9：LIMIT 截断结果。
#[tokio::test]
async fn test_e2e_pg_tables_with_limit() {
    let port = find_free_port(19832).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "limit_user").await;

    for name in &["a", "b", "c", "d"] {
        send_query_and_read(&mut stream, &format!("CREATE TABLE {name} (id BIGINT)")).await;
    }

    let resp = send_query_and_read(&mut stream, "SELECT * FROM pg_tables LIMIT 2").await;
    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 2, "LIMIT 2 should return 2 rows");
    assert_eq!(
        extract_command_complete_tag(&resp).as_deref(),
        Some("SELECT 2")
    );
}

/// 验收场景 10：LIMIT + OFFSET 分页。
#[tokio::test]
async fn test_e2e_pg_tables_limit_offset_pagination() {
    let port = find_free_port(19932).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "page_user").await;

    for name in &["a", "b", "c", "d", "e"] {
        send_query_and_read(&mut stream, &format!("CREATE TABLE {name} (id BIGINT)")).await;
    }

    // 排序后取第 2、3 条（OFFSET 1 LIMIT 2）
    let resp = send_query_and_read(
        &mut stream,
        "SELECT * FROM pg_tables ORDER BY tablename LIMIT 2 OFFSET 1",
    )
    .await;
    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 2, "should return 2 rows");
    let names: Vec<String> = rows
        .iter()
        .map(|r| String::from_utf8_lossy(&r[1]).to_string())
        .collect();
    assert_eq!(names, vec!["b".to_string(), "c".to_string()]);
}

/// 验收场景 11：指定列查询（非 SELECT *）。
#[tokio::test]
async fn test_e2e_pg_tables_specific_columns() {
    let port = find_free_port(20032).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "cols_user").await;

    send_query_and_read(&mut stream, "CREATE TABLE spec (id BIGINT)").await;

    let resp = send_query_and_read(&mut stream, "SELECT tablename FROM pg_tables").await;
    let types = parse_message_types(&resp);
    assert_eq!(
        types,
        vec![
            MSG_ROW_DESCRIPTION,
            MSG_DATA_ROW,
            MSG_COMMAND_COMPLETE,
            MSG_READY_FOR_QUERY,
        ]
    );
    let cols = extract_column_names(&resp);
    assert_eq!(cols.len(), 1, "should project only 1 column");
    assert_eq!(cols[0], "tablename");

    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 1, "row should have 1 column");
    assert_eq!(&rows[0][0], b"spec");
}

/// 验收场景 12：列别名查询。
#[tokio::test]
async fn test_e2e_pg_tables_column_alias() {
    let port = find_free_port(20132).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "alias_user").await;

    send_query_and_read(&mut stream, "CREATE TABLE aliased (id BIGINT)").await;

    let resp = send_query_and_read(&mut stream, "SELECT tablename AS name FROM pg_tables").await;
    let cols = extract_column_names(&resp);
    assert_eq!(cols.len(), 1);
    assert_eq!(cols[0], "name", "column alias should be applied");

    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 1);
    assert_eq!(&rows[0][0], b"aliased");
}

/// 验收场景 13：DBeaver 模拟 — 完整浏览流程。
/// 1. 连接后查询所有表
/// 2. 对每个表查询列信息
#[tokio::test]
async fn test_e2e_dbeaver_style_browse_workflow() {
    let port = find_free_port(20232).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "dbeaver_user").await;

    // 创建测试 schema
    send_query_and_read(
        &mut stream,
        "CREATE TABLE users (id BIGINT, name TEXT, email TEXT)",
    )
    .await;
    send_query_and_read(
        &mut stream,
        "CREATE TABLE orders (id BIGINT, user_id BIGINT, total DOUBLE PRECISION)",
    )
    .await;

    // Step 1: 查询所有表（DBeaver 左侧树）
    let resp = send_query_and_read(
        &mut stream,
        "SELECT tablename FROM pg_tables ORDER BY tablename ASC",
    )
    .await;
    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 2, "should list 2 tables");
    let table_names: Vec<String> = rows
        .iter()
        .map(|r| String::from_utf8_lossy(&r[0]).to_string())
        .collect();
    assert_eq!(table_names, vec!["orders".to_string(), "users".to_string()]);

    // Step 2: 对 users 表查询列信息
    let resp = send_query_and_read(
        &mut stream,
        "SELECT column_name, data_type FROM information_schema.columns WHERE table_name = 'users' ORDER BY ordinal_position ASC",
    )
    .await;
    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 3, "users table should have 3 columns");
    assert_eq!(&rows[0][0], b"id");
    assert_eq!(&rows[0][1], b"BIGINT");
    assert_eq!(&rows[1][0], b"name");
    assert_eq!(&rows[1][1], b"TEXT");
    assert_eq!(&rows[2][0], b"email");
    assert_eq!(&rows[2][1], b"TEXT");
}

/// 验收场景 14：JOIN 查询应被拒绝（当前实现不支持）。
#[tokio::test]
async fn test_e2e_system_table_join_falls_back_to_normal_planner() {
    let port = find_free_port(20332).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "join_user").await;

    send_query_and_read(&mut stream, "CREATE TABLE base (id BIGINT)").await;

    // JOIN 查询：不被系统表拦截器处理，走正常 Planner
    // 由于 pg_tables 不在 catalog 中注册，Planner 会返回 TableNotFound 错误
    let resp = send_query_and_read(
        &mut stream,
        "SELECT * FROM pg_tables t1 JOIN pg_tables t2 ON t1.tablename = t2.tablename",
    )
    .await;
    let types = parse_message_types(&resp);
    assert!(
        types.contains(&MSG_ERROR_RESPONSE),
        "JOIN should fall back to Planner and return error (pg_tables not registered): {:?}",
        types
    );
}

/// 验收场景 15：扩展查询协议下也能查询系统表。
#[tokio::test]
async fn test_e2e_system_table_via_extended_query_protocol() {
    use szrsql_protocol::pgwire::message::{
        MSG_BIND, MSG_BIND_COMPLETE, MSG_EXECUTE, MSG_PARSE, MSG_PARSE_COMPLETE, MSG_SYNC,
    };

    let port = find_free_port(20432).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "ext_user").await;

    send_query_and_read(&mut stream, "CREATE TABLE ext_t (id BIGINT)").await;

    // 扩展查询协议：Parse + Bind + Execute + Sync
    // Parse 消息 payload: statement_name(cstring) + sql(cstring) + param_oids_count(i16) + oids
    let sql = "SELECT tablename FROM pg_tables";
    let mut parse_payload = Vec::new();
    parse_payload.push(0); // unnamed statement (NUL terminator only)
    parse_payload.extend_from_slice(sql.as_bytes());
    parse_payload.push(0); // NUL terminator for sql
    parse_payload.extend_from_slice(&0i16.to_be_bytes()); // 0 param oids
    let mut parse_msg = Vec::new();
    parse_msg.push(MSG_PARSE);
    parse_msg.extend_from_slice(&((parse_payload.len() + 4) as i32).to_be_bytes());
    parse_msg.extend_from_slice(&parse_payload);

    // Bind 消息 payload: portal(cstring) + statement(cstring) + param_fmt_count(i16) + param_count(i16) + result_fmt_count(i16)
    let mut bind_payload = Vec::new();
    bind_payload.push(0); // unnamed portal
    bind_payload.push(0); // unnamed statement
    bind_payload.extend_from_slice(&0i16.to_be_bytes()); // 0 param format codes
    bind_payload.extend_from_slice(&0i16.to_be_bytes()); // 0 params
    bind_payload.extend_from_slice(&0i16.to_be_bytes()); // 0 result format codes
    let mut bind_msg = Vec::new();
    bind_msg.push(MSG_BIND);
    bind_msg.extend_from_slice(&((bind_payload.len() + 4) as i32).to_be_bytes());
    bind_msg.extend_from_slice(&bind_payload);

    // Execute 消息 payload: portal(cstring) + max_rows(i32)
    let mut execute_payload = Vec::new();
    execute_payload.push(0); // unnamed portal
    execute_payload.extend_from_slice(&0i32.to_be_bytes()); // max_rows=0 (unlimited)
    let mut execute_msg = Vec::new();
    execute_msg.push(MSG_EXECUTE);
    execute_msg.extend_from_slice(&((execute_payload.len() + 4) as i32).to_be_bytes());
    execute_msg.extend_from_slice(&execute_payload);

    // Sync 消息: Type + Length=4
    let mut sync_msg = Vec::new();
    sync_msg.push(MSG_SYNC);
    sync_msg.extend_from_slice(&4i32.to_be_bytes());

    // 发送全部消息
    stream.write_all(&parse_msg).await.expect("write parse");
    stream.write_all(&bind_msg).await.expect("write bind");
    stream.write_all(&execute_msg).await.expect("write execute");
    stream.write_all(&sync_msg).await.expect("write sync");
    stream.flush().await.expect("flush");

    let resp = read_until_ready_for_query(&mut stream).await;
    let types = parse_message_types(&resp);

    // 期望 ParseComplete + BindComplete + RowDescription + DataRow + CommandComplete + ReadyForQuery
    assert!(
        types.contains(&MSG_PARSE_COMPLETE),
        "should include ParseComplete: {:?}",
        types
    );
    assert!(
        types.contains(&MSG_BIND_COMPLETE),
        "should include BindComplete: {:?}",
        types
    );
    assert!(
        types.contains(&MSG_DATA_ROW),
        "should include DataRow: {:?}",
        types
    );
    assert!(
        types.contains(&MSG_COMMAND_COMPLETE),
        "should include CommandComplete: {:?}",
        types
    );

    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 1, "should have 1 row");
    assert_eq!(&rows[0][0], b"ext_t");
}

/// 验收场景 16：DROP TABLE 后 pg_tables 不再列出该表。
#[tokio::test]
async fn test_e2e_pg_tables_reflects_drop_table() {
    let port = find_free_port(20532).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;
    let mut stream = setup_connection(port, "drop_user").await;

    send_query_and_read(&mut stream, "CREATE TABLE temp_t (id BIGINT)").await;

    // 确认存在
    let resp = send_query_and_read(&mut stream, "SELECT * FROM pg_tables").await;
    assert_eq!(extract_data_rows(&resp).len(), 1);

    // DROP
    send_query_and_read(&mut stream, "DROP TABLE temp_t").await;

    // 应不再列出
    let resp = send_query_and_read(&mut stream, "SELECT * FROM pg_tables").await;
    let types = parse_message_types(&resp);
    assert_eq!(
        types,
        vec![
            MSG_ROW_DESCRIPTION,
            MSG_COMMAND_COMPLETE,
            MSG_READY_FOR_QUERY,
        ]
    );
    let rows = extract_data_rows(&resp);
    assert_eq!(rows.len(), 0, "table should no longer appear after DROP");
}
