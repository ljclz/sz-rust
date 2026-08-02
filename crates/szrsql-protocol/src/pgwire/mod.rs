//! SzRSQL pgwire 协议层。
//!
//! 对应 `SzRSQL实施进度.md` Phase 4.1 — pgwire 集成 + 启动消息握手。
//! 对应 `SzRSQL实施进度.md` Phase 4.2 — 简单查询协议（接入真实 SQL 执行器）。
//! 对应 `SzRSQL实施进度.md` Phase 4.3 — 扩展查询协议（Parse/Bind/Execute/Describe/Close/Sync/Flush）。
//! 对应 `SzRSQL实施进度.md` Phase 4.4 — 认证（trust + scram-sha-256）。
//! 对应 `SzRSQL实施进度.md` Phase 4.5 — TLS 1.3（rustls 集成）。
//! 对应 `SzRSQL实施进度.md` Phase 4.6 — 错误码 + 通知（LISTEN/NOTIFY/UNLISTEN）。
//! 对应 `SzRSQL实施进度.md` Phase 4.7 — 元数据查询（pg_catalog + information_schema 子集）。
//! 对应 `SzRSQL实施进度.md` Phase 4.8 — COPY FROM/TO（CSV/TEXT 文件导入导出）。
//! 对应 `SzRSQL实施进度.md` Phase 4.11 — 优雅关闭（Graceful Shutdown）。
//! 对应 `SzRSQL实施进度.md` Phase 4.12 — 信号处理 + Crash Handler。
//! 对应 `SzRSQL实施进度.md` Phase 4.13 — 进程守护化 + PID 文件。
//!
//! 本模块实现 PostgreSQL 前端/后端协议 v3.0 的最小子集：
//! - 启动消息（StartupMessage / SSLRequest / CancelRequest）解析
//! - 后端握手响应（AuthenticationOk / ParameterStatus / BackendKeyData / ReadyForQuery）
//! - 简单查询响应（RowDescription / DataRow / CommandComplete / EmptyQueryResponse）
//! - 错误响应（ErrorResponse）与通知（NoticeResponse）
//! - 会话级 SQL 执行服务（ExecutorService）
//! - Phase 4.3 扩展查询：命名预处理语句与 portal
//! - Phase 4.4 认证：trust 免密 + SCRAM-SHA-256 密码认证（RFC 5802 + RFC 7677）
//! - Phase 4.5 TLS：rustls 集成，支持 SSLRequest 协商与 TLS 1.3 加密
//! - Phase 4.6 通知：跨会话 LISTEN/NOTIFY/UNLISTEN 与 NotificationResponse 消息
//! - Phase 4.7 元数据：pg_tables/pg_indexes + information_schema.tables/columns 等系统表查询
//! - Phase 4.8 COPY：CSV/TEXT 文件批量导入（COPY FROM）与导出（COPY TO）
//! - Phase 4.11 优雅关闭：SIGTERM/SIGINT 信号 → Draining → 排空/abort → Closed
//! - Phase 4.12 信号处理：SIGTERM 优雅 / SIGINT 立即；panic hook 写入崩溃日志
//! - Phase 4.13 进程守护化：daemonize（Unix 双 fork）+ PidFile RAII 管理（重复启动检测、stale 清理）
//!
//! 参考文档：<https://www.postgresql.org/docs/current/protocol.html>

pub mod auth;
pub mod copy;
pub mod crash;
pub mod daemon;
pub mod dirty_tracker;
pub mod lifecycle;
pub mod message;
pub mod notify;
pub mod pg_types;
pub mod replication;
pub mod server;
pub mod session;
pub mod startup;
pub mod system_tables;
pub mod tls;

/// P2-14：会话取消注册表类型别名（HTTP 管理端点 + pgwire 会话共享）
pub use crate::http::CancelRegistry;
pub use auth::{AuthError, AuthMode, ScramServerSession, SharedScramCredentials};
pub use crash::{install_crash_handler, CrashConfig};
pub use daemon::{daemonize, DaemonError, PidFile, PidFileError};
pub use dirty_tracker::DirtyTableTracker;
pub use lifecycle::{ShutdownCoordinator, ShutdownSignal, ShutdownState};
pub use message::{BackendMessage, ErrorResponse, FrontendMessage, Severity, SqlState};
pub use notify::{Notification, NotifyHub};
pub use server::{PgwireConfig, PgwireServer};
pub use session::{
    ExecutorService, ExtendedExecuteResult, ExtendedPreparedStatement, Portal, PortalDescription,
    QueryResult, ResultColumn, SessionError, StatementDescription, TransactionState,
};
pub use startup::{StartupError, StartupMessage, StartupParams, PROTOCOL_VERSION_3_0};
/// ADV-CONC-1：re-export InMemoryTable，供 MySQL/TDS/Oracle 协议共享表存储使用
pub use szrsql_sql::executor::InMemoryTable;
pub use tls::{TlsConfig, TlsError};
