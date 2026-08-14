//! MCP 工具桥接：将 sz-rust-mcp 7 工具暴露为 Agent 可用工具

pub mod bridge;

pub use bridge::{McpToolAdapter, McpToolBridge};
