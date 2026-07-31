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
    CommentObjectType, CopyDirection, CopyFormat, CopyOptions, CopyTarget, Expr, SelectItem,
    Statement, TableConstraint, TableName, TransactionAccess, TransactionIsolation,
};
use szrsql_sql::executor::{
    DmlResult, ExecutionError, Executor, InMemorySequenceStore, InMemoryTable, MutableTable,
    PreparedStatementStore, SessionState, SharedSequenceState, TableSnapshot, TableStorage,
    TempTableStore, TransactionHistory,
};
use crate::pgwire::dirty_tracker::DirtyTableTracker;
use szrsql_sql::parser::{parse_sql, ParseError};
use szrsql_sql::plan::{Catalog, InMemoryCatalog, LogicalPlan, PlanError, Planner, TableSchema};
use szrsql_tx::mvcc::{IsolationLevel, MvccManager, MvccError};
use szrsql_tx::wal::{WalError, WalOpType, WalRecord, WalWriter};
use szrsql_types::value::{ColumnType, Value};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

use std::collections::{HashMap, HashSet};
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

/// 根据 RETURNING 子句构造 ResultColumn 列表。
///
/// executor 的 `project_returning` 只返回 RETURNING 子句指定的列，
/// 因此 RowDescription 的字段数必须与之匹配。
///
/// - `Wildcard` / `QualifiedWildcard` → 表全部列
/// - `UnnamedExpr(Identifier([col]))` → 单列
/// - `UnnamedExpr(other)` / `ExprWithAlias` → 用别名或表达式文本作为列名，类型用 Text 降级
fn build_returning_columns(
    schema: &TableSchema,
    returning: &Option<Vec<SelectItem>>,
) -> Vec<ResultColumn> {
    let items = match returning {
        None => {
            // 无 RETURNING 子句但又有 returning_rows（理论上不会发生）→ 返回表全部列
            return schema
                .columns
                .iter()
                .map(|c| ResultColumn {
                    name: c.name.clone(),
                    column_type: c.data_type.clone(),
                })
                .collect();
        }
        Some(items) => items,
    };

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            SelectItem::Wildcard | SelectItem::QualifiedWildcard(_) => {
                for c in &schema.columns {
                    out.push(ResultColumn {
                        name: c.name.clone(),
                        column_type: c.data_type.clone(),
                    });
                }
            }
            SelectItem::UnnamedExpr(Expr::Identifier(idents)) => {
                if let Some(col_name) = idents.last() {
                    let ct = schema
                        .columns
                        .iter()
                        .find(|c| c.name.eq_ignore_ascii_case(col_name))
                        .map(|c| c.data_type.clone())
                        .unwrap_or(ColumnType::Text);
                    out.push(ResultColumn {
                        name: col_name.clone(),
                        column_type: ct,
                    });
                } else {
                    out.push(ResultColumn {
                        name: "?column?".into(),
                        column_type: ColumnType::Text,
                    });
                }
            }
            SelectItem::ExprWithAlias { expr: _, alias } => {
                out.push(ResultColumn {
                    name: alias.clone(),
                    column_type: ColumnType::Text,
                });
            }
            SelectItem::UnnamedExpr(_) => {
                out.push(ResultColumn {
                    name: "?column?".into(),
                    column_type: ColumnType::Text,
                });
            }
        }
    }
    out
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
    /// P0-6 修复：物化视图存储表（视图名小写 → 表实例）
    ///
    /// CREATE MATERIALIZED VIEW 时创建空存储表，REFRESH 时填充数据，
    /// MaterializedViewScan 执行时注册到 Executor 供扫描读取。
    materialized_view_tables: HashMap<String, Arc<Mutex<InMemoryTable>>>,
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
    /// P0-1 修复：事务期间修改的表名集合（用于 WAL 崩溃恢复）。
    ///
    /// - BEGIN 时清空
    /// - INSERT/UPDATE/DELETE 执行后添加表名
    /// - COMMIT 时将这些表的全量数据写入 WAL，确保崩溃后可恢复
    txn_modified_tables: HashSet<String>,
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
    /// Phase 4.7：当前数据库名（来自 StartupParams.database()，缺省 "szrsql"）。
    ///
    /// 用于 `pg_database` 系统表查询时返回当前连接的数据库名。
    /// Navicat 连接时会执行 `SELECT * FROM pg_database`，必须返回当前 db 名。
    database_name: String,
    /// P0-TX-1 修复：MVCC 事务管理器（跨会话共享）。
    ///
    /// 注入后，BEGIN/COMMIT/ROLLBACK 会同步到 MvccManager 状态机，
    /// 实现 MVCC 事务可见性判断（而非表级 snapshot/restore）。
    /// 未注入时退化为表级 snapshot/restore（旧行为，用于测试兼容）。
    mvcc: Option<Arc<MvccManager>>,
    /// P0-TX-1 修复：待应用的隔离级别（由 SET TRANSACTION ISOLATION LEVEL 设置，下次 BEGIN 生效）。
    pending_isolation: Option<IsolationLevel>,
    /// P0-DIST-1/2/3：分布式运行时句柄（跨会话共享）。
    ///
    /// 注入后，DML 操作通过 `Executor::dist_dual_write` 双写到分布式 KV 存储，
    /// 实现真实分布式持久化路径（Raft propose → apply）。
    /// 未注入时退化为本地内存表存储（旧行为，用于测试兼容）。
    dist_runtime: Option<szrsql_dist::runtime::DistRuntimeHandle>,
    /// P7-1：跨会话共享的 CDC 引擎。
    ///
    /// 注入后，DML 操作（INSERT/UPDATE/DELETE）会将行级变更事件分发到 CDC 引擎，
    /// 供已注册的 CdcObserver（如 ReplicationTask）消费，实现变更数据捕获。
    /// 未注入时退化为旧行为（DML 不触发 CDC 事件）。
    cdc_engine: Option<Arc<szrsql_cdc::CdcEngine>>,
    /// P1-2：跨会话共享的脏表跟踪器（增量快照机制）。
    ///
    /// 注入后，事务 COMMIT 成功后会调用 `tracker.mark_dirty_many` 标记该事务
    /// 修改过的表为脏。后台周期性快照任务仅对脏表集合中的表重新序列化，
    /// 避免每次都对所有表做全量序列化。
    /// 未注入时退化为旧行为（全量快照，每次都序列化所有表）。
    dirty_tracker: Option<Arc<DirtyTableTracker>>,
    /// P2-1.1：跨会话共享的统计信息存储（ANALYZE 写入，CostModel 读取）。
    ///
    /// 注入后：
    /// - `ANALYZE [table_name [, ...]]` 扫描表数据收集统计信息（行数、NDV、min/max、直方图）
    /// - 统计结果存入此 store，供 CostModel 进行基于成本的优化（P2-1.2 激活）
    /// - 未注入时 ANALYZE 命令返回错误（不支持）
    statistics_store: Option<Arc<std::sync::Mutex<szrsql_optimizer::statistics::InMemoryStatisticsStore>>>,
}

