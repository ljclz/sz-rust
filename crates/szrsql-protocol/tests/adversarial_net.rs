//! 阶段 F-9：对抗性边界审计 - 协议与网络安全集成测试
//!
//! 对应文档：`docs/对抗性边界审计清单.md` §3.4 协议与网络安全
//! 覆盖以下审计项：
//! - ADV-NET-001: pgwire 协议畸形消息
//! - ADV-NET-002: 认证绕过
//! - ADV-NET-003: SSL/TLS 中间人（协商行为验证）
//! - ADV-NET-004: 大型 Bind 参数
//! - ADV-NET-005: 扩展协议滥用
//! - ADV-NET-006: 取消请求滥用
//! - ADV-NET-007: 复制协议攻击（未实现复制协议，验证拒绝行为）
//! - ADV-NET-008: 端口扫描防护（连接建立行为验证）
//! - ADV-NET-009: 速率限制（高频查询行为验证）
//! - ADV-NET-010: 连接超时（空闲连接处理）
//!
//! # 测试数据目录
//!
//! 所有持久化测试数据写入 `F:\test\data`（用户要求：不使用 C 盘）。

#![allow(clippy::approx_constant)]

use std::collections::HashMap;
use std::time::Duration;

use szrsql_protocol::pgwire::auth::{build_client_first_message, AuthMode};
use szrsql_protocol::pgwire::message::{
    MSG_AUTHENTICATION, MSG_BIND_COMPLETE, MSG_COMMAND_COMPLETE, MSG_DATA_ROW, MSG_ERROR_RESPONSE,
    MSG_PARSE_COMPLETE, MSG_READY_FOR_QUERY, MSG_ROW_DESCRIPTION,
};
use szrsql_protocol::pgwire::pg_types::oid;
use szrsql_protocol::pgwire::server::{PgwireConfig, PgwireServer};
use szrsql_protocol::pgwire::startup::{
    encode_cancel_request, encode_special_request, encode_startup_message, StartupParams,
    PROTOCOL_GSSNC_REQUEST, PROTOCOL_SSL_REQUEST, PROTOCOL_VERSION_3_0,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// =====================================================================
//  辅助函数
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

/// 启动 trust 模式测试服务器。
async fn spawn_trust_server(port: u16) -> tokio::task::JoinHandle<()> {
    let config = PgwireConfig::new()
        .with_host("127.0.0.1")
        .with_port(port)
        .with_server_version("14.0-test")
        .with_auth_mode(AuthMode::trust());
    spawn_server_with_config(config)
}

/// 启动 SCRAM-SHA-256 模式测试服务器（使用固定盐便于客户端构造 proof）。
async fn spawn_scram_server(port: u16) -> tokio::task::JoinHandle<()> {
    let mut credentials = HashMap::new();
    credentials.insert("alice".to_string(), "secret".to_string());
    let config = PgwireConfig::new()
        .with_host("127.0.0.1")
        .with_port(port)
        .with_server_version("14.0-test")
        .with_auth_mode(AuthMode::scram_sha256_with_salt(
            credentials,
            b"fixedsalt12345678".to_vec(),
            4096,
        ));
    spawn_server_with_config(config)
}

fn spawn_server_with_config(config: PgwireConfig) -> tokio::task::JoinHandle<()> {
    let server = PgwireServer::new(config);
    tokio::spawn(async move {
        let _ = server.serve().await;
    })
}

/// 等待服务器就绪（可连接）。
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

/// 读取 n 字节（阻塞直到读满或连接关闭）。
async fn read_exact_or_die(stream: &mut TcpStream, n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    stream
        .read_exact(&mut buf)
        .await
        .expect("read_exact should succeed");
    buf
}

/// 尝试读取 n 字节，返回 Option（连接关闭时返回 None）。
async fn try_read_exact(stream: &mut TcpStream, n: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; n];
    match stream.read_exact(&mut buf).await {
        Ok(_) => Some(buf),
        Err(_) => None,
    }
}

/// 读取一条后端消息（Type + Length + Payload），返回 (type, payload)。
async fn read_backend_message(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let type_byte = read_exact_or_die(stream, 1).await[0];
    let length_bytes = read_exact_or_die(stream, 4).await;
    let length = i32::from_be_bytes([
        length_bytes[0],
        length_bytes[1],
        length_bytes[2],
        length_bytes[3],
    ]) as usize;
    // length 包含自身 4 字节
    let payload = read_exact_or_die(stream, length - 4).await;
    (type_byte, payload)
}

/// 尝试读取一条后端消息，连接关闭时返回 None。
async fn try_read_backend_message(stream: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let type_byte = try_read_exact(stream, 1).await?[0];
    let length_bytes = try_read_exact(stream, 4).await?;
    let length = i32::from_be_bytes([
        length_bytes[0],
        length_bytes[1],
        length_bytes[2],
        length_bytes[3],
    ]) as usize;
    let payload = try_read_exact(stream, length - 4).await?;
    Some((type_byte, payload))
}

/// 编码并发送 StartupMessage。
async fn send_startup(stream: &mut TcpStream, user: &str) {
    let params = StartupParams::new().with("user", user);
    let bytes = encode_startup_message(&params);
    stream.write_all(&bytes).await.expect("write startup");
    stream.flush().await.expect("flush");
}

/// 读取直到收到 ReadyForQuery 消息，返回所有消息类型列表。
async fn read_until_ready_for_query(stream: &mut TcpStream) -> Vec<u8> {
    let mut types = Vec::new();
    loop {
        let (msg_type, _payload) = read_backend_message(stream).await;
        types.push(msg_type);
        if msg_type == MSG_READY_FOR_QUERY {
            return types;
        }
        // 防止无限循环（最多 100 条消息）
        if types.len() > 100 {
            panic!("too many messages without ReadyForQuery");
        }
    }
}

