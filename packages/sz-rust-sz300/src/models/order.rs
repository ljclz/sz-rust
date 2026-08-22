use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 订单模型实体（对齐 PHP Order 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    /// 订单主键 ID
    pub order_id: Option<i64>,
    /// 订单号
    pub order_no: String,
    /// 商户 ID
    pub merchant_id: i64,
    /// 设备 ID
    pub device_id: i64,
    /// 订单总金额（单位：分）
    pub total_fen: i64,
    /// 订单总重量（单位：克）
    pub total_weight_g: i64,
    /// 订单项数量
    pub item_count: i32,
    /// 订单状态（0=已取消，1=待支付，2=已支付，3=已退款）
    pub status: i8,
    /// 支付方式（0=未支付，1=微信，2=支付宝，3=现金）
    pub pay_method: i8,
    /// 支付时间
    pub pay_at: Option<String>,
    /// 离线序列号（用于离线订单去重）
    pub offline_seq: String,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for Order {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "order"
    }

    fn pk_name() -> &'static str {
        "order_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.order_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.order_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for Order {
    fn columns() -> Vec<&'static str> {
        vec![
            "order_id",
            "order_no",
            "merchant_id",
            "device_id",
            "total_fen",
            "total_weight_g",
            "item_count",
            "status",
            "pay_method",
            "pay_at",
            "offline_seq",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "order_no",
            "merchant_id",
            "device_id",
            "total_fen",
            "total_weight_g",
            "item_count",
            "status",
            "pay_method",
            "pay_at",
            "offline_seq",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["order_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "order_id" => self.order_id.map(Value::I64),
            "order_no" => Some(Value::String(self.order_no.clone())),
            "merchant_id" => Some(Value::I64(self.merchant_id)),
            "device_id" => Some(Value::I64(self.device_id)),
            "total_fen" => Some(Value::I64(self.total_fen)),
            "total_weight_g" => Some(Value::I64(self.total_weight_g)),
            "item_count" => Some(Value::I32(self.item_count)),
            "status" => Some(Value::I32(self.status as i32)),
            "pay_method" => Some(Value::I32(self.pay_method as i32)),
            "pay_at" => self.pay_at.as_ref().map(|s| Value::String(s.clone())),
            "offline_seq" => Some(Value::String(self.offline_seq.clone())),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("order_id").and_then(|v| v.as_i64()) {
            self.order_id = Some(v);
        }
        if let Some(v) = map.get("order_no").and_then(|v| v.as_str()) {
            self.order_no = v.to_string();
        }
        if let Some(v) = map.get("merchant_id").and_then(|v| v.as_i64()) {
            self.merchant_id = v;
        }
        if let Some(v) = map.get("device_id").and_then(|v| v.as_i64()) {
            self.device_id = v;
        }
        if let Some(v) = map.get("total_fen").and_then(|v| v.as_i64()) {
            self.total_fen = v;
        }
        if let Some(v) = map.get("total_weight_g").and_then(|v| v.as_i64()) {
            self.total_weight_g = v;
        }
        if let Some(v) = map.get("item_count").and_then(|v| v.as_i64()) {
            self.item_count = v as i32;
        }
        if let Some(v) = map.get("status").and_then(|v| v.as_i64()) {
            self.status = v as i8;
        }
        if let Some(v) = map.get("pay_method").and_then(|v| v.as_i64()) {
            self.pay_method = v as i8;
        }
        if let Some(v) = map.get("pay_at").and_then(|v| v.as_str()) {
            self.pay_at = Some(v.to_string());
        }
        if let Some(v) = map.get("offline_seq").and_then(|v| v.as_str()) {
            self.offline_seq = v.to_string();
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
        if let Some(v) = map.get("updated_at").and_then(|v| v.as_str()) {
            self.updated_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for Order {
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

    fn sample() -> Order {
        Order {
            order_id: Some(1),
            order_no: "O20260101001".into(),
            merchant_id: 7,
            device_id: 3,
            total_fen: 12500,
            total_weight_g: 500,
            item_count: 2,
            status: 1,
            pay_method: 1,
            pay_at: Some("2026-01-01 10:00:00".into()),
            offline_seq: "SEQ001".into(),
            created_at: Some("2026-01-01".into()),
            updated_at: Some("2026-01-01".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(Order::table_name(), "order");
        assert_eq!(Order::pk_name(), "order_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut o = sample();
        assert_eq!(o.pk(), 1);
        o.order_id = None;
        assert_eq!(o.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut o = sample();
        o.set_pk(77);
        assert_eq!(o.order_id, Some(77));
    }

    #[test]
    fn timestamp_fields_present() {
        let tf = Order::timestamp_fields().expect("应有时间戳");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, Some("updated_at"));
    }

    #[test]
    fn columns_count() {
        assert_eq!(Order::columns().len(), 13);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = Order::fillable();
        assert!(!f.contains(&"order_id"));
        assert!(f.contains(&"order_no"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(Order::guarded(), vec!["order_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let o = sample();
        assert_eq!(o.get_column_value("order_id"), Some(Value::I64(1)));
        assert_eq!(
            o.get_column_value("order_no"),
            Some(Value::String("O20260101001".into()))
        );
        assert_eq!(o.get_column_value("total_fen"), Some(Value::I64(12500)));
        assert_eq!(o.get_column_value("status"), Some(Value::I32(1)));
        assert_eq!(o.get_column_value("pay_method"), Some(Value::I32(1)));
        assert_eq!(
            o.get_column_value("offline_seq"),
            Some(Value::String("SEQ001".into()))
        );
        assert_eq!(o.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let o = Order {
            order_id: None,
            pay_at: None,
            created_at: None,
            updated_at: None,
            ..sample()
        };
        assert_eq!(o.get_column_value("order_id"), None);
        assert_eq!(o.get_column_value("pay_at"), None);
    }

    #[test]
    fn from_value_populates_all() {
        let mut o = Order {
            order_id: None,
            order_no: String::new(),
            merchant_id: 0,
            device_id: 0,
            total_fen: 0,
            total_weight_g: 0,
            item_count: 0,
            status: 0,
            pay_method: 0,
            pay_at: None,
            offline_seq: String::new(),
            created_at: None,
            updated_at: None,
        };
        let mut m = HashMap::new();
        m.insert("order_id".into(), Value::I64(5));
        m.insert("order_no".into(), Value::String("O999".into()));
        m.insert("merchant_id".into(), Value::I64(2));
        m.insert("device_id".into(), Value::I64(1));
        m.insert("total_fen".into(), Value::I64(9900));
        m.insert("total_weight_g".into(), Value::I64(300));
        m.insert("item_count".into(), Value::I64(1));
        m.insert("status".into(), Value::I64(2));
        m.insert("pay_method".into(), Value::I64(2));
        m.insert("pay_at".into(), Value::String("2026-01-02".into()));
        m.insert("offline_seq".into(), Value::String("S2".into()));
        m.insert("created_at".into(), Value::String("2026-01-01".into()));
        m.insert("updated_at".into(), Value::String("2026-01-02".into()));
        o.from_value(m);
        assert_eq!(o.order_id, Some(5));
        assert_eq!(o.order_no, "O999");
        assert_eq!(o.total_fen, 9900);
        assert_eq!(o.status, 2);
        assert_eq!(o.pay_method, 2);
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut o = sample();
        o.from_value(HashMap::new());
        assert_eq!(o.order_id, Some(1));
    }

    #[test]
    fn relation_loader_returns_none() {
        let o = sample();
        assert!(o.get_relation("any").is_none());
        assert_eq!(o.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut o = sample();
        o.set_relation_data("x", Value::Null);
        assert!(o.get_relation("x").is_none());
    }
}
