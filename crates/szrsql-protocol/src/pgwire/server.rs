//! pgwire TCP 服务器与连接处理。
//!
//! # 架构
//!
//! - `PgwireServer`：持有 `PgwireConfig` 配置，绑定监听端口
//! - 每个客户端连接在独立 tokio task 中处理
//! - 单个连接的处理流程：
//!     1. 等待 StartupMessage（可能先收到 SSLRequest，Phase 4.5 升级为 TLS）
//!     2. 校验 user/database
//!     3. 发送 AuthenticationOk + ParameterStatus* + BackendKeyData + ReadyForQuery
//!     4. 进入主循环，等待前端消息
//!        - Query / Terminate（简单查询协议，Phase 4.2）
//!        - Parse / Bind / Execute / Describe / Close / Sync / Flush（扩展查询协议，Phase 4.3）
//!
//! Phase 4.3 扩展查询：
//! - 错误后服务器进入 "aborted" 状态，忽略除 Sync/Flush 外的所有消息，直到收到 Sync
//! - Sync 触发 ReadyForQuery 响应
//! - Flush 仅 flush 缓冲但不发送 ReadyForQuery
//!
//! Phase 4.5 TLS：
//! - 收到 SSLRequest 时根据 `PgwireConfig::tls` 配置回复 'S' 或 'N'
//! - 配置了 TLS：回复 'S' 后执行 rustls TLS 1.3 握手，stream 升级为加密流
//! - 未配置 TLS：回复 'N'，客户端应回退明文继续 StartupMessage

use crate::pgwire::auth::{
    AuthError, AuthMode, ScramServerSession, SharedScramCredentials, SCRAM_MECHANISM,
};
use crate::pgwire::dirty_tracker::DirtyTableTracker;
use crate::pgwire::lifecycle::{ShutdownCoordinator, ShutdownSignal};
use crate::pgwire::message::{
    BackendMessage, ErrorResponse, FrontendMessage, SqlState, STATUS_IDLE,
    STATUS_IN_FAILED_TRANSACTION, STATUS_IN_TRANSACTION,
};
use crate::pgwire::notify::NotifyHub;
use crate::pgwire::pg_types::{
    column_type_oid, column_type_size, column_type_supports_binary, value_to_binary, value_to_text,
};
use crate::pgwire::session::{
    ExecutorService, ExtendedExecuteResult, QueryResult, SessionError, TransactionState,
};
use crate::pgwire::startup::{
    build_auth_error_response, build_protocol_error_response, build_startup_response, StartupError,
    StartupMessage, StartupParams,
};
use crate::pgwire::tls::{TlsConfig, TlsError};
use bytes::{Buf, BufMut, BytesMut};
// P0-1：脱敏集成需要 Value 类型
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use szrsql_replication::stream::ReplicationPrimary;
use szrsql_sql::executor::{InMemoryTable, SharedSequenceState};
use szrsql_tx::lock::LockManager;
use szrsql_tx::mvcc::MvccManager;
use szrsql_tx::wal::WalWriter;
use szrsql_types::value::Value;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, RwLock, Semaphore};

// =====================================================================
//  配置
// =====================================================================

/// pgwire 服务器配置。
#[derive(Debug, Clone)]
pub struct PgwireConfig {
    /// 监听地址（如 "127.0.0.1" 或 "0.0.0.0"）。
    pub host: String,
    /// 监听端口（PG 默认 5432）。
    pub port: u16,
    /// 服务器版本号字符串（发送给客户端作为 ParameterStatus）。
    pub server_version: String,
    /// 允许连接的数据库列表（空表示允许所有）。Phase 4.1 暂不强制校验。
    pub allowed_databases: Vec<String>,
    /// 允许连接的用户列表（空表示允许所有）。Phase 4.1 暂不强制校验。
    pub allowed_users: Vec<String>,
    /// Phase 4.4：认证模式（默认 Trust）。
    pub auth_mode: AuthMode,
    /// Phase 4.5：TLS 配置（None 表示不支持 SSL，收到 SSLRequest 回复 'N'）。
    pub tls: Option<TlsConfig>,
    /// Phase 4.5：是否强制 TLS（拒绝明文连接）。
    ///
    /// 为 `true` 时，收到非 SSLRequest 的 StartupMessage 将被拒绝
    /// （回复 'E' + "SSLRequired" 并关闭连接），强制客户端使用 SSLRequest。
    /// 为 `false` 时（默认），允许客户端回退到明文连接。
    pub require_tls: bool,
    /// Phase 4.11：优雅关闭超时（默认 30s）。
    ///
    /// 收到关闭信号后，等待活跃连接完成的最长时间；超时后强制中止剩余连接。
    pub shutdown_timeout: std::time::Duration,
    /// 连接空闲超时（默认 300s = 5 分钟；`Duration::ZERO` 表示禁用）。
    ///
    /// 当连接在此时间内未收到任何客户端消息时，服务器主动关闭连接并释放
    /// session 资源（回滚未提交事务、释放行锁），避免因客户端异常断开
    /// （如被 kill -9 / Stop-Process 强制终止，TCP 未发送 FIN）导致的
    /// 会话死锁和资源泄漏。
    pub connection_idle_timeout: std::time::Duration,
    /// 最大并发连接数（默认 100；0 表示不限制）。
    ///
    /// 超过此限制时新连接将被拒绝（回复 FATAL 错误并关闭），
    /// 防止连接数耗尽导致 OOM 或文件描述符耗尽。
    pub max_connections: usize,
    /// P2-14：运行时可热重载的 SCRAM 凭据存储（None 表示使用启动时快照）。
    ///
    /// 注入后，认证路径优先从该共享存储读取最新凭据，
    /// `/api/v1/config/reload` 热重载后新连接立即生效。
    pub shared_scram: Option<Arc<SharedScramCredentials>>,
}

impl Default for PgwireConfig {
    /// 默认配置：监听 `127.0.0.1:5432`，服务器版本 `15.0-szrsql`，trust 认证，无 TLS。
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 5432,
            server_version: "15.0-szrsql".to_string(),
            allowed_databases: Vec::new(),
            allowed_users: Vec::new(),
            auth_mode: AuthMode::Trust,
            tls: None,
            require_tls: false,
            shutdown_timeout: std::time::Duration::from_secs(30),
            connection_idle_timeout: std::time::Duration::from_secs(300),
            max_connections: 100,
            shared_scram: None,
        }
    }
}

impl PgwireConfig {
    /// 构造监听 `127.0.0.1:5432` 的默认配置。
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

    /// 设置服务器版本号。
    pub fn with_server_version(mut self, version: impl Into<String>) -> Self {
        self.server_version = version.into();
        self
    }

    /// Phase 4.4：设置认证模式。
    pub fn with_auth_mode(mut self, mode: AuthMode) -> Self {
        self.auth_mode = mode;
        self
    }

    /// Phase 4.5：启用 TLS 1.3 加密（收到 SSLRequest 时回复 'S' 并执行 TLS 握手）。
    pub fn with_tls(mut self, tls: TlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Phase 4.5：设置是否强制 TLS（拒绝明文连接）。
    ///
    /// 为 `true` 时，客户端必须先发送 SSLRequest 升级为 TLS 才能继续握手；
    /// 直接发送明文 StartupMessage 将被拒绝（回复 'E' + "SSLRequired"）。
    pub fn with_require_tls(mut self, require: bool) -> Self {
        self.require_tls = require;
        self
    }

    /// Phase 4.11：设置优雅关闭超时。
    ///
    /// 收到关闭信号后，等待活跃连接完成的最长时间；超时后强制中止剩余连接。
    pub fn with_shutdown_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// 设置连接空闲超时（`Duration::ZERO` 表示禁用）。
    ///
    /// 当连接在此时间内未收到任何客户端消息时，服务器主动关闭连接并释放
    /// session 资源（回滚未提交事务、释放行锁），避免客户端异常断开导致的死锁。
    pub fn with_connection_idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.connection_idle_timeout = timeout;
        self
    }

    /// 设置最大并发连接数（0 表示不限制）。
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections = max;
        self
    }

    /// P2-14：注入运行时可热重载的 SCRAM 凭据存储。
    ///
    /// 注入后认证路径优先从该存储读取最新凭据（支持 `/api/v1/config/reload` 热重载），
    /// 未注入时使用 `with_auth_mode` 设置的启动时快照。
    pub fn with_shared_scram_credentials(mut self, shared: Arc<SharedScramCredentials>) -> Self {
        self.shared_scram = Some(shared);
        self
    }
}

// =====================================================================
//  ServerError
// =====================================================================

