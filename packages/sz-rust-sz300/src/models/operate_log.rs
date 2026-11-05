// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 操作日志模型实体（对齐 PHP OperateLog 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperateLog {
    /// 日志主键 ID
    pub log_id: Option<i64>,
    /// 商户 ID（NULL 表示系统级操作/无商户归属；019_nullable_operate_log_merchant.sql 后改 NULLABLE）
    pub merchant_id: Option<i64>,
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
            "merchant_id" => self.merchant_id.map(Value::I64),
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
            self.merchant_id = Some(v);
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

    #[test]
    fn serde_roundtrip_preserves_nullable_merchant_id() {
        // 覆盖 merchant_id = Some（商户级操作）与 None（系统级操作）两种情况
        let with_merchant = OperateLog {
            log_id: Some(1),
            merchant_id: Some(55),
            operator: "admin".into(),
            action: "create_order".into(),
            detail: "创建订单 ORD-1".into(),
            ip: "10.0.0.1".into(),
            created_at: Some("2026-03-03 00:00:00".into()),
        };
        let json = serde_json::to_string(&with_merchant).expect("serialize");
        let back: OperateLog = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.log_id, Some(1));
        assert_eq!(back.merchant_id, Some(55));
        assert_eq!(back.operator, "admin");
        assert_eq!(back.action, "create_order");
        assert_eq!(back.detail, "创建订单 ORD-1");
        assert_eq!(back.ip, "10.0.0.1");
        assert_eq!(back.created_at.as_deref(), Some("2026-03-03 00:00:00"));

        let system_level = OperateLog {
            log_id: None,
            merchant_id: None,
            operator: "system".into(),
            action: "rotate_key".into(),
            detail: String::new(),
            ip: "127.0.0.1".into(),
            created_at: None,
        };
        let json2 = serde_json::to_string(&system_level).expect("serialize");
        let back2: OperateLog = serde_json::from_str(&json2).expect("deserialize");
        assert_eq!(back2.log_id, None);
        assert_eq!(back2.merchant_id, None);
        assert_eq!(back2.created_at, None);
    }

    #[test]
    fn model_trait_metadata_and_pk_defaults() {
        assert_eq!(OperateLog::table_name(), "operate_log");
        assert_eq!(OperateLog::pk_name(), "log_id");
        let mut m = OperateLog {
            log_id: None,
            merchant_id: None,
            operator: String::new(),
            action: String::new(),
            detail: String::new(),
            ip: String::new(),
            created_at: None,
        };
        assert_eq!(m.pk(), 0);
        m.set_pk(777);
        assert_eq!(m.pk(), 777);
        assert_eq!(OperateLog::columns().len(), 7);
        assert_eq!(OperateLog::fillable().len(), 5);
        assert_eq!(OperateLog::guarded().len(), 1);
        // merchant_id 为 None 时 get_column_value 返回 None
        assert!(m.get_column_value("merchant_id").is_none());
        assert!(m.get_column_value("operator").is_some());
        assert!(m.get_column_value("zzz").is_none());
        assert!(OperateLog::timestamp_fields().is_some());
    }
}
