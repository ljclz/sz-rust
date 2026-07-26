//! Phase 4.6 端到端集成测试 — LISTEN/NOTIFY/UNLISTEN 跨会话通知。
//!
//! 覆盖验收场景：
//! - 单会话 LISTEN + NOTIFY 自身收到通知
//! - 跨会话 NOTIFY：会话 A LISTEN，会话 B NOTIFY，A 收到通知
//! - UNLISTEN 取消监听后不再收到通知
//! - UNLISTEN * 取消所有监听
//! - NOTIFY 无监听者时静默成功
//! - NOTIFY 带负载字符串
//! - 多会话监听同一频道，NOTIFY 全部收到
//! - 通知格式校验：NotificationResponse（'A'）含 pid + channel + payload
//!
//! 这些测试通过启动实际 TCP 服务器 + 多个模拟客户端验证跨会话通知投递。

use std::time::Duration;
use szrsql_protocol::pgwire::{
    message::{MSG_COMMAND_COMPLETE, MSG_NOTIFICATION_RESPONSE},
    server::{PgwireConfig, PgwireServer},
    startup::{encode_startup_message, StartupParams},
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

/// 启动一个测试服务器，返回其监听端口。
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

/// 读取直到收到 ReadyForQuery 消息，返回所有收到的字节。
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

/// 在缓冲区中反向查找指定类型的消息起始位置。
fn find_last_message_start(buf: &[u8], msg_type: u8) -> Option<usize> {
    if buf.len() < 6 {
        return None;
    }
    (0..=buf.len() - 6).rev().find(|&i| buf[i] == msg_type)
}

/// 解析后端响应字节流，返回按顺序的消息类型列表。
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

/// 辅助：执行 startup 握手并返回已建立连接的 stream。
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

/// 辅助：发送一条 Query 并读取响应直到 ReadyForQuery。
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

/// 从响应字节中提取所有 NotificationResponse（'A'）消息的 (pid, channel, payload)。
fn extract_notifications(buf: &[u8]) -> Vec<(i32, String, String)> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < buf.len() {
        let msg_type = buf[i];
        let msg_len = i32::from_be_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]) as usize;
        if msg_type == MSG_NOTIFICATION_RESPONSE {
            // payload: i32 pid + cstring channel + cstring payload
            let payload_start = i + 5;
            let pid = i32::from_be_bytes([
                buf[payload_start],
                buf[payload_start + 1],
                buf[payload_start + 2],
                buf[payload_start + 3],
            ]);
            // 找 channel cstring
            let channel_start = payload_start + 4;
            let channel_end = buf[channel_start..]
                .iter()
                .position(|&b| b == 0)
                .expect("channel cstring terminator");
            let channel =
                String::from_utf8(buf[channel_start..channel_start + channel_end].to_vec())
                    .expect("channel utf8");
            // 找 payload cstring
            let payload_start = channel_start + channel_end + 1;
            let payload_end = buf[payload_start..]
                .iter()
                .position(|&b| b == 0)
                .expect("payload cstring terminator");
            let payload =
                String::from_utf8(buf[payload_start..payload_start + payload_end].to_vec())
                    .expect("payload utf8");
            result.push((pid, channel, payload));
        }
        i += 1 + msg_len;
    }
    result
}

// =====================================================================
//  Phase 4.6 端到端集成测试
// =====================================================================