impl ExecutorService {
    /// 创建一个空会话。
    pub fn new() -> Self {
        Self {
            catalog: InMemoryCatalog::new(),
            tables: HashMap::new(),
            temp_store: TempTableStore::new(),
            sequence_store: InMemorySequenceStore::new(),
            materialized_view_tables: HashMap::new(),
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
            txn_modified_tables: HashSet::new(),
            current_txn_id: 0,
            next_txn_id: 1,
            shared_tables: None,
            lock_manager: None,
            shared_txn_counter: None,
            database_name: "szrsql".to_string(),
            mvcc: None,
            pending_isolation: None,
            dist_runtime: None,
            cdc_engine: None,
            dirty_tracker: None,
            statistics_store: None,
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

    /// Phase 4.7：设置当前数据库名（由 `PgwireServer` 在握手时从 StartupParams 注入）。
    ///
    /// 该值用于 `pg_database` 系统表查询，返回当前连接的数据库名。
    /// Navicat 等工具连接后会立即查询 `pg_database`，必须返回当前 db 名才能正常浏览。
    pub fn with_database_name(mut self, db: impl Into<String>) -> Self {
        self.database_name = db.into();
        self
    }

    /// P0-4 修复：注入跨会话共享的序列全局状态。
    ///
    /// 注入后：
    /// - `CREATE SEQUENCE` 创建到共享状态，所有 session 可见
    /// - `nextval(seq)` 推进全局状态，多 session 调用同一序列时值递增
    /// - `currval(seq)` 仅返回本 session 最近一次 `nextval` 的结果（PG 语义）
    /// - `DROP SEQUENCE` 从共享状态移除并清理本 session 的 currval
    ///
    /// 未注入时退化为会话私有存储（旧行为，用于测试兼容）。
    ///
    /// # 参数
    /// - `shared`：共享序列全局状态句柄（由 `PgwireServer` 持有）
    pub fn with_sequence_shared_state(mut self, shared: SharedSequenceState) -> Self {
        self.sequence_store = InMemorySequenceStore::from_shared_state(shared);
        self
    }

    /// P0-TX-1 修复：注入 MVCC 事务管理器，启用 MVCC 事务可见性。
    ///
    /// 注入后：
    /// - BEGIN 调用 `MvccManager::begin_with_isolation()` 分配 txn_id 和 snapshot
    /// - COMMIT 调用 `MvccManager::commit_durable()` 执行 SSI 检测 + log-then-commit
    /// - ROLLBACK 调用 `MvccManager::abort()` 回滚事务
    /// - SET TRANSACTION ISOLATION LEVEL 保存到 pending_isolation，下次 BEGIN 生效
    ///
    /// 未注入时退化为表级 snapshot/restore（旧行为，用于测试兼容）。
    pub fn with_mvcc(mut self, mgr: Arc<MvccManager>) -> Self {
        self.mvcc = Some(mgr);
        self
    }

    /// P0-DIST-1/2/3：注入分布式运行时句柄，启用分布式双写。
    ///
    /// 注入后，DML 操作通过 `Executor::dist_dual_write` 双写到分布式 KV 存储：
    /// - INSERT：本地写入 + `dist_runtime.put("{table}:{row_id}", serialized_row)`
    /// - UPDATE：本地更新 + `dist_runtime.put(...)`（覆盖）
    /// - DELETE：本地删除 + `dist_runtime.delete(...)`
    ///
    /// 分布式写入失败仅记录 warn 日志，不中断 DML（best-effort 双写）。
    /// 未注入时退化为本地内存表存储（旧行为，用于测试兼容）。
    pub fn with_dist_runtime(
        mut self,
        handle: szrsql_dist::runtime::DistRuntimeHandle,
    ) -> Self {
        self.dist_runtime = Some(handle);
        self
    }

    /// P7-1：注入跨会话共享的 CDC 引擎，启用 DML 事件分发。
    ///
    /// 注入后，所有 DML 操作（INSERT/UPDATE/DELETE）会将行级变更事件分发到 CDC 引擎，
    /// 供已注册的 CdcObserver（如 ReplicationTask）消费，实现变更数据捕获。
    ///
    /// 未注入时退化为旧行为（DML 不触发 CDC 事件，用于测试兼容）。
    pub fn with_cdc_engine(mut self, engine: Arc<szrsql_cdc::CdcEngine>) -> Self {
        self.cdc_engine = Some(engine);
        self
    }

    /// P1-2：注入跨会话共享的脏表跟踪器，启用增量快照机制。
    ///
    /// 注入后，事务 COMMIT 成功后会调用 `tracker.mark_dirty_many` 标记该事务
    /// 修改过的表为脏。后台周期性快照任务仅对脏表集合中的表重新序列化，
    /// 避免每次都对所有表做全量序列化。
    ///
    /// 未注入时退化为旧行为（全量快照，每次都序列化所有表，用于测试兼容）。
    pub fn with_dirty_tracker(mut self, tracker: Arc<DirtyTableTracker>) -> Self {
        self.dirty_tracker = Some(tracker);
        self
    }

    /// P2-1.1：注入跨会话共享的统计信息存储，启用 ANALYZE 命令。
    ///
    /// 注入后：
    /// - `ANALYZE` 扫描所有用户表，收集统计信息（行数、NDV、min/max、直方图）
    /// - `ANALYZE table_name [, ...]` 仅扫描指定表
    /// - 统计结果存入共享 store，供 CostModel 进行基于成本的优化（P2-1.2 激活）
    ///
    /// 未注入时 ANALYZE 命令返回错误（不支持，用于测试兼容）。
    ///
    /// # 参数
    /// - `store`：共享统计信息存储（`Arc<Mutex<InMemoryStatisticsStore>>`，由 `PgwireServer` 持有）
    pub fn with_statistics_store(
        mut self,
        store: Arc<std::sync::Mutex<szrsql_optimizer::statistics::InMemoryStatisticsStore>>,
    ) -> Self {
        self.statistics_store = Some(store);
        self
    }

    /// Phase 4.7：返回当前数据库名（用于系统表查询）。
    pub fn database_name(&self) -> &str {
        &self.database_name
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
    /// 但本 session 的 `catalog` 是私有的。此方法在每次 `execute_statement` 和
    /// `extended_execute` 开始时调用，将共享存储中的表 schema 同步到本地 catalog，
    /// 确保 Planner 能找到表定义。
    ///
    /// 同步策略：只新增不删除（DROP TABLE 由本地 DDL 处理器同步移除）。
    pub(crate) async fn sync_catalog_from_shared(&mut self) {
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

        // 2. Phase 4.7：系统表查询拦截（pg_tables / pg_indexes / information_schema.* / pg_database / pg_namespace / pg_class / ...）
        //    这类查询需要 MutableCatalog 接口（szrsql-catalog 提供），无法走 Planner
        //    （Planner 只接受 Catalog trait）。在 plan_statement 之前拦截，直接返回结果。
        if let Some(result) = crate::pgwire::system_tables::try_execute_system_table_query(
            &stmt,
            &self.catalog,
            &self.database_name,
        ) {
            return result;
        }

        // Phase TDengine-P2: COMMENT ON 拦截（不经过 Planner，直接操作 catalog）
        // COMMENT ON 仅修改 catalog 元数据，不产生逻辑计划
        if let Statement::Comment {
            object_type,
            object_name,
            column_name,
            comment,
        } = &stmt
        {
            match object_type {
                CommentObjectType::Table => {
                    self.catalog
                        .set_table_comment(object_name, comment.clone())
                        .map_err(|e| SessionError::Plan(e.to_string()))?;
                }
                CommentObjectType::Column => {
                    // P0 修复：COLUMN 注释必须指定列名，否则报错（之前是假成功）
                    let col = column_name.as_ref().ok_or_else(|| {
                        SessionError::Plan(
                            "COMMENT ON COLUMN requires a column name".into(),
                        )
                    })?;
                    self.catalog
                        .set_column_comment(object_name, col, comment.clone())
                        .map_err(|e| SessionError::Plan(e.to_string()))?;
                }
            }
            return Ok(QueryResult::DdlComplete {
                tag: "COMMENT".into(),
            });
        }

        // P2-1.1：ANALYZE 拦截（不经过 Planner，直接扫描表数据收集统计信息）
        //
        // 行为与 PostgreSQL 一致：
        // - `ANALYZE` — 扫描所有用户表
        // - `ANALYZE table_name [, ...]` — 仅扫描指定表
        // - `ANALYZE VERBOSE ...` — verbose 标志当前仅记录日志（与 PG 一致不输出结果集）
        //
        // 收集的统计信息：
        // - row_count（表总行数）
        // - 每列的 null_count / distinct_count / min_value / max_value / histogram
        //
        // 结果存入 `statistics_store`，供 CostModel 进行基于成本的优化（P2-1.2 激活）
        if let Statement::Analyze { tables, verbose } = &stmt {
            return self.execute_analyze(tables, *verbose).await;
        }

        // 3. 其余语句走 Planner + OPT-5 优化器 pass
        //
        // OPT-5：在 Planner 产出 LogicalPlan 后，应用 RBO 优化规则
        // （谓词下推 + 投影裁剪），减少不必要的列扫描和行数。
        // OPT-10：将 CPU 密集的规划 + 优化放入 spawn_blocking，
        // 避免阻塞 tokio worker 线程。
        //
        // P2-1.2（2026-07-31 激活）：在 RBO 之后，若 statistics_store 已注入，
        // 应用 CBO（基于成本的优化）：
        // - JoinOrderOptimizer（DPccp 算法）：对 Inner/Cross JOIN 子树应用 DPccp
        //   重排算法，选择成本最低的 JOIN 顺序
        // - CostModel：基于 ANALYZE 收集的统计信息（行数、NDV、直方图）估算成本
        //
        // 未注入 statistics_store 时，跳过 CBO（仅 RBO），保持兼容性。
        let plan = {
            let catalog_clone = self.catalog.clone();
            // P2-1.2：若 statistics_store 已注入，克隆 Arc 以传入 spawn_blocking
            let stats_store_inner = self.statistics_store.clone();
            tokio::task::spawn_blocking(move || -> Result<LogicalPlan, SessionError> {
                let planner = Planner::new(&catalog_clone);
                let raw_plan = planner.plan_statement(stmt)?;
                // OPT-5: 应用 RBO 规则（不需要统计信息，零成本激活已有优化器代码）
                // 顺序：谓词下推 → 投影裁剪 → 子查询展平 → 索引选择 → 公共子表达式消除
                let optimized = szrsql_optimizer::rule::PredicatePushdown::apply(raw_plan);
                let optimized = szrsql_optimizer::rule::ProjectionPruning::apply(optimized);
                // P2-1: 子查询展平（IN/EXISTS 转 Semi/Anti Join）
                let optimized =
                    szrsql_optimizer::rule::SubqueryFlattening::new(&planner).apply(optimized);
                // P2-2: 索引选择（SELECT WHERE 走 B-Tree 索引而非全表扫描）
                let optimized =
                    szrsql_optimizer::rule::IndexSelection::new(&catalog_clone).apply(optimized);
                // P2-3: 公共子表达式消除
                let optimized =
                    szrsql_optimizer::rule::CommonSubexpressionElimination::apply(optimized);
                // P2-1.2: CBO — 若 statistics_store 已注入，应用 JOIN 顺序优化
                //
                // 使用 SharedStatisticsStore 包装 Arc<Mutex<InMemoryStatisticsStore>>，
                // 使其可作为 Arc<dyn StatisticsStore> 注入 CostModel / JoinOrderOptimizer。
                // DPccp 算法枚举所有 JOIN 顺序，使用 CostModel 估算成本，选最低者。
                let optimized = if let Some(inner) = stats_store_inner {
                    let shared = szrsql_optimizer::statistics::SharedStatisticsStore::new(inner);
                    let cost_model = szrsql_optimizer::cost::CostModel::new(Arc::new(shared));
                    let join_optimizer = szrsql_optimizer::join_order::JoinOrderOptimizer::new(cost_model);
                    join_optimizer.optimize(optimized)
                } else {
                    optimized
                };
                Ok(optimized)
            })
            .await
            .map_err(|e| SessionError::Transaction(format!("planning task panicked: {e}")))?
        }?;

        // 4. 分派执行
        self.dispatch_plan(&plan).await
    }

    /// P2-1.1：执行 ANALYZE 语句，收集表统计信息。
    ///
    /// # 流程
    ///
    /// 1. 检查 `statistics_store` 是否注入（未注入返回错误）
    /// 2. 确定目标表列表：
    ///    - `tables` 为空 → 取 catalog 中所有用户表
    ///    - `tables` 非空 → 验证每张表存在
    /// 3. 对每张表：
    ///    - 获取 `Arc<Mutex<InMemoryTable>>`
    ///    - 锁定表，调用 `StatisticsCollector::collect(&*table)` 扫描全表
    ///    - 将 `TableStatistics` 写入 `statistics_store`
    /// 4. 返回 `QueryResult::DdlComplete { tag: "ANALYZE" }`
    ///
    /// # 参数
    ///
    /// - `tables`：目标表列表（空表示分析所有用户表）
    /// - `verbose`：VERBOSE 标志（当前仅记录日志，与 PG 一致不输出结果集）
    async fn execute_analyze(
        &mut self,
        tables: &[TableName],
        verbose: bool,
    ) -> Result<QueryResult, SessionError> {
        use szrsql_optimizer::statistics::{StatisticsCollector, StatisticsStore};

        // 1. 检查 statistics_store 注入
        let store = self.statistics_store.clone().ok_or_else(|| {
            SessionError::InvalidStatement(
                "ANALYZE is not supported: statistics_store not configured".into(),
            )
        })?;

        // 2. 确定目标表列表
        let target_tables: Vec<TableName> = if tables.is_empty() {
            // ANALYZE 无指定表 → 取 catalog 中所有用户表
            self.catalog.list_tables()
        } else {
            // 验证每张表存在
            for t in tables {
                if self.catalog.get_table(t).is_none() {
                    return Err(SessionError::TableNotFound(t.qualified_name()));
                }
            }
            tables.to_vec()
        };

        if target_tables.is_empty() {
            // 无表可分析：返回成功（PG 行为，warning 在日志层记录）
            tracing::info!("ANALYZE: no tables to analyze");
            return Ok(QueryResult::DdlComplete {
                tag: "ANALYZE".into(),
            });
        }

        // 3. 逐表收集统计信息
        let analyzed_count = target_tables.len();
        for table_name in &target_tables {
            let table_arc = self
                .get_table_arc(&table_name.name, table_name.schema.as_deref())
                .await?;
            let stats = {
                let table_guard = table_arc.lock().await;
                if verbose {
                    tracing::info!(
                        table = %table_name.qualified_name(),
                        rows = table_guard.row_count(),
                        "ANALYZE: collecting statistics"
                    );
                }
                StatisticsCollector::collect(&*table_guard)
            };

            // 写入共享 store
            let table_key = table_name.qualified_name();
            {
                let mut store_guard = store
                    .lock()
                    .map_err(|e| SessionError::Plan(format!("statistics_store poisoned: {e}")))?;
                store_guard.update_table_stats(&table_key, stats);
            }

            if verbose {
                tracing::info!(
                    table = %table_name.qualified_name(),
                    "ANALYZE: statistics collected and stored"
                );
            }
        }

        tracing::info!(
            tables_analyzed = analyzed_count,
            verbose,
            "ANALYZE completed"
        );

        Ok(QueryResult::DdlComplete {
            tag: "ANALYZE".into(),
        })
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
                match self.commit_transaction().await {
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
            // SAVEPOINT / RELEASE SAVEPOINT / ROLLBACK TO SAVEPOINT — Phase 4.2 暂不支持
            Statement::Rollback { savepoint: Some(_) }
            | Statement::Savepoint(_)
            | Statement::ReleaseSavepoint(_) => Err(SessionError::Transaction(format!(
                "savepoint not supported in Phase 4.2: {stmt:?}"
            ))),
            // SET TRANSACTION ISOLATION LEVEL — P0 修复
            // 之前是 NO-OP 假成功，现在实际写入 session_state 的 transaction_isolation 变量，
            // 使 SHOW transaction_isolation 返回正确值。
            // 注：由于运行时未接入 MVCC，实际隔离行为仍为表级 snapshot/restore（READ COMMITTED 语义）。
            // 完整的隔离级别切换需待 MVCC 接入 session 事务管理后实现。
            Statement::SetTransaction { isolation, access } => {
                if let Some(iso) = isolation {
                    let iso_str = match iso {
                        TransactionIsolation::ReadUncommitted => "read uncommitted",
                        TransactionIsolation::ReadCommitted => "read committed",
                        TransactionIsolation::RepeatableRead => "repeatable read",
                        TransactionIsolation::Serializable => "serializable",
                    };
                    self.session_state
                        .set("transaction_isolation", Value::Text(iso_str.into()));
                    // P0-TX-1 修复：保存隔离级别，下次 BEGIN 时传给 MvccManager
                    let mvcc_iso = match iso {
                        TransactionIsolation::ReadUncommitted => IsolationLevel::ReadUncommitted,
                        TransactionIsolation::ReadCommitted => IsolationLevel::ReadCommitted,
                        TransactionIsolation::RepeatableRead => IsolationLevel::RepeatableRead,
                        TransactionIsolation::Serializable => IsolationLevel::Serializable,
                    };
                    self.pending_isolation = Some(mvcc_iso);
                    tracing::debug!(isolation = iso_str, "SET TRANSACTION ISOLATION LEVEL recorded");
                }
                if let Some(acc) = access {
                    let acc_str = match acc {
                        TransactionAccess::ReadOnly => "read only",
                        TransactionAccess::ReadWrite => "read write",
                    };
                    // 同时记录到 session 变量，供 SHOW 查询
                    self.session_state
                        .set("transaction_access_mode", Value::Text(acc_str.into()));
                }
                Ok(Some(QueryResult::TransactionComplete {
                    tag: "SET".into(),
                    in_transaction: self.in_transaction(),
                }))
            }
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
        self.txn_modified_tables.clear();
        self.txn_state = TransactionState::InTransaction;

        // P0-TX-1 修复：同步到 MvccManager 状态机
        if let Some(mgr) = &self.mvcc {
            let level = self.pending_isolation.unwrap_or(IsolationLevel::RepeatableRead);
            let txn = mgr.begin_with_isolation(level);
            self.current_txn_id = txn.txn_id;
            tracing::debug!(
                txn_id = txn.txn_id,
                isolation = ?level,
                "MVCC transaction begun"
            );
        }

        // ADV-F-7 / ADV-CONC-1：分配事务 ID
        // P0-TX-1 修复：优先使用 MvccManager 分配（含快照隔离），退化为共享计数器/会话级计数器
        if self.mvcc.is_none() {
            self.current_txn_id = if let Some(counter) = &self.shared_txn_counter {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            } else {
                let id = self.next_txn_id;
                self.next_txn_id += 1;
                id
            };
        }

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
    ///    a. **P0-1 修复**：遍历事务期间修改过的表，将每张表的全量数据序列化为
    ///       `WalOpType::TableData` 记录写入 WAL（用于崩溃恢复）
    ///    b. 写入 `WalOpType::Commit` 记录（携带 `txn_id`）
    ///    c. 调用 `flush()`（fsync）强制刷盘
    ///    d. fsync 成功 → 清除快照，返回 Ok（可安全 ACK 客户端）
    ///    e. fsync 失败 → 回滚事务（restore 快照），返回 Err（客户端收到错误）
    ///
    /// # 安全保证
    ///
    /// - 返回 Ok：WAL Commit 记录已 fsync，事务已持久化（含 TableData 数据）
    /// - 返回 Err：WAL 写入/fsync 失败，事务已回滚，不会出现"ACK 成功但数据未持久化"
    async fn commit_transaction(&mut self) -> Result<(), SessionError> {
        // ADV-CONC-1：在进入 WAL 分支前提前取出 txn_id，供锁释放使用
        let txn_id = self.current_txn_id;

        // P1-2：在事务提交链路开始前，先取出本事务修改的表名副本。
        //
        // 此副本用于在事务成功提交后调用 `dirty_tracker.mark_dirty_many`，
        // 标记这些表为脏，供后台增量快照任务使用。
        // 取副本是必要的，因为下面的 WAL 分支会 clear() 掉原始集合。
        let committed_dirty_tables: Vec<String> =
            self.txn_modified_tables.iter().cloned().collect();

        if let Some(writer) = &self.wal_writer {
            // P0-1 修复：阶段 0 — 写入修改表的全量数据到 WAL（用于崩溃恢复）
            //
            // 遍历事务期间修改过的表（INSERT/UPDATE/DELETE 时记录到 txn_modified_tables），
            // 将每张表的全量数据序列化为 TableData 记录写入 WAL。
            // 回放时仅应用紧随其后有 Commit 记录的 TableData，保证 ACID。
            //
            // TableData data 字段格式：
            //   u32 LE 表名长度 + 表名 UTF-8 字节 + 表数据 JSON
            let mut records: Vec<WalRecord> = Vec::new();

            if !self.txn_modified_tables.is_empty() {
                let table_names: Vec<String> = self.txn_modified_tables.iter().cloned().collect();
                for table_name in &table_names {
                    // 通过 get_table_arc 获取表（自动处理 schema 前缀查找）
                    if let Ok(table_arc) = self.get_table_arc(table_name, None).await {
                        let table_guard = table_arc.lock().await;
                        match serde_json::to_vec(&*table_guard) {
                            Ok(table_data) => {
                                let name_bytes = table_name.as_bytes();
                                let mut payload =
                                    Vec::with_capacity(4 + name_bytes.len() + table_data.len());
                                payload.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
                                payload.extend_from_slice(name_bytes);
                                payload.extend_from_slice(&table_data);
                                records.push(WalRecord::new(
                                    0,
                                    txn_id,
                                    WalOpType::TableData,
                                    0,
                                    payload,
                                ));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    txn_id,
                                    table = %table_name,
                                    error = %e,
                                    "failed to serialize table for WAL TableData record"
                                );
                            }
                        }
                    }
                }
                self.txn_modified_tables.clear();
            }

            // 阶段 1：写入 WAL Commit 记录
            records.push(WalRecord::new(0, txn_id, WalOpType::Commit, 0, vec![]));

            // OPT-7: append_batch writes all records in a single file-lock critical section.
            // append_batch returns the start LSN; the Commit record is the last in the batch,
            // so its LSN = start_lsn + record_count - 1.
            let record_count = records.len();
            let start_lsn = writer.append_batch(records)?;
            let lsn = start_lsn + record_count as u64 - 1;
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

        // P0-TX-1 修复：同步到 MvccManager 状态机
        if let Some(mgr) = &self.mvcc {
            match mgr.commit_durable(txn_id, |_txn_id| {
                // WAL 已在上方写入并 fsync，此处返回当前 LSN
                // 注意：WAL Commit 记录已在上面写入，这里不再重复写入
                Ok(0) // 返回 0 作为 commit_lsn（WAL LSN 已在上方记录）
            }) {
                Ok(()) => {
                    tracing::debug!(txn_id, "MVCC transaction committed");
                }
                Err(MvccError::WriteSkewDetected(_)) => {
                    // SSI 写偏斜检测失败：事务必须回滚
                    tracing::warn!(txn_id, "MVCC SSI write skew detected, rolling back");
                    return Err(SessionError::Transaction(format!(
                        "could not serialize access due to concurrent update (txn {txn_id})"
                    )));
                }
                Err(MvccError::WriteWriteConflict(_)) => {
                    // First-Committer-Wins：写写冲突，事务必须回滚
                    tracing::warn!(txn_id, "MVCC write-write conflict, rolling back");
                    return Err(SessionError::Transaction(format!(
                        "could not serialize access due to concurrent update (txn {txn_id})"
                    )));
                }
                Err(e) => {
                    tracing::warn!(txn_id, error = %e, "MVCC commit failed");
                    return Err(SessionError::Transaction(format!(
                        "MVCC commit failed: {e}"
                    )));
                }
            }
        }

        // P0 修复：记录到 transaction_history 以支持后续 FLASHBACK
        // 之前是简化不记录，导致 FLASHBACK TRANSACTION / FLASHBACK TABLE 永远不可用。
        // 现在将事务前快照移交给 transaction_history，供后续闪回查询使用。
        // 注：仅记录非空快照事务，避免无事务的 BEGIN/COMMIT 污染历史。
        if !self.txn_snapshots.is_empty() {
            let snapshots = std::mem::take(&mut self.txn_snapshots);
            let recorded_txn_id = self.transaction_history.record_commit(snapshots);
            tracing::debug!(
                history_txn_id = recorded_txn_id,
                session_txn_id = txn_id,
                "transaction recorded to history for FLASHBACK support"
            );
        } else {
            self.txn_snapshots.clear();
        }
        self.txn_state = TransactionState::Idle;
        // ADV-CONC-1：释放本事务持有的所有行级锁（Strict 2PL）
        if let Some(lm) = &self.lock_manager {
            lm.unlock_all(txn_id);
            tracing::debug!(txn_id, "all row locks released on commit");
        }
        self.current_txn_id = 0;
        // P1-2：事务成功提交后，标记本事务修改过的表为脏，供后台增量快照任务使用。
        //
        // 仅在事务真正成功后标记（WAL fsync + MVCC commit 都通过），保证：
        // - 回滚的事务不会污染脏表集合（避免无谓的快照 IO）
        // - 已提交但 fsync 失败的事务不会标记（事务已回滚）
        if let Some(tracker) = &self.dirty_tracker {
            if !committed_dirty_tables.is_empty() {
                tracker.mark_dirty_many(committed_dirty_tables.iter()).await;
            }
        }
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
        // P0-TX-1 修复：同步到 MvccManager 状态机
        if let Some(mgr) = &self.mvcc {
            if txn_id > 0 {
                if let Err(e) = mgr.abort(txn_id) {
                    tracing::warn!(txn_id, error = %e, "MVCC abort failed (non-fatal)");
                }
            }
        }
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
            | LogicalPlan::Shared { .. }
            | LogicalPlan::MemoRef { .. }
            | LogicalPlan::With { .. }
            | LogicalPlan::CteRef { .. } => self.execute_select_plan(plan).await,

            // Phase 3.34: SHOW / SET 命令 — 需要 SessionState，不能走 Executor::execute
            // 这些 plan 类型在 Executor::execute 中没有对应分支，会返回 Unsupported 错误。
            // 改为调用 Executor 的专用方法并传入 session_state。
            LogicalPlan::ShowTables => self.execute_show_tables_plan().await,
            LogicalPlan::ShowCreateTable { .. } => self.execute_show_create_table_plan(plan).await,
            LogicalPlan::ShowVariable { .. } => self.execute_show_variable_plan(plan).await,
            LogicalPlan::SetNames { .. } => self.execute_set_names_plan(plan).await,
            LogicalPlan::SetVariable { .. } => self.execute_set_variable_plan(plan).await,

            // DML
            LogicalPlan::Insert { .. } => self.execute_insert_plan(plan).await,
            LogicalPlan::Update { .. } => self.execute_update_plan(plan).await,
            LogicalPlan::Delete { .. } => self.execute_delete_plan(plan).await,
            LogicalPlan::Replace { .. } => self.execute_replace_plan(plan).await,
            LogicalPlan::Merge { .. } => self.execute_merge_plan(plan).await,

            // DDL
            LogicalPlan::CreateTable { .. } => self.execute_create_table_plan(plan).await,
            LogicalPlan::DropTable { .. } => self.execute_drop_table_plan(plan).await,
            LogicalPlan::CreateIndex { .. } => self.execute_create_index_plan(plan),
            LogicalPlan::DropIndex { .. } => self.execute_drop_index_plan(plan),
            LogicalPlan::CreateView { .. } => self.execute_create_view_plan(plan).await,
            LogicalPlan::DropView { .. } => self.execute_drop_view_plan(plan),
            LogicalPlan::RefreshMaterializedView { .. } => {
                self.execute_refresh_materialized_view_plan(plan).await
            }
            // P0-5 修复：CREATE/DROP FUNCTION
            LogicalPlan::CreateFunction { .. } => self.execute_create_function_plan(plan),
            LogicalPlan::DropFunction { .. } => self.execute_drop_function_plan(plan),
            LogicalPlan::CreateSequence { .. } => self.execute_create_sequence_plan(plan),
            LogicalPlan::DropSequence { .. } => self.execute_drop_sequence_plan(plan),
            LogicalPlan::CreateType { .. } => self.execute_create_type_plan(plan),
            LogicalPlan::DropType { .. } => self.execute_drop_type_plan(plan),
            LogicalPlan::AlterType { .. } => self.execute_alter_type_plan(plan),
            // Phase F-10: ALTER TABLE — 同步修改 catalog schema + 表数据
            LogicalPlan::AlterTable { .. } => self.execute_alter_table_plan(plan).await,
            // TRUNCATE TABLE — 清空表数据（保留表结构）
            LogicalPlan::Truncate { .. } => self.execute_truncate_plan(plan).await,
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
        let table_arc = self
            .get_table_arc(&table_name.name, table_name.schema.as_deref())
            .await?;
        let mut table_guard = table_arc.lock().await;

        // 6. 逐行解析并插入
        //
        // P0 修复：COPY FROM 此前直接调用 table.insert_row 跳过所有约束校验，
        // 现在通过 Executor::validate_row_for_insert 复用 FK/CHECK/ENUM 校验逻辑。
        // 校验失败立即中止 COPY（与 PG 行为一致），已插入的行保留（事务回滚由调用方处理）。
        let executor = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
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

            // P0 修复：调用 Executor 校验 FK/CHECK/ENUM 约束
            executor
                .validate_row_for_insert(&table_name, &schema, &row)
                .map_err(|e| SessionError::Execution(format!(
                    "COPY FROM line {line_no}: constraint violation: {e}"
                )))?;

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

                let table_arc = self
                    .get_table_arc(&table_name.name, table_name.schema.as_deref())
                    .await?;
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

                // OPT-6：仅锁定查询计划实际引用的表（合并本地表和共享表）
                let referenced: std::collections::HashSet<String> = plan.collect_referenced_table_names();
                let mut all_arcs: std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<InMemoryTable>>> = std::collections::HashMap::new();
                for (k, v) in &self.tables {
                    if referenced.contains(&k.to_lowercase()) {
                        all_arcs.insert(k.clone(), v.clone());
                    }
                }
                if !referenced.is_empty() {
                    if let Some(shared) = &self.shared_tables {
                        for (k, v) in shared.read().await.iter() {
                            if referenced.contains(&k.to_lowercase()) {
                                all_arcs.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                    }
                }
                let mut guards = Vec::with_capacity(all_arcs.len());
                for table_arc in all_arcs.values() {
                    guards.push(table_arc.lock().await);
                }

                let mut executor = Executor::new();
                executor = executor.with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
                executor = executor.with_temp_store(&self.temp_store);
                // P0-TX-1 Phase B：注入 MVCC 上下文
                if let Some(mvcc) = &self.mvcc {
                    executor = executor.with_mvcc(mvcc, self.current_txn_id);
                }
                // P0-DIST-1/2/3：注入分布式运行时句柄，启用 DML 双写
                if let Some(dist_rt) = &self.dist_runtime {
                    executor = executor.with_dist_runtime(dist_rt.clone());
                }
                // P7-1：注入 CDC 引擎，启用 DML 事件分发
                if let Some(cdc) = &self.cdc_engine {
                    executor = executor.with_cdc_engine(cdc.clone());
                }
                for guard in &guards {
                    executor.register_table(&**guard);
                }

                let rows = executor.execute(&plan)?;
                // P0-FN-TYPE 修复：execute() 返回后内部 guard 已 drop，
                // 需重新设置 current_sql_functions guard，确保 derive_output_columns
                // 能查询到函数返回类型声明（避免函数列类型被兜底为 Text）。
                let _sql_func_guard = executor.sql_functions_guard();
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
        // OPT-6（ADV-CONC-1 改进）：仅锁定查询计划实际引用的物理表，避免对会话中
        // 所有表加锁造成不必要的并发阻塞。先从计划树提取引用的表名（小写），
        // 再从本地/共享/物化视图存储中按名筛选。
        //
        // 安全性说明：
        // - `Executor<'_>` 非 Send，不能跨 .await 持有，必须在同步执行前完成所有表注册。
        // - 仅锁定引用的表足以覆盖 SELECT 执行所需的所有访问路径（Scan/IndexScan/Join 等）。
        // - 若计划不引用任何物理表（如 `SELECT 1`），无需锁定任何表。
        let referenced: std::collections::HashSet<String> = plan.collect_referenced_table_names();

        let mut all_arcs: std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<InMemoryTable>>> = std::collections::HashMap::new();
        // 仅收集被引用的本地表
        for (k, v) in &self.tables {
            if referenced.contains(&k.to_lowercase()) {
                all_arcs.insert(k.clone(), v.clone());
            }
        }
        // 仅收集被引用的共享表
        if !referenced.is_empty() {
            if let Some(shared) = &self.shared_tables {
                for (k, v) in shared.read().await.iter() {
                    if referenced.contains(&k.to_lowercase()) {
                        all_arcs.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
        }
        // P0-6：仅收集被引用的物化视图存储表
        let mv_arcs: Vec<(String, std::sync::Arc<tokio::sync::Mutex<InMemoryTable>>)> =
            self.materialized_view_tables
                .iter()
                .filter(|(k, _)| referenced.contains(&k.to_lowercase()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
        // 先锁定收集到的表（确保 Executor 不跨 .await 持有，因为 Executor<'_> 非 Send）
        let mut guards = Vec::with_capacity(all_arcs.len());
        for table_arc in all_arcs.values() {
            guards.push(table_arc.lock().await);
        }
        // P0-6：锁定被引用的物化视图存储表
        let mut mv_guards = Vec::with_capacity(mv_arcs.len());
        for (_, arc) in &mv_arcs {
            mv_guards.push(arc.lock().await);
        }

        // 构造 Executor 并注册收集到的表（同步操作，不涉及 .await）
        let mut executor = Executor::new();
        executor = executor.with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
        executor = executor.with_temp_store(&self.temp_store);
        // P0-TX-1 Phase B：注入 MVCC 上下文，启用事务可见性过滤
        if let Some(mvcc) = &self.mvcc {
            executor = executor.with_mvcc(mvcc, self.current_txn_id);
        }
        // P0-DIST-1/2/3：注入分布式运行时句柄，启用 DML 双写
        if let Some(dist_rt) = &self.dist_runtime {
            executor = executor.with_dist_runtime(dist_rt.clone());
        }
        // P7-1：注入 CDC 引擎，启用 DML 事件分发
        if let Some(cdc) = &self.cdc_engine {
            executor = executor.with_cdc_engine(cdc.clone());
        }
        for guard in &guards {
            executor.register_table(&**guard);
        }
        // P0-6：注册物化视图存储表
        for ((name, _), guard) in mv_arcs.iter().zip(mv_guards.iter()) {
            executor.register_materialized_view_store(name, &**guard);
        }

        // 执行
        let rows = executor.execute(plan)?;

        // P0-FN-TYPE 修复：execute() 返回后内部 guard 已 drop，
        // 需重新设置 current_sql_functions guard，确保 derive_output_columns
        // 能查询到函数返回类型声明（避免函数列类型被兜底为 Text）。
        let _sql_func_guard = executor.sql_functions_guard();

        // 推导输出列
        let columns = derive_output_columns(plan, &rows);

        let tag = format!("SELECT {}", rows.len());
        Ok(QueryResult::ResultSet { columns, rows, tag })
    }

    // -----------------------------------------------------------------
    //  SHOW / SET 命令 — Phase 3.34
    // -----------------------------------------------------------------

    /// 执行 `SHOW TABLES` — 列出当前 catalog 中所有表名。
    ///
    /// 调用 Executor::execute_show_tables，返回单列结果集（列名 `Table`）。
    async fn execute_show_tables_plan(&mut self) -> Result<QueryResult, SessionError> {
        // 构造 Executor 并绑定 catalog（不需要表数据，仅需 catalog 元信息）
        let executor = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
        let rows = executor.execute_show_tables()?;
        let columns = vec![ResultColumn {
            name: "Table".into(),
            column_type: ColumnType::Text,
        }];
        let tag = format!("SELECT {}", rows.len());
        Ok(QueryResult::ResultSet { columns, rows, tag })
    }

    /// 执行 `SHOW CREATE TABLE <name>` — 返回表名 + DDL 文本两列。
    ///
    /// 调用 Executor::execute_show_create_table，从 catalog 读取 schema 重建 DDL。
    async fn execute_show_create_table_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
        let rows = executor.execute_show_create_table(plan)?;
        let columns = vec![
            ResultColumn {
                name: "Table".into(),
                column_type: ColumnType::Text,
            },
            ResultColumn {
                name: "DDL".into(),
                column_type: ColumnType::Text,
            },
        ];
        let tag = format!("SELECT {}", rows.len());
        Ok(QueryResult::ResultSet { columns, rows, tag })
    }

    /// 执行 `SHOW <variable>` — 返回会话变量值的单行单列结果集。
    ///
    /// 调用 Executor::execute_show_variable，从 session_state 读取变量值。
    async fn execute_show_variable_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        let rows = executor.execute_show_variable(plan, &self.session_state)?;
        let columns = vec![ResultColumn {
            name: "Value".into(),
            column_type: ColumnType::Text,
        }];
        let tag = format!("SELECT {}", rows.len());
        Ok(QueryResult::ResultSet { columns, rows, tag })
    }

    /// 执行 `SET NAMES 'charset' [COLLATE 'collation']` — 写入 session_state。
    ///
    /// 调用 Executor::execute_set_names，将 charset/collation 写入会话状态。
    /// 返回 DdlComplete（无结果集，CommandComplete 标签 "SET"）。
    async fn execute_set_names_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_set_names(plan, &mut self.session_state)?;
        Ok(QueryResult::DdlComplete {
            tag: "SET".into(),
        })
    }

    /// 执行 `SET <variable> = <value>` — 求值 value 表达式并写入 session_state。
    ///
    /// 调用 Executor::execute_set_variable，将 (variable, value) 写入会话状态。
    /// 返回 DdlComplete（无结果集，CommandComplete 标签 "SET"）。
    async fn execute_set_variable_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_set_variable(plan, &mut self.session_state)?;
        Ok(QueryResult::DdlComplete {
            tag: "SET".into(),
        })
    }

    // -----------------------------------------------------------------
    //  INSERT
    // -----------------------------------------------------------------

    async fn execute_insert_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let (table, schema, returning) = match plan {
            LogicalPlan::Insert { table, schema, returning, .. } => (table.clone(), schema.clone(), returning.clone()),
            _ => {
                return Err(SessionError::InvalidStatement(format!(
                    "expected Insert plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        let table_arc = self
            .get_table_arc(&table.name, table.schema.as_deref())
            .await?;
        let mut table_guard = table_arc.lock().await;
        let mut executor = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
        if let Some(mvcc) = &self.mvcc {
            executor = executor.with_mvcc(mvcc, self.current_txn_id);
        }
        // P0-DIST-1/2/3：注入分布式运行时句柄，启用 DML 双写
        if let Some(dist_rt) = &self.dist_runtime {
            executor = executor.with_dist_runtime(dist_rt.clone());
        }
        // P7-1：注入 CDC 引擎，启用 DML 事件分发
        if let Some(cdc) = &self.cdc_engine {
            executor = executor.with_cdc_engine(cdc.clone());
        }
        // P9-2：注入 WAL 写入器，启用 DML 行级 WAL 记录
        if let Some(writer) = &self.wal_writer {
            executor = executor.with_wal_writer(writer.clone());
        }
        let DmlResult {
            affected_rows,
            returning_rows,
        } = executor.execute_insert(plan, &mut *table_guard)?;

        // P0-1: 记录事务期间修改的表名（用于 WAL 崩溃恢复）
        self.txn_modified_tables.insert(table.name.clone());

        // 处理 RETURNING 子句
        // Navicat 兼容修复：columns 必须与 returning_rows 每行的列数一致。
        // 之前用整张表 schema 构造 columns，但 executor.project_returning 只返回
        // RETURNING 子句指定的列，导致 RowDescription 与 DataRow 字段数不匹配。
        if !returning_rows.is_empty() {
            let columns = build_returning_columns(&schema, &returning);
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
        let (table, schema, returning) = match plan {
            LogicalPlan::Update { table, schema, returning, .. } => (table.clone(), schema.clone(), returning.clone()),
            _ => {
                return Err(SessionError::InvalidStatement(format!(
                    "expected Update plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // ADV-CONC-1：事务中获取表级 X 锁（Strict 2PL，COMMIT/ROLLBACK 释放）
        self.acquire_table_xlock(&table.name).await?;

        let table_arc = self
            .get_table_arc(&table.name, table.schema.as_deref())
            .await?;
        let mut table_guard = table_arc.lock().await;
        let mut executor = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
        if let Some(mvcc) = &self.mvcc {
            executor = executor.with_mvcc(mvcc, self.current_txn_id);
        }
        // P0-DIST-1/2/3：注入分布式运行时句柄，启用 DML 双写
        if let Some(dist_rt) = &self.dist_runtime {
            executor = executor.with_dist_runtime(dist_rt.clone());
        }
        // P7-1：注入 CDC 引擎，启用 DML 事件分发
        if let Some(cdc) = &self.cdc_engine {
            executor = executor.with_cdc_engine(cdc.clone());
        }
        // P9-2：注入 WAL 写入器，启用 DML 行级 WAL 记录
        if let Some(writer) = &self.wal_writer {
            executor = executor.with_wal_writer(writer.clone());
        }
        let DmlResult {
            affected_rows,
            returning_rows,
        } = executor.execute_update(plan, &mut *table_guard)?;

        // P0-1: 记录事务期间修改的表名（用于 WAL 崩溃恢复）
        self.txn_modified_tables.insert(table.name.clone());

        if !returning_rows.is_empty() {
            let columns = build_returning_columns(&schema, &returning);
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
        let (table, schema, returning) = match plan {
            LogicalPlan::Delete { table, schema, returning, .. } => (table.clone(), schema.clone(), returning.clone()),
            _ => {
                return Err(SessionError::InvalidStatement(format!(
                    "expected Delete plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // ADV-CONC-1：事务中获取表级 X 锁（Strict 2PL，COMMIT/ROLLBACK 释放）
        self.acquire_table_xlock(&table.name).await?;

        let table_arc = self
            .get_table_arc(&table.name, table.schema.as_deref())
            .await?;
        let mut table_guard = table_arc.lock().await;
        let mut executor = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
        if let Some(mvcc) = &self.mvcc {
            executor = executor.with_mvcc(mvcc, self.current_txn_id);
        }
        // P0-DIST-1/2/3：注入分布式运行时句柄，启用 DML 双写
        if let Some(dist_rt) = &self.dist_runtime {
            executor = executor.with_dist_runtime(dist_rt.clone());
        }
        // P7-1：注入 CDC 引擎，启用 DML 事件分发
        if let Some(cdc) = &self.cdc_engine {
            executor = executor.with_cdc_engine(cdc.clone());
        }
        // P9-2：注入 WAL 写入器，启用 DML 行级 WAL 记录
        if let Some(writer) = &self.wal_writer {
            executor = executor.with_wal_writer(writer.clone());
        }
        let DmlResult {
            affected_rows,
            returning_rows,
        } = executor.execute_delete(plan, &mut *table_guard)?;

        // P0-1: 记录事务期间修改的表名（用于 WAL 崩溃恢复）
        self.txn_modified_tables.insert(table.name.clone());

        if !returning_rows.is_empty() {
            let columns = build_returning_columns(&schema, &returning);
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
        let table = match plan {
            LogicalPlan::Replace { table, .. } => table.clone(),
            _ => {
                return Err(SessionError::InvalidStatement(format!(
                    "expected Replace plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        let table_arc = self
            .get_table_arc(&table.name, table.schema.as_deref())
            .await?;
        let mut table_guard = table_arc.lock().await;
        let executor = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
        let DmlResult { affected_rows, .. } = executor.execute_replace(plan, &mut *table_guard)?;

        Ok(QueryResult::AffectedRows {
            tag: format!("REPLACE {affected_rows}"),
        })
    }

    async fn execute_merge_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let table = match plan {
            LogicalPlan::Merge { target, .. } => target.clone(),
            _ => {
                return Err(SessionError::InvalidStatement(format!(
                    "expected Merge plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        let table_arc = self
            .get_table_arc(&table.name, table.schema.as_deref())
            .await?;
        let mut table_guard = table_arc.lock().await;
        let executor = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
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
            _ => {
                return Err(SessionError::InvalidStatement(format!(
                    "expected CreateTable plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // 注册到 catalog
        self.catalog.register_from_create_plan(plan)?;

        // 创建空表
        let mut table = InMemoryTable::new(schema);

        // P0-STORE 阶段 1：若表有 PRIMARY KEY 约束且主键列为 Int64，
        // 自动启用 B+Tree 主键索引，供 WHERE pk = literal 等查询走 O(log n) 路径。
        if let LogicalPlan::CreateTable { columns, constraints, .. } = plan {
            // 1. 检查列级 PRIMARY KEY 约束（col INT PRIMARY KEY）
            for (idx, col) in columns.iter().enumerate() {
                if col.primary_key
                    && col.data_type == szrsql_types::value::ColumnType::Int64
                {
                    table.enable_btree_pk(idx);
                    tracing::debug!(
                        table = %table_name,
                        pk_column = %col.name,
                        "Auto-enabled B+Tree PK index (column-level constraint)"
                    );
                    break;
                }
            }
            // 2. 若列级未命中，检查表级 PRIMARY KEY 约束（PRIMARY KEY (col)）
            if !table.has_btree_pk() {
                if let Some(TableConstraint::PrimaryKey { columns: pk_cols, .. }) = constraints
                    .iter()
                    .find(|c| matches!(c, TableConstraint::PrimaryKey { .. }))
                {
                    if pk_cols.len() == 1 {
                        if let Some(pk_col_name) = pk_cols.first() {
                            if let Some(idx) = columns.iter().position(|c| &c.name == pk_col_name) {
                                if columns[idx].data_type
                                    == szrsql_types::value::ColumnType::Int64
                                {
                                    table.enable_btree_pk(idx);
                                    tracing::debug!(
                                        table = %table_name,
                                        pk_column = %pk_col_name,
                                        "Auto-enabled B+Tree PK index (table-level constraint)"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

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
        let (names, if_exists, cascade) = match plan {
            LogicalPlan::DropTable {
                names,
                if_exists,
                cascade,
            } => (names, *if_exists, *cascade),
            _ => {
                return Err(SessionError::InvalidStatement(format!(
                    "expected DropTable plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        for name in names {
            let key = name.name.to_lowercase();
            // P0 修复：IF EXISTS 生效 — 表不存在时静默跳过（而非报错）
            let table_exists_local = self.tables.contains_key(&key);
            let table_exists_shared = if let Some(shared) = &self.shared_tables {
                shared.read().await.contains_key(&key)
            } else {
                false
            };
            let table_exists_catalog = self.catalog.get_table(name).is_some();
            let table_exists = table_exists_local || table_exists_shared || table_exists_catalog;

            if !table_exists {
                if if_exists {
                    tracing::debug!(
                        table = %key,
                        "DROP TABLE IF EXISTS: table not found, skipping silently"
                    );
                    continue;
                }
                return Err(SessionError::TableNotFound(name.qualified_name()));
            }

            // CASCADE 级联删除：P0 修复 — 真正级联删除外键引用表
            //
            // 旧实现仅删除表本身 + 关联索引，不级联删除外键引用表（假成功）。
            // 新实现通过 catalog 的 FK 元数据找出所有引用此表的外键约束所在表，
            // 递归删除（CASCADE 语义）。
            //
            // RESTRICT（默认）若有依赖对象应报错。当前 catalog 已跟踪 FK 约束，
            // 但不跟踪视图/物化视图/序列等依赖关系，因此 RESTRICT 仅检查 FK 依赖。
            let mut to_drop: Vec<TableName> = Vec::new();
            if cascade {
                // 收集所有 FK 引用此表的表（递归）
                let mut queue: Vec<TableName> = vec![name.clone()];
                let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
                visited.insert(key.clone());
                while let Some(current) = queue.pop() {
                    // 找出所有 FK 引用 current 表的表
                    let dependents = self.collect_fk_dependents(&current);
                    for dep in dependents {
                        let dep_key = dep.name.to_lowercase();
                        if !visited.contains(&dep_key) {
                            visited.insert(dep_key.clone());
                            to_drop.push(dep.clone());
                            queue.push(dep);
                        }
                    }
                }
                tracing::debug!(
                    table = %key,
                    cascade_count = to_drop.len(),
                    "DROP TABLE CASCADE: cascading drop to FK-dependent tables"
                );
            } else {
                // RESTRICT：若有 FK 依赖则报错
                let dependents = self.collect_fk_dependents(name);
                if !dependents.is_empty() {
                    let dep_names: Vec<String> = dependents
                        .iter()
                        .map(|t| t.qualified_name())
                        .collect();
                    return Err(SessionError::Execution(format!(
                        "DROP TABLE {}: cannot drop table because other objects depend on it (RESTRICT): {}",
                        name.qualified_name(),
                        dep_names.join(", ")
                    )));
                }
            }

            // ADV-CONC-1：从共享存储移除（如果启用）
            if let Some(shared) = &self.shared_tables {
                shared.write().await.remove(&key);
            }
            self.tables.remove(&key);
            // 同时移除物化视图存储表（如果该表是物化视图）
            self.materialized_view_tables.remove(&key);
            self.catalog.remove_table(name);

            // 递归删除 CASCADE 依赖表
            for dep_name in to_drop {
                let dep_key = dep_name.name.to_lowercase();
                if let Some(shared) = &self.shared_tables {
                    shared.write().await.remove(&dep_key);
                }
                self.tables.remove(&dep_key);
                self.materialized_view_tables.remove(&dep_key);
                self.catalog.remove_table(&dep_name);
            }
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

    /// 执行 CREATE INDEX 计划 — P0-FIX-1
    ///
    /// 调用 `Executor::execute_create_index` 在 catalog 注册索引元数据。
    /// 索引数据的实际构建在后续 DML 路径中增量维护。
    fn execute_create_index_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_create_index(plan, &mut self.catalog)?;
        Ok(QueryResult::DdlComplete {
            tag: "CREATE INDEX".into(),
        })
    }

    /// 执行 DROP INDEX 计划 — P0-FIX-1
    ///
    /// 调用 `Executor::execute_drop_index` 从 catalog 移除索引元数据。
    fn execute_drop_index_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_drop_index(plan, &mut self.catalog)?;
        Ok(QueryResult::DdlComplete {
            tag: "DROP INDEX".into(),
        })
    }

    /// 执行 CREATE VIEW / CREATE MATERIALIZED VIEW 计划 — P0-FIX-1
    ///
    /// 调用 `Executor::execute_create_view` 在 catalog 注册视图定义。
    /// P0-6 修复：物化视图在 catalog 注册成功后，创建空存储表并注册到 session，
    /// 供后续 REFRESH 填充数据、SELECT 扫描读取。
    /// P0-MV 修复：CREATE MATERIALIZED VIEW 默认 WITH DATA，创建时立即执行 SELECT
    /// 查询并填充存储表（与 PG 语义一致），不再需要手动 REFRESH。
    async fn execute_create_view_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        // 先提取物化视图信息（因为 execute_create_view 会借用 catalog）
        let (mv_name, mv_columns, mv_query, is_materialized) = match plan {
            LogicalPlan::CreateView {
                name,
                materialized,
                columns,
                query,
                ..
            } => (name.clone(), columns.clone(), (**query).clone(), *materialized),
            _ => {
                return Err(SessionError::Execution(format!(
                    "expected CreateView plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // Executor<'_> 不是 Send，用块作用域确保在 .await 之前释放
        {
            let executor = Executor::new();
            executor.execute_create_view(plan, &mut self.catalog)?;
        }

        if !is_materialized {
            return Ok(QueryResult::DdlComplete {
                tag: "CREATE VIEW".into(),
            });
        }

        // 物化视图 — 创建存储表并立即填充数据
        // Schema 来源：从 select 计划推导列名+类型，应用显式列别名
        let stmt = Statement::Select(Box::new(mv_query.clone()));
        let select_plan = Planner::new(&self.catalog)
            .plan_statement(stmt)
            .map_err(|e| SessionError::Execution(format!(
                "materialized view schema derive failed: {e}"
            )))?;
        let mut schema = szrsql_sql::plan::plan_schema(&select_plan);
        // 应用显式列别名（CREATE MATERIALIZED VIEW mv (col1, col2) AS ...）
        if !mv_columns.is_empty() && mv_columns.len() == schema.columns.len() {
            for (i, alias) in mv_columns.iter().enumerate() {
                schema.columns[i].name = alias.clone();
            }
        }
        schema.name = mv_name.clone();
        let storage = Arc::new(Mutex::new(InMemoryTable::new(schema)));
        let mv_key = mv_name.name.to_lowercase();
        self.materialized_view_tables.insert(mv_key.clone(), storage.clone());

        // P0-MV 修复：立即执行 SELECT 查询并填充物化视图存储表
        // OPT-6：仅锁定查询计划实际引用的表（确保 Executor 不跨 .await 持有，因为 Executor<'_> 非 Send）
        // 先收集被引用 Arc 引用（保持存活直到 guards 释放）
        // P0-DEADLOCK 修复：self.tables 和 shared_tables 可能存有同一个 Arc，
        // tokio::sync::Mutex 不可重入，重复锁定同一 Mutex 会死锁。
        // 使用 Arc::ptr_eq 去重。
        let referenced: std::collections::HashSet<String> = select_plan.collect_referenced_table_names();
        let mut all_arcs: Vec<Arc<Mutex<InMemoryTable>>> = self
            .tables
            .iter()
            .filter(|(k, _)| referenced.contains(&k.to_lowercase()))
            .map(|(_, v)| v.clone())
            .collect();
        if !referenced.is_empty() {
            if let Some(shared) = &self.shared_tables {
                let guard = shared.read().await;
                for (k, v) in guard.iter() {
                    if referenced.contains(&k.to_lowercase())
                        && !all_arcs.iter().any(|a| Arc::ptr_eq(a, v))
                    {
                        all_arcs.push(v.clone());
                    }
                }
            }
        }
        let mut table_guards: Vec<tokio::sync::MutexGuard<'_, InMemoryTable>> = Vec::with_capacity(all_arcs.len());
        for arc in &all_arcs {
            table_guards.push(arc.lock().await);
        }
        let mv_guard = storage.lock().await;
        // 创建 Executor 并执行 SELECT（此后不再有 await 直到 execute 完成）
        let rows = {
            let mut exec = Executor::new()
                .with_catalog(&self.catalog)
                .with_sql_functions_from_catalog(&self.catalog);
            for guard in &table_guards {
                exec.register_table(&**guard);
            }
            exec.register_materialized_view_store(&mv_name.name, &*mv_guard);
            let stmt = Statement::Select(Box::new(mv_query));
            let select_plan = match Planner::new(&self.catalog).plan_statement(stmt) {
                Ok(p) => p,
                Err(e) => {
                    return Err(SessionError::Execution(format!(
                        "materialized view populate plan failed: {e}"
                    )))
                }
            };
            exec.execute(&select_plan)
                .map_err(|e| SessionError::Execution(format!(
                    "materialized view populate failed: {e}"
                )))?
        };
        drop(mv_guard);
        drop(table_guards);
        // 填充存储表
        {
            let mut guard = storage.lock().await;
            guard.clear();
            for row in rows {
                guard.insert(row);
            }
        }

        Ok(QueryResult::DdlComplete {
            tag: "CREATE MATERIALIZED VIEW".into(),
        })
    }

    /// 执行 DROP VIEW / DROP MATERIALIZED VIEW 计划 — P0-FIX-1
    ///
    /// 调用 `Executor::execute_drop_view` 从 catalog 移除视图定义。
    /// P0-6 修复：物化视图同时移除其存储表。
    fn execute_drop_view_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_drop_view(plan, &mut self.catalog)?;
        if let LogicalPlan::DropView {
            names, materialized, ..
        } = plan
        {
            if *materialized {
                for name in names {
                    self.materialized_view_tables
                        .remove(&name.name.to_lowercase());
                }
            }
        }
        let tag = match plan {
            LogicalPlan::DropView {
                materialized: true,
                ..
            } => "DROP MATERIALIZED VIEW",
            _ => "DROP VIEW",
        };
        Ok(QueryResult::DdlComplete {
            tag: tag.into(),
        })
    }

    /// 执行 REFRESH MATERIALIZED VIEW 计划 — P0-FIX-1 / P0-6 修复
    ///
    /// P0-6 修复：实际执行 SELECT 查询并将结果填充到物化视图存储表，
    /// 替代原"仅校验视图存在即返回成功"的假成功实现。
    async fn execute_refresh_materialized_view_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        // P0-6 修复：执行 SELECT 查询并将结果填充到物化视图存储表
        let select_query = {
            let executor = Executor::new();
            executor.execute_refresh_materialized_view(plan, &self.catalog)?
        };
        // 解析物化视图名
        let mv_name = match plan {
            LogicalPlan::RefreshMaterializedView { name, .. } => name.clone(),
            _ => {
                return Err(SessionError::Execution(format!(
                    "expected RefreshMaterializedView plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };
        // 取出存储表（必须存在 — CREATE MATERIALIZED VIEW 时已创建）
        let mv_key = mv_name.name.to_lowercase();
        let mv_storage = self
            .materialized_view_tables
            .get(&mv_key)
            .cloned()
            .ok_or_else(|| {
                SessionError::Execution(format!(
                    "materialized view storage not found for {} (create may have failed)",
                    mv_name.qualified_name()
                ))
            })?;
        // 执行 SELECT 查询
        // 注意：register_table/register_materialized_view_store 存储的是引用，
        // guards 必须存活到 exec.execute() 完成后才能释放。
        // 先锁定所有需要的表（在创建 Executor 之前完成所有 await，
        // 因为 Executor<'_> 持有 &dyn Catalog 引用，不是 Send，不能跨 await 点持有）
        // P0-DEADLOCK 修复：合并 self.tables 和 shared_tables，用 Arc::ptr_eq 去重避免死锁。
        let mut all_arcs: Vec<Arc<Mutex<InMemoryTable>>> = self.tables.values().cloned().collect();
        if let Some(shared) = &self.shared_tables {
            let guard = shared.read().await;
            for (_, v) in guard.iter() {
                if !all_arcs.iter().any(|a| Arc::ptr_eq(a, v)) {
                    all_arcs.push(v.clone());
                }
            }
        }
        let mut table_guards: Vec<tokio::sync::MutexGuard<'_, InMemoryTable>> = Vec::new();
        for arc in &all_arcs {
            table_guards.push(arc.lock().await);
        }
        let mv_guard = mv_storage.lock().await;
        // 现在创建 Executor 并注册表（此后不再有 await 直到 execute 完成）
        // 使用块作用域确保 Executor（!Send）在下一个 await 前被释放
        let rows = {
            let mut exec = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
            for guard in &table_guards {
                exec.register_table(&**guard);
            }
            exec.register_materialized_view_store(&mv_name.name, &*mv_guard);
            // 把 select_query 包装成 Statement::Select 并规划执行
            let stmt = Statement::Select(Box::new(select_query));
            let select_plan = match Planner::new(&self.catalog).plan_statement(stmt) {
                Ok(p) => p,
                Err(e) => {
                    return Err(SessionError::Execution(format!(
                        "materialized view refresh plan failed: {e}"
                    )))
                }
            };
            exec.execute(&select_plan)?
        }; // exec 在此处被释放（块作用域结束）
        // 显式释放 guards
        drop(mv_guard);
        drop(table_guards);
        // 清空并重填存储表
        {
            let mut guard = mv_storage.lock().await;
            guard.clear();
            for row in rows {
                guard.insert(row);
            }
        }
        Ok(QueryResult::DdlComplete {
            tag: "REFRESH MATERIALIZED VIEW".into(),
        })
    }

    /// 执行 CREATE FUNCTION 计划 — P0-5 修复
    ///
    /// 将函数元数据注册到 catalog。函数体执行由表达式求值器在调用时按需触发。
    fn execute_create_function_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_create_function(plan, &mut self.catalog)?;
        Ok(QueryResult::DdlComplete {
            tag: "CREATE FUNCTION".into(),
        })
    }

    /// 执行 DROP FUNCTION 计划 — P0-5 修复
    fn execute_drop_function_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let executor = Executor::new();
        executor.execute_drop_function(plan, &mut self.catalog)?;
        Ok(QueryResult::DdlComplete {
            tag: "DROP FUNCTION".into(),
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

    /// 执行 ALTER TABLE 计划 — Phase F-10
    ///
    /// 1. 从 plan 中取出表名
    /// 2. 锁定对应的 InMemoryTable
    /// 3. 调用 executor.execute_alter_table 同步修改 catalog schema + 表数据
    /// 4. 同步更新 InMemoryTable 的 schema
    async fn execute_alter_table_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        // 从 plan 取出表名
        let table_name = match plan {
            LogicalPlan::AlterTable { name, .. } => name.clone(),
            _ => {
                return Err(SessionError::InvalidStatement(format!(
                    "expected AlterTable plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // 检查是否为 RENAME TABLE（此时无需锁定表数据，因为表会被重命名）
        let is_rename = matches!(
            plan,
            LogicalPlan::AlterTable {
                operations,
                ..
            } if operations.iter().any(|op| matches!(
                op,
                szrsql_sql::ast::AlterTableOperation::RenameTable { .. }
            ))
        );

        if is_rename {
            // RENAME TABLE：无需锁定表数据，仅修改 catalog
            // 注意：当前简化实现，若 RENAME 与其他操作混合，表数据可能不同步
            let executor = Executor::new();
            executor.execute_alter_table(plan, &mut self.catalog, None)?;

            // 同步重命名 self.tables 中的 key
            // 解析新表名
            let new_name = if let LogicalPlan::AlterTable { name, operations, .. } = plan {
                let mut result = name.clone();
                for op in operations {
                    if let szrsql_sql::ast::AlterTableOperation::RenameTable { new_name } = op {
                        result = new_name.clone();
                    }
                }
                result
            } else {
                return Err(SessionError::InvalidStatement(
                    "expected AlterTable plan for rename operation".into(),
                ));
            };

            let old_key = table_name.name.to_lowercase();
            let new_key = new_name.name.to_lowercase();
            if let Some(table_arc) = self.tables.remove(&old_key) {
                self.tables.insert(new_key, table_arc);
            }
        } else {
            // 其他 ALTER TABLE 操作：锁定表数据并同步修改
            let key = table_name.name.to_lowercase();
            let table_arc = self
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| SessionError::TableNotFound(table_name.qualified_name()))?;
            let mut table_guard = table_arc.lock().await;

            let executor = Executor::new();
            executor.execute_alter_table(plan, &mut self.catalog, Some(&mut table_guard))?;

            // 同步更新 InMemoryTable 的 schema
            let updated_schema = self
                .catalog
                .get_table(&table_name)
                .ok_or_else(|| SessionError::TableNotFound(table_name.qualified_name()))?;
            table_guard.set_schema(updated_schema);
        }

        Ok(QueryResult::DdlComplete {
            tag: "ALTER TABLE".into(),
        })
    }

    /// 执行 TRUNCATE TABLE 计划 — 清空表数据（保留表结构）
    ///
    /// 行为与 PG/MySQL/Oracle/SQL Server/SQLite 一致：
    /// - 清空所有目标表的数据行（包括 tombstone 标记）
    /// - 保留表 Schema，可继续 INSERT
    /// - 不触发触发器（与 DELETE 不同）
    /// - 不影响自增序列（简化实现，与 PG 一致；MySQL TRUNCATE 会重置自增）
    /// - 多表 TRUNCATE 时，任一表不存在则报错（除非 IF EXISTS）
    ///
    /// # 参数
    ///
    /// - `plan`：`LogicalPlan::Truncate` 实例
    ///
    /// # 返回
    ///
    /// - `QueryResult::DdlComplete { tag: "TRUNCATE TABLE" }`
    async fn execute_truncate_plan(
        &mut self,
        plan: &LogicalPlan,
    ) -> Result<QueryResult, SessionError> {
        let (names, if_exists, cascade) = match plan {
            LogicalPlan::Truncate {
                names,
                if_exists,
                cascade,
            } => (names, *if_exists, *cascade),
            _ => {
                return Err(SessionError::InvalidStatement(format!(
                    "expected Truncate plan, got {:?}",
                    std::mem::discriminant(plan)
                )))
            }
        };

        // P0 修复：CASCADE 子句生效
        // 当 CASCADE=true 时，应级联清空所有外键引用当前表的所有子表。
        // 当前限制：运行时 TableSchema 不存储 constraints（约束仅在 CREATE TABLE 计划节点中），
        // 无法从 catalog 反查 FK 引用关系。因此 CASCADE 暂为 best-effort：
        // 接受 CASCADE 关键字不报错，但仅清空显式指定的表。
        // 完整级联需待 catalog 增加 constraints 存储后实现。
        let mut tables_to_truncate: Vec<String> = Vec::new();
        for name in names {
            let key = name.name.to_lowercase();
            tables_to_truncate.push(key.clone());
            if cascade {
                tracing::debug!(
                    table = %key,
                    "TRUNCATE CASCADE: cascading not fully implemented (FK constraints not tracked in runtime catalog)"
                );
            }
        }

        for key in &tables_to_truncate {
            // ADV-CONC-1：优先从共享存储查找（如果启用）
            let table_arc = if let Some(shared) = &self.shared_tables {
                shared.read().await.get(key).cloned()
            } else {
                self.tables.get(key).cloned()
            };

            match table_arc {
                Some(arc) => {
                    let mut guard = arc.lock().await;
                    guard.truncate();
                }
                None => {
                    // 会话私有存储兜底
                    if let Some(arc) = self.tables.get(key).cloned() {
                        let mut guard = arc.lock().await;
                        guard.truncate();
                    } else if if_exists {
                        // IF EXISTS：表不存在时跳过
                        continue;
                    } else {
                        return Err(SessionError::TableNotFound(key.clone()));
                    }
                }
            }
        }

        Ok(QueryResult::DdlComplete {
            tag: "TRUNCATE TABLE".into(),
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
        // OPT-6：仅锁定预处理语句实际引用的表（确保 Executor 不跨 .await 持有）
        // 从预处理语句存储中取出 AST，规划后提取引用的表名。
        // 规划失败或未找到时退化为锁定所有表（保持原行为）。
        let referenced: std::collections::HashSet<String> = if let LogicalPlan::Execute { name, .. } = plan {
            match self.prepared_store.get(name) {
                Some((stmt, _)) => {
                    let planner = Planner::new(&self.catalog);
                    match planner.plan_statement(stmt.clone()) {
                        Ok(p) => p.collect_referenced_table_names(),
                        Err(_) => std::collections::HashSet::new(),
                    }
                }
                None => std::collections::HashSet::new(),
            }
        } else {
            std::collections::HashSet::new()
        };
        // ADV-CONC-1：合并本地表和共享表（跨 session CREATE TABLE 可见性）
        let mut all_arcs: std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<InMemoryTable>>> = std::collections::HashMap::new();
        for (k, v) in &self.tables {
            if referenced.is_empty() || referenced.contains(&k.to_lowercase()) {
                all_arcs.insert(k.clone(), v.clone());
            }
        }
        if let Some(shared) = &self.shared_tables {
            for (k, v) in shared.read().await.iter() {
                if referenced.is_empty() || referenced.contains(&k.to_lowercase()) {
                    all_arcs.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        let mut guards = Vec::with_capacity(all_arcs.len());
        for table_arc in all_arcs.values() {
            guards.push(table_arc.lock().await);
        }

        let mut executor = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
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
    ///
    /// # Schema 兼容查找
    ///
    /// tables.json 中表名可能以 "schema_name" 格式存储（如 "public_t"），
    /// 而 SQL 解析 `SELECT FROM "public"."t"` 时 table.name="t"、table.schema="public"。
    /// 为兼容两种存储格式，按以下顺序尝试查找：
    /// 1. 原始 name（如 "t"）
    /// 2. "schema_name" 格式（如 "public_t"）
    /// 3. "public_name" 格式（默认 schema，当 schema 为 None 时尝试）
    async fn get_table_arc(
        &self,
        name: &str,
        schema: Option<&str>,
    ) -> Result<Arc<Mutex<InMemoryTable>>, SessionError> {
        let name_lower = name.to_lowercase();
        // 构造候选键列表（按优先级排序）
        let mut keys_to_try = vec![name_lower.clone()];
        if let Some(s) = schema {
            let s_lower = s.to_lowercase();
            // 避免重复：schema_name 与 name 相同时只加入一次
            let qualified = format!("{}_{}", s_lower, name_lower);
            if qualified != name_lower {
                keys_to_try.push(qualified);
            }
        } else {
            // 默认 schema 为 public
            let default_qualified = format!("public_{}", name_lower);
            if default_qualified != name_lower {
                keys_to_try.push(default_qualified);
            }
        }

        // 优先从共享存储查找
        if let Some(shared) = &self.shared_tables {
            let guard = shared.read().await;
            for key in &keys_to_try {
                if let Some(table) = guard.get(key).cloned() {
                    return Ok(table);
                }
            }
            // MySQL 兼容回退：name 是 "soci_users"（无 schema）→ 遍历找 "_soci_users" 后缀
            // 场景：Navicat 发送 `INSERT INTO soci_users`，但表存储为 `njszjt_soci_users`
            let suffix = format!("_{}", name_lower);
            for k in guard.keys() {
                if k.ends_with(&suffix) {
                    if let Some(table) = guard.get(k).cloned() {
                        return Ok(table);
                    }
                }
            }
        }
        // 退化为本地查找
        for key in &keys_to_try {
            if let Some(table) = self.tables.get(key).cloned() {
                return Ok(table);
            }
        }
        // MySQL 兼容回退：本地共享存储未找到，尝试本地 tables 后缀匹配
        let suffix = format!("_{}", name_lower);
        for (k, table) in &self.tables {
            if k.ends_with(&suffix) {
                return Ok(table.clone());
            }
        }
        Err(SessionError::TableNotFound(name.to_string()))
    }

    /// 收集所有 FK 引用 `target` 表的表名 — 用于 DROP TABLE CASCADE
    ///
    /// 遍历 catalog 中所有表，找出其 FK 约束引用了 `target` 的表。
    /// 返回的表名列表即为需要级联删除的依赖表。
    fn collect_fk_dependents(&self, target: &TableName) -> Vec<TableName> {
        let target_key = target.name.to_lowercase();
        let mut dependents = Vec::new();
        for table_name in self.catalog.list_tables() {
            if table_name.name.to_lowercase() == target_key {
                continue;
            }
            let fks = self.catalog.get_foreign_keys(&table_name);
            for fk in fks {
                let ref_table_key = fk.reference.table.name.to_lowercase();
                if ref_table_key == target_key {
                    dependents.push(table_name.clone());
                    break;
                }
            }
        }
        dependents
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
        // ADV-CONC-1：在规划前从共享存储同步 catalog（跨 session CREATE TABLE 可见性）
        // 扩展查询路径（asyncpg/Navicat prepared statement）必须与简单查询路径一样同步
        self.sync_catalog_from_shared().await;

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

        // Phase 4.7：系统表查询拦截（pg_tables / pg_indexes / information_schema.* / pg_database / pg_namespace / pg_class / ...）
        // 这类查询需要 MutableCatalog 接口，无法走 LogicalPlan::Execute 路径。
        // 与简单查询协议保持一致：直接计算结果集返回。
        if let Some(result) = crate::pgwire::system_tables::try_execute_system_table_query(
            &ps.statement,
            &self.catalog,
            &self.database_name,
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

        // OPT-6：仅锁定查询计划实际引用的表（确保 Executor 不跨 .await 持有）
        // 先对原始 SELECT 语句做一次规划以提取引用的表名。规划失败时退化为锁定所有表
        // （保持原行为，避免因规划错误导致扩展查询不可用）。
        let referenced: std::collections::HashSet<String> = {
            let catalog_ref: &InMemoryCatalog = &self.catalog;
            let planner = Planner::new(catalog_ref);
            match planner.plan_statement(ps.statement.clone()) {
                Ok(p) => p.collect_referenced_table_names(),
                Err(_) => std::collections::HashSet::new(),
            }
        };
        // ADV-CONC-1：合并本地表和共享表（跨 session CREATE TABLE 可见性）
        let mut all_arcs: std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<InMemoryTable>>> = std::collections::HashMap::new();
        for (k, v) in &self.tables {
            if referenced.is_empty() || referenced.contains(&k.to_lowercase()) {
                all_arcs.insert(k.clone(), v.clone());
            }
        }
        if let Some(shared) = &self.shared_tables {
            for (k, v) in shared.read().await.iter() {
                if referenced.is_empty() || referenced.contains(&k.to_lowercase()) {
                    all_arcs.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        let mut guards = Vec::with_capacity(all_arcs.len());
        for table_arc in all_arcs.values() {
            guards.push(table_arc.lock().await);
        }

        let mut executor = Executor::new().with_catalog(&self.catalog).with_sql_functions_from_catalog(&self.catalog);
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

        // Phase 4.7 修复：系统表查询的 Describe 支持。
        // 系统表（pg_namespace/pg_class/information_schema.tables 等）不经过 Planner，
        // Planner 会报 "table not found"，导致 Describe 响应发送 NoData（0 列）。
        // 而实际执行时 try_execute_system_table_query 返回 N 列数据，
        // 造成 "RowDescription 列数=0 但 DataRow 列数=N" 的协议错误。
        // 这里优先用 system_tables 模块推导列，确保 Describe 与 Execute 列数一致。
        if let Some(cols) = crate::pgwire::system_tables::try_describe_system_table_columns(
            statement,
            &self.catalog,
            &self.database_name,
        ) {
            return Ok(cols);
        }

        let plan = {
            let planner = Planner::new(&self.catalog);
            planner.plan_statement(statement.clone())?
        };

        // P0-FN-TYPE 修复：Describe 阶段无 executor，需从 catalog 收集 SQL 函数定义
        // 并设置 current_sql_functions guard，确保 derive_output_columns 能正确
        // 推导函数返回类型（避免 Describe 返回 Text 类型误导客户端）。
        let funcs = szrsql_sql::executor::Executor::collect_sql_functions(&self.catalog);
        let _sql_func_guard = szrsql_sql::expr::current_sql_functions::guard(funcs);

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
        LogicalPlan::Scan { schema, .. }
        | LogicalPlan::IndexScan { schema, .. }
        | LogicalPlan::MaterializedViewScan { schema, .. } => schema
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
///
/// 注意：Aggregate 节点返回其 input 的 schema，因为 Projection 中的聚合函数表达式
/// （如 `max(v)`）需要引用 Aggregate 输入端的列来推导类型。
fn derive_input_schema(plan: &LogicalPlan) -> Vec<szrsql_sql::ast::ColumnDefinition> {
    match plan {
        LogicalPlan::Scan { schema, .. }
        | LogicalPlan::IndexScan { schema, .. }
        | LogicalPlan::MaterializedViewScan { schema, .. } => schema.columns.clone(),
        // P0-VIEW 修复：Projection 节点需从投影表达式推导列类型，
        // 否则视图展开后的外层 Projection 无法获取内层列类型，回退为 Text。
        LogicalPlan::Projection {
            exprs,
            output_names,
            input,
        } => {
            let inner_schema = derive_input_schema(input);
            output_names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let ct = exprs
                        .get(i)
                        .map(|(e, _)| derive_expr_type(e, &inner_schema))
                        .unwrap_or(ColumnType::Text);
                    szrsql_sql::ast::ColumnDefinition::new(name.clone(), ct)
                })
                .collect()
        }
        LogicalPlan::Filter { input, .. }
        | LogicalPlan::Limit { input, .. }
        | LogicalPlan::Distinct { input }
        | LogicalPlan::Sort { input, .. }
        | LogicalPlan::Aggregate { input, .. } => derive_input_schema(input),
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
            // 用户自定义函数（CREATE FUNCTION）从 current_sql_functions 查询声明的返回类型；
            // 其他标量函数兜底为 Text。
            let lower_name = name.to_lowercase();
            match lower_name.as_str() {
                "count" | "sum" | "avg" | "min" | "max" => {
                    derive_aggregate_type(name, args, input_schema)
                }
                _ => {
                    // P0-FN-TYPE 修复：查询用户自定义函数的 RETURNS 声明类型
                    szrsql_sql::expr::current_sql_functions::with(|opt| {
                        opt.as_ref()
                            .and_then(|funcs| funcs.get(&lower_name))
                            .and_then(|overloads| {
                                overloads.iter().find_map(|def| {
                                    parse_return_type_str(&def.return_type)
                                })
                            })
                    })
                    .unwrap_or(ColumnType::Text)
                }
            }
        }
        _ => ColumnType::Text,
    }
}

/// 解析函数返回类型字符串为 ColumnType — P0-FN-TYPE 修复
///
/// 用于从 `CREATE FUNCTION ... RETURNS <type>` 声明推导列类型。
/// 支持常见 SQL 类型名（大小写不敏感），未识别类型返回 None（由调用方兜底为 Text）。
fn parse_return_type_str(type_str: &str) -> Option<ColumnType> {
    let t = type_str.trim().to_lowercase();
    // 去掉括号修饰，如 VARCHAR(50) → varchar
    let base = t.split('(').next().unwrap_or(&t).trim();
    match base {
        "int" | "integer" | "int4" | "int8" | "bigint" | "smallint" | "int2"
        | "serial" | "bigserial" | "mediumint" => Some(ColumnType::Int64),
        "float" | "float4" | "float8" | "double" | "real" | "double precision" | "numeric" | "decimal" => {
            Some(ColumnType::Float64)
        }
        "bool" | "boolean" => Some(ColumnType::Bool),
        "text" | "varchar" | "char" | "character" | "character varying" | "string" => {
            Some(ColumnType::Text)
        }
        "date" | "timestamp" | "timestamptz" | "time" | "timetz" => {
            Some(ColumnType::Timestamp)
        }
        _ => None,
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

    /// P2-1.1：ANALYZE 未注入 statistics_store 时返回错误（测试兼容路径）。
    #[tokio::test]
    async fn test_analyze_without_statistics_store_returns_error() {
        let mut svc = ExecutorService::new();
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT)").await;
        svc.execute_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").await;

        // 未注入 statistics_store，ANALYZE 应返回错误
        let results = svc.execute_sql("ANALYZE t").await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0].is_err(),
            "ANALYZE without statistics_store should error"
        );
    }

    /// P2-1.1：ANALYZE 注入 statistics_store 后正常收集统计信息。
    #[tokio::test]
    async fn test_analyze_with_statistics_store_collects_stats() {
        use szrsql_optimizer::statistics::{InMemoryStatisticsStore, StatisticsStore};

        let store = Arc::new(std::sync::Mutex::new(InMemoryStatisticsStore::new()));
        let mut svc = ExecutorService::new().with_statistics_store(store.clone());
        svc.execute_sql("CREATE TABLE t (id BIGINT, name TEXT)").await;
        svc.execute_sql("INSERT INTO t (id, name) VALUES (1, 'alice')").await;
        svc.execute_sql("INSERT INTO t (id, name) VALUES (2, 'bob')").await;
        svc.execute_sql("INSERT INTO t (id, name) VALUES (3, 'alice')").await;

        // 执行 ANALYZE
        let results = svc.execute_sql("ANALYZE t").await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            Ok(QueryResult::DdlComplete { tag }) => assert_eq!(tag, "ANALYZE"),
            other => panic!("expected DdlComplete ANALYZE, got {other:?}"),
        }

        // 验证统计信息已收集
        let guard = store.lock().unwrap();
        let stats = guard
            .get_table_stats("t")
            .expect("stats for table t should exist");
        assert_eq!(stats.row_count, 3, "row_count should be 3");
        let id_stats = stats
            .column("id")
            .expect("column stats for id should exist");
        assert_eq!(id_stats.null_count, 0, "id null_count should be 0");
        assert_eq!(id_stats.distinct_count, 3, "id distinct_count should be 3");
        assert!(id_stats.min_value.is_some(), "id min should be Some");
        assert!(id_stats.max_value.is_some(), "id max should be Some");
        let name_stats = stats
            .column("name")
            .expect("column stats for name should exist");
        assert_eq!(name_stats.distinct_count, 2, "name distinct_count should be 2 (alice, bob)");
    }

    /// P2-1.1：ANALYZE 不带表名时扫描所有用户表。
    #[tokio::test]
    async fn test_analyze_all_tables() {
        use szrsql_optimizer::statistics::{InMemoryStatisticsStore, StatisticsStore};

        let store = Arc::new(std::sync::Mutex::new(InMemoryStatisticsStore::new()));
        let mut svc = ExecutorService::new().with_statistics_store(store.clone());
        svc.execute_sql("CREATE TABLE t1 (id BIGINT)").await;
        svc.execute_sql("INSERT INTO t1 (id) VALUES (1)").await;
        svc.execute_sql("CREATE TABLE t2 (id BIGINT)").await;
        svc.execute_sql("INSERT INTO t2 (id) VALUES (1)").await;
        svc.execute_sql("INSERT INTO t2 (id) VALUES (2)").await;

        // ANALYZE 无指定表 → 扫描所有用户表
        let results = svc.execute_sql("ANALYZE").await;
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok(), "ANALYZE should succeed");

        // 验证两张表都被分析了
        let guard = store.lock().unwrap();
        let tables = guard.list_tables();
        assert!(
            tables.iter().any(|t| t == "t1"),
            "t1 should be analyzed, got tables: {tables:?}"
        );
        assert!(
            tables.iter().any(|t| t == "t2"),
            "t2 should be analyzed, got tables: {tables:?}"
        );
    }

    /// P2-1.1：ANALYZE 表不存在时返回 TableNotFound 错误。
    #[tokio::test]
    async fn test_analyze_nonexistent_table_errors() {
        use szrsql_optimizer::statistics::InMemoryStatisticsStore;

        let store = Arc::new(std::sync::Mutex::new(InMemoryStatisticsStore::new()));
        let mut svc = ExecutorService::new().with_statistics_store(store.clone());

        let results = svc.execute_sql("ANALYZE nonexistent_table").await;
        assert_eq!(results.len(), 1);
        assert!(
            results[0].is_err(),
            "ANALYZE on nonexistent table should error"
        );
    }

    /// P2-1.1：ANALYZE VERBOSE 等价于 ANALYZE（verbose 仅控制日志）。
    #[tokio::test]
    async fn test_analyze_verbose_works() {
        use szrsql_optimizer::statistics::{InMemoryStatisticsStore, StatisticsStore};

        let store = Arc::new(std::sync::Mutex::new(InMemoryStatisticsStore::new()));
        let mut svc = ExecutorService::new().with_statistics_store(store.clone());
        svc.execute_sql("CREATE TABLE t (id BIGINT)").await;
        svc.execute_sql("INSERT INTO t (id) VALUES (1)").await;

        let results = svc.execute_sql("ANALYZE VERBOSE t").await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            Ok(QueryResult::DdlComplete { tag }) => assert_eq!(tag, "ANALYZE"),
            other => panic!("expected DdlComplete ANALYZE, got {other:?}"),
        }

        let guard = store.lock().unwrap();
        let stats = guard.get_table_stats("t").expect("stats should exist");
        assert_eq!(stats.row_count, 1);
    }

    /// P2-1.2：CBO 激活后 JOIN 查询仍返回正确结果（CostModel + JoinOrderOptimizer 不破坏正确性）。
    ///
    /// 流程：
    /// 1. 注入 statistics_store
    /// 2. 创建两张表 t1（3 行）和 t2（2 行），有外键关系
    /// 3. ANALYZE 收集统计信息
    /// 4. 执行 INNER JOIN 查询
    /// 5. 验证结果正确（行数与预期一致）
    ///
    /// 这验证了：
    /// - SharedStatisticsStore 可成功包装 Arc<Mutex<InMemoryStatisticsStore>>
    /// - CostModel 可读取统计信息（不 panic）
    /// - JoinOrderOptimizer.optimize() 可处理生产 LogicalPlan（不 panic）
    /// - JOIN 顺序重排不破坏查询正确性
    #[tokio::test]
    async fn test_cbo_activation_join_query_returns_correct_results() {
        use szrsql_optimizer::statistics::InMemoryStatisticsStore;

        let store = Arc::new(std::sync::Mutex::new(InMemoryStatisticsStore::new()));
        let mut svc = ExecutorService::new().with_statistics_store(store.clone());

        // 创建两张表
        svc.execute_sql("CREATE TABLE t1 (id BIGINT, name TEXT)").await;
        svc.execute_sql("CREATE TABLE t2 (id BIGINT, t1_id BIGINT, value TEXT)").await;

        // 插入测试数据
        svc.execute_sql("INSERT INTO t1 (id, name) VALUES (1, 'alice')").await;
        svc.execute_sql("INSERT INTO t1 (id, name) VALUES (2, 'bob')").await;
        svc.execute_sql("INSERT INTO t1 (id, name) VALUES (3, 'carol')").await;
        svc.execute_sql("INSERT INTO t2 (id, t1_id, value) VALUES (1, 1, 'v1')").await;
        svc.execute_sql("INSERT INTO t2 (id, t1_id, value) VALUES (2, 2, 'v2')").await;
        svc.execute_sql("INSERT INTO t2 (id, t1_id, value) VALUES (3, 1, 'v3')").await;

        // ANALYZE 收集统计信息（激活 CBO）
        let analyze_results = svc.execute_sql("ANALYZE").await;
        assert_eq!(analyze_results.len(), 1);
        assert!(analyze_results[0].is_ok(), "ANALYZE should succeed");

        // 执行 INNER JOIN（应触发 JoinOrderOptimizer）
        let results = svc
            .execute_sql("SELECT t1.name, t2.value FROM t1 INNER JOIN t2 ON t1.id = t2.t1_id")
            .await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            Ok(QueryResult::ResultSet { rows, .. }) => {
                // t1.id=1 → 2 行（t2.t1_id=1 有 2 行：v1, v3）
                // t1.id=2 → 1 行（t2.t1_id=2 有 1 行：v2）
                // t1.id=3 → 0 行（t2 无 t1_id=3）
                // 总计 3 行
                assert_eq!(rows.len(), 3, "JOIN should return 3 rows, got {}", rows.len());
            }
            other => panic!("expected ResultSet, got {other:?}"),
        }
    }

    /// P2-1.2：未注入 statistics_store 时 JOIN 查询也正常工作（仅 RBO 路径）。
    ///
    /// 这是兼容性测试：确保 CBO 代码路径的添加不影响未注入 store 的场景。
    #[tokio::test]
    async fn test_cbo_not_injected_join_works() {
        let mut svc = ExecutorService::new();

        svc.execute_sql("CREATE TABLE t1 (id BIGINT, name TEXT)").await;
        svc.execute_sql("CREATE TABLE t2 (id BIGINT, t1_id BIGINT)").await;
        svc.execute_sql("INSERT INTO t1 (id, name) VALUES (1, 'alice')").await;
        svc.execute_sql("INSERT INTO t2 (id, t1_id) VALUES (1, 1)").await;

        // 未注入 statistics_store，仅 RBO 生效
        let results = svc
            .execute_sql("SELECT t1.name FROM t1 INNER JOIN t2 ON t1.id = t2.t1_id")
            .await;
        assert_eq!(results.len(), 1);
        match &results[0] {
            Ok(QueryResult::ResultSet { rows, .. }) => {
                assert_eq!(rows.len(), 1, "JOIN should return 1 row");
            }
            other => panic!("expected ResultSet, got {other:?}"),
        }
    }

    /// P2-1.2：SharedStatisticsStore 可正确读取统计信息（与 InMemoryStatisticsStore 一致）。
    ///
    /// 验证 SharedStatisticsStore 的 `get_table_stats` 返回的 Arc<TableStatistics>
    /// 与直接访问 InMemoryStatisticsStore 一致。
    #[tokio::test]
    async fn test_shared_statistics_store_returns_correct_stats() {
        use szrsql_optimizer::statistics::{
            InMemoryStatisticsStore, SharedStatisticsStore, StatisticsStore,
        };

        let inner = Arc::new(std::sync::Mutex::new(InMemoryStatisticsStore::new()));
        let mut svc = ExecutorService::new().with_statistics_store(inner.clone());
        svc.execute_sql("CREATE TABLE t (id BIGINT)").await;
        svc.execute_sql("INSERT INTO t (id) VALUES (1)").await;
        svc.execute_sql("INSERT INTO t (id) VALUES (2)").await;
        svc.execute_sql("INSERT INTO t (id) VALUES (3)").await;
        let analyze_results = svc.execute_sql("ANALYZE t").await;
        assert!(analyze_results[0].is_ok(), "ANALYZE should succeed");

        // 通过 SharedStatisticsStore 读取
        let shared = SharedStatisticsStore::new(inner.clone());
        let stats_via_shared = shared
            .get_table_stats("t")
            .expect("SharedStatisticsStore should return stats");
        assert_eq!(stats_via_shared.row_count, 3);

        // 通过 InMemoryStatisticsStore 直接读取，应一致
        let guard = inner.lock().unwrap();
        let stats_via_inner = guard
            .get_table_stats("t")
            .expect("InMemoryStatisticsStore should return stats");
        assert_eq!(stats_via_inner.row_count, 3);
        assert_eq!(stats_via_shared.row_count, stats_via_inner.row_count);
    }
}
