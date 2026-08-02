//! Oracle Net (TNS) 服务器主入口 — TCP 监听 + TNS 握手 + 命令循环。
//!
//! 服务器生命周期：
//! ```text
//! 1. 监听 TCP 端口（Oracle 默认 1521）
//! 2. 接受客户端连接
//! 3. TNS 握手：接收 Connect Request → 发送 Accept Response
//! 4. 命令循环：读取 Data 包 → 解析 SQL → 执行 → 返回结果
//! 5. 客户端断开 → 清理
//! ```
//!
//! # L2 协议级兼容
//!
//! 本服务器实现 Oracle Net 协议的 L2 级兼容：
//! - TNS 包格式编解码（包类型/标志位/校验和）
//! - TNS 握手流程（Connect/Accept/Refuse）
//! - 基本数据包交换（Data 类型包的收发）
//!
//! 客户端（如 Navicat、SQL Developer）可建立到本服务器的 TCP 连接，
//! 完成 TNS 握手后进入命令循环。SQL 语句通过 ExecutorService 执行，
//! 结果以简化 TTC 响应返回。

use crate::tns_handshake::{
    negotiate_sdu, negotiate_tdu, negotiate_version, AcceptResponse, ConnectRequest, HandshakeError,
};
use crate::tns_packet::{PacketType, TnsPacket, TnsPacketCodec, TnsPacketError};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use szrsql_protocol::pgwire::session::{ExecutorService, QueryResult};
use szrsql_protocol::pgwire::InMemoryTable;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, RwLock};

/// Oracle 服务器配置。
#[derive(Debug, Clone)]
pub struct OracleConfig {
    /// 监听地址
    pub host: String,
    /// 监听端口（Oracle 默认 1521）
    pub port: u16,
    /// 服务器版本字符串
    pub server_version: String,
    /// 服务名（SID/Service Name，如 "ORCL"）
    pub service_name: String,
    /// 连接空闲超时（默认 300s = 5 分钟；`Duration::ZERO` 表示禁用）。
    ///
    /// 当连接在此时间内未收到任何客户端消息时，服务器主动关闭连接并释放
    /// session 资源，避免客户端异常断开导致的死锁。
    pub connection_idle_timeout: std::time::Duration,
}

impl Default for OracleConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 1521,
            server_version: "19.0-szrsql".to_string(),
            service_name: "ORCL".to_string(),
            connection_idle_timeout: std::time::Duration::from_secs(300),
        }
    }
}

impl OracleConfig {
    /// 创建默认配置。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置监听地址。
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    /// 设置监听端口。
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// 设置服务器版本字符串。
    pub fn with_server_version(mut self, version: impl Into<String>) -> Self {
        self.server_version = version.into();
        self
    }

    /// 设置服务名（SID/Service Name）。
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// 设置连接空闲超时（`Duration::ZERO` 表示禁用）。
    pub fn with_connection_idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.connection_idle_timeout = timeout;
        self
    }
}

