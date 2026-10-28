// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 可视化数据模型（spec §5.27 规则 1，§6.27 规则 1）
//!
//! 表/字段/关系，字段类型校验。

use crate::error::CodegenUiError;

/// 字段类型（spec §6.27 规则 1）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FieldType {
    String,
    Integer,
    BigInt,
    Float,
    Decimal,
    Boolean,
    DateTime,
    Date,
    Json,
    Binary,
    Text,
    Uuid,
}

impl FieldType {
    /// 从字符串解析
    pub fn parse(s: &str) -> Result<Self, CodegenUiError> {
        match s.to_lowercase().as_str() {
            "string" => Ok(Self::String),
            "integer" | "int" | "i32" => Ok(Self::Integer),
            "bigint" | "i64" => Ok(Self::BigInt),
            "float" | "f64" => Ok(Self::Float),
            "decimal" => Ok(Self::Decimal),
            "boolean" | "bool" => Ok(Self::Boolean),
            "datetime" => Ok(Self::DateTime),
            "date" => Ok(Self::Date),
            "json" => Ok(Self::Json),
            "binary" | "bytes" => Ok(Self::Binary),
            "text" => Ok(Self::Text),
            "uuid" => Ok(Self::Uuid),
            _ => Err(CodegenUiError::InvalidParam(format!("未知字段类型: {s}"))),
        }
    }

    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::BigInt => "bigint",
            Self::Float => "float",
            Self::Decimal => "decimal",
            Self::Boolean => "boolean",
            Self::DateTime => "datetime",
            Self::Date => "date",
            Self::Json => "json",
            Self::Binary => "binary",
            Self::Text => "text",
            Self::Uuid => "uuid",
        }
    }

    /// 转为 Rust 类型
    pub fn to_rust_type(&self) -> &'static str {
        match self {
            Self::String => "String",
            Self::Integer => "i32",
            Self::BigInt => "i64",
            Self::Float => "f64",
            Self::Decimal => "Decimal",
            Self::Boolean => "bool",
            Self::DateTime => "DateTime<Utc>",
            Self::Date => "NaiveDate",
            Self::Json => "serde_json::Value",
            Self::Binary => "Vec<u8>",
            Self::Text => "String",
            Self::Uuid => "Uuid",
        }
    }
}

/// 数据模型字段（spec §6.27 规则 1）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelField {
    /// 字段名
    pub name: String,
    /// 字段类型
    pub field_type: FieldType,
    /// 是否可空
    pub nullable: bool,
    /// 注释
    pub comment: Option<String>,
}

impl ModelField {
    /// 创建字段
    pub fn new(name: impl Into<String>, field_type: FieldType) -> Self {
        Self {
            name: name.into(),
            field_type,
            nullable: false,
            comment: None,
        }
    }

    /// 设置可空
    pub fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }

    /// 设置注释
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }
}

/// 数据模型关系
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ModelRelation {
    /// 一对一
    HasOne { target: String, foreign_key: String },
    /// 一对多
    HasMany { target: String, foreign_key: String },
    /// 多对一
    BelongsTo { target: String, foreign_key: String },
    /// 多对多
    ManyToMany { target: String, pivot: String },
}

/// 可视化数据模型（spec §5.27 规则 1）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DataModel {
    /// 表名
    pub table_name: String,
    /// 字段列表
    pub fields: Vec<ModelField>,
    /// 关系列表
    pub relations: Vec<ModelRelation>,
    /// 注释
    pub comment: Option<String>,
}

impl DataModel {
    /// 创建模型
    pub fn new(table_name: impl Into<String>) -> Self {
        Self {
            table_name: table_name.into(),
            fields: Vec::new(),
            relations: Vec::new(),
            comment: None,
        }
    }

    /// 添加字段
    pub fn with_field(mut self, field: ModelField) -> Self {
        self.fields.push(field);
        self
    }

    /// 添加关系
    pub fn with_relation(mut self, relation: ModelRelation) -> Self {
        self.relations.push(relation);
        self
    }

    /// 校验模型（spec §6.27 规则 1）
    pub fn validate(&self) -> Result<(), CodegenUiError> {
        if self.table_name.is_empty() {
            return Err(CodegenUiError::InvalidParam("表名不能为空".into()));
        }
        if self.fields.is_empty() {
            return Err(CodegenUiError::InvalidParam("字段列表不能为空".into()));
        }
        for field in &self.fields {
            if field.name.is_empty() {
                return Err(CodegenUiError::InvalidParam("字段名不能为空".into()));
            }
        }
        Ok(())
    }

