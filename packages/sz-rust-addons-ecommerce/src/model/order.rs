//! Order（订单）模型

use serde::{Deserialize, Serialize};
use sz_rust_core::orm::repository::EntityAttributes;
use sz_rust_core::orm::Value;

/// 订单实体
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Order {
    pub id: i64,
    /// 订单号
    pub order_no: String,
    /// 用户 ID
    pub user_id: i64,
    /// 总金额
    pub total_amount: f64,
    /// 实付金额
    pub paid_amount: f64,
    /// 状态（pending/paid/shipped/completed/cancelled）
    pub status: String,
    /// 收货地址
    pub shipping_address: String,
    /// 备注
    pub remark: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl EntityAttributes for Order {
    fn get_attribute(&self, field: &str) -> Option<Value> {
        match field {
            "id" => Some(Value::I64(self.id)),
            "order_no" => Some(Value::String(self.order_no.clone())),
            "user_id" => Some(Value::I64(self.user_id)),
            "total_amount" => Some(Value::F64(self.total_amount)),
            "paid_amount" => Some(Value::F64(self.paid_amount)),
            "status" => Some(Value::String(self.status.clone())),
            "shipping_address" => Some(Value::String(self.shipping_address.clone())),
            "remark" => Some(Value::String(self.remark.clone())),
            "created_at" => Some(Value::I64(self.created_at)),
            "updated_at" => Some(Value::I64(self.updated_at)),
            _ => None,
        }
    }
}
