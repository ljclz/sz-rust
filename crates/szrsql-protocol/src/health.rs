//! Phase 7d.18 — Docker HEALTHCHECK 健康检查器。
//!
//! 提供 TCP 连接探针，用于 Docker HEALTHCHECK 或 Kubernetes liveness/readiness probe。
//!
//! # 设计
//!
//! - **零外部依赖**：仅依赖 tokio TCP + std，与 http.rs 风格一致
//! - **超时控制**：默认 3s 超时，避免 HEALTHCHECK 阻塞
//! - **双探针模式**：
//!   - `check_tcp()` — TCP 连接到 pgwire 端口（5432），最轻量
//!   - `check_http_healthz()` — HTTP GET /healthz，验证 HTTP 管理端点可用性
//! - **退出码语义**：Healthy → exit 0，Unhealthy → exit 1
//!
//! # 用法
//!
//! ```bash
//! # TCP 探针（推荐用于 Dockerfile HEALTHCHECK）
//! szrsql-health --host 127.0.0.1 --port 5432
//!
//! # HTTP 探针（需启用 --http-port）
//! szrsql-health --http --host 127.0.0.1 --port 8080
//! ```

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

// =====================================================================
//  HealthStatus
// =====================================================================

/// 健康检查结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthStatus {
    /// 健康：服务可用。
    Healthy,
    /// 不健康：服务不可用，附带原因。
    Unhealthy { reason: String },
}

impl HealthStatus {
    /// 是否健康。
    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }

    /// 转为进程退出码（Healthy → 0，Unhealthy → 1）。
    pub fn exit_code(&self) -> i32 {
        match self {
            HealthStatus::Healthy => 0,
            HealthStatus::Unhealthy { .. } => 1,
        }
    }

    /// 获取原因描述（Healthy 返回 "healthy"）。
    pub fn reason(&self) -> &str {
        match self {
            HealthStatus::Healthy => "healthy",
            HealthStatus::Unhealthy { reason } => reason,
        }
    }
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Unhealthy { reason } => write!(f, "unhealthy: {reason}"),
        }
    }
}

// =====================================================================
//  HealthChecker
// =====================================================================

/// 健康检查器。
///
/// 通过 TCP 连接或 HTTP GET 验证 SzRSQL 服务可用性。
pub struct HealthChecker {
    /// 目标主机（默认 127.0.0.1）。
    pub host: String,
    /// 目标端口（默认 5432）。
    pub port: u16,
    /// 连接超时（默认 3s）。
    pub timeout: Duration,
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 5432,
            timeout: Duration::from_secs(3),
        }
    }
}

impl HealthChecker {
    /// 创建默认健康检查器（127.0.0.1:5432，3s 超时）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置目标主机。
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    /// 设置目标端口。
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// 设置连接超时。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// TCP 连接探针。
    ///
    /// 尝试建立 TCP 连接到 `host:port`，成功则返回 `Healthy`。
    /// 这是最轻量的探针，仅验证端口监听，不验证协议握手。
    pub async fn check_tcp(&self) -> HealthStatus {
        let addr = format!("{}:{}", self.host, self.port);
        match timeout(self.timeout, TcpStream::connect(&addr)).await {
            Ok(Ok(_stream)) => HealthStatus::Healthy,
            Ok(Err(e)) => HealthStatus::Unhealthy {
                reason: format!("tcp connect to {addr} failed: {e}"),
            },
            Err(_) => HealthStatus::Unhealthy {
                reason: format!("tcp connect to {addr} timed out after {:?}", self.timeout),
            },
        }
    }

