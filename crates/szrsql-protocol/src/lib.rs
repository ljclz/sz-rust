//! SzRSQL 协议层：PG Wire/REST/WebSocket。
//!
//! 对应 `SzRSQL技术实现方案.md` 8.2 节。
//!
//! Phase 4.1 交付物：
//! - `pgwire/message.rs` — pgwire 消息定义与编解码
//! - `pgwire/startup.rs` — 启动消息握手
//! - `pgwire/server.rs` — TCP 服务器与连接处理
//! - `pgwire/mod.rs` — 模块导出
//!
//! Phase 4.5.8-4.5.10 交付物：
//! - `http.rs` — HTTP/1.1 管理端点（healthz/readyz/metrics + sessions/cancel/backup/config）
//!
//! Phase 7d.17 交付物：
//! - `openapi.rs` — OpenAPI 3.0 规范生成 + Swagger UI 页面渲染
//!
//! Phase 7d.18 交付物：
//! - `health.rs` — Docker HEALTHCHECK 健康检查器（TCP/HTTP 探针）

#![allow(dead_code)]

pub mod health;
pub mod http;
pub mod openapi;
pub mod pgwire;

pub use health::{HealthChecker, HealthStatus};
pub use http::{HttpConfig, HttpError, HttpServer, ManagementHandle, MetricsRegistry};

/// 返回 crate 版本号，供 workspace 骨架冒烟测试使用。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_nonempty() {
        assert!(!version().is_empty());
    }
}
