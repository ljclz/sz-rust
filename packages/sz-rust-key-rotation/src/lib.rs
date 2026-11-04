// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 密钥轮换（自动轮换策略 + 密钥版本管理 + 无缝切换）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate 控制，默认不启用。

#![forbid(unsafe_code)]

pub mod error;
pub mod manager;

pub use error::*;
pub use manager::*;
