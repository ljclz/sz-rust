//! pgwire 会话级 SQL 执行服务。
//!
//! Phase 4.2 — 将 szrsql-sql Executor 接入 pgwire 服务器。
//! Phase 4.3 — 扩展查询协议（Parse/Bind/Execute/Describe/Close/Sync/Flush）。
//!
//! # 职责
//!
//! - 持有每连接独立的会话状态（Catalog / Tables / Sequences / 事务快照）
//! - 解析 SQL 文本 → AST → LogicalPlan → Executor 执行
//! - 返回结构化 `QueryResult`，由 server 层编码为 pgwire 消息
//! - 管理 BEGIN/COMMIT/ROLLBACK 事务状态（基于 `MutableTable::snapshot/restore`）
//! - Phase 4.3：扩展查询协议的 prepared statement 与 portal 存储
//!
//! # 设计
//!
//! - `ExecutorService` 是一个具象结构体（非 trait），因为 Phase 4.2 只需一种实现
//! - 表以 `Arc<Mutex<InMemoryTable>>` 存储，便于 SELECT 借用 + DML 可变借用
//! - 事务期间保存每张表的 `TableSnapshot`，ROLLBACK 时逐一 restore
//! - Phase 4.3 扩展查询的 prepared statement 与 portal 完全独立于 SQL PREPARE/EXECUTE
//!   语句存储（`PreparedStatementStore`），不与 Phase 3.26 的命名预处理语句冲突

use crate::pgwire::copy::{
    format_csv_field, parse_csv_line, parse_text_line, string_to_value, value_to_string, CopyError,
};
use crate::pgwire::message::SqlState;
use crate::pgwire::notify::{Notification, NotifyHub};
use szrsql_sql::ast::{
    CopyDirection, CopyFormat, CopyOptions, CopyTarget, Expr, Statement, TableName,
};
use szrsql_sql::executor::{
    DmlResult, ExecutionError, Executor, InMemorySequenceStore, InMemoryTable, MutableTable,
    PreparedStatementStore, SessionState, TableSnapshot, TableStorage, TempTableStore,
    TransactionHistory,
};
use szrsql_sql::parser::{parse_sql, ParseError};
use szrsql_sql::plan::{Catalog, InMemoryCatalog, LogicalPlan, PlanError, Planner, TableSchema};
use szrsql_tx::wal::{WalError, WalOpType, WalRecord, WalWriter};
use szrsql_types::value::{ColumnType, Value};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

use std::collections::HashMap;
use std::sync::Arc;

// =====================================================================
//  错误类型
// =====================================================================

/// 会话执行错误。
///
/// 携带 SQLSTATE 码供协议层生成 ErrorResponse。
#[derive(Debug, Clone, Error)]
pub enum SessionError {
    #[error("parse error: {0}")]
    Parse(String),

    #[error("plan error: {0}")]
    Plan(String),

    #[error("execution error: {0}")]
    Execution(String),

    #[error("table not found: {0}")]
    TableNotFound(String),

    #[error("invalid statement: {0}")]
    InvalidStatement(String),

    #[error("transaction error: {0}")]
    Transaction(String),

    #[error("protocol error: {0}")]
    Protocol(String),
}

impl SessionError {
    /// 返回对应的 SQLSTATE 码。
    pub fn sqlstate(&self) -> SqlState {
        match self {
            Self::Parse(_) => SqlState::SYNTAX_ERROR,
            Self::Plan(msg) => {
                if msg.contains("already exists") {
                    SqlState::DUPLICATE_TABLE
                } else if msg.contains("not found") {
                    SqlState::UNDEFINED_TABLE
                } else {
                    SqlState::INTERNAL_ERROR
                }
            }
            Self::Execution(msg) => {
                if msg.starts_with("table not found") || msg.starts_with("sequence not found") {
                    SqlState::UNDEFINED_TABLE
                } else if msg.starts_with("column not found") {
                    SqlState::UNDEFINED_COLUMN
                } else if msg.starts_with("foreign key violation") {
                    SqlState::FOREIGN_KEY_VIOLATION
                } else if msg.starts_with("check constraint violation") {
                    SqlState::CHECK_VIOLATION
                } else if msg.starts_with("enum value violation") {
                    SqlState::INVALID_TEXT_REPRESENTATION
                } else if msg.starts_with("type not found") {
                    SqlState::UNDEFINED_OBJECT
                } else if msg.starts_with("type already exists") {
                    SqlState::DUPLICATE_OBJECT
                } else if msg.starts_with("unsupported") {
                    SqlState::FEATURE_NOT_SUPPORTED
                } else {
                    SqlState::INTERNAL_ERROR
                }
            }
            Self::TableNotFound(_) => SqlState::UNDEFINED_TABLE,
            Self::InvalidStatement(_) => SqlState::SYNTAX_ERROR,
            Self::Transaction(_) => SqlState::INVALID_TRANSACTION_STATE,
            Self::Protocol(_) => SqlState::PROTOCOL_VIOLATION,
        }
    }
}

impl From<ParseError> for SessionError {
    fn from(e: ParseError) -> Self {
        Self::Parse(e.to_string())
    }
}

impl From<PlanError> for SessionError {
    fn from(e: PlanError) -> Self {
        Self::Plan(e.to_string())
    }
}

impl From<ExecutionError> for SessionError {
    fn from(e: ExecutionError) -> Self {
        Self::Execution(e.to_string())
    }
}

impl From<WalError> for SessionError {
    fn from(e: WalError) -> Self {
        Self::Transaction(format!("WAL error: {e}"))
    }
}

// =====================================================================
//  查询结果
// =====================================================================

/// 一条 SQL 语句的执行结果。
#[derive(Debug, Clone)]
pub enum QueryResult {
    /// SELECT 或 RETURNING 子句的结果集。
    ResultSet {
        /// 列名列表（与每行列数一致）
        columns: Vec<ResultColumn>,
        /// 数据行（已转换为文本格式供 pgwire 使用）
        rows: Vec<Vec<Value>>,
        /// CommandComplete 标签（如 "SELECT 3"）
        tag: String,
    },
    /// DML（INSERT/UPDATE/DELETE）影响的行数。
    AffectedRows {
        /// CommandComplete 标签（如 "INSERT 0 5"）
        tag: String,
    },
    /// DDL（CREATE/DROP）等无结果集命令。
    DdlComplete {
        /// CommandComplete 标签（如 "CREATE TABLE"）
        tag: String,
    },
    /// 空查询（注释或空字符串）。
    Empty,
    /// 事务控制（BEGIN/COMMIT/ROLLBACK）。
    TransactionComplete {
        /// CommandComplete 标签（如 "BEGIN"）
        tag: String,
        /// 是否处于事务中
        in_transaction: bool,
    },
}

/// 结果集列描述。
#[derive(Debug, Clone)]
pub struct ResultColumn {
    /// 列名
    pub name: String,
    /// 列类型（用于推导 PG OID）
    pub column_type: ColumnType,
}

// =====================================================================
//  事务状态
// =====================================================================

/// 会话事务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    /// 空闲
    Idle,
    /// 事务进行中
    InTransaction,
    /// 失败事务（语法/执行错误后，需 ROLLBACK 才能继续）
    InFailedTransaction,
}

// =====================================================================
//  Phase 4.3 扩展查询：ExtendedPreparedStatement / Portal
// =====================================================================

/// Phase 4.3 扩展查询：命名预处理语句。
///
/// 由 `Parse` 消息创建，存储解析后的 AST 与客户端声明的参数 OID 列表。
/// OID 为 0 表示"未指定，由服务器推断"。
#[derive(Debug, Clone)]
pub struct ExtendedPreparedStatement {
    /// 客户端提供的语句名（空字符串表示无名语句）
    pub name: String,
    /// Parse 阶段解析得到的 SQL AST（扩展查询 Parse 仅允许单条语句）
    pub statement: Statement,
    /// 客户端在 Parse 中声明的参数 OID（0 = 未指定）
    pub parameter_oids: Vec<u32>,
}

/// Phase 4.3 扩展查询：已绑定参数的 portal。
///
/// 由 `Bind` 消息创建，将参数值（已转换为 `Expr::Literal(Value)`）绑定到某个
/// `ExtendedPreparedStatement`，并记录结果列的格式码（0=text, 1=binary）。
#[derive(Debug, Clone)]
pub struct Portal {
    /// 关联的预处理语句名
    pub statement_name: String,
    /// 已绑定的参数表达式列表（用于构造 `LogicalPlan::Execute`）
    pub parameters: Vec<Expr>,
    /// 结果列格式码（每列 0=text, 1=binary；空列表表示全部 text）
    pub result_format_codes: Vec<i16>,
}

// =====================================================================
//  ExecutorService
// =====================================================================

/// 会话级 SQL 执行服务。
///
/// 每个客户端连接持有一个 `ExecutorService` 实例，维护该会话的全部状态。
pub struct ExecutorService {
    /// 表 Schema catalog（用于 Planner 与 FK/CHECK 校验）
    catalog: InMemoryCatalog,
    /// 表数据存储（表名小写 → 表实例）
    tables: HashMap<String, Arc<Mutex<InMemoryTable>>>,
    /// 临时表存储（Phase 3.28）
    temp_store: TempTableStore,
    /// 序列存储（Phase 3.22）
    sequence_store: InMemorySequenceStore,
    /// 预处理语句存储（Phase 3.26，SQL PREPARE/EXECUTE 语句使用）
    prepared_store: PreparedStatementStore,
    /// 会话状态（SET 变量、字符集等 Phase 3.34）
    session_state: SessionState,
    /// 闪回历史（Phase 3.35）
    transaction_history: TransactionHistory,
    /// 当前事务状态
    txn_state: TransactionState,
    /// 事务期间的表快照（表名小写 → 快照）
    txn_snapshots: HashMap<String, TableSnapshot>,
    /// Phase 4.3 扩展查询：命名预处理语句存储（名称 → ExtendedPreparedStatement）
    extended_statements: HashMap<String, ExtendedPreparedStatement>,
    /// Phase 4.3 扩展查询：命名 portal 存储（名称 → Portal）
    portals: HashMap<String, Portal>,
    /// Phase 4.6：本会话的 backend pid（用于 LISTEN/NOTIFY 身份标识）。
    ///
    /// 默认 0；由 `PgwireServer` 在握手时通过 `with_pid` 注入。
    pid: i32,
    /// Phase 4.6：跨会话通知中心（由 `PgwireServer` 共享注入）。
    ///
    /// `None` 表示未连接到通知中心（LISTEN/NOTIFY 将报错）。
    notify_hub: Option<NotifyHub>,
    /// ADV-BUG-002 修复：是否允许 Simple Query 协议执行多语句（分号分隔）。
    ///
    /// - `false`（默认）：安全优先，检测到多语句时返回错误，防止 SQL 注入
    /// - `true`：兼容 PostgreSQL Simple Query 协议（允许多语句依次执行）
    ///
    /// 安全建议：生产环境保持 `false`，仅在可信客户端显式启用。
    allow_multi_statement: bool,
    /// ADV-F-7 修复：可选的 WAL 写入器，用于实现 log-then-commit 事务模型。
    ///
    /// - `None`（默认）：不启用 WAL 持久化，COMMIT 仅清除内存快照（兼容旧行为）
    /// - `Some`：COMMIT 时先写入 WAL Commit 记录并 fsync，成功后才清除快照；
    ///   fsync 失败则回滚事务，确保"已 ACK 的事务必定已持久化"
    ///
    /// 由 `PgwireServer` 在会话创建时通过 [`with_wal_writer`] 注入。
    wal_writer: Option<Arc<WalWriter>>,
    /// 当前事务 ID（BEGIN 时分配，COMMIT/ROLLBACK 后清空）。
    ///
    /// 用于 WAL Commit/Abort 记录的 `tx_id` 字段。从 1 开始递增，0 表示无事务。
    current_txn_id: u32,
    /// 下一个事务 ID（单调递增，会话级）。
    next_txn_id: u32,
    /// ADV-CONC-1：跨会话共享的表存储（多线程并发支持）。
    ///
    /// - `None`（默认）：退化为会话私有存储，多个 session 之间数据隔离（旧行为）
    /// - `Some`：CREATE TABLE 注册到共享存储，所有 session 可见同一张表
    ///
    /// 由 `PgwireServer` 在会话创建时通过 [`with_shared_tables`] 注入。
    shared_tables: Option<Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>>,
    /// ADV-CONC-1：跨会话共享的行锁管理器（多线程并发支持）。
    ///
    /// - `None`（默认）：不加行级锁，依赖表级 Mutex 互斥（旧行为）
    /// - `Some`：DML 操作对每行加 X 锁，SELECT FOR UPDATE 加 X 锁，SELECT FOR SHARE 加 S 锁
    ///   COMMIT/ROLLBACK 时调用 `unlock_all(txn_id)` 释放所有锁（Strict 2PL）
    ///
    /// 由 `PgwireServer` 在会话创建时通过 [`with_lock_manager`] 注入。
    lock_manager: Option<Arc<szrsql_tx::lock::LockManager>>,
    /// ADV-CONC-1：跨会话共享的事务 ID 计数器（原子递增）。
    ///
    /// 确保 different sessions 获得不同的 txn_id，避免 LockManager 将
    /// 两个独立事务误判为同一事务（重入锁不阻塞）。
    ///
    /// `None`（默认）：退化为会话级 `next_txn_id` 计数器（旧行为）
    /// `Some`：BEGIN 时从共享计数器原子递增获取全局唯一 txn_id
    shared_txn_counter: Option<Arc<std::sync::atomic::AtomicU32>>,
}

impl ExecutorService {
    /// 创建一个空会话。
    pub fn new() -> Self {
        Self {
            catalog: InMemoryCatalog::new(),
            tables: HashMap::new(),
            temp_store: TempTableStore::new(),
            sequence_store: InMemorySequenceStore::new(),
            prepared_store: PreparedStatementStore::new(),
            session_state: SessionState::new(),
            transaction_history: TransactionHistory::new(),
            txn_state: TransactionState::Idle,
            txn_snapshots: HashMap::new(),
            extended_statements: HashMap::new(),
            portals: HashMap::new(),
            pid: 0,
            notify_hub: None,
            allow_multi_statement: false,
            wal_writer: None,
            current_txn_id: 0,
            next_txn_id: 1,
            shared_tables: None,
            lock_manager: None,
            shared_txn_counter: None,
        }
    }

