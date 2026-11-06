// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 结算模型实体（对齐 PHP Settlement 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settlement {
    /// 结算主键 ID
    pub settle_id: Option<i64>,
    /// 商户 ID
    pub merchant_id: i64,
    /// 结算日期（格式：YYYY-MM-DD）
    pub settle_date: String,
    /// 结算总金额（单位：分）
    pub total_fen: i64,
    /// 订单数量
    pub order_count: i32,
    /// 手续费（单位：分）
    pub fee_fen: i64,
    /// 状态（0=待结算，1=已结算，2=已失败）
    pub status: i8,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for Settlement {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "settlement"
    }

    fn pk_name() -> &'static str {
        "settle_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.settle_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.settle_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for Settlement {
    fn columns() -> Vec<&'static str> {
        vec![
            "settle_id",
            "merchant_id",
            "settle_date",
            "total_fen",
            "order_count",
            "fee_fen",
            "status",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "merchant_id",
            "settle_date",
            "total_fen",
            "order_count",
            "fee_fen",
            "status",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["settle_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "settle_id" => self.settle_id.map(Value::I64),
            "merchant_id" => Some(Value::I64(self.merchant_id)),
            "settle_date" => Some(Value::String(self.settle_date.clone())),
            "total_fen" => Some(Value::I64(self.total_fen)),
            "order_count" => Some(Value::I32(self.order_count)),
            "fee_fen" => Some(Value::I64(self.fee_fen)),
            "status" => Some(Value::I32(self.status as i32)),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("settle_id").and_then(|v| v.as_i64()) {
            self.settle_id = Some(v);
        }
        if let Some(v) = map.get("merchant_id").and_then(|v| v.as_i64()) {
            self.merchant_id = v;
        }
        if let Some(v) = map.get("settle_date").and_then(|v| v.as_str()) {
            self.settle_date = v.to_string();
        }
        if let Some(v) = map.get("total_fen").and_then(|v| v.as_i64()) {
            self.total_fen = v;
        }
        if let Some(v) = map.get("order_count").and_then(|v| v.as_i64()) {
            self.order_count = v.clamp(0, i32::MAX as i64) as i32;
        }
        if let Some(v) = map.get("fee_fen").and_then(|v| v.as_i64()) {
            self.fee_fen = v;
        }
        if let Some(v) = map.get("status").and_then(|v| v.as_i64()) {
            self.status = v.clamp(i8::MIN as i64, i8::MAX as i64) as i8;
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
        if let Some(v) = map.get("updated_at").and_then(|v| v.as_str()) {
            self.updated_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for Settlement {
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
        let s = Settlement {
            settle_id: Some(2026),
            merchant_id: 5,
            settle_date: "2026-06-06".into(),
            total_fen: 999999,
            order_count: 42,
            fee_fen: 5000,
            status: 1,
            created_at: Some("2026-06-07 00:00:00".into()),
            updated_at: None,
        };
        let json = serde_json::to_string(&s).expect("serialize");
        let back: Settlement = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.settle_id, Some(2026));
        assert_eq!(back.merchant_id, 5);
        assert_eq!(back.settle_date, "2026-06-06");
        assert_eq!(back.total_fen, 999999);
        assert_eq!(back.order_count, 42);
        assert_eq!(back.fee_fen, 5000);
        assert_eq!(back.status, 1);
        assert_eq!(back.created_at.as_deref(), Some("2026-06-07 00:00:00"));
        assert_eq!(back.updated_at, None);
    }

    #[test]
    fn model_trait_metadata_and_pk_defaults() {
        assert_eq!(Settlement::table_name(), "settlement");
        assert_eq!(Settlement::pk_name(), "settle_id");
        let mut s = Settlement {
            settle_id: None,
            merchant_id: 0,
            settle_date: String::new(),
            total_fen: 0,
            order_count: 0,
            fee_fen: 0,
            status: 0,
            created_at: None,
            updated_at: None,
        };
        assert_eq!(s.pk(), 0);
        assert!(s.get_column_value("settle_id").is_none());
        s.set_pk(31);
        assert_eq!(s.pk(), 31);
        assert!(s.get_column_value("settle_id").is_some());
        assert_eq!(Settlement::columns().len(), 9);
        assert_eq!(Settlement::fillable().len(), 6);
        assert_eq!(Settlement::guarded().len(), 1);
        assert!(s.get_column_value("merchant_id").is_some());
        assert!(s.get_column_value("fee_fen").is_some());
        assert!(s.get_column_value("xxx").is_none());
        assert!(Settlement::timestamp_fields().is_some());
    }

    #[test]
    fn from_value_populates_all_fields_then_empty_keeps() {
        use std::collections::HashMap;
        let mut s = Settlement {
            settle_id: None,
            merchant_id: 0,
            settle_date: String::new(),
            total_fen: 0,
            order_count: 0,
            fee_fen: 0,
            status: 0,
            created_at: None,
            updated_at: None,
        };
        let mut map = HashMap::new();
        map.insert("settle_id".to_string(), Value::I64(2026));
        map.insert("merchant_id".to_string(), Value::I64(5));
        map.insert(
            "settle_date".to_string(),
            Value::String("2026-06-06".into()),
        );
        map.insert("total_fen".to_string(), Value::I64(999999));
        map.insert("order_count".to_string(), Value::I64(42));
        map.insert("fee_fen".to_string(), Value::I64(5000));
        map.insert("status".to_string(), Value::I64(1));
        map.insert("created_at".to_string(), Value::String("c".into()));
        map.insert("updated_at".to_string(), Value::String("u".into()));
        s.from_value(map);
        assert_eq!(s.settle_id, Some(2026));
        assert_eq!(s.merchant_id, 5);
        assert_eq!(s.settle_date, "2026-06-06");
        assert_eq!(s.total_fen, 999999);
        assert_eq!(s.order_count, 42);
        assert_eq!(s.fee_fen, 5000);
        assert_eq!(s.status, 1);
        assert_eq!(s.created_at.as_deref(), Some("c"));
        assert_eq!(s.updated_at.as_deref(), Some("u"));
        s.from_value(HashMap::new());
        assert_eq!(s.settle_id, Some(2026));
        assert_eq!(s.merchant_id, 5);
        assert_eq!(s.settle_date, "2026-06-06");
    }
}
