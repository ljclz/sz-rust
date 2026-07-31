//! TDS 服务器主入口 — TCP 监听 + 握手 + 命令循环。
//!
//! 服务器生命周期：
//! ```text
//! 1. 监听 TCP 端口
//! 2. 接受客户端连接
//! 3. Pre-Login 握手（ OPTION/VERSION/ENCRYPTION/INSTOPT/THREADID）
//! 4. Login7 认证（用户名 + 混淆密码）
//! 5. 命令循环：SQLBatch → 执行 SQL → 返回结果集
//! 6. 客户端断开 → 清理
//! ```

use crate::auth::{AuthError, AuthMode, AuthSession};
use crate::command::{parse_command, Command, CommandError, RpcCommand, SqlBatchCommand};
use crate::handshake::{
    EncryptionValue, ErrorToken, HandshakeError, Login7, LoginAck, PreLogin, PreLoginOption,
    PreLoginOptionType,
};
use crate::packet::{PacketCodec, PacketError, TdsPacket, TdsPacketType};
use crate::result_set::{encode_envchange, ColumnMetaData, DoneStatus, EnvChangeType, ResultSetEncoder};
use szrsql_protocol::pgwire::session::{ExecutorService, QueryResult};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

/// TDS 服务器配置。
#[derive(Debug, Clone)]
pub struct TdsConfig {
    /// 监听地址
    pub host: String,
    /// 监听端口（SQL Server 默认 1433）
    pub port: u16,
    /// 服务器版本字符串
    pub server_version: String,
    /// 认证模式
    pub auth_mode: AuthMode,
    /// 允许的数据库列表（空表示全部允许）
    pub allowed_databases: Vec<String>,
    /// 连接空闲超时（默认 300s = 5 分钟；`Duration::ZERO` 表示禁用）。
    ///
    /// 当连接在此时间内未收到任何客户端消息时，服务器主动关闭连接并释放
    /// session 资源，避免客户端异常断开导致的死锁。
    pub connection_idle_timeout: std::time::Duration,
}

impl Default for TdsConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 1433,
            server_version: "15.00.2000".to_string(),
            auth_mode: AuthMode::Trust,
            allowed_databases: Vec::new(),
            connection_idle_timeout: std::time::Duration::from_secs(300),
        }
    }
}

impl TdsConfig {
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

    /// 设置认证模式。
    pub fn with_auth_mode(mut self, mode: AuthMode) -> Self {
        self.auth_mode = mode;
        self
    }

    /// 设置允许的数据库列表。
    pub fn with_allowed_databases(mut self, dbs: Vec<String>) -> Self {
        self.allowed_databases = dbs;
        self
    }

    /// 设置连接空闲超时（`Duration::ZERO` 表示禁用）。
    pub fn with_connection_idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.connection_idle_timeout = timeout;
        self
    }
}

