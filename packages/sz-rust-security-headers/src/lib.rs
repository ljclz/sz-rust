// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 安全头中间件（CSP/HSTS/X-Frame-Options 等安全响应头）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate 控制，默认不启用。

#![forbid(unsafe_code)]
pub mod error;

pub use error::*;