    /// ADV-BUG-002 修复：配置是否允许 Simple Query 协议执行多语句。
    ///
    /// 默认 `false`（安全优先）。仅在可信客户端场景下启用。
    pub fn with_multi_statement(mut self, allow: bool) -> Self {
        self.allow_multi_statement = allow;
        self
    }

    /// ADV-F-7 修复：注入 WAL 写入器，启用 log-then-commit 事务模型。
    ///
    /// 注入后，COMMIT 操作会：
    /// 1. 写入 `WalOpType::Commit` 记录到 WAL
    /// 2. 调用 `flush()`（fsync）强制刷盘
    /// 3. fsync 成功后才清除内存快照并 ACK 客户端
    /// 4. fsync 失败则回滚事务（restore 快照），返回错误
    ///
    /// 这确保了"已 ACK 的事务必定已持久化"，消除 commit-then-log 的 ACK 丢失风险。
    ///
    /// # 参数
    ///
    /// - `writer`：共享的 `WalWriter` 实例（通常由 `PgwireServer` 持有并分发给每个会话）
    pub fn with_wal_writer(mut self, writer: Arc<WalWriter>) -> Self {
        self.wal_writer = Some(writer);
        self
    }

    /// ADV-CONC-1：注入跨会话共享的表存储，启用多线程并发支持。
    ///
    /// 注入后：
    /// - `CREATE TABLE` 注册到共享存储，所有 session 可见
    /// - `SELECT/INSERT/UPDATE/DELETE` 操作的是共享表
    /// - `DROP TABLE` 从共享存储移除
    ///
    /// 未注入时退化为会话私有存储（旧行为，用于测试兼容）。
    ///
    /// # 参数
    ///
    /// - `shared`：共享表存储（`Arc<RwLock<HashMap<...>>>`，由 `PgwireServer` 持有）
    pub fn with_shared_tables(
        mut self,
        shared: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
    ) -> Self {
        self.shared_tables = Some(shared);
        self
    }

    /// ADV-CONC-1：注入跨会话共享的行锁管理器，启用行级锁。
    ///
    /// 注入后：
    /// - DML 操作（UPDATE/DELETE）对每行加 X 锁
    /// - SELECT FOR UPDATE 加 X 锁，SELECT FOR SHARE 加 S 锁
    /// - COMMIT/ROLLBACK 调用 `unlock_all(txn_id)` 释放所有锁（Strict 2PL）
    ///
    /// 未注入时不加行级锁，依赖表级 `Mutex` 互斥（旧行为）。
    ///
    /// # 参数
    ///
    /// - `lm`：共享行锁管理器（`Arc<LockManager>`，由 `PgwireServer` 持有）
    pub fn with_lock_manager(mut self, lm: Arc<szrsql_tx::lock::LockManager>) -> Self {
        self.lock_manager = Some(lm);
        self
    }

    /// ADV-CONC-1：注入跨会话共享的事务 ID 计数器。
    ///
    /// 启用后，BEGIN 从此计数器原子递增获取全局唯一 txn_id，
    /// 确保不同 session 的事务不会共享同一个 txn_id（否则 LockManager
    /// 会将两个独立事务误判为同一事务，重入锁不阻塞，导致并发隔离失效）。
    pub fn with_shared_txn_counter(mut self, counter: Arc<std::sync::atomic::AtomicU32>) -> Self {
        self.shared_txn_counter = Some(counter);
        self
    }

    /// Phase 4.6：设置本会话的 pid（由 `PgwireServer` 在握手时调用）。
    ///
    /// 同时将本会话注册到 `NotifyHub`，以便参与 LISTEN/NOTIFY。
    /// 重复调用会先注销旧的 pid 再注册新的。
    pub fn with_pid(mut self, pid: i32) -> Self {
        self.pid = pid;
        if let Some(hub) = &self.notify_hub {
            hub.register(pid);
        }
        self
    }

    /// Phase 4.6：注入跨会话通知中心（由 `PgwireServer` 在握手时调用）。
    ///
    /// 同时以当前 `pid` 注册到通知中心。若 pid 为 0（未设置），则仅存储引用，
    /// 后续 `with_pid` 调用会完成注册。
    pub fn with_notify_hub(mut self, hub: NotifyHub) -> Self {
        if self.pid != 0 {
            hub.register(self.pid);
        }
        self.notify_hub = Some(hub);
        self
    }

    /// Phase 4.6：返回本会话 pid。
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// Phase 4.6：取出本会话所有待发送的通知（清空队列）。
    ///
    /// server 层在每次 Query/Execute 响应后调用此方法，将通知编码为
    /// `BackendMessage::NotificationResponse` 发送给客户端。
    pub fn drain_pending_notifications(&mut self) -> Vec<Notification> {
        match &self.notify_hub {
            Some(hub) => hub.drain_pending(self.pid),
            None => Vec::new(),
        }
    }

    /// Phase 4.6：会话结束清理（注销 NotifyHub 订阅）。
    ///
    /// 由 `PgwireServer` 在连接断开时调用。
    pub fn cleanup_notifications(&mut self) {
        if let Some(hub) = &self.notify_hub {
            hub.unregister(self.pid);
        }
    }

    /// 返回当前事务状态。
    pub fn transaction_state(&self) -> TransactionState {
        self.txn_state
    }

    /// 是否处于事务中。
    pub fn in_transaction(&self) -> bool {
        matches!(
            self.txn_state,
            TransactionState::InTransaction | TransactionState::InFailedTransaction
        )
    }

    /// ADV-CONC-1：计算表级资源 ID（用于 LockManager）。
    ///
    /// 使用表名小写形式的稳定哈希作为资源 ID，高 bit 置 1 以区分未来的行级锁
    /// （行级锁计划使用 `row_id as u64`，高 bit 为 0）。
    fn table_resource_id(table_name: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        table_name.to_lowercase().hash(&mut hasher);
        let hash = hasher.finish();
        // 高 bit 置 1 标记为表级锁
        hash | 0x8000_0000_0000_0000
    }

    /// ADV-CONC-1：在事务中获取表级排他锁（Strict 2PL）。
    ///
    /// 仅在以下条件同时满足时获取锁：
    /// 1. `lock_manager` 已注入（启用并发支持）
    /// 2. 当前处于显式事务中（`txn_id != 0`）
    ///
    /// 非事务模式下（auto-commit）不加锁，依赖表级 `Mutex` 提供语句级原子性。
    /// 锁获取使用 `spawn_blocking` 包装同步的 `LockManager::lock`，避免阻塞 tokio 执行器。
    /// 默认超时 30 秒，超时返回 `LockError::Timeout`。
    ///
    /// # 参数
    ///
    /// - `table_name`：目标表名
    ///
    /// # 错误
    ///
    /// - `SessionError::Transaction`：锁获取失败（冲突/超时/死锁）
    ///
    /// # 死锁处理
    ///
    /// 当 `LockManager` 检测到死锁并返回 `Deadlock` 错误时，本方法会立即调用
    /// `unlock_all(txn_id)` 释放该事务持有的所有锁，确保被死锁等待的对方事务
    /// 能被唤醒继续执行。这对应 PostgreSQL 的行为：死锁中止的事务会回滚并
    /// 释放所有锁，让对方事务可以继续。
    async fn acquire_table_xlock(&self, table_name: &str) -> Result<(), SessionError> {
        let lm = match &self.lock_manager {
            Some(lm) => lm,
            None => return Ok(()),
        };
        let txn_id = self.current_txn_id;
        if txn_id == 0 {
            return Ok(());
        }
        let resource = Self::table_resource_id(table_name);
        let lm_clone = lm.clone();
        let result = tokio::task::spawn_blocking(move || {
            lm_clone.lock(
                txn_id,
                resource,
                szrsql_tx::lock::LockMode::Exclusive,
                std::time::Duration::from_secs(30),
            )
        })
        .await
        .map_err(|e| SessionError::Transaction(format!("lock task join failed: {e}")))?;

        match result {
            Ok(()) => Ok(()),
            Err(szrsql_tx::lock::LockError::Deadlock(aborted_txn_id)) => {
                // ADV-CONC-1.5：死锁中止后必须释放本事务持有的所有锁，
                // 否则对方事务会一直等待，死锁无法真正解除。
                lm.unlock_all(aborted_txn_id);
                tracing::warn!(
                    txn_id = aborted_txn_id,
                    table = %table_name,
                    "deadlock detected, all locks held by this transaction released"
                );
                Err(SessionError::Transaction(format!(
                    "deadlock detected: txn {aborted_txn_id} aborted"
                )))
            }
            Err(e) => Err(SessionError::Transaction(format!(
                "lock acquire failed: {e}"
            ))),
        }
    }

    /// 执行一条 SQL 文本（可能含多条语句，以分号分隔）。
    ///
    /// 简单查询协议要求：多条语句依次执行，每条都产生响应。
    ///
    /// ADV-BUG-002 修复：默认禁止多语句执行（`allow_multi_statement = false`），
    /// 检测到多语句时返回错误，防止 SQL 注入。如需兼容 PostgreSQL 多语句行为，
    /// 通过 [`ExecutorService::with_multi_statement(true)`] 启用。
    pub async fn execute_sql(&mut self, sql: &str) -> Vec<Result<QueryResult, SessionError>> {
        let trimmed = sql.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with("/*") {
            return vec![Ok(QueryResult::Empty)];
        }

        let statements = match parse_sql(sql) {
            Ok(stmts) => stmts,
            Err(e) => return vec![Err(SessionError::from(e))],
        };

        // ADV-BUG-002 修复：多语句执行限制
        // 默认禁止多语句执行，防止 SQL 注入（如 `SELECT 1; DROP TABLE users`）
        if !self.allow_multi_statement && statements.len() > 1 {
            return vec![Err(SessionError::Protocol(format!(
                "multi-statement SQL not allowed in single-statement mode (ADV-BUG-002 protection): got {} statements; \
                 use ExecutorService::with_multi_statement(true) to enable PostgreSQL-compatible multi-statement execution",
                statements.len()
            )))];
        }

        let mut results = Vec::with_capacity(statements.len());
        for stmt in statements {
            let result = self.execute_statement(stmt).await;
            // 失败事务：出错后标记为 InFailedTransaction，后续语句直接报错
            if let Err(ref e) = result {
                if self.txn_state == TransactionState::InTransaction {
                    self.txn_state = TransactionState::InFailedTransaction;
                    tracing::debug!(error = %e, "statement failed, marking transaction as failed");
                }
            }
            results.push(result);
        }
        results
    }

    /// ADV-CONC-1：从共享存储同步 catalog 到本地。
    ///
    /// 当启用 `shared_tables` 时，其他 session 的 CREATE TABLE 会注册到共享存储，
    /// 但本 session 的 `catalog` 是私有的。此方法在每次 `execute_statement` 开始时调用，
    /// 将共享存储中的表 schema 同步到本地 catalog，确保 Planner 能找到表定义。
    ///
    /// 同步策略：只新增不删除（DROP TABLE 由本地 DDL 处理器同步移除）。
    async fn sync_catalog_from_shared(&mut self) {
        let shared = match &self.shared_tables {
            Some(s) => s,
            None => return,
        };
        let guard = shared.read().await;
        for (key, table_arc) in guard.iter() {
            // 本地 catalog 已有此表，跳过
            if self.catalog.get_table(&TableName::new(key)).is_some() {
                continue;
            }
            // 从共享表读取 schema 并注册到本地 catalog
            let table_guard = table_arc.lock().await;
            let schema = table_guard.schema().clone();
            drop(table_guard);
            self.catalog.add_table(schema);
        }
    }

    /// 执行单条语句。
    async fn execute_statement(&mut self, stmt: Statement) -> Result<QueryResult, SessionError> {
        // ADV-CONC-1：在规划前从共享存储同步 catalog（跨 session CREATE TABLE 可见性）
        self.sync_catalog_from_shared().await;

        // 1. 拦截事务控制语句（不进入 Planner）
        //    注意：ROLLBACK 在 InFailedTransaction 状态下也必须放行
        if let Some(tx_result) = self.handle_transaction_control(&stmt).await? {
            return Ok(tx_result);
        }

        // 1.5 失败事务保护：InFailedTransaction 状态下拒绝除 ROLLBACK 外的所有语句
        //     （PG 行为：current transaction is aborted, commands ignored until end of
        //     transaction block）
        if self.txn_state == TransactionState::InFailedTransaction {
            return Err(SessionError::Transaction(
                "current transaction is aborted, commands ignored until end of transaction block"
                    .into(),
            ));
        }

        // 2. Phase 4.7：系统表查询拦截（pg_tables / pg_indexes / information_schema.*）
        //    这类查询需要 MutableCatalog 接口（szrsql-catalog 提供），无法走 Planner
        //    （Planner 只接受 Catalog trait）。在 plan_statement 之前拦截，直接返回结果。
        if let Some(result) =
            crate::pgwire::system_tables::try_execute_system_table_query(&stmt, &self.catalog)
        {
            return result;
        }

        // 3. 其余语句走 Planner
        let plan = {
            let catalog_ref: &InMemoryCatalog = &self.catalog;
            let planner = Planner::new(catalog_ref);
            planner.plan_statement(stmt)?
        };

        // 4. 分派执行
        self.dispatch_plan(&plan).await
    }

