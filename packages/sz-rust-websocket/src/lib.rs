// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! WebSocket 长连接（连接管理 + 心跳 + 消息广播 + 房间/频道）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate 控制，默认不启用。
//!
//! # 组成
//!
//! - [`manager`]：连接生命周期管理（建立/保持/关闭）
//! - [`room`]：房间/频道分组与隔离
//! - [`broadcast`]：全局/房间消息广播
//! - [`heartbeat`]：心跳保活 + 空闲超时关闭
//! - [`auth`]：连接认证（token 验证）

#![forbid(unsafe_code)]

pub mod auth;
pub mod broadcast;
pub mod error;
pub mod heartbeat;
pub mod manager;
pub mod room;

pub use error::*;
