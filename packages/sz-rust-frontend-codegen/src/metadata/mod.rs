// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 模型元信息结构

pub mod field;
pub mod model;
pub mod relation;
pub mod validation;

pub use field::FieldMetadata;
pub use model::ModelMetadata;
pub use relation::{RelationKind, RelationMetadata};
pub use validation::{ValidationRule, ValidationRuleType};
