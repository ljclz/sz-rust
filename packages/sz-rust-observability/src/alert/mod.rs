// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 告警规则引擎（规则评估 + 通知渠道 + 静默/抑制）
//!
//! v1.7.0 新增模块，通过 Cargo feature gate 控制，默认不启用。
//! 合并自 sz-rust-alert-engine（ADR-053 阶段3，2026-11-06）。

#![allow(missing_docs)]

pub mod error;
pub mod rule;
pub mod silence;

pub use error::*;
pub use rule::*;
pub use silence::*;