use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, RelationLoader, TimestampFields, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub cat_id: Option<i64>,
    pub name: String,
    pub parent_id: i64,
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
        vec![
            "cat_id",
            "name",
            "parent_id",
            "sort_order",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "name",
            "parent_id",
            "sort_order",
        ]
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
