use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 操作日志模型实体（对齐 PHP OperateLog 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperateLog {
    /// 日志主键 ID
    pub log_id: Option<i64>,
    /// 商户 ID
    pub merchant_id: i64,
    /// 操作人
    pub operator: String,
    /// 操作动作
    pub action: String,
    /// 操作详情
    pub detail: String,
    /// 操作 IP 地址
    pub ip: String,
    /// 创建时间
    pub created_at: Option<String>,
}

impl Model for OperateLog {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "operate_log"
    }

    fn pk_name() -> &'static str {
        "log_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.log_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.log_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::new(Some("created_at"), None))
    }
}

impl ModelExt for OperateLog {
    fn columns() -> Vec<&'static str> {
        vec![
            "log_id",
            "merchant_id",
            "operator",
            "action",
            "detail",
            "ip",
            "created_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec!["merchant_id", "operator", "action", "detail", "ip"]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["log_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "log_id" => self.log_id.map(Value::I64),
            "merchant_id" => Some(Value::I64(self.merchant_id)),
            "operator" => Some(Value::String(self.operator.clone())),
            "action" => Some(Value::String(self.action.clone())),
            "detail" => Some(Value::String(self.detail.clone())),
            "ip" => Some(Value::String(self.ip.clone())),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("log_id").and_then(|v| v.as_i64()) {
            self.log_id = Some(v);
        }
        if let Some(v) = map.get("merchant_id").and_then(|v| v.as_i64()) {
            self.merchant_id = v;
        }
        if let Some(v) = map.get("operator").and_then(|v| v.as_str()) {
            self.operator = v.to_string();
        }
        if let Some(v) = map.get("action").and_then(|v| v.as_str()) {
            self.action = v.to_string();
        }
        if let Some(v) = map.get("detail").and_then(|v| v.as_str()) {
            self.detail = v.to_string();
        }
        if let Some(v) = map.get("ip").and_then(|v| v.as_str()) {
            self.ip = v.to_string();
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for OperateLog {
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

    fn sample() -> OperateLog {
        OperateLog {
            log_id: Some(1),
            merchant_id: 7,
            operator: "admin".into(),
            action: "login".into(),
            detail: "用户登录".into(),
            ip: "127.0.0.1".into(),
            created_at: Some("2026-01-01".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(OperateLog::table_name(), "operate_log");
        assert_eq!(OperateLog::pk_name(), "log_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut l = sample();
        assert_eq!(l.pk(), 1);
        l.log_id = None;
        assert_eq!(l.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut l = sample();
        l.set_pk(44);
        assert_eq!(l.log_id, Some(44));
    }

    #[test]
    fn timestamp_fields_created_only() {
        let tf = OperateLog::timestamp_fields().expect("应有时间戳");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, None);
    }

    #[test]
    fn columns_count() {
        assert_eq!(OperateLog::columns().len(), 7);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = OperateLog::fillable();
        assert!(!f.contains(&"log_id"));
        assert!(f.contains(&"operator"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(OperateLog::guarded(), vec!["log_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let l = sample();
        assert_eq!(l.get_column_value("log_id"), Some(Value::I64(1)));
        assert_eq!(l.get_column_value("merchant_id"), Some(Value::I64(7)));
        assert_eq!(
            l.get_column_value("operator"),
            Some(Value::String("admin".into()))
        );
        assert_eq!(
            l.get_column_value("action"),
            Some(Value::String("login".into()))
        );
        assert_eq!(
            l.get_column_value("ip"),
            Some(Value::String("127.0.0.1".into()))
        );
        assert_eq!(l.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let l = OperateLog {
            log_id: None,
            created_at: None,
            ..sample()
        };
        assert_eq!(l.get_column_value("log_id"), None);
        assert_eq!(l.get_column_value("created_at"), None);
    }

    #[test]
    fn from_value_populates_all() {
        let mut l = OperateLog {
            log_id: None,
            merchant_id: 0,
            operator: String::new(),
            action: String::new(),
            detail: String::new(),
            ip: String::new(),
            created_at: None,
        };
        let mut m = HashMap::new();
        m.insert("log_id".into(), Value::I64(9));
        m.insert("merchant_id".into(), Value::I64(2));
        m.insert("operator".into(), Value::String("sys".into()));
        m.insert("action".into(), Value::String("bind".into()));
        m.insert("detail".into(), Value::String("绑定".into()));
        m.insert("ip".into(), Value::String("10.0.0.1".into()));
        m.insert("created_at".into(), Value::String("2026-01-01".into()));
        l.from_value(m);
        assert_eq!(l.log_id, Some(9));
        assert_eq!(l.operator, "sys");
        assert_eq!(l.action, "bind");
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut l = sample();
        l.from_value(HashMap::new());
        assert_eq!(l.log_id, Some(1));
    }

    #[test]
    fn relation_loader_returns_none() {
        let l = sample();
        assert!(l.get_relation("any").is_none());
        assert_eq!(l.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut l = sample();
        l.set_relation_data("x", Value::Null);
        assert!(l.get_relation("x").is_none());
    }
}