/// 服务器运行错误。
#[derive(Debug, Error)]
pub enum ServerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("startup error: {0}")]
    Startup(#[from] StartupError),

    /// Phase 4.5：TLS 配置或握手错误。
    #[error("tls error: {0}")]
    Tls(#[from] TlsError),
}

// =====================================================================
//  FirstStartupMessage（Phase 4.5）
// =====================================================================

/// `handle_connection` 中读取到的首个启动消息类型。
///
/// 用于决定是否需要执行 TLS 升级。
enum FirstStartupMessage {
    /// 客户端在发送任何消息前断开。
    None,
    /// SSLRequest：客户端请求 SSL 加密。
    SslRequest,
    /// GSSNCRequest：客户端请求 GSSAPI 加密（不支持，将回复 'N'）。
    GssencRequest,
    /// CancelRequest：取消查询请求（不进入主循环）。
    CancelRequest,
    /// Startup：正常启动消息（buf 中已包含未消费的 StartupMessage）。
    Startup,
}

// =====================================================================
//  NotifyCleanupGuard（Phase 4.6）
// =====================================================================

/// RAII 守卫：确保会话结束时从 `NotifyHub` 注销订阅。
///
/// 由于 `handle_main_loop` 有多个 early return 路径（Terminate / 断开 / 协议错误），
/// 使用 RAII 守卫保证 `unregister` 总是被调用，避免内存泄漏。
struct NotifyCleanupGuard {
    hub: NotifyHub,
    pid: i32,
    /// 是否已主动清理（避免重复调用 unregister，虽然 unregister 是幂等的）
    cleaned: bool,
}

impl NotifyCleanupGuard {
    fn new(hub: NotifyHub, pid: i32) -> Self {
        Self {
            hub,
            pid,
            cleaned: false,
        }
    }

    /// 主动清理（标记为已清理，drop 时不再重复调用）
    fn cleanup(&mut self) {
        if !self.cleaned {
            self.hub.unregister(self.pid);
            self.cleaned = true;
        }
    }
}

impl Drop for NotifyCleanupGuard {
    fn drop(&mut self) {
        self.cleanup();
    }
}

// =====================================================================
//  CancelRegistryGuard（OPT-13）
// =====================================================================

/// RAII 守卫：确保会话结束时从 `cancel_registry` 注销。
///
/// 与 `NotifyCleanupGuard` 类似，确保连接因任何原因退出时
/// （正常 Terminate、客户端断开、协议错误）都从注册表移除 PID，避免内存泄漏。
struct CancelRegistryGuard {
    registry: Option<Arc<std::sync::Mutex<HashMap<i32, Arc<tokio::sync::Notify>>>>>,
    pid: i32,
}

impl CancelRegistryGuard {
    fn new(
        registry: Option<Arc<std::sync::Mutex<HashMap<i32, Arc<tokio::sync::Notify>>>>>,
        pid: i32,
    ) -> Self {
        Self { registry, pid }
    }
}

impl Drop for CancelRegistryGuard {
    fn drop(&mut self) {
        if let Some(registry) = &self.registry {
            if let Ok(mut map) = registry.lock() {
                map.remove(&self.pid);
            }
        }
    }
}

// =====================================================================
//  PgwireServer
// =====================================================================

/// pgwire TCP 服务器。
pub struct PgwireServer {
    config: PgwireConfig,
    pid_counter: AtomicI32,
    /// Phase 4.6：跨会话通知中心，所有连接共享同一实例。
    notify_hub: NotifyHub,
    /// Phase 4.11：优雅关闭协调器。
    shutdown: ShutdownCoordinator,
    /// ADV-F-7：共享的 WAL 写入器（log-then-commit 模型所需）。
    ///
    /// 当通过 [`with_wal_writer`] 注入时，每个 session 在 COMMIT 时会：
    /// 1. 写入 WalOpType::Commit 记录
    /// 2. 调用 flush()（fsync）强制刷盘
    /// 3. fsync 成功后才向客户端 ACK 成功
    ///
    /// 为 None 时（默认），退化为旧的 commit-then-log 行为，仅用于测试兼容。
    wal_writer: Option<Arc<WalWriter>>,
    /// ADV-CONC-1：跨会话共享的表存储（多线程并发支持）。
    ///
    /// 所有 session 共享同一份表数据，CREATE TABLE 注册到共享存储，
    /// 其他 session 可见。未启用时（None），每个 session 持有私有表副本。
    shared_tables: Option<Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>>,
    /// ADV-CONC-1：跨会话共享的行锁管理器（多线程并发支持）。
    ///
    /// DML 操作对每行加 X 锁，SELECT FOR UPDATE/SHARE 加对应锁，
    /// COMMIT/ROLLBACK 时 unlock_all(txn_id)（Strict 2PL）。
    lock_manager: Option<Arc<LockManager>>,
    shared_txn_counter: Option<Arc<std::sync::atomic::AtomicU32>>,
    /// P0-4 修复：跨会话共享的序列全局状态。
    ///
    /// 启用后所有 session 共享同一份序列 nextval 推进状态，符合 PG 语义。
    /// `None`（默认）：退化为每 session 私有序列存储（旧行为，用于测试兼容）。
    shared_sequence_state: Option<SharedSequenceState>,
    /// P0-TX-1 修复：跨会话共享的 MVCC 事务管理器。
    ///
    /// 启用后，每个 session 的 BEGIN/COMMIT/ROLLBACK 会同步到 MvccManager，
    /// 实现 MVCC 事务可见性判断（而非表级 snapshot/restore）。
    /// 未启用时退化为表级 snapshot/restore（旧行为，用于测试兼容）。
    mvcc: Option<Arc<MvccManager>>,
    /// P0-DIST-1/2/3：跨会话共享的分布式运行时句柄。
    ///
    /// 启用后，DML 操作通过 `dist_dual_write` 双写到分布式 KV 存储
    /// （Raft propose → apply），实现真实分布式持久化路径。
    /// 未启用时退化为本地内存表存储（旧行为，用于测试兼容）。
    dist_runtime: Option<szrsql_dist::runtime::DistRuntimeHandle>,
    /// P7-1：跨会话共享的 CDC 引擎。
    ///
    /// 启用后，DML 操作（INSERT/UPDATE/DELETE）会将行级变更事件分发到 CDC 引擎，
    /// 供已注册的 CdcObserver（如 ReplicationTask）消费，实现变更数据捕获。
    /// 未启用时退化为旧行为（DML 不触发 CDC 事件）。
    cdc_engine: Option<Arc<szrsql_cdc::CdcEngine>>,
    /// P1-2：跨会话共享的脏表跟踪器（用于增量快照机制）。
    ///
    /// 启用后，session 在事务 COMMIT 成功后调用 `tracker.mark_dirty(table_name)`，
    /// 后台周期性快照任务仅对脏表集合中的表重新序列化，避免无谓的全量 IO。
    /// 未启用时退化为旧行为（全量快照，每次都序列化所有表）。
    dirty_tracker: Option<Arc<DirtyTableTracker>>,
    /// P2-1.1：跨会话共享的统计信息存储（ANALYZE 写入，CostModel 读取）。
    ///
    /// 启用后，`ANALYZE` 命令扫描表数据收集统计信息（行数、NDV、min/max、直方图），
    /// 结果存入此 store，供 CostModel 进行基于成本的优化。
    /// 未启用时 ANALYZE 命令返回错误（不支持）。
    statistics_store:
        Option<Arc<std::sync::Mutex<szrsql_optimizer::statistics::InMemoryStatisticsStore>>>,
    /// OPT-12：跨会话共享的 SQL 防火墙（SQL 注入检测 + 禁止命令 + 白名单）。
    ///
    /// 启用后，每个 session 的 `handle_query` 在执行 SQL 前调用 `firewall.check(sql)`：
    /// - 命中注入特征 → 返回 ERROR，不执行
    /// - 命中禁止命令 → 返回 ERROR，不执行
    /// - 不在白名单 → 返回 ERROR，不执行
    /// 未启用时（None）跳过安全检查（旧行为，用于测试兼容）。
    security_firewall: Option<Arc<tokio::sync::Mutex<szrsql_security::firewall::SqlFirewall>>>,
    /// OPT-12：跨会话共享的审计日志（不可变 append-only + SHA-256 哈希链）。
    ///
    /// 启用后，每个 session 的 `handle_query` 在执行 SQL 后记录审计事件：
    /// - 事件包含 SQL 文本、执行结果（成功/失败）、客户端信息
    /// - 哈希链保证日志不可篡改
    /// 未启用时（None）跳过审计记录（旧行为，用于测试兼容）。
    audit_log: Option<Arc<tokio::sync::Mutex<szrsql_security::audit::AuditLog>>>,
    /// P0-1：跨会话共享的 TDE 透明页级加密引擎（AES-256-CTR）。
    ///
    /// 启用后，WAL Full Page Image（FPI）记录在写入前调用 `tde.encrypt_page(page)` 加密，
    /// 崩溃恢复读取时调用 `tde.decrypt_page(ciphertext)` 解密。
    /// 未启用时（None）跳过页加密（旧行为，用于测试兼容）。
    tde_engine: Option<Arc<tokio::sync::Mutex<szrsql_security::tde::TdeEngine>>>,
    /// P0-1：跨会话共享的列加密引擎（AES-256-GCM）。
    ///
    /// 启用后，executor 在 INSERT 配置列时调用 `col_enc.encrypt(...)`，
    /// SELECT 时调用 `col_enc.decrypt(...)`。未启用时跳过列加密。
    column_encryption_engine:
        Option<Arc<tokio::sync::Mutex<szrsql_security::column_enc::ColumnEncryptionEngine>>>,
    /// P0-1：跨会话共享的数据脱敏引擎。
    ///
    /// 启用后，`handle_query` 在编码 SELECT 结果时根据表/列规则对敏感字段脱敏。
    /// 未启用时跳过脱敏（旧行为，用于测试兼容）。
    masking_engine: Option<Arc<tokio::sync::Mutex<szrsql_security::masking::MaskingEngine>>>,
    /// P0-1：跨会话共享的密码策略注册表。
    ///
    /// 启用后，CREATE ROLE / ALTER ROLE 修改密码时调用 `registry.validate(...)`，
    /// 不满足复杂度/历史/有效期策略时返回 ERROR。未启用时跳过校验。
    password_profile_registry:
        Option<Arc<tokio::sync::Mutex<szrsql_security::password_profile::PasswordProfileRegistry>>>,
    /// P2-1：跨会话共享的 HLC 混合逻辑时钟（Multi-Master 因果排序）。
    ///
    /// 启用后传递给每个 session 的 Executor，用于 DML 操作的 HLC 时间戳生成。
    /// 未启用时退化为旧行为（不生成 HLC 时间戳）。
    hlc_clock: Option<Arc<std::sync::Mutex<szrsql_dist::conflict::HlcClock>>>,
    /// P2-1：跨会话共享的冲突日志（Multi-Master 写入冲突审计）。
    ///
    /// 启用后传递给每个 session 的 Executor，用于记录写-写冲突事件。
    /// 未启用时退化为旧行为（不记录冲突日志）。
    conflict_log: Option<Arc<std::sync::Mutex<szrsql_dist::conflict::ConflictLog>>>,
    /// P2-1：本节点 ID（Multi-Master 写操作来源标识）。
    ///
    /// 默认为 1（单节点模式）。注入后传递给每个 session 的 Executor。
    node_id: u64,
    /// 生产监控告警：跨会话共享的 Prometheus 指标注册表。
    ///
    /// 启用后，连接建立/断开、查询执行、事务提交/回滚等关键事件会更新对应计数器，
    /// 通过 HTTP `/metrics` 端点暴露 Prometheus 文本格式指标。
    /// 未启用时（None）跳过指标收集（旧行为，用于测试兼容）。
    metrics: Option<Arc<crate::http::MetricsRegistry>>,
    /// OPT-13：会话取消注册表（PID → Notify）。
    ///
    /// 启用后，HTTP `/api/v1/cancel/{pid}` 端点可触发指定会话的查询取消。
    /// 每个连接在建立时注册 `Arc<Notify>`，主循环在等待消息时通过
    /// `tokio::select!` 监听取消信号；收到信号后发送 ErrorResponse + ReadyForQuery。
    /// 连接断开时自动注销。未启用时（None）取消端点返回 503。
    cancel_registry: Option<Arc<std::sync::Mutex<HashMap<i32, Arc<tokio::sync::Notify>>>>>,
    /// P2-2.2：跨会话共享的流复制主库实例。
    ///
    /// 启用后，每个 session 在事务 COMMIT 成功后将 WAL 记录（TableData + Commit）
    /// 推送到 `ReplicationPrimary`，由后者扇出到所有已连接的 TCP 备库。
    /// 未启用时（None）跳过复制推送（旧行为，用于单节点模式或测试兼容）。
    replication_primary: Option<Arc<ReplicationPrimary>>,
    /// 连接数限制信号量。
    ///
    /// 许可数 = `config.max_connections`；每个连接任务持有一个 `OwnedSemaphorePermit`，
    /// 连接结束后自动释放。`max_connections == 0` 时使用 `usize::MAX` 表示不限制。
    conn_semaphore: Arc<Semaphore>,
}

impl PgwireServer {
    /// 构造一个新服务器实例。
    pub fn new(config: PgwireConfig) -> Self {
        let shutdown = ShutdownCoordinator::new(config.shutdown_timeout);
        let max_conn = if config.max_connections == 0 {
            usize::MAX
        } else {
            config.max_connections
        };
        Self {
            config,
            pid_counter: AtomicI32::new(1),
            notify_hub: NotifyHub::new(),
            shutdown,
            wal_writer: None,
            shared_tables: None,
            lock_manager: None,
            shared_txn_counter: None,
            shared_sequence_state: None,
            mvcc: None,
            dist_runtime: None,
            cdc_engine: None,
            hlc_clock: None,
            conflict_log: None,
            node_id: 1,
            dirty_tracker: None,
            statistics_store: None,
            security_firewall: None,
            audit_log: None,
            tde_engine: None,
            column_encryption_engine: None,
            masking_engine: None,
            password_profile_registry: None,
            metrics: None,
            cancel_registry: None,
            replication_primary: None,
            conn_semaphore: Arc::new(Semaphore::new(max_conn)),
        }
    }

    /// ADV-F-7：注入共享的 WalWriter，启用 log-then-commit 事务模型。
    ///
    /// 启用后，所有 session 的 COMMIT 操作会先写 WAL Commit 记录并 fsync，
    /// 然后才向客户端返回成功。这消除了"ACK 成功但数据未持久化"的风险。
    ///
    /// # 参数
    ///
    /// - `writer`：共享的 `WalWriter` 实例（通常在 main.rs 中创建一次，所有连接共享）
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let writer = Arc::new(WalWriter::create_new("/var/lib/szrsql/wal.log")?);
    /// let server = PgwireServer::new(config).with_wal_writer(writer);
    /// ```
    pub fn with_wal_writer(mut self, writer: Arc<WalWriter>) -> Self {
        self.wal_writer = Some(writer);
        self
    }

    /// ADV-CONC-1：启用多线程并发支持（共享表存储 + 行级锁）。
    ///
    /// 启用后：
    /// - 所有 session 共享同一份表数据（CREATE TABLE 对其他 session 可见）
    /// - DML 操作通过共享 `LockManager` 实现行级锁（Strict 2PL）
    /// - 跨 session 的 UPDATE/DELETE 互斥，避免丢失更新
    ///
    /// 未启用时，每个 session 持有私有表副本（旧行为，用于测试兼容）。
    ///
    /// # 参数
    ///
    /// - `shared_tables`：共享表存储
    /// - `lock_manager`：共享行锁管理器
    pub fn with_concurrency(
        mut self,
        shared_tables: Arc<RwLock<HashMap<String, Arc<Mutex<InMemoryTable>>>>>,
        lock_manager: Arc<LockManager>,
    ) -> Self {
        self.shared_tables = Some(shared_tables);
        self.lock_manager = Some(lock_manager);
        self
    }

    /// ADV-CONC-1：注入跨会话共享的事务 ID 计数器。
    ///
    /// 启用后，每个 session 的 BEGIN 从此计数器原子递增获取全局唯一 txn_id，
    /// 确保不同 session 的事务不会共享同一个 txn_id（否则 LockManager 会将
    /// 两个独立事务误判为同一事务，重入锁不阻塞，导致并发隔离失效）。
    pub fn with_shared_txn_counter(mut self, counter: Arc<std::sync::atomic::AtomicU32>) -> Self {
        self.shared_txn_counter = Some(counter);
        self
    }

    /// P0-4 修复：注入跨会话共享的序列全局状态。
    ///
    /// 启用后所有 session 共享同一份序列 nextval 推进状态：
    /// - `CREATE SEQUENCE` 在共享状态中创建，所有 session 可见
    /// - `nextval(seq)` 推进全局状态，多 session 调用同一序列时返回递增值
    /// - `currval(seq)` 仍按 session 隔离（PG 语义）
    ///
    /// 未启用时退化为每 session 私有序列存储（旧行为，用于测试兼容）。
    pub fn with_shared_sequence_state(mut self, state: SharedSequenceState) -> Self {
        self.shared_sequence_state = Some(state);
        self
    }

    /// P0-TX-1 修复：注入跨会话共享的 MVCC 事务管理器。
    ///
    /// 启用后，所有 session 的 BEGIN/COMMIT/ROLLBACK 会同步到 MvccManager 状态机，
    /// 实现 MVCC 事务可见性判断、SSI 写偏斜检测、First-Committer-Wins。
    ///
    /// 未启用时退化为表级 snapshot/restore（旧行为，用于测试兼容）。
    pub fn with_mvcc(mut self, mgr: Arc<MvccManager>) -> Self {
        self.mvcc = Some(mgr);
        self
    }

    /// P0-DIST-1/2/3：注入跨会话共享的分布式运行时句柄。
    ///
    /// 启用后，DML 操作通过 `Executor::dist_dual_write` 双写到分布式 KV 存储
    /// （Raft propose → apply），实现真实分布式持久化路径。
    ///
    /// 未启用时退化为本地内存表存储（旧行为，用于测试兼容）。
    ///
    /// # 参数
    /// - `handle`：`Arc<RwLock<DistRuntime>>` 共享句柄（由 main.rs 创建并初始化）
    pub fn with_dist_runtime(mut self, handle: szrsql_dist::runtime::DistRuntimeHandle) -> Self {
        self.dist_runtime = Some(handle);
        self
    }

    /// P7-1：注入跨会话共享的 CDC 引擎，启用 DML 事件分发。
    ///
    /// 启用后，所有 session 的 DML 操作（INSERT/UPDATE/DELETE）会将行级变更事件
    /// 分发到 CDC 引擎，供已注册的 CdcObserver（如 ReplicationTask）消费，
    /// 实现变更数据捕获。
    ///
    /// 未启用时退化为旧行为（DML 不触发 CDC 事件）。
    ///
    /// # 参数
    /// - `engine`：共享的 `CdcEngine` 实例（由 main.rs 创建一次，所有连接共享）
    pub fn with_cdc_engine(mut self, engine: Arc<szrsql_cdc::CdcEngine>) -> Self {
        self.cdc_engine = Some(engine);
        self
    }

    /// P2-1：注入跨会话共享的 HLC 混合逻辑时钟，启用 Multi-Master 因果排序。
    ///
    /// 启用后，会传递给每个 session 的 Executor，用于 DML 操作的 HLC 时间戳生成。
    /// 未启用时退化为旧行为（不生成 HLC 时间戳）。
    pub fn with_hlc_clock(
        mut self,
        clock: Arc<std::sync::Mutex<szrsql_dist::conflict::HlcClock>>,
    ) -> Self {
        self.hlc_clock = Some(clock);
        self
    }

    /// P2-1：注入跨会话共享的冲突日志，启用 Multi-Master 写入冲突审计。
    ///
    /// 启用后，会传递给每个 session 的 Executor，用于记录写-写冲突事件。
    /// 未启用时退化为旧行为（不记录冲突日志）。
    pub fn with_conflict_log(
        mut self,
        log: Arc<std::sync::Mutex<szrsql_dist::conflict::ConflictLog>>,
    ) -> Self {
        self.conflict_log = Some(log);
        self
    }

    /// P2-1：设置本节点 ID（Multi-Master 写操作来源标识）。
    pub fn with_node_id(mut self, node_id: u64) -> Self {
        self.node_id = node_id;
        self
    }

    /// P1-2：注入跨会话共享的脏表跟踪器，启用增量快照机制。
    ///
    /// 启用后，每个 session 在事务 COMMIT 成功后会调用 `tracker.mark_dirty(table_name)`
    /// 标记该事务修改过的表为脏。后台周期性快照任务仅对脏表集合中的表重新序列化，
    /// 避免每次都对所有表做全量序列化。
    ///
    /// 未启用时退化为旧行为（全量快照）。
    ///
    /// # 参数
    ///
    /// - `tracker`：共享的 `DirtyTableTracker` 实例（通常在 main.rs 中创建一次，
    ///   传递给 PgwireServer 和后台快照任务共享）
    pub fn with_dirty_tracker(mut self, tracker: Arc<DirtyTableTracker>) -> Self {
        self.dirty_tracker = Some(tracker);
        self
    }

    /// P2-1.1：注入跨会话共享的统计信息存储，启用 ANALYZE 命令支持。
    ///
    /// 启用后，所有 session 的 `ANALYZE [table_name [, ...]]` 命令会扫描表数据
    /// 收集统计信息（行数、NDV、min/max、直方图），存入共享 store，
    /// 供 CostModel 进行基于成本的优化（P2-1.2 激活）。
    ///
    /// 未启用时 ANALYZE 命令返回错误（不支持，用于测试兼容）。
    ///
    /// # 参数
    ///
    /// - `store`：共享的统计信息存储（通常在 main.rs 中创建一次，所有连接共享）
    pub fn with_statistics_store(
        mut self,
        store: Arc<std::sync::Mutex<szrsql_optimizer::statistics::InMemoryStatisticsStore>>,
    ) -> Self {
        self.statistics_store = Some(store);
        self
    }

    /// OPT-12：注入共享的 SQL 防火墙，启用 SQL 注入检测和命令过滤。
    ///
    /// 启用后，每个 session 的 `handle_query` 在执行 SQL 前调用 `firewall.check(sql)`，
    /// 命中注入特征/禁止命令/不在白名单的 SQL 将被拒绝执行并返回 ERROR。
    pub fn with_security_firewall(
        mut self,
        firewall: Arc<tokio::sync::Mutex<szrsql_security::firewall::SqlFirewall>>,
    ) -> Self {
        self.security_firewall = Some(firewall);
        self
    }

    /// OPT-12：注入共享的审计日志，启用 SQL 审计记录。
    ///
    /// 启用后，每个 session 的 `handle_query` 在执行 SQL 后记录审计事件，
    /// 事件包含 SQL 文本和执行结果，哈希链保证日志不可篡改。
    pub fn with_audit_log(
        mut self,
        audit: Arc<tokio::sync::Mutex<szrsql_security::audit::AuditLog>>,
    ) -> Self {
        self.audit_log = Some(audit);
        self
    }

    /// P0-1：注入共享的 TDE 透明页级加密引擎。
    ///
    /// 启用后，WAL FPI 记录写入前加密、读取时解密，防止磁盘镜像被离线读取。
    pub fn with_tde_engine(
        mut self,
        tde: Arc<tokio::sync::Mutex<szrsql_security::tde::TdeEngine>>,
    ) -> Self {
        self.tde_engine = Some(tde);
        self
    }

    /// P0-1：注入共享的列加密引擎。
    ///
    /// 启用后，executor 对配置为加密的列在写入时加密、读取时解密。
    pub fn with_column_encryption_engine(
        mut self,
        engine: Arc<tokio::sync::Mutex<szrsql_security::column_enc::ColumnEncryptionEngine>>,
    ) -> Self {
        self.column_encryption_engine = Some(engine);
        self
    }

    /// P0-1：注入共享的数据脱敏引擎。
    ///
    /// 启用后，`handle_query` 在编码 SELECT 结果时对命中规则的列脱敏。
    pub fn with_masking_engine(
        mut self,
        engine: Arc<tokio::sync::Mutex<szrsql_security::masking::MaskingEngine>>,
    ) -> Self {
        self.masking_engine = Some(engine);
        self
    }

    /// P0-1：注入共享的密码策略注册表。
    ///
    /// 启用后，CREATE ROLE / ALTER ROLE 修改密码时按注册的策略校验。
    pub fn with_password_profile_registry(
        mut self,
        registry: Arc<
            tokio::sync::Mutex<szrsql_security::password_profile::PasswordProfileRegistry>,
        >,
    ) -> Self {
        self.password_profile_registry = Some(registry);
        self
    }

    /// 生产监控告警：注入共享的 Prometheus 指标注册表。
    ///
    /// 启用后，连接建立/断开、查询执行、事务提交/回滚等关键事件会更新对应计数器，
    /// 通过 HTTP `/metrics` 端点暴露 Prometheus 文本格式指标。
    ///
    /// 通常在 main.rs 中创建一个 `Arc<MetricsRegistry>` 实例，同时注入
    /// `PgwireServer`（用于计数）和 `HttpServer`（用于暴露 `/metrics`）。
    pub fn with_metrics(mut self, metrics: Arc<crate::http::MetricsRegistry>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// OPT-13：注入会话取消注册表，启用 HTTP `/api/v1/cancel/{pid}` 端点的真实取消逻辑。
    ///
    /// 启用后，每个连接在主循环等待消息时通过 `tokio::select!` 监听取消信号。
    /// HTTP 端点调用 `Notify::notify_one()` 触发取消，连接发送 ErrorResponse 后继续。
    pub fn with_cancel_registry(
        mut self,
        registry: Arc<std::sync::Mutex<HashMap<i32, Arc<tokio::sync::Notify>>>>,
    ) -> Self {
        self.cancel_registry = Some(registry);
        self
    }

    /// P2-2.2：注入跨会话共享的流复制主库实例。
    ///
    /// 启用后，每个 session 在事务 COMMIT 成功后将 WAL 记录推送到 `ReplicationPrimary`，
    /// 由后者扇出到所有已连接的 TCP 备库（通过 `TcpReplicationServer`）。
    ///
    /// 未启用时（None）跳过复制推送（旧行为，用于单节点模式或测试兼容）。
    ///
    /// # 参数
    /// - `primary`：共享的 `ReplicationPrimary` 实例（由 main.rs 创建一次，
    ///   同时传给 `TcpReplicationServer` 用于接受备库连接）
    pub fn with_replication_primary(mut self, primary: Arc<ReplicationPrimary>) -> Self {
        self.replication_primary = Some(primary);
        self
    }

    /// 返回服务器配置引用。
    pub fn config(&self) -> &PgwireConfig {
        &self.config
    }

    /// Phase 4.6：返回服务器共享的 `NotifyHub` 引用。
    ///
    /// 用于测试场景中预先注册监听者或检查通知投递状态。
    pub fn notify_hub(&self) -> &NotifyHub {
        &self.notify_hub
    }

    /// Phase 4.11：返回关闭协调器引用。
    ///
    /// 用于外部触发优雅关闭（调用 `shutdown().await`）。
    pub fn shutdown_coordinator(&self) -> &ShutdownCoordinator {
        &self.shutdown
    }

    /// 启动服务器并阻塞当前任务接受连接。
    ///
    /// 每个连接在独立 tokio task 中处理，互不影响。
    ///
    /// **注意**：此方法不会响应外部关闭信号，服务器将一直运行直到 accept 失败。
    /// 如需优雅关闭，请使用 [`serve_with_shutdown`](Self::serve_with_shutdown)。
    pub async fn serve(self) -> Result<(), ServerError> {
        // 使用一个永不触发的 shutdown future，保持向后兼容
        let (_tx, rx) = tokio::sync::oneshot::channel::<ShutdownSignal>();
        let never_shutdown = async move {
            let _ = rx.await;
            ShutdownSignal::Graceful
        };
        self.serve_with_shutdown(never_shutdown).await
    }

    /// Phase 4.11：启动服务器，接受连接直到 `shutdown` future 完成。
    /// Phase 4.12：`shutdown` future 返回 `ShutdownSignal` 区分优雅/立即关闭。
    ///
    /// 当 `shutdown` future 完成时：
    /// 1. 停止接受新连接（新连接立即被拒绝并返回 "shutting down" 错误）
    /// 2. 根据信号类型执行关闭：
    ///    - `ShutdownSignal::Graceful`（SIGTERM）：等待活跃连接完成，最多 `config.shutdown_timeout`
    ///    - `ShutdownSignal::Immediate`（SIGINT/Ctrl+C）：立即 abort_all，不等待
    /// 3. 返回 `Ok(())`（退出码 0）
    ///
    /// # 参数
    ///
    /// - `shutdown`：触发关闭的 future，返回 `ShutdownSignal`。例如：
    ///   - SIGTERM（优雅）：`tokio::signal::unix::signal(SignalKind::terminate())` → `ShutdownSignal::Graceful`
    ///   - SIGINT/Ctrl+C（立即）：`tokio::signal::ctrl_c()` → `ShutdownSignal::Immediate`
    ///   - 外部 `oneshot::Receiver<ShutdownSignal>`
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let server = PgwireServer::new(config);
    /// server.serve_with_shutdown(async {
    ///     tokio::signal::ctrl_c().await.ok();
    ///     ShutdownSignal::Immediate
    /// }).await?;
    /// ```
    pub async fn serve_with_shutdown<F>(self, shutdown: F) -> Result<(), ServerError>
    where
        F: std::future::Future<Output = ShutdownSignal>,
    {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("pgwire server listening on {addr}");

        let inner = Arc::new(self);
        let tasks = inner.shutdown.tasks();

        // 将 shutdown future 包装为共享 future，以便在 select 中使用
        let shutdown = Box::pin(shutdown);

        // accept 循环：与 shutdown signal 竞争
        tokio::pin!(shutdown);

        let signal: ShutdownSignal = loop {
            tokio::select! {
                biased; // 优先检查 shutdown signal

                s = &mut shutdown => {
                    tracing::info!(?s, "shutdown signal received, stopping accept loop");
                    break s;
                }

                accept_result = listener.accept() => {
                    match accept_result {
                        Ok((mut stream, peer)) => {
                            tracing::debug!("accepted connection from {peer}");

                            // Phase 4.11：关闭中拒绝新连接
                            if inner.shutdown.is_rejecting() {
                                tracing::warn!(
                                    peer = %peer,
                                    "rejecting new connection during shutdown"
                                );
                                // 发送 FATAL 错误并关闭连接
                                let mut resp = BytesMut::new();
                                build_protocol_error_response(
                                    "FATAL: server is shutting down",
                                    &mut resp,
                                );
                                let _ = stream.write_all(&resp).await;
                                let _ = stream.flush().await;
                                continue;
                            }

                            // 连接数硬限制：尝试获取信号量许可
                            let permit = match inner.conn_semaphore.clone().try_acquire_owned() {
                                Ok(p) => p,
                                Err(_) => {
                                    tracing::warn!(
                                        peer = %peer,
                                        max = inner.config.max_connections,
                                        "rejecting connection: too many connections"
                                    );
                                    let mut resp = BytesMut::new();
                                    build_protocol_error_response(
                                        "FATAL: too many connections for role \"szrsql\"",
                                        &mut resp,
                                    );
                                    let _ = stream.write_all(&resp).await;
                                    let _ = stream.flush().await;
                                    continue;
                                }
                            };

                            let inner = Arc::clone(&inner);
                            let tasks = Arc::clone(&tasks);
                            // 生产监控告警：记录连接计数
                            if let Some(m) = &inner.metrics {
                                m.inc_connections();
                                m.inc_active_connections();
                            }
                            // Phase 4.11：用 JoinSet 跟踪连接任务，支持优雅排空
                            tasks.lock().await.spawn(async move {
                                // permit 在连接任务结束时自动 drop → 释放许可
                                let _permit = permit;
                                // 生产监控告警：连接结束时减少活跃连接数
                                let metrics_clone = inner.metrics.clone();
                                let result = inner.handle_connection(stream).await;
                                if let Err(e) = &result {
                                    tracing::warn!("connection handler error: {e}");
                                }
                                if let Some(m) = &metrics_clone {
                                    m.dec_active_connections();
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "accept failed");
                            return Err(ServerError::Io(e));
                        }
                    }
                }
            }
        };

        // Phase 4.12：根据信号类型执行对应的关闭策略
        let all_drained = inner.shutdown.shutdown_with_signal(signal).await;
        tracing::info!(all_drained, "pgwire server stopped");

        Ok(())
    }

    /// 处理单个客户端连接。
    ///
    /// Phase 4.5：在原 Phase 4.1 启动握手之前增加 SSL 协商阶段。
    /// 1. 读取首个启动消息以判断是否为 SSLRequest
    /// 2. 若为 SSLRequest 且配置了 TLS：回复 'S' → TLS 握手 → stream 升级
    /// 3. 若为 SSLRequest 但未配置 TLS：回复 'N' → 客户端回退明文
    /// 4. 继续启动握手与主循环
    pub async fn handle_connection(&self, mut stream: TcpStream) -> Result<(), ServerError> {
        let mut buf = BytesMut::with_capacity(1024);

        // 阶段 0：读取首个启动消息以判断是否需要 TLS 升级
        let first = self
            .read_first_startup_message(&mut stream, &mut buf)
            .await?;
        match first {
            FirstStartupMessage::None => Ok(()), // 客户端在握手前断开
            FirstStartupMessage::CancelRequest => {
                tracing::debug!("received CancelRequest, ignoring");
                Ok(())
            }
            FirstStartupMessage::GssencRequest => {
                // Phase 4.5 不支持 GSSAPI，回复 'N' 让客户端回退明文
                stream.write_all(b"N").await?;
                stream.flush().await?;
                self.handle_full_connection(stream, buf).await
            }
            FirstStartupMessage::SslRequest => {
                if let Some(tls) = &self.config.tls {
                    tracing::debug!("SSLRequest received, upgrading to TLS 1.3");
                    stream.write_all(b"S").await?;
                    stream.flush().await?;
                    let acceptor = tokio_rustls::TlsAcceptor::from(tls.server_config()?);
                    let tls_stream = acceptor.accept(stream).await?;
                    self.handle_full_connection(tls_stream, buf).await
                } else {
                    tracing::debug!("SSLRequest received but TLS not configured, refusing");
                    stream.write_all(b"N").await?;
                    stream.flush().await?;
                    self.handle_full_connection(stream, buf).await
                }
            }
            FirstStartupMessage::Startup => {
                // Phase 4.5：require_tls=true 时拒绝明文 StartupMessage
                if self.config.require_tls {
                    tracing::warn!(
                        "plaintext StartupMessage rejected: server requires TLS (require_tls=true)"
                    );
                    let mut resp = BytesMut::new();
                    build_protocol_error_response(
                        "SSLRequired: server requires SSL encryption",
                        &mut resp,
                    );
                    let _ = stream.write_all(&resp).await;
                    let _ = stream.flush().await;
                    return Ok(());
                }
                // 首个消息为 Startup（buf 中已包含未消费的 StartupMessage），直接处理
                self.handle_full_connection(stream, buf).await
            }
        }
    }

    /// Phase 4.5：完整连接处理（启动握手 + 主循环），接受泛型 stream。
    ///
    /// 既支持明文 `TcpStream`，也支持加密 `TlsStream<TcpStream>`。
    async fn handle_full_connection<S>(
        &self,
        stream: S,
        initial_buf: BytesMut,
    ) -> Result<(), ServerError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut stream = stream;
        let (startup_params, pid) = match self.handle_startup(&mut stream, initial_buf).await? {
            Some(p) => p,
            None => return Ok(()), // SSLRequest 等无需继续握手
        };

        // 阶段 2：主循环（处理 Query / Terminate）
        self.handle_main_loop(&mut stream, &startup_params, pid)
            .await
    }

    /// Phase 4.5：读取首个启动消息以判断类型（不消费 Startup/CancelRequest，仅消费 SSLRequest/GssencRequest）。
    async fn read_first_startup_message(
        &self,
        stream: &mut TcpStream,
        buf: &mut BytesMut,
    ) -> Result<FirstStartupMessage, ServerError> {
        loop {
            let mut tmp = buf.clone();
            match StartupMessage::decode(&mut tmp) {
                Ok(Some(msg)) => match msg {
                    StartupMessage::SslRequest => {
                        // 消费 SSLRequest
                        let consumed = buf.len() - tmp.len();
                        buf.advance(consumed);
                        return Ok(FirstStartupMessage::SslRequest);
                    }
                    StartupMessage::GssencRequest => {
                        let consumed = buf.len() - tmp.len();
                        buf.advance(consumed);
                        return Ok(FirstStartupMessage::GssencRequest);
                    }
                    StartupMessage::CancelRequest { .. } => {
                        let consumed = buf.len() - tmp.len();
                        buf.advance(consumed);
                        return Ok(FirstStartupMessage::CancelRequest);
                    }
                    StartupMessage::Startup(_) => {
                        // 不消费 buf，让 handle_startup 后续处理
                        return Ok(FirstStartupMessage::Startup);
                    }
                },
                Ok(None) => {
                    let mut chunk = [0u8; 4096];
                    let n = read_with_idle_timeout(
                        stream,
                        &mut chunk,
                        self.config.connection_idle_timeout,
                    )
                    .await?;
                    if n == 0 {
                        tracing::debug!("client disconnected before first startup message");
                        return Ok(FirstStartupMessage::None);
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                Err(e) => {
                    let mut resp = BytesMut::new();
                    build_protocol_error_response(&e.to_string(), &mut resp);
                    let _ = stream.write_all(&resp).await;
                    let _ = stream.flush().await;
                    return Err(ServerError::Startup(e));
                }
            }
        }
    }

    /// 处理启动握手阶段。
    ///
    /// 返回 `Ok(Some(params))` 表示握手成功，可进入主循环；
    /// 返回 `Ok(None)` 表示连接应关闭（如 SSL 请求被拒绝后客户端断开）。
    async fn handle_startup<S>(
        &self,
        stream: &mut S,
        initial_buf: BytesMut,
    ) -> Result<Option<(StartupParams, i32)>, ServerError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut buf = initial_buf;

        loop {
            // 尝试解码启动消息
            let mut tmp = buf.clone();
            match StartupMessage::decode(&mut tmp) {
                Ok(Some(msg)) => {
                    // 成功解码，advance 实际缓冲区
                    let consumed = buf.len() - tmp.len();
                    buf.advance(consumed);
                    match msg {
                        StartupMessage::Startup(params) => {
                            // 校验 user / database
                            if let Some(err) = self.validate_user_database(&params) {
                                let mut resp = BytesMut::new();
                                build_auth_error_response(&err, &mut resp);
                                let _ = stream.write_all(&resp).await;
                                let _ = stream.flush().await;
                                return Ok(None);
                            }

                            // Phase 4.4：执行认证（trust 或 scram-sha-256）
                            if !self
                                .perform_authentication(stream, &mut buf, &params)
                                .await?
                            {
                                return Ok(None);
                            }

                            // 生成 pid 和 secret_key
                            let pid = self.pid_counter.fetch_add(1, Ordering::SeqCst);
                            let secret_key = generate_secret_key(pid);

                            // 构造握手响应
                            let mut resp = BytesMut::new();
                            let app_name =
                                params.params.get("application_name").map(|s| s.as_str());
                            build_startup_response(
                                pid,
                                secret_key,
                                &self.config.server_version,
                                app_name,
                                &mut resp,
                            );
                            stream.write_all(&resp).await?;
                            stream.flush().await?;
                            // Phase 4.6：同时返回 pid，用于主循环中向 session 注入 NotifyHub
                            return Ok(Some((params, pid)));
                        }
                        StartupMessage::SslRequest | StartupMessage::GssencRequest => {
                            // Phase 4.5：SSL 协商已在 handle_connection 完成，
                            // 此处再收到 SSLRequest 视为客户端错误，回复 'N' 拒绝
                            tracing::warn!(
                                "received SSLRequest/GssencRequest after TLS negotiation, refusing"
                            );
                            stream.write_all(b"N").await?;
                            stream.flush().await?;
                            continue;
                        }
                        StartupMessage::CancelRequest { .. } => {
                            // Phase 4.1 暂不实现取消查询，直接关闭连接
                            tracing::debug!("received CancelRequest, ignoring");
                            return Ok(None);
                        }
                    }
                }
                Ok(None) => {
                    // 数据不足，继续读取
                    let mut chunk = [0u8; 4096];
                    let n = read_with_idle_timeout(
                        stream,
                        &mut chunk,
                        self.config.connection_idle_timeout,
                    )
                    .await?;
                    if n == 0 {
                        tracing::debug!("client disconnected during startup");
                        return Ok(None);
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                Err(e) => {
                    // 协议错误：发送 ErrorResponse 并关闭连接
                    let mut resp = BytesMut::new();
                    build_protocol_error_response(&e.to_string(), &mut resp);
                    let _ = stream.write_all(&resp).await;
                    let _ = stream.flush().await;
                    return Err(ServerError::Startup(e));
                }
            }
        }
    }

    /// Phase 4.4：执行认证流程。
    ///
    /// - Trust 模式：直接返回 `Ok(true)`（无需任何认证消息交换）
    /// - ScramSha256 模式：执行完整 SCRAM 握手
    ///
    /// 返回 `Ok(true)` 表示认证成功；`Ok(false)` 表示认证失败（已发送 ErrorResponse，
    /// 调用方应关闭连接）；`Err` 表示 IO 错误。
    async fn perform_authentication<S>(
        &self,
        stream: &mut S,
        buf: &mut BytesMut,
        _params: &StartupParams,
    ) -> Result<bool, ServerError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        match &self.config.auth_mode {
            AuthMode::Trust => Ok(true),
            AuthMode::ScramSha256 {
                credentials,
                salt,
                iterations,
            } => {
                // P2-14：若配置了共享凭据存储，优先使用运行时热重载后的凭据。
                // 未注入时回退到启动时快照（保持向后兼容）。
                let (credentials, salt, iterations) = match &self.config.shared_scram {
                    Some(shared) => {
                        let store = shared.current();
                        (store.credentials.clone(), store.salt(), store.iterations)
                    }
                    None => (credentials.clone(), salt.clone(), *iterations),
                };
                self.perform_scram_auth(stream, buf, &credentials, &salt, iterations)
                    .await
                    .map_err(ServerError::Io)
            }
        }
    }

    /// 执行 SCRAM-SHA-256 认证握手。
    ///
    /// 返回 `Ok(true)` 表示认证成功；返回 `Ok(false)` 表示认证失败（已发送 ErrorResponse）。
    async fn perform_scram_auth<S>(
        &self,
        stream: &mut S,
        buf: &mut BytesMut,
        credentials: &std::collections::HashMap<String, String>,
        salt: &[u8],
        iterations: u32,
    ) -> Result<bool, std::io::Error>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        // 步骤 1：发送 AuthenticationSASL（列出支持的机制）
        let mut resp = BytesMut::new();
        BackendMessage::AuthenticationSASL {
            mechanisms: vec![SCRAM_MECHANISM.to_string()],
        }
        .encode(&mut resp);
        stream.write_all(&resp).await?;
        stream.flush().await?;

        // 构造 SCRAM 会话
        let mut session = ScramServerSession::new(credentials.clone(), salt.to_vec(), iterations);

        // 步骤 2：等待客户端 SASLInitialResponse
        let initial_response =
            match read_frontend_message(stream, buf, self.config.connection_idle_timeout).await? {
                FrontendMessage::SASLInitialResponse {
                    mechanism,
                    initial_response,
                } => {
                    if mechanism != SCRAM_MECHANISM {
                        let err = ErrorResponse::fatal(
                            SqlState::INVALID_AUTHORIZATION_SPECIFICATION,
                            format!("unsupported SASL mechanism: {mechanism}"),
                        );
                        let mut resp = BytesMut::new();
                        BackendMessage::ErrorResponse(err).encode(&mut resp);
                        let _ = stream.write_all(&resp).await;
                        let _ = stream.flush().await;
                        return Ok(false);
                    }
                    match initial_response {
                        Some(data) => data,
                        None => {
                            let err = ErrorResponse::fatal(
                                SqlState::INVALID_AUTHORIZATION_SPECIFICATION,
                                "client did not provide initial SASL response",
                            );
                            let mut resp = BytesMut::new();
                            BackendMessage::ErrorResponse(err).encode(&mut resp);
                            let _ = stream.write_all(&resp).await;
                            let _ = stream.flush().await;
                            return Ok(false);
                        }
                    }
                }
                FrontendMessage::Terminate => {
                    tracing::debug!("client terminated during SCRAM auth");
                    return Ok(false);
                }
                other => {
                    tracing::warn!(msg = ?other, "unexpected message during SCRAM auth");
                    return Ok(false);
                }
            };

        // 步骤 3：处理 client-first，发送 AuthenticationSASLContinue
        match session.handle_client_first(&initial_response) {
            Ok(server_first) => {
                let mut resp = BytesMut::new();
                BackendMessage::AuthenticationSASLContinue { data: server_first }.encode(&mut resp);
                stream.write_all(&resp).await?;
                stream.flush().await?;
            }
            Err(auth_err) => {
                send_auth_error_response(stream, &auth_err).await?;
                return Ok(false);
            }
        }

        // 步骤 4：等待客户端 SASLResponse（client-final）
        let final_data =
            match read_frontend_message(stream, buf, self.config.connection_idle_timeout).await? {
                FrontendMessage::SASLResponse { data } => data,
                FrontendMessage::Terminate => {
                    tracing::debug!("client terminated during SCRAM auth (final stage)");
                    return Ok(false);
                }
                other => {
                    tracing::warn!(msg = ?other, "unexpected message during SCRAM auth final");
                    return Ok(false);
                }
            };

        // 步骤 5：验证 client-final，发送 AuthenticationSASLFinal + AuthenticationOk
        match session.handle_client_final(&final_data) {
            Ok(server_final) => {
                let mut resp = BytesMut::new();
                BackendMessage::AuthenticationSASLFinal { data: server_final }.encode(&mut resp);
                BackendMessage::AuthenticationOk.encode(&mut resp);
                stream.write_all(&resp).await?;
                stream.flush().await?;
                tracing::info!(
                    user = session.username(),
                    "SCRAM-SHA-256 authentication succeeded"
                );
                Ok(true)
            }
            Err(auth_err) => {
                send_auth_error_response(stream, &auth_err).await?;
                Ok(false)
            }
        }
    }

    /// 校验 user / database 是否在允许列表中。
    fn validate_user_database(&self, params: &StartupParams) -> Option<String> {
        let _ = params; // 暂时不强制校验
        let _ = &self.config.allowed_users;
        let _ = &self.config.allowed_databases;
        None
    }

    /// 处理主循环：接收前端消息并响应。
    async fn handle_main_loop<S>(
        &self,
        stream: &mut S,
        params: &StartupParams,
        pid: i32,
    ) -> Result<(), ServerError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut buf = BytesMut::with_capacity(8192);
        // Phase 4.2：每个连接持有一个 ExecutorService，维护 catalog / tables / 事务状态
        // Phase 4.6：注入 pid 和共享 NotifyHub，使 LISTEN/NOTIFY 跨会话广播
        // ADV-F-7：注入共享 WalWriter，启用 log-then-commit 事务模型
        // ADV-CONC-1：注入共享表存储 + 行锁管理器，启用多线程并发
        // Phase 4.7：注入 database 名（来自 StartupParams，缺省 "szrsql"），供 pg_database 系统表查询
        let db_name = params.database().unwrap_or("szrsql");
        // 简单查询协议启用多语句执行（PostgreSQL 协议规范允许，Navicat/psql 等客户端连接时
        // 会发送 "SET AUTOCOMMIT ON; SET extra_float_digits = 3" 这样的多语句）。
        // 扩展查询协议（Parse/Bind/Execute）不受影响，仍强制单语句（PG 协议要求）。
        let mut session = ExecutorService::new()
            .with_pid(pid)
            .with_database_name(db_name)
            .with_notify_hub(self.notify_hub.clone())
            .with_multi_statement(true);
        if let Some(writer) = &self.wal_writer {
            session = session.with_wal_writer(writer.clone());
        }
        if let (Some(shared), Some(lm)) = (&self.shared_tables, &self.lock_manager) {
            session = session
                .with_shared_tables(shared.clone())
                .with_lock_manager(lm.clone());
            if let Some(counter) = &self.shared_txn_counter {
                session = session.with_shared_txn_counter(counter.clone());
            }
        }
        // P0-4：注入跨会话共享的序列全局状态
        if let Some(seq_state) = &self.shared_sequence_state {
            session = session.with_sequence_shared_state(seq_state.clone());
        }
        // P0-TX-1：注入 MVCC 事务管理器
        if let Some(mvcc) = &self.mvcc {
            session = session.with_mvcc(mvcc.clone());
        }
        // P0-DIST-1/2/3：注入分布式运行时句柄
        if let Some(dist_rt) = &self.dist_runtime {
            session = session.with_dist_runtime(dist_rt.clone());
        }
        // P7-1：注入 CDC 引擎，启用 DML 事件分发
        if let Some(cdc) = &self.cdc_engine {
            session = session.with_cdc_engine(cdc.clone());
        }
        // P2-1：注入 HLC 时钟和冲突日志，启用 Multi-Master 因果排序和冲突审计
        if let Some(hlc) = &self.hlc_clock {
            session = session.with_hlc_clock(hlc.clone());
        }
        if let Some(log) = &self.conflict_log {
            session = session.with_conflict_log(log.clone());
        }
        session = session.with_node_id(self.node_id);
        // P1-2：注入脏表跟踪器，启用增量快照机制
        if let Some(tracker) = &self.dirty_tracker {
            session = session.with_dirty_tracker(tracker.clone());
        }
        // P2-1.1：注入统计信息存储，启用 ANALYZE 命令
        if let Some(stats) = &self.statistics_store {
            session = session.with_statistics_store(stats.clone());
        }
        // P2-2.2：注入流复制主库实例，启用 COMMIT 后 WAL 记录推送到备库
        if let Some(primary) = &self.replication_primary {
            session = session.with_replication_primary(primary.clone());
        }
        // Phase 4.6：RAII 守卫，确保连接断开时从 NotifyHub 注销（避免内存泄漏）
        let _notify_guard = NotifyCleanupGuard::new(self.notify_hub.clone(), pid);
        // OPT-13：注册会话取消信号，HTTP /api/v1/cancel/{pid} 可触发
        let cancel_notify = self.cancel_registry.as_ref().map(|registry| {
            let notify = Arc::new(tokio::sync::Notify::new());
            let notify_clone = Arc::clone(&notify);
            if let Ok(mut map) = registry.lock() {
                map.insert(pid, notify_clone);
            }
            notify
        });
        // RAII 守卫：连接断开时从 cancel_registry 注销（避免内存泄漏）
        let _cancel_guard = CancelRegistryGuard::new(self.cancel_registry.clone(), pid);
        // Phase 4.3：扩展查询错误后的 "aborted" 状态，需等待 Sync 才能继续
        let mut extended_aborted = false;

        loop {
            // 尝试解码前端消息
            let mut tmp = buf.clone();
            match FrontendMessage::decode(&mut tmp) {
                Ok(Some(msg)) => {
                    let consumed = buf.len() - tmp.len();
                    buf.advance(consumed);

                    // Phase 4.3：aborted 状态下仅 Sync/Flush 有效
                    if extended_aborted {
                        match msg {
                            FrontendMessage::Sync => {
                                extended_aborted = false;
                                let mut resp = BytesMut::new();
                                let status = self.session_status(&session);
                                BackendMessage::ReadyForQuery { status }.encode(&mut resp);
                                stream.write_all(&resp).await?;
                                stream.flush().await?;
                            }
                            FrontendMessage::Flush => {
                                stream.flush().await?;
                            }
                            FrontendMessage::Terminate => {
                                tracing::debug!("client sent Terminate during aborted state");
                                return Ok(());
                            }
                            // 其他消息在 aborted 状态下被忽略（PG 行为）
                            _ => tracing::debug!(
                                msg = ?msg,
                                "ignoring message in aborted extended query state"
                            ),
                        }
                        continue;
                    }

                    match msg {
                        FrontendMessage::Query { sql } => {
                            self.handle_query(stream, &sql, &mut session).await?;
                        }
                        FrontendMessage::Terminate => {
                            tracing::debug!("client sent Terminate, closing connection");
                            return Ok(());
                        }
                        FrontendMessage::Parse {
                            statement_name,
                            sql,
                            parameter_oids,
                        } => {
                            // ADV-CONC-1：Parse 之前同步共享 catalog，确保后续 Describe 能推导列
                            session.sync_catalog_from_shared().await;
                            let mut resp = BytesMut::new();
                            match session.extended_parse(&statement_name, &sql, parameter_oids) {
                                Ok(()) => {
                                    BackendMessage::ParseComplete.encode(&mut resp);
                                }
                                Err(e) => {
                                    self.encode_session_error(&e, &mut resp);
                                    extended_aborted = true;
                                }
                            }
                            stream.write_all(&resp).await?;
                            stream.flush().await?;
                        }
                        FrontendMessage::Bind {
                            portal_name,
                            statement_name,
                            parameter_format_codes,
                            parameters,
                            result_format_codes,
                        } => {
                            let mut resp = BytesMut::new();
                            match session.extended_bind(
                                &portal_name,
                                &statement_name,
                                &parameter_format_codes,
                                &parameters,
                                result_format_codes,
                            ) {
                                Ok(()) => {
                                    BackendMessage::BindComplete.encode(&mut resp);
                                }
                                Err(e) => {
                                    self.encode_session_error(&e, &mut resp);
                                    extended_aborted = true;
                                }
                            }
                            stream.write_all(&resp).await?;
                            stream.flush().await?;
                        }
                        FrontendMessage::Execute {
                            portal_name,
                            max_rows,
                        } => {
                            let mut resp = BytesMut::new();
                            match session.extended_execute(&portal_name, max_rows).await {
                                Ok(result) => {
                                    self.encode_extended_execute_result(&result, &mut resp);
                                }
                                Err(e) => {
                                    self.encode_session_error(&e, &mut resp);
                                    extended_aborted = true;
                                }
                            }
                            // Phase 4.6：Execute 后也发送待处理通知
                            // （NOTIFY 通过扩展查询执行时，通知需在 Execute 响应后投递）
                            self.encode_pending_notifications(&mut session, &mut resp);
                            stream.write_all(&resp).await?;
                            stream.flush().await?;
                        }
                        FrontendMessage::Describe { variant, name } => {
                            let mut resp = BytesMut::new();
                            match variant {
                                b'S' => {
                                    match session.extended_describe_statement(&name) {
                                        Ok(desc) => {
                                            // ParameterDescription
                                            BackendMessage::ParameterDescription {
                                                parameter_oids: desc.parameter_oids.clone(),
                                            }
                                            .encode(&mut resp);
                                            // RowDescription 或 NoData
                                            // Describe statement 时 portal 尚未创建，
                                            // 客户端未声明格式码 → 全部按 text 编码（PG 行为）
                                            if desc.result_columns.is_empty() {
                                                BackendMessage::NoData.encode(&mut resp);
                                            } else {
                                                encode_row_description(
                                                    &desc.result_columns,
                                                    &[],
                                                    &mut resp,
                                                );
                                            }
                                        }
                                        Err(e) => {
                                            self.encode_session_error(&e, &mut resp);
                                            extended_aborted = true;
                                        }
                                    }
                                }
                                b'P' => match session.extended_describe_portal(&name) {
                                    Ok(desc) => {
                                        if desc.result_columns.is_empty() {
                                            BackendMessage::NoData.encode(&mut resp);
                                        } else {
                                            // Describe portal 时使用 Bind 中声明的格式码，
                                            // 与 Execute 时实际使用的格式码保持一致
                                            encode_row_description(
                                                &desc.result_columns,
                                                &desc.result_format_codes,
                                                &mut resp,
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        self.encode_session_error(&e, &mut resp);
                                        extended_aborted = true;
                                    }
                                },
                                other => {
                                    let err = ErrorResponse::error(
                                        SqlState::PROTOCOL_VIOLATION,
                                        format!("invalid Describe variant: 0x{:02X}", other),
                                    );
                                    BackendMessage::ErrorResponse(err).encode(&mut resp);
                                    extended_aborted = true;
                                }
                            }
                            stream.write_all(&resp).await?;
                            stream.flush().await?;
                        }
                        FrontendMessage::Close { variant, name } => {
                            let mut resp = BytesMut::new();
                            match session.extended_close(variant, &name) {
                                Ok(()) => {
                                    BackendMessage::CloseComplete.encode(&mut resp);
                                }
                                Err(e) => {
                                    self.encode_session_error(&e, &mut resp);
                                    extended_aborted = true;
                                }
                            }
                            stream.write_all(&resp).await?;
                            stream.flush().await?;
                        }
                        FrontendMessage::Sync => {
                            let mut resp = BytesMut::new();
                            // Phase 4.6：Sync 时也发送待处理通知（与 Query 一致）
                            self.encode_pending_notifications(&mut session, &mut resp);
                            let status = self.session_status(&session);
                            BackendMessage::ReadyForQuery { status }.encode(&mut resp);
                            stream.write_all(&resp).await?;
                            stream.flush().await?;
                        }
                        FrontendMessage::Flush => {
                            // Flush 仅刷新缓冲，不发送 ReadyForQuery
                            // Phase 4.6：Flush 时也发送待处理通知（PG 行为：Flush 强制刷新缓冲）
                            let mut resp = BytesMut::new();
                            self.encode_pending_notifications(&mut session, &mut resp);
                            if !resp.is_empty() {
                                stream.write_all(&resp).await?;
                            }
                            stream.flush().await?;
                        }
                        FrontendMessage::SASLInitialResponse { .. }
                        | FrontendMessage::SASLResponse { .. } => {
                            // SASL 消息只应在认证阶段发送；进入主循环后视为协议错误
                            let err = ErrorResponse::fatal(
                                SqlState::PROTOCOL_VIOLATION,
                                "SASL message received outside authentication phase",
                            );
                            let mut resp = BytesMut::new();
                            BackendMessage::ErrorResponse(err).encode(&mut resp);
                            stream.write_all(&resp).await?;
                            stream.flush().await?;
                            return Ok(());
                        }
                    }
                }
                Ok(None) => {
                    // 数据不足，继续读取
                    let mut chunk = [0u8; 4096];
                    // OPT-13：在等待数据时同时监听取消信号
                    let read_result = if let Some(notify) = &cancel_notify {
                        tokio::select! {
                            n = read_with_idle_timeout(
                                stream,
                                &mut chunk,
                                self.config.connection_idle_timeout,
                            ) => n,
                            _ = notify.notified() => {
                                tracing::info!(pid, "OPT-13: query cancelled by HTTP request");
                                let err = ErrorResponse::error(
                                    SqlState::INTERNAL_ERROR,
                                    "canceling statement due to user request",
                                );
                                let mut resp = BytesMut::new();
                                BackendMessage::ErrorResponse(err).encode(&mut resp);
                                let status = self.session_status(&session);
                                BackendMessage::ReadyForQuery { status }.encode(&mut resp);
                                let _ = stream.write_all(&resp).await;
                                let _ = stream.flush().await;
                                continue;
                            }
                        }
                    } else {
                        read_with_idle_timeout(
                            stream,
                            &mut chunk,
                            self.config.connection_idle_timeout,
                        )
                        .await
                    };
                    let n = read_result?;
                    if n == 0 {
                        tracing::debug!("client disconnected");
                        return Ok(());
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                Err(e) => {
                    // 协议错误：发送 ErrorResponse 并准备关闭
                    let err = ErrorResponse::error(
                        SqlState::PROTOCOL_VIOLATION,
                        format!("protocol error: {e}"),
                    );
                    let mut resp = BytesMut::new();
                    BackendMessage::ErrorResponse(err).encode(&mut resp);
                    let _ = stream.write_all(&resp).await;
                    let _ = stream.flush().await;
                    return Err(ServerError::Io(e));
                }
            }
        }
    }

    /// 处理简单查询（Phase 4.2：接入真实 SQL 执行器）。
    ///
    /// 简单查询协议允许在一条 Query 中包含多条以 `;` 分隔的 SQL 语句，
    /// 每条语句产生独立的响应序列：
    /// - ResultSet → RowDescription + DataRow* + CommandComplete
    /// - AffectedRows → CommandComplete
    /// - DdlComplete → CommandComplete
    /// - Empty → EmptyQueryResponse
    /// - TransactionComplete → CommandComplete
    ///
    /// 最后追加一条 ReadyForQuery 标识事务状态。
    async fn handle_query<S>(
        &self,
        stream: &mut S,
        sql: &str,
        session: &mut ExecutorService,
    ) -> Result<(), ServerError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        tracing::debug!(sql = sql.trim(), "received query");

        // 生产监控告警：查询计数 +1
        if let Some(m) = &self.metrics {
            m.inc_queries();
        }

        // OPT-12：SQL 防火墙检查（注入检测 + 禁止命令 + 白名单）
        // 命中防火墙规则的 SQL 直接返回 ERROR，不执行
        if let Some(firewall) = &self.security_firewall {
            let mut fw = firewall.lock().await;
            match fw.check(sql) {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(error = %e, sql = sql.trim(), "OPT-12: SQL blocked by firewall");
                    if let Some(m) = &self.metrics {
                        m.inc_errors();
                    }
                    let err = ErrorResponse::error(
                        SqlState::SYNTAX_ERROR,
                        format!("SQL blocked by firewall: {e}"),
                    );
                    let mut resp = BytesMut::new();
                    BackendMessage::ErrorResponse(err).encode(&mut resp);
                    stream.write_all(&resp).await?;
                    let mut ready = BytesMut::new();
                    BackendMessage::ReadyForQuery {
                        status: STATUS_IDLE,
                    }
                    .encode(&mut ready);
                    stream.write_all(&ready).await?;
                    return Ok(());
                }
            }
        }

        let mut results = session.execute_sql(sql).await;

        // P0-1：数据脱敏（在结果集编码前对命中规则的列应用脱敏规则）
        // 仅对 ResultSet 类型生效，根据 SQL 中提取的表名 + 列名匹配 MaskingPolicy。
        self.apply_masking_to_results(sql, &mut results);

        // OPT-12：审计日志记录（SQL 执行结果）
        if let Some(audit) = &self.audit_log {
            let mut audit_log = audit.lock().await;
            let success = results.iter().all(|r| r.is_ok());
            let event = szrsql_security::audit::AuditEvent::builder()
                .detail(sql.to_string())
                .command(if success {
                    szrsql_security::audit::AuditCommand::Other("QUERY".to_string())
                } else {
                    szrsql_security::audit::AuditCommand::Other("QUERY_FAILED".to_string())
                })
                .build();
            let _ = audit_log.record(event);
        }

        let mut resp = BytesMut::new();
        for result in results {
            match result {
                Ok(query_result) => {
                    // 生产监控告警：统计事务提交/回滚
                    if let Some(m) = &self.metrics {
                        if let QueryResult::TransactionComplete { tag, .. } = &query_result {
                            let upper = tag.to_uppercase();
                            if upper.starts_with("COMMIT") {
                                m.inc_commits();
                            } else if upper.starts_with("ROLLBACK") {
                                m.inc_rollbacks();
                            }
                        }
                    }
                    // 简单查询协议始终使用 text 格式（PG 协议规范）
                    self.encode_query_result(&query_result, &[], &mut resp);
                }
                Err(e) => {
                    // 生产监控告警：错误计数 +1
                    if let Some(m) = &self.metrics {
                        m.inc_errors();
                    }
                    self.encode_session_error(&e, &mut resp);
                    // 出错后停止本批次后续语句的响应（与 PG 一致）
                    break;
                }
            }
        }

        // Phase 4.6：在 ReadyForQuery 之前发送待处理的 NotificationResponse。
        // PG 协议规范：通知可在任何时间发送，但通常紧跟在查询响应之后。
        self.encode_pending_notifications(session, &mut resp);

        // 发送 ReadyForQuery 标识事务状态
        let status = match session.transaction_state() {
            TransactionState::Idle => STATUS_IDLE,
            TransactionState::InTransaction => STATUS_IN_TRANSACTION,
            TransactionState::InFailedTransaction => STATUS_IN_FAILED_TRANSACTION,
        };
        BackendMessage::ReadyForQuery { status }.encode(&mut resp);

        stream.write_all(&resp).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Phase 4.6：将 session 中所有待发送的通知编码为 `NotificationResponse` 消息。
    ///
    /// 通知在 `ReadyForQuery` 之前发送（PG 协议规范允许任意时机，但实现中通常
    /// 在响应完当前查询后、ReadyForQuery 之前批量发送）。
    fn encode_pending_notifications(&self, session: &mut ExecutorService, dst: &mut BytesMut) {
        for n in session.drain_pending_notifications() {
            BackendMessage::NotificationResponse {
                pid: n.notifier_pid,
                channel: n.channel,
                payload: n.payload,
            }
            .encode(dst);
        }
    }

    /// 将 `QueryResult` 编码为 pgwire 消息序列。
    ///
    /// `format_codes` 来自客户端 Bind 请求：
    /// - 简单查询协议调用方传 `&[]`（始终 text）
    /// - 扩展查询协议调用方传 portal 的 `result_format_codes`
    fn encode_query_result(&self, result: &QueryResult, format_codes: &[i16], dst: &mut BytesMut) {
        match result {
            QueryResult::ResultSet { columns, rows, tag } => {
                // RowDescription
                encode_row_description(columns, format_codes, dst);
                // DataRow*
                for row in rows {
                    encode_data_row(row, columns, format_codes, dst);
                }
                // CommandComplete
                BackendMessage::CommandComplete { tag: tag.clone() }.encode(dst);
            }
            QueryResult::AffectedRows { tag } => {
                BackendMessage::CommandComplete { tag: tag.clone() }.encode(dst);
            }
            QueryResult::DdlComplete { tag } => {
                BackendMessage::CommandComplete { tag: tag.clone() }.encode(dst);
            }
            QueryResult::Empty => {
                BackendMessage::EmptyQueryResponse.encode(dst);
            }
            QueryResult::TransactionComplete { tag, .. } => {
                BackendMessage::CommandComplete { tag: tag.clone() }.encode(dst);
            }
        }
    }

    /// 将扩展查询 `Execute` 的结果编码为 pgwire 消息序列。
    ///
    /// - `Complete { ResultSet, .. }` → RowDescription + DataRow* + CommandComplete
    /// - `Complete { AffectedRows/DdlComplete/Empty/TransactionComplete, .. }` → CommandComplete 等
    /// - `Suspended` → RowDescription + DataRow*（前 max_rows 行）+ PortalSuspended
    /// - `Transaction` → CommandComplete
    fn encode_extended_execute_result(&self, result: &ExtendedExecuteResult, dst: &mut BytesMut) {
        match result {
            ExtendedExecuteResult::Complete {
                result,
                result_format_codes,
            } => {
                self.encode_query_result(result, result_format_codes, dst);
            }
            ExtendedExecuteResult::Suspended {
                columns,
                rows,
                result_format_codes,
            } => {
                // RowDescription（仅一次，由首次 Execute 发送）
                encode_row_description(columns, result_format_codes, dst);
                for row in rows {
                    encode_data_row(row, columns, result_format_codes, dst);
                }
                BackendMessage::PortalSuspended.encode(dst);
            }
            ExtendedExecuteResult::Transaction(query_result) => {
                // 事务控制语句（BEGIN/COMMIT/ROLLBACK）无结果集，使用 text 格式
                self.encode_query_result(query_result, &[], dst);
            }
        }
    }

    /// 返回当前会话的 ReadyForQuery status 字节。
    fn session_status(&self, session: &ExecutorService) -> u8 {
        match session.transaction_state() {
            TransactionState::Idle => STATUS_IDLE,
            TransactionState::InTransaction => STATUS_IN_TRANSACTION,
            TransactionState::InFailedTransaction => STATUS_IN_FAILED_TRANSACTION,
        }
    }

    /// 将 `SessionError` 编码为 ErrorResponse。
    fn encode_session_error(&self, e: &SessionError, dst: &mut BytesMut) {
        let err = ErrorResponse::error(e.sqlstate(), e.to_string());
        BackendMessage::ErrorResponse(err).encode(dst);
    }

    /// P0-1：对查询结果应用数据脱敏。
    ///
    /// 流程：
    /// 1. 从 SQL 文本中提取主表名（FROM/UPDATE/INTO/JOIN 后的标识符）
    /// 2. 对每个 ResultSet，遍历每行每列：
    ///    - 若 masking_engine 已注册该 (table, column) 策略，则替换 Text 值为脱敏后值
    ///    - 其他类型值不变（脱敏仅对文本生效；非文本列应先 CAST 为 TEXT 才能脱敏）
    /// 3. 使用 admin context（角色为空）→ 任何注册策略都生效
    ///    （未来可从 session.user_data 注入实际用户角色）
    ///
    /// 性能：单次 SQL 解析 + O(rows × columns) 检查；未注册策略时 fast-path 跳过。
    fn apply_masking_to_results(
        &self,
        sql: &str,
        results: &mut [Result<QueryResult, SessionError>],
    ) {
        let masking = match &self.masking_engine {
            Some(m) => m.clone(),
            None => return,
        };

        // 提取 SQL 主表名（单条 SQL 共享一个表名上下文）
        let table_name = match extract_main_table_name(sql) {
            Some(t) => t,
            None => return,
        };

        // 使用 admin context（roles=[] 表示无授权角色，因此所有策略都生效）
        // 未来可从 session 提取实际角色列表
        let ctx = szrsql_security::masking::MaskingContext::unauthorized("anonymous");

        // 对每个 ResultSet 应用脱敏（同步锁，避免跨 await 持锁）
        // tokio::sync::Mutex::blocking_lock 直接返回 MutexGuard（非 Result）
        let mut engine = masking.blocking_lock();

        for result in results.iter_mut() {
            if let Ok(QueryResult::ResultSet { columns, rows, .. }) = result {
                // 预计算需要脱敏的列索引
                let masked_cols: Vec<(usize, String)> = columns
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| {
                        if engine.is_masked(&table_name, &c.name) {
                            Some((i, c.name.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();

                if masked_cols.is_empty() {
                    continue;
                }

                // 对每行的对应列应用脱敏
                for row in rows.iter_mut() {
                    for (idx, col_name) in &masked_cols {
                        if let Some(Value::Text(text)) = row.get_mut(*idx) {
                            let masked = engine.mask_value(&table_name, col_name, text, &ctx);
                            *text = masked;
                        }
                    }
                }
            }
        }
    }

    /// P0-1：使用密码策略注册表校验密码。
    ///
    /// 公开 API，供未来 CREATE ROLE / ALTER ROLE 路径调用。
    /// 返回 Ok(()) 表示密码符合 `default` profile；Err 包含具体失败原因。
    pub async fn validate_password(&self, password: &str) -> Result<(), ServerError> {
        let registry = match &self.password_profile_registry {
            Some(r) => r.clone(),
            None => return Ok(()),
        };
        let guard = registry.lock().await;
        match guard.get("default") {
            Some(profile) => profile.validate(password).map_err(|e| {
                ServerError::Startup(StartupError::InvalidMessage(format!(
                    "password policy violation: {e}"
                )))
            }),
            None => Ok(()),
        }
    }

    /// P0-1：使用 TDE 引擎加密页数据。
    ///
    /// 公开 API，供 WalWriter 在写入 FPI 记录前调用。
    /// 返回加密后的字节；未启用 TDE 时返回原始字节。
    pub async fn encrypt_page(
        &self,
        page_id: u64,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, ServerError> {
        let tde = match &self.tde_engine {
            Some(t) => t.clone(),
            None => return Ok(plaintext.to_vec()),
        };
        let mut guard = tde.lock().await;
        guard.encrypt_page(page_id, plaintext).map_err(|e| {
            ServerError::Startup(StartupError::InvalidMessage(format!(
                "tde encrypt failed: {e}"
            )))
        })
    }

    /// P0-1：使用 TDE 引擎解密页数据。
    pub async fn decrypt_page(
        &self,
        page_id: u64,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, ServerError> {
        let tde = match &self.tde_engine {
            Some(t) => t.clone(),
            None => return Ok(ciphertext.to_vec()),
        };
        let mut guard = tde.lock().await;
        guard.decrypt_page(page_id, ciphertext).map_err(|e| {
            ServerError::Startup(StartupError::InvalidMessage(format!(
                "tde decrypt failed: {e}"
            )))
        })
    }

    /// P0-1：使用列加密引擎加密列值。
    ///
    /// 公开 API，供 executor 在 INSERT 配置列时调用。
    pub async fn encrypt_column_value(
        &self,
        table: &str,
        column: &str,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, ServerError> {
        let engine = match &self.column_encryption_engine {
            Some(e) => e.clone(),
            None => return Ok(plaintext.to_vec()),
        };
        let mut guard = engine.lock().await;
        guard.encrypt(table, column, plaintext).map_err(|e| {
            ServerError::Startup(StartupError::InvalidMessage(format!(
                "column encrypt failed: {e}"
            )))
        })
    }

    /// P0-1：使用列加密引擎解密列值。
    pub async fn decrypt_column_value(
        &self,
        table: &str,
        column: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, ServerError> {
        let engine = match &self.column_encryption_engine {
            Some(e) => e.clone(),
            None => return Ok(ciphertext.to_vec()),
        };
        let mut guard = engine.lock().await;
        guard.decrypt(table, column, ciphertext).map_err(|e| {
            ServerError::Startup(StartupError::InvalidMessage(format!(
                "column decrypt failed: {e}"
            )))
        })
    }
}

/// P0-1：从 SQL 文本中提取主表名（用于脱敏上下文匹配）。
///
/// 识别模式（大小写不敏感）：
/// - `SELECT ... FROM <table> ...`
/// - `UPDATE <table> SET ...`
/// - `INSERT INTO <table> ...`
/// - `DELETE FROM <table> ...`
/// - `... JOIN <table> ...`（仅作为后备，主表取 FROM 后的）
///
/// 表名规则：以字母或下划线开头，后跟字母数字下划线。
/// 引号包裹的标识符（"table"）也支持。
///
/// 返回值规则：
/// - 普通标识符：转为小写（SQL 不区分大小写）
/// - 引号标识符：保留原大小写（SQL 标准要求）
///
/// 提取失败返回 None。
fn extract_main_table_name(sql: &str) -> Option<String> {
    let upper = sql.to_uppercase();
    let bytes = sql.as_bytes();
    let upper_bytes = upper.as_bytes();

    // 关键字列表（按优先级：FROM > UPDATE > INTO > DELETE FROM）
    // 找到第一个 FROM（在 SELECT 上下文）
    let from_pos = upper_bytes
        .windows(4)
        .position(|w| w == b"FROM")
        .filter(|&p| {
            // 前一个字符必须是空白或非字母（避免匹配 FROMXY）
            p == 0 || !bytes[p - 1].is_ascii_alphabetic()
        })
        .filter(|&p| {
            // 后一个字符必须是空白（避免匹配 FROMXY）
            p + 4 >= bytes.len() || bytes[p + 4].is_ascii_whitespace()
        });

    if let Some(pos) = from_pos {
        if let Some(name) = parse_identifier_at(bytes, pos + 4) {
            return Some(normalize_identifier(name));
        }
    }

    // UPDATE <table>
    if let Some(pos) = find_keyword(upper_bytes, b"UPDATE") {
        if let Some(name) = parse_identifier_at(bytes, pos + 6) {
            return Some(normalize_identifier(name));
        }
    }

    // INTO <table>
    if let Some(pos) = find_keyword(upper_bytes, b"INTO") {
        if let Some(name) = parse_identifier_at(bytes, pos + 4) {
            return Some(normalize_identifier(name));
        }
    }

    None
}

/// P0-1：标识符规范化——引号标识符保留大小写，普通标识符转小写。
fn normalize_identifier(name: String) -> String {
    // 引号标识符通过 parse_identifier_at 返回时已剥离引号，
    // 但我们通过检查原始 SQL 上下文无法回溯；改为约定：
    // - 若 name 包含空格或非 ASCII 字母数字/下划线 → 视为引号标识符，保留原样
    // - 否则视为普通标识符，转小写
    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        name.to_lowercase()
    } else {
        // 引号标识符（包含空格、点号等特殊字符），保留原大小写
        name
    }
}

/// 在 upper_bytes 中查找关键字（前缀匹配 + 边界检查）。
fn find_keyword(upper_bytes: &[u8], kw: &[u8]) -> Option<usize> {
    upper_bytes
        .windows(kw.len())
        .position(|w| w == kw)
        .filter(|&p| {
            (p == 0 || !upper_bytes[p - 1].is_ascii_alphabetic())
                && (p + kw.len() >= upper_bytes.len()
                    || upper_bytes[p + kw.len()].is_ascii_whitespace())
        })
}

/// 从 bytes[pos..] 开始跳过空白，解析一个 SQL 标识符。
///
/// 支持：
/// - 普通标识符：[A-Za-z_][A-Za-z0-9_]*
/// - 引号标识符："..."（双引号包裹）
fn parse_identifier_at(bytes: &[u8], mut pos: usize) -> Option<String> {
    // 跳过空白
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    if pos >= bytes.len() {
        return None;
    }

    // 引号标识符
    if bytes[pos] == b'"' {
        let start = pos + 1;
        let end = bytes[start..]
            .iter()
            .position(|&b| b == b'"')
            .map(|e| start + e)?;
        return Some(String::from_utf8_lossy(&bytes[start..end]).to_string());
    }

    // 普通标识符
    let start = pos;
    while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
        pos += 1;
    }
    if pos == start {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes[start..pos]).to_string())
}

// =====================================================================
//  编码辅助函数
// =====================================================================

/// 编码 RowDescription 消息。
///
/// 字段格式（每字段）：
/// ```text
/// name (cstring) + table_oid (i32) + col_attr (i16) + type_oid (i32)
///              + type_size (i16) + type_modifier (i32) + format_code (i16)
/// ```
///
/// `format_codes` 为客户端在 Bind 中请求的格式码：
/// - 空切片 → 全部 text（简单查询协议或 Describe statement 场景）
/// - 长度 1 → 所有列使用同一格式码
/// - 长度 N → 每列对应一个格式码
///
/// 实际写入的 format_code 综合客户端请求与类型支持情况：
/// - 客户端请求 binary (1) 且类型支持 binary → 1
/// - 否则 → 0（text）
fn encode_row_description(
    columns: &[crate::pgwire::session::ResultColumn],
    format_codes: &[i16],
    dst: &mut BytesMut,
) {
    use crate::pgwire::message::MSG_ROW_DESCRIPTION;

    // 先写占位 Type + Length=0
    dst.put_u8(MSG_ROW_DESCRIPTION);
    let length_pos = dst.len();
    dst.put_i32(0);
    let payload_start = dst.len();

    // 字段数量
    dst.put_i16(columns.len() as i16);

    // 每个字段
    for (i, col) in columns.iter().enumerate() {
        // name (cstring)
        dst.put_slice(col.name.as_bytes());
        dst.put_u8(0);
        // table OID (0 = 不来自具体表)
        dst.put_i32(0);
        // column attribute number (0)
        dst.put_i16(0);
        // type OID
        dst.put_u32(column_type_oid(&col.column_type));
        // type size
        dst.put_i16(column_type_size(&col.column_type));
        // type modifier (-1 = unknown)
        dst.put_i32(-1);
        // format code：综合客户端请求与类型支持
        let actual = actual_format_code(format_codes, i, &col.column_type);
        dst.put_i16(actual);
    }

    // 回填 length
    let payload_len = dst.len() - payload_start;
    let total_len = (payload_len + 4) as i32;
    dst[length_pos..length_pos + 4].copy_from_slice(&total_len.to_be_bytes());
}

/// 编码 DataRow 消息。
///
/// 字段格式：
/// ```text
/// column_count (i16) + (length (i32) + data (N bytes))*
/// ```
/// NULL 列以 length = -1 表示（无 data 字节）。
///
/// `format_codes` 为 `encode_row_description` 中确定的**实际**格式码，
/// 长度规则：空切片或长度 1 表示全部使用同一格式；长度 N 表示逐列指定。
/// 本函数会按格式码选择 `value_to_binary` 或 `value_to_text` 编码。
fn encode_data_row(
    row: &[szrsql_types::value::Value],
    columns: &[crate::pgwire::session::ResultColumn],
    format_codes: &[i16],
    dst: &mut BytesMut,
) {
    use crate::pgwire::message::MSG_DATA_ROW;

    // 先写占位 Type + Length=0
    dst.put_u8(MSG_DATA_ROW);
    let length_pos = dst.len();
    dst.put_i32(0);
    let payload_start = dst.len();

    // 列数
    dst.put_i16(row.len() as i16);

    // 每列值
    for (i, value) in row.iter().enumerate() {
        let actual = actual_format_code(
            format_codes,
            i,
            &columns
                .get(i)
                .map(|c| c.column_type.clone())
                .unwrap_or(szrsql_types::value::ColumnType::Null),
        );
        if actual == 1 {
            // 二进制格式
            match value_to_binary(value) {
                Some(bytes) => {
                    dst.put_i32(bytes.len() as i32);
                    dst.put_slice(&bytes);
                }
                None => {
                    // NULL 或不支持二进制的值 → length = -1
                    dst.put_i32(-1);
                }
            }
        } else {
            // 文本格式
            match value_to_text(value) {
                Some(text) => {
                    let bytes = text.as_bytes();
                    dst.put_i32(bytes.len() as i32);
                    dst.put_slice(bytes);
                }
                None => {
                    // NULL
                    dst.put_i32(-1);
                }
            }
        }
    }

    // 回填 length
    let payload_len = dst.len() - payload_start;
    let total_len = (payload_len + 4) as i32;
    dst[length_pos..length_pos + 4].copy_from_slice(&total_len.to_be_bytes());
}

/// 解析客户端在 Bind 中请求的格式码（per-column）。
///
/// PG 协议规定：
/// - 空列表 → 全部 text（format_code = 0）
/// - 长度 1 → 所有列使用 format_codes[0]
/// - 长度 N → 每列对应 format_codes[i]
fn resolve_requested_format(format_codes: &[i16], column_index: usize) -> i16 {
    if format_codes.is_empty() {
        0
    } else if format_codes.len() == 1 {
        format_codes[0]
    } else {
        format_codes.get(column_index).copied().unwrap_or(0)
    }
}

/// 计算实际使用的格式码：综合客户端请求与类型支持情况。
///
/// - 客户端请求 binary (1) 且类型支持 binary → 1
/// - 否则 → 0（text）
fn actual_format_code(
    format_codes: &[i16],
    column_index: usize,
    column_type: &szrsql_types::value::ColumnType,
) -> i16 {
    let requested = resolve_requested_format(format_codes, column_index);
    if requested == 1 && column_type_supports_binary(column_type) {
        1
    } else {
        0
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 基于 pid 生成 secret_key（伪随机但确定性）。
fn generate_secret_key(pid: i32) -> i32 {
    // 简单的哈希混合：pid * 2654435761 mod 2^31
    let h = (pid as i64).wrapping_mul(2_654_435_761i64) & 0x7FFF_FFFF;
    h as i32 ^ 0x5555_5555i32
}

/// Phase 4.4：从流中读取并解码下一条前端消息。
///
/// 读取数据，支持连接空闲超时。
///
/// 当 `idle_timeout` 为 `Duration::ZERO` 时，不启用超时（阻塞等待）。
/// 超时后返回 `TimedOut` 错误，调用方据此关闭连接并释放 session 资源
/// （回滚未提交事务、释放行锁），避免客户端异常断开导致的死锁。
async fn read_with_idle_timeout<S>(
    stream: &mut S,
    chunk: &mut [u8],
    idle_timeout: std::time::Duration,
) -> Result<usize, std::io::Error>
where
    S: AsyncRead + Unpin,
{
    if idle_timeout.is_zero() {
        return stream.read(chunk).await;
    }
    match tokio::time::timeout(idle_timeout, stream.read(chunk)).await {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!(
                timeout_secs = idle_timeout.as_secs(),
                "connection idle timeout, closing connection"
            );
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connection idle timeout",
            ))
        }
    }
}

/// 用于 SCRAM 认证阶段读取 SASLInitialResponse / SASLResponse。
/// 复用主循环的 `FrontendMessage::decode` 流式解码逻辑。
async fn read_frontend_message<S>(
    stream: &mut S,
    buf: &mut BytesMut,
    idle_timeout: std::time::Duration,
) -> Result<FrontendMessage, std::io::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let mut tmp = buf.clone();
        match FrontendMessage::decode(&mut tmp) {
            Ok(Some(msg)) => {
                let consumed = buf.len() - tmp.len();
                buf.advance(consumed);
                return Ok(msg);
            }
            Ok(None) => {
                let mut chunk = [0u8; 4096];
                let n = read_with_idle_timeout(stream, &mut chunk, idle_timeout).await?;
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "client disconnected during SCRAM auth",
                    ));
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Phase 4.4：发送认证失败的 ErrorResponse 并 flush。
async fn send_auth_error_response<S>(stream: &mut S, err: &AuthError) -> Result<(), std::io::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    tracing::warn!(error = %err, "SCRAM authentication failed");
    let err_resp = ErrorResponse::fatal(
        SqlState::INVALID_AUTHORIZATION_SPECIFICATION,
        err.to_string(),
    );
    let mut resp = BytesMut::new();
    BackendMessage::ErrorResponse(err_resp).encode(&mut resp);
    stream.write_all(&resp).await?;
    stream.flush().await?;
    Ok(())
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pgwire::message::{
        FrontendMessage, MSG_AUTHENTICATION, MSG_COMMAND_COMPLETE, MSG_DATA_ROW,
        MSG_EMPTY_QUERY_RESPONSE, MSG_PARAMETER_STATUS, MSG_QUERY, MSG_READY_FOR_QUERY,
        MSG_ROW_DESCRIPTION, MSG_TERMINATE, STATUS_IDLE, STATUS_IN_TRANSACTION,
    };
    use crate::pgwire::startup::{
        encode_cancel_request, encode_special_request, encode_startup_message, StartupParams,
        PROTOCOL_GSSNC_REQUEST, PROTOCOL_SSL_REQUEST,
    };
    use bytes::BufMut;

    // ---- 配置测试 ----

    #[test]
    fn test_pgwire_config_default() {
        let config = PgwireConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 5432);
        assert!(!config.server_version.is_empty());
    }

    #[test]
    fn test_pgwire_config_builder() {
        let config = PgwireConfig::new()
            .with_host("0.0.0.0")
            .with_port(6543)
            .with_server_version("15.0-szrsql");
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 6543);
        assert_eq!(config.server_version, "15.0-szrsql");
    }

    // ---- require_tls 配置测试 ----

    #[test]
    fn test_pgwire_config_require_tls_default_false() {
        let config = PgwireConfig::default();
        assert!(!config.require_tls, "require_tls should default to false");
    }

    // ---- P0-1：安全组件集成测试 ----

    #[test]
    fn test_extract_main_table_name_select() {
        assert_eq!(
            extract_main_table_name("SELECT * FROM users WHERE id = 1"),
            Some("users".to_string())
        );
        assert_eq!(
            extract_main_table_name("select id, name from accounts"),
            Some("accounts".to_string())
        );
        // 引号标识符
        assert_eq!(
            extract_main_table_name("SELECT * FROM \"User Table\""),
            Some("User Table".to_string())
        );
        // 大小写混合
        assert_eq!(
            extract_main_table_name("Select * FrOm Orders"),
            Some("orders".to_string())
        );
    }

    #[test]
    fn test_extract_main_table_name_update() {
        assert_eq!(
            extract_main_table_name("UPDATE accounts SET balance = 100"),
            Some("accounts".to_string())
        );
    }

    #[test]
    fn test_extract_main_table_name_insert() {
        assert_eq!(
            extract_main_table_name("INSERT INTO orders (id) VALUES (1)"),
            Some("orders".to_string())
        );
    }

    #[test]
    fn test_extract_main_table_name_no_table() {
        assert_eq!(extract_main_table_name("SELECT 1 + 2"), None);
        assert_eq!(extract_main_table_name("BEGIN"), None);
        assert_eq!(extract_main_table_name(""), None);
    }

    /// 验证 MaskingEngine 在 handle_query 路径中对 ResultSet 的 Text 列应用脱敏规则。
    ///
    /// 流程：
    /// 1. 构造 PgwireServer + 注入 MaskingEngine（注册 (users, email) 策略）
    /// 2. 构造 ResultSet：表 users，列 email，2 行数据
    /// 3. 调用 apply_masking_to_results（私有方法，通过模拟 SQL 路径测试）
    /// 4. 断言 email 列被脱敏（非原值）
    #[test]
    fn test_apply_masking_to_results_masks_registered_column() {
        use crate::pgwire::session::{QueryResult, ResultColumn};
        use szrsql_security::masking::{MaskingEngine, MaskingPolicy, MaskingRule};
        use szrsql_types::value::{ColumnType, Value};

        // 1. 构造 MaskingEngine 并注册 (users, email) → fixed_with("***") 策略
        let mut engine = MaskingEngine::new();
        let policy = MaskingPolicy::new(
            "email_mask",
            "users",
            "email",
            MaskingRule::fixed_with("***".to_string()),
        );
        engine.register(policy).unwrap();
        let engine_arc = Arc::new(tokio::sync::Mutex::new(engine));

        // 2. 构造 PgwireServer 并注入 MaskingEngine
        let config = PgwireConfig::default();
        let mut server = PgwireServer::new(config);
        server.masking_engine = Some(engine_arc);

        // 3. 构造 ResultSet 模拟 SELECT email FROM users
        let columns = vec![ResultColumn {
            name: "email".to_string(),
            column_type: ColumnType::Text,
        }];
        let rows = vec![
            vec![Value::Text("alice@example.com".to_string())],
            vec![Value::Text("bob@example.com".to_string())],
        ];
        let mut results = vec![Ok(QueryResult::ResultSet {
            columns,
            rows,
            tag: "SELECT 2".to_string(),
        })];

        // 4. 应用脱敏
        server.apply_masking_to_results("SELECT email FROM users", &mut results);

        // 5. 断言：email 列已被脱敏为 "***"
        if let Ok(QueryResult::ResultSet { rows, .. }) = &results[0] {
            assert_eq!(
                rows[0][0],
                Value::Text("***".to_string()),
                "row 0 email should be masked"
            );
            assert_eq!(
                rows[1][0],
                Value::Text("***".to_string()),
                "row 1 email should be masked"
            );
        } else {
            panic!("expected ResultSet");
        }
    }

    /// 验证未注册的列不受脱敏影响。
    #[test]
    fn test_apply_masking_to_results_skips_unregistered_column() {
        use crate::pgwire::session::{QueryResult, ResultColumn};
        use szrsql_security::masking::MaskingEngine;
        use szrsql_types::value::{ColumnType, Value};

        let engine = MaskingEngine::new(); // 空注册表
        let engine_arc = Arc::new(tokio::sync::Mutex::new(engine));

        let config = PgwireConfig::default();
        let mut server = PgwireServer::new(config);
        server.masking_engine = Some(engine_arc);

        let columns = vec![ResultColumn {
            name: "name".to_string(),
            column_type: ColumnType::Text,
        }];
        let original = "Alice";
        let rows = vec![vec![Value::Text(original.to_string())]];
        let mut results = vec![Ok(QueryResult::ResultSet {
            columns,
            rows,
            tag: "SELECT 1".to_string(),
        })];

        server.apply_masking_to_results("SELECT name FROM users", &mut results);

        if let Ok(QueryResult::ResultSet { rows, .. }) = &results[0] {
            assert_eq!(
                rows[0][0],
                Value::Text(original.to_string()),
                "unregistered column should not be masked"
            );
        } else {
            panic!("expected ResultSet");
        }
    }

    /// 验证未注入 MaskingEngine 时 apply_masking_to_results 为 no-op。
    #[test]
    fn test_apply_masking_to_results_noop_when_engine_absent() {
        use crate::pgwire::session::{QueryResult, ResultColumn};
        use szrsql_types::value::{ColumnType, Value};

        let config = PgwireConfig::default();
        let server = PgwireServer::new(config); // 无 masking_engine

        let columns = vec![ResultColumn {
            name: "x".to_string(),
            column_type: ColumnType::Text,
        }];
        let rows = vec![vec![Value::Text("hello".to_string())]];
        let mut results = vec![Ok(QueryResult::ResultSet {
            columns,
            rows,
            tag: "SELECT 1".to_string(),
        })];

        // 不应 panic，也不应修改值
        server.apply_masking_to_results("SELECT x FROM t", &mut results);

        if let Ok(QueryResult::ResultSet { rows, .. }) = &results[0] {
            assert_eq!(rows[0][0], Value::Text("hello".to_string()));
        }
    }

    /// 验证密码策略注册表：default profile 应拒绝过短密码。
    #[tokio::test]
    async fn test_validate_password_rejects_short_password() {
        use szrsql_security::password_profile::{PasswordProfile, PasswordProfileRegistry};

        let mut registry = PasswordProfileRegistry::new();
        // 替换 default 为严格策略：min_length=12
        let strict = PasswordProfile::builder("default").min_length(12).build();
        registry.upsert(strict).unwrap();
        let registry_arc = Arc::new(tokio::sync::Mutex::new(registry));

        let config = PgwireConfig::default();
        let mut server = PgwireServer::new(config);
        server.password_profile_registry = Some(registry_arc);

        // 短密码应被拒绝
        let result = server.validate_password("short").await;
        assert!(result.is_err(), "short password should be rejected");

        // 满足所有复杂度要求（min_length=12 + 默认大小写/数字/特殊字符）的密码应通过
        let result = server.validate_password("Longenough1!").await;
        assert!(result.is_ok(), "long password should pass: {:?}", result);
    }

    /// 验证 TDE 页加密/解密往返。
    #[tokio::test]
    async fn test_encrypt_decrypt_page_roundtrip() {
        use szrsql_security::tde::TdeEngine;

        let mut tde = TdeEngine::new();
        let key = [0x42u8; 32];
        tde.enable(&key).unwrap();
        let tde_arc = Arc::new(tokio::sync::Mutex::new(tde));

        let config = PgwireConfig::default();
        let mut server = PgwireServer::new(config);
        server.tde_engine = Some(tde_arc);

        let page_id = 42u64;
        let plaintext = b"hello TDE page encryption world! This is a test page.";
        let ciphertext = server.encrypt_page(page_id, plaintext).await.unwrap();
        assert_ne!(
            ciphertext,
            plaintext.to_vec(),
            "ciphertext should differ from plaintext"
        );

        let decrypted = server.decrypt_page(page_id, &ciphertext).await.unwrap();
        assert_eq!(
            decrypted,
            plaintext.to_vec(),
            "decrypted should match original"
        );
    }

    /// 验证未启用 TDE 时 encrypt_page 为 passthrough。
    #[tokio::test]
    async fn test_encrypt_page_passthrough_when_tde_absent() {
        let config = PgwireConfig::default();
        let server = PgwireServer::new(config); // 无 tde_engine

        let plaintext = b"unencrypted page";
        let result = server.encrypt_page(1, plaintext).await.unwrap();
        assert_eq!(result, plaintext.to_vec());
    }

    /// 验证列加密往返：注册列密钥 → 加密 → 解密 → 一致。
    #[tokio::test]
    async fn test_encrypt_decrypt_column_value_roundtrip() {
        use szrsql_security::column_enc::{
            ColumnEncryptionConfig, ColumnEncryptionEngine, ColumnKey,
        };

        let mut engine = ColumnEncryptionEngine::new();
        // 注册列密钥并配置 (users, ssn) 加密
        let key = ColumnKey::generate("k1");
        engine.register_key(key);
        engine
            .register_column(ColumnEncryptionConfig::new("users", "ssn", "k1"))
            .unwrap();
        let engine_arc = Arc::new(tokio::sync::Mutex::new(engine));

        let config = PgwireConfig::default();
        let mut server = PgwireServer::new(config);
        server.column_encryption_engine = Some(engine_arc);

        let plaintext = b"123-45-6789";
        let ciphertext = server
            .encrypt_column_value("users", "ssn", plaintext)
            .await
            .unwrap();
        assert_ne!(
            ciphertext,
            plaintext.to_vec(),
            "ciphertext should differ from plaintext"
        );

        let decrypted = server
            .decrypt_column_value("users", "ssn", &ciphertext)
            .await
            .unwrap();
        assert_eq!(
            decrypted,
            plaintext.to_vec(),
            "decrypted column value should match original"
        );
    }

    #[test]
    fn test_pgwire_config_with_require_tls() {
        let config = PgwireConfig::new().with_require_tls(true);
        assert!(config.require_tls);
        // 链式调用后再次关闭
        let config = config.with_require_tls(false);
        assert!(!config.require_tls);
    }

    // ---- 服务器构造 ----

    #[test]
    fn test_pgwire_server_new() {
        let config = PgwireConfig::new();
        let server = PgwireServer::new(config);
        assert_eq!(server.config().port, 5432);
    }

    // ---- pid 生成 ----

    #[test]
    fn test_pid_counter_increments() {
        let server = PgwireServer::new(PgwireConfig::new());
        let pid1 = server.pid_counter.fetch_add(1, Ordering::SeqCst);
        let pid2 = server.pid_counter.fetch_add(1, Ordering::SeqCst);
        assert_eq!(pid2, pid1 + 1);
    }

    #[test]
    fn test_generate_secret_key_deterministic() {
        let k1 = generate_secret_key(1234);
        let k2 = generate_secret_key(1234);
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_generate_secret_key_differs_for_different_pid() {
        let k1 = generate_secret_key(1);
        let k2 = generate_secret_key(2);
        assert_ne!(k1, k2);
    }

    // ---- encode_row_description / encode_data_row 编码正确性 ----

    #[test]
    fn test_encode_row_description_single_int8_column() {
        use crate::pgwire::session::ResultColumn;
        use szrsql_types::value::ColumnType;

        let columns = vec![ResultColumn {
            name: "id".into(),
            column_type: ColumnType::Int64,
        }];
        let mut dst = BytesMut::new();
        // 简单查询路径使用空 format_codes（全部 text）
        encode_row_description(&columns, &[], &mut dst);

        // RowDescription: Type='T' + Length + field_count(i16)
        assert_eq!(dst[0], MSG_ROW_DESCRIPTION);
        let field_count = i16::from_be_bytes([dst[5], dst[6]]);
        assert_eq!(field_count, 1);
        // 检查字段名 "id\0"
        assert_eq!(&dst[7..10], b"id\0");
    }

    #[test]
    fn test_encode_data_row_single_int_value() {
        use crate::pgwire::session::ResultColumn;
        use szrsql_types::value::{ColumnType, Value};

        let row = vec![Value::Int64(42)];
        let columns = vec![ResultColumn {
            name: "id".into(),
            column_type: ColumnType::Int64,
        }];
        let mut dst = BytesMut::new();
        encode_data_row(&row, &columns, &[], &mut dst);

        // DataRow: Type='D' + Length + column_count(i16)
        assert_eq!(dst[0], MSG_DATA_ROW);
        let col_count = i16::from_be_bytes([dst[5], dst[6]]);
        assert_eq!(col_count, 1);
        // 列长度 (i32)
        let col_len = i32::from_be_bytes([dst[7], dst[8], dst[9], dst[10]]);
        assert_eq!(col_len, 2); // "42" 占 2 字节
        assert_eq!(&dst[11..13], b"42");
    }

    #[test]
    fn test_encode_data_row_null_value() {
        use crate::pgwire::session::ResultColumn;
        use szrsql_types::value::{ColumnType, Value};

        let row = vec![Value::Null];
        let columns = vec![ResultColumn {
            name: "x".into(),
            column_type: ColumnType::Null,
        }];
        let mut dst = BytesMut::new();
        encode_data_row(&row, &columns, &[], &mut dst);

        // 列长度应为 -1 表示 NULL
        let col_len = i32::from_be_bytes([dst[7], dst[8], dst[9], dst[10]]);
        assert_eq!(col_len, -1);
    }

    #[test]
    fn test_encode_query_result_result_set() {
        use crate::pgwire::session::ResultColumn;
        use szrsql_types::value::{ColumnType, Value};

        let server = PgwireServer::new(PgwireConfig::new());
        let result = QueryResult::ResultSet {
            columns: vec![ResultColumn {
                name: "v".into(),
                column_type: ColumnType::Int64,
            }],
            rows: vec![vec![Value::Int64(1)]],
            tag: "SELECT 1".into(),
        };
        let mut dst = BytesMut::new();
        server.encode_query_result(&result, &[], &mut dst);

        // 应包含：RowDescription + DataRow + CommandComplete
        let mut types = Vec::new();
        let mut i = 0;
        while i < dst.len() {
            let msg_type = dst[i];
            let msg_len =
                i32::from_be_bytes([dst[i + 1], dst[i + 2], dst[i + 3], dst[i + 4]]) as usize;
            types.push(msg_type);
            i += 1 + msg_len;
        }
        assert_eq!(
            types,
            vec![MSG_ROW_DESCRIPTION, MSG_DATA_ROW, MSG_COMMAND_COMPLETE]
        );
    }

    // ---- 端到端：模拟客户端发送启动消息 ----

    #[test]
    fn test_simulate_startup_handshake_decode() {
        // 模拟客户端发送的 StartupMessage
        let params = StartupParams::new()
            .with("user", "alice")
            .with("database", "testdb")
            .with("application_name", "psql");
        let client_bytes = encode_startup_message(&params);

        // 服务器端解码
        let mut src = client_bytes;
        let msg = StartupMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");

        match msg {
            StartupMessage::Startup(decoded) => {
                assert_eq!(decoded.user(), Some("alice"));
                assert_eq!(decoded.database(), Some("testdb"));
                assert_eq!(
                    decoded.params.get("application_name"),
                    Some(&"psql".to_string())
                );
            }
            other => panic!("expected Startup, got {other:?}"),
        }
    }

    #[test]
    fn test_simulate_ssl_request_then_startup() {
        // 模拟客户端先发 SSLRequest，服务器拒绝（'N'），客户端再发 Startup
        let ssl_bytes = encode_special_request(PROTOCOL_SSL_REQUEST);
        let mut src = ssl_bytes;
        let msg = StartupMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        assert_eq!(msg, StartupMessage::SslRequest);

        // 然后客户端发送 StartupMessage（模拟在收到 'N' 之后）
        let params = StartupParams::new().with("user", "bob");
        let startup_bytes = encode_startup_message(&params);
        let mut src2 = startup_bytes;
        let msg2 = StartupMessage::decode(&mut src2)
            .unwrap()
            .expect("should decode");
        match msg2 {
            StartupMessage::Startup(p) => assert_eq!(p.user(), Some("bob")),
            other => panic!("expected Startup, got {other:?}"),
        }
    }

    #[test]
    fn test_simulate_cancel_request() {
        let bytes = encode_cancel_request(4242, -12345);
        let mut src = bytes;
        let msg = StartupMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        assert_eq!(
            msg,
            StartupMessage::CancelRequest {
                pid: 4242,
                secret_key: -12345
            }
        );
    }

    #[test]
    fn test_simulate_gssenc_request() {
        let bytes = encode_special_request(PROTOCOL_GSSNC_REQUEST);
        let mut src = bytes;
        let msg = StartupMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        assert_eq!(msg, StartupMessage::GssencRequest);
    }

    // ---- 模拟 Query 消息解码 ----

    #[test]
    fn test_simulate_query_message_decode() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_QUERY);
        buf.put_i32(13); // length = 4 + 8 + 1 (含 \0)
        buf.put_slice(b"SELECT 1\0");

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        match msg {
            FrontendMessage::Query { sql } => assert_eq!(sql, "SELECT 1"),
            other => panic!("expected Query, got {other:?}"),
        }
    }

    #[test]
    fn test_simulate_terminate_message_decode() {
        let mut buf = BytesMut::new();
        buf.put_u8(MSG_TERMINATE);
        buf.put_i32(4);

        let mut src = buf;
        let msg = FrontendMessage::decode(&mut src)
            .unwrap()
            .expect("should decode");
        assert_eq!(msg, FrontendMessage::Terminate);
    }

    // ---- 握手响应完整性测试 ----

    #[test]
    fn test_handshake_response_message_order() {
        let mut dst = BytesMut::new();
        crate::pgwire::startup::build_startup_response(1234, -5678, "14.0", Some("psql"), &mut dst);

        let mut types = Vec::new();
        let mut i = 0;
        while i < dst.len() {
            let msg_type = dst[i];
            let msg_len =
                i32::from_be_bytes([dst[i + 1], dst[i + 2], dst[i + 3], dst[i + 4]]) as usize;
            types.push(msg_type);
            i += 1 + msg_len;
        }

        // 顺序：AuthenticationOk → ParameterStatus* → BackendKeyData → ReadyForQuery
        assert_eq!(types[0], MSG_AUTHENTICATION);
        for t in &types[1..types.len() - 2] {
            assert_eq!(*t, MSG_PARAMETER_STATUS);
        }
        // 应包含 application_name 的 ParameterStatus
        assert!(types.len() >= 9, "expected at least 9 messages");
    }

    // ---- 协议错误响应测试 ----

    #[test]
    fn test_protocol_error_response_is_fatal() {
        let mut dst = BytesMut::new();
        crate::pgwire::startup::build_protocol_error_response(
            "unsupported protocol version",
            &mut dst,
        );
        assert_eq!(dst[0], b'E');
        let s = String::from_utf8_lossy(&dst);
        assert!(s.contains("FATAL"));
        assert!(s.contains("08P01"));
        assert!(s.contains("unsupported protocol version"));
    }

    // ---- EmptyQuery 处理 ----

    #[test]
    fn test_empty_query_response_format() {
        let mut dst = BytesMut::new();
        // 模拟空 SQL
        if "".trim().is_empty() {
            BackendMessage::EmptyQueryResponse.encode(&mut dst);
            BackendMessage::ReadyForQuery {
                status: STATUS_IDLE,
            }
            .encode(&mut dst);
        }
        // 第一条是 EmptyQueryResponse: 'I' + length=4
        assert_eq!(dst[0], MSG_EMPTY_QUERY_RESPONSE);
        let len = i32::from_be_bytes([dst[1], dst[2], dst[3], dst[4]]) as usize;
        assert_eq!(len, 4);
        // 第二条起始位置 = 1 + len = 5
        assert_eq!(dst[5], MSG_READY_FOR_QUERY);
        // ReadyForQuery 内部 status='I'
        assert_eq!(dst[10], STATUS_IDLE);
    }

    // ---- STATUS 常量 ----

    #[test]
    fn test_status_idle_constant() {
        assert_eq!(STATUS_IDLE, b'I');
        assert_eq!(STATUS_IN_TRANSACTION, b'T');
    }
}
