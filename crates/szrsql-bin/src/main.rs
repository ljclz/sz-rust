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

use clap::Parser;
use szrsql_protocol::pgwire::{
    daemonize, install_crash_handler, CrashConfig, PgwireConfig, PgwireServer, PidFile,
    ShutdownSignal,
};
use szrsql_protocol::{HttpConfig, HttpServer, MetricsRegistry};
use tracing_subscriber::EnvFilter;

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

    /// ADV-F-7：WAL 文件路径（启用 log-then-commit 事务模型）。
    ///
    /// 设置后，服务器在启动时创建共享的 `WalWriter`，所有 session 的 COMMIT 操作
    /// 会先写入 WAL Commit 记录并 fsync，然后才向客户端返回成功。
    /// 这消除了"ACK 成功但数据未持久化"的风险。
    ///
    /// 未设置时（默认），退化为 commit-then-log 行为，仅用于测试/兼容。
    /// **生产环境强烈建议设置此参数**。
    #[arg(long)]
    wal_path: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化 tracing 日志
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let args = Args::parse();

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
        crash_log_dir = ?args.crash_log_dir,
        daemon = args.daemon,
        pid_file = ?args.pid_file,
        http_port = args.http_port,
        http_host = %args.http_host,
        wal_path = ?args.wal_path,
        "starting SzRSQL pgwire server"
    );

    let config = PgwireConfig::new()
        .with_host(args.host)
        .with_port(args.port)
        .with_server_version(args.server_version)
        .with_shutdown_timeout(std::time::Duration::from_secs(args.shutdown_timeout));

    // ADV-F-7：创建共享 WalWriter（如果指定了 --wal-path）
    // 启用 log-then-commit 事务模型：COMMIT 先写 WAL 并 fsync，再 ACK 客户端
    let wal_writer: Option<Arc<szrsql_tx::wal::WalWriter>> = if let Some(wal_path) = &args.wal_path {
        // 确保父目录存在
        if let Some(parent) = wal_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        match szrsql_tx::wal::WalWriter::create_new(wal_path) {
            Ok(writer) => {
                tracing::info!(
                    wal_path = %wal_path.display(),
                    "WAL writer created, log-then-commit transaction model enabled"
                );
                Some(Arc::new(writer))
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    wal_path = %wal_path.display(),
                    "failed to create WAL writer, falling back to commit-then-log"
                );
                return Err(e.into());
            }
        }
    } else {
        tracing::warn!(
            "WAL path not specified (--wal-path), running in commit-then-log mode. \
             This is NOT recommended for production. Set --wal-path to enable log-then-commit."
        );
        None
    };

    // ADV-CONC-1：创建共享表存储、锁管理器和事务 ID 计数器（跨 session 全局唯一）
    let shared_tables = Arc::new(RwLock::new(HashMap::new()));
    let lock_manager = Arc::new(LockManager::new());
    let shared_txn_counter = Arc::new(AtomicU32::new(1));

    let mut server_builder = PgwireServer::new(config)
        .with_concurrency(shared_tables, lock_manager)
        .with_shared_txn_counter(shared_txn_counter);
    if let Some(writer) = wal_writer {
        server_builder = server_builder.with_wal_writer(writer);
    }
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
        let metrics = Arc::new(MetricsRegistry::new());
        // 订阅 pgwire 服务器的关闭状态
        let shutdown_rx = server.shutdown_coordinator().subscribe();
        let http_server = HttpServer::new(http_config, metrics, shutdown_rx);
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

    // Phase 4.12：安装信号处理器，返回 ShutdownSignal（Graceful=SIGTERM / Immediate=SIGINT）
    let shutdown_signal = setup_signal_handler();

    // 根据信号类型执行关闭策略：
    // - Graceful（SIGTERM）：等待活跃连接排空（最多 shutdown_timeout）
    // - Immediate（SIGINT/Ctrl+C）：立即 abort_all，不等待
    if let Err(e) = server.serve_with_shutdown(shutdown_signal).await {
        tracing::error!(error = %e, "pgwire server exited with error");
        // 即使 pgwire 失败，也要等待 HTTP 服务器退出
        if let Some(handle) = http_handle {
            let _ = handle.await;
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

    // _pid_file 在此处 drop，自动删除 PID 文件
    Ok(())
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