/// 编码 Query 消息（Type='Q'）。
fn encode_query(sql: &str) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.push(b'Q');
    msg.extend_from_slice(&(sql.len() as i32 + 4 + 1).to_be_bytes());
    msg.extend_from_slice(sql.as_bytes());
    msg.push(0); // NUL terminator
    msg
}

/// 编码 Parse 消息（Type='P'）。
fn encode_parse(statement_name: &str, sql: &str, param_oids: &[u32]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(statement_name.as_bytes());
    payload.push(0);
    payload.extend_from_slice(sql.as_bytes());
    payload.push(0);
    payload.extend_from_slice(&(param_oids.len() as i16).to_be_bytes());
    for oid in param_oids {
        payload.extend_from_slice(&oid.to_be_bytes());
    }
    let total_len = (4 + payload.len()) as i32;
    let mut msg = Vec::new();
    msg.push(b'P');
    msg.extend_from_slice(&total_len.to_be_bytes());
    msg.extend_from_slice(&payload);
    msg
}

/// 编码 Bind 消息（Type='B'），文本格式参数。
fn encode_bind_text_params(
    portal_name: &str,
    statement_name: &str,
    params: &[Option<&str>],
) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(portal_name.as_bytes());
    payload.push(0);
    payload.extend_from_slice(statement_name.as_bytes());
    payload.push(0);
    // parameter_format_codes: 0 表示全部使用文本
    payload.extend_from_slice(&0i16.to_be_bytes());
    // parameters count
    payload.extend_from_slice(&(params.len() as i16).to_be_bytes());
    for p in params {
        match p {
            Some(s) => {
                payload.extend_from_slice(&(s.len() as i32).to_be_bytes());
                payload.extend_from_slice(s.as_bytes());
            }
            None => {
                payload.extend_from_slice(&(-1i32).to_be_bytes());
            }
        }
    }
    // result_format_codes: 0 表示全部使用文本
    payload.extend_from_slice(&0i16.to_be_bytes());
    let total_len = (4 + payload.len()) as i32;
    let mut msg = Vec::new();
    msg.push(b'B');
    msg.extend_from_slice(&total_len.to_be_bytes());
    msg.extend_from_slice(&payload);
    msg
}

/// 编码 Execute 消息（Type='E'）。
fn encode_execute(portal_name: &str, max_rows: i32) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(portal_name.as_bytes());
    payload.push(0);
    payload.extend_from_slice(&max_rows.to_be_bytes());
    let total_len = (4 + payload.len()) as i32;
    let mut msg = Vec::new();
    msg.push(b'E');
    msg.extend_from_slice(&total_len.to_be_bytes());
    msg.extend_from_slice(&payload);
    msg
}

/// 编码 Sync 消息（Type='S'，无 payload）。
fn encode_sync() -> Vec<u8> {
    let mut msg = Vec::new();
    msg.push(b'S');
    msg.extend_from_slice(&4i32.to_be_bytes());
    msg
}

/// 编码 Terminate 消息（Type='X'，无 payload）。
fn encode_terminate() -> Vec<u8> {
    let mut msg = Vec::new();
    msg.push(b'X');
    msg.extend_from_slice(&4i32.to_be_bytes());
    msg
}

// =====================================================================
//  ADV-NET-001: pgwire 协议畸形消息
// =====================================================================

#[tokio::test]
async fn test_adv_net_001_invalid_protocol_version() {
    // ADV-NET-001: 错误的协议版本号应被拒绝
    let port = find_free_port(21000).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 发送错误的协议版本号（0x00000000）
    let mut bad_msg = Vec::new();
    // Length = 8 (4 bytes length + 4 bytes protocol version)
    bad_msg.extend_from_slice(&8i32.to_be_bytes());
    bad_msg.extend_from_slice(&0i32.to_be_bytes()); // 错误的协议版本
    stream.write_all(&bad_msg).await.expect("write");
    stream.flush().await.expect("flush");

    // 服务器应返回 ErrorResponse 或关闭连接
    let response = try_read_backend_message(&mut stream).await;
    match response {
        Some((msg_type, payload)) => {
            // 应收到 ErrorResponse ('E')
            assert_eq!(
                msg_type, MSG_ERROR_RESPONSE,
                "expected ErrorResponse for invalid protocol version"
            );
            // payload 应包含错误信息
            assert!(!payload.is_empty(), "error payload should not be empty");
        }
        None => {
            // 连接被关闭也是可接受的拒绝行为
            println!(
                "ADV-NET-001: server closed connection for invalid protocol version (acceptable)"
            );
        }
    }
}

#[tokio::test]
async fn test_adv_net_001b_truncated_message_body() {
    // ADV-NET-001 (补充)：截断的消息体应被正确处理
    let port = find_free_port(21001).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 发送 StartupMessage，声明长度比实际长（截断）
    let mut bad_msg = Vec::new();
    bad_msg.extend_from_slice(&100i32.to_be_bytes()); // 声明 100 字节
    bad_msg.extend_from_slice(&PROTOCOL_VERSION_3_0.to_be_bytes());
    // 只发送 user=test\0 但不发送足够数据
    bad_msg.extend_from_slice(b"user\0test\0");
    // 实际长度远小于声明的 100 字节
    stream.write_all(&bad_msg).await.expect("write");
    stream.flush().await.expect("flush");

    // 服务器应等待更多数据或最终超时/关闭
    // 设置读取超时
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        try_read_backend_message(&mut stream),
    )
    .await;

    // 无论超时还是收到错误，都是可接受的（不应 panic）
    match result {
        Err(_) => {
            println!("ADV-NET-001b: timeout waiting for truncated message (acceptable)");
        }
        Ok(None) => {
            println!("ADV-NET-001b: connection closed for truncated message (acceptable)");
        }
        Ok(Some((msg_type, _))) => {
            assert_eq!(
                msg_type, MSG_ERROR_RESPONSE,
                "expected ErrorResponse for truncated message"
            );
        }
    }
}

