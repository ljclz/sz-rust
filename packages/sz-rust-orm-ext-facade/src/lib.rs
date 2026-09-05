// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! sz-rust-orm-ext-facade — ORM 扩展层（P3 解耦）
//!
//! 从 sz-rust-core 提取的 ORM 框架层抽象，基于 [`sz_rust_orm_facade`]（sz-orm-* 全家桶）构建：
//!
//! - [`model`]：BaseModel 基类与模型辅助（对齐 PHP `think\Model`）
//! - [`hooks`]：Model 钩子 / 全局作用域 / 软删除（对齐 PHP `think\model\concern\SoftDelete` 等）
//! - [`relation`]：关联关系（BelongsTo/HasMany/HasOne/Morph* 等，对齐 PHP `think\model\relation\*`）
//!
//! sz-rust-core 通过 `pub use sz_rust_orm_ext_facade::{model, hooks, relation}` 保留向后兼容路径。

pub mod hooks;
pub mod model;
pub mod relation;
