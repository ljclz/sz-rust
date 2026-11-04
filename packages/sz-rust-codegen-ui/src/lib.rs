// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 代码生成器 UI（可视化建模 + 模板编辑 + 代码预览 + 导出/部署）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate 控制，默认不启用。
//!
//! # 组成
//!
//! - [`model`]：可视化数据模型（spec §5.27 规则 1）
//! - [`template_editor`]：代码模板编辑（spec §5.27 规则 2）
//! - [`preview`]：代码预览 + 语法高亮（spec §5.27 规则 3）
//! - [`export`]：代码导出（spec §5.27 规则 4）
//! - [`deploy`]：代码部署（spec §5.27 规则 5）

#![forbid(unsafe_code)]

pub mod deploy;
pub mod error;
pub mod export;
pub mod model;
pub mod preview;
pub mod template_editor;

pub use error::*;
