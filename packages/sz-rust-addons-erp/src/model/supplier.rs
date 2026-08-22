//! Supplier（供应商）模型
//!
//! 对应数据库表 `erp_suppliers`。

use serde::{Deserialize, Serialize};
use sz_rust_core::orm::repository::EntityAttributes;
use sz_rust_core::orm::Value;

/// 供应商实体
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Supplier {
    /// 供应商 ID
    pub id: i64,
    /// 供应商名称
    pub name: String,
    /// 联系人
    pub contact: String,
    /// 联系电话
    pub phone: String,
    /// 电子邮箱
    pub email: String,
    /// 地址
    pub address: String,
    /// 信用等级（1-5）
    pub credit_level: u8,
    /// 备注
    pub remark: String,
    /// 创建时间（Unix 时间戳）
    pub created_at: i64,
    /// 更新时间（Unix 时间戳）
    pub updated_at: i64,
}

impl EntityAttributes for Supplier {
    fn get_attribute(&self, field: &str) -> Option<Value> {
        match field {
            "id" => Some(Value::I64(self.id)),
            "name" => Some(Value::String(self.name.clone())),
            "contact" => Some(Value::String(self.contact.clone())),
            "phone" => Some(Value::String(self.phone.clone())),
            "email" => Some(Value::String(self.email.clone())),
            "address" => Some(Value::String(self.address.clone())),
            "credit_level" => Some(Value::U8(self.credit_level)),
            "remark" => Some(Value::String(self.remark.clone())),
            "created_at" => Some(Value::I64(self.created_at)),
            "updated_at" => Some(Value::I64(self.updated_at)),
            _ => None,
        }
    }
}
