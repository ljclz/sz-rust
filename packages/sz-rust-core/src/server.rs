// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! HTTP 服务器模块 — axum::serve 启动器
//!
//! 对齐 PHP `think\swoole` / `think-worker` 启动入口，封装 axum::serve。
//!
//! ## 功能
//!
//! - `serve()`：基础启动器（不含 graceful shutdown）
//! - `serve_with_graceful_shutdown()`：带优雅关闭（监听 Ctrl+C）
//! - `serve_with_listener()`：使用自定义 tokio::net::TcpListener（测试友好）
//! - `build_tcp_listener()`：构造 TCP listener
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_core::server::serve;
//! use axum::Router;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let router = Router::new();
//! serve(router, "127.0.0.1:8801").await?;
//! # Ok(())
//! # }
//! ```

use std::convert::Infallible;
use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;
use tower::Service;

/// 启动 HTTP 服务器
///
/// 阻塞当前异步任务，直到服务器关闭。
///
/// ## 参数
///
/// - `router`：axum::Router（实现了 Service）
/// - `addr`：监听地址，例如 `"127.0.0.1:8801"` 或 `"0.0.0.0:80"`
///
/// ## 错误
///
/// 绑定端口失败时返回 `std::io::Error`。
pub async fn serve(router: Router, addr: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}

/// 启动 HTTP 服务器（带优雅关闭）
///
/// 监听 Ctrl+C 信号，收到后启动 graceful shutdown。
///
/// ## 参数
///
/// - `router`：axum::Router
/// - `addr`：监听地址
pub async fn serve_with_graceful_shutdown(router: Router, addr: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// 启动 HTTP 服务器（使用已有 TcpListener，测试友好）
///
/// 适用于测试场景：测试代码可以 `listener.local_addr()` 获取实际端口，
/// 然后在另一个 task 中连接。也适用于 Unix socket 等自定义 listener。
///
/// ## 参数
///
/// - `router`：axum::Router
/// - `listener`：已绑定的 tokio::net::TcpListener
pub async fn serve_with_listener(router: Router, listener: TcpListener) -> std::io::Result<()> {
    axum::serve(listener, router).await?;
    Ok(())
}

/// 构造 TCP listener
///
/// 内部使用 `tokio::net::TcpListener::bind`，返回 listener 和实际绑定的地址。
/// 适用于测试场景：传入 `"127.0.0.1:0"` 让 OS 分配端口。
///
/// ## 返回
///
/// `(TcpListener, SocketAddr)`，addr 是实际绑定的地址（端口可能为 0 表示由 OS 分配）。
pub async fn build_tcp_listener(addr: &str) -> std::io::Result<(TcpListener, SocketAddr)> {
    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    Ok((listener, local_addr))
}

/// 优雅关闭信号监听
///
/// 监听 Ctrl+C / SIGTERM，返回后触发 axum 的 graceful shutdown。
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

// 用于编译期验证 Router 满足 Service trait 约束
#[allow(dead_code)]
fn _assert_router_is_service()
where
    Router: Service<
        http::Request<axum::body::Body>,
        Response = axum::response::Response,
        Error = Infallible,
    >,
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_build_tcp_listener_with_random_port() {
        let (listener, addr) = build_tcp_listener("127.0.0.1:0").await.unwrap();
        assert!(addr.port() > 0);
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        // listener 必须可用
        let _ = listener.local_addr().unwrap();
    }

    #[tokio::test]
    async fn test_build_tcp_listener_bind_error_for_invalid_addr() {
        // 端口号超出范围
        let result = build_tcp_listener("127.0.0.1:99999").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_serve_with_listener_responds_to_request() {
        let router = Router::new().route("/", axum::routing::get(|| async { "hello from server" }));
        let (listener, addr) = build_tcp_listener("127.0.0.1:0").await.unwrap();

        tokio::spawn(async move {
            let _ = serve_with_listener(router, listener).await;
        });

        // 等服务器就绪
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let body = http_get_body(addr.to_string().as_str(), "/").await;
        assert!(body.contains("hello from server"));
    }

    #[tokio::test]
    async fn test_router_responds_via_oneshot() {
        // 验证 Router 不需要真实 TCP 也能直接 oneshot 测试
        let router = Router::new().route("/ping", axum::routing::get(|| async { "pong" }));
        let request = Request::builder()
            .method(Method::GET)
            .uri("/ping")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"pong");
    }

    /// 最小 HTTP/1.1 GET 客户端，避免引入 reqwest 依赖
    ///
    /// 返回响应体（HTTP body 部分）字符串。
    async fn http_get_body(host: &str, path: &str) -> String {
        let mut stream = TcpStream::connect(host).await.unwrap();
        let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf).to_string();

        // 分离 header / body
        if let Some(idx) = response.find("\r\n\r\n") {
            response[idx + 4..].to_string()
        } else {
            response
        }
    }

    // ---- 补充测试：覆盖 serve() 和 serve_with_graceful_shutdown() ----

    #[tokio::test]
    async fn test_serve_responds_to_request() {
        // 获取空闲端口（bind 后 drop 释放端口，serve 内部重新 bind）
        let addr = {
            let (_, addr) = build_tcp_listener("127.0.0.1:0").await.unwrap();
            addr
        };
        let addr_str = addr.to_string();

        let router = Router::new().route("/", axum::routing::get(|| async { "serve ok" }));
        tokio::spawn(async move {
            let _ = serve(router, &addr_str).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let host = addr.to_string();
        let body = http_get_body(&host, "/").await;
        assert!(body.contains("serve ok"));
    }

    #[tokio::test]
    async fn test_serve_with_graceful_shutdown_responds_to_request() {
        let addr = {
            let (_, addr) = build_tcp_listener("127.0.0.1:0").await.unwrap();
            addr
        };
        let addr_str = addr.to_string();

        let router = Router::new().route("/", axum::routing::get(|| async { "graceful ok" }));
        tokio::spawn(async move {
            let _ = serve_with_graceful_shutdown(router, &addr_str).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let host = addr.to_string();
        let body = http_get_body(&host, "/").await;
        assert!(body.contains("graceful ok"));
    }

    #[tokio::test]
    async fn test_build_tcp_listener_wildcard_addr() {
        let (listener, addr) = build_tcp_listener("0.0.0.0:0").await.unwrap();
        assert!(addr.port() > 0);
        let _ = listener.local_addr().unwrap();
    }

    #[tokio::test]
    async fn test_build_tcp_listener_invalid_ip() {
        let result = build_tcp_listener("invalid_addr:8080").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_build_tcp_listener_empty_addr() {
        let result = build_tcp_listener("").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_serve_with_listener_multiple_routes() {
        let router = Router::new()
            .route("/", axum::routing::get(|| async { "home" }))
            .route("/api", axum::routing::get(|| async { "api" }));
        let (listener, addr) = build_tcp_listener("127.0.0.1:0").await.unwrap();

        tokio::spawn(async move {
            let _ = serve_with_listener(router, listener).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let host = addr.to_string();
        let body1 = http_get_body(&host, "/").await;
        assert!(body1.contains("home"));

        let body2 = http_get_body(&host, "/api").await;
        assert!(body2.contains("api"));
    }
}
