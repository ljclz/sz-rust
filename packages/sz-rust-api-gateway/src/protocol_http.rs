// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! HTTP 协议转换（T029）
//!
//! HTTP→HTTP 转发（最常用，优先）

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::GatewayError;

/// HTTP 请求转换配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HttpTransform {
    /// 请求头映射（源 → 目标）
    pub header_map: HashMap<String, String>,
    /// 移除的请求头
    pub strip_headers: Vec<String>,
    /// 请求体转换（JSON 字段映射）
    pub body_field_map: HashMap<String, String>,
}

impl HttpTransform {
    /// 创建空转换
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加头映射
    pub fn with_header(mut self, from: &str, to: &str) -> Self {
        self.header_map.insert(from.to_string(), to.to_string());
        self
    }

    /// 添加要移除的头
    pub fn strip(mut self, header: &str) -> Self {
        self.strip_headers.push(header.to_string());
        self
    }

    /// 添加体字段映射
    pub fn with_body_field(mut self, from: &str, to: &str) -> Self {
        self.body_field_map.insert(from.to_string(), to.to_string());
        self
    }

    /// 转换请求头
    pub fn transform_headers(&self, headers: &HashMap<String, String>) -> HashMap<String, String> {
        let mut result = HashMap::new();
        for (key, value) in headers {
            if self.strip_headers.contains(key) {
                continue;
            }
            let new_key = self
                .header_map
                .get(key)
                .cloned()
                .unwrap_or_else(|| key.clone());
            result.insert(new_key, value.clone());
        }
        result
    }

    /// 转换请求体（JSON 字段映射）
    pub fn transform_body(&self, body: &[u8]) -> Result<Vec<u8>, GatewayError> {
        if self.body_field_map.is_empty() {
            return Ok(body.to_vec());
        }

        let mut json: serde_json::Value = serde_json::from_slice(body)
            .map_err(|e| GatewayError::ProtocolConversion(format!("body parse: {e}")))?;

        if let Some(obj) = json.as_object_mut() {
            let original = obj.clone();
            for (from, to) in &self.body_field_map {
                if let Some(value) = original.get(from) {
                    obj.insert(to.clone(), value.clone());
                    if from != to {
                        obj.remove(from);
                    }
                }
            }
        }

        serde_json::to_vec(&json)
            .map_err(|e| GatewayError::ProtocolConversion(format!("body serialize: {e}")))
    }
}

/// HTTP 响应转换
pub fn transform_response(
    body: &[u8],
    field_map: &HashMap<String, String>,
) -> Result<Vec<u8>, GatewayError> {
    if field_map.is_empty() {
        return Ok(body.to_vec());
    }

    let mut json: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| GatewayError::ProtocolConversion(format!("response parse: {e}")))?;

    if let Some(obj) = json.as_object_mut() {
        let original = obj.clone();
        for (from, to) in field_map {
            if let Some(value) = original.get(from) {
                obj.insert(to.clone(), value.clone());
                if from != to {
                    obj.remove(from);
                }
            }
        }
    }

    serde_json::to_vec(&json)
        .map_err(|e| GatewayError::ProtocolConversion(format!("response serialize: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_transform_default() {
        let t = HttpTransform::default();
        let headers = HashMap::from([("X-Key".into(), "value".into())]);
        let result = t.transform_headers(&headers);
        assert_eq!(result.get("X-Key").unwrap(), "value");
    }

    #[test]
    fn test_header_mapping() {
        let t = HttpTransform::new().with_header("X-Old", "X-New");
        let headers = HashMap::from([
            ("X-Old".into(), "value".into()),
            ("X-Keep".into(), "keep".into()),
        ]);
        let result = t.transform_headers(&headers);
        assert_eq!(result.get("X-New").unwrap(), "value");
        assert!(!result.contains_key("X-Old"));
        assert_eq!(result.get("X-Keep").unwrap(), "keep");
    }

    #[test]
    fn test_strip_headers() {
        let t = HttpTransform::new().strip("X-Secret").strip("X-Internal");
        let headers = HashMap::from([
            ("X-Secret".into(), "secret".into()),
            ("X-Internal".into(), "internal".into()),
            ("X-Public".into(), "public".into()),
        ]);
        let result = t.transform_headers(&headers);
        assert!(!result.contains_key("X-Secret"));
        assert!(!result.contains_key("X-Internal"));
        assert_eq!(result.get("X-Public").unwrap(), "public");
    }

    #[test]
    fn test_body_transform_no_map() {
        let t = HttpTransform::new();
        let body = br#"{"name":"test"}"#.to_vec();
        let result = t.transform_body(&body).unwrap();
        assert_eq!(result, body);
    }

    #[test]
    fn test_body_transform_field_map() {
        let t = HttpTransform::new().with_body_field("name", "username");
        let body = br#"{"name":"test","age":30}"#;
        let result = t.transform_body(body).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(json["username"], "test");
        assert!(json.get("name").is_none());
        assert_eq!(json["age"], 30);
    }

    #[test]
    fn test_body_transform_invalid_json() {
        let t = HttpTransform::new().with_body_field("a", "b");
        let result = t.transform_body(b"not json");
        assert!(matches!(result, Err(GatewayError::ProtocolConversion(_))));
    }

    #[test]
    fn test_response_transform() {
        let map = HashMap::from([("code".into(), "status".into())]);
        let body = br#"{"code":200,"msg":"ok"}"#;
        let result = transform_response(body, &map).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(json["status"], 200);
        assert!(json.get("code").is_none());
    }

    #[test]
    fn test_response_transform_no_map() {
        let map = HashMap::new();
        let body = br#"{"key":"value"}"#;
        let result = transform_response(body, &map).unwrap();
        assert_eq!(result, body);
    }

    #[test]
    fn test_combined_header_and_body_transform() {
        let t = HttpTransform::new()
            .with_header("X-V1", "X-Version")
            .strip("X-Debug")
            .with_body_field("old_field", "new_field");

        let headers = HashMap::from([
            ("X-V1".into(), "2".into()),
            ("X-Debug".into(), "true".into()),
        ]);
        let transformed_headers = t.transform_headers(&headers);
        assert_eq!(transformed_headers.get("X-Version").unwrap(), "2");
        assert!(!transformed_headers.contains_key("X-Debug"));

        let body = br#"{"old_field":"data"}"#;
        let transformed_body = t.transform_body(body).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&transformed_body).unwrap();
        assert_eq!(json["new_field"], "data");
    }
}
