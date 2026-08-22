//! sz-orm-websocket 服务端接入
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think-worker` 的 WebSocket 服务端模型：
//!
//! ```php
//! $worker = new Workerman\Worker("websocket://0.0.0.0:2346");
//! $worker->onConnect = function($connection) { /* ... */ };
//! $worker->onMessage = function($connection, $data) { /* ... */ };
//! $worker->onClose = function($connection) { /* ... */ };
//! $worker->count = 4;  // worker 进程数
//! Worker::runAll();
//! ```
//!
//! Rust 端复用 `sz_orm_websocket::WsServer`（基于 tokio-tungstenite）。
//!
//! ## 设计
//!
//! - `WebSocketRuntime`：封装 WsServer，提供 start/stop lifecycle
//! - 默认使用 `DefaultWebSocketHandler`（echo 模式）
//! - 监听 `CancellationToken` 优雅停止

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::orm::{DefaultWebSocketHandler, WebSocketHandler, WsError, WsServer};

/// WebSocket 运行时配置
#[derive(Debug, Clone)]
pub struct WebSocketRuntimeConfig {
    /// 监听地址（如 "0.0.0.0:2346"）
    pub listen_addr: String,
}

impl Default for WebSocketRuntimeConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:2346".to_string(),
        }
    }
}

impl WebSocketRuntimeConfig {
    /// 创建新配置
    pub fn new(listen_addr: impl Into<String>) -> Self {
        Self {
            listen_addr: listen_addr.into(),
        }
    }
}

/// WebSocket 运行时
///
/// 封装 `sz_orm_websocket::WsServer`，提供服务端 lifecycle 管理。
///
/// ## 设计
///
/// - 默认使用 `DefaultWebSocketHandler`（echo 模式），可替换为自定义 handler
/// - `start` 方法 spawn 后台任务，返回 `JoinHandle<()>`
/// - `stop` 方法调用 `WsServer::stop()`，触发 oneshot shutdown
/// - 监听 `CancellationToken`：收到信号后自动调用 `WsServer::stop()`
///
/// ## 用法
///
/// ```rust,ignore
/// use sz_rust_core::runtime::websocket::{WebSocketRuntime, WebSocketRuntimeConfig};
/// use tokio_util::sync::CancellationToken;
///
/// let runtime = WebSocketRuntime::new(WebSocketRuntimeConfig::new("0.0.0.0:2346"));
/// let token = CancellationToken::new();
/// let handle = runtime.start(token.clone());
/// // ... 业务运行 ...
/// token.cancel();
/// let _ = handle.await;
/// ```
pub struct WebSocketRuntime {
    config: WebSocketRuntimeConfig,
    server: Arc<WsServer>,
}

impl WebSocketRuntime {
    /// 创建 WebSocket 运行时，使用默认 handler
    pub fn new(config: WebSocketRuntimeConfig) -> Self {
        let server = Arc::new(WsServer::new(&config.listen_addr));
        Self { config, server }
    }

    /// 启动 WebSocket 服务（返回 JoinHandle，调用方持有）
    ///
    /// - 使用 `DefaultWebSocketHandler` 处理连接
    /// - 监听 `token.cancelled()`，收到信号后调用 `server.stop()`
    pub fn start(&self, token: CancellationToken) -> tokio::task::JoinHandle<Result<(), WsError>> {
        let server = self.server.clone();
        let handler: Arc<dyn WebSocketHandler> = Arc::new(DefaultWebSocketHandler::new());

        tokio::spawn(async move {
            // 启动 server（在子任务中运行，避免阻塞）
            let server_clone = server.clone();
            let mut start_task = tokio::spawn(async move { server_clone.start(handler).await });

            // 监听 cancel，select! 通过 &mut 引用避免 move
            tokio::select! {
                _ = token.cancelled() => {
                    let _ = server.stop().await;
                    // start_task 尚未被 move，可以 await
                    let _ = (&mut start_task).await;
                    Ok(())
                }
                result = &mut start_task => {
                    match result {
                        Ok(inner) => inner,
                        Err(e) => Err(WsError::Connection(format!("start task panicked: {}", e))),
                    }
                }
            }
        })
    }

    /// 使用自定义 handler 启动 WebSocket 服务
    pub fn start_with_handler(
        &self,
        handler: Arc<dyn WebSocketHandler>,
        token: CancellationToken,
    ) -> tokio::task::JoinHandle<Result<(), WsError>> {
        let server = self.server.clone();

        tokio::spawn(async move {
            let server_clone = server.clone();
            let mut start_task = tokio::spawn(async move { server_clone.start(handler).await });

            tokio::select! {
                _ = token.cancelled() => {
                    let _ = server.stop().await;
                    let _ = (&mut start_task).await;
                    Ok(())
                }
                result = &mut start_task => {
                    match result {
                        Ok(inner) => inner,
                        Err(e) => Err(WsError::Connection(format!("start task panicked: {}", e))),
                    }
                }
            }
        })
    }

