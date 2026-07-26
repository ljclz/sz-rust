use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 商户模型实体（对齐 PHP Merchant 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Merchant {
    /// 商户主键 ID
    pub merchant_id: Option<i64>,
    /// 所属市场 ID
    pub market_id: i64,
    /// 商户名称
    pub name: String,
    /// 摊位号
    pub stall_no: String,
    /// 联系电话
    pub contact_phone: String,
    /// 经营品类
    pub category: String,
    /// 状态（0=禁用，1=启用）
    pub status: i8,
    /// 银行账号
    pub bank_account: String,
    /// 开户行名称
    pub bank_name: String,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for Merchant {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "merchant"
    }

    fn pk_name() -> &'static str {
        "merchant_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.merchant_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.merchant_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for Merchant {
    fn columns() -> Vec<&'static str> {
        vec![
            "merchant_id",
            "market_id",
            "name",
            "stall_no",
            "contact_phone",
            "category",
            "status",
            "bank_account",
            "bank_name",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "market_id",
            "name",
            "stall_no",
            "contact_phone",
            "category",
            "status",
            "bank_account",
            "bank_name",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["merchant_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "merchant_id" => self.merchant_id.map(Value::I64),
            "market_id" => Some(Value::I64(self.market_id)),
            "name" => Some(Value::String(self.name.clone())),
            "stall_no" => Some(Value::String(self.stall_no.clone())),
            "contact_phone" => Some(Value::String(self.contact_phone.clone())),
            "category" => Some(Value::String(self.category.clone())),
            "status" => Some(Value::I32(self.status as i32)),
            "bank_account" => Some(Value::String(self.bank_account.clone())),
            "bank_name" => Some(Value::String(self.bank_name.clone())),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("merchant_id").and_then(|v| v.as_i64()) {
            self.merchant_id = Some(v);
        }
        if let Some(v) = map.get("market_id").and_then(|v| v.as_i64()) {
            self.market_id = v;
        }
        if let Some(v) = map.get("name").and_then(|v| v.as_str()) {
            self.name = v.to_string();
        }
        if let Some(v) = map.get("stall_no").and_then(|v| v.as_str()) {
            self.stall_no = v.to_string();
        }
        if let Some(v) = map.get("contact_phone").and_then(|v| v.as_str()) {
            self.contact_phone = v.to_string();
        }
        if let Some(v) = map.get("category").and_then(|v| v.as_str()) {
            self.category = v.to_string();
        }
        if let Some(v) = map.get("status").and_then(|v| v.as_i64()) {
            self.status = v as i8;
        }
        if let Some(v) = map.get("bank_account").and_then(|v| v.as_str()) {
            self.bank_account = v.to_string();
        }
        if let Some(v) = map.get("bank_name").and_then(|v| v.as_str()) {
            self.bank_name = v.to_string();
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
        if let Some(v) = map.get("updated_at").and_then(|v| v.as_str()) {
            self.updated_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for Merchant {
    fn get_relation(&self, _name: &str) -> Option<&Value> {
        None
    }

    fn set_relation_data(&mut self, _name: &str, _data: Value) {}

    fn get_relation_fk_value(&self, _fk_name: &str) -> String {
        String::new()
    }
}
