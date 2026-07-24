use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 订单模型实体（对齐 PHP Order 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    /// 订单主键 ID
    pub order_id: Option<i64>,
    /// 订单号
    pub order_no: String,
    /// 商户 ID
    pub merchant_id: i64,
    /// 设备 ID
    pub device_id: i64,
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
            "device_id" => Some(Value::I64(self.device_id)),
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
            self.device_id = v;
        }
        if let Some(v) = map.get("total_fen").and_then(|v| v.as_i64()) {
            self.total_fen = v;
        }
        if let Some(v) = map.get("total_weight_g").and_then(|v| v.as_i64()) {
            self.total_weight_g = v;
        }
        if let Some(v) = map.get("item_count").and_then(|v| v.as_i64()) {
            self.item_count = v as i32;
        }
        if let Some(v) = map.get("status").and_then(|v| v.as_i64()) {
            self.status = v as i8;
        }
        if let Some(v) = map.get("pay_method").and_then(|v| v.as_i64()) {
            self.pay_method = v as i8;
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