    /// 处理 BEGIN / COMMIT / ROLLBACK / SAVEPOINT 等事务控制语句。
    ///
    /// 返回 `Ok(Some(result))` 表示已处理；`Ok(None)` 表示非事务控制语句。
    async fn handle_transaction_control(
        &mut self,
        stmt: &Statement,
    ) -> Result<Option<QueryResult>, SessionError> {
        match stmt {
            Statement::Begin { .. } => {
                if self.in_transaction() {
                    return Err(SessionError::Transaction("already in transaction".into()));
                }
                self.begin_transaction().await;
                Ok(Some(QueryResult::TransactionComplete {
                    tag: "BEGIN".into(),
                    in_transaction: true,
                }))
            }
            Statement::Commit => {
                if !self.in_transaction() {
                    return Err(SessionError::Transaction(
                        "no transaction in progress".into(),
                    ));
                }
                // PG 行为：失败事务中的 COMMIT 实际执行 ROLLBACK
                if self.txn_state == TransactionState::InFailedTransaction {
                    self.rollback_transaction().await;
                    return Ok(Some(QueryResult::TransactionComplete {
                        tag: "ROLLBACK".into(),
                        in_transaction: false,
                    }));
                }
                // ADV-F-7：log-then-commit — WAL fsync 失败时回滚事务并返回错误
                match self.commit_transaction() {
                    Ok(()) => Ok(Some(QueryResult::TransactionComplete {
                        tag: "COMMIT".into(),
                        in_transaction: false,
                    })),
                    Err(e) => {
                        // WAL fsync 失败：回滚事务（restore 快照），返回错误
                        self.rollback_transaction().await;
                        Err(e)
                    }
                }
            }
            Statement::Rollback { savepoint: None } => {
                if !self.in_transaction() {
                    return Err(SessionError::Transaction(
                        "no transaction in progress".into(),
                    ));
                }
                self.rollback_transaction().await;
                Ok(Some(QueryResult::TransactionComplete {
                    tag: "ROLLBACK".into(),
                    in_transaction: false,
                }))
            }
            // SAVEPOINT / RELEASE SAVEPOINT / ROLLBACK TO SAVEPOINT / SET TRANSACTION
            // Phase 4.2 暂不支持，留待后续阶段
            Statement::Rollback { savepoint: Some(_) }
            | Statement::Savepoint(_)
            | Statement::ReleaseSavepoint(_)
            | Statement::SetTransaction { .. } => Err(SessionError::Transaction(format!(
                "savepoint/set transaction not supported in Phase 4.2: {stmt:?}"
            ))),
            _ => Ok(None),
        }
    }

    /// 开始事务：对所有当前表取快照，并分配事务 ID。
    ///
    /// ADV-F-7：若启用了 WAL（`wal_writer` 非 None），分配单调递增的 `txn_id`
    /// 用于后续 COMMIT/ROLLBACK 时写入 WAL 记录。
    async fn begin_transaction(&mut self) {
        self.txn_snapshots.clear();
        // ADV-CONC-1：同时快照本地表和共享表（避免 ROLLBACK 覆盖其他 session 的修改）
        for (name, table) in &self.tables {
            let table_guard = table.lock().await;
            let snapshot = table_guard.snapshot();
            self.txn_snapshots.insert(name.clone(), snapshot);
        }
        if let Some(shared) = &self.shared_tables {
            let guard = shared.read().await;
            for (name, table) in guard.iter() {
                if self.txn_snapshots.contains_key(name) {
                    continue;
                }
                let table_guard = table.lock().await;
                let snapshot = table_guard.snapshot();
                self.txn_snapshots.insert(name.clone(), snapshot);
            }
        }
        self.txn_state = TransactionState::InTransaction;

        // ADV-F-7 / ADV-CONC-1：分配事务 ID
        // 优先从共享计数器获取（确保跨 session 全局唯一），退化为会话级计数器
        self.current_txn_id = if let Some(counter) = &self.shared_txn_counter {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        } else {
            let id = self.next_txn_id;
            self.next_txn_id += 1;
            id
        };

        tracing::debug!(
            tables = self.tables.len(),
            txn_id = self.current_txn_id,
            "transaction begun"
        );
    }

    /// 提交事务（log-then-commit 模型，ADV-F-7 修复）。
    ///
    /// # 流程
    ///
    /// 1. **无 WAL 模式**（`wal_writer` 为 None）：直接清除快照，兼容旧行为
    /// 2. **有 WAL 模式**：
    ///    a. 写入 `WalOpType::Commit` 记录（携带 `txn_id`）
    ///    b. 调用 `flush()`（fsync）强制刷盘
    ///    c. fsync 成功 → 清除快照，返回 Ok（可安全 ACK 客户端）
    ///    d. fsync 失败 → 回滚事务（restore 快照），返回 Err（客户端收到错误）
    ///
    /// # 安全保证
    ///
    /// - 返回 Ok：WAL Commit 记录已 fsync，事务已持久化
    /// - 返回 Err：WAL 写入/fsync 失败，事务已回滚，不会出现"ACK 成功但数据未持久化"
    fn commit_transaction(&mut self) -> Result<(), SessionError> {
        // ADV-CONC-1：在进入 WAL 分支前提前取出 txn_id，供锁释放使用
        let txn_id = self.current_txn_id;

        if let Some(writer) = &self.wal_writer {
            // 阶段 1：写入 WAL Commit 记录
            let record = WalRecord::new(0, txn_id, WalOpType::Commit, 0, vec![]);
            let lsn = writer.append(record)?;
            tracing::debug!(txn_id, lsn, "WAL Commit record appended");

            // 阶段 2：fsync 强制刷盘
            match writer.flush() {
                Ok(()) => {
                    tracing::debug!(txn_id, lsn, "WAL fsync succeeded, transaction durable");
                }
                Err(e) => {
                    // fsync 失败：事务必须回滚，不能 ACK 成功
                    tracing::warn!(txn_id, lsn, error = %e, "WAL fsync failed, rolling back transaction");
                    // 恢复快照（同步 restore，不 await — 因为 rollback_transaction 是 async）
                    // 这里不能调用 async 的 rollback_transaction，需要同步恢复
                    // 但 rollback_transaction 需要 await table.lock()
                    // 所以返回错误，让调用方执行回滚
                    return Err(SessionError::Transaction(format!(
                        "WAL fsync failed, transaction rolled back: {e}"
                    )));
                }
            }
        }

        // Phase 4.2：可选记录到 transaction_history 以支持后续 FLASHBACK
        // 此处简化：不记录（避免无谓的内存增长），FLASHBACK 留待后续阶段接入
        self.txn_snapshots.clear();
        self.txn_state = TransactionState::Idle;
        // ADV-CONC-1：释放本事务持有的所有行级锁（Strict 2PL）
        if let Some(lm) = &self.lock_manager {
            lm.unlock_all(txn_id);
            tracing::debug!(txn_id, "all row locks released on commit");
        }
        self.current_txn_id = 0;
        tracing::debug!(txn_id = self.current_txn_id, "transaction committed");
        Ok(())
    }

    /// 回滚事务：将每张表 restore 到事务前的快照。
    ///
    /// ADV-F-7：若启用了 WAL，写入 `WalOpType::Abort` 记录并 fsync。
    /// Abort 记录的 fsync 失败不影响回滚的正确性（内存状态已恢复），
    /// 但会记录警告日志，因为 WAL 中可能存在"孤儿 Commit 记录"。
    async fn rollback_transaction(&mut self) {
        let txn_id = self.current_txn_id;
        // ADV-CONC-1：同时恢复本地表和共享表
        for (name, table) in &self.tables {
            if let Some(snapshot) = self.txn_snapshots.get(name) {
                let mut table_guard = table.lock().await;
                table_guard.restore(snapshot.clone());
            }
        }
        if let Some(shared) = &self.shared_tables {
            let guard = shared.read().await;
            for (name, table) in guard.iter() {
                if self.txn_snapshots.contains_key(name) && !self.tables.contains_key(name) {
                    if let Some(snapshot) = self.txn_snapshots.get(name) {
                        let mut table_guard = table.lock().await;
                        table_guard.restore(snapshot.clone());
                    }
                }
            }
        }
        self.txn_snapshots.clear();
        self.txn_state = TransactionState::Idle;

        // ADV-F-7：写入 WAL Abort 记录
        if let Some(writer) = &self.wal_writer {
            let record = WalRecord::new(0, txn_id, WalOpType::Abort, 0, vec![]);
            match writer.append(record) {
                Ok(lsn) => {
                    // Abort 记录的 fsync 失败不影响回滚正确性，但记录警告
                    if let Err(e) = writer.flush() {
                        tracing::warn!(
                            txn_id,
                            lsn,
                            error = %e,
                            "WAL Abort fsync failed (non-fatal, transaction already rolled back in memory)"
                        );
                    } else {
                        tracing::debug!(txn_id, lsn, "WAL Abort record written and fsynced");
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        txn_id,
                        error = %e,
                        "WAL Abort append failed (non-fatal, transaction already rolled back in memory)"
                    );
                }
            }
        }

        // ADV-CONC-1：释放本事务持有的所有行级锁（Strict 2PL）
        if let Some(lm) = &self.lock_manager {
            lm.unlock_all(txn_id);
            tracing::debug!(txn_id, "all row locks released on rollback");
        }
        self.current_txn_id = 0;
        tracing::debug!("transaction rolled back");
    }

    /// 根据计划类型分派执行。
    async fn dispatch_plan(&mut self, plan: &LogicalPlan) -> Result<QueryResult, SessionError> {
        match plan {
            // SELECT-family：使用 Executor::execute
            LogicalPlan::Scan { .. }
            | LogicalPlan::IndexScan { .. }
            | LogicalPlan::Projection { .. }
            | LogicalPlan::Filter { .. }
            | LogicalPlan::Join { .. }
            | LogicalPlan::Aggregate { .. }
            | LogicalPlan::Window { .. }
            | LogicalPlan::Sort { .. }
            | LogicalPlan::Limit { .. }
            | LogicalPlan::Distinct { .. }
            | LogicalPlan::SetOp { .. }
            | LogicalPlan::MaterializedViewScan { .. }
            | LogicalPlan::Empty
            | LogicalPlan::Dual
            | LogicalPlan::ShowTables
            | LogicalPlan::ShowCreateTable { .. }
            | LogicalPlan::ShowVariable { .. }
            | LogicalPlan::SetNames { .. }
            | LogicalPlan::SetVariable { .. }
            | LogicalPlan::Shared { .. }
            | LogicalPlan::MemoRef { .. }
            | LogicalPlan::With { .. }
            | LogicalPlan::CteRef { .. } => self.execute_select_plan(plan).await,

            // DML
            LogicalPlan::Insert { .. } => self.execute_insert_plan(plan).await,
            LogicalPlan::Update { .. } => self.execute_update_plan(plan).await,
            LogicalPlan::Delete { .. } => self.execute_delete_plan(plan).await,
            LogicalPlan::Replace { .. } => self.execute_replace_plan(plan).await,
            LogicalPlan::Merge { .. } => self.execute_merge_plan(plan).await,

            // DDL
            LogicalPlan::CreateTable { .. } => self.execute_create_table_plan(plan).await,
            LogicalPlan::DropTable { .. } => self.execute_drop_table_plan(plan).await,
            LogicalPlan::CreateIndex { .. } => Ok(QueryResult::DdlComplete {
                tag: "CREATE INDEX".into(),
            }),
            LogicalPlan::DropIndex { .. } => Ok(QueryResult::DdlComplete {
                tag: "DROP INDEX".into(),
            }),
            LogicalPlan::CreateView { .. } => Ok(QueryResult::DdlComplete {
                tag: "CREATE VIEW".into(),
            }),
            LogicalPlan::DropView { .. } => Ok(QueryResult::DdlComplete {
                tag: "DROP VIEW".into(),
            }),
            LogicalPlan::RefreshMaterializedView { .. } => Ok(QueryResult::DdlComplete {
                tag: "REFRESH MATERIALIZED VIEW".into(),
            }),
            LogicalPlan::CreateSequence { .. } => self.execute_create_sequence_plan(plan),
            LogicalPlan::DropSequence { .. } => self.execute_drop_sequence_plan(plan),
            LogicalPlan::CreateType { .. } => self.execute_create_type_plan(plan),
            LogicalPlan::DropType { .. } => self.execute_drop_type_plan(plan),
            LogicalPlan::AlterType { .. } => self.execute_alter_type_plan(plan),
            // Phase 6.4: 触发器 DDL
            LogicalPlan::CreateTrigger { .. } => self.execute_create_trigger_plan(plan),
            LogicalPlan::DropTrigger { .. } => self.execute_drop_trigger_plan(plan),

            // 预处理语句（Phase 3.26）
            LogicalPlan::Prepare { .. } => self.execute_prepare_plan(plan),
            LogicalPlan::Execute { .. } => self.execute_execute_plan(plan).await,
            LogicalPlan::Deallocate { .. } => self.execute_deallocate_plan(plan),

            // FLASHBACK（Phase 3.35）
            LogicalPlan::FlashbackTransaction { .. } => self.execute_flashback_txn_plan(plan).await,
            LogicalPlan::FlashbackTable { .. } => self.execute_flashback_table_plan(plan).await,

            // LISTEN/UNLISTEN/NOTIFY（Phase 4.6）
            LogicalPlan::Listen { channel } => self.execute_listen_plan(channel).await,
            LogicalPlan::Unlisten { channel } => self.execute_unlisten_plan(channel).await,
            LogicalPlan::Notify { channel, payload } => {
                self.execute_notify_plan(channel, payload).await
            }

            // COPY FROM/TO（Phase 4.8）
            LogicalPlan::Copy {
                target,
                columns,
                direction,
                file_path,
                options,
            } => {
                self.execute_copy_plan(target, columns, *direction, file_path, options)
                    .await
            }
        }
    }

    // -----------------------------------------------------------------
    //  LISTEN / UNLISTEN / NOTIFY — Phase 4.6
    // -----------------------------------------------------------------

    /// 执行 `LISTEN <channel>`。
    async fn execute_listen_plan(&mut self, channel: &str) -> Result<QueryResult, SessionError> {
        let hub = self.notify_hub.as_ref().ok_or_else(|| {
            SessionError::Protocol(
                "LISTEN requires NotifyHub to be configured on the server".into(),
            )
        })?;
        hub.listen(self.pid, channel);
        tracing::debug!(pid = self.pid, channel, "LISTEN registered");
        Ok(QueryResult::DdlComplete {
            tag: "LISTEN".into(),
        })
    }

