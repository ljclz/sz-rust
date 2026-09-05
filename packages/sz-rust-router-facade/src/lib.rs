// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! sz-rust-router-facade — 路由层（P3 解耦）
//!
//! 从 sz-rust-core 提取的三层路由机制（对齐 PHP ThinkPHP 路由体系）：
//!
//! - [`router`]：路由构建层（`parse_path` / `RouterBuilder` / 资源路由）
//! - [`routing`]：三层路由机制（属性宏 / 配置式 / 约定式）
//! - [`websocket_route`]：WebSocket 路由注册
//! - [`openapi`]：OpenAPI 规范自动生成（消费 [`routing`] 的规则）
//!
//! sz-rust-core 通过 `pub use sz_rust_router_facade::{router, routing, websocket_route, openapi}`
//! 保留向后兼容路径。

pub mod openapi;
pub mod router;
pub mod routing;
pub mod simd_str;
pub mod websocket_route;
