//! SzRSQL 数据库服务二进制入口。
//!
//! Phase 4.1：启动 pgwire 服务器，监听 5432 端口（可通过 --port 修改）。
//! Phase 4.11：优雅关闭（SIGTERM → 排空活跃连接 → 退出码 0）。
//! Phase 4.12：信号处理 + Crash Handler：
//!   - SIGTERM → 优雅关闭（等待活跃连接最多 shutdown_timeout）
//!   - SIGINT / Ctrl+C → 立即关闭（不等活跃事务，直接 abort_all）
//!   - panic → 通过 std::panic::set_hook 捕获 → 写入崩溃日志（含 backtrace + WAL LSN 占位）
//!
//! Phase 4.13：进程守护化 + PID 文件：
//!   - --daemon：Unix 双 fork + setsid 守护进程化（Windows 不支持）
//!   - --pid-file：PID 文件 RAII 管理（重复启动检测、stale 清理、自动删除）
//!
//! Phase 4.5.8-4.5.10：HTTP 管理端点：
//!   - --http-port：HTTP 管理端口（默认 0 = 不监听）
//!   - --http-host：HTTP 监听地址（默认 127.0.0.1）
//!   - --http-auth-token：管理端点 Bearer token 鉴权
//!   - 端点：/healthz、/readyz、/metrics、/api/v1/sessions、/api/v1/cancel/{pid}、/api/v1/backup、/api/v1/config/reload
//!
//! 用法：
//! ```bash
//! szrsql                          # 默认 127.0.0.1:5432
//! szrsql --host 0.0.0.0 --port 6543
//! szrsql --daemon --pid-file /var/run/szrsql.pid
//! szrsql --http-port 8080         # 启用 HTTP 管理端点
//! szrsql --version
//! ```
//!
//! 信号处理：
//! - SIGTERM：触发优雅关闭（停止接受新连接，等待活跃连接最多 30s）
//! - SIGINT / Ctrl+C：触发立即关闭（不等待活跃事务，直接 abort）
//! - panic：写入崩溃日志到 `--crash-log-dir` 指定目录（默认当前目录）

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use tokio::sync::RwLock;
use szrsql_tx::lock::LockManager;
use szrsql_tx::mvcc::MvccManager;

use clap::Parser;
use szrsql_protocol::pgwire::{
    auth::CredentialStore, daemonize, install_crash_handler, tls::TlsConfig, CrashConfig,
    PgwireConfig, PgwireServer, PidFile, ShutdownSignal,
};
use szrsql_protocol::{HttpConfig, HttpServer, MetricsRegistry};
use tracing_subscriber::EnvFilter;

mod persistence;

/// SzRSQL 数据库服务命令行参数。
#[derive(Parser, Debug)]
#[command(name = "szrsql", version, about = "SzRSQL 数据库服务")]
struct Args {
    /// 监听地址（默认 127.0.0.1）。
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// 监听端口（默认 5432）。
    #[arg(long, default_value_t = 5432)]
    port: u16,

    /// 服务器版本号字符串（发送给客户端作为 server_version ParameterStatus）。
    #[arg(long, default_value = "14.0-szrsql")]
    server_version: String,

    /// Phase 4.11：优雅关闭超时（秒）。
    ///
    /// 收到 SIGTERM 后，等待活跃连接完成的最长时间；超时后强制中止。
    /// SIGINT/Ctrl+C 不受此超时影响，会立即强制中止。
    #[arg(long, default_value_t = 30)]
    shutdown_timeout: u64,

    /// 连接空闲超时（秒，默认 300 = 5 分钟，0 表示禁用）。
    ///
    /// 当连接在此时间内未收到任何客户端消息时，服务器主动关闭连接并释放
    /// session 资源（回滚未提交事务、释放行锁），避免客户端异常断开
    /// （如被 kill -9 / Stop-Process 强制终止，TCP 未发送 FIN）导致的
    /// 会话死锁和资源泄漏。
    ///
    /// 适用于所有协议（PostgreSQL / MySQL / TDS / Oracle）。
    #[arg(long, default_value_t = 300)]
    connection_idle_timeout: u64,

    /// Phase 4.12：崩溃日志输出目录。
    ///
    /// panic 时写入崩溃日志文件（含时间戳、backtrace、WAL LSN 占位）。
    /// 默认为当前目录 `.`。
    #[arg(long, default_value = ".")]
    crash_log_dir: PathBuf,

    /// Phase 4.12：是否禁用 backtrace 捕获（减少 panic hook 开销）。
    #[arg(long, default_value_t = false)]
    no_backtrace: bool,

    /// Phase 4.13：是否以守护进程模式运行（后台运行）。
    ///
    /// Unix：执行双 fork + setsid 守护进程化，父进程立即退出。
    /// Windows：不支持，返回错误。
    #[arg(long, default_value_t = false)]
    daemon: bool,

    /// Phase 4.13：PID 文件路径。
    ///
    /// 启动时写入当前进程 PID，用于防止重复启动。
    /// 如果文件已存在且对应进程存活，拒绝启动。
    /// 如果对应进程已死，清理 stale 文件后重新创建。
    /// 进程退出时（正常或信号）自动删除 PID 文件。
    #[arg(long)]
    pid_file: Option<PathBuf>,

    /// Phase 4.5.10：HTTP 管理端口（默认 0 = 不监听）。
    ///
    /// 启用后提供 healthz/readyz/metrics 等管理端点。
    /// 建议仅绑定 127.0.0.1 避免外部访问。
    #[arg(long, default_value_t = 0)]
    http_port: u16,

    /// Phase 4.5.10：HTTP 监听地址（默认 127.0.0.1，仅本地访问）。
    #[arg(long, default_value = "127.0.0.1")]
    http_host: String,

    /// Phase 4.5.10：HTTP 管理端点 Bearer token 鉴权。
    ///
    /// 设置后，/api/v1/* 端点需要 `Authorization: Bearer <token>` header。
    /// healthz/readyz/metrics 端点无需鉴权（用于 K8s 探针和 Prometheus 抓取）。
    #[arg(long)]
    http_auth_token: Option<String>,

    /// ADV-F-7：WAL 文件路径（启用 log-then-commit 事务模型，默认启用）。
    ///
    /// 设置后，服务器在启动时创建共享的 `WalWriter`，所有 session 的 COMMIT 操作
    /// 会先写入 WAL Commit 记录并 fsync，然后才向客户端返回成功。
    /// 这消除了"ACK 成功但数据未持久化"的风险。
    ///
    /// **2026-07-31 更新**：WAL 现已默认启用，默认路径为 `{data-dir}/wal.log`。
    /// 设置为空字符串（`--wal-path ""`）可显式禁用 WAL（仅用于测试/兼容，不推荐）。
    #[arg(long)]
    wal_path: Option<PathBuf>,

    /// Phase 4.5：MySQL 协议监听端口（默认 0 = 不监听）。
    ///
    /// 启用后，SzRSQL 同时监听 MySQL Wire Protocol，Navicat 可用 MySQL 协议连接。
    /// 典型端口：3306（避免与本地 MySQL 冲突，建议用 3307/3308）。
    #[arg(long, default_value_t = 0)]
    mysql_port: u16,

    /// Phase 4.5：TDS 协议监听端口（默认 0 = 不监听）。
    ///
    /// 启用后，SzRSQL 同时监听 SQL Server TDS 协议，Navicat 可用 SQL Server 协议连接。
    /// 典型端口：1433。
    #[arg(long, default_value_t = 0)]
    tds_port: u16,

    /// Phase 4.5：Oracle 协议监听端口（默认 0 = 不监听）。
    ///
    /// 启用后，SzRSQL 同时监听 Oracle Net (TNS) 协议，Navicat 可用 Oracle 协议连接。
    /// 典型端口：1521（避免与本地 Oracle 冲突，建议用 1522/1523）。
    #[arg(long, default_value_t = 0)]
    oracle_port: u16,

    /// Phase 4.5：Oracle 服务名（SID/Service Name，默认 ORCL）。
    ///
    /// 客户端连接时需指定匹配的服务名。
    #[arg(long, default_value = "ORCL")]
    oracle_service_name: String,

    /// Phase 4.5：SQLite 协议监听端口（默认 0 = 不监听）。
    ///
    /// 启用后 SzRSQL 暴露 JSON 行协议的 TCP 入口，允许远程客户端执行 SQL。
    /// 典型端口：9432。
    #[arg(long, default_value_t = 0)]
    sqlite_port: u16,

