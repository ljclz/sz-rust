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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample() -> SystemConfig {
        SystemConfig {
            key_name: "site_name".into(),
            value: "鲜视达".into(),
            created_at: Some("2026-01-01".into()),
            updated_at: Some("2026-01-02".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(SystemConfig::table_name(), "system_config");
        assert_eq!(SystemConfig::pk_name(), "key_name");
    }

    #[test]
    fn pk_returns_key_name() {
        let s = sample();
        assert_eq!(s.pk(), "site_name");
    }

    #[test]
    fn set_pk_updates_key_name() {
        let mut s = sample();
        s.set_pk("new_key".into());
        assert_eq!(s.key_name, "new_key");
    }

    #[test]
    fn timestamp_fields_present() {
        let tf = SystemConfig::timestamp_fields().expect("应有时间戳");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, Some("updated_at"));
    }

    #[test]
    fn columns_count() {
        assert_eq!(SystemConfig::columns().len(), 4);
    }

    #[test]
    fn fillable_contains_key_and_value() {
        let f = SystemConfig::fillable();
        assert!(f.contains(&"key_name"));
        assert!(f.contains(&"value"));
    }

    #[test]
    fn guarded_empty() {
        assert!(SystemConfig::guarded().is_empty());
    }

    #[test]
    fn get_column_value_all_fields() {
        let s = sample();
        assert_eq!(
            s.get_column_value("key_name"),
            Some(Value::String("site_name".into()))
        );
        assert_eq!(
            s.get_column_value("value"),
            Some(Value::String("鲜视达".into()))
        );
        assert_eq!(s.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let s = SystemConfig {
            created_at: None,
            updated_at: None,
            ..sample()
        };
        assert_eq!(s.get_column_value("created_at"), None);
    }

    #[test]
    fn from_value_populates_all() {
        let mut s = SystemConfig {
            key_name: String::new(),
            value: String::new(),
            created_at: None,
            updated_at: None,
        };
        let mut m = HashMap::new();
        m.insert("key_name".into(), Value::String("k".into()));
        m.insert("value".into(), Value::String("v".into()));
        m.insert("created_at".into(), Value::String("2026-01-01".into()));
        m.insert("updated_at".into(), Value::String("2026-01-02".into()));
        s.from_value(m);
        assert_eq!(s.key_name, "k");
        assert_eq!(s.value, "v");
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut s = sample();
        s.from_value(HashMap::new());
        assert_eq!(s.key_name, "site_name");
    }

    #[test]
    fn relation_loader_returns_none() {
        let s = sample();
        assert!(s.get_relation("any").is_none());
        assert_eq!(s.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut s = sample();
        s.set_relation_data("x", Value::Null);
        assert!(s.get_relation("x").is_none());
    }
}
