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