    /// 数据持久化目录（默认 ./data，默认启用持久化）。
    ///
    /// 启动时从 `{data-dir}/tables.json` 加载表数据，后台每 5 秒自动保存。
    /// 服务器关闭时执行最后一次保存。
    /// 设置为空字符串可禁用持久化（不推荐，仅用于测试）。
    #[arg(long, default_value = "./data")]
    data_dir: String,

    /// 强制以空表集启动（默认 false）。
    ///
    /// 仅在快照文件存在但解析失败时生效：
    /// - false（默认）：快照损坏时拒绝启动，避免数据丢失。
    /// - true：快照损坏时打印警告并以空表集启动（明确确认接受数据丢失风险）。
    ///
    /// 快照文件不存在（首次启动）时，无论此参数取值都返回空表集（正常行为）。
    #[arg(long, default_value_t = false)]
    force_empty: bool,

    /// Phase 4.5：TLS 证书文件路径（PEM 格式）。
    ///
    /// 设置后启用 TLS 1.3 加密。需同时提供 --tls-key。
    /// 客户端可通过 SSLRequest 升级为 TLS 加密连接。
    #[arg(long)]
    tls_cert: Option<PathBuf>,

    /// Phase 4.5：TLS 私钥文件路径（PEM 格式）。
    ///
    /// 需与 --tls-cert 配合使用。
    #[arg(long)]
    tls_key: Option<PathBuf>,

    /// Phase 4.5：客户端 CA 证书路径（PEM 格式，用于 mutual TLS）。
    ///
    /// 设置后启用 mutual TLS（双向认证）：服务器在 TLS 握手时验证客户端证书。
    /// 需同时提供 --tls-cert 和 --tls-key。
    #[arg(long)]
    tls_client_ca: Option<PathBuf>,

    /// Phase 4.5：强制 TLS（拒绝明文连接）。
    ///
    /// 为 true 时，客户端必须使用 SSLRequest 升级为 TLS 才能连接。
    /// 直接发送明文 StartupMessage 的客户端将被拒绝。
    /// 需同时提供 --tls-cert 和 --tls-key。
    #[arg(long, default_value_t = false)]
    require_tls: bool,

    /// P7-4：启用 MCP（Model Context Protocol）服务器 stdio 模式。
    ///
    /// 启用后，主进程会 fork 一个独立线程运行 MCP stdio 主循环，
    /// 暴露 35 个 LLM 工具（Schema/Query/SlowQuery/TxLock/Perf/Maintenance/
    /// Alerting/Insight/Replication 9 大类别）。
    ///
    /// 其中 5 个 Replication 类工具直接操作 `ReplicationTaskManager`：
    /// - `create_replication_task` / `list_replication_tasks` /
    ///   `monitor_replication_task` / `stop_replication_task` /
    ///   `replication_manager_stats`
    ///
    /// MCP 服务器与 pgwire 服务器共享 `CdcEngine`，所有 CDC 任务管理操作
    /// 真实生效（非 mock）。
    ///
    /// 典型用法：`szrsql --mcp-stdio < mcp_input.json > mcp_output.json`
    #[arg(long, default_value_t = false)]
    mcp_stdio: bool,

    /// P8-3：集群模式（single = 单节点自选举，cluster = 多节点 TCP 集群）。
    #[arg(long, default_value = "single")]
    cluster_mode: String,

    /// P8-3：本节点 ID（集群模式必填，1-based）。
    #[arg(long, default_value_t = 1)]
    node_id: u64,

    /// P8-3：Raft RPC 监听地址（集群模式必填，如 127.0.0.1:7000）。
    #[arg(long)]
    raft_listen_addr: Option<String>,

    /// P8-3：集群所有节点地址列表（集群模式必填）。
    ///
    /// 格式：`node_id@host:port`，多个用逗号分隔。
    /// 示例：`1@127.0.0.1:7000,2@127.0.0.1:7001,3@127.0.0.1:7002`
    #[arg(long)]
    peers: Option<String>,

    /// P8-3：Raft tick 周期（毫秒，默认 50，与 heartbeat_interval 对齐）。
    #[arg(long, default_value_t = 50)]
    raft_tick_ms: u64,

    /// P2-1：启用 Multi-Master/DistTxn 模式。
    ///
    /// 启用后构造 HlcClock（混合逻辑时钟）、ConflictLog（冲突日志）和
    /// ClusterTxnCoordinator（跨节点事务协调器），为 Multi-Master 写入
    /// 冲突检测和分布式 2PC 事务协调奠定基础。
    /// 可与 `--cluster-mode cluster` 组合使用；单独启用时使用内存集群。
    #[arg(long, default_value_t = false)]
    multi_master: bool,

    /// OPT-4：pgwire 认证模式（trust = 信任所有连接，scram = SCRAM-SHA-256）。
    ///
    /// 默认 trust 保持向后兼容。启用 scram 后，凭据从 `--auth-file` 加载；
    /// 文件不存在时自动创建空凭据文件（首次启动）。
    /// CREATE ROLE / ALTER ROLE 修改的凭据会持久化回 auth-file。
    #[arg(long, default_value = "trust")]
    auth_mode: String,

    /// OPT-4：SCRAM 凭据文件路径（默认 {data-dir}/auth.json）。
    ///
    /// 仅在 --auth-mode=scram 时生效。文件格式见 CredentialStore 文档。
    #[arg(long)]
    auth_file: Option<PathBuf>,

    /// OPT-12：禁用 SQL 防火墙（默认 false = 启用）。
    ///
    /// 启用后，每个 session 的 SQL 在执行前经过 `SqlFirewall::check`：
    /// - SQL 注入特征检测（`' OR 1=1`、`UNION SELECT`、堆叠查询等）
    /// - 禁止命令过滤（默认无禁止命令，可通过 API 配置）
    /// - 白名单匹配（默认空白名单 = 允许所有）
    /// 命中规则的 SQL 返回 ERROR，不执行。
    #[arg(long, default_value_t = false)]
    no_firewall: bool,

    /// OPT-12：禁用审计日志（默认 false = 启用）。
    ///
    /// 启用后，每个 session 执行的 SQL 记录到不可变 append-only 审计日志，
    /// 使用 SHA-256 哈希链保证日志不可篡改。
    /// 注意：审计日志存储在内存中，长运行服务器需定期导出避免内存增长。
    #[arg(long, default_value_t = false)]
    no_audit_log: bool,

    /// P2-2.2：TCP 流复制监听端口（默认 0 = 不监听）。
    ///
    /// 启用后，主库在此端口监听备库连接，将 WAL 记录通过 TCP 推送到备库。
    /// 典型端口：5434（避免与 pgwire 5432 冲突）。
    /// 需同时启用 WAL（--wal-path 或默认）。
    #[arg(long, default_value_t = 0)]
    repl_port: u16,

    /// P2-2.2：作为备库连接到主库地址（格式：host:port，如 192.168.1.10:5434）。
    ///
    /// 启用后，本节点作为备库运行，通过 TCP 连接主库接收 WAL 复制流。
    /// 与 --repl-port 互斥（备库不监听复制端口）。
    #[arg(long)]
    replica_of: Option<String>,
}

