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
            self.order_count = v as i32;
        }
        if let Some(v) = map.get("fee_fen").and_then(|v| v.as_i64()) {
            self.fee_fen = v;
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
    use std::collections::HashMap;

    fn sample() -> Settlement {
        Settlement {
            settle_id: Some(1),
            merchant_id: 7,
            settle_date: "2026-01-01".into(),
            total_fen: 100000,
            order_count: 10,
            fee_fen: 500,
            status: 1,
            created_at: Some("2026-01-01".into()),
            updated_at: Some("2026-01-02".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(Settlement::table_name(), "settlement");
        assert_eq!(Settlement::pk_name(), "settle_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut s = sample();
        assert_eq!(s.pk(), 1);
        s.settle_id = None;
        assert_eq!(s.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut s = sample();
        s.set_pk(55);
        assert_eq!(s.settle_id, Some(55));
    }

    #[test]
    fn timestamp_fields_present() {
        let tf = Settlement::timestamp_fields().expect("应有时间戳");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, Some("updated_at"));
    }

    #[test]
    fn columns_count() {
        assert_eq!(Settlement::columns().len(), 9);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = Settlement::fillable();
        assert!(!f.contains(&"settle_id"));
        assert!(f.contains(&"settle_date"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(Settlement::guarded(), vec!["settle_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let s = sample();
        assert_eq!(s.get_column_value("settle_id"), Some(Value::I64(1)));
        assert_eq!(s.get_column_value("merchant_id"), Some(Value::I64(7)));
        assert_eq!(
            s.get_column_value("settle_date"),
            Some(Value::String("2026-01-01".into()))
        );
        assert_eq!(s.get_column_value("total_fen"), Some(Value::I64(100000)));
        assert_eq!(s.get_column_value("order_count"), Some(Value::I32(10)));
        assert_eq!(s.get_column_value("fee_fen"), Some(Value::I64(500)));
        assert_eq!(s.get_column_value("status"), Some(Value::I32(1)));
        assert_eq!(s.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let s = Settlement {
            settle_id: None,
            created_at: None,
            updated_at: None,
            ..sample()
        };
        assert_eq!(s.get_column_value("settle_id"), None);
        assert_eq!(s.get_column_value("created_at"), None);
    }

    #[test]
    fn from_value_populates_all() {
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
        let mut m = HashMap::new();
        m.insert("settle_id".into(), Value::I64(9));
        m.insert("merchant_id".into(), Value::I64(2));
        m.insert("settle_date".into(), Value::String("2026-02-01".into()));
        m.insert("total_fen".into(), Value::I64(50000));
        m.insert("order_count".into(), Value::I64(5));
        m.insert("fee_fen".into(), Value::I64(250));
        m.insert("status".into(), Value::I64(0));
        m.insert("created_at".into(), Value::String("2026-02-01".into()));
        m.insert("updated_at".into(), Value::String("2026-02-02".into()));
        s.from_value(m);
        assert_eq!(s.settle_id, Some(9));
        assert_eq!(s.settle_date, "2026-02-01");
        assert_eq!(s.order_count, 5);
        assert_eq!(s.status, 0);
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut s = sample();
        s.from_value(HashMap::new());
        assert_eq!(s.settle_id, Some(1));
    }

    #[test]
    fn relation_loader_returns_none() {
        let s = sample();
        assert!(s.get_relation("any").is_none());
        assert_eq!(s.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut s = sample();
        s.set_relation_data("x", Value::Null);
        assert!(s.get_relation("x").is_none());
    }
}
