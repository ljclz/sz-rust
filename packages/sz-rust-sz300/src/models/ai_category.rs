// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// AI 分类模型实体（对齐 PHP AiCategory 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCategory {
    /// AI 分类主键 ID
    pub ai_class_id: Option<i64>,
    /// AI 分类名称
    pub name: String,
    /// 关联的商品类目 ID（NULL 表示未关联；018_nullable_ai_category_cat.sql 后改 NULLABLE）
    pub cat_id: Option<i64>,
    /// 模型版本号
    pub model_version: String,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for AiCategory {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "ai_category"
    }

    fn pk_name() -> &'static str {
        "ai_class_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.ai_class_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.ai_class_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for AiCategory {
    fn columns() -> Vec<&'static str> {
        vec![
            "ai_class_id",
            "name",
            "cat_id",
            "model_version",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec!["name", "cat_id", "model_version"]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["ai_class_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "ai_class_id" => self.ai_class_id.map(Value::I64),
            "name" => Some(Value::String(self.name.clone())),
            "cat_id" => self.cat_id.map(Value::I64),
            "model_version" => Some(Value::String(self.model_version.clone())),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("ai_class_id").and_then(|v| v.as_i64()) {
            self.ai_class_id = Some(v);
        }
        if let Some(v) = map.get("name").and_then(|v| v.as_str()) {
            self.name = v.to_string();
        }
        if let Some(v) = map.get("cat_id").and_then(|v| v.as_i64()) {
            self.cat_id = Some(v);
        }
        if let Some(v) = map.get("model_version").and_then(|v| v.as_str()) {
            self.model_version = v.to_string();
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
        if let Some(v) = map.get("updated_at").and_then(|v| v.as_str()) {
            self.updated_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for AiCategory {
    fn get_relation(&self, _name: &str) -> Option<&Value> {
        None
    }

    fn set_relation_data(&mut self, _name: &str, _data: Value) {}

    fn get_relation_fk_value(&self, _fk_name: &str) -> String {
        String::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn serde_roundtrip_preserves_all_fields() {
        let a = AiCategory {
            ai_class_id: Some(3),
            name: "鱼类识别".into(),
            cat_id: Some(5),
            model_version: "v2.1".into(),
            created_at: Some("2026-01-01 00:00:00".into()),
            updated_at: None,
        };
        let json = serde_json::to_string(&a).expect("serialize");
        let back: AiCategory = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.ai_class_id, Some(3));
        assert_eq!(back.name, "鱼类识别");
        assert_eq!(back.cat_id, Some(5));
        assert_eq!(back.model_version, "v2.1");
        assert_eq!(back.created_at.as_deref(), Some("2026-01-01 00:00:00"));
        assert_eq!(back.updated_at, None);
    }

    #[test]
    fn model_trait_metadata_and_pk_defaults() {
        assert_eq!(AiCategory::table_name(), "ai_category");
        assert_eq!(AiCategory::pk_name(), "ai_class_id");
        let mut a = AiCategory {
            ai_class_id: None,
            name: String::new(),
            cat_id: None,
            model_version: String::new(),
            created_at: None,
            updated_at: None,
        };
        assert_eq!(a.pk(), 0);
        assert!(a.get_column_value("ai_class_id").is_none());
        assert!(a.get_column_value("cat_id").is_none());
        assert!(a.get_column_value("created_at").is_none());
        assert!(a.get_column_value("updated_at").is_none());
        a.set_pk(77);
        assert_eq!(a.pk(), 77);
        assert!(a.get_column_value("ai_class_id").is_some());
        a.cat_id = Some(2);
        a.created_at = Some("c".into());
        a.updated_at = Some("u".into());
        assert!(a.get_column_value("cat_id").is_some());
        assert!(a.get_column_value("name").is_some());
        assert!(a.get_column_value("model_version").is_some());
        assert!(a.get_column_value("created_at").is_some());
        assert!(a.get_column_value("updated_at").is_some());
        assert!(a.get_column_value("nope").is_none());
        assert_eq!(AiCategory::columns().len(), 6);
        assert_eq!(AiCategory::fillable().len(), 3);
        assert_eq!(AiCategory::guarded().len(), 1);
        assert!(AiCategory::timestamp_fields().is_some());
    }

    #[test]
    fn from_value_populates_all_fields() {
        let mut a = AiCategory {
            ai_class_id: None,
            name: String::new(),
            cat_id: None,
            model_version: String::new(),
            created_at: None,
            updated_at: None,
        };
        let mut map = HashMap::new();
        map.insert("ai_class_id".to_string(), Value::I64(9));
        map.insert("name".to_string(), Value::String("贝类".into()));
        map.insert("cat_id".to_string(), Value::I64(4));
        map.insert("model_version".to_string(), Value::String("v3".into()));
        map.insert("created_at".to_string(), Value::String("c".into()));
        map.insert("updated_at".to_string(), Value::String("u".into()));
        a.from_value(map);
        assert_eq!(a.ai_class_id, Some(9));
        assert_eq!(a.name, "贝类");
        assert_eq!(a.cat_id, Some(4));
        assert_eq!(a.model_version, "v3");
        assert_eq!(a.created_at.as_deref(), Some("c"));
        assert_eq!(a.updated_at.as_deref(), Some("u"));
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut a = AiCategory {
            ai_class_id: Some(1),
            name: "x".into(),
            cat_id: Some(2),
            model_version: "v".into(),
            created_at: Some("c".into()),
            updated_at: Some("u".into()),
        };
        a.from_value(HashMap::new());
        assert_eq!(a.ai_class_id, Some(1));
        assert_eq!(a.name, "x");
        assert_eq!(a.model_version, "v");
    }

    #[test]
    fn relation_loader_returns_none() {
        let a = AiCategory {
            ai_class_id: Some(1),
            name: String::new(),
            cat_id: None,
            model_version: String::new(),
            created_at: None,
            updated_at: None,
        };
        assert!(a.get_relation("any").is_none());
        let mut a2 = a.clone();
        a2.set_relation_data("any", Value::String("v".into()));
        assert_eq!(a2.get_relation_fk_value("any"), String::new());
    }
}
