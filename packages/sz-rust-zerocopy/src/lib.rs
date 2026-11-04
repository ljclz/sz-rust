// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! # sz-rust-zerocopy — 零拷贝序列化（spec §5.9）
//!
//! rkyv/zerocopy 集成 + 零分配 + 跨序列化器互操作。
//! 全部通过安全 API 实现，workspace forbid(unsafe_code)。

#![forbid(unsafe_code)]

pub mod error;
pub mod serializer;

pub use error::*;
pub use serializer::*;