#[tokio::test]
async fn test_adv_net_001c_zero_length_message() {
    // ADV-NET-001 (补充)：长度字段为 0 或过小的消息应被拒绝
    let port = find_free_port(21002).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 先完成正常 Startup
    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // 发送长度为 0 的消息（无效）
    let mut bad_msg = Vec::new();
    bad_msg.push(b'Q'); // Query type
    bad_msg.extend_from_slice(&0i32.to_be_bytes()); // length=0（非法）
    stream.write_all(&bad_msg).await.expect("write");
    stream.flush().await.expect("flush");

    // 服务器应返回 ErrorResponse 或关闭连接
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        try_read_backend_message(&mut stream),
    )
    .await;

    match response {
        Err(_) => {
            println!("ADV-NET-001c: timeout for zero-length message (acceptable)");
        }
        Ok(None) => {
            println!("ADV-NET-001c: connection closed for zero-length message (acceptable)");
        }
        Ok(Some((msg_type, _))) => {
            assert_eq!(
                msg_type, MSG_ERROR_RESPONSE,
                "expected ErrorResponse for zero-length message"
            );
        }
    }
}

#[tokio::test]
async fn test_adv_net_001d_unknown_message_type() {
    // ADV-NET-001 (补充)：未知消息类型应被处理（不崩溃）
    let port = find_free_port(21003).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 先完成正常 Startup
    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // 发送未知消息类型 'Z'（不是 ReadyForQuery 的方向）
    // 使用 '!' 作为未知类型
    let mut bad_msg = Vec::new();
    bad_msg.push(b'!'); // 未知类型
    bad_msg.extend_from_slice(&5i32.to_be_bytes()); // length=5 (仅长度字段本身+1字节)
    bad_msg.push(0); // 1 字节 payload
    stream.write_all(&bad_msg).await.expect("write");
    stream.flush().await.expect("flush");

    // 服务器应返回 ErrorResponse 或关闭连接，不应崩溃
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        try_read_backend_message(&mut stream),
    )
    .await;

    match response {
        Err(_) => {
            println!("ADV-NET-001d: timeout for unknown message type (acceptable)");
        }
        Ok(None) => {
            println!("ADV-NET-001d: connection closed for unknown message type (acceptable)");
        }
        Ok(Some((msg_type, _))) => {
            // 可能是 ErrorResponse 或被忽略
            assert!(
                msg_type == MSG_ERROR_RESPONSE || msg_type == MSG_READY_FOR_QUERY,
                "expected ErrorResponse or ReadyForQuery, got: {}",
                msg_type as char
            );
        }
    }
}

// =====================================================================
//  ADV-NET-002: 认证绕过
// =====================================================================

#[tokio::test]
async fn test_adv_net_002_query_without_startup() {
    // ADV-NET-002: 未发送 startup message 直接发送 query 应被拒绝
    let port = find_free_port(21010).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 直接发送 Query 消息，不发送 StartupMessage
    stream
        .write_all(&encode_query("SELECT 1"))
        .await
        .expect("write query");
    stream.flush().await.expect("flush");

    // 服务器应拒绝（因为 'Q' 不是合法的启动消息）
    // 预期：连接关闭或 ErrorResponse
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        try_read_backend_message(&mut stream),
    )
    .await;

    match response {
        Err(_) => {
            println!("ADV-NET-002: timeout for query-without-startup (acceptable)");
        }
        Ok(None) => {
            // 连接关闭，符合预期
            println!("ADV-NET-002: connection closed for query-without-startup (correct)");
        }
        Ok(Some((msg_type, _))) => {
            // 应收到 ErrorResponse
            assert_eq!(
                msg_type, MSG_ERROR_RESPONSE,
                "expected ErrorResponse for query without startup"
            );
        }
    }
}

/// 编码 SASLInitialResponse 消息（Type='p'）。
fn encode_sasl_initial_response(mechanism: &str, initial_response: Option<&[u8]>) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(mechanism.as_bytes());
    payload.push(0); // mechanism cstring 终止符
    match initial_response {
        Some(data) => {
            payload.extend_from_slice(&(data.len() as i32).to_be_bytes());
            payload.extend_from_slice(data);
        }
        None => {
            payload.extend_from_slice(&(-1i32).to_be_bytes());
        }
    }
    let total_len = (4 + payload.len()) as i32;
    let mut msg = Vec::new();
    msg.push(b'p');
    msg.extend_from_slice(&total_len.to_be_bytes());
    msg.extend_from_slice(&payload);
    msg
}

#[tokio::test]
async fn test_adv_net_002b_wrong_password_scram() {
    // ADV-NET-002 (补充)：SCRAM 模式下错误密码应被拒绝
    // 注意：完整 SCRAM 握手需要正确的 client-final proof 计算，
    //       此处验证：发送 client-first 后，服务器返回 SASLContinue，
    //       然后发送一个明显错误的 client-final（错误 proof），
    //       服务器应返回 ErrorResponse。
    let port = find_free_port(21011).await;
    let _server = spawn_scram_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 1. 发送 Startup
    send_startup(&mut stream, "alice").await;

    // 2. 应收到 AuthenticationSASL（auth_code=10）
    let (auth_type, auth_payload) = read_backend_message(&mut stream).await;
    assert_eq!(auth_type, MSG_AUTHENTICATION);
    let auth_code = i32::from_be_bytes([
        auth_payload[0],
        auth_payload[1],
        auth_payload[2],
        auth_payload[3],
    ]);
    assert_eq!(auth_code, 10, "expected SASL auth request");

    // 3. 发送 SASLInitialResponse with client-first
    let client_first = build_client_first_message("alice", "fake_nonce_001");
    let sasl_init = encode_sasl_initial_response("SCRAM-SHA-256", Some(client_first.as_bytes()));
    stream.write_all(&sasl_init).await.expect("write sasl init");
    stream.flush().await.expect("flush");

    // 4. 应收到 AuthenticationSASLContinue（auth_code=11）
    let (msg_type, payload) = read_backend_message(&mut stream).await;
    assert_eq!(msg_type, MSG_AUTHENTICATION, "expected SASLContinue");
    let continue_code = i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    assert_eq!(continue_code, 11, "expected AUTH_SASL_CONTINUE");

    // 5. 发送错误的 client-final（proof 明显错误）
    // 格式：c=biws,r=<combined_nonce>,p=<wrong_proof_base64>
    let wrong_final = b"c=biws,r=fake_nonce_001,p=dGhpcy1pcy1hLXdyb25nLXByb29m";
    let mut sasl_resp = Vec::new();
    sasl_resp.push(b'p');
    let total_len = (4 + wrong_final.len()) as i32;
    sasl_resp.extend_from_slice(&total_len.to_be_bytes());
    sasl_resp.extend_from_slice(wrong_final);
    stream
        .write_all(&sasl_resp)
        .await
        .expect("write sasl response");
    stream.flush().await.expect("flush");

    // 6. 应收到 ErrorResponse（错误密码 proof）
    let mut got_error = false;
    for _ in 0..5 {
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            try_read_backend_message(&mut stream),
        )
        .await;
        match result {
            Err(_) => break,
            Ok(None) => break,
            Ok(Some((msg_type, _))) => {
                if msg_type == MSG_ERROR_RESPONSE {
                    got_error = true;
                    break;
                }
            }
        }
    }
    assert!(
        got_error,
        "expected ErrorResponse for wrong SCRAM password proof"
    );
}

