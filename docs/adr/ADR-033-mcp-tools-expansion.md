# ADR-033: MCP 工具扩展 7→15+

- **状态**: Accepted
- **日期**: 2026-08-13
- **相关代码**: `packages/sz-rust-mcp/src/tool.rs`, `packages/sz-rust-mcp/src/tools/`, `packages/sz-rust-mcp/src/whitelist.rs`

## 背景

P1-4 缺口：MCP 工具仅 7 个基础工具，缺乏 CRUD/迁移/测试/部署/插件管理等扩展工具。

## 决策

1. **8+ 新工具**：CRUD（4）+ 迁移（2）+ 测试（1）+ 部署（1）+ 插件（2）
2. **McpTool trait**：统一 `async fn execute(&self, args) -> Result<Value>`
3. **Capability 适配**：`ExtendedMcpAdapter` 将 McpTool 适配为 Capability trait
4. **白名单鉴权**：ToolWhitelist 控制工具调用权限
5. **敏感操作 ConfirmationRequired**：部署/删除等操作需要用户确认

## 替代方案

- **直接注册为 Capability**：循环依赖（sz-rust-capability 依赖 sz-rust-mcp）
- **HTTP API**：引入网络开销，不符合进程内调用原则

## Bug 定位提示

- `tool.rs` — McpTool trait + ToolError + ToolInfo
- `tools/crud.rs` — 4 个 CRUD 工具
- `tools/deploy.rs` — McpDeployRun 部署工具
- `whitelist.rs` — ToolWhitelist 白名单鉴权

## 影响

- 28 tests (mcp) + 42 tests (capability) passed
- 工具通过 `register_extended_mcp_tools` 批量注册
- json_type_of 修复 integer/number 区分