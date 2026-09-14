// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 插件管理工具 — 安装/卸载插件。

use crate::tool::{McpTool, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};

/// 安装插件（从插件市场下载并注册到 CapabilityRegistry）。
pub struct McpPluginInstall;

#[async_trait]
impl McpTool for McpPluginInstall {
    fn name(&self) -> &str {
        "plugin_install"
    }
    fn description(&self) -> &str {
        "从插件市场安装插件（cargo add + 注册到 CapabilityRegistry）"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "plugin_name": {"type": "string", "description": "插件名称"},
                "version": {"type": "string", "description": "版本（可选）"}
            },
            "required": ["plugin_name"]
        })
    }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let plugin_name = args
            .get("plugin_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("缺少 plugin_name".into()))?;
        let version = args.get("version").and_then(|v| v.as_str());

        let mut cmd = tokio::process::Command::new("cargo");
        cmd.arg("add").arg(plugin_name);
        if let Some(v) = version {
            cmd.arg("--version").arg(v);
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("cargo add 失败: {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();

        Ok(json!({
            "success": success,
            "plugin_name": plugin_name,
            "version": version,
            "stdout": stdout,
            "stderr": stderr,
            "post_install": format!("调用 CapabilityRegistry::register 注册 {}", plugin_name)
        }))
    }
}

/// 卸载插件（需要人工确认）。
pub struct McpPluginUninstall;

#[async_trait]
impl McpTool for McpPluginUninstall {
    fn name(&self) -> &str {
        "plugin_uninstall"
    }
    fn description(&self) -> &str {
        "卸载插件（cargo remove + 清理注册，需要人工确认）"
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "plugin_name": {"type": "string"}
            },
            "required": ["plugin_name"]
        })
    }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let plugin_name = args
            .get("plugin_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("缺少 plugin_name".into()))?;

        let output = tokio::process::Command::new("cargo")
            .arg("remove")
            .arg(plugin_name)
            .output()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("cargo remove 失败: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();

        Ok(json!({
            "success": success,
            "plugin_name": plugin_name,
            "stdout": stdout,
            "stderr": stderr,
            "post_uninstall": format!("调用 CapabilityRegistry::unregister(\"{}\") 清理注册", plugin_name)
        }))
    }
}
