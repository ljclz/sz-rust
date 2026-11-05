// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 市场模型实体（对齐 PHP Market 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    /// 市场主键 ID
    pub market_id: Option<i64>,
    /// 市场名称
    pub name: String,
    /// 市场地址
    pub address: String,
    /// 联系人
    pub contact: String,
    /// 联系电话
    pub phone: String,
    /// 状态（0=禁用，1=启用）
    pub status: i8,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for Market {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "market"
    }

    fn pk_name() -> &'static str {
        "market_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.market_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.market_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for Market {
    fn columns() -> Vec<&'static str> {
        vec![
            "market_id",
            "name",
            "address",
            "contact",
            "phone",
            "status",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec!["name", "address", "contact", "phone", "status"]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["market_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "market_id" => self.market_id.map(Value::I64),
            "name" => Some(Value::String(self.name.clone())),
            "address" => Some(Value::String(self.address.clone())),
            "contact" => Some(Value::String(self.contact.clone())),
            "phone" => Some(Value::String(self.phone.clone())),
            "status" => Some(Value::I32(self.status as i32)),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("market_id").and_then(|v| v.as_i64()) {
            self.market_id = Some(v);
        }
        if let Some(v) = map.get("name").and_then(|v| v.as_str()) {
            self.name = v.to_string();
        }
        if let Some(v) = map.get("address").and_then(|v| v.as_str()) {
            self.address = v.to_string();
        }
        if let Some(v) = map.get("contact").and_then(|v| v.as_str()) {
            self.contact = v.to_string();
        }
        if let Some(v) = map.get("phone").and_then(|v| v.as_str()) {
            self.phone = v.to_string();
        }
        if let Some(v) = map.get("status").and_then(|v| v.as_i64()) {
            self.status = v as i8;
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
        if let Some(v) = map.get("updated_at").and_then(|v| v.as_str()) {
            self.updated_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for Market {
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
        let m = Market {
            market_id: Some(42),
            name: "城中市场".into(),
            address: "南京路 1 号".into(),
            contact: "张三".into(),
            phone: "13812345678".into(),
            status: 1,
            created_at: Some("2026-01-02 03:04:05".into()),
            updated_at: None,
        };
        let json = serde_json::to_string(&m).expect("serialize");
        let back: Market = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.market_id, Some(42));
        assert_eq!(back.name, "城中市场");
        assert_eq!(back.address, "南京路 1 号");
        assert_eq!(back.contact, "张三");
        assert_eq!(back.phone, "13812345678");
        assert_eq!(back.status, 1);
        assert_eq!(back.created_at.as_deref(), Some("2026-01-02 03:04:05"));
        assert_eq!(back.updated_at, None);
    }

    #[test]
    fn model_trait_metadata_and_pk_defaults() {
        assert_eq!(Market::table_name(), "market");
        assert_eq!(Market::pk_name(), "market_id");
        let mut m = Market {
            market_id: None,
            name: String::new(),
            address: String::new(),
            contact: String::new(),
            phone: String::new(),
            status: 0,
            created_at: None,
            updated_at: None,
        };
        // pk() 在 market_id=None 时回退 0
        assert_eq!(m.pk(), 0);
        m.set_pk(99);
        assert_eq!(m.pk(), 99);
        assert_eq!(Market::columns().len(), 8);
        assert_eq!(Market::fillable().len(), 5);
        assert_eq!(Market::guarded().len(), 1);
        // get_column_value 已知列返回 Some，未知列返回 None
        assert!(m.get_column_value("market_id").is_some());
        assert!(m.get_column_value("name").is_some());
        assert!(m.get_column_value("status").is_some());
        assert!(m.get_column_value("nonexistent").is_none());
        // timestamp_fields 配置了 created_at + updated_at
        assert!(Market::timestamp_fields().is_some());
    }
}