    /// 手动停止服务
    pub async fn stop(&self) -> Result<(), WsError> {
        self.server.stop().await
    }

    /// 获取当前连接数
    pub async fn connection_count(&self) -> usize {
        self.server.connection_count().await
    }

    /// 广播消息到所有连接
    pub async fn broadcast_to_all(&self, data: Vec<u8>) -> Result<usize, WsError> {
        self.server.broadcast_to_all(data).await
    }

    /// 是否仍在运行
    pub async fn is_running(&self) -> bool {
        self.server.is_running().await
    }

    /// 获取配置
    pub fn config(&self) -> &WebSocketRuntimeConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_websocket_runtime_config_default() {
        let config = WebSocketRuntimeConfig::default();
        assert_eq!(config.listen_addr, "0.0.0.0:2346");
    }

    #[test]
    fn test_websocket_runtime_config_custom() {
        let config = WebSocketRuntimeConfig::new("127.0.0.1:8080");
        assert_eq!(config.listen_addr, "127.0.0.1:8080");
    }

    #[tokio::test]
    async fn test_websocket_runtime_creation() {
        let runtime = WebSocketRuntime::new(WebSocketRuntimeConfig::new("127.0.0.1:0"));
        assert_eq!(runtime.config().listen_addr, "127.0.0.1:0");
        assert!(!runtime.is_running().await);
        assert_eq!(runtime.connection_count().await, 0);
    }

    #[tokio::test]
    async fn test_websocket_start_and_cancel() {
        // 使用 port 0 让 OS 分配可用端口
        let runtime = WebSocketRuntime::new(WebSocketRuntimeConfig::new("127.0.0.1:0"));
        let token = CancellationToken::new();
        let handle = runtime.start(token.clone());

        // 给 server 一点时间启动
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 触发关闭
        token.cancel();

        // 等待任务退出
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "websocket task should stop on cancel");
    }

    #[tokio::test]
    async fn test_websocket_start_with_handler_and_cancel() {
        let runtime = WebSocketRuntime::new(WebSocketRuntimeConfig::new("127.0.0.1:0"));
        let handler: Arc<dyn WebSocketHandler> = Arc::new(DefaultWebSocketHandler::new());
        let token = CancellationToken::new();
        let handle = runtime.start_with_handler(handler, token.clone());

        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();

        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "websocket task should stop on cancel");
    }

    #[tokio::test]
    async fn test_websocket_broadcast_no_connections() {
        let runtime = WebSocketRuntime::new(WebSocketRuntimeConfig::new("127.0.0.1:0"));
        // 没有连接时广播应返回 Ok(0)
        let result = runtime.broadcast_to_all(b"hello".to_vec()).await;
        match result {
            Ok(n) => assert_eq!(n, 0, "无连接时广播应送达 0 个接收者"),
            Err(e) => panic!("无连接广播不应失败，实际: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_websocket_manual_stop() {
        let runtime = WebSocketRuntime::new(WebSocketRuntimeConfig::new("127.0.0.1:0"));
        let token = CancellationToken::new();
        let handle = runtime.start(token.clone());

        tokio::time::sleep(Duration::from_millis(50)).await;

        // 手动停止
        let stop_result = runtime.stop().await;
        assert!(
            stop_result.is_ok(),
            "stop() 应成功，实际: {:?}",
            stop_result
        );

        // 等待任务退出
        let exit = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(exit.is_ok(), "stop 后 websocket 任务应在 2s 内退出");
    }

    #[test]
    fn test_config_accessor() {
        let runtime = WebSocketRuntime::new(WebSocketRuntimeConfig::new("0.0.0.0:9999"));
        assert_eq!(runtime.config().listen_addr, "0.0.0.0:9999");
    }

    #[tokio::test]
    async fn test_multiple_websocket_runtimes() {
        // 验证可以创建多个 runtime 实例（不启动）
        let rt1 = WebSocketRuntime::new(WebSocketRuntimeConfig::new("127.0.0.1:0"));
        let rt2 = WebSocketRuntime::new(WebSocketRuntimeConfig::new("127.0.0.1:0"));

        assert_eq!(rt1.connection_count().await, 0);
        assert_eq!(rt2.connection_count().await, 0);
    }
}