/// 验收场景 1：单会话 LISTEN + NOTIFY 自身收到通知。
///
/// PG 语义：会话执行 LISTEN 后，自身 NOTIFY 同一频道时也会收到通知。
#[tokio::test]
async fn test_e2e_listen_notify_self_receives_notification() {
    let port = find_free_port(16432).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut stream = setup_connection(port, "alice").await;

    // LISTEN events
    let resp = send_query_and_read(&mut stream, "LISTEN events").await;
    let types = parse_message_types(&resp);
    assert!(
        types.contains(&MSG_COMMAND_COMPLETE),
        "LISTEN should return CommandComplete, got {:?}",
        types
    );

    // NOTIFY events, 'hello' — 应在响应中收到自身的通知
    let resp = send_query_and_read(&mut stream, "NOTIFY events, 'hello'").await;
    let types = parse_message_types(&resp);
    // CommandComplete（NOTIFY）+ NotificationResponse（自身监听）+ ReadyForQuery
    assert!(
        types.contains(&MSG_COMMAND_COMPLETE),
        "NOTIFY should return CommandComplete, got {:?}",
        types
    );
    assert!(
        types.contains(&MSG_NOTIFICATION_RESPONSE),
        "self-NOTIFY should produce NotificationResponse, got {:?}",
        types
    );

    let notifications = extract_notifications(&resp);
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].1, "events");
    assert_eq!(notifications[0].2, "hello");
}

/// 验收场景 2：跨会话 NOTIFY — A LISTEN，B NOTIFY，A 收到通知。
#[tokio::test]
async fn test_e2e_cross_session_notify_delivered() {
    let port = find_free_port(16532).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut conn_a = setup_connection(port, "alice").await;
    let mut conn_b = setup_connection(port, "bob").await;

    // A: LISTEN events
    let _ = send_query_and_read(&mut conn_a, "LISTEN events").await;

    // B: NOTIFY events, 'from-bob'
    let resp_b = send_query_and_read(&mut conn_b, "NOTIFY events, 'from-bob'").await;
    let types_b = parse_message_types(&resp_b);
    assert!(
        types_b.contains(&MSG_COMMAND_COMPLETE),
        "B's NOTIFY should return CommandComplete, got {:?}",
        types_b
    );
    // B 未监听 events，不应收到通知
    assert!(
        !types_b.contains(&MSG_NOTIFICATION_RESPONSE),
        "B should not receive notification (not listening), got {:?}",
        types_b
    );

    // A 发送一个 SELECT 以触发 ReadyForQuery（这会让 A 收到 pending 通知）
    // 实际上 A 的下一次 Query 响应中应包含通知
    let resp_a = send_query_and_read(&mut conn_a, "SELECT 1").await;
    let types_a = parse_message_types(&resp_a);
    assert!(
        types_a.contains(&MSG_NOTIFICATION_RESPONSE),
        "A should receive pending notification on next query, got {:?}",
        types_a
    );

    let notifications = extract_notifications(&resp_a);
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].1, "events");
    assert_eq!(notifications[0].2, "from-bob");
}

/// 验收场景 3：UNLISTEN 取消监听后不再收到通知。
#[tokio::test]
async fn test_e2e_unlisten_stops_notifications() {
    let port = find_free_port(16632).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut conn_a = setup_connection(port, "alice").await;
    let mut conn_b = setup_connection(port, "bob").await;

    // A: LISTEN events
    let _ = send_query_and_read(&mut conn_a, "LISTEN events").await;

    // A: UNLISTEN events
    let _ = send_query_and_read(&mut conn_a, "UNLISTEN events").await;

    // B: NOTIFY events
    let _ = send_query_and_read(&mut conn_b, "NOTIFY events, 'after-unlisten'").await;

    // A: 查询不应收到通知
    let resp_a = send_query_and_read(&mut conn_a, "SELECT 1").await;
    let types_a = parse_message_types(&resp_a);
    assert!(
        !types_a.contains(&MSG_NOTIFICATION_RESPONSE),
        "A should not receive notification after UNLISTEN, got {:?}",
        types_a
    );
}