/// Oracle 服务器错误。
#[derive(Debug, Error)]
pub enum OracleServerError {
    /// IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// TNS 包错误
    #[error("tns packet error: {0}")]
    Packet(#[from] TnsPacketError),
    /// 握手错误
    #[error("handshake error: {0}")]
    Handshake(#[from] HandshakeError),
}

/// Oracle Net 服务器。
pub struct OracleServer {
    /// 配置
    config: OracleConfig,
    /// 连接 ID 计数器
    connection_id_counter: AtomicI32,
    /// 跨会话共享的表存储（None = 独立存储，不与其他协议共享）
    shared_tables: Option<Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>>,
    /// 跨会话共享的锁管理器
    lock_manager: Option<Arc<szrsql_tx::lock::LockManager>>,
    /// 跨会话共享的事务 ID 计数器
    shared_txn_counter: Option<Arc<AtomicU32>>,
}

impl OracleServer {
    /// 创建新服务器。
    pub fn new(config: OracleConfig) -> Self {
        Self {
            config,
            connection_id_counter: AtomicI32::new(1),
            shared_tables: None,
            lock_manager: None,
            shared_txn_counter: None,
        }
    }

    /// 注入跨会话共享的表存储，使 Oracle 协议与 pgwire/MySQL 共享同一份数据。
    pub fn with_shared_tables(
        mut self,
        tables: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
    ) -> Self {
        self.shared_tables = Some(tables);
        self
    }

    /// 注入跨会话共享的锁管理器，启用行级并发控制。
    pub fn with_lock_manager(mut self, lm: Arc<szrsql_tx::lock::LockManager>) -> Self {
        self.lock_manager = Some(lm);
        self
    }

    /// 注入跨会话共享的事务 ID 计数器，保证事务 ID 全局唯一。
    pub fn with_shared_txn_counter(mut self, counter: Arc<AtomicU32>) -> Self {
        self.shared_txn_counter = Some(counter);
        self
    }

    /// 启动监听循环。
    pub async fn serve(self) -> Result<(), OracleServerError> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("Oracle Net server listening on {}", addr);
        let self_arc = Arc::new(self);
        loop {
            let (stream, peer_addr) = listener.accept().await?;
            tracing::debug!("Oracle connection from {}", peer_addr);
            let server = Arc::clone(&self_arc);
            tokio::spawn(async move {
                if let Err(e) = server.handle_connection(stream).await {
                    tracing::warn!("Oracle connection error: {}", e);
                }
            });
        }
    }

    /// 处理单个客户端连接。
    pub async fn handle_connection(&self, stream: TcpStream) -> Result<(), OracleServerError> {
        let conn_id = self.connection_id_counter.fetch_add(1, Ordering::SeqCst) as u32;
        let mut conn = Connection::new(self.config.clone(), conn_id);
        // BUG-1 修复：注入共享存储/锁管理器/事务计数器，使 Oracle 协议与 pgwire/MySQL 共享同一份数据。
        let mut executor = ExecutorService::new();
        if let Some(st) = &self.shared_tables {
            executor = executor.with_shared_tables(st.clone());
        }
        if let Some(lm) = &self.lock_manager {
            executor = executor.with_lock_manager(lm.clone());
        }
        if let Some(tc) = &self.shared_txn_counter {
            executor = executor.with_shared_txn_counter(tc.clone());
        }
        conn.executor = Arc::new(Mutex::new(executor));
        conn.handle(stream).await
    }
}

/// 单个客户端连接。
struct Connection {
    /// 配置
    config: OracleConfig,
    /// 连接 ID
    conn_id: u32,
    /// SQL 执行器
    executor: Arc<Mutex<ExecutorService>>,
}

impl Connection {
    /// 创建新连接。
    fn new(config: OracleConfig, conn_id: u32) -> Self {
        Self {
            config,
            conn_id,
            executor: Arc::new(Mutex::new(ExecutorService::new())),
        }
    }

    /// 处理连接主流程。
    async fn handle(&mut self, mut stream: TcpStream) -> Result<(), OracleServerError> {
        // 1. TNS 握手
        self.do_handshake(&mut stream).await?;
        tracing::info!("Oracle conn {} TNS handshake completed", self.conn_id);

        // 2. 命令循环
        let idle_timeout = self.config.connection_idle_timeout;
        loop {
            // 连接空闲超时包装：超时后关闭连接，释放 session 资源
            let read_result = if idle_timeout.is_zero() {
                TnsPacketCodec::read_packet(&mut stream).await
            } else {
                match tokio::time::timeout(idle_timeout, TnsPacketCodec::read_packet(&mut stream))
                    .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        tracing::warn!(
                            conn_id = self.conn_id,
                            timeout_secs = idle_timeout.as_secs(),
                            "Oracle connection idle timeout, closing"
                        );
                        break;
                    }
                }
            };
            let packet = match read_result {
                Ok(p) => p,
                Err(TnsPacketError::Io(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof
                        || e.kind() == std::io::ErrorKind::ConnectionReset =>
                {
                    break;
                }
                Err(e) => return Err(e.into()),
            };

            match packet.packet_type {
                PacketType::Data => {
                    // Data 包：可能包含 SQL 请求或 TTC 控制消息
                    self.handle_data_packet(&mut stream, &packet).await?;
                }
                PacketType::Control => {
                    // Control 包：发送空 Control 响应（保持连接）
                    let resp = TnsPacket::new(PacketType::Control, Vec::new());
                    TnsPacketCodec::write_packet(&mut stream, &resp).await?;
                }
                PacketType::Connect => {
                    // 重复 Connect 包：重新握手
                    self.do_handshake(&mut stream).await?;
                }
                _ => {
                    // 其他类型：返回 Error 包
                    let err_payload =
                        format!("unsupported packet type: {:?}", packet.packet_type).into_bytes();
                    let err_packet = TnsPacket::new(PacketType::Error, err_payload);
                    TnsPacketCodec::write_packet(&mut stream, &err_packet).await?;
                }
            }
        }
        Ok(())
    }