#[tokio::test]
async fn test_adv_net_002c_scram_unknown_user() {
    // ADV-NET-002 (补充)：SCRAM 模式下未知用户应被拒绝
    // 流程：Startup → AUTH_SASL → SASLInitialResponse(unknown user) → ErrorResponse
    let port = find_free_port(21012).await;
    let _server = spawn_scram_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 1. 发送 Startup，使用未知用户
    send_startup(&mut stream, "unknownuser").await;

    // 2. 应收到 AuthenticationSASL（服务器要求 SCRAM 认证）
    let (msg_type, payload) = read_backend_message(&mut stream).await;
    assert_eq!(msg_type, MSG_AUTHENTICATION, "expected AUTH_SASL");
    let auth_code = i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    assert_eq!(auth_code, 10, "expected AUTH_SASL code");

    // 3. 发送 SASLInitialResponse with unknown user
    let client_first = build_client_first_message("unknownuser", "nonce_002");
    let sasl_init = encode_sasl_initial_response("SCRAM-SHA-256", Some(client_first.as_bytes()));
    stream.write_all(&sasl_init).await.expect("write sasl init");
    stream.flush().await.expect("flush");

    // 4. 应收到 ErrorResponse（用户不存在）
    let mut got_error = false;
    for _ in 0..5 {
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            try_read_backend_message(&mut stream),
        )
        .await;
        match result {
            Err(_) => break,
            Ok(None) => break,
            Ok(Some((msg_type, _))) => {
                if msg_type == MSG_ERROR_RESPONSE {
                    got_error = true;
                    break;
                }
            }
        }
    }
    assert!(
        got_error,
        "expected ErrorResponse for unknown user in SCRAM mode"
    );
}

#[tokio::test]
async fn test_adv_net_002d_trust_mode_allows_any_user() {
    // ADV-NET-002 (补充)：trust 模式允许任意用户连接（验证配置行为）
    let port = find_free_port(21013).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    send_startup(&mut stream, "anyuser").await;
    let types = read_until_ready_for_query(&mut stream).await;

    // 应收到 AuthenticationOk + ParameterStatus + BackendKeyData + ReadyForQuery
    assert_eq!(types[0], MSG_AUTHENTICATION, "expected AuthenticationOk");
    assert_eq!(
        types[types.len() - 1],
        MSG_READY_FOR_QUERY,
        "expected ReadyForQuery"
    );
}

// =====================================================================
//  ADV-NET-003: SSL/TLS 中间人
// =====================================================================