/// 验收场景 4：UNLISTEN * 取消所有监听。
#[tokio::test]
async fn test_e2e_unlisten_all_stops_all_notifications() {
    let port = find_free_port(16732).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut conn_a = setup_connection(port, "alice").await;
    let mut conn_b = setup_connection(port, "bob").await;

    // A: LISTEN ch1 + LISTEN ch2
    let _ = send_query_and_read(&mut conn_a, "LISTEN ch1").await;
    let _ = send_query_and_read(&mut conn_a, "LISTEN ch2").await;

    // A: UNLISTEN *
    let _ = send_query_and_read(&mut conn_a, "UNLISTEN *").await;

    // B: NOTIFY ch1 + NOTIFY ch2
    let _ = send_query_and_read(&mut conn_b, "NOTIFY ch1, 'a'").await;
    let _ = send_query_and_read(&mut conn_b, "NOTIFY ch2, 'b'").await;

    // A: 不应收到任何通知
    let resp_a = send_query_and_read(&mut conn_a, "SELECT 1").await;
    let types_a = parse_message_types(&resp_a);
    assert!(
        !types_a.contains(&MSG_NOTIFICATION_RESPONSE),
        "A should not receive any notification after UNLISTEN *, got {:?}",
        types_a
    );
}

/// 验收场景 5：NOTIFY 无监听者时静默成功（仍返回 CommandComplete）。
#[tokio::test]
async fn test_e2e_notify_no_listeners_succeeds_silently() {
    let port = find_free_port(16832).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut stream = setup_connection(port, "alice").await;

    let resp = send_query_and_read(&mut stream, "NOTIFY orphan, 'nobody'").await;
    let types = parse_message_types(&resp);
    assert!(
        types.contains(&MSG_COMMAND_COMPLETE),
        "NOTIFY should return CommandComplete even without listeners, got {:?}",
        types
    );
    assert!(
        !types.contains(&MSG_NOTIFICATION_RESPONSE),
        "NOTIFY without listeners should not produce NotificationResponse, got {:?}",
        types
    );
}

/// 验收场景 6：NOTIFY 不带负载（默认空字符串）。
#[tokio::test]
async fn test_e2e_notify_empty_payload() {
    let port = find_free_port(16932).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut stream = setup_connection(port, "alice").await;

    let _ = send_query_and_read(&mut stream, "LISTEN ch").await;
    let resp = send_query_and_read(&mut stream, "NOTIFY ch").await;

    let notifications = extract_notifications(&resp);
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].1, "ch");
    assert_eq!(notifications[0].2, "");
}

/// 验收场景 7：多会话监听同一频道，NOTIFY 全部收到。
#[tokio::test]
async fn test_e2e_multiple_listeners_all_receive_notification() {
    let port = find_free_port(17032).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut conn_a = setup_connection(port, "alice").await;
    let mut conn_b = setup_connection(port, "bob").await;
    let mut conn_c = setup_connection(port, "carol").await;

    // 三方都 LISTEN events
    let _ = send_query_and_read(&mut conn_a, "LISTEN events").await;
    let _ = send_query_and_read(&mut conn_b, "LISTEN events").await;
    let _ = send_query_and_read(&mut conn_c, "LISTEN events").await;

    // A 发送 NOTIFY
    let resp_a = send_query_and_read(&mut conn_a, "NOTIFY events, 'broadcast'").await;
    // A 应收到自身通知
    let a_notifications = extract_notifications(&resp_a);
    assert_eq!(a_notifications.len(), 1);

    // B 在下次查询时应收到通知
    let resp_b = send_query_and_read(&mut conn_b, "SELECT 1").await;
    let b_notifications = extract_notifications(&resp_b);
    assert_eq!(b_notifications.len(), 1);
    assert_eq!(b_notifications[0].1, "events");
    assert_eq!(b_notifications[0].2, "broadcast");

    // C 在下次查询时应收到通知
    let resp_c = send_query_and_read(&mut conn_c, "SELECT 1").await;
    let c_notifications = extract_notifications(&resp_c);
    assert_eq!(c_notifications.len(), 1);
    assert_eq!(c_notifications[0].1, "events");
    assert_eq!(c_notifications[0].2, "broadcast");
}

