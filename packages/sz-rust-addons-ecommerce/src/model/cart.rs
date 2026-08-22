//! Cart（购物车）模型

use serde::{Deserialize, Serialize};
use sz_rust_core::orm::repository::EntityAttributes;
use sz_rust_core::orm::Value;

/// 购物车项实体
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CartItem {
    pub id: i64,
    /// 用户 ID
    pub user_id: i64,
    /// 商品 ID
    pub product_id: i64,
    /// 数量
    pub quantity: i64,
    /// 选中状态
    pub selected: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl EntityAttributes for CartItem {
    fn get_attribute(&self, field: &str) -> Option<Value> {
        match field {
            "id" => Some(Value::I64(self.id)),
            "user_id" => Some(Value::I64(self.user_id)),
            "product_id" => Some(Value::I64(self.product_id)),
            "quantity" => Some(Value::I64(self.quantity)),
            "selected" => Some(Value::Bool(self.selected)),
            "created_at" => Some(Value::I64(self.created_at)),
            "updated_at" => Some(Value::I64(self.updated_at)),
            _ => None,
        }
    }
}
