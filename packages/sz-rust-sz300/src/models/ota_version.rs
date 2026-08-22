use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use sz_rust_core::orm::{Model, ModelExt, RelationLoader, TimestampFields, Value};

/// OTA 版本模型实体（对齐 PHP OtaVersion 模型）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtaVersion {
    /// OTA 版本主键 ID
    pub ota_id: Option<i64>,
    /// 版本号
    pub version: String,
    /// 适用的设备型号
    pub model: String,
    /// 固件下载地址
    pub url: String,
    /// 固件 MD5 校验值
    pub md5: String,
    /// 更新日志
    pub changelog: String,
    /// 固件大小（单位：字节）
    pub size: i64,
    /// 是否强制更新（0=否，1=是）
    pub force_update: i8,
    /// 状态（0=禁用，1=启用）
    pub status: i8,
    /// 创建时间
    pub created_at: Option<String>,
    /// 更新时间
    pub updated_at: Option<String>,
}

impl Model for OtaVersion {
    type PrimaryKey = i64;

    fn table_name() -> &'static str {
        "ota_version"
    }

    fn pk_name() -> &'static str {
        "ota_id"
    }

    fn pk(&self) -> Self::PrimaryKey {
        self.ota_id.unwrap_or(0)
    }

    fn set_pk(&mut self, pk: Self::PrimaryKey) {
        self.ota_id = Some(pk);
    }

    fn timestamp_fields() -> Option<TimestampFields> {
        Some(TimestampFields::with_both("created_at", "updated_at"))
    }
}

