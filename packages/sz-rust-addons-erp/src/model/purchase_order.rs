//! PurchaseOrder（采购单）模型
//!
//! 对应数据库表 `erp_purchase_orders`。

use serde::{Deserialize, Serialize};
use sz_rust_core::orm::repository::EntityAttributes;
use sz_rust_core::orm::Value;

/// 采购单实体
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PurchaseOrder {
    /// 采购单 ID
    pub id: i64,
    /// 供应商 ID
    pub supplier_id: i64,
    /// 商品 ID
    pub product_id: i64,
    /// 采购数量
    pub quantity: i64,
    /// 单价
    pub unit_price: f64,
    /// 总金额
    pub total_amount: f64,
    /// 状态（pending/approved/received/cancelled）
    pub status: String,
    /// 采购日期
    pub order_date: i64,
    /// 备注
    pub remark: String,
    /// 创建时间（Unix 时间戳）
    pub created_at: i64,
    /// 更新时间（Unix 时间戳）
    pub updated_at: i64,
}

impl EntityAttributes for PurchaseOrder {
    fn get_attribute(&self, field: &str) -> Option<Value> {
        match field {
            "id" => Some(Value::I64(self.id)),
            "supplier_id" => Some(Value::I64(self.supplier_id)),
            "product_id" => Some(Value::I64(self.product_id)),
            "quantity" => Some(Value::I64(self.quantity)),
            "unit_price" => Some(Value::F64(self.unit_price)),
            "total_amount" => Some(Value::F64(self.total_amount)),
            "status" => Some(Value::String(self.status.clone())),
            "order_date" => Some(Value::I64(self.order_date)),
            "remark" => Some(Value::String(self.remark.clone())),
            "created_at" => Some(Value::I64(self.created_at)),
            "updated_at" => Some(Value::I64(self.updated_at)),
            _ => None,
        }
    }
}