/// 验收场景 8：NotificationResponse 消息格式校验（pid + channel + payload）。
#[tokio::test]
async fn test_e2e_notification_response_message_format() {
    let port = find_free_port(17132).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut stream = setup_connection(port, "alice").await;

    let _ = send_query_and_read(&mut stream, "LISTEN fmt").await;
    let resp = send_query_and_read(&mut stream, "NOTIFY fmt, 'payload-data'").await;

    let notifications = extract_notifications(&resp);
    assert_eq!(notifications.len(), 1);
    let (pid, channel, payload) = &notifications[0];
    // pid 应为正整数（由服务器 pid_counter 分配）
    assert!(*pid > 0, "notification pid should be positive, got {pid}");
    assert_eq!(channel, "fmt");
    assert_eq!(payload, "payload-data");
}

/// 验收场景 9：重复 LISTEN 同一频道幂等（只收到一次通知）。
#[tokio::test]
async fn test_e2e_duplicate_listen_is_idempotent() {
    let port = find_free_port(17232).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut stream = setup_connection(port, "alice").await;

    // 重复 LISTEN 同一频道
    let _ = send_query_and_read(&mut stream, "LISTEN dup").await;
    let _ = send_query_and_read(&mut stream, "LISTEN dup").await;
    let _ = send_query_and_read(&mut stream, "LISTEN dup").await;

    // NOTIFY 应只收到一次通知
    let resp = send_query_and_read(&mut stream, "NOTIFY dup, 'once'").await;
    let notifications = extract_notifications(&resp);
    assert_eq!(
        notifications.len(),
        1,
        "duplicate LISTEN should only produce 1 notification, got {:?}",
        notifications
    );
}

/// 验收场景 10：UNLISTEN 未监听的频道是幂等的（不报错）。
#[tokio::test]
async fn test_e2e_unlisten_nonexistent_channel_is_idempotent() {
    let port = find_free_port(17332).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut stream = setup_connection(port, "alice").await;

    // UNLISTEN 未监听的频道应成功返回 CommandComplete
    let resp = send_query_and_read(&mut stream, "UNLISTEN never_listened").await;
    let types = parse_message_types(&resp);
    assert!(
        types.contains(&MSG_COMMAND_COMPLETE),
        "UNLISTEN on non-listened channel should still return CommandComplete, got {:?}",
        types
    );
}

/// 验收场景 11：多条 NOTIFY 在下次查询响应时全部投递。
#[tokio::test]
async fn test_e2e_multiple_pending_notifications_all_delivered() {
    let port = find_free_port(17432).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut conn_a = setup_connection(port, "alice").await;
    let mut conn_b = setup_connection(port, "bob").await;

    let _ = send_query_and_read(&mut conn_a, "LISTEN multi").await;

    // B 连续发送 3 条 NOTIFY
    let _ = send_query_and_read(&mut conn_b, "NOTIFY multi, '1'").await;
    let _ = send_query_and_read(&mut conn_b, "NOTIFY multi, '2'").await;
    let _ = send_query_and_read(&mut conn_b, "NOTIFY multi, '3'").await;

    // A 下次查询应一次收到 3 条通知
    let resp_a = send_query_and_read(&mut conn_a, "SELECT 1").await;
    let notifications = extract_notifications(&resp_a);
    assert_eq!(notifications.len(), 3);
    assert_eq!(notifications[0].2, "1");
    assert_eq!(notifications[1].2, "2");
    assert_eq!(notifications[2].2, "3");
}