    /// HTTP /healthz 探针。
    ///
    /// 发送 `GET /healthz HTTP/1.1` 请求，验证返回 200 + `{"status":"ok"}`。
    /// 比 TCP 探针更严格：验证 HTTP 管理服务器可响应请求。
    pub async fn check_http_healthz(&self) -> HealthStatus {
        let addr = format!("{}:{}", self.host, self.port);
        let connect_future = TcpStream::connect(&addr);

        let mut stream = match timeout(self.timeout, connect_future).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                return HealthStatus::Unhealthy {
                    reason: format!("tcp connect to {addr} failed: {e}"),
                };
            }
            Err(_) => {
                return HealthStatus::Unhealthy {
                    reason: format!("tcp connect to {addr} timed out after {:?}", self.timeout),
                };
            }
        };

        // 发送 HTTP 请求
        let request = format!(
            "GET /healthz HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.host
        );
        if let Err(e) = timeout(self.timeout, stream.write_all(request.as_bytes())).await {
            return HealthStatus::Unhealthy {
                reason: format!("http write failed: {e}"),
            };
        }
        // 显式 flush（TcpStream::flush 是 no-op，但保持语义清晰）
        if let Err(e) = timeout(self.timeout, stream.flush()).await {
            return HealthStatus::Unhealthy {
                reason: format!("http flush failed: {e}"),
            };
        }

        // 读取响应
        let mut buf = Vec::with_capacity(256);
        if let Err(e) = timeout(self.timeout, stream.read_to_end(&mut buf)).await {
            return HealthStatus::Unhealthy {
                reason: format!("http read failed: {e}"),
            };
        }

        let response = String::from_utf8_lossy(&buf);
        if response.contains("200 OK") && response.contains(r#""status":"ok""#) {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy {
                reason: format!("http /healthz returned unexpected response: {response}"),
            }
        }
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU16, Ordering};

    /// 辅助：查找可用端口（线程安全递增）。
    static NEXT_PORT: AtomicU16 = AtomicU16::new(19000);

    fn find_free_port() -> u16 {
        loop {
            let port = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return port;
            }
        }
    }

    /// 辅助：启动一个临时 TCP 服务器（模拟 HTTP /healthz 响应）。
    async fn start_echo_server(port: u16) -> tokio::task::JoinHandle<()> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("bind failed");
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                // 先读取客户端请求（避免 RST 竞态）
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                // 返回 HTTP 200 + {"status":"ok"} 响应
                let resp = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}";
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.flush().await;
            }
        })
    }

    // ==================== HealthStatus ====================

    #[test]
    fn test_health_status_is_healthy() {
        assert!(HealthStatus::Healthy.is_healthy());
        assert!(!HealthStatus::Unhealthy {
            reason: "test".to_string()
        }
        .is_healthy());
    }

    #[test]
    fn test_health_status_exit_code() {
        assert_eq!(HealthStatus::Healthy.exit_code(), 0);
        assert_eq!(
            HealthStatus::Unhealthy {
                reason: "fail".to_string()
            }
            .exit_code(),
            1
        );
    }

    #[test]
    fn test_health_status_reason() {
        assert_eq!(HealthStatus::Healthy.reason(), "healthy");
        let unhealthy = HealthStatus::Unhealthy {
            reason: "connection refused".to_string(),
        };
        assert_eq!(unhealthy.reason(), "connection refused");
    }

    #[test]
    fn test_health_status_display() {
        assert_eq!(format!("{}", HealthStatus::Healthy), "healthy");
        let unhealthy = HealthStatus::Unhealthy {
            reason: "timeout".to_string(),
        };
        assert_eq!(format!("{unhealthy}"), "unhealthy: timeout");
    }

    // ==================== HealthChecker 配置 ====================

    #[test]
    fn test_health_checker_default() {
        let checker = HealthChecker::new();
        assert_eq!(checker.host, "127.0.0.1");
        assert_eq!(checker.port, 5432);
        assert_eq!(checker.timeout, Duration::from_secs(3));
    }

    #[test]
    fn test_health_checker_builder() {
        let checker = HealthChecker::new()
            .with_host("0.0.0.0")
            .with_port(8080)
            .with_timeout(Duration::from_secs(5));
        assert_eq!(checker.host, "0.0.0.0");
        assert_eq!(checker.port, 8080);
        assert_eq!(checker.timeout, Duration::from_secs(5));
    }

    // ==================== TCP 探针 ====================

    #[tokio::test]
    async fn test_check_tcp_healthy() {
        let port = find_free_port();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("bind failed");
        let handle = tokio::spawn(async move {
            // 仅 accept 一次后退出
            let _ = listener.accept().await;
        });

        let checker = HealthChecker::new()
            .with_host("127.0.0.1")
            .with_port(port)
            .with_timeout(Duration::from_secs(1));
        let status = checker.check_tcp().await;
        assert!(status.is_healthy(), "should be healthy: {status}");

        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_check_tcp_connection_refused() {
        // 使用一个几乎不可能被占用的端口
        let port = find_free_port();
        // 不启动服务器，直接连接应失败
        let checker = HealthChecker::new()
            .with_host("127.0.0.1")
            .with_port(port)
            .with_timeout(Duration::from_secs(1));
        let status = checker.check_tcp().await;
        assert!(!status.is_healthy());
        assert!(status.reason().contains("tcp connect"));
    }

    #[tokio::test]
    async fn test_check_tcp_timeout() {
        // 连接到一个不可达地址触发超时
        // 10.255.255.1 通常是不可达的（除非配置了特殊路由）
        let checker = HealthChecker::new()
            .with_host("10.255.255.1")
            .with_port(1)
            .with_timeout(Duration::from_millis(100));
        let status = checker.check_tcp().await;
        // 可能是超时或连接失败，都不健康
        assert!(!status.is_healthy());
    }

    // ==================== HTTP /healthz 探针 ====================

    #[tokio::test]
    async fn test_check_http_healthz_healthy() {
        let port = find_free_port();
        let handle = start_echo_server(port).await;

        let checker = HealthChecker::new()
            .with_host("127.0.0.1")
            .with_port(port)
            .with_timeout(Duration::from_secs(1));
        let status = checker.check_http_healthz().await;
        assert!(status.is_healthy(), "should be healthy: {status}");

        handle.abort();
    }

    #[tokio::test]
    async fn test_check_http_healthz_connection_refused() {
        let port = find_free_port();
        let checker = HealthChecker::new()
            .with_host("127.0.0.1")
            .with_port(port)
            .with_timeout(Duration::from_secs(1));
        let status = checker.check_http_healthz().await;
        assert!(!status.is_healthy());
        assert!(status.reason().contains("tcp connect"));
    }

    #[tokio::test]
    async fn test_check_http_healthz_bad_response() {
        let port = find_free_port();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("bind failed");
        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            while let Ok((mut stream, _)) = listener.accept().await {
                // 先读取客户端请求（避免 RST 竞态）
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                // 返回 503 错误响应
                let resp = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.flush().await;
            }
        });

        let checker = HealthChecker::new()
            .with_host("127.0.0.1")
            .with_port(port)
            .with_timeout(Duration::from_secs(1));
        let status = checker.check_http_healthz().await;
        assert!(!status.is_healthy());
        assert!(status.reason().contains("unexpected response"));

        handle.abort();
    }
}