    /// TNS 握手：接收 Connect Request → 发送 Accept Response。
    async fn do_handshake(&self, stream: &mut TcpStream) -> Result<(), OracleServerError> {
        // 读取 Connect Request
        let connect_packet = TnsPacketCodec::read_packet(stream).await?;
        let connect_req = ConnectRequest::from_packet(&connect_packet)?;

        // 版本与 SDU/TDU 协商
        let negotiated_version = negotiate_version(connect_req.version);
        let negotiated_sdu = negotiate_sdu(connect_req.sdu);
        let negotiated_tdu = negotiate_tdu(connect_req.tdu);

        tracing::debug!(
            conn_id = self.conn_id,
            client_version = connect_req.version,
            negotiated_version,
            sdu = negotiated_sdu,
            tdu = negotiated_tdu,
            client_service_name = %connect_req.service_name,
            server_service_name = %self.config.service_name,
            server_version = %self.config.server_version,
            "TNS Connect received"
        );

        // 发送 Accept Response
        let accept =
            AcceptResponse::new(negotiated_version).with_sdu_tdu(negotiated_sdu, negotiated_tdu);
        let accept_packet = accept.encode();
        TnsPacketCodec::write_packet(stream, &accept_packet).await?;

        Ok(())
    }

    /// 处理 Data 包 — 解析 TTC 消息或直接执行 SQL。
    ///
    /// Oracle TTC 协议非常复杂，这里采用简化策略：
    /// 1. 尝试从 Data 负载中提取 ASCII SQL 文本（以 SELECT/INSERT/UPDATE/DELETE/CREATE 等开头）
    /// 2. 若提取到 SQL，通过 ExecutorService 执行
    /// 3. 返回简化的 Data 包响应（包含执行结果或错误消息）
    async fn handle_data_packet(
        &mut self,
        stream: &mut TcpStream,
        packet: &TnsPacket,
    ) -> Result<(), OracleServerError> {
        let payload = &packet.data;

        // 尝试从 TTC 负载中提取 SQL 文本
        if let Some(sql) = extract_sql_from_ttc_payload(payload) {
            tracing::debug!(
                conn_id = self.conn_id,
                sql = %sql,
                "extracted SQL from TTC payload"
            );

            let mut executor = self.executor.lock().await;
            let results = executor.execute_sql(&sql).await;
            drop(executor);

            // 构造简化响应：将结果集或错误以文本形式返回
            let response_text = format_results(&results);
            let response_bytes = response_text.into_bytes();
            let resp_packet = TnsPacket::data_packet(response_bytes);
            TnsPacketCodec::write_packet(stream, &resp_packet).await?;
        } else {
            // 非 SQL 数据包：返回 OK 包保持连接
            let ok_packet = TnsPacket::new(PacketType::Ok, Vec::new());
            TnsPacketCodec::write_packet(stream, &ok_packet).await?;
        }
        Ok(())
    }
}

/// 从 TTC 负载中尝试提取 SQL 文本（支持 UTF-8 多字节字符）。
///
/// TTC 协议在 Data 包负载前有若干控制字节，SQL 文本通常以 ASCII 字符出现。
/// 本函数扫描负载，查找以 SQL 关键字开头的子串，并支持 UTF-8 编码的非 ASCII 字符
/// （如中文、日文等），避免在多字节字符中间截断导致解析失败。
fn extract_sql_from_ttc_payload(payload: &[u8]) -> Option<String> {
    // SQL 关键字列表（大写）
    const SQL_KEYWORDS: &[&str] = &[
        "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP", "ALTER", "BEGIN", "COMMIT",
        "ROLLBACK", "SET", "SHOW", "WITH", "MERGE", "TRUNCATE",
    ];

    // 扫描负载，查找 SQL 关键字
    for i in 0..payload.len() {
        let remaining = &payload[i..];
        for keyword in SQL_KEYWORDS {
            let kw_bytes = keyword.as_bytes();
            if remaining.len() >= kw_bytes.len()
                && remaining[..kw_bytes.len()].eq_ignore_ascii_case(kw_bytes)
            {
                // 找到关键字，提取后续文本直到遇到 NUL 或控制字符
                let mut end = i + kw_bytes.len();
                while end < payload.len() {
                    let b = payload[end];
                    // NUL 或 TTC 控制字节：终止
                    if b == 0 || b == 0x01 || b == 0x03 {
                        break;
                    }
                    // 可打印 ASCII 或常见空白：接受
                    if b >= 0x20 && b < 0x7F || b == b'\n' || b == b'\r' || b == b'\t' {
                        end += 1;
                    } else if b >= 0x80 {
                        // UTF-8 多字节字符首字节（0x80..=0xFF）：
                        // 根据首字节判断后续字节数，验证完整的多字节序列
                        let cont_len = if b >= 0xF0 {
                            3 // 4 字节字符：1 首字节 + 3 续字节
                        } else if b >= 0xE0 {
                            2 // 3 字节字符：1 首字节 + 2 续字节
                        } else if b >= 0xC0 {
                            1 // 2 字节字符：1 首字节 + 1 续字节
                        } else {
                            // 0x80..=0xBF 是续字节，不应出现在首字节位置，终止
                            break;
                        };
                        // 检查后续字节是否都在范围内且为续字节（0x80..=0xBF）
                        if end + cont_len >= payload.len() {
                            // 不完整的多字节序列，终止
                            break;
                        }
                        let mut valid = true;
                        for k in 1..=cont_len {
                            if payload[end + k] & 0xC0 != 0x80 {
                                valid = false;
                                break;
                            }
                        }
                        if !valid {
                            break;
                        }
                        end += 1 + cont_len;
                    } else {
                        // 其他控制字符（0x02, 0x04..=0x1F）：终止
                        break;
                    }
                }
                let sql_bytes = &payload[i..end];
                // 严格 UTF-8 解码（无效序列返回 None）
                match std::str::from_utf8(sql_bytes) {
                    Ok(s) if s.len() >= keyword.len() => return Some(s.to_string()),
                    _ => {}
                }
            }
        }
    }
    None
}

/// 将 ExecutorService 的执行结果格式化为文本响应。
fn format_results(
    results: &[Result<QueryResult, szrsql_protocol::pgwire::session::SessionError>],
) -> String {
    let mut output = String::new();
    for result in results {
        match result {
            Ok(QueryResult::ResultSet { columns, rows, tag }) => {
                output.push_str(&format!("{}\n", tag));
                // 列名
                if !columns.is_empty() {
                    let header: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
                    output.push_str(&header.join(" | "));
                    output.push('\n');
                }
                // 数据行
                for row in rows {
                    let cells: Vec<String> = row.iter().map(|v| format_value(v)).collect();
                    output.push_str(&cells.join(" | "));
                    output.push('\n');
                }
            }
            Ok(QueryResult::AffectedRows { tag }) => {
                output.push_str(&format!("{}\n", tag));
            }
            Ok(QueryResult::DdlComplete { tag }) => {
                output.push_str(&format!("{}\n", tag));
            }
            Ok(QueryResult::TransactionComplete { tag, .. }) => {
                output.push_str(&format!("{}\n", tag));
            }
            Ok(QueryResult::Empty) => {
                output.push_str("OK\n");
            }
            Err(e) => {
                output.push_str(&format!("ERROR: {}\n", e));
            }
        }
    }
    output
}

/// 格式化 Value 为文本表示。
fn format_value(value: &szrsql_types::value::Value) -> String {
    use szrsql_types::value::Value;
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int64(n) => n.to_string(),
        Value::Float64(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        _ => format!("{:?}", value),
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tns_handshake::{DEFAULT_SDU, DEFAULT_TDU, TNS_VERSION_314};

    #[test]
    fn oracle_config_default() {
        let config = OracleConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 1521);
        assert_eq!(config.service_name, "ORCL");
    }

    #[test]
    fn oracle_config_builder_chain() {
        let config = OracleConfig::new()
            .with_host("0.0.0.0")
            .with_port(1522)
            .with_server_version("21.0-szrsql")
            .with_service_name("XEPDB1");
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 1522);
        assert_eq!(config.server_version, "21.0-szrsql");
        assert_eq!(config.service_name, "XEPDB1");
    }

