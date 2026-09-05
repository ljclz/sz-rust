// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 字段元信息

use serde::{Deserialize, Serialize};

use super::{RelationMetadata, ValidationRule};

/// 字段元信息
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldMetadata {
    /// 字段名
    pub name: String,
    /// Rust 类型
    pub rust_type: String,
    /// TypeScript 类型
    pub ts_type: String,
    /// SQL 类型
    pub sql_type: String,
    /// 是否可空
    pub is_nullable: bool,
    /// 是否主键
    pub is_primary_key: bool,
    /// 是否索引
    pub is_indexed: bool,
    /// 是否敏感字段
    pub is_sensitive: bool,
    /// 是否自动时间戳
    pub is_auto_timestamp: bool,
    /// 验证规则
    pub validation_rules: Vec<ValidationRule>,
    /// 关系
    pub relation: Option<RelationMetadata>,
    /// 文档注释
    pub doc_comment: Option<String>,
}
