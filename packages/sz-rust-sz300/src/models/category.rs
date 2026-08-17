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
    /// 父类目 ID（0 表示顶级类目）
    pub parent_id: i64,
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
            "parent_id" => Some(Value::I64(self.parent_id)),
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
            self.parent_id = v;
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

    fn sample() -> Category {
        Category {
            cat_id: Some(1),
            name: "蔬菜".into(),
            parent_id: 0,
            sort_order: 1,
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(Category::table_name(), "category");
        assert_eq!(Category::pk_name(), "cat_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut c = sample();
        assert_eq!(c.pk(), 1);
        c.cat_id = None;
        assert_eq!(c.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut c = sample();
        c.set_pk(11);
        assert_eq!(c.cat_id, Some(11));
    }

    #[test]
    fn timestamp_fields_none() {
        assert!(Category::timestamp_fields().is_none());
    }

    #[test]
    fn columns_count() {
        assert_eq!(Category::columns().len(), 4);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = Category::fillable();
        assert!(!f.contains(&"cat_id"));
        assert!(f.contains(&"name"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(Category::guarded(), vec!["cat_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let c = sample();
        assert_eq!(c.get_column_value("cat_id"), Some(Value::I64(1)));
        assert_eq!(
            c.get_column_value("name"),
            Some(Value::String("蔬菜".into()))
        );
        assert_eq!(c.get_column_value("parent_id"), Some(Value::I64(0)));
        assert_eq!(c.get_column_value("sort_order"), Some(Value::I32(1)));
        assert_eq!(c.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_pk() {
        let c = Category {
            cat_id: None,
            ..sample()
        };
        assert_eq!(c.get_column_value("cat_id"), None);
    }

    #[test]
    fn from_value_populates_all() {
        let mut c = Category {
            cat_id: None,
            name: String::new(),
            parent_id: 0,
            sort_order: 0,
        };
        let mut m = HashMap::new();
        m.insert("cat_id".into(), Value::I64(5));
        m.insert("name".into(), Value::String("水果".into()));
        m.insert("parent_id".into(), Value::I64(2));
        m.insert("sort_order".into(), Value::I64(10));
        c.from_value(m);
        assert_eq!(c.cat_id, Some(5));
        assert_eq!(c.name, "水果");
        assert_eq!(c.sort_order, 10);
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut c = sample();
        c.from_value(HashMap::new());
        assert_eq!(c.cat_id, Some(1));
    }

    #[test]
    fn relation_loader_returns_none() {
        let c = sample();
        assert!(c.get_relation("any").is_none());
        assert_eq!(c.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut c = sample();
        c.set_relation_data("x", Value::Null);
        assert!(c.get_relation("x").is_none());
    }
}
