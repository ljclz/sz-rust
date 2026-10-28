// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! # sz-rust-pipeline — 异步流水线（spec §5.8）
//!
//! 请求处理阶段流水线化 + 阶段并行 + 背压控制 + 阶段取消。
//!
//! ## 流水线阶段
//!
//! 典型阶段：解析 → 认证 → 路由 → 处理 → 序列化。
//! 无依赖阶段并行执行，有依赖阶段按拓扑顺序执行。

#![forbid(unsafe_code)]

pub mod backpressure;
pub mod error;
pub mod metrics;
pub mod pipeline;
pub mod stage;

pub use backpressure::*;
pub use error::*;
pub use metrics::*;
pub use pipeline::*;
pub use stage::*;
