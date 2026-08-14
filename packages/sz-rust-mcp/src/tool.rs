//! McpTool trait 抽象 — 统一 MCP 工具接口。
//!
//! 对应 design.md §2.2.2 接口 13。
//! 所有 trait 必须 `Send + Sync + 'static`。

use async_trait::async_trait;
use serde_json::Value;

/// MCP 工具错误类型。
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("参数校验失败: {0}")]
    InvalidArgs(String),
    #[error("执行失败: {0}")]
    ExecutionFailed(String),
    #[error("权限不足: {0}")]
    PermissionDenied(String),
    #[error("需要人工确认")]
    ConfirmationRequired,
    #[error("超时: {0}")]
    Timeout(String),
}

/// MCP 工具 trait。
#[async_trait]
pub trait McpTool: Send + Sync + 'static {
    /// 工具名称。
    fn name(&self) -> &str;

    /// 工具描述。
    fn description(&self) -> &str;

    /// 输入参数 JSON Schema。
    fn input_schema(&self) -> Value;

    /// 是否需要人工确认（敏感操作）。
    fn requires_confirmation(&self) -> bool {
        false
    }

    /// 执行工具。
    async fn execute(&self, args: Value) -> Result<Value, ToolError>;
}

/// 工具信息（用于 tools/list 响应）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub requires_confirmation: bool,
}

impl ToolInfo {
    pub fn from_tool(tool: &dyn McpTool) -> Self {
        Self {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            input_schema: tool.input_schema(),
            requires_confirmation: tool.requires_confirmation(),
        }
    }
}
