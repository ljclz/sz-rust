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
    use std::collections::HashMap;

    fn sample() -> Merchant {
        Merchant {
            merchant_id: Some(1),
            market_id: 2,
            name: "太平店".into(),
            stall_no: "A01".into(),
            contact_phone: "13800138000".into(),
            category: "蔬菜".into(),
            status: 1,
            bank_account: "6222000000000000000".into(),
            bank_name: "工商银行".into(),
            created_at: Some("2026-01-01".into()),
            updated_at: Some("2026-01-02".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(Merchant::table_name(), "merchant");
        assert_eq!(Merchant::pk_name(), "merchant_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut m = sample();
        assert_eq!(m.pk(), 1);
        m.merchant_id = None;
        assert_eq!(m.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut m = sample();
        m.set_pk(42);
        assert_eq!(m.merchant_id, Some(42));
    }

    #[test]
    fn timestamp_fields_present() {
        let tf = Merchant::timestamp_fields().expect("应有时间戳");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, Some("updated_at"));
    }

    #[test]
    fn columns_count() {
        assert_eq!(Merchant::columns().len(), 11);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = Merchant::fillable();
        assert!(!f.contains(&"merchant_id"));
        assert!(f.contains(&"name"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(Merchant::guarded(), vec!["merchant_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let m = sample();
        assert_eq!(m.get_column_value("merchant_id"), Some(Value::I64(1)));
        assert_eq!(m.get_column_value("market_id"), Some(Value::I64(2)));
        assert_eq!(
            m.get_column_value("name"),
            Some(Value::String("太平店".into()))
        );
        assert_eq!(m.get_column_value("status"), Some(Value::I32(1)));
        assert_eq!(
            m.get_column_value("bank_account"),
            Some(Value::String("6222000000000000000".into()))
        );
        assert_eq!(m.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let m = Merchant {
            merchant_id: None,
            created_at: None,
            updated_at: None,
            ..sample()
        };
        assert_eq!(m.get_column_value("merchant_id"), None);
        assert_eq!(m.get_column_value("created_at"), None);
    }

    #[test]
    fn from_value_populates_all() {
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
        map.insert("merchant_id".into(), Value::I64(10));
        map.insert("market_id".into(), Value::I64(3));
        map.insert("name".into(), Value::String("新店".into()));
        map.insert("stall_no".into(), Value::String("B02".into()));
        map.insert("contact_phone".into(), Value::String("13900139000".into()));
        map.insert("category".into(), Value::String("水果".into()));
        map.insert("status".into(), Value::I64(0));
        map.insert("bank_account".into(), Value::String("acc".into()));
        map.insert("bank_name".into(), Value::String("建行".into()));
        map.insert("created_at".into(), Value::String("2026-01-01".into()));
        map.insert("updated_at".into(), Value::String("2026-01-02".into()));
        m.from_value(map);
        assert_eq!(m.merchant_id, Some(10));
        assert_eq!(m.market_id, 3);
        assert_eq!(m.name, "新店");
        assert_eq!(m.status, 0);
        assert_eq!(m.bank_name, "建行");
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut m = sample();
        m.from_value(HashMap::new());
        assert_eq!(m.merchant_id, Some(1));
    }

    #[test]
    fn relation_loader_returns_none() {
        let m = sample();
        assert!(m.get_relation("any").is_none());
        assert_eq!(m.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut m = sample();
        m.set_relation_data("x", Value::Null);
        assert!(m.get_relation("x").is_none());
    }
}
