// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! SZ-Rust 统一门面（v1.3.0 P3）
//!
//! 对齐 PHP `think\facade\*`，提供八个静态门面，统一 OnceCell 委托模式。
//!
//! | 门面 | 委托目标 | 对齐 PHP |
//! |------|---------|----------|
//! | [`Cache`] | `sz-rust-cache-facade` | `think\facade\Cache` |
//! | [`Db`] | `sz-rust-orm-facade` (Pool) | `think\facade\Db` |
//! | [`Event`] | `sz-rust-state-facade` | `think\facade\Event` |
//! | [`Queue`] | `sz-rust-orm-facade` | `think\facade\Queue` |
//! | [`Log`] | `sz-rust-middleware-facade` | `think\facade\Log` |
//! | [`Config`] | `sz-rust-infra-facade` | `think\facade\Config` |
//! | [`Request`] | `sz-rust-http-facade` | `think\Request` |
//! | [`Response`] | `sz-rust-http-facade` | `think\Response` |
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_facade::Cache;
//! use serde_json::json;
//!
//! Cache::set("key", &json!("value"), None).unwrap();
//! let val = Cache::get("key").unwrap();
//! ```

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod error;
pub use error::FacadeError;

pub mod cache;
pub mod config;
pub mod db;
pub mod event;
pub mod log;
pub mod queue;
pub mod request;
pub mod response;

pub use cache::Cache;
pub use config::Config;
pub use db::Db;
pub use event::Event;
pub use log::Log;
pub use queue::Queue;
pub use request::Request;
pub use response::Response;

#[cfg(feature = "test-utils")]
pub mod mock;
