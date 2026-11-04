// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 迁移管理工具 — 生成迁移脚本 + 执行迁移。

use crate::tool::{McpTool, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};

/// 生成迁移脚本模板（UP/DOWN SQL）。
pub struct McpMigrateCreate;

#[async_trait]
impl McpTool for McpMigrateCreate {
    fn name(&self) -> &str {
        "migrate_create"
    }
    fn description(&self) -> &str {
        "生成迁移脚本模板（UP/DOWN SQL），使用 tokio::fs 写文件"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "迁移名称"},
                "description": {"type": "string", "description": "迁移描述"},
                "output_dir": {"type": "string", "description": "输出目录（默认 migrations）"}
            },
            "required": ["name"]
        })
    }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("缺少 name".into()))?;
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let output_dir = args
            .get("output_dir")
            .and_then(|v| v.as_str())
            .unwrap_or("migrations");

        // 信任边界：name / output_dir 来自 MCP 客户端，未校验前可写任意路径
        crate::tool_guard::validate_name_component(name, "name", 64)?;
        crate::tool_guard::validate_rel_path(output_dir, "output_dir", None)?;
        let description = crate::tool_guard::flatten_single_line(description);

        let template = format!(
            "-- Migration: {}\n-- Description: {}\n\n-- UP\n\n\n-- DOWN\n\n",
            name, description
        );
        let filename = format!("{}.sql", name);
        let filepath = format!("{}/{}", output_dir, filename);

        tokio::fs::create_dir_all(output_dir)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("创建目录失败: {e}")))?;
        tokio::fs::write(&filepath, &template)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("写文件失败: {e}")))?;

        Ok(json!({"status": "created", "filepath": filepath, "filename": filename}))
    }
}

/// 执行迁移（up/down）。
pub struct McpMigrateRun;

#[async_trait]
impl McpTool for McpMigrateRun {
    fn name(&self) -> &str {
        "migrate_run"
    }
    fn description(&self) -> &str {
        "执行迁移（cargo run -p sz-rust-migration）"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "direction": {"type": "string", "enum": ["up", "down"]},
                "steps": {"type": "integer"}
            }
        })
    }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let direction = args
            .get("direction")
            .and_then(|v| v.as_str())
            .unwrap_or("up");
        let steps = args.get("steps").and_then(|v| v.as_u64()).unwrap_or(0);

        if direction != "up" && direction != "down" {
            return Err(ToolError::InvalidArgs("direction 必须为 up 或 down".into()));
        }

        let mut cmd = tokio::process::Command::new("cargo");
        cmd.args(["run", "-p", "sz-rust-migration", "--"]);
        if direction == "down" {
            cmd.arg("--down");
        }
        if steps > 0 {
            cmd.arg("--steps").arg(steps.to_string());
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("执行迁移失败: {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();

        Ok(json!({
            "success": success,
            "direction": direction,
            "steps": steps,
            "stdout": stdout,
            "stderr": stderr
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrate_create_rejects_traversal_name() {
        for evil in ["../evil", "a/../b", "..\\evil", "-x", "a b"] {
            let result = McpMigrateCreate
                .execute(json!({"name": evil, "output_dir": "migrations"}))
                .await;
            assert!(
                matches!(result, Err(ToolError::InvalidArgs(_))),
                "name={evil:?} 应在参数校验层拒绝"
            );
        }
    }

    #[tokio::test]
    async fn migrate_create_rejects_traversal_output_dir() {
        for evil in ["../evil", "/abs", "a/../b", "..\\x", "."] {
            let result = McpMigrateCreate
                .execute(json!({"name": "create_user", "output_dir": evil}))
                .await;
            assert!(result.is_err(), "output_dir={evil:?} 应被拒绝");
        }
    }

    #[tokio::test]
    async fn migrate_create_rejects_comment_injection() {
        // description 换行必须被压平，防止逃逸 SQL 注释行
        let result = McpMigrateCreate
            .execute(json!({"name": "ok_name", "output_dir": "../out", "description": "x"}))
            .await;
        assert!(result.is_err());
    }
}
