//! Deal（商机）模型
//!
//! 对应数据库表 `crm_deal`。纯数据载体，不依赖 Model trait 体系。

use serde::{Deserialize, Serialize};

/// 商机
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Deal {
    pub id: i64,
    pub name: String,
    /// 商机阶段
    #[serde(default)]
    pub stage: String,
    /// 预估金额
    #[serde(default)]
    pub amount: f64,
    /// 关联联系人 ID
    #[serde(default)]
    pub contact_id: i64,
    /// 关联线索 ID
    #[serde(default)]
    pub lead_id: i64,
    /// 负责人 ID
    #[serde(default)]
    pub owner_id: i64,
    /// 备注
    #[serde(default)]
    pub remark: String,
    /// 赢单概率（0-100）
    #[serde(default)]
    pub probability: u8,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

impl Deal {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for Deal {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            stage: "initial".to_string(),
            amount: 0.0,
            contact_id: 0,
            lead_id: 0,
            owner_id: 0,
            remark: String::new(),
            probability: 0,
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl sz_rust_core::orm::repository::EntityAttributes for Deal {
    fn get_attribute(&self, field: &str) -> Option<sz_rust_core::orm::Value> {
        use sz_rust_core::orm::Value as V;
        match field {
            "id" => Some(V::I64(self.id)),
            "name" => Some(V::String(self.name.clone())),
            "stage" => Some(V::String(self.stage.clone())),
            "amount" => Some(V::F64(self.amount)),
            "contact_id" => Some(V::I64(self.contact_id)),
            "lead_id" => Some(V::I64(self.lead_id)),
            "owner_id" => Some(V::I64(self.owner_id)),
            "remark" => Some(V::String(self.remark.clone())),
            "probability" => Some(V::U8(self.probability)),
            "created_at" => Some(V::I64(self.created_at)),
            "updated_at" => Some(V::I64(self.updated_at)),
            _ => None,
        }
    }
}
