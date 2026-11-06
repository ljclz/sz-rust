// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_preserves_all_fields() {
        let m = Merchant {
            merchant_id: Some(7),
            market_id: 3,
            name: "鲜视达旗舰店".into(),
            stall_no: "A-12".into(),
            contact_phone: "13900000000".into(),
            category: "水产".into(),
            status: 1,
            bank_account: "6222000000000000000".into(),
            bank_name: "工商银行".into(),
            created_at: Some("2026-02-02 00:00:00".into()),
            updated_at: None,
        };
        let json = serde_json::to_string(&m).expect("serialize");
        let back: Merchant = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.merchant_id, Some(7));
        assert_eq!(back.market_id, 3);
        assert_eq!(back.name, "鲜视达旗舰店");
        assert_eq!(back.stall_no, "A-12");
        assert_eq!(back.contact_phone, "13900000000");
        assert_eq!(back.category, "水产");
        assert_eq!(back.status, 1);
        assert_eq!(back.bank_account, "6222000000000000000");
        assert_eq!(back.bank_name, "工商银行");
        assert_eq!(back.created_at.as_deref(), Some("2026-02-02 00:00:00"));
        assert_eq!(back.updated_at, None);
    }

    #[test]
    fn model_trait_metadata_and_pk_defaults() {
        assert_eq!(Merchant::table_name(), "merchant");
        assert_eq!(Merchant::pk_name(), "merchant_id");
        let mut m = Merchant {
            merchant_id: None,
            market_id: 0,
            name: String::new(),
            stall_no: String::new(),
            contact_phone: String::new(),
            category: String::new(),
            status: 0,
            bank_account: String::new(),
            bank_name: String::new(),
            created_at: None,
            updated_at: None,
        };
        assert_eq!(m.pk(), 0);
        m.set_pk(123);
        assert_eq!(m.pk(), 123);
        assert_eq!(Merchant::columns().len(), 11);
        assert_eq!(Merchant::fillable().len(), 8);
        assert_eq!(Merchant::guarded().len(), 1);
        assert!(m.get_column_value("merchant_id").is_some());
        assert!(m.get_column_value("market_id").is_some());
        assert!(m.get_column_value("bank_account").is_some());
        assert!(m.get_column_value("nope").is_none());
        assert!(Merchant::timestamp_fields().is_some());
    }

    #[test]
    fn from_value_populates_all_fields_then_empty_keeps() {
        use std::collections::HashMap;
        let mut m = Merchant {
            merchant_id: None,
            market_id: 0,
            name: String::new(),
            stall_no: String::new(),
            contact_phone: String::new(),
            category: String::new(),
            status: 0,
            bank_account: String::new(),
            bank_name: String::new(),
            created_at: None,
            updated_at: None,
        };
        let mut map = HashMap::new();
        map.insert("merchant_id".to_string(), Value::I64(7));
        map.insert("market_id".to_string(), Value::I64(3));
        map.insert("name".to_string(), Value::String("店".into()));
        map.insert("stall_no".to_string(), Value::String("A-1".into()));
        map.insert(
            "contact_phone".to_string(),
            Value::String("13800000000".into()),
        );
        map.insert("category".to_string(), Value::String("水产".into()));
        map.insert("status".to_string(), Value::I64(1));
        map.insert("bank_account".to_string(), Value::String("6222".into()));
        map.insert("bank_name".to_string(), Value::String("工行".into()));
        map.insert("created_at".to_string(), Value::String("c".into()));
        map.insert("updated_at".to_string(), Value::String("u".into()));
        m.from_value(map);
        assert_eq!(m.merchant_id, Some(7));
        assert_eq!(m.market_id, 3);
        assert_eq!(m.name, "店");
        assert_eq!(m.stall_no, "A-1");
        assert_eq!(m.contact_phone, "13800000000");
        assert_eq!(m.category, "水产");
        assert_eq!(m.status, 1);
        assert_eq!(m.bank_account, "6222");
        assert_eq!(m.bank_name, "工行");
        assert_eq!(m.created_at.as_deref(), Some("c"));
        assert_eq!(m.updated_at.as_deref(), Some("u"));
        // 空 map 不改变已有值（覆盖所有 if-let None 分支）
        m.from_value(HashMap::new());
        assert_eq!(m.merchant_id, Some(7));
        assert_eq!(m.market_id, 3);
        assert_eq!(m.name, "店");
    }
}
