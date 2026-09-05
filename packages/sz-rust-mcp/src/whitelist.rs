// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 工具白名单鉴权 — 从 config/mcp.toml 读取白名单，拒绝未授权工具调用。

use crate::tool::ToolError;
use std::collections::HashSet;
use std::path::Path;

/// 工具白名单管理器。
#[derive(Debug, Clone)]
pub struct ToolWhitelist {
    allowed: HashSet<String>,
    sensitive_ops: HashSet<String>,
}

impl ToolWhitelist {
    /// 创建空白名单（拒绝所有工具）。
    pub fn new() -> Self {
        Self {
            allowed: HashSet::new(),
            sensitive_ops: HashSet::new(),
        }
    }

    /// 创建允许所有工具的白名单。
    pub fn allow_all() -> Self {
        let mut wl = Self::new();
        wl.allowed.insert("*".to_string());
        wl
    }

    /// 添加允许的工具。
    pub fn allow(mut self, tool: &str) -> Self {
        self.allowed.insert(tool.to_string());
        self
    }

    /// 标记敏感操作（需要确认）。
    pub fn mark_sensitive(mut self, tool: &str) -> Self {
        self.sensitive_ops.insert(tool.to_string());
        self
    }

    /// 检查工具是否被允许调用。
    pub fn check(&self, tool_name: &str) -> Result<(), ToolError> {
        if self.allowed.contains("*") || self.allowed.contains(tool_name) {
            if self.sensitive_ops.contains(tool_name) {
                return Err(ToolError::ConfirmationRequired);
            }
            Ok(())
        } else {
            Err(ToolError::PermissionDenied(format!(
                "工具 {} 不在白名单中",
                tool_name
            )))
        }
    }

    /// 从 TOML 配置文件加载白名单。
    ///
    /// 格式：
    /// ```toml
    /// [whitelist]
    /// allowed = ["crud_create", "crud_read", "test_run"]
    /// sensitive = ["crud_delete", "deploy_run", "plugin_uninstall"]
    /// ```
    pub async fn load_from_file(path: &Path) -> Result<Self, ToolError> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("读取白名单配置失败: {e}")))?;
        Self::parse_toml(&content)
    }

    /// 解析 TOML 配置文本。
    fn parse_toml(content: &str) -> Result<Self, ToolError> {
        let value: toml::Value = toml::from_str(content)
            .map_err(|e| ToolError::ExecutionFailed(format!("解析 TOML 失败: {e}")))?;

        let mut wl = Self::new();
        if let Some(table) = value.as_table() {
            if let Some(whitelist) = table.get("whitelist").and_then(|v| v.as_table()) {
                if let Some(allowed) = whitelist.get("allowed").and_then(|v| v.as_array()) {
                    for a in allowed {
                        if let Some(s) = a.as_str() {
                            wl.allowed.insert(s.to_string());
                        }
                    }
                }
                if let Some(sensitive) = whitelist.get("sensitive").and_then(|v| v.as_array()) {
                    for s in sensitive {
                        if let Some(s) = s.as_str() {
                            wl.sensitive_ops.insert(s.to_string());
                        }
                    }
                }
            }
        }
        Ok(wl)
    }

    /// 返回允许的工具数量。
    pub fn allowed_count(&self) -> usize {
        self.allowed.len()
    }

    /// 判断是否为敏感操作。
    pub fn is_sensitive(&self, tool_name: &str) -> bool {
        self.sensitive_ops.contains(tool_name)
    }
}

impl Default for ToolWhitelist {
    fn default() -> Self {
        Self::allow_all()
            .mark_sensitive("crud_delete")
            .mark_sensitive("deploy_run")
            .mark_sensitive("plugin_uninstall")
    }
}
