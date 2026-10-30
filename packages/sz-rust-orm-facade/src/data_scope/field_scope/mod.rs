// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 字段级数据权限 — 按表+角色维度控制字段可见性

pub mod error;
pub mod evaluator;
pub mod filter;
pub mod policy;
pub mod registry;
pub mod result;
pub mod visibility;

pub use error::FieldScopeError;
pub use evaluator::{DefaultFieldScopeEvaluator, FieldScopeEvaluator};
pub use filter::FieldFilter;
pub use policy::FieldScopePolicy;
pub use registry::FieldScopePolicyRegistry;
pub use result::FieldScopeResult;
pub use visibility::FieldVisibility;
