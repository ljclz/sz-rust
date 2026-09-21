// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 模型元信息

use serde::{Deserialize, Serialize};

use super::{FieldMetadata, RelationMetadata, ValidationRule};

/// 模型元信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelMetadata {
    /// 模型名（结构体名）
    pub name: String,
    /// 表名
    pub table_name: String,
    /// 模块名（snake_case）
    pub module_name: String,
    /// 字段列表
    pub fields: Vec<FieldMetadata>,
    /// 关系列表
    pub relations: Vec<RelationMetadata>,
    /// 验证规则
    pub validations: Vec<ValidationRule>,
    /// 文档注释
    pub doc_comment: Option<String>,
}

impl ModelMetadata {
    /// 返回主键字段引用
    pub fn primary_key(&self) -> Option<&FieldMetadata> {
        self.fields.iter().find(|f| f.is_primary_key)
    }

    /// 返回可写字段（排除主键与自动时间戳）
    pub fn writable_fields(&self) -> Vec<&FieldMetadata> {
        self.fields
            .iter()
            .filter(|f| !f.is_primary_key && !f.is_auto_timestamp)
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{FieldMetadata, RelationKind, RelationMetadata, ValidationRuleType};

    fn make_field(name: &str, pk: bool, auto_ts: bool) -> FieldMetadata {
        FieldMetadata {
            name: name.to_string(),
            rust_type: "String".to_string(),
            ts_type: "string".to_string(),
            sql_type: String::new(),
            is_nullable: false,
            is_primary_key: pk,
            is_indexed: false,
            is_sensitive: false,
            is_auto_timestamp: auto_ts,
            validation_rules: Vec::new(),
            relation: None,
            doc_comment: None,
        }
    }

    fn make_model(fields: Vec<FieldMetadata>) -> ModelMetadata {
        ModelMetadata {
            name: "User".to_string(),
            table_name: "user".to_string(),
            module_name: "user".to_string(),
            fields,
            relations: Vec::new(),
            validations: Vec::new(),
            doc_comment: None,
        }
    }

    #[test]
    fn test_primary_key_found() {
        let model = make_model(vec![
            make_field("id", true, false),
            make_field("name", false, false),
        ]);
        let pk = model.primary_key();
        assert!(pk.is_some());
        assert_eq!(pk.unwrap().name, "id");
    }

    #[test]
    fn test_primary_key_not_found() {
        let model = make_model(vec![make_field("name", false, false)]);
        assert!(model.primary_key().is_none());
    }

    #[test]
    fn test_primary_key_empty_fields() {
        let model = make_model(Vec::new());
        assert!(model.primary_key().is_none());
    }

    #[test]
    fn test_writable_fields_excludes_pk_and_timestamps() {
        let model = make_model(vec![
            make_field("id", true, false),
            make_field("name", false, false),
            make_field("created_at", false, true),
            make_field("updated_at", false, true),
            make_field("email", false, false),
        ]);
        let writable = model.writable_fields();
        assert_eq!(writable.len(), 2);
        let names: Vec<&str> = writable.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"name"));
        assert!(names.contains(&"email"));
        assert!(!names.contains(&"id"));
        assert!(!names.contains(&"created_at"));
        assert!(!names.contains(&"updated_at"));
    }

    #[test]
    fn test_writable_fields_all_excluded() {
        let model = make_model(vec![
            make_field("id", true, false),
            make_field("created_at", false, true),
        ]);
        assert!(model.writable_fields().is_empty());
    }

    #[test]
    fn test_writable_fields_empty_model() {
        let model = make_model(Vec::new());
        assert!(model.writable_fields().is_empty());
    }

    #[test]
    fn test_model_metadata_serde_roundtrip() {
        let model = ModelMetadata {
            name: "Product".to_string(),
            table_name: "product".to_string(),
            module_name: "product".to_string(),
            fields: vec![make_field("id", true, false)],
            relations: vec![RelationMetadata {
                kind: RelationKind::HasMany,
                target_model: "Order".to_string(),
                foreign_key: Some("product_id".to_string()),
                through_table: None,
            }],
            validations: vec![ValidationRule {
                rule_type: ValidationRuleType::Required,
                param: None,
                message: Some("必填".to_string()),
            }],
            doc_comment: Some("产品模型".to_string()),
        };
        let json = serde_json::to_string(&model).unwrap();
        let back: ModelMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "Product");
        assert_eq!(back.table_name, "product");
        assert_eq!(back.fields.len(), 1);
        assert_eq!(back.relations.len(), 1);
        assert_eq!(back.relations[0].kind, RelationKind::HasMany);
        assert_eq!(back.validations.len(), 1);
        assert_eq!(back.doc_comment, Some("产品模型".to_string()));
    }
}
