// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 部署工具 — 通过 Node.js ssh2 包执行远程部署。

use crate::tool::{McpTool, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

/// 通过 Node.js ssh2 包执行远程部署（禁止 sshpass）。
pub struct McpDeployRun;

#[async_trait]
impl McpTool for McpDeployRun {
    fn name(&self) -> &str {
        "deploy_run"
    }
    fn description(&self) -> &str {
        "通过 Node.js ssh2 包执行远程部署（需要人工确认）"
    }
    fn requires_confirmation(&self) -> bool {
        true
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script_path": {"type": "string", "description": "部署脚本路径（默认 scripts/e2e_deploy.js）"},
                "target": {"type": "string", "enum": ["docker", "k8s", "bare_metal"]},
                "timeout_secs": {"type": "integer", "description": "超时秒数（默认 600）"}
            },
            "required": ["target"]
        })
    }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let script_path = args
            .get("script_path")
            .and_then(|v| v.as_str())
            .unwrap_or("scripts/e2e_deploy.js");
        let target = args
            .get("target")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("缺少 target".into()))?;
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(600);

        if target != "docker" && target != "k8s" && target != "bare_metal" {
            return Err(ToolError::InvalidArgs(
                "target 必须为 docker/k8s/bare_metal".into(),
            ));
        }

        let mut cmd = tokio::process::Command::new("node");
        cmd.arg(script_path).arg("--target").arg(target);

        let timeout = Duration::from_secs(timeout_secs);
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| ToolError::Timeout(format!("部署超时（{}秒）", timeout_secs)))?
            .map_err(|e| ToolError::ExecutionFailed(format!("执行部署失败: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();

        Ok(json!({
            "success": success,
            "target": target,
            "script": script_path,
            "stdout": stdout,
            "stderr": stderr
        }))
    }
}
