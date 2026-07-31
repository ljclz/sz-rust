//! MySQL 服务器主入口 — TCP 监听 + 握手 + 命令循环。

use crate::auth::{AuthError, AuthMode, AuthSession};
use crate::command::{parse_command, Command, CommandError, InitDbCommand, QueryCommand};
use crate::handshake::{
    error_codes, sql_states, ErrPacket, HandshakeError, HandshakeResponse41, HandshakeV10,
    OkPacket, SERVER_MORE_RESULTS_EXISTS, SERVER_STATUS_AUTOCOMMIT,
};
use crate::packet::{Packet, PacketCodec, PacketError};
use crate::prepared_statement::{
    encode_binary_row, substitute_placeholders, PreparedStatementStore, StmtCloseCommand,
    StmtExecuteCommand, StmtPrepareCommand, StmtResetCommand, StmtSendLongDataCommand,
    PrepareOkPacket,
};
use crate::result_set::{ColumnDefinition, ResultSetEncoder};
use crate::types::MysqlType;
use szrsql_protocol::pgwire::session::{ExecutorService, QueryResult};
use szrsql_protocol::pgwire::InMemoryTable;
use szrsql_types::value::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, RwLock};

#[derive(Debug, Clone)]
pub struct MysqlConfig {
    pub host: String,
    pub port: u16,
    pub server_version: String,
    pub auth_mode: AuthMode,
    pub allowed_databases: Vec<String>,
    /// 连接空闲超时（默认 300s = 5 分钟；`Duration::ZERO` 表示禁用）。
    ///
    /// 当连接在此时间内未收到任何客户端消息时，服务器主动关闭连接并释放
    /// session 资源，避免客户端异常断开导致的死锁。
    pub connection_idle_timeout: std::time::Duration,
}

impl Default for MysqlConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 3306,
            server_version: "8.0.32-szrsql".to_string(),
            auth_mode: AuthMode::Trust,
            allowed_databases: Vec::new(),
            connection_idle_timeout: std::time::Duration::from_secs(300),
        }
    }
}

impl MysqlConfig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }
    pub fn with_server_version(mut self, version: impl Into<String>) -> Self {
        self.server_version = version.into();
        self
    }
    pub fn with_auth_mode(mut self, mode: AuthMode) -> Self {
        self.auth_mode = mode;
        self
    }
    /// 设置连接空闲超时（`Duration::ZERO` 表示禁用）。
    pub fn with_connection_idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.connection_idle_timeout = timeout;
        self
    }
}