/// 验收场景 12：扩展查询协议下 Execute NOTIFY 也能投递通知。
///
/// 扩展查询协议：Parse + Bind + Execute + Sync 批量发送，服务器在 Sync 时
/// 一次性返回所有响应（ParseComplete + BindComplete + CommandComplete +
/// NotificationResponse + ReadyForQuery）。
#[tokio::test]
async fn test_e2e_extended_query_notify_delivers_to_self() {
    use szrsql_protocol::pgwire::message::{
        MSG_BIND, MSG_BIND_COMPLETE, MSG_EXECUTE, MSG_PARSE, MSG_PARSE_COMPLETE, MSG_SYNC,
    };

    let port = find_free_port(17532).await;
    let _server = spawn_test_server(port).await;
    wait_for_server(port).await;

    let mut stream = setup_connection(port, "alice").await;

    // 简单查询先 LISTEN
    let _ = send_query_and_read(&mut stream, "LISTEN extch").await;

    // 扩展查询：Parse + Bind + Execute + Sync 批量发送 NOTIFY
    let sql = "NOTIFY extch, 'extended'";

    // 构造 Parse 消息（无名语句，0 个参数）
    let mut parse_msg = Vec::new();
    parse_msg.push(MSG_PARSE);
    let parse_payload = {
        let mut p = Vec::new();
        p.push(0); // stmt_name = ""
        p.extend_from_slice(sql.as_bytes());
        p.push(0);
        p.extend_from_slice(&0i16.to_be_bytes()); // 0 个参数
        p
    };
    parse_msg.extend_from_slice(&(parse_payload.len() as i32 + 4).to_be_bytes());
    parse_msg.extend_from_slice(&parse_payload);

    // 构造 Bind 消息（无名 portal 绑定到无名语句）
    let mut bind_msg = Vec::new();
    bind_msg.push(MSG_BIND);
    let bind_payload = {
        let mut p = Vec::new();
        p.push(0); // portal_name = ""
        p.push(0); // statement_name = ""
        p.extend_from_slice(&0i16.to_be_bytes()); // 0 个参数格式码
        p.extend_from_slice(&0i16.to_be_bytes()); // 0 个参数
        p.extend_from_slice(&0i16.to_be_bytes()); // 0 个结果格式码
        p
    };
    bind_msg.extend_from_slice(&(bind_payload.len() as i32 + 4).to_be_bytes());
    bind_msg.extend_from_slice(&bind_payload);

    // 构造 Execute 消息（无名 portal，max_rows=0）
    let mut execute_msg = Vec::new();
    execute_msg.push(MSG_EXECUTE);
    let execute_payload = {
        let mut p = Vec::new();
        p.push(0); // portal_name = ""
        p.extend_from_slice(&0i32.to_be_bytes()); // max_rows = 0
        p
    };
    execute_msg.extend_from_slice(&(execute_payload.len() as i32 + 4).to_be_bytes());
    execute_msg.extend_from_slice(&execute_payload);

    // 构造 Sync 消息
    let mut sync_msg = Vec::new();
    sync_msg.push(MSG_SYNC);
    sync_msg.extend_from_slice(&4i32.to_be_bytes());

    // 批量发送所有消息
    let mut batch = Vec::new();
    batch.extend_from_slice(&parse_msg);
    batch.extend_from_slice(&bind_msg);
    batch.extend_from_slice(&execute_msg);
    batch.extend_from_slice(&sync_msg);
    stream.write_all(&batch).await.expect("write batch");
    stream.flush().await.expect("flush");

    // 读取响应直到 ReadyForQuery
    let response = read_until_ready_for_query(&mut stream).await;
    let types = parse_message_types(&response);

    // 期望顺序：ParseComplete + BindComplete + CommandComplete + NotificationResponse + ReadyForQuery
    assert_eq!(
        types[0], MSG_PARSE_COMPLETE,
        "expected ParseComplete first, got {:?}",
        types
    );
    assert_eq!(
        types[1], MSG_BIND_COMPLETE,
        "expected BindComplete second, got {:?}",
        types
    );
    assert!(
        types.contains(&MSG_COMMAND_COMPLETE),
        "extended Execute NOTIFY should return CommandComplete, got {:?}",
        types
    );
    assert!(
        types.contains(&MSG_NOTIFICATION_RESPONSE),
        "extended Execute NOTIFY should produce NotificationResponse, got {:?}",
        types
    );

    let notifications = extract_notifications(&response);
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].1, "extch");
    assert_eq!(notifications[0].2, "extended");
}
