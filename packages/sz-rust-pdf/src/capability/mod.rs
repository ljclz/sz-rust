// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use sz_rust_addons_loader::capability_hook::CapabilityHook;
use sz_rust_capability::{CapError, CapResult, Capability, CapabilityRegistry, CapabilitySource};

use crate::{base64_encode, PdfState};

pub const PDF_CAPABILITY_NAMES: [&str; 3] = [
    "pdf.export_csv",
    "pdf.export_csv_download",
    "pdf.health_check",
];

pub struct PdfPlugin {
    state: PdfState,
}

impl PdfPlugin {
    pub fn new(state: PdfState) -> Self {
        Self { state }
    }
}

impl CapabilityHook for PdfPlugin {
    fn register_capabilities(&self, registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
        let caps: Vec<Arc<dyn Capability>> = vec![
            Arc::new(ExportCsvCapability::new()),
            Arc::new(ExportCsvDownloadCapability::new()),
            Arc::new(HealthCheckCapability::new(self.state.clone())),
        ];
        let mut names = Vec::with_capacity(caps.len());
        for cap in caps {
            let name = cap.name().to_string();
            registry.register(cap);
            names.push(name);
        }
        Ok(names)
    }

    fn capability_names(&self) -> Vec<String> {
        PDF_CAPABILITY_NAMES.iter().map(|s| s.to_string()).collect()
    }
}

pub struct ExportCsvCapability;

impl Default for ExportCsvCapability {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportCsvCapability {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Capability for ExportCsvCapability {
    fn name(&self) -> &'static str {
        "pdf.export_csv"
    }

    fn description(&self) -> &'static str {
        "导出 CSV 文件（返回 Base64 编码内容）"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filename": { "type": "string" },
                "headers": { "type": "array", "items": { "type": "string" } },
                "rows": { "type": "array", "items": { "type": "array", "items": { "type": "string" } } }
            },
            "required": ["filename", "headers", "rows"]
        })
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["pdf", "csv", "export", "write"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, params: Value) -> CapResult<Value> {
        let filename = params
            .get("filename")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ValidationError("filename is required".into()))?;
        let headers: Vec<String> = params
            .get("headers")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(|| CapError::ValidationError("headers is required".into()))?;
        let rows: Vec<Vec<String>> = params
            .get("rows")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(|| CapError::ValidationError("rows is required".into()))?;

        let bytes = crate::csv_export::export_csv_to_bytes(&headers, &rows).unwrap_or_default();
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "filename": filename,
                "format": "csv",
                "size": bytes.len(),
                "content_base64": base64_encode(&bytes)
            }
        }))
    }
}

pub struct ExportCsvDownloadCapability;

impl Default for ExportCsvDownloadCapability {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportCsvDownloadCapability {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Capability for ExportCsvDownloadCapability {
    fn name(&self) -> &'static str {
        "pdf.export_csv_download"
    }

    fn description(&self) -> &'static str {
        "导出 CSV 文件（直接下载二进制流）"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filename": { "type": "string" },
                "headers": { "type": "array", "items": { "type": "string" } },
                "rows": { "type": "array", "items": { "type": "array", "items": { "type": "string" } } }
            },
            "required": ["filename", "headers", "rows"]
        })
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["pdf", "csv", "export", "download"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, params: Value) -> CapResult<Value> {
        let headers: Vec<String> = params
            .get("headers")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(|| CapError::ValidationError("headers is required".into()))?;
        let rows: Vec<Vec<String>> = params
            .get("rows")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .ok_or_else(|| CapError::ValidationError("rows is required".into()))?;

        let bytes = crate::csv_export::export_csv_to_bytes(&headers, &rows).unwrap_or_default();
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "size": bytes.len(),
                "content_base64": base64_encode(&bytes)
            }
        }))
    }
}

pub struct HealthCheckCapability {
    state: PdfState,
}

impl HealthCheckCapability {
    pub fn new(state: PdfState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Capability for HealthCheckCapability {
    fn name(&self) -> &'static str {
        "pdf.health_check"
    }

    fn description(&self) -> &'static str {
        "PDF 服务健康检查"
    }

    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Plugin
    }

    fn tags(&self) -> &'static [&'static str] {
        &["pdf", "health", "read"]
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, _params: Value) -> CapResult<Value> {
        Ok(json!({
            "code": 1,
            "msg": "success",
            "data": {
                "plugin": "pdf",
                "status": "active",
                "modules": self.state.modules,
                "version": self.state.version
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pdf_capability_names() {
        let plugin = PdfPlugin::new(PdfState::default());
        let names = plugin.capability_names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"pdf.export_csv".to_string()));
        assert!(names.contains(&"pdf.export_csv_download".to_string()));
        assert!(names.contains(&"pdf.health_check".to_string()));
    }

    #[tokio::test]
    async fn test_export_csv_capability() {
        let cap = ExportCsvCapability::new();
        let params = json!({
            "filename": "test.csv",
            "headers": ["a", "b"],
            "rows": [["1", "2"]]
        });
        let result = cap.call(params).await.unwrap();
        assert_eq!(result["code"], 1);
        assert!(result["data"]["size"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_health_check_capability() {
        let cap = HealthCheckCapability::new(PdfState::default());
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["code"], 1);
        assert_eq!(result["data"]["plugin"], "pdf");
        assert_eq!(result["data"]["status"], "active");
    }
}
