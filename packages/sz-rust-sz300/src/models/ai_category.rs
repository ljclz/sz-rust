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
    /// 关联的商品类目 ID
    pub cat_id: i64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample() -> AiCategory {
        AiCategory {
            ai_class_id: Some(1),
            name: "叶菜类".into(),
            cat_id: 3,
            model_version: "v1.0".into(),
            created_at: Some("2026-01-01".into()),
            updated_at: Some("2026-01-02".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(AiCategory::table_name(), "ai_category");
        assert_eq!(AiCategory::pk_name(), "ai_class_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut a = sample();
        assert_eq!(a.pk(), 1);
        a.ai_class_id = None;
        assert_eq!(a.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut a = sample();
        a.set_pk(22);
        assert_eq!(a.ai_class_id, Some(22));
    }

    #[test]
    fn timestamp_fields_present() {
        let tf = AiCategory::timestamp_fields().expect("应有时间戳");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, Some("updated_at"));
    }

    #[test]
    fn columns_count() {
        assert_eq!(AiCategory::columns().len(), 6);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = AiCategory::fillable();
        assert!(!f.contains(&"ai_class_id"));
        assert!(f.contains(&"name"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(AiCategory::guarded(), vec!["ai_class_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let a = sample();
        assert_eq!(a.get_column_value("ai_class_id"), Some(Value::I64(1)));
        assert_eq!(
            a.get_column_value("name"),
            Some(Value::String("叶菜类".into()))
        );
        assert_eq!(a.get_column_value("cat_id"), Some(Value::I64(3)));
        assert_eq!(
            a.get_column_value("model_version"),
            Some(Value::String("v1.0".into()))
        );
        assert_eq!(a.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let a = AiCategory {
            ai_class_id: None,
            created_at: None,
            updated_at: None,
            ..sample()
        };
        assert_eq!(a.get_column_value("ai_class_id"), None);
    }

    #[test]
    fn from_value_populates_all() {
        let mut a = AiCategory {
            ai_class_id: None,
            name: String::new(),
            cat_id: 0,
            model_version: String::new(),
            created_at: None,
            updated_at: None,
        };
        let mut m = HashMap::new();
        m.insert("ai_class_id".into(), Value::I64(5));
        m.insert("name".into(), Value::String("根茎类".into()));
        m.insert("cat_id".into(), Value::I64(2));
        m.insert("model_version".into(), Value::String("v2.0".into()));
        m.insert("created_at".into(), Value::String("2026-01-01".into()));
        m.insert("updated_at".into(), Value::String("2026-01-02".into()));
        a.from_value(m);
        assert_eq!(a.ai_class_id, Some(5));
        assert_eq!(a.name, "根茎类");
        assert_eq!(a.model_version, "v2.0");
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut a = sample();
        a.from_value(HashMap::new());
        assert_eq!(a.ai_class_id, Some(1));
    }

    #[test]
    fn relation_loader_returns_none() {
        let a = sample();
        assert!(a.get_relation("any").is_none());
        assert_eq!(a.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut a = sample();
        a.set_relation_data("x", Value::Null);
        assert!(a.get_relation("x").is_none());
    }
}
