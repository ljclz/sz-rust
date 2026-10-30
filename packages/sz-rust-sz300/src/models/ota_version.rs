// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
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
    /// 适用的设备型号（生产列名 `device_model`）
    pub device_model: String,
    /// 固件下载地址
    pub url: String,
    /// 固件 MD5 校验值
    pub md5: String,
    /// 更新日志
    pub changelog: String,
    /// 固件大小（单位：字节）
    pub size: i64,
    /// 是否强制更新（0=否，1=是；生产列名 `forced`）
    pub forced: i8,
    /// 状态（0=禁用，1=启用）
    pub status: i8,
    /// 创建时间
    pub created_at: Option<String>,
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
        // 生产表 ota_version 只有 created_at，没有 updated_at
        Some(TimestampFields::new(Some("created_at"), None))
    }
}

impl ModelExt for OtaVersion {
    fn columns() -> Vec<&'static str> {
        vec![
            "ota_id",
            "version",
            "device_model",
            "url",
            "md5",
            "changelog",
            "size",
            "forced",
            "status",
            "created_at",
        ]
    }

    fn fillable() -> Vec<&'static str> {
        vec![
            "version",
            "device_model",
            "url",
            "md5",
            "changelog",
            "size",
            "forced",
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
            "device_model" => Some(Value::String(self.device_model.clone())),
            "url" => Some(Value::String(self.url.clone())),
            "md5" => Some(Value::String(self.md5.clone())),
            "changelog" => Some(Value::String(self.changelog.clone())),
            "size" => Some(Value::I64(self.size)),
            "forced" => Some(Value::I32(self.forced as i32)),
            "status" => Some(Value::I32(self.status as i32)),
            "created_at" => self.created_at.as_ref().map(|s| Value::String(s.clone())),
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
        if let Some(v) = map.get("device_model").and_then(|v| v.as_str()) {
            self.device_model = v.to_string();
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
        if let Some(v) = map.get("forced").and_then(|v| v.as_i64()) {
            self.forced = v as i8;
        }
        if let Some(v) = map.get("status").and_then(|v| v.as_i64()) {
            self.status = v as i8;
        }
        if let Some(v) = map.get("created_at").and_then(|v| v.as_str()) {
            self.created_at = Some(v.to_string());
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
