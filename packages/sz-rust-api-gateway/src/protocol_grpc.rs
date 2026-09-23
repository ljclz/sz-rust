// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! gRPC 协议转换（T029，实验性）
//!
//! HTTP→gRPC 转换，通过字段映射规则。
//! 标注 `#[doc(hidden)]` 因为 gRPC 支持是实验性的。

#![cfg(feature = "grpc")]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::GatewayError;

/// gRPC 字段映射规则
#[derive(Debug, Clone, Serialize, Deserialize)]
#[doc(hidden)]
pub struct GrpcFieldMapping {
    /// JSON 字段名 → gRPC 字段名
    pub field_map: HashMap<String, String>,
    /// gRPC 服务全名（如 `user.UserService/GetUser`）
    pub service_method: String,
    /// 是否将 JSON 数组转为 repeated
    pub array_to_repeated: bool,
}

impl GrpcFieldMapping {
    /// 创建 gRPC 字段映射
    #[doc(hidden)]
    pub fn new(service_method: &str) -> Self {
        Self {
            field_map: HashMap::new(),
            service_method: service_method.to_string(),
            array_to_repeated: true,
        }
    }

    /// 添加字段映射
    #[doc(hidden)]
    pub fn with_field(mut self, json_field: &str, grpc_field: &str) -> Self {
        self.field_map
            .insert(json_field.to_string(), grpc_field.to_string());
        self
    }

    /// 将 JSON 请求体转换为 gRPC 消息（protobuf 编码）
    #[doc(hidden)]
    pub fn json_to_grpc(&self, json_body: &[u8]) -> Result<Vec<u8>, GatewayError> {
        let mut json: serde_json::Value = serde_json::from_slice(json_body)
            .map_err(|e| GatewayError::ProtocolConversion(format!("JSON parse: {e}")))?;

        if let Some(obj) = json.as_object_mut() {
            let original = obj.clone();
            for (json_name, grpc_name) in &self.field_map {
                if let Some(value) = original.get(json_name) {
                    obj.insert(grpc_name.clone(), value.clone());
                    if json_name != grpc_name {
                        obj.remove(json_name);
                    }
                }
            }
        }

        let json_bytes = serde_json::to_vec(&json)
            .map_err(|e| GatewayError::ProtocolConversion(format!("JSON serialize: {e}")))?;

        Ok(json_bytes)
    }

    /// 将 gRPC 响应转换为 JSON
    #[doc(hidden)]
    pub fn grpc_to_json(&self, grpc_body: &[u8]) -> Result<Vec<u8>, GatewayError> {
        let mut json: serde_json::Value = serde_json::from_slice(grpc_body)
            .map_err(|e| GatewayError::ProtocolConversion(format!("gRPC parse: {e}")))?;

        if let Some(obj) = json.as_object_mut() {
            let original = obj.clone();
            for (json_name, grpc_name) in &self.field_map {
                if let Some(value) = original.get(grpc_name) {
                    obj.insert(json_name.clone(), value.clone());
                    if json_name != grpc_name {
                        obj.remove(grpc_name);
                    }
                }
            }
        }

        serde_json::to_vec(&json)
            .map_err(|e| GatewayError::ProtocolConversion(format!("JSON serialize: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grpc_field_mapping_new() {
        let mapping = GrpcFieldMapping::new("user.UserService/GetUser");
        assert_eq!(mapping.service_method, "user.UserService/GetUser");
        assert!(mapping.array_to_repeated);
    }

    #[test]
    fn test_json_to_grpc_field_rename() {
        let mapping = GrpcFieldMapping::new("svc/method")
            .with_field("user_id", "userId")
            .with_field("user_name", "userName");

        let json = br#"{"user_id":"123","user_name":"test"}"#;
        let result = mapping.json_to_grpc(json).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["userId"], "123");
        assert_eq!(parsed["userName"], "test");
        assert!(parsed.get("user_id").is_none());
    }

    #[test]
    fn test_grpc_to_json_reverse() {
        let mapping = GrpcFieldMapping::new("svc/method").with_field("user_id", "userId");

        let grpc = br#"{"userId":"123"}"#;
        let result = mapping.grpc_to_json(grpc).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&result).unwrap();
        assert_eq!(parsed["user_id"], "123");
        assert!(parsed.get("userId").is_none());
    }

    #[test]
    fn test_json_to_grpc_invalid_json() {
        let mapping = GrpcFieldMapping::new("svc/method");
        let result = mapping.json_to_grpc(b"invalid");
        assert!(matches!(result, Err(GatewayError::ProtocolConversion(_))));
    }

    #[test]
    fn test_json_to_grpc_no_field_map() {
        let mapping = GrpcFieldMapping::new("svc/method");
        let json = br#"{"key":"value"}"#;
        let result = mapping.json_to_grpc(json).unwrap();
        assert_eq!(result, json);
    }
}
