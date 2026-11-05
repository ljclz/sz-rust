// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 订单模型实体（对齐 PHP Order 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    /// 订单主键 ID
    pub order_id: Option<i64>,
    /// 订单号
    pub order_no: String,
    /// 商户 ID
    pub merchant_id: i64,
    /// 设备 ID（NULL 表示未关联设备，如后台手工单；014_nullable_order_device.sql 后改 NULLABLE）
    pub device_id: Option<i64>,
    /// 订单总金额（单位：分）
    pub total_fen: i64,
    /// 订单总重量（单位：克）
    pub total_weight_g: i64,
    /// 订单项数量
    pub item_count: i32,
    /// 订单状态（0=已取消，1=待支付，2=已支付，3=已退款）
    pub status: i8,
    /// 支付方式（0=未支付，1=微信，2=支付宝，3=现金）
    pub pay_method: i8,
    /// 支付时间
    pub pay_at: Option<String>,
    /// 离线序列号（用于离线订单去重）
    pub offline_seq: String,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for Order {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "order"
    }

    fn pk_name() -> &'static str {
        "order_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.order_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.order_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for Order {
    fn columns() -> Vec<&'static str> {
        vec![
            "order_id",
            "order_no",
            "merchant_id",
            "device_id",
            "total_fen",
            "total_weight_g",
            "item_count",
            "status",
            "pay_method",
            "pay_at",
            "offline_seq",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "order_no",
            "merchant_id",
            "device_id",
            "total_fen",
            "total_weight_g",
            "item_count",
            "status",
            "pay_method",
            "pay_at",
            "offline_seq",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["order_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "order_id" => self.order_id.map(Value::I64),
            "order_no" => Some(Value::String(self.order_no.clone())),
            "merchant_id" => Some(Value::I64(self.merchant_id)),
            "device_id" => self.device_id.map(Value::I64),
            "total_fen" => Some(Value::I64(self.total_fen)),
            "total_weight_g" => Some(Value::I64(self.total_weight_g)),
            "item_count" => Some(Value::I32(self.item_count)),
            "status" => Some(Value::I32(self.status as i32)),
            "pay_method" => Some(Value::I32(self.pay_method as i32)),
            "pay_at" => self.pay_at.as_ref().map(|s| Value::String(s.clone())),
            "offline_seq" => Some(Value::String(self.offline_seq.clone())),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("order_id").and_then(|v| v.as_i64()) {
            self.order_id = Some(v);
        }
        if let Some(v) = map.get("order_no").and_then(|v| v.as_str()) {
            self.order_no = v.to_string();
        }
        if let Some(v) = map.get("merchant_id").and_then(|v| v.as_i64()) {
            self.merchant_id = v;
        }
        if let Some(v) = map.get("device_id").and_then(|v| v.as_i64()) {
            self.device_id = Some(v);
        }
        if let Some(v) = map.get("total_fen").and_then(|v| v.as_i64()) {
            self.total_fen = v;
        }
        if let Some(v) = map.get("total_weight_g").and_then(|v| v.as_i64()) {
            self.total_weight_g = v;
        }
        if let Some(v) = map.get("item_count").and_then(|v| v.as_i64()) {
            self.item_count = v.clamp(0, i32::MAX as i64) as i32;
        }
        if let Some(v) = map.get("status").and_then(|v| v.as_i64()) {
            self.status = v.clamp(i8::MIN as i64, i8::MAX as i64) as i8;
        }
        if let Some(v) = map.get("pay_method").and_then(|v| v.as_i64()) {
            self.pay_method = v.clamp(i8::MIN as i64, i8::MAX as i64) as i8;
        }
        if let Some(v) = map.get("pay_at").and_then(|v| v.as_str()) {
            self.pay_at = Some(v.to_string());
        }
        if let Some(v) = map.get("offline_seq").and_then(|v| v.as_str()) {
            self.offline_seq = v.to_string();
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
        if let Some(v) = map.get("updated_at").and_then(|v| v.as_str()) {
            self.updated_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for Order {
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
    fn serde_roundtrip_preserves_nullable_device_id() {
        // device_id = Some（设备下单）与 None（后台手工单）两种情况
        let device_order = Order {
            order_id: Some(100),
            order_no: "ORD20260303001".into(),
            merchant_id: 5,
            device_id: Some(8),
            total_fen: 12345,
            total_weight_g: 500,
            item_count: 2,
            status: 2,
            pay_method: 1,
            pay_at: Some("2026-03-03 12:00:00".into()),
            offline_seq: String::new(),
            created_at: Some("2026-03-03 11:59:59".into()),
            updated_at: None,
        };
        let json = serde_json::to_string(&device_order).expect("serialize");
        let back: Order = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.order_id, Some(100));
        assert_eq!(back.order_no, "ORD20260303001");
        assert_eq!(back.merchant_id, 5);
        assert_eq!(back.device_id, Some(8));
        assert_eq!(back.total_fen, 12345);
        assert_eq!(back.total_weight_g, 500);
        assert_eq!(back.item_count, 2);
        assert_eq!(back.status, 2);
        assert_eq!(back.pay_method, 1);
        assert_eq!(back.pay_at.as_deref(), Some("2026-03-03 12:00:00"));
        assert_eq!(back.created_at.as_deref(), Some("2026-03-03 11:59:59"));
        assert_eq!(back.updated_at, None);

        let manual_order = Order {
            order_id: None,
            order_no: "MANUAL-1".into(),
            merchant_id: 1,
            device_id: None,
            total_fen: 0,
            total_weight_g: 0,
            item_count: 0,
            status: 1,
            pay_method: 0,
            pay_at: None,
            offline_seq: "OFFLINE-1".into(),
            created_at: None,
            updated_at: None,
        };
        let json2 = serde_json::to_string(&manual_order).expect("serialize");
        let back2: Order = serde_json::from_str(&json2).expect("deserialize");
        assert_eq!(back2.device_id, None);
        assert_eq!(back2.offline_seq, "OFFLINE-1");
    }

    #[test]
    fn model_trait_metadata_and_pk_defaults() {
        assert_eq!(Order::table_name(), "order");
        assert_eq!(Order::pk_name(), "order_id");
        let mut o = Order {
            order_id: None,
            order_no: String::new(),
            merchant_id: 0,
            device_id: None,
            total_fen: 0,
            total_weight_g: 0,
            item_count: 0,
            status: 0,
            pay_method: 0,
            pay_at: None,
            offline_seq: String::new(),
            created_at: None,
            updated_at: None,
        };
        assert_eq!(o.pk(), 0);
        assert!(o.get_column_value("order_id").is_none());
        o.set_pk(2024);
        assert_eq!(o.pk(), 2024);
        assert!(o.get_column_value("order_id").is_some());
        assert_eq!(Order::columns().len(), 13);
        assert_eq!(Order::fillable().len(), 10);
        assert_eq!(Order::guarded().len(), 1);
        assert!(o.get_column_value("order_no").is_some());
        assert!(o.get_column_value("device_id").is_none());
        assert!(o.get_column_value("xyz").is_none());
        assert!(Order::timestamp_fields().is_some());
    }
}
