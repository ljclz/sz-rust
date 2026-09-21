// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
/// 能力来源类型，用于来源过滤和分组统计。
///
/// - `Skill`：AI 内置能力（如 LLM 对话、代码搜索、文件操作）
/// - `Plugin`：业务插件能力（如 CRM 搜索客户、CMS 发布文章）
/// - `Service`：框架内置服务能力（如 MCP 工具适配）
///
/// 序列化为小写形式：`"skill"` / `"plugin"` / `"service"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilitySource {
    Skill,
    Plugin,
    Service,
}