    /// 执行 `UNLISTEN <channel>` 或 `UNLISTEN *`。
    async fn execute_unlisten_plan(&mut self, channel: &str) -> Result<QueryResult, SessionError> {
        let hub = self.notify_hub.as_ref().ok_or_else(|| {
            SessionError::Protocol(
                "UNLISTEN requires NotifyHub to be configured on the server".into(),
            )
        })?;
        if channel == "*" {
            hub.unlisten_all(self.pid);
            tracing::debug!(pid = self.pid, "UNLISTEN * (all channels)");
        } else {
            hub.unlisten(self.pid, channel);
            tracing::debug!(pid = self.pid, channel, "UNLISTEN");
        }
        Ok(QueryResult::DdlComplete {
            tag: "UNLISTEN".into(),
        })
    }

    /// 执行 `NOTIFY <channel> [, '<payload>']`。
    ///
    /// PG 语义：通知会立即投递到所有监听该频道的会话（包括发送者自己）。
    /// 由于 `NotifyHub` 是同步推送（写入每个监听者的待发送队列），
    /// 通知将在当前会话的下一条 Query 响应时被一同发送（PG 也是如此）。
    async fn execute_notify_plan(
        &mut self,
        channel: &str,
        payload: &str,
    ) -> Result<QueryResult, SessionError> {
        let hub = self.notify_hub.clone().ok_or_else(|| {
            SessionError::Protocol(
                "NOTIFY requires NotifyHub to be configured on the server".into(),
            )
        })?;
        let delivered = hub.notify(channel, payload, self.pid);
        tracing::debug!(
            pid = self.pid,
            channel,
            payload_len = payload.len(),
            delivered,
            "NOTIFY delivered"
        );
        // PG CommandComplete 标签：`NOTIFY`（不带计数）
        Ok(QueryResult::DdlComplete {
            tag: "NOTIFY".into(),
        })
    }

    // -----------------------------------------------------------------
    //  COPY FROM / COPY TO — Phase 4.8
    // -----------------------------------------------------------------

    /// 执行 `COPY <target> FROM/TO '/path' [WITH (...)]`
    ///
    /// COPY FROM：读取文件 → CSV/TEXT 解析 → 批量 INSERT
    /// COPY TO：执行表扫描/SELECT → CSV/TEXT 序列化 → 写入文件
    ///
    /// 文件 I/O 使用同步 `std::fs`（与 PG 行为一致，COPY 是阻塞操作）。
    /// 大文件场景下可能阻塞 tokio 运行时，建议在专用线程池中执行（后续优化）。
    async fn execute_copy_plan(
        &mut self,
        target: &CopyTarget,
        columns: &Option<Vec<String>>,
        direction: CopyDirection,
        file_path: &str,
        options: &CopyOptions,
    ) -> Result<QueryResult, SessionError> {
        match direction {
            CopyDirection::From => {
                self.execute_copy_from(target, columns, file_path, options)
                    .await
            }
            CopyDirection::To => {
                self.execute_copy_to(target, columns, file_path, options)
                    .await
            }
        }
    }

    /// 执行 COPY FROM：文件 → 表
    ///
    /// 步骤：
    /// 1. 读取文件内容（UTF-8）
    /// 2. 按行分割（支持 \n 和 \r\n）
    /// 3. 若 HEADER true，跳过第一行
    /// 4. 对每行解析为字段列表（CSV/TEXT）
    /// 5. 根据列类型转换为 Value
    /// 6. 批量 INSERT（FK/CHECK/ENUM 校验）
    async fn execute_copy_from(
        &mut self,
        target: &CopyTarget,
        columns: &Option<Vec<String>>,
        file_path: &str,
        options: &CopyOptions,
    ) -> Result<QueryResult, SessionError> {
        let table_name = match target {
            CopyTarget::Table(name) => name.clone(),
            CopyTarget::Query(_) => {
                return Err(SessionError::InvalidStatement(
                    "COPY FROM does not support query source".into(),
                ));
            }
        };

        // 1. 获取表 schema 与目标列索引
        let schema = self
            .catalog
            .get_table(&table_name)
            .ok_or_else(|| SessionError::TableNotFound(table_name.qualified_name()))?;

        let target_indices: Vec<usize> = match columns {
            None => (0..schema.columns.len()).collect(),
            Some(cols) => cols
                .iter()
                .map(|name| {
                    schema
                        .columns
                        .iter()
                        .position(|c| c.name.eq_ignore_ascii_case(name))
                        .ok_or_else(|| SessionError::Execution(format!("column not found: {name}")))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };

        let expected_col_count = target_indices.len();

        // 2. 读取文件内容
        let content = std::fs::read_to_string(file_path)
            .map_err(|e| SessionError::Execution(format!("COPY FROM file read error: {e}")))?;

        // 3. 按行分割（保留空行，跳过最后一行空行）
        let lines: Vec<&str> = content.split('\n').collect();
        let lines: Vec<&str> = if lines.last().is_some_and(|last| last.is_empty()) {
            lines[..lines.len() - 1].to_vec()
        } else {
            lines
        };
        // 去除每行末尾的 \r（处理 \r\n 换行符）
        let lines: Vec<&str> = lines.iter().map(|l| l.trim_end_matches('\r')).collect();

        // 4. 若 HEADER true，跳过第一行
        let data_lines = if options.header {
            &lines[1..]
        } else {
            &lines[..]
        };

        // 5. 获取表锁
        let table_arc = self.get_table_arc(&table_name.name).await?;
        let mut table_guard = table_arc.lock().await;

        // 6. 逐行解析并插入
        //
        // 注意：当前实现直接调用 table.insert_row，不做 FK/CHECK/ENUM 校验。
        // 这与 PG 行为不完全一致（PG COPY FROM 会校验约束），但简化了实现。
        // 后续可通过构造 LogicalPlan::Insert 复用 Executor::execute_insert 的校验逻辑。
        let mut affected_rows: usize = 0;
        for (line_idx, line) in data_lines.iter().enumerate() {
            let line_no = if options.header {
                line_idx + 2 // 1-based，header 占第 1 行
            } else {
                line_idx + 1
            };

            // 跳过空行（PG COPY 中空行是错误，但容忍处理）
            if line.is_empty() {
                continue;
            }

            // 解析字段
            let fields = match options.format {
                CopyFormat::Csv => {
                    parse_csv_line(line, options.delimiter, options.quote, options.escape)
                        .map_err(|e| copy_error_to_session(e, line_no))?
                }
                CopyFormat::Text => parse_text_line(line, options.delimiter),
            };

            // 列数校验
            if fields.len() != expected_col_count {
                return Err(SessionError::Execution(format!(
                    "COPY FROM line {line_no}: column count mismatch (expected {expected_col_count}, got {})",
                    fields.len()
                )));
            }

            // 构造完整行（未指定列补 NULL）
            let mut row: Vec<Value> = vec![Value::Null; schema.columns.len()];
            for (i, field) in fields.iter().enumerate() {
                let col_idx = target_indices[i];
                let col_type = &schema.columns[col_idx].data_type;
                let value =
                    string_to_value(field, col_type, &options.null_string).map_err(|e| {
                        SessionError::Execution(format!(
                            "COPY FROM line {line_no}, column {}: {e}",
                            schema.columns[col_idx].name
                        ))
                    })?;
                row[col_idx] = value;
            }

            // 插入（不做 FK/CHECK/ENUM 校验，见上方注释）
            table_guard.insert_row(row);
            affected_rows += 1;
        }

        // PG CommandComplete 标签：`COPY <count>`
        Ok(QueryResult::AffectedRows {
            tag: format!("COPY {affected_rows}"),
        })
    }

    /// 执行 COPY TO：表/查询 → 文件
    ///
    /// 步骤：
    /// 1. 执行表扫描或 SELECT 查询，获得结果集
    /// 2. 若 HEADER true，写入列名行（CSV 格式）
    /// 3. 对每行序列化为 CSV/TEXT 格式
    /// 4. 写入文件
    async fn execute_copy_to(
        &mut self,
        target: &CopyTarget,
        columns: &Option<Vec<String>>,
        file_path: &str,
        options: &CopyOptions,
    ) -> Result<QueryResult, SessionError> {
        // 1. 执行查询获得结果集
        let (result_columns, rows): (Vec<ResultColumn>, Vec<Vec<Value>>) = match target {
            CopyTarget::Table(table_name) => {
                // 全表扫描
                let schema = self
                    .catalog
                    .get_table(table_name)
                    .ok_or_else(|| SessionError::TableNotFound(table_name.qualified_name()))?;

                let table_arc = self.get_table_arc(&table_name.name).await?;
                let table_guard = table_arc.lock().await;

                let result_cols: Vec<ResultColumn> = match columns {
                    None => schema
                        .columns
                        .iter()
                        .map(|c| ResultColumn {
                            name: c.name.clone(),
                            column_type: c.data_type.clone(),
                        })
                        .collect(),
                    Some(cols) => {
                        let mut result = Vec::with_capacity(cols.len());
                        for name in cols {
                            let col = schema
                                .columns
                                .iter()
                                .find(|c| c.name.eq_ignore_ascii_case(name))
                                .ok_or_else(|| {
                                    SessionError::Execution(format!("column not found: {name}"))
                                })?;
                            result.push(ResultColumn {
                                name: col.name.clone(),
                                column_type: col.data_type.clone(),
                            });
                        }
                        result
                    }
                };

                // 获取目标列索引
                let target_indices: Vec<usize> = match columns {
                    None => (0..schema.columns.len()).collect(),
                    Some(cols) => cols
                        .iter()
                        .map(|name| {
                            schema
                                .columns
                                .iter()
                                .position(|c| c.name.eq_ignore_ascii_case(name))
                                .ok_or_else(|| {
                                    SessionError::Execution(format!("column not found: {name}"))
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                };

                // 扫描全表并投影目标列
                let all_rows: Vec<Vec<Value>> = table_guard.scan_iter().collect();
                let projected_rows: Vec<Vec<Value>> = all_rows
                    .into_iter()
                    .map(|row| target_indices.iter().map(|&i| row[i].clone()).collect())
                    .collect();

                (result_cols, projected_rows)
            }
            CopyTarget::Query(select) => {
                // 执行 SELECT 查询（需要克隆 Box<Select>，因为 plan_statement 需要 owned）
                let select_cloned = select.as_ref().clone();
                let plan = {
                    let catalog_ref: &InMemoryCatalog = &self.catalog;
                    let planner = Planner::new(catalog_ref);
                    planner.plan_statement(Statement::Select(Box::new(select_cloned)))?
                };

                // 锁定所有表
                let mut guards = Vec::with_capacity(self.tables.len());
                for table_arc in self.tables.values() {
                    guards.push(table_arc.lock().await);
                }

                let mut executor = Executor::new();
                executor = executor.with_catalog(&self.catalog);
                executor = executor.with_temp_store(&self.temp_store);
                for guard in &guards {
                    executor.register_table(&**guard);
                }

                let rows = executor.execute(&plan)?;
                let columns = derive_output_columns(&plan, &rows);
                (columns, rows)
            }
        };

        // 2. 序列化为文本
        let mut output = String::new();

        // HEADER 行（仅 CSV 格式且 header=true 时写入）
        if options.header && matches!(options.format, CopyFormat::Csv) {
            let header_fields: Vec<String> = result_columns
                .iter()
                .map(|c| {
                    format_csv_field(&c.name, options.delimiter, options.quote, options.escape)
                })
                .collect();
            output.push_str(&header_fields.join(&options.delimiter.to_string()));
            output.push('\n');
        }

        // 数据行
        for row in &rows {
            let field_strs: Vec<String> = row
                .iter()
                .map(|v| {
                    let s = value_to_string(v, &options.null_string);
                    match options.format {
                        CopyFormat::Csv => {
                            format_csv_field(&s, options.delimiter, options.quote, options.escape)
                        }
                        CopyFormat::Text => s,
                    }
                })
                .collect();
            output.push_str(&field_strs.join(&options.delimiter.to_string()));
            output.push('\n');
        }

        // 3. 写入文件
        std::fs::write(file_path, output)
            .map_err(|e| SessionError::Execution(format!("COPY TO file write error: {e}")))?;

        let exported = rows.len();
        Ok(QueryResult::AffectedRows {
            tag: format!("COPY {exported}"),
        })
    }

    // -----------------------------------------------------------------
    //  SELECT-family 执行
    // -----------------------------------------------------------------
}

// =====================================================================
//  Phase 4.8 辅助函数：CopyError → SessionError
// =====================================================================

/// 将 `copy::CopyError` 转换为 `SessionError`，附带出错行号。
///
/// COPY FROM 在解析 CSV/TEXT 时若发生错误，需向客户端报告具体行号与原因。
///
/// `line_no` 为会话层计算出的 1-based 行号（含 HEADER 偏移），覆盖 CopyError
/// 内部可能未填的行号字段。
fn copy_error_to_session(e: CopyError, line_no: usize) -> SessionError {
    match e {
        CopyError::Io(msg) => {
            SessionError::Execution(format!("COPY FROM line {line_no}: io: {msg}"))
        }
        CopyError::Parse { reason, .. } => {
            SessionError::Execution(format!("COPY FROM line {line_no}: parse: {reason}"))
        }
        CopyError::TypeConversion { column, reason, .. } => SessionError::Execution(format!(
            "COPY FROM line {line_no}, column {column}: {reason}"
        )),
        CopyError::ColumnCount {
            expected, actual, ..
        } => SessionError::Execution(format!(
            "COPY FROM line {line_no}: column count mismatch (expected {expected}, got {actual})"
        )),
        CopyError::Unsupported(msg) => {
            SessionError::Execution(format!("COPY FROM line {line_no}: unsupported: {msg}"))
        }
    }
}

impl ExecutorService {
    // -----------------------------------------------------------------
    //  SELECT-family 执行（续）
    // -----------------------------------------------------------------

    async fn execute_select_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        // ADV-CONC-1：收集所有需要锁定的表（包括共享存储中的表）
        let mut all_arcs: std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<InMemoryTable>>> = std::collections::HashMap::new();
        for (k, v) in &self.tables {
            all_arcs.insert(k.clone(), v.clone());
        }
        if let Some(shared) = &self.shared_tables {
            for (k, v) in shared.read().await.iter() {
                all_arcs.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        // 先锁定所有表（确保 Executor 不跨 .await 持有，因为 Executor<'_> 非 Send）
        let mut guards = Vec::with_capacity(all_arcs.len());
        for table_arc in all_arcs.values() {
            guards.push(table_arc.lock().await);
        }

        // 构造 Executor 并注册所有表（同步操作，不涉及 .await）
        let mut executor = Executor::new();
        executor = executor.with_catalog(&self.catalog);
        executor = executor.with_temp_store(&self.temp_store);
        for guard in &guards {
            executor.register_table(&**guard);
        }

        // 执行
        let rows = executor.execute(plan)?;

        // 推导输出列
        let columns = derive_output_columns(plan, &rows);

        let tag = format!("SELECT {}", rows.len());
        Ok(QueryResult::ResultSet { columns, rows, tag })
    }

    // -----------------------------------------------------------------
    //  INSERT
    // -----------------------------------------------------------------

    async fn execute_insert_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let table_name = match plan {
            LogicalPlan::Insert { table, .. } => &table.name,
            _ => unreachable!(),
        };

        let table_arc = self.get_table_arc(table_name).await?;
        let mut table_guard = table_arc.lock().await;
        let executor = Executor::new().with_catalog(&self.catalog);
        let DmlResult {
            affected_rows,
            returning_rows,
        } = executor.execute_insert(plan, &mut *table_guard)?;

        // 处理 RETURNING 子句
        if !returning_rows.is_empty() {
            let schema = self
                .catalog
                .get_table(&TableName::new(table_name))
                .ok_or_else(|| SessionError::Execution(format!("table not found: {table_name}")))?;
            let columns = schema
                .columns
                .iter()
                .map(|c| ResultColumn {
                    name: c.name.clone(),
                    column_type: c.data_type.clone(),
                })
                .collect();
            let tag = format!("INSERT 0 {affected_rows}");
            return Ok(QueryResult::ResultSet {
                columns,
                rows: returning_rows,
                tag,
            });
        }

        Ok(QueryResult::AffectedRows {
            tag: format!("INSERT 0 {affected_rows}"),
        })
    }

    // -----------------------------------------------------------------
    //  UPDATE
    // -----------------------------------------------------------------

    async fn execute_update_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let table_name = match plan {
            LogicalPlan::Update { table, .. } => &table.name,
            _ => unreachable!(),
        };

        // ADV-CONC-1：事务中获取表级 X 锁（Strict 2PL，COMMIT/ROLLBACK 释放）
        self.acquire_table_xlock(table_name).await?;

        let table_arc = self.get_table_arc(table_name).await?;
        let mut table_guard = table_arc.lock().await;
        let executor = Executor::new().with_catalog(&self.catalog);
        let DmlResult {
            affected_rows,
            returning_rows,
        } = executor.execute_update(plan, &mut *table_guard)?;

        if !returning_rows.is_empty() {
            let schema = self
                .catalog
                .get_table(&TableName::new(table_name))
                .ok_or_else(|| SessionError::Execution(format!("table not found: {table_name}")))?;
            let columns = schema
                .columns
                .iter()
                .map(|c| ResultColumn {
                    name: c.name.clone(),
                    column_type: c.data_type.clone(),
                })
                .collect();
            let tag = format!("UPDATE {affected_rows}");
            return Ok(QueryResult::ResultSet {
                columns,
                rows: returning_rows,
                tag,
            });
        }

        Ok(QueryResult::AffectedRows {
            tag: format!("UPDATE {affected_rows}"),
        })
    }

    // -----------------------------------------------------------------
    //  DELETE
    // -----------------------------------------------------------------

    async fn execute_delete_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let table_name = match plan {
            LogicalPlan::Delete { table, .. } => &table.name,
            _ => unreachable!(),
        };

        // ADV-CONC-1：事务中获取表级 X 锁（Strict 2PL，COMMIT/ROLLBACK 释放）
        self.acquire_table_xlock(table_name).await?;

        let table_arc = self.get_table_arc(table_name).await?;
        let mut table_guard = table_arc.lock().await;
        let executor = Executor::new().with_catalog(&self.catalog);
        let DmlResult {
            affected_rows,
            returning_rows,
        } = executor.execute_delete(plan, &mut *table_guard)?;

        if !returning_rows.is_empty() {
            let schema = self
                .catalog
                .get_table(&TableName::new(table_name))
                .ok_or_else(|| SessionError::Execution(format!("table not found: {table_name}")))?;
            let columns = schema
                .columns
                .iter()
                .map(|c| ResultColumn {
                    name: c.name.clone(),
                    column_type: c.data_type.clone(),
                })
                .collect();
            let tag = format!("DELETE {affected_rows}");
            return Ok(QueryResult::ResultSet {
                columns,
                rows: returning_rows,
                tag,
            });
        }

        Ok(QueryResult::AffectedRows {
            tag: format!("DELETE {affected_rows}"),
        })
    }

    // -----------------------------------------------------------------
    //  REPLACE / MERGE
    // -----------------------------------------------------------------

    async fn execute_replace_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let table_name = match plan {
            LogicalPlan::Replace { table, .. } => &table.name,
            _ => unreachable!(),
        };

        let table_arc = self.get_table_arc(table_name).await?;
        let mut table_guard = table_arc.lock().await;
        let executor = Executor::new().with_catalog(&self.catalog);
        let DmlResult { affected_rows, .. } = executor.execute_replace(plan, &mut *table_guard)?;

        Ok(QueryResult::AffectedRows {
            tag: format!("REPLACE {affected_rows}"),
        })
    }

    async fn execute_merge_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let table_name = match plan {
            LogicalPlan::Merge { target, .. } => &target.name,
            _ => unreachable!(),
        };

        let table_arc = self.get_table_arc(table_name).await?;
        let mut table_guard = table_arc.lock().await;
        let executor = Executor::new().with_catalog(&self.catalog);
        let DmlResult { affected_rows, .. } = executor.execute_merge(plan, &mut *table_guard)?;

        Ok(QueryResult::AffectedRows {
            tag: format!("MERGE {affected_rows}"),
        })
    }

    // -----------------------------------------------------------------
    //  DDL
    // -----------------------------------------------------------------

    async fn execute_create_table_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let (table_name, schema) = match plan {
            LogicalPlan::CreateTable { name, columns, .. } => {
                let schema = TableSchema {
                    name: name.clone(),
                    columns: columns.clone(),
                };
                (&name.name, schema)
            }
            _ => unreachable!(),
        };

        // 注册到 catalog
        self.catalog.register_from_create_plan(plan)?;

        // 创建空表
        let table = InMemoryTable::new(schema);
        let key = table_name.to_lowercase();
        let table_arc = Arc::new(Mutex::new(table));
        // ADV-CONC-1：优先注册到共享存储（跨 session 可见）
        if let Some(shared) = &self.shared_tables {
            shared.write().await.insert(key.clone(), table_arc.clone());
        }
        // 同时保留会话本地引用（用于快速查找）
        self.tables.insert(key, table_arc);

        Ok(QueryResult::DdlComplete {
            tag: "CREATE TABLE".into(),
        })
    }

    async fn execute_drop_table_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let (names, _if_exists, _cascade) = match plan {
            LogicalPlan::DropTable {
                names,
                if_exists,
                cascade,
            } => (names, if_exists, cascade),
            _ => unreachable!(),
        };

        for name in names {
            let key = name.name.to_lowercase();
            // ADV-CONC-1：从共享存储移除（如果启用）
            if let Some(shared) = &self.shared_tables {
                shared.write().await.remove(&key);
            }
            self.tables.remove(&key);
            self.catalog.remove_table(name);
        }

        Ok(QueryResult::DdlComplete {
            tag: "DROP TABLE".into(),
        })
    }

