use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 系统配置模型实体（对齐 PHP SystemConfig 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemConfig {
    /// 配置键名（主键）
    pub key_name: String,
    /// 配置值
    pub value: String,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for SystemConfig {
    type PrimaryKey = String;

    fn table_name() -> &'static str {
        "system_config"
    }

    fn pk_name() -> &'static str {
        "key_name"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.key_name.clone()
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.key_name = pk;
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for SystemConfig {
    fn columns() -> Vec<&'static str> {
        vec!["key_name", "value", "created_at", "updated_at"]
    }

    fn fillable() -> Vec<&'static str> {
        vec!["key_name", "value"]
    }

    fn guarded() -> Vec<&'static str> {
        vec![]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "key_name" => Some(Value::String(self.key_name.clone())),
            "value" => Some(Value::String(self.value.clone())),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("key_name").and_then(|v| v.as_str()) {
            self.key_name = v.to_string();
        }
        if let Some(v) = map.get("value").and_then(|v| v.as_str()) {
            self.value = v.to_string();
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
        if let Some(v) = map.get("updated_at").and_then(|v| v.as_str()) {
            self.updated_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for SystemConfig {
    fn get_relation(&self, _name: &str) -> Option<&Value> {
        None
    }

    fn set_relation_data(&mut self, _name: &str, _data: Value) {}

    fn get_relation_fk_value(&self, _fk_name: &str) -> String {
        String::new()
    }
}
