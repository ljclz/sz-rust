//! Phase 4.4 端到端集成测试 — trust + SCRAM-SHA-256 认证。
//!
//! 完整覆盖进度表 Phase 4.4 验收标准：
//! > pg_hba.conf 配置 trust → 免密连接
//! > 配置 scram-sha-256 → 密码连接
//! > 两种认证方式均正确
//!
//! 测试覆盖：
//! - trust 模式：连接成功，可执行查询
//! - scram-sha-256 模式：正确密码握手成功，可执行查询
//! - scram-sha-256 模式：错误密码 → ErrorResponse + 连接关闭
//! - scram-sha-256 模式：未知用户 → ErrorResponse + 连接关闭
//! - scram-sha-256 模式：错误的 SASL 机制名 → ErrorResponse
//! - scram-sha-256 模式：客户端首条消息缺少 initial_response → ErrorResponse

use std::collections::HashMap;
use std::time::Duration;
use szrsql_protocol::pgwire::auth::{
    build_client_final_message, build_client_first_message, AuthMode,
};
use szrsql_protocol::pgwire::message::{
    MSG_AUTHENTICATION, MSG_COMMAND_COMPLETE, MSG_DATA_ROW, MSG_ERROR_RESPONSE,
    MSG_READY_FOR_QUERY, MSG_ROW_DESCRIPTION,
};
use szrsql_protocol::pgwire::server::{PgwireConfig, PgwireServer};
use szrsql_protocol::pgwire::startup::{encode_startup_message, StartupParams};
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
async fn spawn_scram_server(
    port: u16,
    credentials: HashMap<String, String>,
    salt: Vec<u8>,
    iterations: u32,
) -> tokio::task::JoinHandle<()> {
    let config = PgwireConfig::new()
        .with_host("127.0.0.1")
        .with_port(port)
        .with_server_version("14.0-test")
        .with_auth_mode(AuthMode::scram_sha256_with_salt(
            credentials,
            salt,
            iterations,
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

/// 读取一条后端消息，返回完整的字节流（Type + Length + Payload）。
async fn read_backend_message_full(stream: &mut TcpStream) -> Vec<u8> {
    let type_byte = read_exact_or_die(stream, 1).await;
    let length_bytes = read_exact_or_die(stream, 4).await;
    let length = i32::from_be_bytes([
        length_bytes[0],
        length_bytes[1],
        length_bytes[2],
        length_bytes[3],
    ]) as usize;
    // length 包含自身 4 字节
    let payload = read_exact_or_die(stream, length - 4).await;
    let mut msg = type_byte;
    msg.extend_from_slice(&length_bytes);
    msg.extend_from_slice(&payload);
    msg
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

/// 编码并发送 StartupMessage。
async fn send_startup(stream: &mut TcpStream, user: &str) {
    let params = StartupParams::new().with("user", user);
    let bytes = encode_startup_message(&params);
    stream.write_all(&bytes).await.expect("write startup");
    stream.flush().await.expect("flush");
}

/// 编码 SASLInitialResponse 消息（Type='p'）。
///
/// 格式：Type='p' + Length + mechanism\0 + i32 initial_response_len + initial_response_bytes
/// Length = 4 + mechanism.len() + 1 + 4 + initial_response.len()
/// 若 initial_response 为 None，则 initial_response_len = -1，无 bytes 部分。
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

/// 编码 SASLResponse 消息（Type='p'）。
///
/// 格式：Type='p' + Length + data
/// Length = 4 + data.len()
fn encode_sasl_response(data: &[u8]) -> Vec<u8> {
    let total_len = (4 + data.len()) as i32;
    let mut msg = Vec::new();
    msg.push(b'p');
    msg.extend_from_slice(&total_len.to_be_bytes());
    msg.extend_from_slice(data);
    msg
}

/// 发送 Query 消息并读取响应直到 ReadyForQuery。
///
/// 返回的缓冲区包含完整的消息字节序列（每条消息：Type + Length + Payload）。
async fn send_query_and_read(stream: &mut TcpStream, sql: &str) -> Vec<u8> {
    let mut query_msg = Vec::new();
    query_msg.push(b'Q');
    query_msg.extend_from_slice(&(sql.len() as i32 + 4 + 1).to_be_bytes());
    query_msg.extend_from_slice(sql.as_bytes());
    query_msg.push(0);
    stream.write_all(&query_msg).await.expect("write query");
    stream.flush().await.expect("flush");

    // 读取直到收到 ReadyForQuery
    let mut buf = Vec::new();
    loop {
        let msg = read_backend_message_full(stream).await;
        let type_byte = msg[0];
        buf.extend_from_slice(&msg);
        if type_byte == MSG_READY_FOR_QUERY {
            return buf;
        }
    }
}

// =====================================================================
//  Trust 模式端到端测试
// =====================================================================

/// 验收场景 1：trust 模式下，Startup 后直接收到 AuthenticationOk，
/// 可执行 SELECT 1 查询。
#[tokio::test]
async fn test_e2e_trust_mode_authentication_and_query() {
    let port = find_free_port(19100).await;
    let _server = spawn_trust_server(port).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 1. 发送 Startup
    send_startup(&mut stream, "alice").await;

    // 2. 读取首条后端消息：应为 AuthenticationOk（'R' + length=8 + auth_code=0）
    let (msg_type, payload) = read_backend_message(&mut stream).await;
    assert_eq!(msg_type, MSG_AUTHENTICATION, "first message should be 'R'");
    assert_eq!(
        payload,
        vec![0, 0, 0, 0],
        "auth code should be 0 (AuthenticationOk)"
    );

    // 3. 继续读取直到 ReadyForQuery
    let mut got_backend_key = false;
    let mut got_ready = false;
    for _ in 0..20 {
        let (t, _) = read_backend_message(&mut stream).await;
        if t == b'K' {
            got_backend_key = true;
        } else if t == MSG_READY_FOR_QUERY {
            got_ready = true;
            break;
        }
    }
    assert!(got_backend_key, "should receive BackendKeyData");
    assert!(got_ready, "should receive ReadyForQuery");

    // 4. 执行 SELECT 1
    let resp = send_query_and_read(&mut stream, "SELECT 1").await;
    // 期望消息序列：RowDescription + DataRow + CommandComplete + ReadyForQuery
    let mut types = Vec::new();
    let mut i = 0;
    // 简化解析：直接查找关键类型字节
    while i < resp.len() {
        types.push(resp[i]);
        if i + 4 < resp.len() {
            let len =
                i32::from_be_bytes([resp[i + 1], resp[i + 2], resp[i + 3], resp[i + 4]]) as usize;
            i += 1 + len;
        } else {
            break;
        }
    }
    assert!(
        types.contains(&MSG_ROW_DESCRIPTION),
        "should have RowDescription"
    );
    assert!(types.contains(&MSG_DATA_ROW), "should have DataRow");
    assert!(
        types.contains(&MSG_COMMAND_COMPLETE),
        "should have CommandComplete"
    );
    assert!(
        types.contains(&MSG_READY_FOR_QUERY),
        "should have ReadyForQuery"
    );
}

// =====================================================================
//  SCRAM-SHA-256 模式端到端测试
// =====================================================================

/// 辅助：完成完整 SCRAM-SHA-256 握手，返回 (stream, server_first, combined_nonce)。
/// 失败时 panic 并打印诊断信息。
async fn scram_handshake(
    stream: &mut TcpStream,
    user: &str,
    password: &str,
    salt: &[u8],
    iterations: u32,
) -> (String, String) {
    // 1. 发送 Startup
    send_startup(stream, user).await;

    // 2. 读取 AuthenticationSASL（'R' + auth_code=10 + 机制列表）
    let (msg_type, payload) = read_backend_message(stream).await;
    assert_eq!(
        msg_type, MSG_AUTHENTICATION,
        "should be Authentication request"
    );
    assert_eq!(
        &payload[0..4],
        &[0, 0, 0, 10],
        "auth code should be 10 (AUTH_SASL)"
    );
    // payload[4..] 应包含 "SCRAM-SHA-256\0\0"
    let mechanisms = &payload[4..];
    assert!(
        mechanisms.starts_with(b"SCRAM-SHA-256\0"),
        "should advertise SCRAM-SHA-256 mechanism, got: {mechanisms:?}"
    );

    // 3. 客户端发送 SASLInitialResponse with client-first
    let client_nonce = "client_test_nonce_001";
    let client_first = build_client_first_message(user, client_nonce);
    let sasl_init = encode_sasl_initial_response("SCRAM-SHA-256", Some(client_first.as_bytes()));
    stream.write_all(&sasl_init).await.expect("write sasl init");
    stream.flush().await.expect("flush");

    // 4. 读取 AuthenticationSASLContinue（'R' + auth_code=11 + server-first）
    let (msg_type, payload) = read_backend_message(stream).await;
    assert_eq!(
        msg_type, MSG_AUTHENTICATION,
        "should be Authentication request"
    );
    assert_eq!(
        &payload[0..4],
        &[0, 0, 0, 11],
        "auth code should be 11 (AUTH_SASL_CONTINUE)"
    );
    let server_first = String::from_utf8(payload[4..].to_vec()).expect("server-first is UTF-8");

    // 提取 combined nonce
    let combined_nonce = server_first
        .split(',')
        .find_map(|p| p.strip_prefix("r=").map(|s| s.to_string()))
        .expect("server-first should contain r=");
    assert!(
        combined_nonce.starts_with(client_nonce),
        "combined nonce should start with client nonce"
    );

    // 5. 客户端构造并发送 SASLResponse with client-final
    let client_first_bare = format!("n={user},r={client_nonce}");
    let client_final = build_client_final_message(
        password,
        salt,
        iterations,
        &client_first_bare,
        &server_first,
        &combined_nonce,
    );
    let sasl_resp = encode_sasl_response(client_final.as_bytes());
    stream
        .write_all(&sasl_resp)
        .await
        .expect("write sasl response");
    stream.flush().await.expect("flush");

    (server_first, combined_nonce)
}

/// 辅助：读取握手后的剩余消息（AuthenticationSASLFinal + AuthenticationOk + ParameterStatus* + BackendKeyData + ReadyForQuery）。
async fn read_post_handshake_response(stream: &mut TcpStream) -> bool {
    let mut got_auth_ok = false;
    let mut got_backend_key = false;
    let mut got_ready = false;
    let mut got_sasl_final = false;

    for _ in 0..30 {
        let (t, payload) = read_backend_message(stream).await;
        match t {
            MSG_AUTHENTICATION => {
                let auth_code =
                    u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                match auth_code {
                    12 => got_sasl_final = true,
                    0 => got_auth_ok = true,
                    _ => panic!("unexpected auth code: {auth_code}"),
                }
            }
            b'K' => got_backend_key = true,
            MSG_READY_FOR_QUERY => {
                got_ready = true;
                break;
            }
            _ => {}
        }
    }
    got_sasl_final && got_auth_ok && got_backend_key && got_ready
}

/// 验收场景 2：scram-sha-256 模式下，正确密码 → 握手成功 → 可执行查询。
#[tokio::test]
async fn test_e2e_scram_correct_password_authenticates_and_queries() {
    let port = find_free_port(19200).await;

    let mut creds = HashMap::new();
    creds.insert("alice".to_string(), "secret123".to_string());
    let salt = b"0123456789abcdef".to_vec();
    let iterations = 4096u32;

    let _server = spawn_scram_server(port, creds, salt.clone(), iterations).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 完成握手
    let (_server_first, _combined) =
        scram_handshake(&mut stream, "alice", "secret123", &salt, iterations).await;

    // 读取握手后的响应
    let ok = read_post_handshake_response(&mut stream).await;
    assert!(
        ok,
        "post-handshake should include SASLFinal + AuthOk + BackendKey + ReadyForQuery"
    );

    // 执行 SELECT 1
    let resp = send_query_and_read(&mut stream, "SELECT 1").await;
    let mut types = Vec::new();
    let mut i = 0;
    while i < resp.len() {
        types.push(resp[i]);
        if i + 4 < resp.len() {
            let len =
                i32::from_be_bytes([resp[i + 1], resp[i + 2], resp[i + 3], resp[i + 4]]) as usize;
            i += 1 + len;
        } else {
            break;
        }
    }
    assert!(types.contains(&MSG_DATA_ROW), "should have DataRow");
    assert!(
        types.contains(&MSG_COMMAND_COMPLETE),
        "should have CommandComplete"
    );
}

/// 验收场景 3：scram-sha-256 模式下，错误密码 → ErrorResponse → 连接关闭。
#[tokio::test]
async fn test_e2e_scram_wrong_password_returns_error() {
    let port = find_free_port(19300).await;

    let mut creds = HashMap::new();
    creds.insert("alice".to_string(), "secret123".to_string());
    let salt = b"0123456789abcdef".to_vec();
    let iterations = 4096u32;

    let _server = spawn_scram_server(port, creds, salt.clone(), iterations).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 完成握手（使用错误密码）
    let _ = scram_handshake(&mut stream, "alice", "WRONG_PASSWORD", &salt, iterations).await;

    // 应收到 ErrorResponse 而非 SASLFinal + AuthOk
    let (msg_type, _payload) = read_backend_message(&mut stream).await;
    assert_eq!(
        msg_type, MSG_ERROR_RESPONSE,
        "wrong password should result in ErrorResponse"
    );

    // 连接应被关闭（读取返回 0 字节或错误）
    let mut buf = [0u8; 64];
    let result = stream.read(&mut buf).await;
    assert!(
        result.is_err() || result.unwrap() == 0,
        "connection should be closed after auth failure"
    );
}

/// 验收场景 4：scram-sha-256 模式下，未知用户 → ErrorResponse。
#[tokio::test]
async fn test_e2e_scram_unknown_user_returns_error() {
    let port = find_free_port(19400).await;

    let mut creds = HashMap::new();
    creds.insert("alice".to_string(), "secret123".to_string());
    let salt = b"0123456789abcdef".to_vec();
    let iterations = 4096u32;

    let _server = spawn_scram_server(port, creds, salt.clone(), iterations).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 1. 发送 Startup（使用未知用户 eve）
    send_startup(&mut stream, "eve").await;

    // 2. 读取 AuthenticationSASL
    let (msg_type, payload) = read_backend_message(&mut stream).await;
    assert_eq!(msg_type, MSG_AUTHENTICATION);
    assert_eq!(&payload[0..4], &[0, 0, 0, 10]);

    // 3. 发送 SASLInitialResponse with unknown user
    let client_nonce = "cn_unknown_user";
    let client_first = build_client_first_message("eve", client_nonce);
    let sasl_init = encode_sasl_initial_response("SCRAM-SHA-256", Some(client_first.as_bytes()));
    stream.write_all(&sasl_init).await.expect("write sasl init");
    stream.flush().await.expect("flush");

    // 4. 应收到 ErrorResponse（UserNotFound）
    let (msg_type, _payload) = read_backend_message(&mut stream).await;
    assert_eq!(
        msg_type, MSG_ERROR_RESPONSE,
        "unknown user should result in ErrorResponse"
    );
}

/// 验收场景 5：scram-sha-256 模式下，客户端发送错误的 SASL 机制名 → ErrorResponse。
#[tokio::test]
async fn test_e2e_scram_wrong_mechanism_returns_error() {
    let port = find_free_port(19500).await;

    let mut creds = HashMap::new();
    creds.insert("alice".to_string(), "secret123".to_string());
    let salt = b"0123456789abcdef".to_vec();
    let iterations = 4096u32;

    let _server = spawn_scram_server(port, creds, salt, iterations).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 1. 发送 Startup
    send_startup(&mut stream, "alice").await;

    // 2. 读取 AuthenticationSASL
    let (_type, _payload) = read_backend_message(&mut stream).await;

    // 3. 发送 SASLInitialResponse with 错误机制名 "PLAIN"
    let client_first = build_client_first_message("alice", "cn");
    let sasl_init = encode_sasl_initial_response("PLAIN", Some(client_first.as_bytes()));
    stream.write_all(&sasl_init).await.expect("write sasl init");
    stream.flush().await.expect("flush");

    // 4. 应收到 ErrorResponse（UnsupportedMechanism）
    let (msg_type, _payload) = read_backend_message(&mut stream).await;
    assert_eq!(
        msg_type, MSG_ERROR_RESPONSE,
        "unsupported SASL mechanism should result in ErrorResponse"
    );
}

/// 验收场景 6：scram-sha-256 模式下，客户端首条消息缺少 initial_response → ErrorResponse。
#[tokio::test]
async fn test_e2e_scram_missing_initial_response_returns_error() {
    let port = find_free_port(19600).await;

    let mut creds = HashMap::new();
    creds.insert("alice".to_string(), "secret123".to_string());
    let salt = b"0123456789abcdef".to_vec();
    let iterations = 4096u32;

    let _server = spawn_scram_server(port, creds, salt, iterations).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 1. 发送 Startup
    send_startup(&mut stream, "alice").await;

    // 2. 读取 AuthenticationSASL
    let (_type, _payload) = read_backend_message(&mut stream).await;

    // 3. 发送 SASLInitialResponse with initial_response = None
    let sasl_init = encode_sasl_initial_response("SCRAM-SHA-256", None);
    stream.write_all(&sasl_init).await.expect("write sasl init");
    stream.flush().await.expect("flush");

    // 4. 应收到 ErrorResponse
    let (msg_type, _payload) = read_backend_message(&mut stream).await;
    assert_eq!(
        msg_type, MSG_ERROR_RESPONSE,
        "missing initial_response should result in ErrorResponse"
    );
}

/// 验收场景 7：scram-sha-256 用户名大小写不敏感 — "ALICE" 应能匹配小写 "alice" 凭据。
#[tokio::test]
async fn test_e2e_scram_username_case_insensitive() {
    let port = find_free_port(19700).await;

    let mut creds = HashMap::new();
    creds.insert("alice".to_string(), "secret123".to_string());
    let salt = b"0123456789abcdef".to_vec();
    let iterations = 4096u32;

    let _server = spawn_scram_server(port, creds, salt.clone(), iterations).await;
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    // 使用大写 ALICE 完成握手
    let _ = scram_handshake(&mut stream, "ALICE", "secret123", &salt, iterations).await;

    // 应握手成功
    let ok = read_post_handshake_response(&mut stream).await;
    assert!(ok, "username case-insensitive matching should succeed");
}

/// 验收场景 8：trust 模式下，默认配置（未显式设置 auth_mode）应等同于 trust。
#[tokio::test]
async fn test_e2e_default_auth_mode_is_trust() {
    let port = find_free_port(19800).await;
    // 使用默认配置（不调用 with_auth_mode）
    let config = PgwireConfig::new()
        .with_host("127.0.0.1")
        .with_port(port)
        .with_server_version("14.0-test");
    let server = PgwireServer::new(config);
    let _handle = tokio::spawn(async move {
        let _ = server.serve().await;
    });
    wait_for_server(port).await;

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect should succeed");

    send_startup(&mut stream, "any_user").await;

    // 应直接收到 AuthenticationOk
    let (msg_type, payload) = read_backend_message(&mut stream).await;
    assert_eq!(msg_type, MSG_AUTHENTICATION);
    assert_eq!(
        payload,
        vec![0, 0, 0, 0],
        "default mode should be Trust (AuthenticationOk)"
    );
}
