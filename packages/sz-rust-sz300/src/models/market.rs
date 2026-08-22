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
    use std::collections::HashMap;

    fn sample() -> Market {
        Market {
            market_id: Some(1),
            name: "太平市场".into(),
            address: "太平路1号".into(),
            contact: "张三".into(),
            phone: "13800138000".into(),
            status: 1,
            created_at: Some("2026-01-01".into()),
            updated_at: Some("2026-01-02".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(Market::table_name(), "market");
        assert_eq!(Market::pk_name(), "market_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut m = sample();
        assert_eq!(m.pk(), 1);
        m.market_id = None;
        assert_eq!(m.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut m = sample();
        m.set_pk(33);
        assert_eq!(m.market_id, Some(33));
    }

    #[test]
    fn timestamp_fields_present() {
        let tf = Market::timestamp_fields().expect("应有时间戳");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, Some("updated_at"));
    }

    #[test]
    fn columns_count() {
        assert_eq!(Market::columns().len(), 8);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = Market::fillable();
        assert!(!f.contains(&"market_id"));
        assert!(f.contains(&"name"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(Market::guarded(), vec!["market_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let m = sample();
        assert_eq!(m.get_column_value("market_id"), Some(Value::I64(1)));
        assert_eq!(
            m.get_column_value("name"),
            Some(Value::String("太平市场".into()))
        );
        assert_eq!(
            m.get_column_value("address"),
            Some(Value::String("太平路1号".into()))
        );
        assert_eq!(m.get_column_value("status"), Some(Value::I32(1)));
        assert_eq!(m.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let m = Market {
            market_id: None,
            created_at: None,
            updated_at: None,
            ..sample()
        };
        assert_eq!(m.get_column_value("market_id"), None);
    }

    #[test]
    fn from_value_populates_all() {
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
        let mut map = HashMap::new();
        map.insert("market_id".into(), Value::I64(5));
        map.insert("name".into(), Value::String("新市场".into()));
        map.insert("address".into(), Value::String("地址".into()));
        map.insert("contact".into(), Value::String("李四".into()));
        map.insert("phone".into(), Value::String("13900139000".into()));
        map.insert("status".into(), Value::I64(0));
        map.insert("created_at".into(), Value::String("2026-01-01".into()));
        map.insert("updated_at".into(), Value::String("2026-01-02".into()));
        m.from_value(map);
        assert_eq!(m.market_id, Some(5));
        assert_eq!(m.name, "新市场");
        assert_eq!(m.status, 0);
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut m = sample();
        m.from_value(HashMap::new());
        assert_eq!(m.market_id, Some(1));
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
