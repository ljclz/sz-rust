// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 基准压测套件（criterion 基准 + 回归检测 + 性能预算门禁）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate 控制，默认不启用。

#![forbid(unsafe_code)]

pub mod benchmark;
pub mod error;

pub use benchmark::*;
pub use error::*;
