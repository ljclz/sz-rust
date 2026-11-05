// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
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
    /// 商品 ID（NULL 表示非商品行，如运费行/手工行；015_nullable_order_item_good.sql 后改 NULLABLE）
    pub good_id: Option<i64>,
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
            "good_id" => self.good_id.map(Value::I64),
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
            self.good_id = Some(v);
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

    #[test]
    fn serde_roundtrip_preserves_nullable_good_id() {
        // good_id = Some（商品行）与 None（运费行/手工行）两种情况
        let item = OrderItem {
            item_id: Some(11),
            order_id: 100,
            good_id: Some(202),
            good_name: "鲈鱼".into(),
            price_fen: 3000,
            weight_g: 250,
            total_fen: 750,
            quantity: 1,
        };
        let json = serde_json::to_string(&item).expect("serialize");
        let back: OrderItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.item_id, Some(11));
        assert_eq!(back.order_id, 100);
        assert_eq!(back.good_id, Some(202));
        assert_eq!(back.good_name, "鲈鱼");
        assert_eq!(back.price_fen, 3000);
        assert_eq!(back.weight_g, 250);
        assert_eq!(back.total_fen, 750);
        assert_eq!(back.quantity, 1);

        let fee_row = OrderItem {
            item_id: None,
            order_id: 100,
            good_id: None,
            good_name: "运费".into(),
            price_fen: 500,
            weight_g: 0,
            total_fen: 500,
            quantity: 1,
        };
        let json2 = serde_json::to_string(&fee_row).expect("serialize");
        let back2: OrderItem = serde_json::from_str(&json2).expect("deserialize");
        assert_eq!(back2.good_id, None);
        assert_eq!(back2.good_name, "运费");
    }

    #[test]
    fn model_trait_metadata_and_pk_defaults() {
        assert_eq!(OrderItem::table_name(), "order_item");
        assert_eq!(OrderItem::pk_name(), "item_id");
        let mut it = OrderItem {
            item_id: None,
            order_id: 0,
            good_id: None,
            good_name: String::new(),
            price_fen: 0,
            weight_g: 0,
            total_fen: 0,
            quantity: 0,
        };
        assert_eq!(it.pk(), 0);
        assert!(it.get_column_value("item_id").is_none());
        it.set_pk(33);
        assert_eq!(it.pk(), 33);
        assert!(it.get_column_value("item_id").is_some());
        assert_eq!(OrderItem::columns().len(), 8);
        assert_eq!(OrderItem::fillable().len(), 7);
        assert_eq!(OrderItem::guarded().len(), 1);
        assert!(it.get_column_value("order_id").is_some());
        assert!(it.get_column_value("good_id").is_none());
        assert!(it.get_column_value("qqq").is_none());
        // OrderItem 无 timestamp_fields
        assert!(OrderItem::timestamp_fields().is_none());
    }
}
