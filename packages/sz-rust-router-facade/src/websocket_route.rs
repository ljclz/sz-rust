//! WebSocket 原生路由 — 基于 axum WebSocketUpgrade
//!
//! 将 WebSocket 处理集成到主 HTTP 路由中，无需独立端口。
//!
//! ## PHP 对齐
//!
//! 对齐 PHP `think-worker` WebSocket + Workerman 的 `websocket://` 协议：
//!
//! ```php
//! $worker = new Worker("websocket://0.0.0.0:2346");
//! $worker->onMessage = function($connection, $data) {
//!     $connection->send("echo: $data");
//! };
//! ```
//!
//! Rust 端通过 axum 的 `WebSocketUpgrade` 提取器，在主 HTTP 端口上
//! 处理 WebSocket 升级请求（如 `GET /ws/chat`），无需独立端口。
//!
//! ## 设计
//!
//! - [`WsHandler`] trait：简化 WebSocket 事件处理（on_connect/on_message/on_close）
//! - [`ws_handler()`]：将 `WsHandler` 转为 axum handler
//! - [`EchoWsHandler`]：默认回显处理器
//! - [`crate::router::RouterBuilder::ws`]：注册 WebSocket 路由
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_router_facade::router::RouterBuilder;
//! use sz_rust_router_facade::websocket_route::{ws_handler, EchoWsHandler};
//!
//! let router = RouterBuilder::new()
//!     .ws("/ws/echo", ws_handler(EchoWsHandler::new()))
//!     .build();
//! ```

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use std::sync::Arc;

// ============================================================================
// WsHandler trait — 简化 WebSocket 事件处理
// ============================================================================

/// WebSocket 事件处理器 trait
///
/// 对齐 PHP Workerman 的 `onConnect` / `onMessage` / `onClose` 事件模型。
/// 用户实现此 trait，通过 [`ws_handler()`] 转为 axum handler。
pub trait WsHandler: Send + Sync + 'static {
    /// 连接建立时调用
    ///
    /// 默认空实现。可用于注册连接、发送欢迎消息等。
    fn on_connect(&self) {}

    /// 收到文本/二进制消息时调用
    ///
    /// 返回 `Some(Message)` 则自动回发给客户端，返回 `None` 则不回发。
    /// 默认返回 `None`。
    fn on_message(&self, _msg: Message) -> Option<Message> {
        None
    }

    /// 连接关闭时调用
    ///
    /// 默认空实现。可用于清理资源、广播离线通知等。
    fn on_close(&self) {}
}

/// 将 [`WsHandler`] 转为 axum `MethodRouter`，可直接注册到路由
///
/// 返回 `MethodRouter` 而非裸 handler，避免 `impl Fn` 在 axum `Handler` trait
/// 推导上的已知限制（`impl Trait` 返回类型无法被泛型 bound 正确解析）。
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_router_facade::websocket_route::{ws_handler, EchoWsHandler};
///
/// let router = axum::Router::new()
///     .route("/ws/echo", ws_handler(EchoWsHandler::new()));
/// ```
pub fn ws_handler<H: WsHandler>(handler: H) -> axum::routing::MethodRouter<()> {
    let handler = Arc::new(handler);
    axum::routing::get(move |ws: WebSocketUpgrade| async move {
        let handler = handler.clone();
        ws.on_upgrade(move |socket| handle_ws_connection(socket, handler))
    })
}

/// 处理 WebSocket 连接生命周期
///
/// 依次调用 `on_connect` → 循环 `on_message` → `on_close`。
async fn handle_ws_connection(mut socket: WebSocket, handler: Arc<dyn WsHandler>) {
    handler.on_connect();

    // 循环接收消息，调用 on_message，有回复则发送
    while let Some(Ok(msg)) = socket.recv().await {
        // 处理 Close 消息：退出循环
        if matches!(msg, Message::Close(_)) {
            break;
        }

        if let Some(reply) = handler.on_message(msg) {
            // 发送回复，失败则退出
            if socket.send(reply).await.is_err() {
                break;
            }
        }
    }

    handler.on_close();
}

// ============================================================================
// EchoWsHandler — 默认回显处理器
// ============================================================================

/// 回显 WebSocket 处理器
///
/// 将收到的消息原样回发给客户端。适用于心跳检测、调试等场景。
#[derive(Debug, Default, Clone)]
pub struct EchoWsHandler;

impl EchoWsHandler {
    /// 创建回显处理器
    pub fn new() -> Self {
        Self
    }
}

impl WsHandler for EchoWsHandler {
    fn on_message(&self, msg: Message) -> Option<Message> {
        Some(msg)
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    /// 测试 EchoWsHandler 基本行为
    #[test]
    fn test_echo_handler_returns_message() {
        let handler = EchoWsHandler::new();
        let msg = Message::text("hello");
        let result = handler.on_message(msg);
        assert!(result.is_some());
    }

    /// 测试 EchoWsHandler 默认构造
    #[test]
    fn test_echo_handler_default() {
        let handler = EchoWsHandler;
        let msg = Message::text("test");
        assert!(handler.on_message(msg).is_some());
    }

    /// 测试自定义 WsHandler
    struct NoReplyHandler;
    impl WsHandler for NoReplyHandler {
        fn on_message(&self, _msg: Message) -> Option<Message> {
            None
        }
    }

    #[test]
    fn test_custom_handler_no_reply() {
        let handler = NoReplyHandler;
        let msg = Message::text("hello");
        assert!(handler.on_message(msg).is_none());
    }

    /// 测试自定义 WsHandler 有回复
    struct PrefixHandler;
    impl WsHandler for PrefixHandler {
        fn on_message(&self, _msg: Message) -> Option<Message> {
            Some(Message::text("prefix: reply"))
        }
    }

    #[test]
    fn test_custom_handler_with_reply() {
        let handler = PrefixHandler;
        let msg = Message::text("input");
        let reply = handler.on_message(msg).unwrap();
        assert_eq!(reply.to_text().unwrap(), "prefix: reply");
    }

    /// 测试 on_connect 和 on_close 默认实现不 panic
    #[test]
    fn test_default_lifecycle_hooks_no_panic() {
        let handler = EchoWsHandler::new();
        handler.on_connect();
        handler.on_close();
    }

    /// 测试 WebSocket 路由注册（HTTP 层面验证 400 Bad Request）
    ///
    /// 非 WebSocket 请求（无 Upgrade 头）访问 WebSocket 路由时，
    /// axum 0.8 返回 400 Bad Request（要求 `Upgrade: websocket` 头）。
    #[tokio::test]
    async fn test_ws_route_registered_as_get() {
        let router = axum::Router::new().route("/ws/echo", ws_handler(EchoWsHandler::new()));

        // 普通 GET 请求（无 Upgrade 头）应返回 400
        let request = Request::builder()
            .method(Method::GET)
            .uri("/ws/echo")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        // axum 0.8 对非 WebSocket 请求返回 400 Bad Request
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    /// 测试 WebSocket 路由 404
    #[tokio::test]
    async fn test_ws_route_not_found() {
        let router = axum::Router::new().route("/ws/echo", ws_handler(EchoWsHandler::new()));

        let request = Request::builder()
            .method(Method::GET)
            .uri("/ws/nonexistent")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// 测试 POST 方法访问 WebSocket 路由返回 405
    #[tokio::test]
    async fn test_ws_route_rejects_post() {
        let router = axum::Router::new().route("/ws/echo", ws_handler(EchoWsHandler::new()));

        let request = Request::builder()
            .method(Method::POST)
            .uri("/ws/echo")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