fn main() -> anyhow::Result<()> {
    // 初始化 tracing 日志
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    // 手动创建 tokio runtime，设置 8MB worker 栈大小（默认 2MB 在 debug 模式下不够用）
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024) // 8MB
        .build()?;

    let args = Args::parse();

    // 在 8MB 栈大小的 tokio runtime 中运行所有 async 逻辑
    runtime.block_on(async move {
    // Phase 4.13：守护进程化（在创建 PID 文件和 crash handler 之前执行）
    // daemonize 后进程 PID 会变（fork），所以 PID 文件必须在 daemonize 之后创建
    if args.daemon {
        tracing::info!("daemon mode requested, daemonizing...");
        if let Err(e) = daemonize() {
            tracing::error!(error = %e, "daemonize failed");
            return Err(e.into());
        }
        // daemonize 后，tracing 的 stderr 输出已被重定向到 /dev/null
        // 实际生产环境应配置 file appender，此处保持简化
    }

    // Phase 4.12：安装崩溃日志 panic hook（必须在任何可能 panic 的代码之前）
    let crash_config = CrashConfig::new()
        .with_log_dir(&args.crash_log_dir)
        .with_backtrace(!args.no_backtrace);
    install_crash_handler(crash_config);

    // Phase 4.13：创建 PID 文件（在 daemonize 之后，确保写入正确的 PID）
    // PidFile 使用 RAII，drop 时自动删除文件
    let _pid_file = if let Some(pid_path) = &args.pid_file {
        match PidFile::create(pid_path) {
            Ok(pf) => {
                tracing::info!(
                    path = %pf.path().display(),
                    pid = pf.pid(),
                    "PID file created"
                );
                Some(pf)
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to create PID file");
                return Err(e.into());
            }
        }
    } else {
        None
    };

    tracing::info!(
        host = %args.host,
        port = args.port,
        version = %args.server_version,
        shutdown_timeout_secs = args.shutdown_timeout,
        connection_idle_timeout_secs = args.connection_idle_timeout,
        crash_log_dir = ?args.crash_log_dir,
        daemon = args.daemon,
        pid_file = ?args.pid_file,
        http_port = args.http_port,
        http_host = %args.http_host,
        wal_path = ?args.wal_path,
        "starting SzRSQL pgwire server"
    );

    // 保存 host 副本，供 MySQL/TDS 协议监听复用（pgwire config 会 move args.host）
    let listen_host = args.host.clone();

    let config = PgwireConfig::new()
        .with_host(args.host)
        .with_port(args.port)
        .with_server_version(args.server_version)
        .with_shutdown_timeout(std::time::Duration::from_secs(args.shutdown_timeout))
        .with_connection_idle_timeout(std::time::Duration::from_secs(args.connection_idle_timeout));

    // Phase 4.5：TLS 配置（rustls 0.23 + tokio-rustls 0.26）
    //
    // --tls-cert + --tls-key：启用单向 TLS（服务器证书）
    // --tls-client-ca：启用 mutual TLS（双向认证，验证客户端证书）
    // --require-tls：强制 TLS（拒绝明文连接，防止降级攻击）
    let config = if let (Some(cert_path), Some(key_path)) = (&args.tls_cert, &args.tls_key) {
        let tls_config = if let Some(ca_path) = &args.tls_client_ca {
            tracing::info!(
                cert = %cert_path.display(),
                key = %key_path.display(),
                client_ca = %ca_path.display(),
                require_tls = args.require_tls,
                "TLS enabled with mutual TLS (client cert verification)"
            );
            TlsConfig::from_files_with_client_auth(cert_path, key_path, ca_path)?
        } else {
            tracing::info!(
                cert = %cert_path.display(),
                key = %key_path.display(),
                require_tls = args.require_tls,
                "TLS enabled (server cert only)"
            );
            TlsConfig::from_files(cert_path, key_path, None)?
        };
        config.with_tls(tls_config).with_require_tls(args.require_tls)
    } else {
        // 未提供 TLS 证书，校验参数一致性
        if args.require_tls {
            anyhow::bail!("--require-tls requires --tls-cert and --tls-key");
        }
        if args.tls_client_ca.is_some() {
            anyhow::bail!("--tls-client-ca requires --tls-cert and --tls-key");
        }
        if args.tls_cert.is_some() || args.tls_key.is_some() {
            anyhow::bail!("--tls-cert and --tls-key must be provided together");
        }
        tracing::info!("TLS not configured (plaintext only)");
        config
    };

    // OPT-4：CredentialStore 接入启动流程
    //
    // --auth-mode=scram 时从凭据文件加载用户密码，启用 SCRAM-SHA-256 认证；
    // 文件不存在时自动创建空凭据文件（首次启动）。--auth-mode=trust（默认）保持
    // 向后兼容，不加载凭据文件。
    //
    // 凭据文件路径解析优先级：
    //   1. --auth-file 显式指定
    //   2. {data-dir}/auth.json（data-dir 非空时）
    //   3. 跳过加载（data-dir 为空且未指定 --auth-file 时，使用空凭据）
    let config = match args.auth_mode.as_str() {
        "scram" => {
            let auth_path = args
                .auth_file
                .clone()
                .or_else(|| {
                    if args.data_dir.is_empty() {
                        None
                    } else {
                        Some(PathBuf::from(&args.data_dir).join("auth.json"))
                    }
                });
            match auth_path {
                Some(path) => {
                    let cred_store = match CredentialStore::load_from_file(&path) {
                        Ok(Some(store)) => {
                            tracing::info!(
                                auth_file = %path.display(),
                                users = store.credentials.len(),
                                "SCRAM credentials loaded from file"
                            );
                            store
                        }
                        Ok(None) => {
                            // 文件不存在：首次启动，创建空凭据文件
                            let store = CredentialStore::new();
                            if let Err(e) = store.save_to_file(&path) {
                                tracing::warn!(
                                    auth_file = %path.display(),
                                    error = %e,
                                    "failed to initialize SCRAM credentials file"
                                );
                            } else {
                                tracing::info!(
                                    auth_file = %path.display(),
                                    "SCRAM credentials file created (first start)"
                                );
                            }
                            store
                        }
                        Err(e) => {
                            anyhow::bail!(
                                "failed to load SCRAM credentials from {}: {}",
                                path.display(),
                                e
                            );
                        }
                    };
                    config.with_auth_mode(cred_store.to_auth_mode())
                }
                None => {
                    tracing::warn!(
                        "SCRAM auth-mode requested but no auth-file and no data-dir; \
                         using empty credentials (all login will fail until CREATE ROLE)"
                    );
                    config.with_auth_mode(CredentialStore::new().to_auth_mode())
                }
            }
        }
        "trust" => {
            tracing::info!("pgwire auth-mode=trust (all connections allowed)");
            config
        }
        other => {
            anyhow::bail!(
                "invalid --auth-mode '{}': expected 'trust' or 'scram'",
                other
            );
        }
    };

    // ADV-F-7：创建共享 WalWriter（默认启用）
    // 启用 log-then-commit 事务模型：COMMIT 先写 WAL 并 fsync，再 ACK 客户端
    //
    // P0-TX-2 修复：启动时先回放已有 WAL 记录（崩溃恢复），再用 open（非 create_new）打开 WAL
    //   - 旧实现用 create_new 截断 WAL，导致已 commit 但未 checkpoint 的事务记录丢失
    //   - 新实现用 WalReplayer::replay_all 回放记录，再用 open 追加打开
    //   - P0-1 修复：WAL 现在记录 TableData（表全量数据），回放时应用到表集合
    //
    // 2026-07-31 更新：WAL 默认启用，默认路径为 {data-dir}/wal.log
    //   - 未指定 --wal-path 时使用默认路径
    //   - --wal-path "" 显式禁用 WAL（仅用于测试）
    //
    // P0-1 修复：保存回放记录，供快照加载后应用 TableData
    let mut wal_records: Vec<szrsql_tx::wal::WalRecord> = Vec::new();
    // 解析 WAL 路径：未指定则使用 {data-dir}/wal.log；显式空字符串则禁用
    let wal_path_resolved: Option<PathBuf> = match &args.wal_path {
        Some(p) if p.as_os_str().is_empty() => {
            tracing::warn!(
                "WAL explicitly disabled (--wal-path \"\"), running in commit-then-log mode. \
                 This is NOT recommended for production."
            );
            None
        }
        Some(p) => Some(p.clone()),
        None => {
            // 默认启用 WAL：使用 {data-dir}/wal.log
            let default_wal = if args.data_dir.is_empty() {
                PathBuf::from("./data/wal.log")
            } else {
                PathBuf::from(&args.data_dir).join("wal.log")
            };
            tracing::info!(
                wal_path = %default_wal.display(),
                "WAL enabled by default (use --wal-path \"\" to disable)"
            );
            Some(default_wal)
        }
    };
    let wal_writer: Option<Arc<szrsql_tx::wal::WalWriter>> = if let Some(wal_path) = &wal_path_resolved {
        // 确保父目录存在
        if let Some(parent) = wal_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        // P0-TX-2 修复：启动时回放已有 WAL 记录（崩溃恢复）
        if wal_path.exists() {
            match szrsql_tx::wal::WalReplayer::replay_all(wal_path) {
                Ok(records) => {
                    let commit_count = records
                        .iter()
                        .filter(|r| r.op_type == szrsql_tx::wal::WalOpType::Commit)
                        .count();
                    let abort_count = records
                        .iter()
                        .filter(|r| r.op_type == szrsql_tx::wal::WalOpType::Abort)
                        .count();
                    let table_data_count = records
                        .iter()
                        .filter(|r| r.op_type == szrsql_tx::wal::WalOpType::TableData)
                        .count();
                    tracing::info!(
                        wal_path = %wal_path.display(),
                        total_records = records.len(),
                        commit_records = commit_count,
                        abort_records = abort_count,
                        table_data_records = table_data_count,
                        "WAL replay completed on startup (crash recovery)"
                    );
                    // P0-1 修复：保存回放记录，供快照加载后应用 TableData
                    wal_records = records;
                }
                Err(e) => {
                    tracing::warn!(
                        wal_path = %wal_path.display(),
                        error = %e,
                        "WAL replay failed on startup, continuing with fresh WAL"
                    );
                }
            }
        }
        // P0-TX-2 修复：用 open（追加模式）而非 create_new（截断）打开 WAL
        match szrsql_tx::wal::WalWriter::open(wal_path) {
            Ok(writer) => {
                tracing::info!(
                    wal_path = %wal_path.display(),
                    "WAL writer opened (append mode), log-then-commit transaction model enabled"
                );
                Some(Arc::new(writer))
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    wal_path = %wal_path.display(),
                    "failed to open WAL writer, falling back to commit-then-log"
                );
                return Err(e.into());
            }
        }
    } else {
        // wal_path_resolved 为 None（用户显式禁用 WAL）
        None
    };

    // ADV-CONC-1：创建共享表存储、锁管理器和事务 ID 计数器（跨 session/跨协议全局唯一）
    // 默认启用快照持久化：从 {data-dir}/tables.json 加载已保存的表数据
    //
    // P0-2 修复：快照文件损坏时不再静默降级为空表集（可能导致数据丢失）。
    //   - 文件不存在（首次启动）→ 空表集（正常）
    //   - 文件存在但损坏 + --force-empty=false → 拒绝启动（默认行为，保护数据）
    //   - 文件存在但损坏 + --force-empty=true → warn + 空表集（用户明确确认）
    let data_dir = std::path::PathBuf::from(&args.data_dir);
    let mut loaded_tables = if args.data_dir.is_empty() {
        tracing::info!("persistence disabled (--data-dir is empty)");
        HashMap::new()
    } else {
        match persistence::load_snapshot(&data_dir) {
            Ok(tables) => {
                tracing::info!(
                    table_count = tables.len(),
                    data_dir = %data_dir.display(),
                    "tables loaded from snapshot"
                );
                tables
            }
            Err(e) => {
                if args.force_empty {
                    tracing::warn!(
                        error = %e,
                        data_dir = %data_dir.display(),
                        force_empty = true,
                        "snapshot file is corrupted but --force-empty was set, \
                         starting with empty table set (data loss accepted by operator)"
                    );
                    HashMap::new()
                } else {
                    tracing::error!(
                        error = %e,
                        data_dir = %data_dir.display(),
                        "failed to load snapshot, refusing to start to avoid data loss \
                         (use --force-empty to override and start with empty table set)"
                    );
                    return Err(e);
                }
            }
        }
    };

    // P0-1 修复：应用 WAL 中的 TableData 记录到已加载的表集合
    //
    // 顺序：先加载 JSON 快照（基础数据）→ 再回放 WAL TableData（增量提交）
    // WAL 回放保证 ACID：仅应用紧随其后有 Commit 记录的 TableData，
    // Abort 记录后的 TableData 被丢弃，未完成事务的 TableData 也不会应用。
    if !wal_records.is_empty() {
        let applied = persistence::apply_wal_table_data(&mut loaded_tables, &wal_records);
        tracing::info!(
            applied_table_count = applied,
            "WAL TableData records applied to loaded tables (crash recovery)"
        );
    }

    // OPT-3：接入 BufferPool 存储磁盘化
    //
    // 为每张已加载的表启用 BufferPool 持久化后端，数据文件落盘到 {data_dir}/{table_name}.db。
    // - 仅当 --data-dir 非空时启用（空表示持久化被禁用，跳过保持纯内存模式）
    // - enable_persistence 内部用 FilePageWriter/FilePageLoader 打开文件后端，文件不存在会自动创建
    // - 单表失败仅记录 warning，不中断启动（避免单表故障影响整体可用性）
    if !args.data_dir.is_empty() {
        for (name, table_arc) in &loaded_tables {
            let db_path = data_dir.join(format!("{name}.db"));
            let mut table = table_arc.lock().await;
            match table.enable_persistence(&db_path) {
                Ok(()) => {
                    tracing::info!(
                        table = %name,
                        path = %db_path.display(),
                        "OPT-3: BufferPool persistence enabled for table"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        table = %name,
                        path = %db_path.display(),
                        error = %e,
                        "OPT-3: failed to enable BufferPool persistence for table, \
                         continuing without persistence (table stays in-memory only)"
                    );
                }
            }

            // P1-1：启用分页存储主路径（Vec<Row> 热缓存 + BufferPool 分页主存）
            //
            // 独立于 persistence（整表快照），paged_storage 按行分页存储，支持：
            // - 增量更新（spill_to_paged_storage 不清空 rows，作为持久化镜像）
            // - 按页读取（restore_from_paged_storage 逐页重建 rows）
            // - 自动溢出（insert/bulk_insert 后行数超阈值时自动 spill）
            //
            // 单表失败仅记录 warning，不中断启动（与 persistence 一致的容错策略）
            let paged_path = data_dir.join(format!("{name}.paged"));
            if let Err(e) = table.enable_paged_storage(&paged_path) {
                tracing::warn!(
                    table = %name,
                    path = %paged_path.display(),
                    error = %e,
                    "P1-1: enable_paged_storage failed, continuing without paged storage \
                     (table stays with Vec<Row> hot cache only)"
                );
            }
        }
    }

    let shared_tables = Arc::new(RwLock::new(loaded_tables));
    let lock_manager = Arc::new(LockManager::new());
    let shared_txn_counter = Arc::new(AtomicU32::new(1));
    // P1-2：跨会话共享的脏表跟踪器（用于增量快照机制）
    let dirty_tracker = persistence::DirtyTableTracker::new();
    // P0-TX-1 修复：创建共享 MVCC 事务管理器
    // 启用 MVCC 事务可见性判断、SSI 写偏斜检测、First-Committer-Wins
    let mvcc_manager = Arc::new(MvccManager::new());
    tracing::info!("MVCC transaction manager initialized");

    // P0-DIST-1/2/3：初始化分布式运行时（DistRuntime）
    //
    // 将 Raft 共识、TSO 时间戳服务、Multi-Raft 分片整合为统一的 DistRuntime，
    // 作为 Arc<RwLock<DistRuntime>> 共享资源注入到 session/executor。
    //
    // 第一轮迭代（当前）：
    // - 单节点模式：Raft 自选举为 Leader，无跨节点 RPC
    // - 单分片：一个 Range 覆盖全键空间
    // - TSO：全局单调递增时间戳，与 MVCC 协同
    // - 真实写入路径：put/delete 通过 Raft propose → advance_commit → apply
    //
    // 后续迭代：
    // - 迭代 2：多节点集群，跨节点日志复制 + 故障恢复
    // - 迭代 3：Percolator 跨分片 2PC，TSO 与 MVCC 时间戳协同
    // 注：DistRuntime 已初始化并通过 12 个集成测试 + 6 个 Executor 集成测试验证。
    // P0-DIST 修复：实际注入到 PgwireServer → Session → Executor 完整链路（之前是 _dist_runtime 未注入）。
    // P8-3：支持多节点集群模式（--cluster-mode cluster）
    let dist_runtime = if args.cluster_mode == "cluster" {
        // P8-3：多节点集群模式
        let node_id = args.node_id;
        let raft_listen_addr = args.raft_listen_addr.as_deref().unwrap_or("127.0.0.1:7000");
        let peers_str = args.peers.as_deref().unwrap_or("");
        let tick_ms = args.raft_tick_ms;

        // 解析 peers: "1@host:port,2@host:port,..."
        let mut all_nodes: Vec<szrsql_dist::raft::NodeId> = Vec::new();
        let mut peer_addrs: Vec<(szrsql_dist::raft::NodeId, std::net::SocketAddr)> = Vec::new();
        for entry in peers_str.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let parts: Vec<&str> = entry.splitn(2, '@').collect();
            if parts.len() != 2 {
                tracing::warn!(entry = entry, "invalid peer entry, expected node_id@host:port");
                continue;
            }
            let nid: u64 = match parts[0].parse() {
                Ok(n) => n,
                Err(_) => {
                    tracing::warn!(entry = entry, "invalid node_id in peer entry");
                    continue;
                }
            };
            let addr: std::net::SocketAddr = match parts[1].parse() {
                Ok(a) => a,
                Err(_) => {
                    tracing::warn!(entry = entry, "invalid socket addr in peer entry");
                    continue;
                }
            };
            all_nodes.push(nid);
            peer_addrs.push((nid, addr));
        }

        if !all_nodes.contains(&node_id) {
            all_nodes.push(node_id);
        }
        all_nodes.sort_unstable();
        all_nodes.dedup();

        tracing::info!(
            node_id = node_id,
            all_nodes = ?all_nodes,
            peer_count = peer_addrs.len(),
            raft_listen_addr = raft_listen_addr,
            tick_ms = tick_ms,
            "P8-3: starting in cluster mode"
        );

        match szrsql_dist::runtime::new_cluster_node_runtime(node_id, &all_nodes, 42) {
            Ok(handle) => {
                // 创建 TcpNetwork 并启动监听
                let network = std::sync::Arc::new(szrsql_dist::network::TcpNetwork::new(node_id));
                for (peer_id, addr) in &peer_addrs {
                    if *peer_id != node_id {
                        network.add_peer(*peer_id, *addr);
                    }
                }
                let listen_addr: std::net::SocketAddr = raft_listen_addr.parse()
                    .unwrap_or_else(|_| "127.0.0.1:7000".parse().unwrap());
                if let Err(e) = network.start_listener(listen_addr) {
                    tracing::warn!(error = %e, "P8-3: TcpNetwork listener start failed");
                }

                // 创建并启动集群驱动器
                let mut driver = szrsql_dist::runtime::ClusterDriver::new(
                    std::sync::Arc::clone(&handle),
                    std::sync::Arc::clone(&network),
                    tick_ms,
                );
                if let Err(e) = driver.start() {
                    tracing::warn!(error = %e, "P8-3: ClusterDriver start failed");
                }
                // driver 必须存活到进程结束，forget 防止 Drop::stop 终止线程
                std::mem::forget(driver);

                tracing::info!(
                    node_id = node_id,
                    shard_count = handle.read().shard_ids().len(),
                    listen_addr = ?network.listen_addr(),
                    peer_count = network.peer_count(),
                    "P8-3: cluster node initialized (TCP network + driver thread)"
                );
                Some(handle)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "P8-3: Failed to initialize cluster DistRuntime, falling back without distributed runtime"
                );
                None
            }
        }
    } else {
        // 单节点模式（默认）
        match szrsql_dist::runtime::new_single_node_runtime(1) {
            Ok(handle) => {
                // 初始化：所有分片 Raft 组自选举为 Leader
                {
                    let mut rt = handle.write();
                    if let Err(e) = rt.init() {
                        tracing::warn!(error = %e, "DistRuntime init failed, continuing in degraded mode");
                    }
                }
                tracing::info!(
                    node_id = 1,
                    shard_count = handle.read().shard_ids().len(),
                    current_ts = handle.read().current_timestamp(),
                    "P0-DIST-1/2/3: DistRuntime initialized (single-node, Raft + TSO + Multi-Raft integrated)"
                );
                Some(handle)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to initialize DistRuntime, continuing without distributed runtime"
                );
                None
            }
        }
    };

    // P2-1：Multi-Master/DistTxn 组件初始化
    //
    // 当 --multi-master 启用时，构造以下三大组件：
    //
    // 1. HlcClock — 混合逻辑时钟（Hybrid Logical Clock, Kulkarni 2014）
    //    - 结合物理时钟（毫秒）与逻辑计数器，在节点间时钟偏差下仍能
    //      正确排序因果相关的事件
    //    - HlcClock 本身为单线程实现，用 Arc<Mutex<>> 包装为线程安全
    //    - 物理时钟使用 SystemTime::now() → 毫秒级 Unix 时间戳
    //
    // 2. ConflictLog — 冲突日志
    //    - 按时间顺序记录 Multi-Master 场景下的写入冲突事件
    //    - 支持持久化编解码（encode/decode），用于审计与回放
    //    - 用 Arc<Mutex<>> 包装为线程安全
    //
    // 3. ClusterTxnCoordinator — 跨节点事务协调器
    //    - 基于 DistCluster 实现 Percolator 两阶段提交（prewrite→commit）
    //    - 自动路由到当前 Leader，Leader 故障时自动重试
    //    - 需要 &mut DistCluster（生命周期绑定），因此存储 DistCluster 本身，
    //      Coordinator 按需从 DistCluster 创建
    //
    // 当前阶段：构造并存储实例，为后续接入执行路径（Executor/Session）奠定基础。
    // 不影响现有单节点模式和集群模式的正常运行。
    let multi_master_components = if args.multi_master {
        // === 1. 构造 HlcClock（混合逻辑时钟）===
        // 物理时钟闭包：返回当前 Unix 时间戳（毫秒）
        // unwrap_or(0) 在 SystemTime 错误（如时钟回拨极端场景）时回退到 0，不 panic
        let hlc_clock = szrsql_dist::conflict::HlcClock::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        });
        let hlc_arc = Arc::new(std::sync::Mutex::new(hlc_clock));
        tracing::info!(
            "P2-1: HlcClock initialized (Hybrid Logical Clock for multi-master causal ordering)"
        );

        // === 2. 构造 ConflictLog（冲突日志）===
        let conflict_log = szrsql_dist::conflict::ConflictLog::new();
        let conflict_log_arc = Arc::new(std::sync::Mutex::new(conflict_log));
        tracing::info!(
            "P2-1: ConflictLog initialized (multi-master conflict audit log)"
        );

        // === 3. 构造 DistCluster + ClusterTxnCoordinator ===
        // ClusterTxnCoordinator::new 签名要求 &mut DistCluster（生命周期绑定），
        // 因此先构造并初始化 DistCluster，Coordinator 按需创建。
        //
        // 节点 ID 列表来源：
        // - 有 --peers 配置：解析其中的 node_id 部分
        // - 无 --peers 配置：默认 3 节点开发集群 [1, 2, 3]
        let node_id = args.node_id;
        let peers_str = args.peers.as_deref().unwrap_or("");
        let cluster_node_ids: Vec<szrsql_dist::raft::NodeId> = if !peers_str.is_empty() {
            // 解析 "1@host:port,2@host:port,..." 中的 node_id 部分
            let mut ids: Vec<u64> = peers_str
                .split(',')
                .filter_map(|entry| {
                    entry.trim().splitn(2, '@').next().and_then(|s| s.parse().ok())
                })
                .collect();
            if !ids.contains(&node_id) {
                ids.push(node_id);
            }
            ids.sort_unstable();
            ids.dedup();
            ids
        } else {
            vec![1, 2, 3]
        };

        match szrsql_dist::cluster::DistCluster::new(&cluster_node_ids, 42) {
            Ok(mut cluster) => {
                // 初始化集群：运行 Raft 选举（500ms 足以保证 Leader 产生）
                if let Err(e) = cluster.init() {
                    tracing::warn!(
                        error = %e,
                        "P2-1: DistCluster init failed; coordinator may operate in degraded mode"
                    );
                }

                // 构造 ClusterTxnCoordinator 并验证可用性（调用 begin 获取初始时间戳）
                {
                    let mut coordinator =
                        szrsql_dist::dist_txn::ClusterTxnCoordinator::new(&mut cluster);
                    let start_ts = coordinator.begin();
                    tracing::info!(
                        start_ts = start_ts,
                        node_ids = ?cluster_node_ids,
                        leader = ?cluster.leader(),
                        "P2-1: ClusterTxnCoordinator initialized (cross-node Percolator 2PC coordinator)"
                    );
                }

                // 存储 DistCluster（Coordinator 是借用，按需从 cluster 创建）
                let cluster_arc = Arc::new(std::sync::Mutex::new(cluster));
                Some((hlc_arc, conflict_log_arc, Some(cluster_arc)))
            }
            Err(e) => {
                // DistCluster 构造失败时，HlcClock 和 ConflictLog 仍然保留
                tracing::warn!(
                    error = %e,
                    node_ids = ?cluster_node_ids,
                    "P2-1: DistCluster creation failed; HlcClock and ConflictLog remain active"
                );
                Some((hlc_arc, conflict_log_arc, None))
            }
        }
    } else {
        None
    };

    // P7-1：初始化 CDC 引擎（变更数据捕获）
    //
    // 创建跨会话共享的 CdcEngine，注入到 PgwireServer → Session → Executor，
    // 使所有 DML 操作（INSERT/UPDATE/DELETE）将行级变更事件分发到 CDC 引擎，
    // 供已注册的 CdcObserver（如 ReplicationTask）消费。
    //
    // 事件流向：
    //   Executor.mvcc_insert/update/delete → dispatch_cdc_* → CdcEngine.dispatch_event
    //     → CdcObserverManager.notify → 所有已注册 CdcObserver.on_event
    //
    // 下游消费者（如 ReplicationTaskManager）可通过 cdc_engine.register_observer_arc
    // 注册自身为 CdcObserver，接收实时变更事件流。
    let cdc_engine = {
        let observer_manager = Arc::new(szrsql_cdc::CdcObserverManager::new());
        let engine = Arc::new(szrsql_cdc::CdcEngine::new(observer_manager));
        tracing::info!(
            observer_count = engine.observer_count(),
            "P7-1: CDC engine initialized (DML event dispatch enabled)"
        );
        engine
    };

    // P7-4：初始化 ReplicationTaskManager + MCP 服务器
    //
    // 构造 ReplicationTaskManager（共享 cdc_engine，独立 slot_manager/decoder/schema_registry）
    // 并在 `--mcp-stdio` 启用时启动 MCP stdio 主循环，暴露 35 个 LLM 工具。
    //
    // ReplicationTaskManager 是 5 个 Replication 类 MCP 工具的执行后端：
    //   create_replication_task / list_replication_tasks /
    //   monitor_replication_task / stop_replication_task /
    //   replication_manager_stats
    //
    // MCP 服务器与 pgwire 服务器共享 cdc_engine，所有任务管理操作真实生效。
    let replication_task_manager: Arc<szrsql_cdc::task::ReplicationTaskManager> = {
        let slot_manager = Arc::new(szrsql_cdc::slot::SlotManager::in_memory());
        let schema_registry = Arc::new(szrsql_cdc::schema::SchemaRegistry::new());
        let decoder = Arc::new(szrsql_cdc::decoder::RowDecoder::new(schema_registry.clone()));
        Arc::new(szrsql_cdc::task::ReplicationTaskManager::new(
            slot_manager,
            decoder,
            schema_registry,
            cdc_engine.clone(),
        ))
    };
    tracing::info!(
        "P7-4: ReplicationTaskManager initialized (shared CdcEngine, in-memory SlotManager)"
    );

    // P7-4：启动 MCP stdio 服务器（独立线程，不阻塞 pgwire 主路径）
    let mcp_handle: Option<std::thread::JoinHandle<Result<(), szrsql_ai::mcp::McpError>>> =
        if args.mcp_stdio {
            tracing::info!(
                "P7-4: starting MCP stdio server (35 tools, 9 categories, ReplicationTaskManager injected)"
            );
            // 构造一个独立的 ManagedCatalog 供 MCP 的 Schema 类工具读取元数据
            // （MCP 服务器与 pgwire 的执行器 catalog 不共享，schema 同步需后续 P8 阶段补齐）
            let catalog: Box<dyn szrsql_catalog::MutableCatalog> =
                Box::new(szrsql_catalog::ManagedCatalog::new());
            let backend = szrsql_ai::mcp_server::CatalogBackend::new(catalog)
                .with_replication(replication_task_manager.clone());
            let mut mcp_server = szrsql_ai::mcp_server::McpServerV2::new(Box::new(backend));
            let handle = std::thread::Builder::new()
                .name("szrsql-mcp-stdio".to_string())
                .spawn(move || mcp_server.run_stdio())
                .map_err(|e| anyhow::anyhow!("spawn MCP stdio thread failed: {e}"))?;
            Some(handle)
        } else {
            None
        };

    // Clone for MySQL/TDS/Oracle servers (跨协议共享同一份表存储)
    let mysql_shared_tables = shared_tables.clone();
    let mysql_lock_manager = lock_manager.clone();
    let mysql_shared_txn_counter = shared_txn_counter.clone();

    // Phase 6.4：启动后台周期性快照保存任务（默认每 5 秒）
    // P1-2：使用增量快照机制（仅保存脏表，无 DML 时跳过 IO）
    // 服务器关闭时 abort 该任务并执行最后一次同步保存
    let persistence_handle = if !args.data_dir.is_empty() {
        let persist_tables = mysql_shared_tables.clone();
        let persist_dir = data_dir.clone();
        Some(persistence::spawn_periodic_incremental_save(
            persist_tables,
            persist_dir,
            5,
            dirty_tracker.clone(),
        ))
    } else {
        None
    };
    // 保存一份 shared_tables 的 Arc 引用用于关闭时最终保存
    let shutdown_persist_tables = mysql_shared_tables.clone();
    let shutdown_dirty_tracker = dirty_tracker.clone();

    // 生产监控告警：创建共享的 Prometheus 指标注册表
    // 同一实例注入 PgwireServer（用于计数）和 HttpServer（用于暴露 /metrics）
    let metrics_registry = Arc::new(MetricsRegistry::new());

    let mut server_builder = PgwireServer::new(config)
        .with_concurrency(shared_tables, lock_manager)
        .with_shared_txn_counter(shared_txn_counter)
        .with_mvcc(mvcc_manager.clone())
        .with_metrics(metrics_registry.clone());
    // P0-DIST-1/2/3：注入分布式运行时句柄（实际接入，非 _dist_runtime 假装入）
    if let Some(dist_rt) = dist_runtime {
        server_builder = server_builder.with_dist_runtime(dist_rt);
    }
    // P7-1：注入 CDC 引擎，启用 DML 事件分发
    server_builder = server_builder.with_cdc_engine(cdc_engine);
    if let Some(writer) = wal_writer {
        server_builder = server_builder.with_wal_writer(writer);
    }
    // P2-1：注入 Multi-Master 组件（HLC 时钟 + 冲突日志 + 节点 ID）
    // 启用 --multi-master 时，将 HlcClock/ConflictLog 注入 PgwireServer，
    // 通过 Session 传递给 Executor，在 DML 路径中生成 HLC 时间戳和记录冲突事件
    if let Some((hlc_arc, conflict_log_arc, _cluster_arc)) = &multi_master_components {
        server_builder = server_builder
            .with_hlc_clock(hlc_arc.clone())
            .with_conflict_log(conflict_log_arc.clone())
            .with_node_id(args.node_id);
        tracing::info!(
            node_id = args.node_id,
            "P2-1: Multi-Master components injected into PgwireServer (HlcClock + ConflictLog)"
        );
    }
    // P1-2：注入脏表跟踪器，启用增量快照机制
    // session 在事务 COMMIT 成功后会调用 tracker.mark_dirty_many 标记修改过的表，
    // 后台周期性快照任务仅序列化脏表，避免无 DML 时的无谓 IO
    server_builder = server_builder.with_dirty_tracker(Arc::new(dirty_tracker));
    // P2-1.1：注入跨会话共享的统计信息存储，启用 ANALYZE 命令
    // ANALYZE 扫描表数据收集统计信息（行数、NDV、min/max、直方图），
    // 结果存入共享 store，供 CostModel 进行基于成本的优化（P2-1.2 激活）
    server_builder = server_builder.with_statistics_store(Arc::new(std::sync::Mutex::new(
        szrsql_optimizer::statistics::InMemoryStatisticsStore::new(),
    )));
    // OPT-12：注入安全模块（SQL 防火墙 + 审计日志）
    // 防火墙：默认启用 SQL 注入检测（不配置白名单/禁止命令时仅拦截注入特征）
    // 审计日志：默认启用，记录所有 SQL 执行事件（SHA-256 哈希链防篡改）
    if !args.no_firewall {
        let firewall = Arc::new(tokio::sync::Mutex::new(
            szrsql_security::firewall::SqlFirewall::new(),
        ));
        server_builder = server_builder.with_security_firewall(firewall);
        tracing::info!("OPT-12: SQL firewall enabled (injection detection active)");
    }
    if !args.no_audit_log {
        let mut audit = szrsql_security::audit::AuditLog::new();
        audit.enable();
        server_builder = server_builder.with_audit_log(Arc::new(tokio::sync::Mutex::new(audit)));
        tracing::info!("OPT-12: audit log enabled (SHA-256 hash chain)");
    }

    // P2-2.2：TCP 流复制初始化
    //
    // 两种角色（互斥）：
    // - 主库（--repl-port != 0）：创建 ReplicationPrimary，启动 TcpReplicationServer 监听备库连接，
    //   注入到 PgwireServer，使 COMMIT 路径将 WAL 记录推送到 ReplicationPrimary 扇出到备库。
    // - 备库（--replica-of <addr>）：启动 TcpReplicationClient 连接主库，接收 WAL 复制流。
    //
    // 前置条件：主库模式需启用 WAL（--wal-path 或默认），否则 COMMIT 路径不产生 WAL 记录。
    if args.repl_port != 0 && args.replica_of.is_some() {
        anyhow::bail!("--repl-port and --replica-of are mutually exclusive (a node cannot be both primary and replica)");
    }
    let replication_primary: Option<Arc<szrsql_replication::stream::ReplicationPrimary>> = if args.repl_port != 0 {
        let primary = Arc::new(szrsql_replication::stream::ReplicationPrimary::new("primary-1"));
        let repl_addr: std::net::SocketAddr = format!("{}:{}", listen_host, args.repl_port)
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid --repl-port address: {e}"))?;
        let tcp_server = szrsql_replication::tcp_transport::TcpReplicationServer::new(
            primary.clone(),
            repl_addr,
        );
        // 启动 TCP 复制服务器（异步，返回 JoinHandle）
        match tcp_server.spawn().await {
            Ok(handle) => {
                tracing::info!(
                    repl_addr = %repl_addr,
                    "P2-2.2: TCP replication server started (primary mode, accepting replica connections)"
                );
                // detach 任务：TcpReplicationServer 在 tokio task 中运行，
                // 进程退出时自动终止。handle 保留防止被立即 cancel。
                std::mem::forget(handle);
            }
            Err(e) => {
                tracing::error!(error = %e, repl_addr = %repl_addr, "P2-2.2: failed to start TCP replication server");
                return Err(e.into());
            }
        }
        // 注入到 PgwireServer（COMMIT 路径将 WAL 记录推送到此 primary）
        server_builder = server_builder.with_replication_primary(primary.clone());
        Some(primary)
    } else {
        None
    };

    // P2-2.2：备库模式 — 连接主库接收 WAL 复制流
    let replica_handles: Option<(tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>)> = if let Some(primary_addr) = &args.replica_of {
        let addr: std::net::SocketAddr = primary_addr
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid --replica-of address '{primary_addr}': {e}"))?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<szrsql_replication::stream::ReplicationMessage>();
        let client = szrsql_replication::tcp_transport::TcpReplicationClient::new(addr);
        tracing::info!(
            primary_addr = %addr,
            "P2-2.2: starting in replica mode, connecting to primary"
        );
        // 启动 TCP 复制客户端（带重试：5 次，每次间隔 1 秒）
        match client.connect_with_retry(5, std::time::Duration::from_secs(1), tx).await {
            Ok(handle) => {
                // 启动 WAL 回放任务：从通道接收 ReplicationMessage 并记录日志
                // （完整回放到本地存储需要后续 P2-2.3 阶段实现，当前仅记录接收计数）
                let replay_handle = tokio::spawn(async move {
                    let mut received_count: u64 = 0;
                    let mut last_lsn: u64 = 0;
                    while let Some(msg) = rx.recv().await {
                        match msg {
                            szrsql_replication::stream::ReplicationMessage::WalBatch { records, start_lsn, end_lsn } => {
                                received_count += records.len() as u64;
                                last_lsn = end_lsn;
                                tracing::debug!(
                                    batch_size = records.len(),
                                    start_lsn,
                                    end_lsn,
                                    total_received = received_count,
                                    "P2-2.2: replica received WalBatch"
                                );
                            }
                            szrsql_replication::stream::ReplicationMessage::Heartbeat { current_lsn } => {
                                last_lsn = current_lsn;
                                tracing::trace!(
                                    current_lsn,
                                    total_received = received_count,
                                    "P2-2.2: replica received Heartbeat"
                                );
                            }
                            szrsql_replication::stream::ReplicationMessage::Eof => {
                                tracing::info!(
                                    total_received = received_count,
                                    last_lsn,
                                    "P2-2.2: replica received Eof from primary, disconnecting"
                                );
                                break;
                            }
                        }
                    }
                    tracing::info!(
                        total_received = received_count,
                        last_lsn,
                        "P2-2.2: replica receiver task exited"
                    );
                });
                tracing::info!(primary_addr = %addr, "P2-2.2: replica connected to primary");
                Some((handle, replay_handle))
            }
            Err(e) => {
                tracing::error!(error = %e, primary_addr = %addr, "P2-2.2: failed to connect to primary after retries");
                return Err(e.into());
            }
        }
    } else {
        None
    };

    let server = server_builder;

    // Phase 4.5.8-4.5.10：HTTP 管理服务器
    // 与 pgwire 服务器共享 ShutdownCoordinator 的 watch 通道
    let http_handle = if args.http_port != 0 {
        let mut http_config = HttpConfig::new()
            .with_host(args.http_host.clone())
            .with_port(args.http_port);
        if let Some(token) = &args.http_auth_token {
            http_config = http_config.with_auth_token(token);
        }
        let metrics = metrics_registry.clone();
        // 订阅 pgwire 服务器的关闭状态
        let shutdown_rx = server.shutdown_coordinator().subscribe();
        // P8-2：构造 CdcService 并注入 HttpServer，启用 /api/v1/cdc/* REST API
        // 端点：租户 CRUD、任务生命周期管理、使用量查询（见 http.rs:332-344）
        let cdc_service = Arc::new(szrsql_cdc::service::CdcService::new(
            replication_task_manager.clone(),
        ));
        let http_server = HttpServer::new(http_config, metrics, shutdown_rx)
            .with_cdc_service(cdc_service);
        let http_host = args.http_host.clone();
        let http_port = args.http_port;
        Some(tokio::spawn(async move {
            tracing::info!(
                host = %http_host,
                port = http_port,
                "HTTP management server starting"
            );
            if let Err(e) = http_server.serve().await {
                tracing::error!(error = %e, "HTTP management server exited with error");
            }
        }))
    } else {
        None
    };

    // Phase 4.5：MySQL 协议监听（L2 协议级兼容）
    // 启用后 Navicat 可用 MySQL 协议连接，典型端口 3306/3307
    let mysql_handle = if args.mysql_port != 0 {
        let mysql_config = szrsql_mysql_protocol::MysqlConfig::new()
            .with_host(listen_host.clone())
            .with_port(args.mysql_port)
            .with_server_version("8.0-szrsql".to_string())
            .with_auth_mode(szrsql_mysql_protocol::AuthMode::Trust)
            .with_connection_idle_timeout(std::time::Duration::from_secs(args.connection_idle_timeout));
        let mysql_server = szrsql_mysql_protocol::MysqlServer::new(mysql_config)
            .with_shared_tables(mysql_shared_tables)
            .with_lock_manager(mysql_lock_manager)
            .with_shared_txn_counter(mysql_shared_txn_counter);
        let mysql_host = listen_host.clone();
        let mysql_port = args.mysql_port;
        Some(tokio::spawn(async move {
            tracing::info!(
                host = %mysql_host,
                port = mysql_port,
                "MySQL protocol server starting (L2 wire-compatible)"
            );
            if let Err(e) = mysql_server.serve().await {
                tracing::error!(error = %e, "MySQL protocol server exited with error");
            }
        }))
    } else {
        None
    };

    // Phase 4.5：TDS 协议监听（L2 协议级兼容，SQL Server）
    // 启用后 Navicat 可用 SQL Server 协议连接，典型端口 1433
    let tds_handle = if args.tds_port != 0 {
        let tds_config = szrsql_tds_protocol::TdsConfig::new()
            .with_host(listen_host.clone())
            .with_port(args.tds_port)
            .with_server_version("15.0-szrsql".to_string())
            .with_auth_mode(szrsql_tds_protocol::AuthMode::Trust)
            .with_connection_idle_timeout(std::time::Duration::from_secs(args.connection_idle_timeout));
        let tds_server = szrsql_tds_protocol::TdsServer::new(tds_config);
        let tds_host = listen_host.clone();
        let tds_port = args.tds_port;
        Some(tokio::spawn(async move {
            tracing::info!(
                host = %tds_host,
                port = tds_port,
                "TDS protocol server starting (L2 wire-compatible, SQL Server)"
            );
            if let Err(e) = tds_server.serve().await {
                tracing::error!(error = %e, "TDS protocol server exited with error");
            }
        }))
    } else {
        None
    };

    // Phase 4.5：Oracle 协议监听（L2 协议级兼容，Oracle Net/TNS）
    // 启用后 Navicat 可用 Oracle 协议连接，典型端口 1521/1522
    let oracle_handle = if args.oracle_port != 0 {
        let oracle_config = szrsql_oracle_bridge::OracleConfig::new()
            .with_host(listen_host.clone())
            .with_port(args.oracle_port)
            .with_service_name(args.oracle_service_name.clone())
            .with_connection_idle_timeout(std::time::Duration::from_secs(args.connection_idle_timeout));
        let oracle_server = szrsql_oracle_bridge::OracleServer::new(oracle_config);
        let oracle_host = listen_host.clone();
        let oracle_port = args.oracle_port;
        let oracle_service = args.oracle_service_name.clone();
        Some(tokio::spawn(async move {
            tracing::info!(
                host = %oracle_host,
                port = oracle_port,
                service = %oracle_service,
                "Oracle Net server starting (L2 wire-compatible, TNS protocol)"
            );
            if let Err(e) = oracle_server.serve().await {
                tracing::error!(error = %e, "Oracle Net server exited with error");
            }
        }))
    } else {
        None
    };

    // Phase 4.5：SQLite 协议监听（JSON 行协议 TCP 入口）
    // 启用后客户端可通过 TCP 发送 JSON 请求执行 SQL，典型端口 9432
    let sqlite_handle = if args.sqlite_port != 0 {
        let sqlite_config = szrsql_sqlite_bridge::SqliteConfig::new()
            .with_host(listen_host.clone())
            .with_port(args.sqlite_port)
            .with_server_version("3.45-szrsql".to_string())
            .with_connection_idle_timeout(std::time::Duration::from_secs(args.connection_idle_timeout));
        let sqlite_server = szrsql_sqlite_bridge::SqliteServer::new(sqlite_config);
        let sqlite_host = listen_host.clone();
        let sqlite_port = args.sqlite_port;
        Some(tokio::spawn(async move {
            tracing::info!(
                host = %sqlite_host,
                port = sqlite_port,
                "SQLite server starting (JSON line protocol over TCP)"
            );
            if let Err(e) = sqlite_server.serve().await {
                tracing::error!(error = %e, "SQLite server exited with error");
            }
        }))
    } else {
        None
    };

    // Phase 4.12：安装信号处理器，返回 ShutdownSignal（Graceful=SIGTERM / Immediate=SIGINT）
    let shutdown_signal = setup_signal_handler();

    // 根据信号类型执行关闭策略：
    // - Graceful（SIGTERM）：等待活跃连接排空（最多 shutdown_timeout）
    // - Immediate（SIGINT/Ctrl+C）：立即 abort_all，不等待
    if let Err(e) = server.serve_with_shutdown(shutdown_signal).await {
        tracing::error!(error = %e, "pgwire server exited with error");
        // 即使 pgwire 失败，也要等待 HTTP/MySQL/TDS/Oracle 服务器退出
        if let Some(handle) = http_handle {
            let _ = handle.await;
        }
        if let Some(handle) = mysql_handle {
            handle.abort();
        }
        if let Some(handle) = tds_handle {
            handle.abort();
        }
        if let Some(handle) = oracle_handle {
            handle.abort();
        }
        if let Some(handle) = sqlite_handle {
            handle.abort();
        }
        // P2-2.2：清理复制资源
        if let Some(primary) = &replication_primary {
            primary.shutdown();
        }
        if let Some((tcp_handle, replay_handle)) = &replica_handles {
            tcp_handle.abort();
            replay_handle.abort();
        }
        return Err(e.into());
    }

    tracing::info!("SzRSQL pgwire server shutdown complete");

    // 等待 HTTP 服务器退出（pgwire shutdown_with_signal 会将状态切换为 Closed，
    // HTTP 服务器的 select! 检测到 Closed 后会退出 serve 循环）
    if let Some(handle) = http_handle {
        match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
            Ok(Ok(())) => tracing::info!("HTTP management server shutdown complete"),
            Ok(Err(e)) => tracing::warn!(error = %e, "HTTP management server task panicked"),
            Err(_) => tracing::warn!("HTTP management server shutdown timeout, aborting"),
        }
    }

    // Phase 4.5：终止 MySQL/TDS/Oracle 协议监听（pgwire 退出时一并终止）
    if let Some(handle) = mysql_handle {
        handle.abort();
        tracing::info!("MySQL protocol server shutdown (aborted)");
    }
    if let Some(handle) = tds_handle {
        handle.abort();
        tracing::info!("TDS protocol server shutdown (aborted)");
    }
    if let Some(handle) = oracle_handle {
        handle.abort();
        tracing::info!("Oracle Net server shutdown (aborted)");
    }
    if let Some(handle) = sqlite_handle {
        handle.abort();
        tracing::info!("SQLite server shutdown (aborted)");
    }

    // P2-2.2：流复制优雅关闭
    // - 主库模式：向所有已连接备库发送 Eof，通知备库正常断开
    // - 备库模式：终止接收任务（主库侧发送 Eof 或连接断开后自然退出）
    if let Some(primary) = &replication_primary {
        primary.shutdown();
        tracing::info!(
            replica_count = primary.replica_count(),
            "P2-2.2: replication primary graceful shutdown (Eof sent to all replicas)"
        );
    }
    if let Some((tcp_handle, replay_handle)) = &replica_handles {
        tcp_handle.abort();
        replay_handle.abort();
        tracing::info!("P2-2.2: replica receiver task stopped");
    }

    // Phase 6.4：终止周期性保存任务，执行最后一次同步保存
    if let Some(handle) = persistence_handle {
        handle.abort();
        tracing::info!("periodic snapshot save task stopped");
    }
    if !args.data_dir.is_empty() {
        tracing::info!("saving final snapshot before shutdown...");
        // P1-2：使用增量快照保存最后的脏表（若无 DML 则跳过）
        if let Err(e) =
            persistence::save_incremental_snapshot(&shutdown_persist_tables, &data_dir, &shutdown_dirty_tracker).await
        {
            tracing::warn!(error = %e, "final incremental snapshot save failed");
        } else {
            tracing::info!("final snapshot saved successfully");
        }
    }

    // P7-4：等待 MCP stdio 线程退出
    // MCP 主循环在 stdin EOF 或收到 shutdown 请求时退出，
    // 此处 join 确保线程资源正确回收（最多等待 5 秒，超时强制 detach）
    if let Some(handle) = mcp_handle {
        tracing::info!("waiting for MCP stdio server to exit...");
        match handle.join() {
            Ok(Ok(())) => tracing::info!("MCP stdio server shutdown complete"),
            Ok(Err(e)) => tracing::warn!(error = ?e, "MCP stdio server exited with error"),
            Err(_) => tracing::warn!("MCP stdio server thread panicked"),
        }
    }

    // _pid_file 在此处 drop，自动删除 PID 文件
    Ok(())
    })
}

