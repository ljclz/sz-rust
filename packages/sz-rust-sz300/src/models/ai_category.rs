use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, RelationLoader, TimestampFields, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCategory {
    pub ai_class_id: Option<i64>,
    pub name: String,
    pub cat_id: i64,
    pub model_version: String,
    pub created_at: Option<String>,
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
            "cat_id" => Some(Value::I64(self.cat_id)),
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
            self.cat_id = v;
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
