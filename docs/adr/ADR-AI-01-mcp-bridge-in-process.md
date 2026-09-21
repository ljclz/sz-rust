# ADR-AI-01: MCP 工具桥接 — 进程内调用 vs stdio JSON-RPC

- **状态**: Accepted
- **日期**: 2026-08-10
- **相关代码**: `packages/sz-rust-ai-facade/src/mcp_bridge/bridge.rs`

## 背景

Agent 引擎需要调用工具完成多步推理任务。sz-rust-mcp 已实现 7 个工具（parse_path/build_select_query/openapi_spec/redaction_check/url_decode/sql_validate/route_conflicts），通过 stdio JSON-RPC 2.0 协议暴露。

两种集成方案：
1. **stdio 子进程**：Agent 通过 spawn 子进程 + JSON-RPC over stdin/stdout 调用 MCP 工具
2. **进程内调用**：Agent 直接调用 `sz_rust_mcp::call_tool()` 函数，无 IPC 开销

## 决策

选择**进程内调用**。

```rust
// packages/sz-rust-ai-facade/src/mcp_bridge/bridge.rs
pub async fn call(&self, name: &str, args: &serde_json::Value) -> Result<serde_json::Value, AiError> {
    let result = sz_rust_mcp::call_tool(name, args)?;
    serde_json::from_str(&result)
}
```

## 理由

- **零 IPC 开销**：无序列化/反序列化往返，无进程启动延迟
- **类型安全**：编译期链接检查，无协议版本不匹配风险
- **统一生命周期**：工具与 Agent 同进程，便于资源管理与优雅关闭
- **MCP 协议兼容**：`call_tool` 内部仍遵循 JSON-RPC 2.0 + 协议版本 2024-11-05 语义

## 代价

- 工具崩溃会影响 Agent 进程（通过 `AiError::ToolExecution` 捕获）
- 无法跨语言调用（Rust 工具限定）

## 影响

`McpToolAdapter` 实现 `Tool` trait，可直接注册到 `ToolRegistry`。