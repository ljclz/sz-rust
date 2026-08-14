//! 测试工具 — 异步执行 cargo test，解析结果。

use crate::tool::{McpTool, ToolError};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

/// 异步执行 cargo test，解析 passed/failed/skipped 数量。
pub struct McpTestRun;

#[async_trait]
impl McpTool for McpTestRun {
    fn name(&self) -> &str {
        "test_run"
    }
    fn description(&self) -> &str {
        "异步执行 cargo test，返回 passed/failed/skipped 数量（5 分钟超时）"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "package": {"type": "string"},
                "test_name": {"type": "string"},
                "flags": {"type": "string"},
                "timeout_secs": {"type": "integer", "description": "超时秒数（默认 300）"}
            }
        })
    }
    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let package = args.get("package").and_then(|v| v.as_str());
        let test_name = args.get("test_name").and_then(|v| v.as_str());
        let flags = args.get("flags").and_then(|v| v.as_str()).unwrap_or("");
        let timeout_secs = args
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(300);

        let mut cmd = tokio::process::Command::new("cargo");
        cmd.arg("test");
        if let Some(pkg) = package {
            cmd.arg("-p").arg(pkg);
        }
        if let Some(tn) = test_name {
            cmd.arg("--").arg(tn);
        }
        if !flags.is_empty() {
            for f in flags.split_whitespace() {
                cmd.arg(f);
            }
        }

        let timeout = Duration::from_secs(timeout_secs);
        let output = tokio::time::timeout(timeout, cmd.output())
            .await
            .map_err(|_| ToolError::Timeout(format!("测试执行超时（{}秒）", timeout_secs)))?
            .map_err(|e| ToolError::ExecutionFailed(format!("执行测试失败: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();

        let (passed, failed, ignored) = parse_test_output(&stdout);

        Ok(json!({
            "success": success,
            "passed": passed,
            "failed": failed,
            "ignored": ignored,
            "stdout": stdout,
            "stderr": stderr
        }))
    }
}

/// 解析 cargo test 输出，提取 passed/failed/ignored 数量。
fn parse_test_output(stdout: &str) -> (u64, u64, u64) {
    let mut passed = 0u64;
    let mut failed = 0u64;
    let mut ignored = 0u64;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("test result:") {
            for part in trimmed.split(',') {
                let part = part.trim();
                if let Some(n) = part.strip_prefix("ok ") {
                    passed = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("FAILED ") {
                    failed = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("ignored ") {
                    ignored = n.parse().unwrap_or(0);
                }
            }
        }
    }

    (passed, failed, ignored)
}
