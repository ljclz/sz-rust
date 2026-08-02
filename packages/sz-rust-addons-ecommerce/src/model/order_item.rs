//! OrderItem（订单项）模型

use serde::{Deserialize, Serialize};
use sz_rust_core::orm::repository::EntityAttributes;
use sz_rust_core::orm::Value;

/// 订单项实体
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OrderItem {
    pub id: i64,
    /// 订单 ID
    pub order_id: i64,
    /// 商品 ID
    pub product_id: i64,
    /// 商品名称（快照）
    pub product_name: String,
    /// 单价
    pub unit_price: f64,
    /// 数量
    pub quantity: i64,
    /// 小计
    pub subtotal: f64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl EntityAttributes for OrderItem {
    fn get_attribute(&self, field: &str) -> Option<Value> {
        match field {
            "id" => Some(Value::I64(self.id)),
            "order_id" => Some(Value::I64(self.order_id)),
            "product_id" => Some(Value::I64(self.product_id)),
            "product_name" => Some(Value::String(self.product_name.clone())),
            "unit_price" => Some(Value::F64(self.unit_price)),
            "quantity" => Some(Value::I64(self.quantity)),
            "subtotal" => Some(Value::F64(self.subtotal)),
            "created_at" => Some(Value::I64(self.created_at)),
            "updated_at" => Some(Value::I64(self.updated_at)),
            _ => None,
        }
    }
}
