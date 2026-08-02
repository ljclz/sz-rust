//! Contact（联系人）模型
//!
//! 对应数据库表 `crm_contact`。
//!
//! 此模型为纯数据载体（PODO），不依赖 sz-orm Model trait 体系。
//! 通过 `InMemoryRepository<Contact>` 或自定义 SQL Repository 进行持久化。

use serde::{Deserialize, Serialize};

/// 联系人
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Contact {
    /// 主键
    pub id: i64,
    /// 姓名
    pub name: String,
    /// 手机号
    #[serde(default)]
    pub phone: String,
    /// 邮箱
    #[serde(default)]
    pub email: String,
    /// 所属客户 ID
    #[serde(default)]
    pub customer_id: i64,
    /// 职位
    #[serde(default)]
    pub position: String,
    /// 备注
    #[serde(default)]
    pub remark: String,
    /// 创建时间戳（毫秒）
    #[serde(default)]
    pub created_at: i64,
    /// 更新时间戳（毫秒）
    #[serde(default)]
    pub updated_at: i64,
}

impl Contact {
    /// 创建空联系人
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for Contact {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            phone: String::new(),
            email: String::new(),
            customer_id: 0,
            position: String::new(),
            remark: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Contact {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "name" => Some(V::String(self.name.clone())),
            "phone" => Some(V::String(self.phone.clone())),
            "email" => Some(V::String(self.email.clone())),
            "customer_id" => Some(V::I64(self.customer_id)),
            "position" => Some(V::String(self.position.clone())),
            "remark" => Some(V::String(self.remark.clone())),
            "created_at" => Some(V::I64(self.created_at)),
            "updated_at" => Some(V::I64(self.updated_at)),
            _ => None,
        }
    }
}