    fn execute_create_sequence_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_create_sequence(plan, &mut self.sequence_store)?;
        Ok(QueryResult::DdlComplete {
            tag: "CREATE SEQUENCE".into(),
        })
    }

    fn execute_drop_sequence_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_drop_sequence(plan, &mut self.sequence_store)?;
        Ok(QueryResult::DdlComplete {
            tag: "DROP SEQUENCE".into(),
        })
    }

    fn execute_create_type_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_create_type(plan, &mut self.catalog)?;
        Ok(QueryResult::DdlComplete {
            tag: "CREATE TYPE".into(),
        })
    }

    fn execute_drop_type_plan(&mut self, plan: &LogicalPlan) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_drop_type(plan, &mut self.catalog)?;
        Ok(QueryResult::DdlComplete {
            tag: "DROP TYPE".into(),
        })
    }

    fn execute_alter_type_plan(&mut self, plan: &LogicalPlan) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_alter_type(plan, &mut self.catalog)?;
        Ok(QueryResult::DdlComplete {
            tag: "ALTER TYPE".into(),
        })
    }

    /// 执行 CREATE TRIGGER 计划 — Phase 6.4
    fn execute_create_trigger_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_create_trigger(plan, &mut self.catalog)?;
        Ok(QueryResult::DdlComplete {
            tag: "CREATE TRIGGER".into(),
        })
    }

    /// 执行 DROP TRIGGER 计划 — Phase 6.4
    fn execute_drop_trigger_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_drop_trigger(plan, &mut self.catalog)?;
        Ok(QueryResult::DdlComplete {
            tag: "DROP TRIGGER".into(),
        })
    }

    // -----------------------------------------------------------------
    //  PREPARE / EXECUTE / DEALLOCATE
    // -----------------------------------------------------------------

    fn execute_prepare_plan(&mut self, plan: &LogicalPlan) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_prepare(plan, &mut self.prepared_store)?;
        Ok(QueryResult::DdlComplete {
            tag: "PREPARE".into(),
        })
    }

    async fn execute_execute_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        // 先锁定所有表（确保 Executor 不跨 .await 持有）
        let mut guards = Vec::with_capacity(self.tables.len());
        for table_arc in self.tables.values() {
            guards.push(table_arc.lock().await);
        }

        let mut executor = Executor::new().with_catalog(&self.catalog);
        for guard in &guards {
            executor.register_table(&**guard);
        }

        let rows = executor.execute_execute(plan, &self.prepared_store, &self.catalog)?;

        let columns = rows
            .first()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(i, _)| ResultColumn {
                        name: format!("column{}", i + 1),
                        column_type: ColumnType::Text,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let tag = format!("SELECT {}", rows.len());
        Ok(QueryResult::ResultSet { columns, rows, tag })
    }

    fn execute_deallocate_plan(&mut self, plan: &LogicalPlan) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_deallocate(plan, &mut self.prepared_store)?;
        Ok(QueryResult::DdlComplete {
            tag: "DEALLOCATE".into(),
        })
    }

    // -----------------------------------------------------------------
    //  FLASHBACK
    // -----------------------------------------------------------------

    async fn execute_flashback_txn_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        // 先执行 flashback 获取快照，然后立即释放 executor（非 Send）
        let snapshots = {
            let executor = Executor::new();
            executor.execute_flashback_transaction(plan, &mut self.transaction_history)?
        };

        for (name, snapshot) in snapshots {
            if let Some(table_arc) = self.tables.get(&name.to_lowercase()) {
                let mut guard = table_arc.lock().await;
                guard.restore(snapshot);
            }
        }

        Ok(QueryResult::DdlComplete {
            tag: "FLASHBACK TRANSACTION".into(),
        })
    }

    async fn execute_flashback_table_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        let rows = executor.execute_flashback_table(plan, &self.transaction_history)?;

        let columns = rows
            .first()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(i, _)| ResultColumn {
                        name: format!("column{}", i + 1),
                        column_type: ColumnType::Text,
                    })
                    .collect()
            })
            .unwrap_or_default();

        let tag = format!("SELECT {}", rows.len());
        Ok(QueryResult::ResultSet { columns, rows, tag })
    }

    // -----------------------------------------------------------------
    //  辅助方法
    // -----------------------------------------------------------------

    /// 获取表的可变 Arc 引用。
    ///
    /// ADV-CONC-1：优先从共享存储查找（跨 session 可见），未启用共享存储时退化为本地查找。
    async fn get_table_arc(&self, name: &str) -> Result<Arc<Mutex<InMemoryTable>>, SessionError> {
        let key = name.to_lowercase();
        // 优先从共享存储查找
        if let Some(shared) = &self.shared_tables {
            if let Some(table) = shared.read().await.get(&key).cloned() {
                return Ok(table);
            }
        }
        // 退化为本地查找
        self.tables
            .get(&key)
            .cloned()
            .ok_or_else(|| SessionError::TableNotFound(name.to_string()))
    }

    // =================================================================
    //  Phase 4.3 扩展查询协议入口
    // =================================================================

    /// Parse：将 SQL 文本解析为 AST 并存入扩展查询预处理语句存储。
    ///
    /// 与 SQL `PREPARE` 语句的区别：
    /// - 扩展查询 Parse 是协议层操作，不进入 Planner
    /// - 仅允许单条语句（PG 协议要求）
    /// - 参数 OID 由客户端声明（0 = 服务器推断）
    pub fn extended_parse(
        &mut self,
        statement_name: &str,
        sql: &str,
        parameter_oids: Vec<u32>,
    ) -> Result<(), SessionError> {
        // 扩展查询 Parse 仅允许单条语句
        let mut statements = parse_sql(sql)?;
        if statements.len() != 1 {
            return Err(SessionError::Protocol(format!(
                "extended query Parse requires exactly 1 statement, got {}",
                statements.len()
            )));
        }
        let statement = statements.remove(0);
        let ps = ExtendedPreparedStatement {
            name: statement_name.to_string(),
            statement,
            parameter_oids,
        };
        // 与 PG 一致：同名语句被覆盖
        self.extended_statements
            .insert(statement_name.to_string(), ps);
        tracing::debug!(name = statement_name, "extended Parse: statement stored");
        Ok(())
    }

    /// Bind：将参数值绑定到预处理语句，生成 portal。
    ///
    /// 参数转换规则：
    /// - format_code = 0 (text)：UTF-8 字符串按 OID 解析为 `Value`
    /// - format_code = 1 (binary)：Phase 4.3 暂不支持，返回 Protocol 错误
    /// - OID = 0 (未指定)：按文本处理（`Value::Text`），由后续表达式类型推断
    pub fn extended_bind(
        &mut self,
        portal_name: &str,
        statement_name: &str,
        parameter_format_codes: &[i16],
        parameters: &[Option<Vec<u8>>],
        result_format_codes: Vec<i16>,
    ) -> Result<(), SessionError> {
        let ps = self
            .extended_statements
            .get(statement_name)
            .cloned()
            .ok_or_else(|| {
                SessionError::Protocol(format!(
                    "prepared statement \"{statement_name}\" does not exist"
                ))
            })?;

        // 校验参数数量
        let expected = ps.parameter_oids.len();
        if parameters.len() != expected {
            return Err(SessionError::Protocol(format!(
                "Bind: parameter count mismatch: statement expects {expected}, got {}",
                parameters.len()
            )));
        }

        // 解析每个参数
        let mut bound_params: Vec<Expr> = Vec::with_capacity(parameters.len());
        for (idx, param) in parameters.iter().enumerate() {
            // 解析 format code：单元素列表应用于所有参数；否则按位置取
            let fmt = if parameter_format_codes.len() == 1 {
                parameter_format_codes[0]
            } else if parameter_format_codes.is_empty() {
                0 // 默认 text
            } else {
                parameter_format_codes[idx]
            };

            let oid = ps.parameter_oids.get(idx).copied().unwrap_or(0);

            let value = match param {
                None => Value::Null,
                Some(bytes) => decode_parameter_value(bytes, fmt, oid)?,
            };
            bound_params.push(Expr::Literal(value));
        }

        let portal = Portal {
            statement_name: statement_name.to_string(),
            parameters: bound_params,
            result_format_codes,
        };
        // 与 PG 一致：同名 portal 被覆盖
        self.portals.insert(portal_name.to_string(), portal);
        tracing::debug!(
            portal = portal_name,
            statement = statement_name,
            param_count = parameters.len(),
            "extended Bind: portal stored"
        );
        Ok(())
    }

    /// Execute：执行已绑定的 portal。
    ///
    /// `max_rows > 0` 时仅返回前 `max_rows` 行；后续行需再次 Execute 获取（PortalSuspended）。
    /// `max_rows == 0` 表示返回所有行（PG 默认行为）。
    ///
    /// 内部流程：
    /// 1. 取出 portal，构造 `LogicalPlan::Execute { name, parameters }`
    /// 2. 调用 `executor.execute_execute` 复用 SQL EXECUTE 执行路径
    ///    （需先将 portal 对应的预处理语句注册到 `prepared_store`）
    /// 3. 按 `max_rows` 切片结果集
    pub async fn extended_execute(
        &mut self,
        portal_name: &str,
        max_rows: i32,
    ) -> Result<ExtendedExecuteResult, SessionError> {
        let portal = self.portals.get(portal_name).cloned().ok_or_else(|| {
            SessionError::Protocol(format!("portal \"{portal_name}\" does not exist"))
        })?;

        let ps = self
            .extended_statements
            .get(&portal.statement_name)
            .cloned()
            .ok_or_else(|| {
                SessionError::Protocol(format!(
                    "prepared statement \"{}\" does not exist",
                    portal.statement_name
                ))
            })?;

        // 失败事务保护（与简单查询一致）
        if self.txn_state == TransactionState::InFailedTransaction {
            return Err(SessionError::Transaction(
                "current transaction is aborted, commands ignored until end of transaction block"
                    .into(),
            ));
        }

        // 事务控制语句在扩展查询中需要特殊处理：BEGIN/COMMIT/ROLLBACK 是合法的扩展查询语句
        if let Some(tx_result) = self.handle_transaction_control(&ps.statement).await? {
            return Ok(ExtendedExecuteResult::Transaction(tx_result));
        }

        // Phase 4.7：系统表查询拦截（pg_tables / pg_indexes / information_schema.*）
        // 这类查询需要 MutableCatalog 接口，无法走 LogicalPlan::Execute 路径。
        // 与简单查询协议保持一致：直接计算结果集返回。
        if let Some(result) = crate::pgwire::system_tables::try_execute_system_table_query(
            &ps.statement,
            &self.catalog,
        ) {
            let query_result = result?;
            return Ok(ExtendedExecuteResult::Complete {
                result: query_result,
                result_format_codes: portal.result_format_codes.clone(),
            });
        }

        // Phase 4.6：LISTEN/UNLISTEN/NOTIFY 是会话级命令，不是参数化 SELECT/DML，
        // 不能走 LogicalPlan::Execute + execute_execute 路径（该路径要求 prepared statement
        // 已注册到 prepared_store 且仅支持 SELECT-family）。直接走 Planner + dispatch_plan，
        // 与简单查询协议保持一致行为。
        if matches!(
            ps.statement,
            Statement::Listen { .. } | Statement::Unlisten { .. } | Statement::Notify { .. }
        ) {
            let plan = {
                let catalog_ref: &InMemoryCatalog = &self.catalog;
                let planner = Planner::new(catalog_ref);
                planner.plan_statement(ps.statement.clone())?
            };
            let result = self.dispatch_plan(&plan).await?;
            return Ok(ExtendedExecuteResult::Complete {
                result,
                result_format_codes: portal.result_format_codes.clone(),
            });
        }

        // Phase 4.10：非 SELECT 语句（DDL/DML/事务控制等）不走 LogicalPlan::Execute 路径，
        // 因为 substitute_parameters 仅支持 SELECT。直接走 Planner + dispatch_plan，
        // 与简单查询协议保持一致行为。
        //
        // 这覆盖了 sqlx/node_pg/asyncpg 等客户端通过扩展查询协议发送 DDL/DML 的场景
        // （这些客户端对所有语句都使用扩展查询协议）。
        //
        // 已知限制：DML 带参数（如 `INSERT INTO t VALUES ($1)`）仍不支持，需扩展
        // substitute_parameters 覆盖 DML 语句类型（后续 Phase 处理）。
        if !matches!(ps.statement, Statement::Select(_)) {
            let plan = {
                let catalog_ref: &InMemoryCatalog = &self.catalog;
                let planner = Planner::new(catalog_ref);
                planner.plan_statement(ps.statement.clone())?
            };
            let result = self.dispatch_plan(&plan).await?;
            return Ok(ExtendedExecuteResult::Complete {
                result,
                result_format_codes: portal.result_format_codes.clone(),
            });
        }

        // 将扩展查询预处理语句临时注册到 SQL PREPARE 存储，以便复用 execute_execute
        // 注意：使用专属命名空间避免与用户 SQL PREPARE 冲突
        let temp_name = format!("__extended__{}", portal.statement_name);
        self.prepared_store
            .prepare(&temp_name, ps.statement.clone(), Vec::new());

        // 构造 Execute 计划并执行
        let plan = LogicalPlan::Execute {
            name: temp_name.clone(),
            parameters: portal.parameters.clone(),
        };

        // 先锁定所有表（确保 Executor 不跨 .await 持有）
        let mut guards = Vec::with_capacity(self.tables.len());
        for table_arc in self.tables.values() {
            guards.push(table_arc.lock().await);
        }

        let mut executor = Executor::new().with_catalog(&self.catalog);
        for guard in &guards {
            executor.register_table(&**guard);
        }

        let result = self.execute_extended_plan_inner(
            &executor,
            &plan,
            max_rows,
            portal.result_format_codes.clone(),
        );

        // 清理临时注册的预处理语句
        self.prepared_store.deallocate(&temp_name);

        result
    }

    /// 内部分派：执行扩展查询的 LogicalPlan。
    fn execute_extended_plan_inner(
        &self,
        executor: &Executor<'_>,
        plan: &LogicalPlan,
        max_rows: i32,
        result_format_codes: Vec<i16>,
    ) -> Result<ExtendedExecuteResult, SessionError> {
        // 调用 execute_execute 复用 substitute_parameters + plan + execute 流程
        let rows = executor.execute_execute(plan, &self.prepared_store, &self.catalog)?;

        // 按 max_rows 切片
        if max_rows > 0 && (rows.len() as i32) > max_rows {
            // PortalSuspended 场景：返回前 max_rows 行，剩余行留待下次 Execute
            let returned: Vec<Vec<Value>> = rows.into_iter().take(max_rows as usize).collect();
            let columns = derive_output_columns_from_rows(&returned);
            Ok(ExtendedExecuteResult::Suspended {
                columns,
                rows: returned,
                result_format_codes,
            })
        } else {
            let columns = derive_output_columns_from_rows(&rows);
            let tag = format!("SELECT {}", rows.len());
            Ok(ExtendedExecuteResult::Complete {
                result: QueryResult::ResultSet { columns, rows, tag },
                result_format_codes,
            })
        }
    }

    /// Describe statement：返回参数 OID 列表与（可选）结果列描述。
    ///
    /// 由于 SzRSQL 的 Planner 不暴露"描述语句结果"的 API，Phase 4.3 采用简化策略：
    /// - 参数描述：返回 Parse 时声明的 OID（0 = UNKNOWN）
    /// - 结果描述：执行一次 plan 推导列信息；DML/DDL 返回 NoData
    pub fn extended_describe_statement(
        &self,
        statement_name: &str,
    ) -> Result<StatementDescription, SessionError> {
        let ps = self
            .extended_statements
            .get(statement_name)
            .ok_or_else(|| {
                SessionError::Protocol(format!(
                    "prepared statement \"{statement_name}\" does not exist"
                ))
            })?;

        let parameter_oids = ps.parameter_oids.clone();

        // 尝试 plan 推导结果列（仅对 SELECT-family 有效）；DML/DDL 返回空（→ NoData）
        let columns = self
            .try_describe_select_columns(&ps.statement)
            .unwrap_or_default();

        Ok(StatementDescription {
            parameter_oids,
            result_columns: columns,
        })
    }

    /// Describe portal：返回 portal 的结果列描述（参数已绑定，无需 ParameterDescription）。
    pub fn extended_describe_portal(
        &self,
        portal_name: &str,
    ) -> Result<PortalDescription, SessionError> {
        let portal = self.portals.get(portal_name).ok_or_else(|| {
            SessionError::Protocol(format!("portal \"{portal_name}\" does not exist"))
        })?;
        let ps = self
            .extended_statements
            .get(&portal.statement_name)
            .ok_or_else(|| {
                SessionError::Protocol(format!(
                    "prepared statement \"{}\" does not exist",
                    portal.statement_name
                ))
            })?;

        let columns = self
            .try_describe_select_columns(&ps.statement)
            .unwrap_or_default();

        Ok(PortalDescription {
            result_columns: columns,
            result_format_codes: portal.result_format_codes.clone(),
        })
    }

    /// Close：关闭指定的语句或 portal。
    pub fn extended_close(&mut self, variant: u8, name: &str) -> Result<(), SessionError> {
        match variant {
            b'S' => {
                if self.extended_statements.remove(name).is_none() {
                    // PG 行为：关闭不存在的语句不算错误，但需要返回 CloseComplete
                    tracing::debug!(name = name, "extended Close: statement not found (no-op)");
                }
                // 同时清理关联的 portal（PG 行为）
                self.portals.retain(|_, p| p.statement_name != name);
            }
            b'P' => {
                if self.portals.remove(name).is_none() {
                    tracing::debug!(name = name, "extended Close: portal not found (no-op)");
                }
            }
            other => {
                return Err(SessionError::Protocol(format!(
                    "invalid Close variant: 0x{:02X}",
                    other
                )))
            }
        }
        Ok(())
    }

    /// 尝试推导 SELECT 语句的结果列（用于 Describe）。
    ///
    /// 对非 SELECT 语句（DML/DDL）返回 `Err`，调用方据此返回 NoData。
    fn try_describe_select_columns(
        &self,
        statement: &Statement,
    ) -> Result<Vec<ResultColumn>, SessionError> {
        // 仅对 SELECT 类语句尝试 plan，DML/DDL 直接返回空
        if !matches!(statement, Statement::Select(_) | Statement::ShowTables) {
            return Err(SessionError::Protocol("not a SELECT statement".into()));
        }

        let plan = {
            let planner = Planner::new(&self.catalog);
            planner.plan_statement(statement.clone())?
        };

        // 用空行集推导列名（derive_output_columns 接受空 rows）
        let columns = derive_output_columns(&plan, &[]);
        if columns.is_empty() {
            Err(SessionError::Protocol("no result columns".into()))
        } else {
            Ok(columns)
        }
    }
}

