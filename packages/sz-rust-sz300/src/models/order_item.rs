use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, RelationLoader, TimestampFields, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderItem {
    pub item_id: Option<i64>,
    pub order_id: i64,
    pub good_id: i64,
    pub good_name: String,
    pub price_fen: i64,
    pub weight_g: i64,
    pub total_fen: i64,
    pub quantity: i32,
}

impl Model for OrderItem {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "order_item"
    }

    fn pk_name() -> &'static str {
        "item_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.item_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.item_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        None
    }
}

impl ModelExt for OrderItem {
    fn columns() -> Vec<&'static str> {
        vec![
            "item_id",
            "order_id",
            "good_id",
            "good_name",
            "price_fen",
            "weight_g",
            "total_fen",
            "quantity",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "order_id",
            "good_id",
            "good_name",
            "price_fen",
            "weight_g",
            "total_fen",
            "quantity",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["item_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "item_id" => self.item_id.map(Value::I64),
            "order_id" => Some(Value::I64(self.order_id)),
            "good_id" => Some(Value::I64(self.good_id)),
            "good_name" => Some(Value::String(self.good_name.clone())),
            "price_fen" => Some(Value::I64(self.price_fen)),
            "weight_g" => Some(Value::I64(self.weight_g)),
            "total_fen" => Some(Value::I64(self.total_fen)),
            "quantity" => Some(Value::I32(self.quantity)),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("item_id").and_then(|v| v.as_i64()) {
            self.item_id = Some(v);
        }
        if let Some(v) = map.get("order_id").and_then(|v| v.as_i64()) {
            self.order_id = v;
        }
        if let Some(v) = map.get("good_id").and_then(|v| v.as_i64()) {
            self.good_id = v;
        }
        if let Some(v) = map.get("good_name").and_then(|v| v.as_str()) {
            self.good_name = v.to_string();
        }
        if let Some(v) = map.get("price_fen").and_then(|v| v.as_i64()) {
            self.price_fen = v;
        }
        if let Some(v) = map.get("weight_g").and_then(|v| v.as_i64()) {
            self.weight_g = v;
        }
        if let Some(v) = map.get("total_fen").and_then(|v| v.as_i64()) {
            self.total_fen = v;
        }
        if let Some(v) = map.get("quantity").and_then(|v| v.as_i64()) {
            self.quantity = v as i32;
        }
    }
}

impl RelationLoader for OrderItem {
    fn get_relation(&self, _name: &str) -> Option<&Value> {
        None
    }

    fn set_relation_data(&mut self, _name: &str, _data: Value) {}

    fn get_relation_fk_value(&self, _fk_name: &str) -> String {
        String::new()
    }
}