    /// 从 JSON 解析模型
    pub fn from_json(json: &serde_json::Value) -> Result<Self, CodegenUiError> {
        let table_name = json
            .get("table_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CodegenUiError::InvalidParam("缺少 table_name".into()))?;
        let mut model = Self::new(table_name);
        if let Some(fields) = json.get("fields").and_then(|v| v.as_array()) {
            for field in fields {
                let name = field
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| CodegenUiError::InvalidParam("缺少字段 name".into()))?;
                let type_str = field
                    .get("type")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| CodegenUiError::InvalidParam("缺少字段 type".into()))?;
                let field_type = FieldType::parse(type_str)?;
                let mut model_field = ModelField::new(name, field_type);
                if let Some(nullable) = field.get("nullable").and_then(|v| v.as_bool()) {
                    if nullable {
                        model_field = model_field.nullable();
                    }
                }
                model = model.with_field(model_field);
            }
        }
        Ok(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_type_from_str() {
        assert_eq!(FieldType::parse("string").unwrap(), FieldType::String);
        assert_eq!(FieldType::parse("i32").unwrap(), FieldType::Integer);
        assert!(FieldType::parse("invalid").is_err());
    }

    #[test]
    fn test_field_type_to_rust() {
        assert_eq!(FieldType::String.to_rust_type(), "String");
        assert_eq!(FieldType::Integer.to_rust_type(), "i32");
        assert_eq!(FieldType::Boolean.to_rust_type(), "bool");
    }

    #[test]
    fn test_data_model_validate() {
        let model = DataModel::new("users")
            .with_field(ModelField::new("id", FieldType::BigInt))
            .with_field(ModelField::new("name", FieldType::String));
        assert!(model.validate().is_ok());
    }

    #[test]
    fn test_data_model_empty_table() {
        let model = DataModel::new("");
        assert!(model.validate().is_err());
    }

    #[test]
    fn test_data_model_no_fields() {
        let model = DataModel::new("users");
        assert!(model.validate().is_err());
    }

    #[test]
    fn test_data_model_from_json() {
        let json = serde_json::json!({
            "table_name": "users",
            "fields": [
                {"name": "id", "type": "bigint"},
                {"name": "name", "type": "string", "nullable": false}
            ]
        });
        let model = DataModel::from_json(&json).unwrap();
        assert_eq!(model.table_name, "users");
        assert_eq!(model.fields.len(), 2);
    }

    #[test]
    fn test_field_type_parse_all_variants() {
        assert_eq!(FieldType::parse("string").unwrap(), FieldType::String);
        assert_eq!(FieldType::parse("integer").unwrap(), FieldType::Integer);
        assert_eq!(FieldType::parse("int").unwrap(), FieldType::Integer);
        assert_eq!(FieldType::parse("i32").unwrap(), FieldType::Integer);
        assert_eq!(FieldType::parse("bigint").unwrap(), FieldType::BigInt);
        assert_eq!(FieldType::parse("i64").unwrap(), FieldType::BigInt);
        assert_eq!(FieldType::parse("float").unwrap(), FieldType::Float);
        assert_eq!(FieldType::parse("f64").unwrap(), FieldType::Float);
        assert_eq!(FieldType::parse("decimal").unwrap(), FieldType::Decimal);
        assert_eq!(FieldType::parse("boolean").unwrap(), FieldType::Boolean);
        assert_eq!(FieldType::parse("bool").unwrap(), FieldType::Boolean);
        assert_eq!(FieldType::parse("datetime").unwrap(), FieldType::DateTime);
        assert_eq!(FieldType::parse("date").unwrap(), FieldType::Date);
        assert_eq!(FieldType::parse("json").unwrap(), FieldType::Json);
        assert_eq!(FieldType::parse("binary").unwrap(), FieldType::Binary);
        assert_eq!(FieldType::parse("bytes").unwrap(), FieldType::Binary);
        assert_eq!(FieldType::parse("text").unwrap(), FieldType::Text);
        assert_eq!(FieldType::parse("uuid").unwrap(), FieldType::Uuid);
    }

    #[test]
    fn test_field_type_as_str_all() {
        assert_eq!(FieldType::String.as_str(), "string");
        assert_eq!(FieldType::Integer.as_str(), "integer");
        assert_eq!(FieldType::BigInt.as_str(), "bigint");
        assert_eq!(FieldType::Float.as_str(), "float");
        assert_eq!(FieldType::Decimal.as_str(), "decimal");
        assert_eq!(FieldType::Boolean.as_str(), "boolean");
        assert_eq!(FieldType::DateTime.as_str(), "datetime");
        assert_eq!(FieldType::Date.as_str(), "date");
        assert_eq!(FieldType::Json.as_str(), "json");
        assert_eq!(FieldType::Binary.as_str(), "binary");
        assert_eq!(FieldType::Text.as_str(), "text");
        assert_eq!(FieldType::Uuid.as_str(), "uuid");
    }

    #[test]
    fn test_field_type_to_rust_type_all() {
        assert_eq!(FieldType::String.to_rust_type(), "String");
        assert_eq!(FieldType::Integer.to_rust_type(), "i32");
        assert_eq!(FieldType::BigInt.to_rust_type(), "i64");
        assert_eq!(FieldType::Float.to_rust_type(), "f64");
        assert_eq!(FieldType::Decimal.to_rust_type(), "Decimal");
        assert_eq!(FieldType::Boolean.to_rust_type(), "bool");
        assert_eq!(FieldType::DateTime.to_rust_type(), "DateTime<Utc>");
        assert_eq!(FieldType::Date.to_rust_type(), "NaiveDate");
        assert_eq!(FieldType::Json.to_rust_type(), "serde_json::Value");
        assert_eq!(FieldType::Binary.to_rust_type(), "Vec<u8>");
        assert_eq!(FieldType::Text.to_rust_type(), "String");
        assert_eq!(FieldType::Uuid.to_rust_type(), "Uuid");
    }

    #[test]
    fn test_model_field_builder() {
        let field = ModelField::new("email", FieldType::String)
            .nullable()
            .with_comment("用户邮箱");
        assert_eq!(field.name, "email");
        assert!(field.nullable);
        assert_eq!(field.comment, Some("用户邮箱".to_string()));
    }

    #[test]
    fn test_data_model_with_relation() {
        let model = DataModel::new("orders")
            .with_field(ModelField::new("id", FieldType::BigInt))
            .with_relation(ModelRelation::BelongsTo {
                target: "users".to_string(),
                foreign_key: "user_id".to_string(),
            })
            .with_relation(ModelRelation::HasMany {
                target: "order_items".to_string(),
                foreign_key: "order_id".to_string(),
            });
        assert_eq!(model.relations.len(), 2);
    }

    #[test]
    fn test_data_model_validate_empty_field_name() {
        let model = DataModel::new("users").with_field(ModelField::new("", FieldType::String));
        assert!(model.validate().is_err());
    }

    #[test]
    fn test_data_model_from_json_missing_table_name() {
        let json = serde_json::json!({"fields": []});
        assert!(DataModel::from_json(&json).is_err());
    }

    #[test]
    fn test_data_model_from_json_missing_field_name() {
        let json = serde_json::json!({
            "table_name": "users",
            "fields": [{"type": "string"}]
        });
        assert!(DataModel::from_json(&json).is_err());
    }

    #[test]
    fn test_data_model_from_json_missing_field_type() {
        let json = serde_json::json!({
            "table_name": "users",
            "fields": [{"name": "id"}]
        });
        assert!(DataModel::from_json(&json).is_err());
    }

    #[test]
    fn test_data_model_from_json_nullable_true() {
        let json = serde_json::json!({
            "table_name": "users",
            "fields": [
                {"name": "id", "type": "bigint"},
                {"name": "email", "type": "string", "nullable": true}
            ]
        });
        let model = DataModel::from_json(&json).unwrap();
        assert!(!model.fields[0].nullable);
        assert!(model.fields[1].nullable);
    }

    #[test]
    fn test_data_model_from_json_no_fields() {
        let json = serde_json::json!({"table_name": "users"});
        let model = DataModel::from_json(&json).unwrap();
        assert_eq!(model.table_name, "users");
        assert!(model.fields.is_empty());
    }

    #[test]
    fn test_data_model_from_json_invalid_type() {
        let json = serde_json::json!({
            "table_name": "users",
            "fields": [{"name": "id", "type": "invalid_type"}]
        });
        assert!(DataModel::from_json(&json).is_err());
    }

    #[test]
    fn test_model_relation_variants() {
        let r1 = ModelRelation::HasOne {
            target: "profile".to_string(),
            foreign_key: "profile_id".to_string(),
        };
        let r2 = ModelRelation::ManyToMany {
            target: "roles".to_string(),
            pivot: "user_roles".to_string(),
        };
        let model = DataModel::new("users")
            .with_field(ModelField::new("id", FieldType::BigInt))
            .with_relation(r1)
            .with_relation(r2);
        assert_eq!(model.relations.len(), 2);
        assert!(model.validate().is_ok());
    }
}