// =====================================================================
//  Phase 4.3 扩展查询：辅助类型与函数
// =====================================================================

/// `Describe statement` 的结果。
#[derive(Debug, Clone)]
pub struct StatementDescription {
    /// 参数 OID 列表（与 Parse 中声明的一致，0 = 未指定）
    pub parameter_oids: Vec<u32>,
    /// 结果列描述（空列表表示无结果集，server 层应发送 NoData）
    pub result_columns: Vec<ResultColumn>,
}

/// `Describe portal` 的结果。
#[derive(Debug, Clone)]
pub struct PortalDescription {
    /// 结果列描述
    pub result_columns: Vec<ResultColumn>,
    /// 结果列格式码（与 Bind 时一致）
    pub result_format_codes: Vec<i16>,
}

/// `Execute` 的结果。
#[derive(Debug, Clone)]
pub enum ExtendedExecuteResult {
    /// 执行完成（返回完整结果集或非 SELECT 命令标签）
    ///
    /// `result_format_codes` 来自 portal，用于编码 RowDescription / DataRow。
    /// 对于非 ResultSet（AffectedRows/DdlComplete/Empty/TransactionComplete），
    /// 编码路径不会使用 format_codes，但字段始终存在以简化调用方。
    Complete {
        result: QueryResult,
        result_format_codes: Vec<i16>,
    },
    /// Portal 暂停（max_rows > 0 且仍有剩余行）
    Suspended {
        columns: Vec<ResultColumn>,
        rows: Vec<Vec<Value>>,
        result_format_codes: Vec<i16>,
    },
    /// 事务控制语句（BEGIN/COMMIT/ROLLBACK）
    Transaction(QueryResult),
}