#[derive(Debug, Error)]
pub enum MysqlServerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("packet error: {0}")]
    Packet(#[from] PacketError),
    #[error("handshake error: {0}")]
    Handshake(#[from] HandshakeError),
    #[error("auth error: {0}")]
    Auth(#[from] AuthError),
    #[error("command error: {0}")]
    Command(#[from] CommandError),
}

pub struct MysqlServer {
    config: MysqlConfig,
    connection_id_counter: AtomicI32,
    /// ADV-CONC-1：跨会话/跨协议共享的表存储（与 PgwireServer 共享同一实例）
    shared_tables: Option<Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>>,
    /// ADV-CONC-1：跨会话/跨协议共享的行锁管理器
    lock_manager: Option<Arc<szrsql_tx::lock::LockManager>>,
    /// ADV-CONC-1：跨会话/跨协议共享的事务 ID 计数器
    shared_txn_counter: Option<Arc<AtomicU32>>,
}

impl MysqlServer {
    pub fn new(config: MysqlConfig) -> Self {
        Self {
            config,
            connection_id_counter: AtomicI32::new(1),
            shared_tables: None,
            lock_manager: None,
            shared_txn_counter: None,
        }
    }

    /// 注入共享表存储（与 PgwireServer 共享同一实例）
    pub fn with_shared_tables(mut self, tables: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>) -> Self {
        self.shared_tables = Some(tables);
        self
    }

    /// 注入共享行锁管理器
    pub fn with_lock_manager(mut self, lm: Arc<szrsql_tx::lock::LockManager>) -> Self {
        self.lock_manager = Some(lm);
        self
    }

    /// 注入共享事务 ID 计数器
    pub fn with_shared_txn_counter(mut self, counter: Arc<AtomicU32>) -> Self {
        self.shared_txn_counter = Some(counter);
        self
    }

    pub async fn serve(self) -> Result<(), MysqlServerError> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("MySQL server listening on {}", addr);
        let self_arc = Arc::new(self);
        loop {
            let (stream, peer_addr) = listener.accept().await?;
            tracing::debug!("MySQL connection from {}", peer_addr);
            let server = Arc::clone(&self_arc);
            tokio::spawn(async move {
                if let Err(e) = server.handle_connection(stream).await {
                    tracing::warn!("MySQL connection error: {}", e);
                }
            });
        }
    }

    pub async fn handle_connection(&self, stream: TcpStream) -> Result<(), MysqlServerError> {
        let conn_id = self.connection_id_counter.fetch_add(1, Ordering::SeqCst) as u32;
        let mut conn = Connection::new(self.config.clone(), conn_id);
        // 注入共享存储到 ExecutorService
        let mut executor = ExecutorService::new();
        if let Some(st) = &self.shared_tables {
            executor = executor.with_shared_tables(st.clone());
            // 同时注入到 Connection，供元数据查询处理器读取真实表结构
            conn.shared_tables = Some(st.clone());
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

struct Connection {
    config: MysqlConfig,
    conn_id: u32,
    seq_id: u8,
    executor: Arc<Mutex<ExecutorService>>,
    current_db: Option<String>,
    /// 跨会话共享的表存储（用于元数据查询处理器读取真实表结构）
    shared_tables: Option<Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>>,
    /// 当前连接的 Prepared Statement 存储（按 stmt_id 索引）
    prepared_statements: PreparedStatementStore,
}

impl Connection {
    fn new(config: MysqlConfig, conn_id: u32) -> Self {
        Self {
            config,
            conn_id,
            seq_id: 0,
            executor: Arc::new(Mutex::new(ExecutorService::new())),
            current_db: None,
            shared_tables: None,
            prepared_statements: PreparedStatementStore::new(),
        }
    }

    async fn handle(&mut self, mut stream: TcpStream) -> Result<(), MysqlServerError> {
        let username = self.do_handshake(&mut stream).await?;
        tracing::info!("MySQL conn {} authenticated as '{}'", self.conn_id, username);
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
                            "MySQL connection idle timeout, closing"
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
            self.seq_id = packet.seq_id.wrapping_add(1);
            let (cmd, payload) = parse_command(&packet.payload)?;
            match cmd {
                Command::Quit => break,
                Command::Query => {
                    let query = QueryCommand::parse(payload);
                    self.handle_query(&mut stream, &query.sql).await?;
                }
                Command::InitDb => {
                    let init = InitDbCommand::parse(payload);
                    self.handle_init_db(&mut stream, &init.database).await?;
                }
                Command::Ping => {
                    self.send_ok(&mut stream, &OkPacket::simple()).await?;
                }
                Command::Statistics => {
                    let stats = "Uptime: 0  Threads: 1";
                    let packet = Packet::new(self.seq_id, stats.as_bytes().to_vec())?;
                    PacketCodec::write_packet(&mut stream, &packet).await?;
                }
                Command::StmtPrepare => {
                    let prepare_cmd = StmtPrepareCommand::parse(payload);
                    self.handle_stmt_prepare(&mut stream, &prepare_cmd.sql).await?;
                }
                Command::StmtExecute => {
                    match self.handle_stmt_execute(&mut stream, payload).await {
                        Ok(()) => {}
                        Err(e) => {
                            tracing::warn!(conn_id = self.conn_id, error = %e, "STMT_EXECUTE failed");
                        }
                    }
                }
                Command::StmtClose => {
                    if let Ok(close_cmd) = StmtCloseCommand::parse(payload) {
                        self.prepared_statements.close(close_cmd.stmt_id);
                        // COM_STMT_CLOSE 无响应包
                        tracing::debug!(
                            conn_id = self.conn_id,
                            stmt_id = close_cmd.stmt_id,
                            "STMT_CLOSE: prepared statement released"
                        );
                    }
                }
                Command::StmtReset => {
                    match StmtResetCommand::parse(payload) {
                        Ok(reset_cmd) => {
                            if let Some(stmt) = self.prepared_statements.get_mut(reset_cmd.stmt_id) {
                                stmt.reset();
                            }
                            // COM_STMT_RESET 响应 OK 包
                            self.send_ok(&mut stream, &OkPacket::simple()).await?;
                        }
                        Err(e) => {
                            let err = ErrPacket::new(
                                error_codes::INTERNAL_ERROR,
                                sql_states::GENERAL,
                                format!("STMT_RESET parse error: {e}"),
                            );
                            self.send_err(&mut stream, &err).await?;
                        }
                    }
                }
                Command::StmtSendLongData => {
                    match StmtSendLongDataCommand::parse(payload) {
                        Ok(sld_cmd) => {
                            if let Some(stmt) = self.prepared_statements.get_mut(sld_cmd.stmt_id) {
                                stmt.append_long_data(sld_cmd.param_id, &sld_cmd.data);
                            }
                            // COM_STMT_SEND_LONG_DATA 无响应包
                            tracing::debug!(
                                conn_id = self.conn_id,
                                stmt_id = sld_cmd.stmt_id,
                                param_id = sld_cmd.param_id,
                                data_len = sld_cmd.data.len(),
                                "STMT_SEND_LONG_DATA: long data buffered"
                            );
                        }
                        Err(e) => {
                            let err = ErrPacket::new(
                                error_codes::INTERNAL_ERROR,
                                sql_states::GENERAL,
                                format!("STMT_SEND_LONG_DATA parse error: {e}"),
                            );
                            self.send_err(&mut stream, &err).await?;
                        }
                    }
                }
                Command::StmtFetch => {
                    // COM_STMT_FETCH：当前实现不使用游标，直接返回 EOF 表示无更多数据
                    let eof = ResultSetEncoder::encode_eof(0, SERVER_STATUS_AUTOCOMMIT);
                    let packet = Packet::new(self.seq_id, eof)?;
                    PacketCodec::write_packet(&mut stream, &packet).await?;
                    self.seq_id = self.seq_id.wrapping_add(1);
                }
                _ => {
                    let err = ErrPacket::new(
                        error_codes::INTERNAL_ERROR,
                        sql_states::GENERAL,
                        format!("unsupported command: {:?}", cmd),
                    );
                    self.send_err(&mut stream, &err).await?;
                }
            }
        }
        Ok(())
    }

    async fn do_handshake(&mut self, stream: &mut TcpStream) -> Result<String, MysqlServerError> {
        let mut auth_session = AuthSession::new(self.config.auth_mode.clone());
        let salt = *auth_session.salt();
        let handshake = HandshakeV10::new(self.config.server_version.clone(), self.conn_id, &salt);
        let packet = Packet::new(self.seq_id, handshake.encode())?;
        PacketCodec::write_packet(stream, &packet).await?;
        self.seq_id = self.seq_id.wrapping_add(1);
        let response_packet = PacketCodec::read_packet(stream).await?;
        self.seq_id = response_packet.seq_id.wrapping_add(1);
        let response = HandshakeResponse41::decode(&response_packet.payload)?;
        tracing::debug!(
            target: "mysql_handshake",
            capability_flags = ?response.capability_flags,
            username = %response.username,
            database = ?response.database,
            auth_plugin_name = %response.auth_plugin_name,
            auth_response_len = response.auth_response.len(),
            "handshake response decoded"
        );
        let auth_plugin = if response.auth_plugin_name.is_empty() {
            "mysql_native_password"
        } else {
            &response.auth_plugin_name
        };
        match auth_session.verify(&response.username, &response.auth_response, auth_plugin) {
            Ok(username) => {
                if let Some(db) = &response.database {
                    if !self.config.allowed_databases.is_empty()
                        && !self.config.allowed_databases.iter().any(|d| d == db)
                    {
                        let err = ErrPacket::new(
                            error_codes::BAD_DB,
                            sql_states::GENERAL,
                            format!("Unknown database '{}'", db),
                        );
                        self.send_err(stream, &err).await?;
                        return Err(MysqlServerError::Auth(AuthError::AccessDenied(
                            response.username.clone(),
                        )));
                    }
                    self.current_db = Some(db.clone());
                }
                self.send_ok(stream, &OkPacket::simple()).await?;
                Ok(username)
            }
            Err(e) => {
                let err = ErrPacket::new(
                    error_codes::ACCESS_DENIED,
                    sql_states::ACCESS_DENIED,
                    format!("Access denied for user '{}'", response.username),
                );
                self.send_err(stream, &err).await?;
                Err(e.into())
            }
        }
    }

    async fn handle_query(&mut self, stream: &mut TcpStream, sql: &str) -> Result<(), MysqlServerError> {
        let trimmed = sql.trim();
        if trimmed.is_empty() {
            self.send_ok(stream, &OkPacket::simple()).await?;
            return Ok(());
        }

        // Navicat 等客户端会发送分号分隔的多语句（multi-statement），
        // 例如：`SHOW VARIABLES LIKE 'a'; SHOW VARIABLES LIKE 'b'; SELECT ...`
        // MySQL 协议要求每条语句返回独立的结果集或 OK 包。
        // 这里按分号拆分（忽略字符串字面量内的分号），逐条执行并返回结果。
        let statements = split_sql_statements(trimmed);
        let non_empty: Vec<&str> = statements.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        let total = non_empty.len();

        for (idx, stmt) in non_empty.iter().enumerate() {
            // 多语句查询：除最后一条语句外，每条语句的响应都需要设置 SERVER_MORE_RESULTS_EXISTS
            let is_last = idx + 1 == total;
            let status_flags = if is_last {
                SERVER_STATUS_AUTOCOMMIT
            } else {
                SERVER_STATUS_AUTOCOMMIT | SERVER_MORE_RESULTS_EXISTS
            };

            // 先尝试元数据查询处理器（SHOW TABLE STATUS / SHOW CREATE TABLE / SHOW COLUMNS /
            // SHOW INDEX / SHOW FULL TABLES / information_schema.* 等）。
            // 命中则直接返回 MySQL 兼容的结果集，跳过正常 SQL 执行路径。
            if let Some((columns, rows)) = crate::mysql_metadata::try_handle_metadata_query(
                stmt,
                &self.current_db,
                &self.shared_tables,
            )
            .await
            {
                tracing::info!(
                    conn_id = self.conn_id,
                    raw_sql = %stmt,
                    col_count = columns.len(),
                    row_count = rows.len(),
                    "MySQL metadata query intercepted"
                );
                self.send_result_set_with_flags(stream, &columns, &rows, status_flags)
                    .await?;
                continue;
            }

            // MySQL 事务语句拦截
            // Navicat/pymysql 默认 autocommit=False，每次执行后调用 commit()。
            // 但 PG session 严格要求 in_transaction() 才能 COMMIT/ROLLBACK，否则报错。
            // MySQL 行为：autocommit 模式下 COMMIT/ROLLBACK 是 no-op（返回 OK 包）。
            // 修复：直接发送 OK 包，不走 executor，避免 PG 严格事务检查。
            //
            // 同时拦截 "END"：MySQL CREATE FUNCTION ... BEGIN ... END 中的 END
            // 可能被 split_sql_statements 误切分为独立语句。PG 方言将 "END" 解析为
            // COMMIT 的同义词，导致 "no transaction in progress" 错误。
            // MySQL 中 END 不是独立语句，作为 no-op 处理。
            let stmt_upper = stmt.to_uppercase();
            let trimmed_upper = stmt_upper.trim_end_matches(';').trim();
            if trimmed_upper == "COMMIT"
                || trimmed_upper == "ROLLBACK"
                || trimmed_upper == "END"
            {
                tracing::info!(
                    conn_id = self.conn_id,
                    raw_sql = %stmt,
                    "MySQL transaction control (no-op in autocommit mode)"
                );
                let ok = OkPacket {
                    affected_rows: 0,
                    last_insert_id: 0,
                    status_flags,
                    warnings: 0,
                    info: String::new(),
                };
                self.send_ok(stream, &ok).await?;
                continue;
            }

            // SELECT @@xxx 系统变量查询拦截器（规则2：字节级精确返回）
            // 直接发送 MySQL 协议级响应，确保布尔值返回 1/0（LONGLONG=8）、字符串返回标准格式
            let stmt_trimmed = stmt.trim_end_matches(';').trim();
            if stmt_trimmed.to_uppercase().starts_with("SELECT @@") {
                if let Some((columns, rows)) = build_system_variable_response(stmt_trimmed) {
                    self.send_result_set_with_flags(stream, &columns, &rows, status_flags).await?;
                    continue;
                }
            }

            // MySQL 兼容：把反引号标识符替换成双引号（sqlparser 不支持反引号）
            // 同时把 SET @@SESSION.xxx / SET @@GLOBAL.xxx 替换成 SET SESSION xxx / SET GLOBAL xxx
            let normalized_sql = normalize_mysql_sql(stmt, self.current_db.as_deref(), &self.config.allowed_databases);
            tracing::info!(conn_id = self.conn_id, raw_sql = %stmt, normalized_sql = %normalized_sql, "MySQL query received");
            let mut executor = self.executor.lock().await;
            let results = executor.execute_sql(&normalized_sql).await;
            drop(executor);
            for result in results {
                match result {
                    Ok(QueryResult::ResultSet { columns, rows, .. }) => {
                        self.send_result_set_with_flags(stream, &columns, &rows, status_flags).await?;
                    }
                    Ok(QueryResult::AffectedRows { tag }) => {
                        let ok = OkPacket {
                            affected_rows: parse_affected_rows(&tag),
                            last_insert_id: 0,
                            status_flags,
                            warnings: 0,
                            info: tag,
                        };
                        self.send_ok(stream, &ok).await?;
                    }
                    Ok(QueryResult::DdlComplete { tag }) => {
                        let ok = OkPacket {
                            affected_rows: 0,
                            last_insert_id: 0,
                            status_flags,
                            warnings: 0,
                            info: tag,
                        };
                        self.send_ok(stream, &ok).await?;
                    }
                    Ok(QueryResult::TransactionComplete { tag, .. }) => {
                        let ok = OkPacket {
                            affected_rows: 0,
                            last_insert_id: 0,
                            status_flags,
                            warnings: 0,
                            info: tag,
                        };
                        self.send_ok(stream, &ok).await?;
                    }
                    Ok(QueryResult::Empty) => {
                        let ok = OkPacket {
                            affected_rows: 0,
                            last_insert_id: 0,
                            status_flags,
                            warnings: 0,
                            info: String::new(),
                        };
                        self.send_ok(stream, &ok).await?;
                    }
                    Err(e) => {
                        let (err_code, sql_state) = map_executor_error(&e);
                        let err = ErrPacket::new(err_code, sql_state, e.to_string());
                        self.send_err(stream, &err).await?;
                        // 多语句中某条失败，后续语句不再执行（与 MySQL 默认行为一致）
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    }

    async fn handle_init_db(&mut self, stream: &mut TcpStream, database: &str) -> Result<(), MysqlServerError> {
        if !self.config.allowed_databases.is_empty()
            && !self.config.allowed_databases.iter().any(|d| d == database)
        {
            let err = ErrPacket::new(
                error_codes::BAD_DB,
                sql_states::GENERAL,
                format!("Unknown database '{}'", database),
            );
            self.send_err(stream, &err).await?;
        } else {
            self.current_db = Some(database.to_string());
            self.send_ok(stream, &OkPacket::simple()).await?;
        }
        Ok(())
    }

    async fn send_result_set_with_flags(&mut self, stream: &mut TcpStream, columns: &[szrsql_protocol::pgwire::session::ResultColumn], rows: &[Vec<szrsql_types::value::Value>], status_flags: u16) -> Result<(), MysqlServerError> {
        let col_defs: Vec<ColumnDefinition> = columns
            .iter()
            .map(|c| ColumnDefinition::new(&c.name, MysqlType::from_column_type(&c.column_type)))
            .collect();
        let packets = ResultSetEncoder::encode_result_set_with_flags(&col_defs, rows, status_flags);
        for payload in packets {
            let packet = Packet::new(self.seq_id, payload)?;
            PacketCodec::write_packet(stream, &packet).await?;
            self.seq_id = self.seq_id.wrapping_add(1);
        }
        Ok(())
    }

    async fn send_ok(&mut self, stream: &mut TcpStream, ok: &OkPacket) -> Result<(), MysqlServerError> {
        let packet = Packet::new(self.seq_id, ok.encode())?;
        PacketCodec::write_packet(stream, &packet).await?;
        self.seq_id = self.seq_id.wrapping_add(1);
        Ok(())
    }

    async fn send_err(&mut self, stream: &mut TcpStream, err: &ErrPacket) -> Result<(), MysqlServerError> {
        let packet = Packet::new(self.seq_id, err.encode())?;
        PacketCodec::write_packet(stream, &packet).await?;
        self.seq_id = self.seq_id.wrapping_add(1);
        Ok(())
    }

    /// 处理 COM_STMT_PREPARE：注册 SQL，返回 PREPARE_OK + 参数/列元数据。
    ///
    /// 响应包序列：
    /// 1. PREPARE_OK（stmt_id + num_columns + num_params）
    /// 2. 参数列定义（num_params 个 ColumnDefinition 包）
    /// 3. EOF 包（若 num_params > 0）
    /// 4. 结果列定义（num_columns 个 ColumnDefinition 包）
    /// 5. EOF 包（若 num_columns > 0）
    ///
    /// 由于此处无法预知 SQL 的结果列（需要执行 EXPLAIN 才能得到），我们采用简化策略：
    /// - 参数数量通过解析 `?` 占位符得到
    /// - 结果列数量设为 0（客户端在 COM_STMT_EXECUTE 后会收到真实列定义）
    async fn handle_stmt_prepare(
        &mut self,
        stream: &mut TcpStream,
        sql: &str,
    ) -> Result<(), MysqlServerError> {
        let normalized = normalize_mysql_sql(sql, self.current_db.as_deref(), &self.config.allowed_databases);
        let stmt_id = self.prepared_statements.prepare(normalized.clone());
        let num_params = self
            .prepared_statements
            .get(stmt_id)
            .map(|s| s.num_params)
            .unwrap_or(0);
        let num_columns: u16 = 0; // 结果列在 EXECUTE 时才发送

        tracing::info!(
            conn_id = self.conn_id,
            stmt_id,
            num_params,
            sql = %normalized,
            "STMT_PREPARE: prepared statement registered"
        );

        // 1. PREPARE_OK
        let prepare_ok = PrepareOkPacket::new(stmt_id, num_columns, num_params);
        let packet = Packet::new(self.seq_id, prepare_ok.encode())?;
        PacketCodec::write_packet(stream, &packet).await?;
        self.seq_id = self.seq_id.wrapping_add(1);

        // 2. 参数列定义（每个参数一个 ColumnDefinition，类型为 VarString 占位）
        for i in 0..num_params {
            let col = ColumnDefinition::new(format!("?{}", i), MysqlType::VarString);
            let packet = Packet::new(self.seq_id, col.encode())?;
            PacketCodec::write_packet(stream, &packet).await?;
            self.seq_id = self.seq_id.wrapping_add(1);
        }

        // 3. 参数 EOF（若 num_params > 0 且未启用 CLIENT_DEPRECATE_EOF）
        if num_params > 0 {
            let eof = ResultSetEncoder::encode_eof(0, SERVER_STATUS_AUTOCOMMIT);
            let packet = Packet::new(self.seq_id, eof)?;
            PacketCodec::write_packet(stream, &packet).await?;
            self.seq_id = self.seq_id.wrapping_add(1);
        }

        // 4. 结果列定义（num_columns=0，跳过）
        // 5. 结果 EOF（若 num_columns > 0，跳过）

        Ok(())
    }

    /// 处理 COM_STMT_EXECUTE：解码参数，替换占位符，执行 SQL，返回二进制行结果集。
    ///
    /// 响应包序列（结果集情况）：
    /// 1. 列数（lenenc int）
    /// 2. 列定义（每个 ColumnDefinition）
    /// 3. EOF 包
    /// 4. 二进制行（每行以 0x00 开头）
    /// 5. EOF / OK 包
    async fn handle_stmt_execute(
        &mut self,
        stream: &mut TcpStream,
        payload: &[u8],
    ) -> Result<(), MysqlServerError> {
        // 1. 读取 prepared statement 的参数数量和 long_data
        let (num_params, long_data_clone, sql_clone) = {
            let peek = match StmtExecuteCommand::parse(payload, 0, &HashMap::new()) {
                Ok(cmd) => cmd,
                Err(e) => {
                    let err = ErrPacket::new(
                        error_codes::INTERNAL_ERROR,
                        sql_states::GENERAL,
                        format!("STMT_EXECUTE header parse error: {e}"),
                    );
                    self.send_err(stream, &err).await?;
                    return Err(MysqlServerError::Command(CommandError::Execution(format!(
                        "STMT_EXECUTE parse error: {e}"
                    ))));
                }
            };
            let stmt_id = peek.stmt_id;
            let stmt = match self.prepared_statements.get(stmt_id) {
                Some(s) => s,
                None => {
                    let err = ErrPacket::new(
                        error_codes::INTERNAL_ERROR,
                        sql_states::GENERAL,
                        format!("unknown stmt_id: {}", stmt_id),
                    );
                    self.send_err(stream, &err).await?;
                    return Err(MysqlServerError::Command(CommandError::Execution(format!(
                        "unknown stmt_id: {stmt_id}"
                    ))));
                }
            };
            (stmt.num_params, stmt.long_data.clone(), stmt.sql.clone())
        };

        // 2. 解码完整参数
        let exec_cmd = match StmtExecuteCommand::parse(payload, num_params, &long_data_clone) {
            Ok(cmd) => cmd,
            Err(e) => {
                let err = ErrPacket::new(
                    error_codes::INTERNAL_ERROR,
                    sql_states::GENERAL,
                    format!("STMT_EXECUTE param decode error: {e}"),
                );
                self.send_err(stream, &err).await?;
                return Err(MysqlServerError::Command(CommandError::Execution(format!(
                    "STMT_EXECUTE param decode error: {e}"
                ))));
            }
        };

        // 3. 替换占位符
        let final_sql = match substitute_placeholders(&sql_clone, &exec_cmd.params) {
            Some(s) => s,
            None => {
                let err = ErrPacket::new(
                    error_codes::INTERNAL_ERROR,
                    sql_states::GENERAL,
                    "parameter count mismatch with placeholders",
                );
                self.send_err(stream, &err).await?;
                return Ok(());
            }
        };

        tracing::info!(
            conn_id = self.conn_id,
            stmt_id = exec_cmd.stmt_id,
            param_count = exec_cmd.params.len(),
            final_sql = %final_sql,
            "STMT_EXECUTE: executing substituted SQL"
        );

        // 4. 执行 SQL
        let mut executor = self.executor.lock().await;
        let results = executor.execute_sql(&final_sql).await;
        drop(executor);

        // 5. 发送响应（二进制协议行）
        for result in results {
            match result {
                Ok(QueryResult::ResultSet { columns, rows, .. }) => {
                    self.send_binary_result_set(stream, &columns, &rows).await?;
                }
                Ok(QueryResult::AffectedRows { tag }) => {
                    let ok = OkPacket {
                        affected_rows: parse_affected_rows(&tag),
                        last_insert_id: 0,
                        status_flags: SERVER_STATUS_AUTOCOMMIT,
                        warnings: 0,
                        info: tag,
                    };
                    self.send_ok(stream, &ok).await?;
                }
                Ok(QueryResult::DdlComplete { tag }) => {
                    let ok = OkPacket {
                        affected_rows: 0,
                        last_insert_id: 0,
                        status_flags: SERVER_STATUS_AUTOCOMMIT,
                        warnings: 0,
                        info: tag,
                    };
                    self.send_ok(stream, &ok).await?;
                }
                Ok(QueryResult::TransactionComplete { tag, .. }) => {
                    let ok = OkPacket {
                        affected_rows: 0,
                        last_insert_id: 0,
                        status_flags: SERVER_STATUS_AUTOCOMMIT,
                        warnings: 0,
                        info: tag,
                    };
                    self.send_ok(stream, &ok).await?;
                }
                Ok(QueryResult::Empty) => {
                    self.send_ok(stream, &OkPacket::simple()).await?;
                }
                Err(e) => {
                    let (err_code, sql_state) = map_executor_error(&e);
                    let err = ErrPacket::new(err_code, sql_state, e.to_string());
                    self.send_err(stream, &err).await?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// 发送二进制协议结果集（用于 COM_STMT_EXECUTE 响应）。
    async fn send_binary_result_set(
        &mut self,
        stream: &mut TcpStream,
        columns: &[szrsql_protocol::pgwire::session::ResultColumn],
        rows: &[Vec<szrsql_types::value::Value>],
    ) -> Result<(), MysqlServerError> {
        let col_defs: Vec<ColumnDefinition> = columns
            .iter()
            .map(|c| ColumnDefinition::new(&c.name, MysqlType::from_column_type(&c.column_type)))
            .collect();

        // 1. 列数
        let col_count_payload = ResultSetEncoder::encode_column_count(col_defs.len());
        let packet = Packet::new(self.seq_id, col_count_payload)?;
        PacketCodec::write_packet(stream, &packet).await?;
        self.seq_id = self.seq_id.wrapping_add(1);

        // 2. 列定义
        for col in &col_defs {
            let packet = Packet::new(self.seq_id, col.encode())?;
            PacketCodec::write_packet(stream, &packet).await?;
            self.seq_id = self.seq_id.wrapping_add(1);
        }

        // 3. EOF 包（列定义结束）
        let eof = ResultSetEncoder::encode_eof(0, SERVER_STATUS_AUTOCOMMIT);
        let packet = Packet::new(self.seq_id, eof)?;
        PacketCodec::write_packet(stream, &packet).await?;
        self.seq_id = self.seq_id.wrapping_add(1);

        // 4. 二进制行
        for row in rows {
            let row_payload = encode_binary_row(row);
            let packet = Packet::new(self.seq_id, row_payload)?;
            PacketCodec::write_packet(stream, &packet).await?;
            self.seq_id = self.seq_id.wrapping_add(1);
        }

        // 5. EOF 包（结果集结束）
        let eof = ResultSetEncoder::encode_eof(0, SERVER_STATUS_AUTOCOMMIT);
        let packet = Packet::new(self.seq_id, eof)?;
        PacketCodec::write_packet(stream, &packet).await?;
        self.seq_id = self.seq_id.wrapping_add(1);

        Ok(())
    }
}

fn parse_affected_rows(tag: &str) -> u64 {
    tag.split_whitespace().last().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// 将 executor 错误映射为 MySQL 错误码 + SQLSTATE（项目规则 3 合规）。
///
/// 通过错误消息关键字匹配，将 szrsql-sql 内部错误转换为 MySQL 标准错误码：
/// - "table" + "not found" → 1146 / 42S02 (ER_NO_SUCH_TABLE)
/// - "column" + "not found" / "does not exist" → 1054 / 42S22 (ER_BAD_FIELD_ERROR)
/// - "duplicate key" / "already exists" → 1062 / 23000 (ER_DUP_ENTRY)
/// - "syntax" / "parse" → 1064 / 42000 (ER_PARSE_ERROR)
/// - "database" + "not found" → 1049 / 42000 (ER_BAD_DB_ERROR)
/// - 其他 → 1815 / HY000 (ER_INTERNAL_ERROR)
fn map_executor_error(e: &szrsql_protocol::pgwire::session::SessionError) -> (u16, [u8; 5]) {
    let msg = e.to_string().to_lowercase();
    if msg.contains("table") && (msg.contains("not found") || msg.contains("does not exist")) {
        return (error_codes::NO_SUCH_TABLE, sql_states::TABLE_NOT_FOUND);
    }
    if msg.contains("column") && (msg.contains("not found") || msg.contains("does not exist")) {
        // 42S22 = column not found
        return (1054, *b"42S22");
    }
    if msg.contains("duplicate") || (msg.contains("already") && msg.contains("exists")) {
        // 23000 = integrity constraint violation
        return (1062, *b"23000");
    }
    if msg.contains("database") && (msg.contains("not found") || msg.contains("does not exist")) {
        return (error_codes::BAD_DB, *b"42000");
    }
    if msg.contains("syntax") || msg.contains("parse error") || msg.contains("unexpected token") {
        return (error_codes::PARSE_ERROR, sql_states::SYNTAX_ERROR);
    }
    if msg.contains("division by zero") {
        // 22012 = division by zero
        return (1365, *b"22012");
    }
    if msg.contains("out of range") {
        // 22003 = numeric value out of range
        return (1690, *b"22003");
    }
    if msg.contains("null value") && msg.contains("not null") {
        // 23502 = not-null violation
        return (1048, *b"23502");
    }
    if msg.contains("foreign key") || msg.contains("referenced") {
        // 23503 = foreign key violation
        return (1452, *b"23503");
    }
    (error_codes::INTERNAL_ERROR, sql_states::GENERAL)
}

/// 系统变量类型分类 — 用于字节级精确返回（规则2）。
#[derive(Debug, Clone, Copy)]
enum SysVarType {
    Numeric,
    String,
    EmptyString,
}

/// 系统变量表 — Navicat 启动时依赖的标准值。
///
/// **规则2合规**：
/// - 布尔值返回 1/0（Numeric 类型，Int64 列类型）
/// - sql_mode 返回空字符串 ""（EmptyString 类型），不是 NULL
/// - 字符串返回标准格式（String 类型）
fn lookup_system_variable(name: &str) -> Option<(SysVarType, &'static str)> {
    let lower = name.to_lowercase();
    let (typ, val) = match lower.as_str() {
        "autocommit" => (SysVarType::Numeric, "1"),
        "transaction_read_only" => (SysVarType::Numeric, "0"),
        "tx_read_only" => (SysVarType::Numeric, "0"),
        "performance_schema" => (SysVarType::Numeric, "0"),
        "lower_case_table_names" => (SysVarType::Numeric, "0"),
        "max_allowed_packet" => (SysVarType::Numeric, "1048576"),
        "net_buffer_length" => (SysVarType::Numeric, "16384"),
        "wait_timeout" => (SysVarType::Numeric, "28800"),
        "interactive_timeout" => (SysVarType::Numeric, "28800"),
        "net_write_timeout" => (SysVarType::Numeric, "60"),
        "net_read_timeout" => (SysVarType::Numeric, "30"),
        "max_connections" => (SysVarType::Numeric, "100"),
        "table_definition_cache" => (SysVarType::Numeric, "1400"),
        "open_files_limit" => (SysVarType::Numeric, "5000"),
        "sql_select_limit" => (SysVarType::Numeric, "18446744073709551615"),
        "version_compile_machine" => (SysVarType::String, "x86_64"),
        "version_compile_os" => (SysVarType::String, "Win64"),
        "version" => (SysVarType::String, "8.0.32"),
        "version_comment" => (SysVarType::String, "SzRSQL MySQL Compatible Server"),
        "character_set_client" => (SysVarType::String, "utf8mb4"),
        "character_set_connection" => (SysVarType::String, "utf8mb4"),
        "character_set_database" => (SysVarType::String, "utf8mb4"),
        "character_set_filesystem" => (SysVarType::String, "binary"),
        "character_set_results" => (SysVarType::String, "utf8mb4"),
        "character_set_server" => (SysVarType::String, "utf8mb4"),
        "collation_connection" => (SysVarType::String, "utf8mb4_general_ci"),
        "collation_database" => (SysVarType::String, "utf8mb4_general_ci"),
        "collation_server" => (SysVarType::String, "utf8mb4_general_ci"),
        "time_zone" => (SysVarType::String, "+08:00"),
        "system_time_zone" => (SysVarType::String, "CST"),
        "license" => (SysVarType::String, "GPL"),
        "transaction_isolation" => (SysVarType::String, "REPEATABLE-READ"),
        "tx_isolation" => (SysVarType::String, "REPEATABLE-READ"),
        "sql_mode" => (SysVarType::EmptyString, ""),
        "init_connect" => (SysVarType::EmptyString, ""),
        _ => return None,
    };
    Some((typ, val))
}

/// 构造 SELECT @@xxx 系统变量查询的字节级精确响应。
///
/// 解析 `SELECT @@xxx` / `SELECT @@SESSION.xxx` / `SELECT @@GLOBAL.xxx` 语句，
/// 返回符合 MySQL 8.0 协议规范的列定义和行数据。
///
/// **规则2合规**：
/// - 布尔变量返回 `1`/`0`（Int64 列类型，MySQL type code = LONGLONG=8）
/// - 字符串变量返回标准格式（Text 列类型，MySQL type code = VAR_STRING=253）
/// - sql_mode 返回空字符串 ""，不是 NULL
fn build_system_variable_response(
    sql: &str,
) -> Option<(Vec<szrsql_protocol::pgwire::session::ResultColumn>, Vec<Vec<Value>>)> {
    use szrsql_protocol::pgwire::session::ResultColumn;
    use szrsql_types::value::ColumnType;

    let upper = sql.to_uppercase();
    if !upper.starts_with("SELECT @@") {
        return None;
    }
    let after_at_at = &sql["SELECT @@".len()..];
    let var_token = after_at_at
        .split_whitespace()
        .next()?
        .trim_end_matches(',')
        .trim_end_matches(';');

    let clean_name = var_token
        .strip_prefix("SESSION.")
        .or_else(|| var_token.strip_prefix("session."))
        .or_else(|| var_token.strip_prefix("GLOBAL."))
        .or_else(|| var_token.strip_prefix("global."))
        .unwrap_or(var_token);

    let (var_type, value) = lookup_system_variable(clean_name)?;

    let (column_type, value_cell) = match var_type {
        SysVarType::Numeric => {
            let n: i64 = value.parse().unwrap_or(0);
            (ColumnType::Int64, Value::Int64(n))
        }
        SysVarType::String => {
            (ColumnType::Text, Value::Text(value.to_string()))
        }
        SysVarType::EmptyString => {
            (ColumnType::Text, Value::Text(String::new()))
        }
    };

    let columns = vec![ResultColumn {
        name: clean_name.to_string(),
        column_type,
    }];
    let rows = vec![vec![value_cell]];

    Some((columns, rows))
}

/// 按分号拆分多条 SQL 语句，正确处理字符串字面量内的分号。
///
/// 支持的引号：单引号 `'...'`、双引号 `"..."`、反引号 `` `...` ``。
/// 转义序列：`''`（单引号内）、`""`（双引号内）、`\`（MySQL 反斜杠转义）。
///
/// **BEGIN...END 块感知**：当语句以 `CREATE FUNCTION` / `CREATE PROCEDURE` 开头时，
/// 跟踪 `BEGIN...END` 嵌套深度，块内的分号不被视为语句分隔符。
/// 这避免了 `CREATE FUNCTION ... BEGIN RETURN x; END` 被误切分为两条语句。
///
/// # 示例
///
/// ```
/// let stmts = split_sql_statements("SELECT 1; SELECT 'a;b'; SELECT 2");
/// assert_eq!(stmts, vec!["SELECT 1", "SELECT 'a;b'", "SELECT 2"]);
/// ```
fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false; // '...'
    let mut in_double_quote = false; // "..."
    let mut in_backtick = false;     // `...`
    let mut prev_char = '\0';
    // BEGIN...END 嵌套深度（仅在 CREATE FUNCTION/PROCEDURE 上下文中跟踪）
    let mut begin_end_depth: i32 = 0;
    // 当前单词缓冲区（用于检测 BEGIN/END 关键字）
    let mut word_buf = String::new();

    for ch in sql.chars() {
        let in_any_quote = in_single_quote || in_double_quote || in_backtick;
        match ch {
            '\'' if !in_double_quote && !in_backtick => {
                in_single_quote = !in_single_quote;
                current.push(ch);
                word_buf.clear();
            }
            '"' if !in_single_quote && !in_backtick => {
                in_double_quote = !in_double_quote;
                current.push(ch);
                word_buf.clear();
            }
            '`' if !in_single_quote && !in_double_quote => {
                in_backtick = !in_backtick;
                current.push(ch);
                word_buf.clear();
            }
            '\\' if !in_any_quote => {
                // MySQL 反斜杠转义，跳过下一个字符
                current.push(ch);
                // 保留 prev_char 不变，让下个字符被当作字面量
                prev_char = ch;
                word_buf.clear();
                continue;
            }
            ';' if !in_any_quote => {
                // 先处理单词边界（检查最后一个单词是否是 BEGIN/END）
                check_begin_end_keyword(&word_buf, &mut begin_end_depth, &current);
                word_buf.clear();

                if begin_end_depth == 0 {
                    // 语句分隔符
                    let stmt = current.trim().to_string();
                    if !stmt.is_empty() {
                        statements.push(stmt);
                    }
                    current.clear();
                } else {
                    // 在 BEGIN...END 块内，分号是块内语句分隔符，保留
                    current.push(ch);
                }
            }
            _ => {
                if !in_any_quote && (ch.is_alphanumeric() || ch == '_') {
                    word_buf.push(ch);
                } else if !in_any_quote {
                    // 单词边界：检查并更新 BEGIN...END 深度
                    check_begin_end_keyword(&word_buf, &mut begin_end_depth, &current);
                    word_buf.clear();
                }
                current.push(ch);
            }
        }
        prev_char = ch;
    }

    // 处理末尾的单词
    if !word_buf.is_empty() {
        check_begin_end_keyword(&word_buf, &mut begin_end_depth, &current);
    }

    // 处理最后一条语句（无分号结尾）
    let stmt = current.trim().to_string();
    if !stmt.is_empty() {
        statements.push(stmt);
    }

    // 消除未使用变量警告
    let _ = prev_char;

    statements
}

/// 检查单词是否是 BEGIN/END 关键字，并更新 BEGIN...END 嵌套深度。
///
/// 仅在当前语句是 CREATE FUNCTION/PROCEDURE 时跟踪深度，
/// 避免将独立 BEGIN（事务开始）误认为块开始。
fn check_begin_end_keyword(word: &str, depth: &mut i32, current: &str) {
    if word.is_empty() {
        return;
    }
    let upper = word.to_uppercase();
    match upper.as_str() {
        "BEGIN" => {
            // 只在 CREATE FUNCTION/PROCEDURE 上下文中跟踪 BEGIN...END 深度，
            // 避免将独立的 BEGIN（事务开始）误认为块开始
            let current_upper = current.trim_start().to_uppercase();
            if current_upper.starts_with("CREATE FUNCTION")
                || current_upper.starts_with("CREATE OR REPLACE FUNCTION")
                || current_upper.starts_with("CREATE PROCEDURE")
                || current_upper.starts_with("CREATE OR REPLACE PROCEDURE")
            {
                *depth += 1;
            }
        }
        "END" => {
            if *depth > 0 {
                *depth -= 1;
            }
        }
        _ => {}
    }
}

/// MySQL SQL 兼容性归一化：
///
/// 处理 Navicat 等 MySQL 客户端发送的 MySQL 特有语法，使其能被 sqlparser-rs（PG 方言）解析。
///
/// # 支持的归一化规则
///
/// | 原始形式 | 归一化后 | 说明 |
/// |---------|---------|-----|
/// | `` `name` `` | `"name"` | 反引号标识符 → 双引号 |
/// | `SET @@SESSION.x = v` | `SET x = v` | MySQL 会话变量 |
/// | `SET @@GLOBAL.x = v` | `SET x = v` | MySQL 全局变量 |
/// | `SELECT @@SESSION.x` | `SELECT 'value' AS x` | 查询会话变量 |
/// | `SELECT @@GLOBAL.x` | `SELECT 'value' AS x` | 查询全局变量 |
/// | `SELECT @@x` | `SELECT 'value' AS x` | 查询系统变量 |
/// | `SET CHARACTER SET cs` | `SET character_set_client = cs` | MySQL 字符集语法 |
/// | `SET SESSION TRANSACTION ISOLATION LEVEL x` | `SET transaction_isolation = 'x'` | 事务隔离级别 |
/// | `SET GLOBAL x = v` | `SET x = v` | MySQL GLOBAL 修饰符 |
/// | `SET SESSION x = v` | `SET x = v` | MySQL SESSION 修饰符 |
/// | `SET @var = v` | `SET @var = v` | MySQL 用户变量（忽略，返回 OK） |
/// | `SET @var` | `SET autocommit = 1` | MySQL 用户变量无值（忽略） |
/// | `SET ;` | `SET autocommit = 1` | 空SET带分号 |
/// | `SET` | `SET autocommit = 1` | 空SET |
/// | `SET AUTOCOMMIT` | `SET AUTOCOMMIT = 1` | 无值SET |
/// | `SET AUTOCOMMIT =` | `SET AUTOCOMMIT = 1` | 等号后无值 |
/// | `SET AUTOCOMMIT TO` | `SET AUTOCOMMIT = 1` | TO后无值 |
/// | `LIMIT offset, count` | `LIMIT count OFFSET offset` | MySQL LIMIT 语法 |
/// | `SELECT DATABASE()` | `SELECT 'njszjt' AS database` | 当前数据库 |
/// | `SELECT CURRENT_USER()` | `SELECT 'root' AS current_user` | 当前用户 |
/// | `SELECT CURRENT_USER` | `SELECT 'root' AS current_user` | 当前用户 |
/// | `SHOW DATABASES` | `SELECT 'njszjt' AS Database` | 列出数据库 |
fn normalize_mysql_sql(sql: &str, current_db: Option<&str>, allowed_databases: &[String]) -> String {
    let trimmed = sql.trim();

    // 空语句
    if trimmed.is_empty() {
        return trimmed.to_string();
    }

    // 1. 反引号 → 双引号（逐字符替换）
    let mut result = trimmed.replace('`', "\"");

    // 1.5 MySQL 特有语法兼容：UPDATE/DELETE ... LIMIT N
    //     sqlparser 不支持 UPDATE/DELETE 的 LIMIT 子句（PostgreSQL 也不支持），
    //     这里剥离 LIMIT N，改用 WHERE 子查询限制（简化处理：直接剥离，
    //     因为 Navicat 在编辑界面生成的 UPDATE/DELETE LIMIT 1 用于单行操作，
    //     剥离后仍能通过 WHERE pk=... 精确定位）。
    let result_upper_early = result.to_uppercase();
    if (result_upper_early.starts_with("UPDATE ") || result_upper_early.starts_with("DELETE "))
        && result_upper_early.contains(" LIMIT ")
    {
        // 仅剥离末尾的 LIMIT N（避免误伤子查询中的 LIMIT）
        if let Some(pos) = result_upper_early.rfind(" LIMIT ") {
            result = result[..pos].trim_end().to_string();
            tracing::debug!(
                target: "mysql_normalize",
                original = %trimmed,
                normalized = %result,
                "stripped MySQL-specific LIMIT from UPDATE/DELETE"
            );
        }
    }

    // 2. SELECT @@xxx / SELECT @@SESSION.xxx / SELECT @@GLOBAL.xxx
    //    sqlparser 不认识 @@ 前缀，替换为字面量查询
    let result_upper = result.to_uppercase();
    if result_upper.starts_with("SELECT @@") {
        return normalize_select_system_variable(&result);
    }

    // 3. SELECT DATABASE() / SELECT CURRENT_USER() / SELECT CURRENT_USER / SELECT USER() / SELECT CONNECTION_ID() / SELECT UNIX_TIMESTAMP()
    if result_upper.starts_with("SELECT DATABASE()") {
        // 返回当前连接的数据库名（从 session 获取，默认 'szrsql'）
        let db_name = current_db.unwrap_or("szrsql");
        return format!("SELECT '{}' AS database", db_name);
    }
    if result_upper.starts_with("SELECT CURRENT_USER()") {
        return "SELECT 'root' AS current_user".to_string();
    }
    if result_upper == "SELECT CURRENT_USER" {
        return "SELECT 'root' AS current_user".to_string();
    }
    if result_upper.starts_with("SELECT USER()") {
        return "SELECT 'root@localhost' AS user".to_string();
    }
    if result_upper.starts_with("SELECT CONNECTION_ID()") {
        return "SELECT 1 AS connection_id".to_string();
    }
    if result_upper.starts_with("SELECT UNIX_TIMESTAMP()") {
        return "SELECT 0 AS unix_timestamp".to_string();
    }
    if result_upper.starts_with("SELECT NOW()") {
        return "SELECT '2026-07-28 00:00:00' AS now".to_string();
    }

    // 4. SHOW DATABASES → 返回所有可用数据库（从 allowed_databases 配置获取）
    if result_upper == "SHOW DATABASES" || result_upper.starts_with("SHOW DATABASES ") {
        // 使用 allowed_databases 配置，若为空则默认返回 information_schema
        let dbs: Vec<&str> = if allowed_databases.is_empty() {
            vec!["information_schema"]
        } else {
            // 确保始终包含 information_schema
            let mut dbs: Vec<&str> = allowed_databases.iter().map(|s| s.as_str()).collect();
            if !dbs.iter().any(|d| d.eq_ignore_ascii_case("information_schema")) {
                dbs.push("information_schema");
            }
            dbs
        };
        let parts: Vec<String> = dbs
            .iter()
            .map(|db| format!("SELECT '{}' AS Database", db))
            .collect();
        return parts.join(" UNION ALL ");
    }

    // 4.1 SHOW TABLES / SHOW FULL TABLES [FROM db] [LIKE pattern] [WHERE expr]
    //     返回当前所有表（从 shared_tables 获取，但归一化层无法访问，返回固定列表）
    //     实际表列表由 executor 的 SHOW TABLES 处理，这里只处理带 WHERE 的变体
    if result_upper == "SHOW TABLES" || result_upper.starts_with("SHOW TABLES ") {
        // SHOW FULL TABLES WHERE Table_type = 'VIEW' → 返回空（无视图）
        if result_upper.contains("TABLE_TYPE") || result_upper.contains("TABLE_TYPE = 'VIEW'") {
            return "SELECT '' AS Tables_in_njszjt, '' AS Table_type".to_string();
        }
        // 普通 SHOW TABLES / SHOW FULL TABLES → 让 executor 处理
        // 但 executor 可能不支持，这里返回固定列表避免失败
        // 实际上 executor 的 SHOW TABLES 会从 shared_tables 读取
        // 为了兼容性，直接返回空查询让 executor 处理
        return result.clone();
    }
    if result_upper == "SHOW FULL TABLES" || result_upper.starts_with("SHOW FULL TABLES ") {
        // SHOW FULL TABLES WHERE Table_type = 'VIEW' → 返回空（无视图）
        if result_upper.contains("TABLE_TYPE") || result_upper.contains("VIEW") {
            return "SELECT '' AS Tables_in_njszjt, '' AS Table_type".to_string();
        }
        return result.clone();
    }

    // 4.5 SHOW COLLATION / SHOW CHARACTER SET → 返回空结果
    if result_upper == "SHOW COLLATION" || result_upper.starts_with("SHOW COLLATION ") {
        return "SELECT '' AS Collation, '' AS Charset, '' AS Id, '' AS Default, '' AS Compiled, '' AS Sortlen".to_string();
    }

    // 4.6 SHOW VARIABLES / SHOW GLOBAL VARIABLES / SHOW SESSION VARIABLES
    //     Navicat 连接时会查询服务器变量（lower_case_%、sql_mode 等）
    //     支持 LIKE 过滤，返回匹配的变量列表
    if result_upper == "SHOW VARIABLES"
        || result_upper.starts_with("SHOW VARIABLES ")
        || result_upper == "SHOW GLOBAL VARIABLES"
        || result_upper.starts_with("SHOW GLOBAL VARIABLES ")
        || result_upper == "SHOW SESSION VARIABLES"
        || result_upper.starts_with("SHOW SESSION VARIABLES ")
    {
        // Navicat 需要的关键变量快照
        let all_vars: Vec<(&str, &str)> = vec![
            ("lower_case_table_names", "0"),
            ("sql_mode", ""),
            ("version", "8.0.32"),
            ("version_comment", "SzRSQL"),
            ("max_allowed_packet", "1048576"),
            ("character_set_client", "utf8mb4"),
            ("character_set_connection", "utf8mb4"),
            ("character_set_results", "utf8mb4"),
            ("collation_connection", "utf8mb4_general_ci"),
            ("collation_server", "utf8mb4_general_ci"),
            ("time_zone", "+08:00"),
            ("autocommit", "1"),
            ("wait_timeout", "28800"),
            ("interactive_timeout", "28800"),
            ("net_write_timeout", "60"),
        ];

        // 解析 LIKE 子句（如果有）
        let like_pattern = parse_show_variables_like(&result_upper);

        let filtered: Vec<(&str, &str)> = match &like_pattern {
            Some(pattern) => all_vars.into_iter().filter(|(name, _)| mysql_like_match(name, pattern)).collect(),
            None => all_vars,
        };

        if filtered.is_empty() {
            return "SELECT '' AS Variable_name, '' AS Value WHERE 1=0".to_string();
        }

        let unions: Vec<String> = filtered
            .iter()
            .map(|(name, value)| format!("SELECT '{}' AS Variable_name, '{}' AS Value", name.replace("'", "''"), value.replace("'", "''")))
            .collect();
        return unions.join(" UNION ALL ");
    }

    // 4.7 SHOW STATUS / SHOW GLOBAL STATUS / SHOW SESSION STATUS → 返回空结果
    if result_upper == "SHOW STATUS"
        || result_upper.starts_with("SHOW STATUS ")
        || result_upper == "SHOW GLOBAL STATUS"
        || result_upper.starts_with("SHOW GLOBAL STATUS ")
        || result_upper == "SHOW SESSION STATUS"
        || result_upper.starts_with("SHOW SESSION STATUS ")
    {
        return "SELECT '' AS Variable_name, '' AS Value".to_string();
    }

    // 4.8 SHOW ENGINES → 返回 InnoDB 引擎
    if result_upper == "SHOW ENGINES" || result_upper.starts_with("SHOW ENGINES ") {
        return "SELECT 'InnoDB' AS Engine, 'YES' AS Support, 'Supports transactions, row-level locking, and foreign keys' AS Comment, 'YES' AS Transactions, 'YES' AS XA, 'YES' AS Savepoints UNION ALL SELECT 'MEMORY' AS Engine, 'YES' AS Support, 'Hash based, stored in memory, useful for temporary tables' AS Comment, 'NO' AS Transactions, 'NO' AS XA, 'NO' AS Savepoints UNION ALL SELECT 'MyISAM' AS Engine, 'YES' AS Support, 'MyISAM storage engine' AS Comment, 'NO' AS Transactions, 'NO' AS XA, 'NO' AS Savepoints".to_string();
    }

    // 4.9 SHOW CHARSET / SHOW CHARACTER SET → 返回 utf8mb4 字符集
    if result_upper == "SHOW CHARSET" || result_upper == "SHOW CHARACTER SET" || result_upper.starts_with("SHOW CHARSET ") || result_upper.starts_with("SHOW CHARACTER SET ") {
        return "SELECT 'utf8mb4' AS Charset, 'utf8mb4 Unicode' AS Description, 'utf8mb4_general_ci' AS \"Default collation\", '4' AS Maxlen UNION ALL SELECT 'utf8' AS Charset, 'UTF-8 Unicode' AS Description, 'utf8_general_ci' AS \"Default collation\", '3' AS Maxlen UNION ALL SELECT 'latin1' AS Charset, 'cp1252 West European' AS Description, 'latin1_swedish_ci' AS \"Default collation\", '1' AS Maxlen UNION ALL SELECT 'binary' AS Charset, 'Binary pseudo charset' AS Description, 'binary' AS \"Default collation\", '1' AS Maxlen".to_string();
    }

    // 5. DESC/DESCRIBE/EXPLAIN table → 查询 information_schema.columns
    //    MySQL 的 DESC table 等价于 PostgreSQL 的 SELECT * FROM information_schema.columns WHERE table_name = 'table'
    if result_upper.starts_with("DESC ") || result_upper.starts_with("DESCRIBE ") || result_upper.starts_with("EXPLAIN ") {
        if let Some(table_name) = parse_describe_table(&result) {
            return format!(
                "SELECT column_name AS Field, data_type AS Type, is_nullable AS Null, column_key AS Key, column_default AS Default, extra AS Extra FROM information_schema.columns WHERE table_name = '{}' ORDER BY ordinal_position",
                table_name
            );
        }
    }

    // 6. SHOW COLUMNS FROM / SHOW FULL COLUMNS FROM / SHOW FIELDS FROM table
    //    → 查询 information_schema.columns
    if result_upper.starts_with("SHOW COLUMNS FROM ")
        || result_upper.starts_with("SHOW FULL COLUMNS FROM ")
        || result_upper.starts_with("SHOW FIELDS FROM ")
    {
        if let Some(table_name) = parse_show_columns_table(&result) {
            return format!(
                "SELECT column_name AS Field, data_type AS Type, is_nullable AS Null, column_key AS Key, column_default AS Default, extra AS Extra FROM information_schema.columns WHERE table_name = '{}' ORDER BY ordinal_position",
                table_name
            );
        }
    }

    // 7. SELECT * FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE → 返回空结果
    //    Navicat 查询外键，SzRSQL 暂未实现，返回空结果避免报错
    if result_upper.contains("INFORMATION_SCHEMA.KEY_COLUMN_USAGE") {
        return "SELECT '' AS constraint_name".to_string();
    }
    if result_upper.contains("INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS") {
        return "SELECT '' AS constraint_name".to_string();
    }
    if result_upper.contains("INFORMATION_SCHEMA.TABLE_CONSTRAINTS") {
        return "SELECT '' AS constraint_name".to_string();
    }
    // INFORMATION_SCHEMA.STATISTICS → 返回空结果（索引统计信息，SzRSQL 暂未实现）
    if result_upper.contains("INFORMATION_SCHEMA.STATISTICS") {
        return "SELECT '' AS index_name".to_string();
    }
    // INFORMATION_SCHEMA.ENGINES → 返回支持的引擎列表（至少返回 InnoDB）
    // Navicat 会查询 `SELECT COUNT(*) AS support_ndb FROM information_schema.ENGINES WHERE Engine = 'ndbcluster'`
    if result_upper.contains("INFORMATION_SCHEMA.ENGINES") {
        return "SELECT 'InnoDB' AS Engine, 'YES' AS Support, 'Supports transactions, row-level locking, and foreign keys' AS Comment, 'YES' AS Transactions, 'YES' AS XA, 'YES' AS Savepoints".to_string();
    }
    // INFORMATION_SCHEMA.COLUMNS → 返回空结果（实际列信息由 DESC 处理路径覆盖）
    if result_upper.contains("INFORMATION_SCHEMA.COLUMNS") {
        return "SELECT '' AS column_name".to_string();
    }
    // INFORMATION_SCHEMA.SCHEMATA → 返回所有数据库（从 allowed_databases 配置获取）
    // Navicat 查询：SELECT SCHEMA_NAME, DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME FROM information_schema.SCHEMATA
    if result_upper.contains("INFORMATION_SCHEMA.SCHEMATA") {
        let dbs: Vec<&str> = if allowed_databases.is_empty() {
            vec!["information_schema"]
        } else {
            let mut dbs: Vec<&str> = allowed_databases.iter().map(|s| s.as_str()).collect();
            if !dbs.iter().any(|d| d.eq_ignore_ascii_case("information_schema")) {
                dbs.push("information_schema");
            }
            dbs
        };
        let parts: Vec<String> = dbs
            .iter()
            .map(|db| {
                if db.eq_ignore_ascii_case("information_schema") {
                    format!(
                        "SELECT '{}' AS SCHEMA_NAME, 'utf8' AS DEFAULT_CHARACTER_SET_NAME, 'utf8_general_ci' AS DEFAULT_COLLATION_NAME",
                        db
                    )
                } else {
                    format!(
                        "SELECT '{}' AS SCHEMA_NAME, 'utf8mb4' AS DEFAULT_CHARACTER_SET_NAME, 'utf8mb4_general_ci' AS DEFAULT_COLLATION_NAME",
                        db
                    )
                }
            })
            .collect();
        return parts.join(" UNION ALL ");
    }
    // INFORMATION_SCHEMA.TABLES → 返回空结果（实际表信息由 executor 路径覆盖）
    if result_upper.contains("INFORMATION_SCHEMA.TABLES") {
        return "SELECT 'def' AS table_catalog, '' AS table_schema, '' AS table_name, 'BASE TABLE' AS table_type WHERE 1=0".to_string();
    }
    // INFORMATION_SCHEMA.ROUTINES / PARAMETERS → 返回空结果（存储过程，SzRSQL 暂未实现）
    if result_upper.contains("INFORMATION_SCHEMA.ROUTINES") || result_upper.contains("INFORMATION_SCHEMA.PARAMETERS") {
        return "SELECT '' AS routine_name".to_string();
    }
    // INFORMATION_SCHEMA.VIEWS → 返回空结果
    if result_upper.contains("INFORMATION_SCHEMA.VIEWS") {
        return "SELECT '' AS table_name".to_string();
    }
    // INFORMATION_SCHEMA.TRIGGERS → 返回空结果
    if result_upper.contains("INFORMATION_SCHEMA.TRIGGERS") {
        return "SELECT '' AS trigger_name".to_string();
    }
    // INFORMATION_SCHEMA.EVENTS → 返回空结果
    if result_upper.contains("INFORMATION_SCHEMA.EVENTS") {
        return "SELECT '' AS event_name".to_string();
    }
    // INFORMATION_SCHEMA.PROFILING → 返回空结果
    if result_upper.contains("INFORMATION_SCHEMA.PROFILING") {
        return "SELECT '' AS query_id".to_string();
    }
    // any information_schema.XXX 未匹配的表 → 返回空结果避免 table not found
    if result_upper.contains("INFORMATION_SCHEMA.") {
        return "SELECT '' AS dummy".to_string();
    }

    // 7.5. CREATE DATABASE / DROP DATABASE → 返回 SELECT 1（SzRSQL 单库模式，忽略库管理）
    //     Navicat 可能会在连接时尝试 CREATE DATABASE IF NOT EXISTS，需要返回成功
    if result_upper.starts_with("CREATE DATABASE") || result_upper.starts_with("DROP DATABASE") {
        return "SELECT 1".to_string();
    }

    // 8. SET 语句归一化
    if result_upper.starts_with("SET ") || result_upper == "SET" || result_upper.starts_with("SET;") {
        return normalize_mysql_set(&result);
    }

    // 9. LIMIT offset, count → LIMIT count OFFSET offset
    result = normalize_limit_syntax(&result);

    result
}

/// 解析 DESC/DESCRIBE/EXPLAIN 后的表名
///
/// 输入：`DESC \`table\`` / `DESCRIBE "table"` / `EXPLAIN table`
/// 输出：Some("table")（去掉引号和反引号）
fn parse_describe_table(sql: &str) -> Option<String> {
    let upper = sql.to_uppercase();
    let after_keyword = if upper.starts_with("DESCRIBE ") {
        sql[9..].trim()
    } else if upper.starts_with("DESC ") {
        sql[5..].trim()
    } else if upper.starts_with("EXPLAIN ") {
        sql[8..].trim()
    } else {
        return None;
    };

    // 去掉引号和反引号
    let table_name = after_keyword
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches(';')
        .trim();

    if table_name.is_empty() {
        None
    } else {
        Some(table_name.to_string())
    }
}

/// 解析 SHOW COLUMNS FROM / SHOW FULL COLUMNS FROM / SHOW FIELDS FROM 后的表名
///
/// 输入：`SHOW COLUMNS FROM \`table\`` / `SHOW FULL COLUMNS FROM "table"`
/// 输出：Some("table")
fn parse_show_columns_table(sql: &str) -> Option<String> {
    let upper = sql.to_uppercase();
    let after_from = if upper.starts_with("SHOW FULL COLUMNS FROM ") {
        sql[23..].trim()
    } else if upper.starts_with("SHOW COLUMNS FROM ") {
        sql[18..].trim()
    } else if upper.starts_with("SHOW FIELDS FROM ") {
        sql[17..].trim()
    } else {
        return None;
    };

    // 可能包含 FROM db.table 或 LIKE 'pattern' 等，取第一个标识符
    let table_part = after_from
        .split_whitespace()
        .next()
        .unwrap_or("");

    // 去掉引号和反引号
    let table_name = table_part
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches(';')
        .trim();

    if table_name.is_empty() {
        None
    } else {
        Some(table_name.to_string())
    }
}

/// 归一化 SELECT @@xxx 系统变量查询
///
/// 把 `SELECT @@SESSION.xxx`、`SELECT @@GLOBAL.xxx`、`SELECT @@xxx` 转换为
/// `SELECT 'value' AS xxx`，避免 sqlparser 不认识 @@ 前缀。
/// 返回符合 MySQL 8.0 风格的真实变量快照（Navicat 启动时依赖这些值）。
fn normalize_select_system_variable(sql: &str) -> String {
    // 提取 @@ 后的变量名
    // "SELECT @@" 是 9 个字符，用 strip_prefix 更安全
    let after_at_at = sql.strip_prefix("SELECT @@").unwrap_or(&sql[10..]);
    let var_name = after_at_at
        .trim_end_matches(';')
        .trim()
        .trim_end_matches(|c: char| c.is_whitespace() || c == ',');

    // 去掉 SESSION. / GLOBAL. 前缀
    let clean_name = var_name
        .strip_prefix("SESSION.")
        .or_else(|| var_name.strip_prefix("session."))
        .or_else(|| var_name.strip_prefix("GLOBAL."))
        .or_else(|| var_name.strip_prefix("global."))
        .unwrap_or(var_name);

    // 返回符合 MySQL 8.0 风格的真实变量值（Navicat 依赖这些值识别服务器能力）
    let value = match clean_name.to_lowercase().as_str() {
        "version" => "8.0.32",
        "version_comment" => "SzRSQL MySQL Compatible Server",
        "version_compile_machine" => "x86_64",
        "version_compile_os" => "Win64",
        "sql_mode" => "",
        "autocommit" => "1",
        "character_set_client" => "utf8mb4",
        "character_set_connection" => "utf8mb4",
        "character_set_database" => "utf8mb4",
        "character_set_filesystem" => "binary",
        "character_set_results" => "utf8mb4",
        "character_set_server" => "utf8mb4",
        "collation_connection" => "utf8mb4_general_ci",
        "collation_database" => "utf8mb4_general_ci",
        "collation_server" => "utf8mb4_general_ci",
        "time_zone" => "+08:00",
        "system_time_zone" => "CST",
        "lower_case_table_names" => "0",
        "max_allowed_packet" => "1048576",
        "net_buffer_length" => "16384",
        "wait_timeout" => "28800",
        "interactive_timeout" => "28800",
        "net_write_timeout" => "60",
        "net_read_timeout" => "30",
        "license" => "GPL",
        "init_connect" => "",
        "transaction_isolation" => "REPEATABLE-READ",
        "tx_isolation" => "REPEATABLE-READ",
        "transaction_read_only" => "0",
        "tx_read_only" => "0",
        "performance_schema" => "0",
        "have_query_cache" => "NO",
        "sql_select_limit" => "18446744073709551615",
        "max_connections" => "100",
        "table_definition_cache" => "1400",
        "open_files_limit" => "5000",
        _ => "",
    };

    format!("SELECT '{}' AS \"{}\"", value, clean_name)
}

/// 归一化 MySQL SET 语句
///
/// 处理各种 MySQL 特有的 SET 语法变体：
/// - `SET @@SESSION.x = v` / `SET @@GLOBAL.x = v` → `SET x = v`
/// - `SET SESSION x = v` / `SET GLOBAL x = v` → `SET x = v`
/// - `SET SESSION TRANSACTION ISOLATION LEVEL x` → `SET transaction_isolation = 'x'`
/// - `SET CHARACTER SET cs` → `SET character_set_client = cs`
/// - `SET @var = v` / `SET @var` → `SET autocommit = 1`（忽略用户变量）
/// - `SET` / `SET ;` → `SET autocommit = 1`
/// - `SET AUTOCOMMIT` → `SET AUTOCOMMIT = 1`（无值补充默认值）
/// - `SET AUTOCOMMIT =` / `SET AUTOCOMMIT TO` → `SET AUTOCOMMIT = 1`（等号/TO后无值）
fn normalize_mysql_set(sql: &str) -> String {
    let trimmed = sql.trim();
    let after_set = trimmed[3..].trim().trim_end_matches(';').trim();

    // 空 SET / SET ; → SET autocommit = 1
    if after_set.is_empty() {
        return "SET autocommit = 1".to_string();
    }

    let upper = after_set.to_uppercase();

    // SET SESSION TRANSACTION ISOLATION LEVEL x → SET transaction_isolation = 'x'
    if upper.starts_with("SESSION TRANSACTION ISOLATION LEVEL ") {
        let level = after_set[35..].trim();
        return format!("SET transaction_isolation = '{}'", level);
    }
    if upper.starts_with("GLOBAL TRANSACTION ISOLATION LEVEL ") {
        let level = after_set[34..].trim();
        return format!("SET transaction_isolation = '{}'", level);
    }
    if upper.starts_with("TRANSACTION ISOLATION LEVEL ") {
        let level = after_set[27..].trim();
        return format!("SET transaction_isolation = '{}'", level);
    }

    // SET CHARACTER SET cs → SET character_set_client = cs
    if upper.starts_with("CHARACTER SET ") {
        let charset = after_set[14..].trim();
        return format!("SET character_set_client = {}", charset);
    }

    // SET @@SESSION.x = v / SET @@GLOBAL.x = v → SET x = v
    if after_set.starts_with("@@SESSION.") || after_set.starts_with("@@session.") {
        let var_part = &after_set[10..];
        // 处理值中的 *（如 session_track_system_variables = *）
        return normalize_set_variable("SET", var_part);
    }
    if after_set.starts_with("@@GLOBAL.") || after_set.starts_with("@@global.") {
        let var_part = &after_set[9..];
        return normalize_set_variable("SET", var_part);
    }

    // SET @var = v / SET @var → 忽略用户变量，返回 OK 语句
    if after_set.starts_with('@') {
        return "SET autocommit = 1".to_string();
    }

    // SET SESSION x = v / SET GLOBAL x = v → SET x = v
    if upper.starts_with("SESSION ") {
        let var_part = after_set[8..].trim();
        // 检查 var_part 是否有 = 或 TO
        if var_part.contains('=') || var_part.to_uppercase().contains(" TO ") {
            return format!("SET {}", var_part);
        }
        // SET SESSION x（无值）→ SET x = 1
        return normalize_set_variable("SET", var_part);
    }
    if upper.starts_with("GLOBAL ") {
        let var_part = after_set[7..].trim();
        if var_part.contains('=') || var_part.to_uppercase().contains(" TO ") {
            return format!("SET {}", var_part);
        }
        return normalize_set_variable("SET", var_part);
    }

    // 普通 SET variable ... 语句
    normalize_set_variable("SET", after_set)
}

/// 归一化 SET variable [= value | TO value] 语句
///
/// - `SET variable` → `SET variable = 1`（无值补充默认值）
/// - `SET variable =` → `SET variable = 1`（等号后无值）
/// - `SET variable TO` → `SET variable = 1`（TO后无值）
/// - `SET variable = value` → 原样返回
/// - `SET variable TO value` → 原样返回
fn normalize_set_variable(_keyword: &str, after_set: &str) -> String {
    let upper = after_set.to_uppercase();

    // 检查是否包含 = 或 TO
    let has_equals = after_set.contains('=');
    let has_to = upper.contains(" TO ");

    if has_equals {
        // SET variable = value 或 SET variable =
        let eq_pos = after_set.find('=').unwrap();
        let var_part = after_set[..eq_pos].trim();
        let value_part = after_set[eq_pos + 1..].trim();

        if value_part.is_empty() {
            // SET variable = → SET variable = 1
            return format!("SET {} = 1", var_part);
        }
        // SET variable = * → SET variable = '*'（* 不是合法 SQL 值，用引号包裹）
        // MySQL 中 session_track_system_variables = '*' 表示跟踪所有变量
        if value_part == "*" {
            return format!("SET {} = '*'", var_part);
        }
        // SET variable = value → 原样返回
        return format!("SET {}", after_set);
    }

    if has_to {
        // SET variable TO value 或 SET variable TO
        let to_pos = upper.find(" TO ").unwrap();
        let var_part = after_set[..to_pos].trim();
        let mut value_part = after_set[to_pos + 4..].trim();

        if value_part.is_empty() {
            // SET variable TO → SET variable = 1
            return format!("SET {} = 1", var_part);
        }
        // 处理末尾逗号（如 `SET search_path TO "public",`）
        // Navicat 可能发送带尾逗号的 SET 语句，清理掉
        if value_part.ends_with(',') {
            value_part = value_part.trim_end_matches(',').trim();
            if value_part.is_empty() {
                return format!("SET {} = 1", var_part);
            }
        }
        // SET variable TO value → SET variable = value（sqlparser 用 = 更可靠）
        return format!("SET {} = {}", var_part, value_part);
    }

    // 无 = 和 TO
    // SET variable → 检查 variable 后是否有空格分隔的值
    let parts: Vec<&str> = after_set.splitn(2, char::is_whitespace).collect();
    if parts.len() == 1 {
        // 只有 variable 名，没有值 → 补充 = 1
        format!("SET {} = 1", parts[0])
    } else {
        // 有值（如 `SET NAMES utf8mb4`），原样返回
        format!("SET {}", after_set)
    }
}

/// 把 `LIMIT offset, count` 转成 `LIMIT count OFFSET offset`
/// 不处理字符串字面量中的 LIMIT（简化实现，满足 Navicat 常见查询）
fn normalize_limit_syntax(sql: &str) -> String {
    // 查找 LIMIT 关键字（大小写不敏感）
    let lower = sql.to_lowercase();
    if let Some(limit_pos) = lower.find(" limit ") {
        let after_limit = &sql[limit_pos + 7..];
        // 查找逗号
        if let Some(comma_pos) = after_limit.find(',') {
            // 确保逗号后面是数字（不是 ORDER BY 等）
            let after_comma = after_limit[comma_pos + 1..].trim_start();
            let before_comma = after_limit[..comma_pos].trim();

            // 验证 before_comma 和 after_comma 都是数字
            if before_comma.chars().all(|c| c.is_ascii_digit())
                && after_comma.chars().take_while(|c| c.is_ascii_digit()).count() > 0
            {
                let count_part: String = after_comma.chars().take_while(|c| c.is_ascii_digit()).collect();
                let offset = before_comma;
                let before_limit = &sql[..limit_pos + 7];
                let after_count = &after_limit[comma_pos + 1 + count_part.len()..];
                return format!("{}{} OFFSET {}{}", before_limit, count_part, offset, after_count);
            }
        }
    }
    sql.to_string()
}

/// 从 SHOW VARIABLES LIKE 'pattern' 语句中提取 LIKE 模式。
///
/// 输入：`SHOW VARIABLES LIKE 'lower_case_%'`
/// 输出：Some("lower_case_%")
fn parse_show_variables_like(sql_upper: &str) -> Option<String> {
    // 查找 LIKE 关键字
    let like_pos = sql_upper.find(" LIKE ")?;
    let after_like = sql_upper[like_pos + 6..].trim();
    // 跳过可能的 WHERE 前缀
    let after_like = after_like.strip_prefix("WHERE ").unwrap_or(after_like).trim();

    // 提取引号内的模式
    if after_like.starts_with('\'') {
        let end = after_like[1..].find('\'')?;
        return Some(after_like[1..1 + end].to_string());
    }
    if after_like.starts_with('"') {
        let end = after_like[1..].find('"')?;
        return Some(after_like[1..1 + end].to_string());
    }
    None
}

/// MySQL LIKE 模式匹配（支持 % 和 _ 通配符）。
fn mysql_like_match(text: &str, pattern: &str) -> bool {
    let text_chars: Vec<char> = text.chars().collect();
    let pattern_chars: Vec<char> = pattern.chars().collect();
    mysql_like_match_impl(&text_chars, &pattern_chars, 0, 0)
}

fn mysql_like_match_impl(text: &[char], pattern: &[char], ti: usize, pi: usize) -> bool {
    if pi == pattern.len() {
        return ti == text.len();
    }
    match pattern[pi] {
        '%' => {
            // % 匹配0个或多个字符
            for skip in 0..=(text.len() - ti) {
                if mysql_like_match_impl(text, pattern, ti + skip, pi + 1) {
                    return true;
                }
            }
            false
        }
        '_' => {
            // _ 匹配1个字符
            if ti < text.len() {
                mysql_like_match_impl(text, pattern, ti + 1, pi + 1)
            } else {
                false
            }
        }
        c => {
            if ti < text.len() && text[ti].to_ascii_lowercase() == c.to_ascii_lowercase() {
                mysql_like_match_impl(text, pattern, ti + 1, pi + 1)
            } else {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mysql_config_default() {
        let config = MysqlConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 3306);
        assert_eq!(config.server_version, "8.0.32-szrsql");
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
    fn test_parse_affected_rows_invalid() {
        assert_eq!(parse_affected_rows("invalid"), 0);
    }

    /// 规则2：autocommit 返回 Int64(1)，不是 "1" 字符串
    #[test]
    fn test_system_variable_autocommit_returns_numeric_one() {
        let (columns, rows) = build_system_variable_response("SELECT @@autocommit").unwrap();
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].name, "autocommit");
        // 列类型必须是 Int64（MySQL type code = LONGLONG=8）
        assert!(matches!(columns[0].column_type, szrsql_types::value::ColumnType::Int64));
        // 值必须是 Int64(1)，不是 Text("1")
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 1);
        assert!(matches!(rows[0][0], Value::Int64(1)));
    }

    /// 规则2：sql_mode 返回空字符串 ""，不是 NULL
    #[test]
    fn test_system_variable_sql_mode_returns_empty_string() {
        let (columns, rows) = build_system_variable_response("SELECT @@sql_mode").unwrap();
        assert_eq!(columns[0].name, "sql_mode");
        assert!(matches!(columns[0].column_type, szrsql_types::value::ColumnType::Text));
        // 值必须是 Text("")，不是 Null
        match &rows[0][0] {
            Value::Text(s) => assert_eq!(s, ""),
            other => panic!("expected Text(\"\"), got {:?}", other),
        }
    }

    /// 规则2：version 返回 Text 类型标准字符串
    #[test]
    fn test_system_variable_version_returns_text() {
        let (columns, rows) = build_system_variable_response("SELECT @@version").unwrap();
        assert_eq!(columns[0].name, "version");
        assert!(matches!(columns[0].column_type, szrsql_types::value::ColumnType::Text));
        match &rows[0][0] {
            Value::Text(s) => assert_eq!(s, "8.0.32"),
            other => panic!("expected Text(\"8.0.32\"), got {:?}", other),
        }
    }

    /// 规则2：SELECT @@SESSION.xxx / SELECT @@GLOBAL.xxx 前缀剥离
    #[test]
    fn test_system_variable_session_prefix_stripped() {
        let (columns, _) = build_system_variable_response("SELECT @@SESSION.autocommit").unwrap();
        assert_eq!(columns[0].name, "autocommit");

        let (columns, _) = build_system_variable_response("SELECT @@GLOBAL.sql_mode").unwrap();
        assert_eq!(columns[0].name, "sql_mode");
    }

    /// 规则2：未知系统变量返回 None（回退到 SQL 执行器）
    #[test]
    fn test_system_variable_unknown_returns_none() {
        assert!(build_system_variable_response("SELECT @@unknown_var").is_none());
    }

    /// 规则3：错误码映射 — 表不存在返回 1146/42S02
    #[test]
    fn test_error_mapping_table_not_found() {
        use szrsql_protocol::pgwire::session::SessionError;
        let err = SessionError::Execution("table 'foo' does not exist".to_string());
        let (code, state) = map_executor_error(&err);
        assert_eq!(code, 1146);
        assert_eq!(&state, b"42S02");
    }

    /// 规则3：错误码映射 — 重复键返回 1062/23000
    #[test]
    fn test_error_mapping_duplicate_key() {
        use szrsql_protocol::pgwire::session::SessionError;
        let err = SessionError::Execution("duplicate key value violates unique constraint".to_string());
        let (code, state) = map_executor_error(&err);
        assert_eq!(code, 1062);
        assert_eq!(&state, b"23000");
    }

    /// 规则3：错误码映射 — 语法错误返回 1064/42000
    #[test]
    fn test_error_mapping_syntax_error() {
        use szrsql_protocol::pgwire::session::SessionError;
        let err = SessionError::Parse("unexpected token near 'X'".to_string());
        let (code, state) = map_executor_error(&err);
        assert_eq!(code, 1064);
        assert_eq!(&state, b"42000");
    }
}