    #[test]
    fn extract_sql_finds_select_statement() {
        // 模拟 TTC 负载：前缀控制字节 + SQL 文本
        let payload = [
            0x01, 0x5e, 0x00, b'S', b'E', b'L', b'E', b'C', b'T', b' ', b'1',
        ];
        let sql = extract_sql_from_ttc_payload(&payload);
        assert_eq!(sql, Some("SELECT 1".to_string()));
    }

    #[test]
    fn extract_sql_finds_create_table() {
        let payload = [
            0x03, 0x5e, b'C', b'R', b'E', b'A', b'T', b'E', b' ', b'T', b'A', b'B', b'L', b'E',
            b' ', b't', b'(', b')',
        ];
        let sql = extract_sql_from_ttc_payload(&payload);
        assert!(sql.is_some());
        assert!(sql.unwrap().starts_with("CREATE"));
    }

    #[test]
    fn extract_sql_returns_none_for_pure_binary_payload() {
        // 纯二进制 TTC 控制包，无 SQL 文本
        let payload = [0x01, 0x02, 0x03, 0x04, 0x05, 0x00, 0x01, 0x5e];
        let sql = extract_sql_from_ttc_payload(&payload);
        assert!(sql.is_none());
    }

    #[test]
    fn extract_sql_case_insensitive_keyword() {
        let payload = [b's', b'e', b'l', b'e', b'c', b't', b' ', b'*'];
        let sql = extract_sql_from_ttc_payload(&payload);
        assert_eq!(sql, Some("select *".to_string()));
    }

