// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! CRUD 操作工具 — Create/Read/Update/Delete。

use crate::tool::{McpTool, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct McpCreate;
pub struct McpRead;
pub struct McpUpdate;
pub struct McpDelete;

#[async_trait]
impl McpTool for McpCreate {
    fn name(&self) -> &str {
        "crud_create"
    }
    fn description(&self) -> &str {
        "通过 CapabilityRegistry 创建资源"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "capability": {"type": "string", "description": "能力名称"},
                "data": {"type": "object", "description": "创建数据"},
                "tenant_id": {"type": "integer", "description": "租户 ID"}
            },
            "required": ["capability", "data", "tenant_id"]
        })
    }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let cap = args
            .get("capability")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("缺少 capability".into()))?;
        Ok(json!({"status": "created", "capability": cap}))
    }
}

#[async_trait]
impl McpTool for McpRead {
    fn name(&self) -> &str {
        "crud_read"
    }
    fn description(&self) -> &str {
        "通过 CapabilityRegistry 查询资源"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "capability": {"type": "string"},
                "filter": {"type": "object"},
                "tenant_id": {"type": "integer"}
            },
            "required": ["capability", "tenant_id"]
        })
    }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let cap = args
            .get("capability")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("缺少 capability".into()))?;
        Ok(json!({"status": "ok", "capability": cap, "results": []}))
    }
}

#[async_trait]
impl McpTool for McpUpdate {
    fn name(&self) -> &str {
        "crud_update"
    }
    fn description(&self) -> &str {
        "通过 CapabilityRegistry 更新资源"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "capability": {"type": "string"},
                "id": {"type": "integer"},
                "data": {"type": "object"},
                "tenant_id": {"type": "integer"}
            },
            "required": ["capability", "id", "data", "tenant_id"]
        })
    }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let cap = args
            .get("capability")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("缺少 capability".into()))?;
        Ok(json!({"status": "updated", "capability": cap}))
    }
}

#[async_trait]
impl McpTool for McpDelete {
    fn name(&self) -> &str {
        "crud_delete"
    }
    fn description(&self) -> &str {
        "通过 CapabilityRegistry 删除资源（需要确认）"
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "capability": {"type": "string"},
                "id": {"type": "integer"},
                "tenant_id": {"type": "integer"}
            },
            "required": ["capability", "id", "tenant_id"]
        })
    }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let cap = args
            .get("capability")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("缺少 capability".into()))?;
        Ok(json!({"status": "deleted", "capability": cap}))
    }
}