/// TDS 服务器错误。
#[derive(Debug, Error)]
pub enum TdsServerError {
    /// IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// 包错误
    #[error("packet error: {0}")]
    Packet(#[from] PacketError),
    /// 握手错误
    #[error("handshake error: {0}")]
    Handshake(#[from] HandshakeError),
    /// 认证错误
    #[error("auth error: {0}")]
    Auth(#[from] AuthError),
    /// 命令错误
    #[error("command error: {0}")]
    Command(#[from] CommandError),
}

/// TDS 服务器。
pub struct TdsServer {
    /// 配置
    config: TdsConfig,
    /// 连接 ID 计数器
    connection_id_counter: AtomicI32,
}

impl TdsServer {
    /// 创建新服务器。
    pub fn new(config: TdsConfig) -> Self {
        Self {
            config,
            connection_id_counter: AtomicI32::new(1),
        }
    }

    /// 启动监听循环。
    pub async fn serve(self) -> Result<(), TdsServerError> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("TDS server listening on {}", addr);
        let self_arc = Arc::new(self);
        loop {
            let (stream, peer_addr) = listener.accept().await?;
            tracing::debug!("TDS connection from {}", peer_addr);
            let server = Arc::clone(&self_arc);
            tokio::spawn(async move {
                if let Err(e) = server.handle_connection(stream).await {
                    tracing::warn!("TDS connection error: {}", e);
                }
            });
        }
    }

    /// 处理单个客户端连接。
    pub async fn handle_connection(&self, stream: TcpStream) -> Result<(), TdsServerError> {
        let conn_id = self.connection_id_counter.fetch_add(1, Ordering::SeqCst) as u32;
        let mut conn = Connection::new(self.config.clone(), conn_id);
        conn.handle(stream).await
    }
}

/// 单个客户端连接。
struct Connection {
    /// 配置
    config: TdsConfig,
    /// 连接 ID
    conn_id: u32,
    /// SQL 执行器
    executor: Arc<Mutex<ExecutorService>>,
    /// 当前数据库
    current_db: Option<String>,
    /// 预处理语句存储（handle → SQL），用于 sp_prepare/sp_execute
    prepared_statements: HashMap<i64, String>,
    /// 下一个预处理语句句柄（递增分配）
    next_prepare_handle: i64,
}

impl Connection {
    /// 创建新连接。
    fn new(config: TdsConfig, conn_id: u32) -> Self {
        Self {
            config,
            conn_id,
            executor: Arc::new(Mutex::new(ExecutorService::new())),
            current_db: None,
            prepared_statements: HashMap::new(),
            next_prepare_handle: 1,
        }
    }

    /// 分配下一个预处理语句句柄。
    fn allocate_handle(&mut self) -> i64 {
        let handle = self.next_prepare_handle;
        self.next_prepare_handle += 1;
        handle
    }

    /// 处理连接主流程。
    async fn handle(&mut self, mut stream: TcpStream) -> Result<(), TdsServerError> {
        let username = self.do_handshake(&mut stream).await?;
        tracing::info!(
            "TDS conn {} authenticated as '{}'",
            self.conn_id,
            username
        );

        let idle_timeout = self.config.connection_idle_timeout;
        loop {
            // 连接空闲超时包装：超时后关闭连接，释放 session 资源
            let read_result = if idle_timeout.is_zero() {
                PacketCodec::read_packet(&mut stream).await
            } else {
                match tokio::time::timeout(idle_timeout, PacketCodec::read_packet(&mut stream)).await {
                    Ok(r) => r,
                    Err(_) => {
                        tracing::warn!(
                            conn_id = self.conn_id,
                            timeout_secs = idle_timeout.as_secs(),
                            "TDS connection idle timeout, closing"
                        );
                        break;
                    }
                }
            };
            let packet = match read_result {
                Ok(p) => p,
                Err(PacketError::Io(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof
                        || e.kind() == std::io::ErrorKind::ConnectionReset =>
                {
                    break;
                }
                Err(e) => return Err(e.into()),
            };
            let packet_type_byte = packet.packet_type as u8;
            let (cmd, payload) = parse_command(packet_type_byte, &packet.payload)?;
            match cmd {
                Command::Logout => break,
                Command::SqlBatch => {
                    let batch = SqlBatchCommand::parse(payload);
                    self.handle_sql_batch(&mut stream, &batch.sql).await?;
                }
                Command::Rpc => {
                    // 解析 RPC 并分发到 sp_executesql/sp_prepare/sp_execute
                    self.handle_rpc(&mut stream, payload).await?;
                }
                Command::Attention => {
                    // Attention 信号：忽略，继续等待下一条命令
                    tracing::debug!("TDS conn {} received Attention signal", self.conn_id);
                }
            }
        }
        Ok(())
    }

    /// 执行 Pre-Login + Login7 握手。
    async fn do_handshake(
        &mut self,
        stream: &mut TcpStream,
    ) -> Result<String, TdsServerError> {
        // 阶段 1：读取 Pre-Login 请求
        let pre_login_req_packet = PacketCodec::read_packet(stream).await?;
        if pre_login_req_packet.packet_type != TdsPacketType::PreLogin {
            return Err(TdsServerError::Handshake(HandshakeError::Protocol(
                format!(
                    "expected Pre-Login packet, got 0x{:02X}",
                    pre_login_req_packet.packet_type as u8
                ),
            )));
        }
        let pre_login_req = PreLogin::decode(&pre_login_req_packet.payload)?;

        // 阶段 2：发送 Pre-Login 响应。
        //
        // 服务器当前无 TLS 配置：返回 Off=0x00 而非 NotSupported=0x03。
        // NotSupported 会让部分客户端（如 JDBC for SQL Server）直接报错，
        // 而 Off 表示服务器支持加密但未启用，允许客户端明文回退继续连接。
        let _client_encryption = pre_login_req
            .find(PreLoginOptionType::Encryption)
            .and_then(|opt| opt.data.first().copied())
            .and_then(EncryptionValue::from_byte);
        let server_encryption = EncryptionValue::Off;

        let pre_login_resp = PreLogin::new()
            .with_option(PreLoginOption::version(15, 0, 2000))
            .with_option(PreLoginOption::encryption(server_encryption))
            .with_option(PreLoginOption::inst_opt(0))
            .with_option(PreLoginOption::thread_id(self.conn_id));
        let resp_packet = TdsPacket::new(TdsPacketType::PreLogin, pre_login_resp.encode())?;
        PacketCodec::write_packet(stream, &resp_packet).await?;

        // 阶段 3：读取 Login7
        let login_packet = PacketCodec::read_packet(stream).await?;
        if login_packet.packet_type != TdsPacketType::Login7 {
            return Err(TdsServerError::Handshake(HandshakeError::Protocol(
                format!(
                    "expected Login7 packet, got 0x{:02X}",
                    login_packet.packet_type as u8
                ),
            )));
        }
        let login = Login7::decode(&login_packet.payload)?;

        // 阶段 4：认证
        let mut auth_session = AuthSession::new(self.config.auth_mode.clone());
        match auth_session.verify(&login.user_name, &login.password) {
            Ok(username) => {
                // 校验数据库
                if !login.database.is_empty()
                    && !self.config.allowed_databases.is_empty()
                    && !self
                        .config
                        .allowed_databases
                        .iter()
                        .any(|d| d == &login.database)
                {
                    let err = ErrorToken::new(
                        4060,
                        format!("Cannot open database '{}'", login.database),
                    );
                    self.send_error_token(stream, &err).await?;
                    return Err(TdsServerError::Auth(AuthError::AccessDenied(username)));
                }
                if !login.database.is_empty() {
                    self.current_db = Some(login.database.clone());
                }
                // 发送 LOGINACK
                self.send_login_ack(stream, &username).await?;
                Ok(username)
            }
            Err(e) => {
                let err = ErrorToken::new(
                    18456,
                    format!("Login failed for user '{}'", login.user_name),
                );
                self.send_error_token(stream, &err).await?;
                Err(e.into())
            }
        }
    }

    /// 发送登录成功响应：ENVCHANGE * N + LOGINACK + DONE。
    ///
    /// 登录成功后依次发送：
    /// 1. PacketSize ENVCHANGE — 通知客户端协商的包大小（默认 4096）
    /// 2. Database ENVCHANGE — 通知当前数据库上下文
    /// 3. Collation ENVCHANGE — 通知排序规则（默认 0x0904d000 = Latin1_General_CI_AS）
    /// 4. LOGINACK token — 认证成功确认
    /// 5. DONE token — 命令完成
    ///
    /// 所有 token 合并到单个 TDS 包中发送，符合 TDS 协议规范。
    async fn send_login_ack(
        &self,
        stream: &mut TcpStream,
        _username: &str,
    ) -> Result<(), TdsServerError> {
        let ack = LoginAck::new(format!("SzRSQL/{}", self.config.server_version));
        let done = ResultSetEncoder::encode_done(DoneStatus::FINAL, 0, 0);
        // 协商包大小（与 DEFAULT_PACKET_SIZE 保持一致）
        let packet_size = crate::handshake::DEFAULT_PACKET_SIZE.to_string();
        // 当前数据库上下文（未指定时为 master）
        let current_db = self.current_db.clone().unwrap_or_else(|| "master".to_string());
        // 合并 ENVCHANGE + LOGINACK + DONE 到单个 payload
        let mut combined = Vec::with_capacity(128);
        combined.extend_from_slice(&encode_envchange(
            EnvChangeType::PacketSize as u8,
            &packet_size,
            &packet_size,
        ));
        combined.extend_from_slice(&encode_envchange(
            EnvChangeType::Database as u8,
            "",
            &current_db,
        ));
        combined.extend_from_slice(&encode_envchange(
            EnvChangeType::Collation as u8,
            "",
            "0x0904d000",
        ));
        combined.extend_from_slice(&ack.encode());
        combined.extend_from_slice(&done);
        let packet = TdsPacket::new(TdsPacketType::Response, combined)?;
        PacketCodec::write_packet(stream, &packet).await?;
        Ok(())
    }

    /// 发送 ERROR token。
    async fn send_error_token(
        &self,
        stream: &mut TcpStream,
        err: &ErrorToken,
    ) -> Result<(), TdsServerError> {
        let packet = TdsPacket::new(TdsPacketType::Response, err.encode())?;
        PacketCodec::write_packet(stream, &packet).await?;
        Ok(())
    }

    /// 处理 SQLBatch 命令。
    async fn handle_sql_batch(
        &mut self,
        stream: &mut TcpStream,
        sql: &str,
    ) -> Result<(), TdsServerError> {
        let trimmed = sql.trim();
        if trimmed.is_empty() {
            let done = ResultSetEncoder::encode_done(DoneStatus::FINAL, 0, 0);
            let packet = TdsPacket::new(TdsPacketType::Response, done)?;
            PacketCodec::write_packet(stream, &packet).await?;
            return Ok(());
        }
        let mut executor = self.executor.lock().await;
        let results = executor.execute_sql(trimmed).await;
        drop(executor);
        for result in results {
            match result {
                Ok(QueryResult::ResultSet { columns, rows, .. }) => {
                    self.send_result_set(stream, &columns, &rows).await?;
                }
                Ok(QueryResult::AffectedRows { tag }) => {
                    let affected = parse_affected_rows(&tag);
                    let done = ResultSetEncoder::encode_done(DoneStatus::FINAL, 0, affected);
                    let packet = TdsPacket::new(TdsPacketType::Response, done)?;
                    PacketCodec::write_packet(stream, &packet).await?;
                }
                Ok(QueryResult::DdlComplete { .. }) => {
                    let done = ResultSetEncoder::encode_done(DoneStatus::FINAL, 0, 0);
                    let packet = TdsPacket::new(TdsPacketType::Response, done)?;
                    PacketCodec::write_packet(stream, &packet).await?;
                }
                Ok(QueryResult::TransactionComplete { .. }) => {
                    let done = ResultSetEncoder::encode_done(DoneStatus::FINAL, 0, 0);
                    let packet = TdsPacket::new(TdsPacketType::Response, done)?;
                    PacketCodec::write_packet(stream, &packet).await?;
                }
                Ok(QueryResult::Empty) => {
                    let done = ResultSetEncoder::encode_done(DoneStatus::FINAL, 0, 0);
                    let packet = TdsPacket::new(TdsPacketType::Response, done)?;
                    PacketCodec::write_packet(stream, &packet).await?;
                }
                Err(e) => {
                    let err = ErrorToken::new(0, e.to_string());
                    self.send_error_token(stream, &err).await?;
                }
            }
        }
        Ok(())
    }

    /// 处理 RPC 命令：分发到 sp_executesql / sp_prepare / sp_execute。
    ///
    /// 这三个存储过程是 SQL Server 客户端驱动（JDBC / ODBC / ADO.NET）
    /// 执行参数化查询的标准方式：
    /// - `sp_executesql`：直接执行带参数的 SQL（一次性）
    /// - `sp_prepare`：预处理 SQL，返回 handle
    /// - `sp_execute`：按 handle 执行已预处理的 SQL
    ///
    /// 其他未识别的 RPC 过程名返回错误。
    async fn handle_rpc(
        &mut self,
        stream: &mut TcpStream,
        payload: &[u8],
    ) -> Result<(), TdsServerError> {
        let rpc = match RpcCommand::parse(payload) {
            Ok(r) => r,
            Err(e) => {
                let err = ErrorToken::new(0, format!("RPC parse error: {e}"));
                self.send_error_token(stream, &err).await?;
                return Ok(());
            }
        };

        // 按过程名分发（不区分大小写）
        let proc_lower = rpc.proc_name.to_lowercase();
        match proc_lower.as_str() {
            "sp_executesql" => self.handle_sp_executesql(stream, &rpc).await?,
            "sp_prepare" => self.handle_sp_prepare(stream, &rpc).await?,
            "sp_execute" => self.handle_sp_execute(stream, &rpc).await?,
            "sp_unprepare" => {
                // sp_unprepare：释放预处理语句，返回空 DONE
                let done = ResultSetEncoder::encode_done(DoneStatus::FINAL, 0, 0);
                let packet = TdsPacket::new(TdsPacketType::Response, done)?;
                PacketCodec::write_packet(stream, &packet).await?;
            }
            _ => {
                let err = ErrorToken::new(
                    0,
                    format!("unsupported RPC procedure: '{}'", rpc.proc_name),
                );
                self.send_error_token(stream, &err).await?;
            }
        }
        Ok(())
    }

    /// 处理 sp_executesql：执行带参数的 SQL。
    ///
    /// 参数1=SQL 语句，参数2=参数定义（如 "@p1 int, @p2 varchar(10)"），参数3+=参数值。
    ///
    /// 参数绑定策略：将 RPC 参数值提取为 SQL 字面量，替换 SQL 文本中的
    /// `@p1`/`@p2`/... 占位符，然后执行替换后的 SQL。这避免了 SQL 注入风险
    /// （字符串值使用单引号转义），同时兼容 SzRSQL 执行器的文本执行接口。
    async fn handle_sp_executesql(
        &mut self,
        stream: &mut TcpStream,
        rpc: &RpcCommand,
    ) -> Result<(), TdsServerError> {
        match rpc.parse_sp_executesql() {
            Ok(parsed) => {
                let final_sql = substitute_rpc_params(&parsed);
                // 复用 SQLBatch 执行路径
                self.handle_sql_batch(stream, &final_sql).await
            }
            Err(e) => {
                let err = ErrorToken::new(0, format!("sp_executesql error: {e}"));
                self.send_error_token(stream, &err).await?;
                Ok(())
            }
        }
    }

    /// 处理 sp_prepare：预处理 SQL 并返回 handle。
    ///
    /// 提取 SQL 语句，分配 handle，存储到 `prepared_statements`，
    /// 通过 RETURNVALUE token (0xAC) 返回 handle 值。
    async fn handle_sp_prepare(
        &mut self,
        stream: &mut TcpStream,
        rpc: &RpcCommand,
    ) -> Result<(), TdsServerError> {
        match rpc.parse_sp_prepare() {
            Ok(sql) => {
                let handle = self.allocate_handle();
                self.prepared_statements.insert(handle, sql);
                // 编码 RETURNVALUE token 返回 handle（INTN 类型）
                let returnvalue = encode_returnvalue_int64(1, "@handle", handle);
                let done = ResultSetEncoder::encode_done(DoneStatus::FINAL, 0, 0);
                let mut combined = Vec::with_capacity(returnvalue.len() + done.len());
                combined.extend_from_slice(&returnvalue);
                combined.extend_from_slice(&done);
                let packet = TdsPacket::new(TdsPacketType::Response, combined)?;
                PacketCodec::write_packet(stream, &packet).await?;
                Ok(())
            }
            Err(e) => {
                let err = ErrorToken::new(0, format!("sp_prepare error: {e}"));
                self.send_error_token(stream, &err).await?;
                Ok(())
            }
        }
    }

    /// 处理 sp_execute：按 handle 执行已预处理的 SQL。
    ///
    /// 参数1=handle，参数2+=参数值。
    /// 参数绑定策略与 sp_executesql 一致：将参数值提取为 SQL 字面量，
    /// 替换 SQL 文本中的 @p1/@p2/... 占位符后执行。
    async fn handle_sp_execute(
        &mut self,
        stream: &mut TcpStream,
        rpc: &RpcCommand,
    ) -> Result<(), TdsServerError> {
        match rpc.parse_sp_execute() {
            Ok(parsed) => {
                let handle = parsed.handle;
                // 先 clone SQL 字符串，释放对 self.prepared_statements 的不可变借用，
                // 再调用 handle_sql_batch（需要可变借用 self）
                let sql = match self.prepared_statements.get(&handle) {
                    Some(s) => s.clone(),
                    None => {
                        let err = ErrorToken::new(
                            0,
                            format!("sp_execute: invalid prepared statement handle: {handle}"),
                        );
                        self.send_error_token(stream, &err).await?;
                        return Ok(());
                    }
                };
                // 参数绑定：替换 SQL 中的 @p1/@p2/... 占位符
                let final_sql = substitute_sp_execute_params(&sql, &parsed.values);
                // 复用 SQLBatch 执行路径
                self.handle_sql_batch(stream, &final_sql).await
            }
            Err(e) => {
                let err = ErrorToken::new(0, format!("sp_execute error: {e}"));
                self.send_error_token(stream, &err).await?;
                Ok(())
            }
        }
    }

    /// 发送结果集：ColumnMetaData + Rows + Done。
    ///
    /// 将所有 token 合并到单个 TDS 包中发送，避免客户端需要读取多个独立 EOM 包。
    /// （TDS 协议允许一个 Response 包中包含多个 token，这是更符合协议规范的做法。）
    ///
    /// 列类型映射：优先从第一行数据推断真实 TDS 类型（`ColumnMetaData::from_value`）；
    /// 若无数据则退化为 NVARCHAR(255)。
    async fn send_result_set(
        &mut self,
        stream: &mut TcpStream,
        columns: &[szrsql_protocol::pgwire::session::ResultColumn],
        rows: &[Vec<szrsql_types::value::Value>],
    ) -> Result<(), TdsServerError> {
        let col_meta: Vec<ColumnMetaData> = columns
            .iter()
            .enumerate()
            .map(|(i, c)| {
                // 从第一行数据推断列类型；无数据时默认 NVARCHAR(255)
                let value = rows.first().and_then(|r| r.get(i));
                match value {
                    Some(v) => ColumnMetaData::from_value(&c.name, v),
                    None => ColumnMetaData::nvarchar(&c.name, 255),
                }
            })
            .collect();
        let payloads = ResultSetEncoder::encode_result_set(&col_meta, rows);
        // 合并所有 token 到单个 payload
        let mut combined = Vec::with_capacity(256);
        for payload in payloads {
            combined.extend_from_slice(&payload);
        }
        let packet = TdsPacket::new(TdsPacketType::Response, combined)?;
        PacketCodec::write_packet(stream, &packet).await?;
        Ok(())
    }
}

/// 从 CommandComplete 标签中解析受影响行数。
fn parse_affected_rows(tag: &str) -> u64 {
    tag.split_whitespace().last().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// 编码 RETURNVALUE token (0xAC) 返回 INTN(8) 类型的 i64 值。
///
/// 用于 sp_prepare 返回 @handle OUTPUT 参数。
///
/// 格式（MS-TDS 2.2.7.8）：
/// ```text
/// token(1B = 0xAC)
/// length(2B LE)      —— 后续 payload 字节数
/// param_ord(2B LE)   —— 参数序号（从1开始）
/// param_name(B_VARCHAR) —— 1B 长度 + UTF-16LE
/// status(1B)         —— 0x01 = OUTPUT
/// user_type(4B BE)   —— 用户类型
/// flags(2B BE)       —— 0x0001 (nullable)
/// type_info          —— INTN(0x26) + max_length(1B = 8)
/// value              —— length(1B = 8) + i64 LE
/// ```
fn encode_returnvalue_int64(param_ord: u16, param_name: &str, value: i64) -> Vec<u8> {
    let name_units: Vec<u16> = param_name.encode_utf16().collect();
    let name_bytes: Vec<u8> = name_units
        .iter()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    // 计算 payload 长度（从 param_ord 到 value 结束）
    // param_ord(2) + name_len(1) + name + status(1) + user_type(4) + flags(2) + type_info(2) + value_len(1) + value(8)
    let payload_len = 2 + 1 + name_bytes.len() + 1 + 4 + 2 + 2 + 1 + 8;
    let mut buf = Vec::with_capacity(3 + payload_len);
    // token
    buf.push(0xAC);
    // length（LE）
    buf.extend_from_slice(&(payload_len as u16).to_le_bytes());
    // param_ord（LE）
    buf.extend_from_slice(&param_ord.to_le_bytes());
    // param_name（B_VARCHAR：1B 长度 + UTF-16LE）
    buf.push(name_units.len() as u8);
    buf.extend_from_slice(&name_bytes);
    // status（0x01 = OUTPUT）
    buf.push(0x01);
    // user_type（4B BE，固定 0）
    buf.extend_from_slice(&0u32.to_be_bytes());
    // flags（2B BE，0x0001 = nullable）
    buf.extend_from_slice(&0x0001u16.to_be_bytes());
    // type_info：INTN(0x26) + max_length(8)
    buf.push(0x26);
    buf.push(8);
    // value：length(8) + i64 LE
    buf.push(8);
    buf.extend_from_slice(&value.to_le_bytes());
    buf
}

// =====================================================================
//  RPC 参数绑定 — 将 RPC 参数值替换为 SQL 字面量
// =====================================================================

/// 将 sp_executesql 的参数值替换到 SQL 文本中。
///
/// 解析 `param_def`（如 "@p1 int, @p2 varchar(10)"）获取参数名列表，
/// 然后按顺序与 `values` 配对，将 SQL 中的 `@p1`/`@p2`/... 替换为
/// 对应的 SQL 字面量。
fn substitute_rpc_params(parsed: &crate::command::SpExecutesqlParams<'_>) -> String {
    let param_names = parse_param_def_names(&parsed.param_def);
    substitute_params_in_sql(&parsed.sql, &param_names, &parsed.values)
}

/// 将 sp_execute 的参数值替换到 SQL 文本中。
///
/// sp_execute 没有参数定义字符串，参数名按 `@p1`/`@p2`/... 顺序生成。
fn substitute_sp_execute_params(sql: &str, values: &[&crate::command::RpcParam]) -> String {
    if values.is_empty() {
        return sql.to_string();
    }
    let param_names: Vec<String> = (1..=values.len())
        .map(|i| format!("@p{i}"))
        .collect();
    substitute_params_in_sql(sql, &param_names, values)
}

/// 从参数定义字符串中提取参数名列表。
///
/// 例如 "@p1 int, @p2 varchar(10)" → ["@p1", "@p2"]
/// 参数名大小写不敏感，统一转为小写以匹配 SQL 中的占位符。
fn parse_param_def_names(param_def: &str) -> Vec<String> {
    param_def
        .split(',')
        .filter_map(|decl| {
            let decl = decl.trim();
            // 跳过空声明
            if decl.is_empty() {
                return None;
            }
            // 提取第一个 token（参数名，以 @ 开头）
            let name = decl.split_whitespace().next()?;
            if name.starts_with('@') {
                Some(name.to_lowercase())
            } else {
                None
            }
        })
        .collect()
}

/// 将参数值替换到 SQL 文本中。
///
/// `param_names` 与 `values` 按位置配对。
/// 替换是大小写不敏感的（SQL 中的 @P1 和 @p1 都能匹配）。
fn substitute_params_in_sql(
    sql: &str,
    param_names: &[String],
    values: &[&crate::command::RpcParam],
) -> String {
    if param_names.is_empty() || values.is_empty() {
        return sql.to_string();
    }
    let mut result = sql.to_string();
    for (i, name) in param_names.iter().enumerate() {
        if i >= values.len() {
            break;
        }
        let literal = rpc_param_to_sql_literal(values[i]);
        // 大小写不敏感替换 @p1 / @P1 / @P1 等
        // 使用简单的逐位置扫描替换，避免正则依赖
        let name_lower = name.to_lowercase();
        let mut replaced = String::with_capacity(result.len());
        let mut remaining = result.as_str();
        while !remaining.is_empty() {
            // 查找 '@' 位置
            if let Some(at_pos) = remaining.find('@') {
                replaced.push_str(&remaining[..at_pos]);
                let after_at = &remaining[at_pos..];
                // 尝试匹配参数名（大小写不敏感）
                let matched = name_lower
                    .len()
                    .min(after_at.len())
                    .eq(&name_lower.len())
                    && after_at[..name_lower.len()].eq_ignore_ascii_case(&name_lower);
                if matched {
                    // 确保后面不是标识符字符（避免 @p1 匹配到 @p10）
                    let after_match = &after_at[name_lower.len()..];
                    let is_boundary = after_match
                        .chars()
                        .next()
                        .map(|c| !c.is_alphanumeric() && c != '_')
                        .unwrap_or(true);
                    if is_boundary {
                        replaced.push_str(&literal);
                        remaining = after_match;
                        continue;
                    }
                }
                // 未匹配，保留 '@' 继续
                replaced.push('@');
                remaining = &after_at[1..];
            } else {
                replaced.push_str(remaining);
                break;
            }
        }
        result = replaced;
    }
    result
}

/// 将 RpcParam 转换为 SQL 字面量字符串。
///
/// 根据 TDS 类型字节选择合适的解码方式：
/// - INTN (0x26) / BIT (0x68) → 整数字面量
/// - FloatN (0x6E) → 浮点字面量
/// - NVarChar (0xE7) / NChar (0xE6) → 带引号的字符串（单引号转义）
/// - BigVarChar (0xA7) → 带引号的 ANSI 字符串
/// - NULL (value is None) → "NULL"
/// - 其他类型 → 尝试字符串/整数解码，回退到 NULL
fn rpc_param_to_sql_literal(param: &crate::command::RpcParam) -> String {
    // NULL 值
    if param.value.is_none() {
        return "NULL".to_string();
    }

    match param.type_byte {
        // INTN (0x26) — 变长整数
        0x26 => match param.as_int() {
            Some(n) => n.to_string(),
            None => "NULL".to_string(),
        },
        // BIT (0x68) — 布尔
        0x68 => match param.as_int() {
            Some(0) => "0".to_string(),
            Some(_) => "1".to_string(),
            None => "NULL".to_string(),
        },
        // FloatN (0x6E) — 浮点数
        0x6E => {
            let bytes = param.value.as_ref().unwrap();
            match bytes.len() {
                4 => {
                    let f = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    format_float_literal(f as f64)
                }
                8 => {
                    let f = f64::from_le_bytes([
                        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
                        bytes[7],
                    ]);
                    format_float_literal(f)
                }
                _ => "NULL".to_string(),
            }
        }
        // NVarChar (0xE7) / NChar (0xE6) — Unicode 字符串
        0xE7 | 0xE6 => match param.as_string() {
            Some(s) => escape_sql_string(&s),
            None => "NULL".to_string(),
        },
        // BigVarChar (0xA7) — ANSI 字符串
        0xA7 => match param.as_ansi_string() {
            Some(s) => escape_sql_string(&s),
            None => "NULL".to_string(),
        },
        // 其他类型：尝试字符串解码，回退到 NULL
        _ => {
            if let Some(s) = param.as_string() {
                escape_sql_string(&s)
            } else if let Some(n) = param.as_int() {
                n.to_string()
            } else {
                "NULL".to_string()
            }
        }
    }
}

/// 将字符串转义为 SQL 字面量（单引号包裹，内部单引号双写）。
fn escape_sql_string(s: &str) -> String {
    let escaped = s.replace('\'', "''");
    format!("'{escaped}'")
}

/// 格式化浮点数为 SQL 字面量（避免科学计数法，保持精度）。
fn format_float_literal(f: f64) -> String {
    if f.is_nan() || f.is_infinite() {
        "NULL".to_string()
    } else {
        // 使用 Debug 格式保证全精度，与 Rust 的 Display 不同
        format!("{f:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handshake::TDS_VERSION_71;

    #[test]
    fn test_tds_config_default() {
        let config = TdsConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 1433);
        assert_eq!(config.server_version, "15.00.2000");
        assert_eq!(config.auth_mode, AuthMode::Trust);
    }

    #[test]
    fn test_tds_config_builder() {
        let config = TdsConfig::new()
            .with_host("0.0.0.0")
            .with_port(1434)
            .with_server_version("14.00.1000")
            .with_allowed_databases(vec!["master".to_string()]);
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 1434);
        assert_eq!(config.server_version, "14.00.1000");
        assert_eq!(config.allowed_databases, vec!["master".to_string()]);
    }

    #[test]
    fn test_parse_affected_rows_insert() {
        assert_eq!(parse_affected_rows("INSERT 0 5"), 5);
    }

    #[test]
    fn test_parse_affected_rows_update() {
        assert_eq!(parse_affected_rows("UPDATE 3"), 3);
    }

    #[test]
    fn test_parse_affected_rows_delete() {
        assert_eq!(parse_affected_rows("DELETE 7"), 7);
    }

    #[test]
    fn test_parse_affected_rows_invalid() {
        assert_eq!(parse_affected_rows("invalid"), 0);
    }

    #[test]
    fn test_parse_affected_rows_empty() {
        assert_eq!(parse_affected_rows(""), 0);
    }

    #[test]
    fn test_tds_server_new() {
        let config = TdsConfig::new().with_port(1444);
        let server = TdsServer::new(config);
        assert_eq!(server.config.port, 1444);
        assert_eq!(server.connection_id_counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_connection_new_initial_state() {
        let config = TdsConfig::default();
        let conn = Connection::new(config, 42);
        assert_eq!(conn.conn_id, 42);
        assert!(conn.current_db.is_none());
        // 新增字段初始状态
        assert!(conn.prepared_statements.is_empty());
        assert_eq!(conn.next_prepare_handle, 1);
    }

    #[test]
    fn test_connection_allocate_handle_increments() {
        let config = TdsConfig::default();
        let mut conn = Connection::new(config, 1);
        let h1 = conn.allocate_handle();
        let h2 = conn.allocate_handle();
        let h3 = conn.allocate_handle();
        assert_eq!(h1, 1);
        assert_eq!(h2, 2);
        assert_eq!(h3, 3);
        assert_eq!(conn.next_prepare_handle, 4);
    }

    #[test]
    fn test_connection_prepared_statements_storage() {
        let config = TdsConfig::default();
        let mut conn = Connection::new(config, 1);
        let handle = conn.allocate_handle();
        conn.prepared_statements
            .insert(handle, "SELECT 1".to_string());
        assert_eq!(conn.prepared_statements.len(), 1);
        assert_eq!(
            conn.prepared_statements.get(&handle),
            Some(&"SELECT 1".to_string())
        );
        // 无效 handle
        assert!(!conn.prepared_statements.contains_key(&999));
    }

    #[test]
    fn test_encode_returnvalue_int64_basic() {
        let bytes = encode_returnvalue_int64(1, "@handle", 42);
        // token
        assert_eq!(bytes[0], 0xAC);
        // length（2B LE）
        let payload_len = u16::from_le_bytes([bytes[1], bytes[2]]) as usize;
        // param_ord(2) + name_len(1) + name("@handle"=7*2=14) + status(1) + user_type(4) + flags(2) + type_info(2) + value_len(1) + value(8)
        // = 2 + 1 + 14 + 1 + 4 + 2 + 2 + 1 + 8 = 35
        assert_eq!(payload_len, 35);
        assert_eq!(bytes.len(), 3 + payload_len);
        // param_ord（2B LE = 1）
        assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 1);
        // param_name 长度（1B = 7 字符）
        assert_eq!(bytes[5], 7);
        // status（0x01 = OUTPUT）
        assert_eq!(bytes[5 + 1 + 14], 0x01);
        // type_info: INTN(0x26) + max_length(8)
        let type_info_pos = 5 + 1 + 14 + 1 + 4 + 2;
        assert_eq!(bytes[type_info_pos], 0x26);
        assert_eq!(bytes[type_info_pos + 1], 8);
        // value: length(8) + i64 LE
        assert_eq!(bytes[type_info_pos + 2], 8);
        let value = i64::from_le_bytes([
            bytes[type_info_pos + 3],
            bytes[type_info_pos + 4],
            bytes[type_info_pos + 5],
            bytes[type_info_pos + 6],
            bytes[type_info_pos + 7],
            bytes[type_info_pos + 8],
            bytes[type_info_pos + 9],
            bytes[type_info_pos + 10],
        ]);
        assert_eq!(value, 42);
    }

    #[test]
    fn test_encode_returnvalue_int64_handle_values() {
        // 测试不同的 handle 值
        for handle in [1i64, 100, i64::MAX, 0, -1] {
            let bytes = encode_returnvalue_int64(1, "@handle", handle);
            assert_eq!(bytes[0], 0xAC);
            // 解析 value
            let type_info_pos = 5 + 1 + 14 + 1 + 4 + 2;
            let value = i64::from_le_bytes([
                bytes[type_info_pos + 3],
                bytes[type_info_pos + 4],
                bytes[type_info_pos + 5],
                bytes[type_info_pos + 6],
                bytes[type_info_pos + 7],
                bytes[type_info_pos + 8],
                bytes[type_info_pos + 9],
                bytes[type_info_pos + 10],
            ]);
            assert_eq!(value, handle);
        }
    }

    #[test]
    fn test_encode_returnvalue_int64_param_ord() {
        // 测试不同的 param_ord
        for ord in [1u16, 2, 10, 100] {
            let bytes = encode_returnvalue_int64(ord, "@handle", 1);
            let parsed_ord = u16::from_le_bytes([bytes[3], bytes[4]]);
            assert_eq!(parsed_ord, ord);
        }
    }

    #[test]
    fn test_tds_config_with_auth_mode() {
        let config = TdsConfig::new().with_auth_mode(AuthMode::Trust);
        assert_eq!(config.auth_mode, AuthMode::Trust);
    }

    #[test]
    fn test_tds_version_constant() {
        // TDS 7.1 = 0x71000001
        assert_eq!(TDS_VERSION_71, 0x71000001);
    }
}
