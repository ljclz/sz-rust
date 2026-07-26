//! Phase 7d.18 — SzRSQL 健康检查二进制。
//!
//! 用于 Docker HEALTHCHECK 或 Kubernetes liveness/readiness probe。
//!
//! # 用法
//!
//! ```bash
//! # TCP 探针（默认，检查 pgwire 端口 5432）
//! szrsql-health
//! szrsql-health --host 127.0.0.1 --port 5432
//!
//! # HTTP 探针（检查 HTTP 管理端点 /healthz）
//! szrsql-health --http --port 8080
//!
//! # 自定义超时
//! szrsql-health --timeout 5
//! ```
//!
//! # 退出码
//!
//! - 0：健康（服务可用）
//! - 1：不健康（服务不可用或超时）
//! - 2：参数错误
//!
//! # Dockerfile 集成
//!
//! ```dockerfile
//! HEALTHCHECK --interval=30s --timeout=5s --start-period=3s --retries=3 \
//!     CMD szrsql-health --host 127.0.0.1 --port 5432 || exit 1
//! ```

use std::process::ExitCode;

use clap::Parser;
use szrsql_protocol::HealthChecker;

/// SzRSQL 健康检查命令行参数。
#[derive(Parser, Debug)]
#[command(name = "szrsql-health", version, about = "SzRSQL 健康检查工具")]
struct HealthArgs {
    /// 目标主机（默认 127.0.0.1）。
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// 目标端口（默认 5432，pgwire 端口）。
    #[arg(long, default_value_t = 5432)]
    port: u16,

    /// 连接超时（秒，默认 3）。
    #[arg(long, default_value_t = 3)]
    timeout: u64,

    /// 使用 HTTP /healthz 探针（默认使用 TCP 探针）。
    ///
    /// 启用后发送 GET /healthz 请求，验证 HTTP 管理服务器可响应。
    /// 需要服务器以 --http-port 启动。
    #[arg(long, default_value_t = false)]
    http: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = HealthArgs::parse();

    let checker = HealthChecker::new()
        .with_host(&args.host)
        .with_port(args.port)
        .with_timeout(std::time::Duration::from_secs(args.timeout));

    let status = if args.http {
        checker.check_http_healthz().await
    } else {
        checker.check_tcp().await
    };

    // 输出结果到 stderr（不干扰 stdout 用于脚本管道）
    eprintln!("[szrsql-health] {status}");

    ExitCode::from(status.exit_code() as u8)
}
