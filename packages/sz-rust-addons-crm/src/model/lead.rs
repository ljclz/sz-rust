//! Lead（线索）模型
//!
//! 对应数据库表 `crm_lead`。纯数据载体，不依赖 Model trait 体系。

use serde::{Deserialize, Serialize};

/// 线索
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Lead {
    pub id: i64,
    pub name: String,
    /// 线索来源
    #[serde(default)]
    pub source: String,
    /// 线索状态: prospect / qualified / converted / lost
    #[serde(default)]
    pub status: String,
    /// 手机号
    #[serde(default)]
    pub phone: String,
    /// 邮箱
    #[serde(default)]
    pub email: String,
    /// 公司名称
    #[serde(default)]
    pub company: String,
    /// 预估金额
    #[serde(default)]
    pub estimated_amount: f64,
    /// 负责人 ID
    #[serde(default)]
    pub owner_id: i64,
    /// 备注
    #[serde(default)]
    pub remark: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

impl Lead {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for Lead {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            source: String::new(),
            status: "prospect".to_string(),
            phone: String::new(),
            email: String::new(),
            company: String::new(),
            estimated_amount: 0.0,
            owner_id: 0,
            remark: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Lead {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "name" => Some(V::String(self.name.clone())),
            "source" => Some(V::String(self.source.clone())),
            "status" => Some(V::String(self.status.clone())),
            "phone" => Some(V::String(self.phone.clone())),
            "email" => Some(V::String(self.email.clone())),
            "company" => Some(V::String(self.company.clone())),
            "estimated_amount" => Some(V::F64(self.estimated_amount)),
            "owner_id" => Some(V::I64(self.owner_id)),
            "remark" => Some(V::String(self.remark.clone())),
            "created_at" => Some(V::I64(self.created_at)),
            "updated_at" => Some(V::I64(self.updated_at)),
            _ => None,
        }
    }
}