/// Phase 4.12：安装信号处理器，返回一个在收到信号时完成的 future。
///
/// 返回 `ShutdownSignal` 区分关闭策略：
/// - **SIGTERM**（Unix）/ **Ctrl+C**（Windows）→ `ShutdownSignal::Graceful`：优雅关闭
/// - **SIGINT / Ctrl+C**（Unix）→ `ShutdownSignal::Immediate`：立即关闭
///
/// # 平台差异
///
/// - **Unix**：同时监听 SIGTERM（优雅）和 SIGINT（立即）；Ctrl+C 在 Unix 上等价于 SIGINT
/// - **Windows**：仅监听 Ctrl+C（映射为 `Graceful`，因为 Windows 无 SIGTERM 概念）
///
/// # 信号优先级
///
/// `tokio::select!` 公平竞争，先到的信号触发关闭。两个信号同时到达时行为不确定。
///
/// 信号处理是协同式的——只在 await 点检查信号，不会中断正在执行的代码。
async fn setup_signal_handler() -> ShutdownSignal {
    // Unix：监听 SIGTERM（优雅）和 SIGINT（立即）
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let terminate = async {
            match signal(SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "SIGTERM signal handler failed");
                    std::future::pending::<()>().await;
                }
            }
        };

        let interrupt = async {
            match signal(SignalKind::interrupt()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "SIGINT signal handler failed");
                    std::future::pending::<()>().await;
                }
            }
        };

        tokio::select! {
            _ = terminate => {
                tracing::info!("received SIGTERM, initiating graceful shutdown");
                ShutdownSignal::Graceful
            }
            _ = interrupt => {
                tracing::info!("received SIGINT (Ctrl+C), initiating immediate shutdown");
                ShutdownSignal::Immediate
            }
        }
    }

    // Windows：仅监听 Ctrl+C（映射为 Graceful，因为 Windows 无 SIGTERM）
    #[cfg(not(unix))]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "ctrl_c signal handler failed");
            std::future::pending::<()>().await;
        }
        tracing::info!("received Ctrl+C, initiating graceful shutdown");
        ShutdownSignal::Graceful
    }
}
