use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// 设备模型实体（对齐 PHP Device 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    /// 设备主键 ID
    pub device_id: Option<i64>,
    /// 绑定的商户 ID（0 表示未绑定）
    pub merchant_id: i64,
    /// 设备序列号
    pub device_sn: String,
    /// 设备型号
    pub device_model: String,
    /// 固件版本号
    pub fw_version: String,
    /// 在线状态（0=离线，1=在线）
    pub status: i8, // 0离线 1在线
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
            "merchant_id" => Some(Value::I64(self.merchant_id)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample() -> Device {
        Device {
            device_id: Some(1),
            merchant_id: 7,
            device_sn: "SN001".into(),
            device_model: "M100".into(),
            fw_version: "1.0".into(),
            status: 1,
            signal_strength: -60,
            bind_at: Some("2026-01-01".into()),
            last_online_at: Some("2026-01-02".into()),
            created_at: Some("2026-01-01".into()),
            updated_at: Some("2026-01-02".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(Device::table_name(), "device");
        assert_eq!(Device::pk_name(), "device_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut d = sample();
        assert_eq!(d.pk(), 1);
        d.device_id = None;
        assert_eq!(d.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut d = sample();
        d.set_pk(99);
        assert_eq!(d.device_id, Some(99));
    }

    #[test]
    fn timestamp_fields_present() {
        let tf = Device::timestamp_fields().expect("应有时间戳字段");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, Some("updated_at"));
    }

    #[test]
    fn columns_count() {
        let cols = Device::columns();
        assert_eq!(cols.len(), 11);
        assert!(cols.contains(&"device_id"));
        assert!(cols.contains(&"device_sn"));
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = Device::fillable();
        assert!(!f.contains(&"device_id"));
        assert!(f.contains(&"device_sn"));
    }

    #[test]
    fn guarded_contains_pk() {
        let g = Device::guarded();
        assert_eq!(g, vec!["device_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let d = sample();
        assert_eq!(d.get_column_value("device_id"), Some(Value::I64(1)));
        assert_eq!(d.get_column_value("merchant_id"), Some(Value::I64(7)));
        assert_eq!(
            d.get_column_value("device_sn"),
            Some(Value::String("SN001".into()))
        );
        assert_eq!(d.get_column_value("status"), Some(Value::I32(1)));
        assert_eq!(d.get_column_value("signal_strength"), Some(Value::I32(-60)));
        assert_eq!(
            d.get_column_value("bind_at"),
            Some(Value::String("2026-01-01".into()))
        );
        assert_eq!(d.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let d = Device {
            device_id: None,
            bind_at: None,
            last_online_at: None,
            created_at: None,
            updated_at: None,
            ..sample()
        };
        assert_eq!(d.get_column_value("device_id"), None);
        assert_eq!(d.get_column_value("bind_at"), None);
    }

    #[test]
    fn from_value_populates_all() {
        let mut d = Device {
            device_id: None,
            merchant_id: 0,
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
        let mut m = HashMap::new();
        m.insert("device_id".into(), Value::I64(5));
        m.insert("merchant_id".into(), Value::I64(3));
        m.insert("device_sn".into(), Value::String("SNX".into()));
        m.insert("device_model".into(), Value::String("M200".into()));
        m.insert("fw_version".into(), Value::String("2.0".into()));
        m.insert("status".into(), Value::I64(0));
        m.insert("signal_strength".into(), Value::I64(-50));
        m.insert("bind_at".into(), Value::String("2026-03-01".into()));
        m.insert("last_online_at".into(), Value::String("2026-03-02".into()));
        m.insert("created_at".into(), Value::String("2026-03-01".into()));
        m.insert("updated_at".into(), Value::String("2026-03-02".into()));
        d.from_value(m);
        assert_eq!(d.device_id, Some(5));
        assert_eq!(d.merchant_id, 3);
        assert_eq!(d.device_sn, "SNX");
        assert_eq!(d.device_model, "M200");
        assert_eq!(d.fw_version, "2.0");
        assert_eq!(d.status, 0);
        assert_eq!(d.signal_strength, -50);
        assert_eq!(d.bind_at, Some("2026-03-01".into()));
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut d = sample();
        d.from_value(HashMap::new());
        assert_eq!(d.device_id, Some(1));
    }

    #[test]
    fn relation_loader_returns_none() {
        let d = sample();
        assert!(d.get_relation("any").is_none());
        assert_eq!(d.get_relation_fk_value("any"), "");
    }

    #[test]
    fn set_relation_data_no_op() {
        let mut d = sample();
        d.set_relation_data("x", Value::Null);
        assert!(d.get_relation("x").is_none());
    }
}
