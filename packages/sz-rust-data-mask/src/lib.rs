// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 数据脱敏（字段级脱敏规则 + 多场景脱敏 + 自定义函数）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate 控制，默认不启用。

#![forbid(unsafe_code)]

pub mod error;
pub mod layer;
pub mod mask;

pub use error::*;
pub use layer::*;
pub use mask::*;