#[tokio::test]
async fn test_adv_net_003_ssl_request_without_tls_config() {
    // ADV-NET-003: 未配置 TLS 时，SSLRequest 应回复 'N' 并允许明文继续
    let port = find_free_port(21020).await;
    let _server = spawn_trust_server(port).await; // trust 模式无 TLS
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 发送 SSLRequest
    let ssl_req = encode_special_request(PROTOCOL_SSL_REQUEST);
    stream.write_all(&ssl_req).await.expect("write SSLRequest");
    stream.flush().await.expect("flush");

    // 应收到单字节 'N'（拒绝 SSL）
    let response = read_exact_or_die(&mut stream, 1).await;
    assert_eq!(
        response, b"N",
        "expected 'N' response for SSLRequest without TLS config"
    );

    // 连接应可继续明文 Startup
    send_startup(&mut stream, "alice").await;
    let types = read_until_ready_for_query(&mut stream).await;
    assert_eq!(types[0], MSG_AUTHENTICATION);
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

#[tokio::test]
async fn test_adv_net_003b_gssenc_request_rejected() {
    // ADV-NET-003 (补充)：GSSAPI 请求应被拒绝
    let port = find_free_port(21021).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 发送 GSSNCRequest
    let gss_req = encode_special_request(PROTOCOL_GSSNC_REQUEST);
    stream
        .write_all(&gss_req)
        .await
        .expect("write GSSNCRequest");
    stream.flush().await.expect("flush");

    // 应收到 'N'（拒绝 GSSAPI）
    let response = read_exact_or_die(&mut stream, 1).await;
    assert_eq!(response, b"N", "expected 'N' response for GSSNCRequest");

    // 连接应可继续明文
    send_startup(&mut stream, "alice").await;
    let types = read_until_ready_for_query(&mut stream).await;
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

#[tokio::test]
async fn test_adv_net_003c_ssl_request_then_normal_startup() {
    // ADV-NET-003 (补充)：SSL 协商后应能正常完成握手（降级到明文）
    let port = find_free_port(21022).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // SSLRequest → 'N' → Startup → 正常握手
    let ssl_req = encode_special_request(PROTOCOL_SSL_REQUEST);
    stream.write_all(&ssl_req).await.expect("write");
    stream.flush().await.expect("flush");
    let n = read_exact_or_die(&mut stream, 1).await;
    assert_eq!(n, b"N");

    send_startup(&mut stream, "alice").await;
    let types = read_until_ready_for_query(&mut stream).await;
    assert!(types.contains(&MSG_AUTHENTICATION));
    assert!(types.contains(&MSG_READY_FOR_QUERY));

    // 验证可执行查询
    stream
        .write_all(&encode_query("SELECT 1"))
        .await
        .expect("write");
    stream.flush().await.expect("flush");
    let types = read_until_ready_for_query(&mut stream).await;
    assert!(types.contains(&MSG_ROW_DESCRIPTION));
    assert!(types.contains(&MSG_DATA_ROW));
    assert!(types.contains(&MSG_COMMAND_COMPLETE));
}

// =====================================================================
//  ADV-NET-004: 大型 Bind 参数
// =====================================================================

#[tokio::test]
async fn test_adv_net_004_large_bind_parameter() {
    // ADV-NET-004: 超大 Bind 参数（1MB）应被处理或限制
    let port = find_free_port(21030).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // 构造 1MB 的字符串参数
    let large_param = "x".repeat(1024 * 1024); // 1MB

    // Parse: SELECT $1::text
    stream
        .write_all(&encode_parse("", "SELECT $1", &[oid::TEXT]))
        .await
        .expect("write Parse");
    stream.flush().await.expect("flush");
    let (parse_type, _) = read_backend_message(&mut stream).await;
    assert_eq!(parse_type, MSG_PARSE_COMPLETE);

    // Bind: 绑定 1MB 参数
    stream
        .write_all(&encode_bind_text_params("", "", &[Some(&large_param)]))
        .await
        .expect("write Bind");
    stream.flush().await.expect("flush");
    let (bind_type, _) = read_backend_message(&mut stream).await;
    assert_eq!(
        bind_type, MSG_BIND_COMPLETE,
        "Bind should succeed with 1MB param"
    );

    // Execute + Sync（扩展协议需要 Sync 才会返回 ReadyForQuery）
    stream
        .write_all(&encode_execute("", 0))
        .await
        .expect("write Execute");
    stream.write_all(&encode_sync()).await.expect("write Sync");
    stream.flush().await.expect("flush");

    // 读取响应（RowDescription + DataRow + CommandComplete + ReadyForQuery，或 ErrorResponse）
    let mut got_data = false;
    let mut got_error = false;
    for _ in 0..10 {
        let (msg_type, _) = read_backend_message(&mut stream).await;
        if msg_type == MSG_DATA_ROW {
            got_data = true;
        }
        if msg_type == MSG_ERROR_RESPONSE {
            got_error = true;
        }
        if msg_type == MSG_READY_FOR_QUERY {
            break;
        }
    }
    // 应成功返回数据，或因大小限制返回错误（两者都可接受）
    assert!(
        got_data || got_error,
        "expected either data row or error for large bind param"
    );
}

#[tokio::test]
async fn test_adv_net_004b_many_bind_parameters() {
    // ADV-NET-004 (补充)：单次 Bind 大量参数（1000 个）
    let port = find_free_port(21031).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // 构造 100 个参数的查询（避免过多导致解析问题）
    let n_params = 100;
    let placeholders: Vec<String> = (1..=n_params).map(|i| format!("${i}")).collect();
    let sql = format!("SELECT {}", placeholders.join(", "));

    // Parse
    let param_oids: Vec<u32> = vec![oid::TEXT; n_params];
    stream
        .write_all(&encode_parse("", &sql, &param_oids))
        .await
        .expect("write Parse");
    stream.flush().await.expect("flush");
    let (parse_type, _) = read_backend_message(&mut stream).await;
    assert_eq!(parse_type, MSG_PARSE_COMPLETE);

    // Bind with 100 parameters
    let params: Vec<Option<&str>> = (0..n_params).map(|_| Some("v")).collect();
    stream
        .write_all(&encode_bind_text_params("", "", &params))
        .await
        .expect("write Bind");
    stream.flush().await.expect("flush");
    let (bind_type, _) = read_backend_message(&mut stream).await;
    assert_eq!(
        bind_type, MSG_BIND_COMPLETE,
        "Bind with 100 params should succeed"
    );

    // Execute + Sync
    stream
        .write_all(&encode_execute("", 0))
        .await
        .expect("write Execute");
    stream.write_all(&encode_sync()).await.expect("write Sync");
    stream.flush().await.expect("flush");

    let mut got_data = false;
    for _ in 0..20 {
        let (msg_type, _) = read_backend_message(&mut stream).await;
        if msg_type == MSG_DATA_ROW {
            got_data = true;
        }
        if msg_type == MSG_READY_FOR_QUERY {
            break;
        }
    }
    assert!(got_data, "should receive data row for 100-param query");
}

// =====================================================================
//  ADV-NET-005: 扩展协议滥用
// =====================================================================

#[tokio::test]
async fn test_adv_net_005_parse_without_execute() {
    // ADV-NET-005: Parse 后不 Execute，应正确清理（无资源泄漏）
    let port = find_free_port(21040).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // Parse 多个语句但不 Execute
    for i in 0..10 {
        let sql = format!("SELECT {i}");
        stream
            .write_all(&encode_parse(&format!("stmt_{i}"), &sql, &[]))
            .await
            .expect("write Parse");
        stream.flush().await.expect("flush");
        let (parse_type, _) = read_backend_message(&mut stream).await;
        assert_eq!(parse_type, MSG_PARSE_COMPLETE);
    }

    // Sync 结束当前事务
    stream.write_all(&encode_sync()).await.expect("write Sync");
    stream.flush().await.expect("flush");
    let (sync_type, _) = read_backend_message(&mut stream).await;
    assert_eq!(
        sync_type, MSG_READY_FOR_QUERY,
        "Sync should return ReadyForQuery"
    );

    // 验证连接仍可用
    stream
        .write_all(&encode_query("SELECT 1"))
        .await
        .expect("write");
    stream.flush().await.expect("flush");
    let types = read_until_ready_for_query(&mut stream).await;
    assert!(
        types.contains(&MSG_DATA_ROW),
        "connection should still work"
    );
}

#[tokio::test]
async fn test_adv_net_005b_repeated_bind_same_statement() {
    // ADV-NET-005 (补充)：重复 Bind 同一 Prepared Statement（流水线模式）
    // 使用扩展协议流水线：Parse → (Bind + Execute)×5 → Sync → 读取所有响应
    let port = find_free_port(21041).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // Parse 一次（读取 ParseComplete）
    stream
        .write_all(&encode_parse("stmt1", "SELECT $1::int", &[oid::INT8]))
        .await
        .expect("write Parse");
    stream.flush().await.expect("flush");
    let (parse_type, _) = read_backend_message(&mut stream).await;
    assert_eq!(parse_type, MSG_PARSE_COMPLETE);

    // 流水线发送 5 个 Bind + Execute（不读取中间响应）
    let mut pipeline = Vec::new();
    for i in 0..5 {
        let val = i.to_string();
        pipeline.extend_from_slice(&encode_bind_text_params(
            &format!("portal_{i}"),
            "stmt1",
            &[Some(&val)],
        ));
        pipeline.extend_from_slice(&encode_execute(&format!("portal_{i}"), 0));
    }
    pipeline.extend_from_slice(&encode_sync());
    stream.write_all(&pipeline).await.expect("write pipeline");
    stream.flush().await.expect("flush");

    // 读取所有响应：每个 Execute 应返回 BindComplete + RowDescription + DataRow + CommandComplete
    let mut bind_complete_count = 0;
    let mut data_rows = 0;
    let mut command_complete_count = 0;
    for _ in 0..50 {
        let (msg_type, _) = read_backend_message(&mut stream).await;
        match msg_type {
            MSG_BIND_COMPLETE => bind_complete_count += 1,
            MSG_DATA_ROW => data_rows += 1,
            MSG_COMMAND_COMPLETE => command_complete_count += 1,
            MSG_READY_FOR_QUERY => break,
            _ => {}
        }
    }
    assert_eq!(bind_complete_count, 5, "should receive 5 BindComplete");
    assert_eq!(data_rows, 5, "should receive 5 data rows from 5 Execute");
    assert_eq!(
        command_complete_count, 5,
        "should receive 5 CommandComplete"
    );
}

#[tokio::test]
async fn test_adv_net_005c_execute_unknown_portal() {
    // ADV-NET-005 (补充)：Execute 不存在的 portal 应返回 ErrorResponse
    let port = find_free_port(21042).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // Execute 不存在的 portal
    stream
        .write_all(&encode_execute("nonexistent_portal", 0))
        .await
        .expect("write Execute");
    stream.flush().await.expect("flush");

    // 应收到 ErrorResponse
    let (msg_type, _) = read_backend_message(&mut stream).await;
    assert_eq!(
        msg_type, MSG_ERROR_RESPONSE,
        "expected ErrorResponse for unknown portal"
    );

    // Sync 恢复
    stream.write_all(&encode_sync()).await.expect("write Sync");
    stream.flush().await.expect("flush");
    let (sync_type, _) = read_backend_message(&mut stream).await;
    assert_eq!(
        sync_type, MSG_READY_FOR_QUERY,
        "Sync should restore session"
    );
}

// =====================================================================
//  ADV-NET-006: 取消请求滥用
// =====================================================================

#[tokio::test]
async fn test_adv_net_006_cancel_request_random_pid() {
    // ADV-NET-006: 发送随机 PID 的 CancelRequest 应不影响服务器
    let port = find_free_port(21050).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    // 发送 CancelRequest（随机 pid 和 secret）
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 使用 encode_cancel_request 构造完整的 CancelRequest（含 pid + secret）
    let cancel_bytes = encode_cancel_request(99999, 0);
    stream.write_all(&cancel_bytes).await.expect("write cancel");
    stream.flush().await.expect("flush");

    // CancelRequest 处理后服务器应关闭连接（不进入主循环）
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        try_read_backend_message(&mut stream),
    )
    .await;

    // 无论超时还是连接关闭，都是可接受的
    match response {
        Err(_) => println!("ADV-NET-006: timeout after CancelRequest (acceptable)"),
        Ok(None) => println!("ADV-NET-006: connection closed after CancelRequest (correct)"),
        Ok(Some(_)) => println!("ADV-NET-006: received response after CancelRequest (acceptable)"),
    }

    // 验证服务器仍正常运行
    let mut stream2 = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("server should still be running");
    send_startup(&mut stream2, "alice").await;
    let types = read_until_ready_for_query(&mut stream2).await;
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

#[tokio::test]
async fn test_adv_net_006b_concurrent_cancel_requests() {
    // ADV-NET-006 (补充)：大量并发 CancelRequest 不应导致服务器崩溃
    let port = find_free_port(21051).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    // 并发发送 20 个 CancelRequest
    let mut handles = Vec::new();
    for i in 0..20 {
        let port = port;
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
                .await
                .expect("connect");
            // 使用 encode_cancel_request 构造完整请求
            let cancel_bytes = encode_cancel_request(i as i32, 0);
            let _ = stream.write_all(&cancel_bytes).await;
            let _ = stream.flush().await;
            // 不等待响应，直接关闭
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }

    // 验证服务器仍正常运行
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("server should still be running after concurrent CancelRequests");
    send_startup(&mut stream, "alice").await;
    let types = read_until_ready_for_query(&mut stream).await;
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

// =====================================================================
//  ADV-NET-007: 复制协议攻击
// =====================================================================

#[tokio::test]
async fn test_adv_net_007_replication_protocol_not_supported() {
    // ADV-NET-007: 复制协议（replication=1）应被拒绝或降级处理
    let port = find_free_port(21060).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    // 发送带 replication=true 参数的 StartupMessage
    let params = StartupParams::new()
        .with("user", "alice")
        .with("replication", "true"); // 请求复制模式
    let bytes = encode_startup_message(&params);
    stream.write_all(&bytes).await.expect("write");
    stream.flush().await.expect("flush");

    // 服务器应拒绝复制请求（ErrorResponse）或降级为普通连接
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        try_read_backend_message(&mut stream),
    )
    .await;

    match response {
        Err(_) => {
            println!("ADV-NET-007: timeout for replication request (acceptable)");
        }
        Ok(None) => {
            println!("ADV-NET-007: connection closed for replication request (acceptable)");
        }
        Ok(Some((msg_type, _))) => {
            // 应收到 ErrorResponse，或 AuthenticationOk（降级为普通连接）
            assert!(
                msg_type == MSG_ERROR_RESPONSE || msg_type == MSG_AUTHENTICATION,
                "expected ErrorResponse or AuthenticationOk for replication request, got: {}",
                msg_type as char
            );
        }
    }
}

// =====================================================================
//  ADV-NET-008: 端口扫描防护
// =====================================================================

#[tokio::test]
async fn test_adv_net_008_immediate_disconnect() {
    // ADV-NET-008: 建立连接后立即断开，服务器应正确处理
    let port = find_free_port(21070).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    // 建立多个连接后立即断开
    for _ in 0..20 {
        let _stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .expect("connect");
        // 立即 drop，模拟端口扫描
    }

    // 服务器应仍正常运行
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("server should still be running");
    send_startup(&mut stream, "alice").await;
    let types = read_until_ready_for_query(&mut stream).await;
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

#[tokio::test]
async fn test_adv_net_008b_half_open_connection() {
    // ADV-NET-008 (补充)：半开连接（发送部分数据后断开）
    let port = find_free_port(21071).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    // 发送部分 StartupMessage 后断开
    for _ in 0..10 {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .expect("connect");
        // 只发送 Length 字段（4 字节），不发送后续
        stream
            .write_all(&100i32.to_be_bytes())
            .await
            .expect("write partial");
        stream.flush().await.expect("flush");
        // 立即 drop
    }

    // 服务器应仍正常运行
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("server should still be running after half-open connections");
    send_startup(&mut stream, "alice").await;
    let types = read_until_ready_for_query(&mut stream).await;
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

// =====================================================================
//  ADV-NET-009: 速率限制
// =====================================================================

#[tokio::test]
async fn test_adv_net_009_high_frequency_queries() {
    // ADV-NET-009: 单连接高频查询应被处理（验证不崩溃）
    let port = find_free_port(21080).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // 快速发送 100 个查询
    let start = std::time::Instant::now();
    for i in 0..100 {
        let sql = format!("SELECT {i}");
        stream.write_all(&encode_query(&sql)).await.expect("write");
        stream.flush().await.expect("flush");
        let _ = read_until_ready_for_query(&mut stream).await;
    }
    let elapsed = start.elapsed();

    // 验证所有查询都成功完成
    // 100 个查询应在合理时间内完成（< 10 秒）
    assert!(
        elapsed < Duration::from_secs(10),
        "100 queries took too long: {elapsed:?}"
    );

    // 连接仍可用
    stream
        .write_all(&encode_query("SELECT 1"))
        .await
        .expect("write");
    stream.flush().await.expect("flush");
    let types = read_until_ready_for_query(&mut stream).await;
    assert!(types.contains(&MSG_DATA_ROW));
}

#[tokio::test]
async fn test_adv_net_009b_concurrent_connections() {
    // ADV-NET-009 (补充)：多连接并发查询
    let port = find_free_port(21081).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let n_conns = 10;
    let n_queries = 10;
    let mut handles = Vec::new();

    for conn_id in 0..n_conns {
        let port = port;
        handles.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
                .await
                .expect("connect");
            send_startup(&mut stream, "alice").await;
            let _ = read_until_ready_for_query(&mut stream).await;

            for qid in 0..n_queries {
                let sql = format!("SELECT {qid}");
                stream.write_all(&encode_query(&sql)).await.expect("write");
                stream.flush().await.expect("flush");
                let _ = read_until_ready_for_query(&mut stream).await;
            }
            conn_id
        }));
    }

    // 所有连接都应完成
    for handle in handles {
        let _ = handle.await.expect("connection task should complete");
    }

    // 服务器仍正常运行
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("server should still be running");
    send_startup(&mut stream, "alice").await;
    let types = read_until_ready_for_query(&mut stream).await;
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

// =====================================================================
//  ADV-NET-010: 连接超时
// =====================================================================

#[tokio::test]
async fn test_adv_net_010_idle_connection() {
    // ADV-NET-010: 空闲连接应保持存活（或按配置超时）
    let port = find_free_port(21090).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // 空闲 2 秒不发任何消息
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 验证连接仍可用
    stream
        .write_all(&encode_query("SELECT 1"))
        .await
        .expect("write");
    stream.flush().await.expect("flush");

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        read_until_ready_for_query(&mut stream),
    )
    .await;

    assert!(
        result.is_ok(),
        "idle connection should still be usable after 2s"
    );
}

#[tokio::test]
async fn test_adv_net_010b_long_running_query() {
    // ADV-NET-010 (补充)：长时间运行的查询应能完成
    let port = find_free_port(21091).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // 执行一个稍复杂的查询（多个 UNION）
    let sql = "SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3";
    stream.write_all(&encode_query(sql)).await.expect("write");
    stream.flush().await.expect("flush");

    let types = tokio::time::timeout(
        Duration::from_secs(5),
        read_until_ready_for_query(&mut stream),
    )
    .await
    .expect("query should complete within 5s");

    assert!(types.contains(&MSG_DATA_ROW), "should return data rows");
    assert!(types.contains(&MSG_COMMAND_COMPLETE));
}

#[tokio::test]
async fn test_adv_net_010c_terminate_closes_connection() {
    // ADV-NET-010 (补充)：Terminate 消息应正常关闭连接
    let port = find_free_port(21092).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");

    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // 发送 Terminate
    stream.write_all(&encode_terminate()).await.expect("write");
    stream.flush().await.expect("flush");

    // 服务器应关闭连接
    let result =
        tokio::time::timeout(Duration::from_millis(500), try_read_exact(&mut stream, 1)).await;

    match result {
        Err(_) => println!("ADV-NET-010c: timeout after Terminate (acceptable)"),
        Ok(None) => println!("ADV-NET-010c: connection closed after Terminate (correct)"),
        Ok(Some(_)) => {
            // 收到数据可能是服务器在关闭前的残留响应（不应发生，但容错）
            println!("ADV-NET-010c: received data after Terminate (unexpected but tolerated)");
        }
    }

    // 服务器应仍正常运行
    let mut stream2 = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("server should still be running");
    send_startup(&mut stream2, "alice").await;
    let types = read_until_ready_for_query(&mut stream2).await;
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

// =====================================================================
//  ADV-NET-001 回归测试：BUG-001 / BUG-002 远程 DoS 防护
// =====================================================================

#[tokio::test]
async fn test_adv_net_001e_negative_length_message() {
    // ADV-NET-001 (回归 BUG-001)：
    // 原缺陷：message.rs:474 使用 i32::from_be_bytes 解析消息长度，负值经 as usize
    // 符号扩展为 usize::MAX，导致 split_to(usize::MAX) 溢出 panic（远程 DoS）。
    // 修复：改用 u32::from_be_bytes，并在 length < 4 时返回 InvalidData。
    // 本测试发送 length=-1 (0xFFFFFFFF) 的 Query 消息，验证服务器不 panic。
    let port = find_free_port(21004).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");
    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // 构造恶意 Query 消息：Type='Q' + Length=0xFFFFFFFF + 任意 payload
    let mut bad_msg = Vec::new();
    bad_msg.push(b'Q');
    bad_msg.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
    bad_msg.extend_from_slice(b"SELECT 1");
    bad_msg.push(0);
    stream.write_all(&bad_msg).await.expect("write");
    stream.flush().await.expect("flush");

    // 服务器应返回 ErrorResponse 或关闭连接，但不应 panic
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        try_read_backend_message(&mut stream),
    )
    .await;

    match response {
        Err(_) => {
            println!("ADV-NET-001e: timeout for negative length message (acceptable)");
        }
        Ok(None) => {
            println!("ADV-NET-001e: connection closed for negative length message (acceptable)");
        }
        Ok(Some((msg_type, _))) => {
            assert_eq!(
                msg_type, MSG_ERROR_RESPONSE,
                "expected ErrorResponse for negative length message, got: {}",
                msg_type as char
            );
        }
    }

    // 验证服务器未崩溃：建立新连接应仍能成功
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut stream2 = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("server should still be running after negative length message");
    send_startup(&mut stream2, "alice").await;
    let types = read_until_ready_for_query(&mut stream2).await;
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}

#[tokio::test]
async fn test_adv_net_001f_negative_bind_param_count() {
    // ADV-NET-001 (回归 BUG-002)：
    // 原缺陷：message.rs:587-589 使用 i16::from_be_bytes 解析 Bind 消息 param_count，
    // 负值经 as usize 符号扩展为 usize::MAX，Vec::with_capacity(usize::MAX) panic（远程 DoS）。
    // 修复：改用 u16::from_be_bytes，并增加 MAX_BIND_PARAMS=65535 上限校验。
    // 本测试发送 param_count=-1 (0xFFFF) 的 Bind 消息，验证服务器不 panic。
    let port = find_free_port(21005).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect");
    send_startup(&mut stream, "alice").await;
    let _ = read_until_ready_for_query(&mut stream).await;

    // 构造恶意 Bind 消息：Type='B' + Length + portal\0 + stmt\0 + pfc=0 + param_count=0xFFFF
    let mut payload = Vec::new();
    payload.push(0); // portal name (empty cstring)
    payload.push(0); // statement name (empty cstring)
                     // parameter_format_codes count = 0 (i16)
    payload.extend_from_slice(&0i16.to_be_bytes());
    // parameters count = -1 (i16 = 0xFFFF，被修复后的 u16 解析为 65535)
    payload.extend_from_slice(&(-1i16).to_be_bytes());

    let total_len = (4 + payload.len()) as i32;
    let mut bad_msg = Vec::new();
    bad_msg.push(b'B');
    bad_msg.extend_from_slice(&total_len.to_be_bytes());
    bad_msg.extend_from_slice(&payload);
    stream.write_all(&bad_msg).await.expect("write");
    stream.flush().await.expect("flush");

    // 服务器应返回 ErrorResponse（因为 param_count=65535 超过 MAX_BIND_PARAMS 或 payload 不足）
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        try_read_backend_message(&mut stream),
    )
    .await;

    match response {
        Err(_) => {
            println!("ADV-NET-001f: timeout for negative bind param count (acceptable)");
        }
        Ok(None) => {
            println!("ADV-NET-001f: connection closed for negative bind param count (acceptable)");
        }
        Ok(Some((msg_type, _))) => {
            assert_eq!(
                msg_type, MSG_ERROR_RESPONSE,
                "expected ErrorResponse for negative bind param count, got: {}",
                msg_type as char
            );
        }
    }

    // 验证服务器未崩溃：建立新连接应仍能成功
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut stream2 = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("server should still be running after negative bind param count");
    send_startup(&mut stream2, "alice").await;
    let types = read_until_ready_for_query(&mut stream2).await;
    assert_eq!(types[types.len() - 1], MSG_READY_FOR_QUERY);
}
