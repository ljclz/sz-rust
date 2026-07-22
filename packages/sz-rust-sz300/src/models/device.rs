use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_orm_core::{Model, ModelExt, RelationLoader, TimestampFields, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub device_id: Option<i64>,
    pub merchant_id: i64,
    pub device_sn: String,
    pub device_model: String,
    pub fw_version: String,
    pub status: i8,           // 0离线 1在线
    pub signal_strength: i32,
    pub bind_at: Option<String>,
    pub last_online_at: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

impl Model for Device {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "device"
    }

    fn pk_name() -> &'static str {
        "device_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.device_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.device_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for Device {
    fn columns() -> Vec<&'static str> {
        vec![
            "device_id",
            "merchant_id",
            "device_sn",
            "device_model",
            "fw_version",
            "status",
            "signal_strength",
            "bind_at",
            "last_online_at",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "merchant_id",
            "device_sn",
            "device_model",
            "fw_version",
            "status",
            "signal_strength",
            "bind_at",
            "last_online_at",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["device_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "device_id" => self.device_id.map(Value::I64),
            "merchant_id" => Some(Value::I64(self.merchant_id)),
            "device_sn" => Some(Value::String(self.device_sn.clone())),
            "device_model" => Some(Value::String(self.device_model.clone())),
            "fw_version" => Some(Value::String(self.fw_version.clone())),
            "status" => Some(Value::I32(self.status as i32)),
            "signal_strength" => Some(Value::I32(self.signal_strength)),
            "bind_at" => self.bind_at.as_ref().map(|s| Value::String(s.clone())),
            "last_online_at" => self.last_online_at.as_ref().map(|s| Value::String(s.clone())),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("device_id").and_then(|v| v.as_i64()) {
            self.device_id = Some(v);
        }
        if let Some(v) = map.get("merchant_id").and_then(|v| v.as_i64()) {
            self.merchant_id = v;
        }
        if let Some(v) = map.get("device_sn").and_then(|v| v.as_str()) {
            self.device_sn = v.to_string();
        }
        if let Some(v) = map.get("device_model").and_then(|v| v.as_str()) {
            self.device_model = v.to_string();
        }
        if let Some(v) = map.get("fw_version").and_then(|v| v.as_str()) {
            self.fw_version = v.to_string();
        }
        if let Some(v) = map.get("status").and_then(|v| v.as_i64()) {
            self.status = v as i8;
        }
        if let Some(v) = map.get("signal_strength").and_then(|v| v.as_i64()) {
            self.signal_strength = v as i32;
        }
        if let Some(v) = map.get("bind_at").and_then(|v| v.as_str()) {
            self.bind_at = Some(v.to_string());
        }
        if let Some(v) = map.get("last_online_at").and_then(|v| v.as_str()) {
            self.last_online_at = Some(v.to_string());
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
        }
        if let Some(v) = map.get("updated_at").and_then(|v| v.as_str()) {
            self.updated_at = Some(v.to_string());
        }
    }
}

impl RelationLoader for Device {
    fn get_relation(&self, _name: &str) -> Option<&Value> {
        None
    }

    fn set_relation_data(&mut self, _name: &str, _data: Value) {}

    fn get_relation_fk_value(&self, _fk_name: &str) -> String {
        String::new()
    }
}