    #[test]
    fn extract_sql_stops_at_nul_terminator() {
        let payload = [b'S', b'E', b'L', b'E', b'C', b'T', b' ', b'1', 0x00, b'X'];
        let sql = extract_sql_from_ttc_payload(&payload);
        assert_eq!(sql, Some("SELECT 1".to_string()));
    }

    #[test]
    fn format_value_formats_common_types() {
        use szrsql_types::value::Value;
        assert_eq!(format_value(&Value::Null), "NULL");
        assert_eq!(format_value(&Value::Bool(true)), "true");
        assert_eq!(format_value(&Value::Int64(42)), "42");
        assert_eq!(format_value(&Value::Text("hello".into())), "hello");
    }

    #[tokio::test]
    async fn oracle_server_handle_connection_completes_handshake() {
        // 验证 TNS 握手流程：Connect → Accept（使用真实 TCP 连接）
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        // 绑定随机端口
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // 服务器端：接受连接并处理握手
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let config = OracleConfig::default();
            let mut conn = Connection::new(config, 1);
            conn.handle(stream).await
        });

        // 客户端：连接服务器
        let mut client = TcpStream::connect(addr).await.unwrap();

        // 构造 Connect Request
        let connect_req = ConnectRequest::new("ORCL").unwrap();
        let connect_packet = connect_req.encode();
        let connect_bytes = connect_packet.encode();
        client.write_all(&connect_bytes).await.unwrap();
        client.flush().await.unwrap();

        // 读取 Accept Response
        let mut buf = vec![0u8; 256];
        let n = client.read(&mut buf).await.unwrap();
        assert!(n >= 8, "should read at least TNS header");

        // 解析 Accept 包
        let (accept_packet, consumed) = TnsPacket::decode(&buf[..n]).unwrap();
        assert_eq!(accept_packet.packet_type, PacketType::Accept);
        assert_eq!(consumed, n);
        let accept = AcceptResponse::decode_payload(&accept_packet.data).unwrap();
        assert_eq!(accept.version, TNS_VERSION_314);
        assert_eq!(accept.sdu, DEFAULT_SDU);
        assert_eq!(accept.tdu, DEFAULT_TDU);

        // 等待服务器任务完成
        let _ = server_handle.await;
    }
}