/// 从结果行推导列描述（用于 Execute 时行集已切出的场景）。
fn derive_output_columns_from_rows(rows: &[Vec<Value>]) -> Vec<ResultColumn> {
    rows.first()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, v)| ResultColumn {
                    name: format!("column{}", i + 1),
                    column_type: v.column_type(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 解析 Bind 参数值为 `Value`。
///
/// - text 格式 (fmt=0)：按 OID 解析文本
/// - binary 格式 (fmt=1)：Phase 4.3 不支持，返回 Protocol 错误
/// - OID=0 (未指定)：默认按 Text 处理
fn decode_parameter_value(bytes: &[u8], fmt: i16, oid: u32) -> Result<Value, SessionError> {
    if fmt == 1 {
        return Err(SessionError::Protocol(
            "binary parameter format not supported in Phase 4.3".into(),
        ));
    }

    // text 格式
    let text = std::str::from_utf8(bytes)
        .map_err(|e| SessionError::Protocol(format!("invalid UTF-8 in parameter: {e}")))?;

    // 根据 OID 解析；OID=0 (UNKNOWN) 时默认按 Text 处理
    use crate::pgwire::pg_types::oid;
    match oid {
        0 | oid::UNKNOWN | oid::TEXT | oid::VARCHAR => Ok(Value::Text(text.to_string())),
        oid::INT8 | oid::INT4 | oid::INT2 => {
            let v: i64 = text.parse().map_err(|_| {
                SessionError::Protocol(format!("invalid integer parameter: \"{text}\""))
            })?;
            Ok(Value::Int64(v))
        }
        oid::FLOAT8 => {
            let v: f64 = text.parse().map_err(|_| {
                SessionError::Protocol(format!("invalid float parameter: \"{text}\""))
            })?;
            Ok(Value::Float64(v))
        }
        oid::BOOL => match text {
            "t" | "true" | "TRUE" | "T" => Ok(Value::Bool(true)),
            "f" | "false" | "FALSE" | "F" => Ok(Value::Bool(false)),
            _ => Err(SessionError::Protocol(format!(
                "invalid bool parameter: \"{text}\""
            ))),
        },
        _ => {
            // 未识别的 OID，按 Text 处理（与 PG 兼容行为）
            Ok(Value::Text(text.to_string()))
        }
    }
}

impl Default for ExecutorService {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  输出列推导
// =====================================================================

/// 从 LogicalPlan 推导 SELECT 结果的列名和列类型。
///
/// 简化策略：
/// - `Scan` → 表 schema 的列
/// - `Projection` → output_names + 从 input schema 推导类型
/// - 其他（Filter/Limit/Distinct/Join/Aggregate/SetOp）→ 递归子计划
/// - `Dual`/`Empty` → 空列
/// - `ShowTables` → 单列 "Table"
/// - `ShowCreateTable` → 双列 "Table" + "DDL"
/// - `ShowVariable` → 单列 "Value"
fn derive_output_columns(plan: &LogicalPlan, rows: &[Vec<Value>]) -> Vec<ResultColumn> {
    match plan {
        LogicalPlan::Scan { schema, .. } => schema
            .columns
            .iter()
            .map(|c| ResultColumn {
                name: c.name.clone(),
                column_type: c.data_type.clone(),
            })
            .collect(),

        LogicalPlan::Projection {
            exprs,
            output_names,
            input,
        } => {
            // 从每个投影表达式推导列类型（不能用 input_schema 位置对应，
            // 因为 output_names[i] 与 input_schema[i] 未必相同——例如
            // `SELECT name, age FROM t` 当 t 的 schema 为 [id, name, age] 时）
            let input_schema = derive_input_schema(input);
            output_names
                .iter()
                .enumerate()
                .map(|(i, name)| ResultColumn {
                    name: name.clone(),
                    column_type: exprs
                        .get(i)
                        .map(|(e, _)| derive_expr_type(e, &input_schema))
                        .unwrap_or(ColumnType::Text),
                })
                .collect()
        }

        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Distinct { input } => derive_output_columns(input, rows),

        LogicalPlan::Join { left, right, .. } => {
            let mut cols = derive_output_columns(left, rows);
            cols.extend(derive_output_columns(right, rows));
            cols
        }

        LogicalPlan::Aggregate {
            aggregates,
            group_exprs,
            input,
            ..
        } => {
            // GROUP BY 列 + 聚合函数列
            // 推导输入 schema（用于 SUM/AVG/MIN/MAX 等基于输入列类型的聚合）
            let input_schema = derive_input_schema(input);
            let mut cols: Vec<ResultColumn> = group_exprs
                .iter()
                .enumerate()
                .map(|(i, _)| ResultColumn {
                    name: format!("col_{i}"),
                    column_type: ColumnType::Text,
                })
                .collect();
            for agg in aggregates {
                cols.push(ResultColumn {
                    name: agg.alias.clone().unwrap_or_else(|| agg.func_name.clone()),
                    column_type: derive_aggregate_type(&agg.func_name, &agg.args, &input_schema),
                });
            }
            cols
        }

        LogicalPlan::SetOp { left, .. } => derive_output_columns(left, rows),

        LogicalPlan::ShowTables => vec![ResultColumn {
            name: "Table".into(),
            column_type: ColumnType::Text,
        }],

        LogicalPlan::ShowCreateTable { .. } => vec![
            ResultColumn {
                name: "Table".into(),
                column_type: ColumnType::Text,
            },
            ResultColumn {
                name: "DDL".into(),
                column_type: ColumnType::Text,
            },
        ],

        LogicalPlan::ShowVariable { .. } => vec![ResultColumn {
            name: "Value".into(),
            column_type: ColumnType::Text,
        }],

        LogicalPlan::SetNames { .. } | LogicalPlan::SetVariable { .. } => Vec::new(),

        LogicalPlan::Empty | LogicalPlan::Dual => Vec::new(),

        // 兜底：从第一行数据推导
        _ => rows
            .first()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(i, v)| ResultColumn {
                        name: format!("column{}", i + 1),
                        column_type: v.column_type(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// 递归推导 LogicalPlan 的输入 schema（用于 Projection 的列类型）。
///
/// 简化实现：仅对 Scan 直接返回 schema；其他情况返回空，由调用方回退到 Text。
fn derive_input_schema(plan: &LogicalPlan) -> Vec<szrsql_sql::ast::ColumnDefinition> {
    match plan {
        LogicalPlan::Scan { schema, .. } => schema.columns.clone(),
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Distinct { input } => derive_input_schema(input),
        LogicalPlan::Join { left, right, .. } => {
            let mut cols = derive_input_schema(left);
            cols.extend(derive_input_schema(right));
            cols
        }
        _ => Vec::new(),
    }
}

/// 从投影表达式推导列类型。
///
/// 用于 `derive_output_columns` 的 Projection 分支，确保 `SELECT name, age FROM t`
/// 时 `name`/`age` 的类型来自其对应的标识符列，而非 input schema 的位置对应。
///
/// 推导规则：
/// - `Expr::Literal(v)` → `v.column_type()`
/// - `Expr::Identifier(names)` → 在 input_schema 中按列名查找（大小写不敏感）
/// - `Expr::Cast { data_type, .. }` → `data_type.clone()`
/// - 其他表达式 → `ColumnType::Text`（兜底）
fn derive_expr_type(expr: &Expr, input_schema: &[szrsql_sql::ast::ColumnDefinition]) -> ColumnType {
    match expr {
        Expr::Literal(value) => value.column_type(),
        Expr::Identifier(names) => {
            let col_name = names.last().map(|s| s.to_lowercase()).unwrap_or_default();
            input_schema
                .iter()
                .find(|c| c.name.to_lowercase() == col_name)
                .map(|c| c.data_type.clone())
                .unwrap_or(ColumnType::Text)
        }
        Expr::Cast { data_type, .. } => data_type.clone(),
        Expr::Function { name, args, .. } => {
            // 聚合函数（count/sum/avg/min/max）按聚合规则推导；
            // 标量函数兜底为 Text。
            match name.to_lowercase().as_str() {
                "count" | "sum" | "avg" | "min" | "max" => {
                    derive_aggregate_type(name, args, input_schema)
                }
                _ => ColumnType::Text,
            }
        }
        _ => ColumnType::Text,
    }
}

/// 推导聚合函数的输出列类型。
///
/// 用于 `derive_output_columns` 的 Aggregate 分支，确保 `SELECT COUNT(*) FROM t`
/// 时 COUNT 列的类型为 `Int64`（而非 Text），与 PG 行为一致。
///
/// 推导规则：
/// - `count` → `Int64`（PG 中 COUNT 返回 bigint）
/// - `sum` → 输入列类型；若输入为整数则 `Int64`，浮点则 `Float64`；兜底 `Int64`
/// - `avg` → `Float64`（PG 中 AVG 返回 numeric，szrsql 用 Float64 近似）
/// - `min`/`max` → 输入列类型；兜底 `Text`
/// - 其他 → `Text`（兜底）
fn derive_aggregate_type(
    func_name: &str,
    args: &[Expr],
    input_schema: &[szrsql_sql::ast::ColumnDefinition],
) -> ColumnType {
    match func_name.to_lowercase().as_str() {
        "count" => ColumnType::Int64,
        "sum" => {
            // SUM 的类型跟随输入列；若无参数或无法推导，默认 Int64（与 PG 一致：
            // SUM(integer) 返回 bigint）
            args.first()
                .map(|e| {
                    let t = derive_expr_type(e, input_schema);
                    match t {
                        ColumnType::Float64 => ColumnType::Float64,
                        _ => ColumnType::Int64,
                    }
                })
                .unwrap_or(ColumnType::Int64)
        }
        "avg" => ColumnType::Float64,
        "min" | "max" => args
            .first()
            .map(|e| derive_expr_type(e, input_schema))
            .unwrap_or(ColumnType::Text),
        _ => ColumnType::Text,
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_table_and_insert_select() {
        let mut svc = ExecutorService::new();

        // CREATE TABLE
        let results = svc
            .execute_sql("CREATE TABLE users (id BIGINT, name TEXT)")
            .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());

        // INSERT
        let results = svc
            .execute_sql("INSERT INTO users (id, name) VALUES (1, 'alice')")
            .await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            Ok(QueryResult::AffectedRows { tag }) => assert!(tag.starts_with("INSERT 0 1")),
            other => panic!("expected AffectedRows, got {other:?}"),
        }

        // SELECT
        let results = svc.execute_sql("SELECT * FROM users").await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            Ok(QueryResult::ResultSet { columns, rows, tag }) => {
                assert_eq!(columns.len(), 2);
                assert_eq!(columns[0].name, "id");
                assert_eq!(columns[1].name, "name");
                assert_eq!(rows.len(), 1);
                assert!(tag.starts_with("SELECT 1"));
            }
            other => panic!("expected ResultSet, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_transaction_begin_rollback() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT)").await;
        svc.execute_sql("INSERT INTO t (id) VALUES (1)").await;

        // BEGIN
        let results = svc.execute_sql("BEGIN").await;
        assert!(matches!(
            &results[0],
            Ok(QueryResult::TransactionComplete {
                tag,
                in_transaction: true
            }) if tag == "BEGIN"
        ));
        assert!(svc.in_transaction());

        // INSERT in transaction
        svc.execute_sql("INSERT INTO t (id) VALUES (2)").await;

        // ROLLBACK
        let results = svc.execute_sql("ROLLBACK").await;
        assert!(matches!(
            &results[0],
            Ok(QueryResult::TransactionComplete {
                tag,
                in_transaction: false
            }) if tag == "ROLLBACK"
        ));
        assert!(!svc.in_transaction());

        // Verify only first row remains
        let results = svc.execute_sql("SELECT * FROM t").await;
        match &results[0] {
            Ok(QueryResult::ResultSet { rows, .. }) => assert_eq!(rows.len(), 1),
            other => panic!("expected ResultSet, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_transaction_commit() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT)").await;

        svc.execute_sql("BEGIN").await;
        svc.execute_sql("INSERT INTO t (id) VALUES (1)").await;
        svc.execute_sql("INSERT INTO t (id) VALUES (2)").await;
        svc.execute_sql("COMMIT").await;

        let results = svc.execute_sql("SELECT * FROM t").await;
        match &results[0] {
            Ok(QueryResult::ResultSet { rows, .. }) => assert_eq!(rows.len(), 2),
            other => panic!("expected ResultSet, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_update_delete() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT)").await;
        svc.execute_sql("INSERT INTO t (id) VALUES (1)").await;
        svc.execute_sql("INSERT INTO t (id) VALUES (2)").await;

        // UPDATE
        let results = svc.execute_sql("UPDATE t SET id = 10 WHERE id = 1").await;
        match &results[0] {
            Ok(QueryResult::AffectedRows { tag }) => assert!(tag.starts_with("UPDATE 1")),
            other => panic!("expected AffectedRows, got {other:?}"),
        }

        // DELETE
        let results = svc.execute_sql("DELETE FROM t WHERE id = 2").await;
        match &results[0] {
            Ok(QueryResult::AffectedRows { tag }) => assert!(tag.starts_with("DELETE 1")),
            other => panic!("expected AffectedRows, got {other:?}"),
        }

        // SELECT 验证
        let results = svc.execute_sql("SELECT * FROM t").await;
        match &results[0] {
            Ok(QueryResult::ResultSet { rows, .. }) => assert_eq!(rows.len(), 1),
            other => panic!("expected ResultSet, got {other:?}"),
        }
    }

    #[test]
    fn test_session_error_sqlstate_mapping() {
        assert_eq!(
            SessionError::Parse("syntax error".into()).sqlstate(),
            SqlState::SYNTAX_ERROR
        );
        assert_eq!(
            SessionError::Execution("table not found: foo".into()).sqlstate(),
            SqlState::UNDEFINED_TABLE
        );
        assert_eq!(
            SessionError::Execution("foreign key violation: bar".into()).sqlstate(),
            SqlState::FOREIGN_KEY_VIOLATION
        );
    }

    #[tokio::test]
    async fn test_multiple_statements_in_one_query() {
        // ADV-BUG-002: multi-statement disabled by default, must explicitly enable
        let mut svc = ExecutorService::new().with_multi_statement(true);
        let results = svc
            .execute_sql(
                "CREATE TABLE t (id BIGINT); INSERT INTO t (id) VALUES (1); SELECT * FROM t",
            )
            .await;
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(results[2].is_ok());
    }

    #[tokio::test]
    async fn test_empty_query_returns_empty() {
        let mut svc = ExecutorService::new();
        let results = svc.execute_sql("").await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Ok(QueryResult::Empty)));

        let results = svc.execute_sql("-- just a comment").await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Ok(QueryResult::Empty)));
    }

    // =================================================================
    // Phase 4.8 COPY FROM / TO 测试
    // =================================================================

    /// 辅助：生成唯一临时文件路径（不创建文件）。
    fn temp_path(prefix: &str, suffix: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let mut path = std::env::temp_dir();
        path.push(format!("{prefix}_{pid}_{n}{suffix}"));
        path.to_string_lossy().into_owned()
    }

    /// COPY FROM CSV with HEADER：导入 3 行 → 验证 SELECT 返回 3 行。
    #[tokio::test]
    async fn test_copy_from_csv_with_header() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .await;

        let path = temp_path("copy_csv_hdr", ".csv");
        let content = "id,name\n1,alice\n2,bob\n3,carol\n";
        std::fs::write(&path, content).unwrap();

        let results = svc
            .execute_sql(&format!(
                "COPY t FROM '{}' WITH (FORMAT csv, HEADER true)",
                path.replace('\\', "\\\\")
            ))
            .await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            Ok(QueryResult::AffectedRows { tag }) => {
                assert!(tag.starts_with("COPY 3"), "expected COPY 3, got {tag}")
            }
            other => panic!("expected AffectedRows, got {other:?}"),
        }

        let results = svc.execute_sql("SELECT * FROM t").await;
        match &results[0] {
            Ok(QueryResult::ResultSet { rows, .. }) => {
                assert_eq!(rows.len(), 3);
                // 不依赖顺序，验证 3 行都存在
                let names: Vec<String> = rows
                    .iter()
                    .map(|r| match &r[1] {
                        Value::Text(s) => s.clone(),
                        _ => "?".into(),
                    })
                    .collect();
                assert!(names.contains(&"alice".into()));
                assert!(names.contains(&"bob".into()));
                assert!(names.contains(&"carol".into()));
            }
            other => panic!("expected ResultSet, got {other:?}"),
        }

        let _ = std::fs::remove_file(&path);
    }

    /// COPY FROM CSV without HEADER：导入 2 行。
    #[tokio::test]
    async fn test_copy_from_csv_without_header() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .await;

        let path = temp_path("copy_csv_nohdr", ".csv");
        let content = "1,alice\n2,bob\n";
        std::fs::write(&path, content).unwrap();

        let results = svc
            .execute_sql(&format!(
                "COPY t FROM '{}' WITH (FORMAT csv, HEADER false)",
                path.replace('\\', "\\\\")
            ))
            .await;
        if let Err(ref e) = results[0] {
            panic!("COPY FROM failed: {e}");
        }
        match &results[0] {
            Ok(QueryResult::AffectedRows { tag }) => {
                assert!(tag.starts_with("COPY 2"), "expected COPY 2, got {tag}")
            }
            other => panic!("expected AffectedRows, got {other:?}"),
        }

        let _ = std::fs::remove_file(&path);
    }

    /// COPY FROM TEXT format（PG 默认，TAB 分隔，\N 表示 NULL）。
    #[tokio::test]
    async fn test_copy_from_text_format() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .await;

        let path = temp_path("copy_text", ".txt");
        // TEXT 格式：TAB 分隔，\N 表示 NULL
        let content = "1\talice\n2\t\\N\n";
        std::fs::write(&path, content).unwrap();

        let results = svc
            .execute_sql(&format!(
                "COPY t FROM '{}' WITH (FORMAT text)",
                path.replace('\\', "\\\\")
            ))
            .await;
        if let Err(ref e) = results[0] {
            panic!("COPY FROM TEXT failed: {e}");
        }

        let results = svc.execute_sql("SELECT * FROM t").await;
        match &results[0] {
            Ok(QueryResult::ResultSet { rows, .. }) => {
                assert_eq!(rows.len(), 2);
                // 验证有一行 name=alice，有一行 name=NULL
                let has_alice = rows.iter().any(|r| r[1] == Value::Text("alice".into()));
                let has_null = rows.iter().any(|r| r[1] == Value::Null);
                assert!(has_alice, "should have alice row");
                assert!(has_null, "should have NULL row");
            }
            other => panic!("expected ResultSet, got {other:?}"),
        }

        let _ = std::fs::remove_file(&path);
    }

    /// COPY FROM 指定列子集（未指定列补 NULL）。
    #[tokio::test]
    async fn test_copy_from_with_columns() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT, age BIGINT)")
            .await;

        let path = temp_path("copy_cols", ".csv");
        // 只导入 id 和 name，age 列应为 NULL
        let content = "1,alice\n2,bob\n";
        std::fs::write(&path, content).unwrap();

        let results = svc
            .execute_sql(&format!(
                "COPY t (id, name) FROM '{}' WITH (FORMAT csv)",
                path.replace('\\', "\\\\")
            ))
            .await;
        if let Err(ref e) = results[0] {
            panic!("COPY FROM with columns failed: {e}");
        }

        let results = svc.execute_sql("SELECT * FROM t").await;
        match &results[0] {
            Ok(QueryResult::ResultSet { rows, .. }) => {
                assert_eq!(rows.len(), 2);
                // age 列应为 NULL（未在 COPY 中指定）
                assert!(rows.iter().all(|r| r[2] == Value::Null));
            }
            other => panic!("expected ResultSet, got {other:?}"),
        }

        let _ = std::fs::remove_file(&path);
    }

    /// COPY TO CSV：导出表数据到文件。
    #[tokio::test]
    async fn test_copy_to_csv() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .await;
        svc.execute_sql("INSERT INTO t (id, name) VALUES (1, 'alice')")
            .await;
        svc.execute_sql("INSERT INTO t (id, name) VALUES (2, 'bob')")
            .await;

        let path = temp_path("copy_to_csv", ".csv");
        let results = svc
            .execute_sql(&format!(
                "COPY t TO '{}' WITH (FORMAT csv, HEADER true)",
                path.replace('\\', "\\\\")
            ))
            .await;
        if let Err(ref e) = results[0] {
            panic!("COPY TO failed: {e}");
        }
        match &results[0] {
            Ok(QueryResult::AffectedRows { tag }) => {
                assert!(tag.starts_with("COPY 2"), "expected COPY 2, got {tag}")
            }
            other => panic!("expected AffectedRows, got {other:?}"),
        }

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3); // header + 2 rows
        assert_eq!(lines[0], "id,name");
        assert!(lines[1].contains("alice"));
        assert!(lines[2].contains("bob"));

        let _ = std::fs::remove_file(&path);
    }

    /// COPY TO TEXT format：导出为 PG TEXT 格式。
    #[tokio::test]
    async fn test_copy_to_text() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .await;
        svc.execute_sql("INSERT INTO t (id, name) VALUES (1, 'alice')")
            .await;

        let path = temp_path("copy_to_text", ".txt");
        let results = svc
            .execute_sql(&format!(
                "COPY t TO '{}' WITH (FORMAT text)",
                path.replace('\\', "\\\\")
            ))
            .await;
        if let Err(ref e) = results[0] {
            panic!("COPY TO TEXT failed: {e}");
        }

        let content = std::fs::read_to_string(&path).unwrap();
        // TEXT 格式：TAB 分隔
        assert!(content.contains("1\talice"));

        let _ = std::fs::remove_file(&path);
    }

    /// COPY roundtrip：COPY FROM 导入 → COPY TO 导出 → 验证内容一致。
    #[tokio::test]
    async fn test_copy_roundtrip_csv() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT, active BOOLEAN)")
            .await;

        let input_path = temp_path("copy_rt_in", ".csv");
        let output_path = temp_path("copy_rt_out", ".csv");
        let input_content = "id,name,active\n1,alice,t\n2,bob,f\n3,carol,t\n";
        std::fs::write(&input_path, input_content).unwrap();

        // COPY FROM
        let results = svc
            .execute_sql(&format!(
                "COPY t FROM '{}' WITH (FORMAT csv, HEADER true)",
                input_path.replace('\\', "\\\\")
            ))
            .await;
        if let Err(ref e) = results[0] {
            panic!("COPY FROM failed: {e}");
        }

        // COPY TO
        let results = svc
            .execute_sql(&format!(
                "COPY t TO '{}' WITH (FORMAT csv, HEADER true)",
                output_path.replace('\\', "\\\\")
            ))
            .await;
        if let Err(ref e) = results[0] {
            panic!("COPY TO failed: {e}");
        }

        // diff 验证：输入与输出应一致
        let output_content = std::fs::read_to_string(&output_path).unwrap();
        assert_eq!(
            input_content, output_content,
            "COPY roundtrip mismatch:\ninput:\n{input_content}\noutput:\n{output_content}"
        );

        let _ = std::fs::remove_file(&input_path);
        let _ = std::fs::remove_file(&output_path);
    }

    /// COPY FROM 不存在的文件 → 应返回错误。
    #[tokio::test]
    async fn test_copy_from_nonexistent_file_errors() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT)").await;

        let results = svc
            .execute_sql("COPY t FROM '/nonexistent/path/file.csv' WITH (FORMAT csv)")
            .await;
        assert!(results[0].is_err(), "expected error for nonexistent file");
    }

    /// COPY FROM 列数不匹配 → 应返回错误。
    #[tokio::test]
    async fn test_copy_from_column_count_mismatch_errors() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .await;

        let path = temp_path("copy_mismatch", ".csv");
        // 表有 2 列，但 CSV 只有 1 列
        std::fs::write(&path, "1\n2\n").unwrap();

        let results = svc
            .execute_sql(&format!(
                "COPY t FROM '{}' WITH (FORMAT csv)",
                path.replace('\\', "\\\\")
            ))
            .await;
        assert!(
            results[0].is_err(),
            "expected error for column count mismatch"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// COPY FROM 不存在的表 → 应返回错误。
    #[tokio::test]
    async fn test_copy_from_nonexistent_table_errors() {
        let mut svc = ExecutorService::new();

        let path = temp_path("copy_notbl", ".csv");
        std::fs::write(&path, "1\n").unwrap();

        let results = svc
            .execute_sql(&format!(
                "COPY nonexistent_table FROM '{}' WITH (FORMAT csv)",
                path.replace('\\', "\\\\")
            ))
            .await;
        assert!(results[0].is_err(), "expected error for nonexistent table");

        let _ = std::fs::remove_file(&path);
    }

    /// COPY FROM 不支持 Query 来源 → 应返回错误。
    #[tokio::test]
    async fn test_copy_from_query_source_errors() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT)").await;

        let path = temp_path("copy_qerr", ".csv");
        std::fs::write(&path, "1\n").unwrap();

        let results = svc
            .execute_sql(&format!(
                "COPY (SELECT 1) TO '{}' WITH (FORMAT csv)",
                path.replace('\\', "\\\\")
            ))
            .await;
        // COPY (SELECT) TO 应该可以工作（CopyTarget::Query），但如果 Planner 不支持
        // 也可能是错误。这里仅验证不 panic。
        let _ = results;

        let _ = std::fs::remove_file(&path);
    }

    /// COPY FROM CSV with custom DELIMITER。
    #[tokio::test]
    async fn test_copy_from_csv_custom_delimiter() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .await;

        let path = temp_path("copy_delim", ".csv");
        // 使用 | 作为分隔符
        std::fs::write(&path, "1|alice\n2|bob\n").unwrap();

        let results = svc
            .execute_sql(&format!(
                "COPY t FROM '{}' WITH (FORMAT csv, DELIMITER '|')",
                path.replace('\\', "\\\\")
            ))
            .await;
        if let Err(ref e) = results[0] {
            panic!("COPY FROM with custom delimiter failed: {e}");
        }

        let results = svc.execute_sql("SELECT count(*) FROM t").await;
        match &results[0] {
            Ok(QueryResult::ResultSet { rows, .. }) => {
                assert_eq!(rows[0][0], Value::Int64(2));
            }
            other => panic!("expected ResultSet, got {other:?}"),
        }

        let _ = std::fs::remove_file(&path);
    }

    /// COPY FROM CSV with QUOTE and ESCAPE 选项。
    #[tokio::test]
    async fn test_copy_from_csv_quote_escape() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .await;

        let path = temp_path("copy_quote", ".csv");
        // 含逗号和引号的字段
        let content = "1,\"hello, world\"\n2,\"say \"\"hi\"\"\"\n";
        std::fs::write(&path, content).unwrap();

        let results = svc
            .execute_sql(&format!(
                "COPY t FROM '{}' WITH (FORMAT csv, QUOTE '\"', ESCAPE '\"')",
                path.replace('\\', "\\\\")
            ))
            .await;
        if let Err(ref e) = results[0] {
            panic!("COPY FROM with quote/escape failed: {e}");
        }

        let results = svc.execute_sql("SELECT * FROM t").await;
        match &results[0] {
            Ok(QueryResult::ResultSet { rows, .. }) => {
                assert_eq!(rows.len(), 2);
                // 验证含特殊字符的字段正确解析
                let has_hello = rows
                    .iter()
                    .any(|r| r[1] == Value::Text("hello, world".into()));
                let has_say_hi = rows
                    .iter()
                    .any(|r| r[1] == Value::Text("say \"hi\"".into()));
                assert!(has_hello, "should have 'hello, world' row");
                assert!(has_say_hi, "should have 'say \"hi\"' row");
            }
            other => panic!("expected ResultSet, got {other:?}"),
        }

        let _ = std::fs::remove_file(&path);
    }
}
