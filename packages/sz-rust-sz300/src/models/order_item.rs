use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 订单明细模型实体（对齐 PHP OrderItem 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderItem {
    /// 订单项主键 ID
    pub item_id: Option<i64>,
    /// 所属订单 ID
    pub order_id: i64,
    /// 商品 ID
    pub good_id: i64,
    /// 商品名称（下单时快照）
    pub good_name: String,
    /// 单价（单位：分）
    pub price_fen: i64,
    /// 重量（单位：克）
    pub weight_g: i64,
    /// 小计金额（单位：分）
    pub total_fen: i64,
    /// 购买数量
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample() -> OrderItem {
        OrderItem {
            item_id: Some(1),
            order_id: 10,
            good_id: 5,
            good_name: "白菜".into(),
            price_fen: 250,
            weight_g: 500,
            total_fen: 12500,
            quantity: 5,
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(OrderItem::table_name(), "order_item");
        assert_eq!(OrderItem::pk_name(), "item_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut i = sample();
        assert_eq!(i.pk(), 1);
        i.item_id = None;
        assert_eq!(i.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut i = sample();
        i.set_pk(33);
        assert_eq!(i.item_id, Some(33));
    }

    #[test]
    fn timestamp_fields_none() {
        assert!(OrderItem::timestamp_fields().is_none());
    }

    #[test]
    fn columns_count() {
        assert_eq!(OrderItem::columns().len(), 8);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = OrderItem::fillable();
        assert!(!f.contains(&"item_id"));
        assert!(f.contains(&"good_name"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(OrderItem::guarded(), vec!["item_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let i = sample();
        assert_eq!(i.get_column_value("item_id"), Some(Value::I64(1)));
        assert_eq!(i.get_column_value("order_id"), Some(Value::I64(10)));
        assert_eq!(
            i.get_column_value("good_name"),
            Some(Value::String("白菜".into()))
        );
        assert_eq!(i.get_column_value("price_fen"), Some(Value::I64(250)));
        assert_eq!(i.get_column_value("quantity"), Some(Value::I32(5)));
        assert_eq!(i.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_pk() {
        let i = OrderItem {
            item_id: None,
            ..sample()
        };
        assert_eq!(i.get_column_value("item_id"), None);
    }

    #[test]
    fn from_value_populates_all() {
        let mut i = OrderItem {
            item_id: None,
            order_id: 0,
            good_id: 0,
            good_name: String::new(),
            price_fen: 0,
            weight_g: 0,
            total_fen: 0,
            quantity: 0,
        };
        let mut m = HashMap::new();
        m.insert("item_id".into(), Value::I64(7));
        m.insert("order_id".into(), Value::I64(20));
        m.insert("good_id".into(), Value::I64(3));
        m.insert("good_name".into(), Value::String("萝卜".into()));
        m.insert("price_fen".into(), Value::I64(100));
        m.insert("weight_g".into(), Value::I64(200));
        m.insert("total_fen".into(), Value::I64(2000));
        m.insert("quantity".into(), Value::I64(20));
        i.from_value(m);
        assert_eq!(i.item_id, Some(7));
        assert_eq!(i.order_id, 20);
        assert_eq!(i.good_name, "萝卜");
        assert_eq!(i.quantity, 20);
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut i = sample();
        i.from_value(HashMap::new());
        assert_eq!(i.item_id, Some(1));
    }

    #[test]
    fn relation_loader_returns_none() {
        let i = sample();
        assert!(i.get_relation("any").is_none());
        assert_eq!(i.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut i = sample();
        i.set_relation_data("x", Value::Null);
        assert!(i.get_relation("x").is_none());
    }
}
