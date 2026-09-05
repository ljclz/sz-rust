// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据范围控制 Data Scope — 应用层行级数据过滤
//!
//! 对齐 FSSADMIN `DataScopeTrait`，在查询时自动注入数据范围过滤器。
//! 与 PostgreSQL RLS 互补：RLS 在数据库层，Data Scope 在应用层，两者取交集。
//!
//! ## 5 种模式
//!
//! - `All`：全部数据（不追加条件）
//! - `Dept`：本部门（`WHERE dept_id = ?`）
//! - `DeptAndSub`：本部门及子部门（`WHERE dept_id IN (...)`）
//! - `Self_`：仅本人（`WHERE creator_id = ?`）
//! - `Custom`：自定义条件（通过 `CustomConditionGenerator` 生成）

pub mod cache;
pub mod context;
pub mod custom;
pub mod error;
pub mod evaluator;
pub mod ext;
pub mod metrics;
pub mod modes;
pub mod rule;

pub use context::DataScopeContext;
pub use error::DataScopeError;
pub use evaluator::{DataScopeEvaluator, DefaultDataScopeEvaluator};
pub use rule::{DataScopeMode, DataScopeRule};
