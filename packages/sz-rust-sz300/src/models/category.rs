// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 商品类目模型实体（对齐 PHP Category 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    /// 类目主键 ID
    pub cat_id: Option<i64>,
    /// 类目名称
    pub name: String,
    /// 父类目 ID（NULL 表示顶级类目；017_nullable_category_parent.sql 后改 NULLABLE）
    pub parent_id: Option<i64>,
    /// 排序权重（数值越小越靠前）
    pub sort_order: i32,
}

impl Model for Category {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "category"
    }

    fn pk_name() -> &'static str {
        "cat_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.cat_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.cat_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }
}

impl ModelExt for Category {
    fn columns() -> Vec<&'static str> {
        vec!["cat_id", "name", "parent_id", "sort_order"]
    }

    fn fillable() -> Vec<&'static str> {
        vec!["name", "parent_id", "sort_order"]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["cat_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "cat_id" => self.cat_id.map(Value::I64),
            "name" => Some(Value::String(self.name.clone())),
            "parent_id" => self.parent_id.map(Value::I64),
            "sort_order" => Some(Value::I32(self.sort_order)),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("cat_id").and_then(|v| v.as_i64()) {
            self.cat_id = Some(v);
        }
        if let Some(v) = map.get("name").and_then(|v| v.as_str()) {
            self.name = v.to_string();
        }
        if let Some(v) = map.get("parent_id").and_then(|v| v.as_i64()) {
            self.parent_id = Some(v);
        }
        if let Some(v) = map.get("sort_order").and_then(|v| v.as_i64()) {
            self.sort_order = v as i32;
        }
    }
}

impl RelationLoader for Category {
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
        let c = Category {
            cat_id: Some(5),
            name: "水产".into(),
            parent_id: Some(1),
            sort_order: 10,
        };
        let json = serde_json::to_string(&c).expect("serialize");
        let back: Category = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.cat_id, Some(5));
        assert_eq!(back.name, "水产");
        assert_eq!(back.parent_id, Some(1));
        assert_eq!(back.sort_order, 10);

        // 顶级类目 parent_id = None
        let top = Category {
            cat_id: None,
            name: "根类目".into(),
            parent_id: None,
            sort_order: 0,
        };
        let j2 = serde_json::to_string(&top).expect("serialize");
        let b2: Category = serde_json::from_str(&j2).expect("deserialize");
        assert_eq!(b2.cat_id, None);
        assert_eq!(b2.parent_id, None);
    }

    #[test]
    fn model_trait_metadata_and_pk_defaults() {
        assert_eq!(Category::table_name(), "category");
        assert_eq!(Category::pk_name(), "cat_id");
        let mut c = Category {
            cat_id: None,
            name: String::new(),
            parent_id: None,
            sort_order: 0,
        };
        assert_eq!(c.pk(), 0);
        assert!(c.get_column_value("cat_id").is_none());
        assert!(c.get_column_value("parent_id").is_none());
        c.set_pk(42);
        assert_eq!(c.pk(), 42);
        assert!(c.get_column_value("cat_id").is_some());
        c.parent_id = Some(9);
        assert!(c.get_column_value("parent_id").is_some());
        assert!(c.get_column_value("name").is_some());
        assert!(c.get_column_value("sort_order").is_some());
        assert!(c.get_column_value("nope").is_none());
        assert_eq!(Category::columns().len(), 4);
        assert_eq!(Category::fillable().len(), 3);
        assert_eq!(Category::guarded().len(), 1);
        assert!(Category::timestamp_fields().is_none());
    }

    #[test]
    fn from_value_populates_all_fields() {
        let mut c = Category {
            cat_id: None,
            name: String::new(),
            parent_id: None,
            sort_order: 0,
        };
        let mut map = HashMap::new();
        map.insert("cat_id".to_string(), Value::I64(7));
        map.insert("name".to_string(), Value::String("蔬菜".into()));
        map.insert("parent_id".to_string(), Value::I64(2));
        map.insert("sort_order".to_string(), Value::I64(5));
        c.from_value(map);
        assert_eq!(c.cat_id, Some(7));
        assert_eq!(c.name, "蔬菜");
        assert_eq!(c.parent_id, Some(2));
        assert_eq!(c.sort_order, 5);
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut c = Category {
            cat_id: Some(1),
            name: "x".into(),
            parent_id: Some(2),
            sort_order: 3,
        };
        c.from_value(HashMap::new());
        assert_eq!(c.cat_id, Some(1));
        assert_eq!(c.name, "x");
        assert_eq!(c.sort_order, 3);
    }

    #[test]
    fn relation_loader_returns_none() {
        let c = Category {
            cat_id: Some(1),
            name: String::new(),
            parent_id: None,
            sort_order: 0,
        };
        assert!(c.get_relation("any").is_none());
        let mut c2 = c.clone();
        c2.set_relation_data("any", Value::String("v".into()));
        assert_eq!(c2.get_relation_fk_value("any"), String::new());
    }
}
