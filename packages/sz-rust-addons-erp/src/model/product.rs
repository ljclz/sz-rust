//! Product（商品）模型
//!
//! 对应数据库表 `erp_products`。

use serde::{Deserialize, Serialize};
use sz_rust_core::orm::repository::EntityAttributes;
use sz_rust_core::orm::Value;

/// 商品实体
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Product {
    /// 商品 ID
    pub id: i64,
    /// 商品名称
    pub name: String,
    /// 商品 SKU
    pub sku: String,
    /// 单价
    pub price: f64,
    /// 库存数量
    pub stock: i64,
    /// 供应商 ID
    pub supplier_id: i64,
    /// 分类
    pub category: String,
    /// 状态（active/inactive）
    pub status: String,
    /// 备注
    pub remark: String,
    /// 创建时间（Unix 时间戳）
    pub created_at: i64,
    /// 更新时间（Unix 时间戳）
    pub updated_at: i64,
}

impl EntityAttributes for Product {
    fn get_attribute(&self, field: &str) -> Option<Value> {
        match field {
            "id" => Some(Value::I64(self.id)),
            "name" => Some(Value::String(self.name.clone())),
            "sku" => Some(Value::String(self.sku.clone())),
            "price" => Some(Value::F64(self.price)),
            "stock" => Some(Value::I64(self.stock)),
            "supplier_id" => Some(Value::I64(self.supplier_id)),
            "category" => Some(Value::String(self.category.clone())),
            "status" => Some(Value::String(self.status.clone())),
            "remark" => Some(Value::String(self.remark.clone())),
            "created_at" => Some(Value::I64(self.created_at)),
            "updated_at" => Some(Value::I64(self.updated_at)),
            _ => None,
        }
    }
}
