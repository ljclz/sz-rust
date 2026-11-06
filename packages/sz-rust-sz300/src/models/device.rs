// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 设备模型实体（对齐 PHP Device 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    /// 设备主键 ID
    pub device_id: Option<i64>,
    /// 绑定的商户 ID（NULL 表示未绑定；013_nullable_device_merchant.sql 后改 NULLABLE）
    pub merchant_id: Option<i64>,
    /// 设备序列号
    pub device_sn: String,
    /// 设备型号
    pub device_model: String,
    /// 固件版本号
    pub fw_version: String,
    /// 在线状态（0=离线，1=在线，2/3=设备上报扩展状态如故障/告警）
    pub status: i8, // 0离线 1在线 2/3扩展（CHECK (status IN (0,1,2,3))，见 010_status_enum.sql）
    /// 信号强度
    pub signal_strength: i32,
    /// 绑定时间
    pub bind_at: Option<String>,
    /// 最后在线时间
    pub last_online_at: Option<String>,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
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
            "merchant_id" => self.merchant_id.map(Value::I64),
            "device_sn" => Some(Value::String(self.device_sn.clone())),
            "device_model" => Some(Value::String(self.device_model.clone())),
            "fw_version" => Some(Value::String(self.fw_version.clone())),
            "status" => Some(Value::I32(self.status as i32)),
            "signal_strength" => Some(Value::I32(self.signal_strength)),
            "bind_at" => self.bind_at.as_ref().map(|s| Value::String(s.clone())),
            "last_online_at" => self
                .last_online_at
                .as_ref()
                .map(|s| Value::String(s.clone())),
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
            self.merchant_id = Some(v);
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
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn serde_roundtrip_preserves_all_fields() {
        let d = Device {
            device_id: Some(101),
            merchant_id: Some(7),
            device_sn: "SN-A001".into(),
            device_model: "SZ-300".into(),
            fw_version: "1.2.3".into(),
            status: 1,
            signal_strength: -67,
            bind_at: Some("2026-01-01 00:00:00".into()),
            last_online_at: Some("2026-02-02 00:00:00".into()),
            created_at: Some("2026-01-01 00:00:00".into()),
            updated_at: None,
        };
        let json = serde_json::to_string(&d).expect("serialize");
        let back: Device = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.device_id, Some(101));
        assert_eq!(back.merchant_id, Some(7));
        assert_eq!(back.device_sn, "SN-A001");
        assert_eq!(back.device_model, "SZ-300");
        assert_eq!(back.fw_version, "1.2.3");
        assert_eq!(back.status, 1);
        assert_eq!(back.signal_strength, -67);
        assert_eq!(back.bind_at.as_deref(), Some("2026-01-01 00:00:00"));
        assert_eq!(back.last_online_at.as_deref(), Some("2026-02-02 00:00:00"));
        assert_eq!(back.created_at.as_deref(), Some("2026-01-01 00:00:00"));
        assert_eq!(back.updated_at, None);
    }

    #[test]
    fn model_trait_metadata_and_pk_defaults() {
        assert_eq!(Device::table_name(), "device");
        assert_eq!(Device::pk_name(), "device_id");
        let mut d = Device {
            device_id: None,
            merchant_id: None,
            device_sn: String::new(),
            device_model: String::new(),
            fw_version: String::new(),
            status: 0,
            signal_strength: 0,
            bind_at: None,
            last_online_at: None,
            created_at: None,
            updated_at: None,
        };
        // pk 默认 0；Option 字段 None 时 get_column_value 返回 None
        assert_eq!(d.pk(), 0);
        assert!(d.get_column_value("device_id").is_none());
        assert!(d.get_column_value("merchant_id").is_none());
        assert!(d.get_column_value("bind_at").is_none());
        assert!(d.get_column_value("last_online_at").is_none());
        assert!(d.get_column_value("created_at").is_none());
        assert!(d.get_column_value("updated_at").is_none());
        d.set_pk(999);
        assert_eq!(d.pk(), 999);
        assert!(d.get_column_value("device_id").is_some());
        // Some 路径
        d.merchant_id = Some(5);
        d.bind_at = Some("b".into());
        d.last_online_at = Some("l".into());
        d.created_at = Some("c".into());
        d.updated_at = Some("u".into());
        assert!(d.get_column_value("merchant_id").is_some());
        assert!(d.get_column_value("device_sn").is_some());
        assert!(d.get_column_value("device_model").is_some());
        assert!(d.get_column_value("fw_version").is_some());
        assert!(d.get_column_value("status").is_some());
        assert!(d.get_column_value("signal_strength").is_some());
        assert!(d.get_column_value("bind_at").is_some());
        assert!(d.get_column_value("last_online_at").is_some());
        assert!(d.get_column_value("created_at").is_some());
        assert!(d.get_column_value("updated_at").is_some());
        // 未知列
        assert!(d.get_column_value("nope").is_none());
        // 元数据
        assert_eq!(Device::columns().len(), 11);
        assert_eq!(Device::fillable().len(), 8);
        assert_eq!(Device::guarded().len(), 1);
        assert!(Device::timestamp_fields().is_some());
    }

    #[test]
    fn from_value_populates_all_fields() {
        let mut d = Device {
            device_id: None,
            merchant_id: None,
            device_sn: String::new(),
            device_model: String::new(),
            fw_version: String::new(),
            status: 0,
            signal_strength: 0,
            bind_at: None,
            last_online_at: None,
            created_at: None,
            updated_at: None,
        };
        let mut map = HashMap::new();
        map.insert("device_id".to_string(), Value::I64(11));
        map.insert("merchant_id".to_string(), Value::I64(22));
        map.insert("device_sn".to_string(), Value::String("sn".into()));
        map.insert("device_model".to_string(), Value::String("mdl".into()));
        map.insert("fw_version".to_string(), Value::String("1.0".into()));
        map.insert("status".to_string(), Value::I64(2));
        map.insert("signal_strength".to_string(), Value::I64(-50));
        map.insert("bind_at".to_string(), Value::String("b".into()));
        map.insert("last_online_at".to_string(), Value::String("l".into()));
        map.insert("created_at".to_string(), Value::String("c".into()));
        map.insert("updated_at".to_string(), Value::String("u".into()));
        d.from_value(map);
        assert_eq!(d.device_id, Some(11));
        assert_eq!(d.merchant_id, Some(22));
        assert_eq!(d.device_sn, "sn");
        assert_eq!(d.device_model, "mdl");
        assert_eq!(d.fw_version, "1.0");
        assert_eq!(d.status, 2);
        assert_eq!(d.signal_strength, -50);
        assert_eq!(d.bind_at.as_deref(), Some("b"));
        assert_eq!(d.last_online_at.as_deref(), Some("l"));
        assert_eq!(d.created_at.as_deref(), Some("c"));
        assert_eq!(d.updated_at.as_deref(), Some("u"));
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut d = Device {
            device_id: Some(1),
            merchant_id: Some(2),
            device_sn: "x".into(),
            device_model: "y".into(),
            fw_version: "z".into(),
            status: 3,
            signal_strength: 4,
            bind_at: Some("b".into()),
            last_online_at: Some("l".into()),
            created_at: Some("c".into()),
            updated_at: Some("u".into()),
        };
        d.from_value(HashMap::new());
        assert_eq!(d.device_id, Some(1));
        assert_eq!(d.merchant_id, Some(2));
        assert_eq!(d.device_sn, "x");
        assert_eq!(d.status, 3);
    }

    #[test]
    fn relation_loader_returns_none() {
        let d = Device {
            device_id: Some(1),
            merchant_id: None,
            device_sn: String::new(),
            device_model: String::new(),
            fw_version: String::new(),
            status: 0,
            signal_strength: 0,
            bind_at: None,
            last_online_at: None,
            created_at: None,
            updated_at: None,
        };
        assert!(d.get_relation("any").is_none());
        let mut d2 = d.clone();
        d2.set_relation_data("any", Value::String("v".into()));
        assert_eq!(d2.get_relation_fk_value("any"), String::new());
    }
}