impl ModelExt for OtaVersion {
    fn columns() -> Vec<&'static str> {
        vec![
            "ota_id",
            "version",
            "model",
            "url",
            "md5",
            "changelog",
            "size",
            "force_update",
            "status",
            "created_at",
            "updated_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "version",
            "model",
            "url",
            "md5",
            "changelog",
            "size",
            "force_update",
            "status",
        ]
    }

    fn guarded() -> Vec<&'static str> {
        vec!["ota_id"]
    }

    fn get_column_value(&self, column: &str) -> Option<Value> {
        match column {
            "ota_id" => self.ota_id.map(Value::I64),
            "version" => Some(Value::String(self.version.clone())),
            "model" => Some(Value::String(self.model.clone())),
            "url" => Some(Value::String(self.url.clone())),
            "md5" => Some(Value::String(self.md5.clone())),
            "changelog" => Some(Value::String(self.changelog.clone())),
            "size" => Some(Value::I64(self.size)),
            "force_update" => Some(Value::I32(self.force_update as i32)),
            "status" => Some(Value::I32(self.status as i32)),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
            "updated_at" => self.updated_at.as_ref().map(|s| Value::String(s.clone())),
            _ => None,
        }
    }

    fn from_value(&mut self, map: HashMap<String, Value>) {
        if let Some(v) = map.get("ota_id").and_then(|v| v.as_i64()) {
            self.ota_id = Some(v);
        }
        if let Some(v) = map.get("version").and_then(|v| v.as_str()) {
            self.version = v.to_string();
        }
        if let Some(v) = map.get("model").and_then(|v| v.as_str()) {
            self.model = v.to_string();
        }
        if let Some(v) = map.get("url").and_then(|v| v.as_str()) {
            self.url = v.to_string();
        }
        if let Some(v) = map.get("md5").and_then(|v| v.as_str()) {
            self.md5 = v.to_string();
        }
        if let Some(v) = map.get("changelog").and_then(|v| v.as_str()) {
            self.changelog = v.to_string();
        }
        if let Some(v) = map.get("size").and_then(|v| v.as_i64()) {
            self.size = v;
        }
        if let Some(v) = map.get("force_update").and_then(|v| v.as_i64()) {
            self.force_update = v as i8;
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

impl RelationLoader for OtaVersion {
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

    fn sample() -> OtaVersion {
        OtaVersion {
            ota_id: Some(1),
            version: "1.0.0".into(),
            model: "M100".into(),
            url: "http://ota.example.com/1.0.0.bin".into(),
            md5: "abc123".into(),
            changelog: "初始版本".into(),
            size: 1024,
            force_update: 0,
            status: 1,
            created_at: Some("2026-01-01".into()),
            updated_at: Some("2026-01-02".into()),
        }
    }

    #[test]
    fn table_and_pk_name() {
        assert_eq!(OtaVersion::table_name(), "ota_version");
        assert_eq!(OtaVersion::pk_name(), "ota_id");
    }

    #[test]
    fn pk_returns_id_or_zero() {
        let mut o = sample();
        assert_eq!(o.pk(), 1);
        o.ota_id = None;
        assert_eq!(o.pk(), 0);
    }

    #[test]
    fn set_pk_updates_id() {
        let mut o = sample();
        o.set_pk(66);
        assert_eq!(o.ota_id, Some(66));
    }

    #[test]
    fn timestamp_fields_present() {
        let tf = OtaVersion::timestamp_fields().expect("应有时间戳");
        assert_eq!(tf.created_at, Some("created_at"));
        assert_eq!(tf.updated_at, Some("updated_at"));
    }

    #[test]
    fn columns_count() {
        assert_eq!(OtaVersion::columns().len(), 11);
    }

    #[test]
    fn fillable_excludes_pk() {
        let f = OtaVersion::fillable();
        assert!(!f.contains(&"ota_id"));
        assert!(f.contains(&"version"));
    }

    #[test]
    fn guarded_contains_pk() {
        assert_eq!(OtaVersion::guarded(), vec!["ota_id"]);
    }

    #[test]
    fn get_column_value_all_fields() {
        let o = sample();
        assert_eq!(o.get_column_value("ota_id"), Some(Value::I64(1)));
        assert_eq!(
            o.get_column_value("version"),
            Some(Value::String("1.0.0".into()))
        );
        assert_eq!(o.get_column_value("size"), Some(Value::I64(1024)));
        assert_eq!(o.get_column_value("force_update"), Some(Value::I32(0)));
        assert_eq!(o.get_column_value("status"), Some(Value::I32(1)));
        assert_eq!(o.get_column_value("nonexistent"), None);
    }

    #[test]
    fn get_column_value_none_fields() {
        let o = OtaVersion {
            ota_id: None,
            created_at: None,
            updated_at: None,
            ..sample()
        };
        assert_eq!(o.get_column_value("ota_id"), None);
        assert_eq!(o.get_column_value("created_at"), None);
    }

    #[test]
    fn from_value_populates_all() {
        let mut o = OtaVersion {
            ota_id: None,
            version: String::new(),
            model: String::new(),
            url: String::new(),
            md5: String::new(),
            changelog: String::new(),
            size: 0,
            force_update: 0,
            status: 0,
            created_at: None,
            updated_at: None,
        };
        let mut m = HashMap::new();
        m.insert("ota_id".into(), Value::I64(5));
        m.insert("version".into(), Value::String("2.0".into()));
        m.insert("model".into(), Value::String("M200".into()));
        m.insert("url".into(), Value::String("http://x".into()));
        m.insert("md5".into(), Value::String("md5".into()));
        m.insert("changelog".into(), Value::String("cl".into()));
        m.insert("size".into(), Value::I64(2048));
        m.insert("force_update".into(), Value::I64(1));
        m.insert("status".into(), Value::I64(0));
        m.insert("created_at".into(), Value::String("2026-01-01".into()));
        m.insert("updated_at".into(), Value::String("2026-01-02".into()));
        o.from_value(m);
        assert_eq!(o.ota_id, Some(5));
        assert_eq!(o.version, "2.0");
        assert_eq!(o.size, 2048);
        assert_eq!(o.force_update, 1);
    }

    #[test]
    fn from_value_empty_map_keeps_defaults() {
        let mut o = sample();
        o.from_value(HashMap::new());
        assert_eq!(o.ota_id, Some(1));
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
