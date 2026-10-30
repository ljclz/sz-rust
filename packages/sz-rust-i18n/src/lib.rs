// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 多语言 i18n 支持（消息资源管理 + 运行时切换 + 缺失键降级 + 复数支持）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate 控制，默认不启用。

#![forbid(unsafe_code)]

pub mod error;
pub mod fallback;
pub mod plural;
pub mod resource;
pub mod translator;

pub use error::*;
pub use fallback::*;
pub use plural::*;
pub use resource::*;
pub use translator::*;
