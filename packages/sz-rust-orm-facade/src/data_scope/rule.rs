// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据范围规则定义 — `DataScopeMode` 枚举与 `DataScopeRule` 结构体

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

static RULE_SEQ: AtomicU64 = AtomicU64::new(0);

/// 数据范围模式（5 种）
///
/// 对齐 FSSADMIN `DataScopeTrait` 的 scope 类型：
/// - `All`：全部数据（超级管理员或无限制场景）
/// - `Dept`：仅本部门数据
/// - `DeptAndSub`：本部门及所有子部门数据
/// - `Self_`：仅本人创建的数据
/// - `Custom`：自定义条件（通过 `CustomConditionGenerator` 生成）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataScopeMode {
    All,
    Dept,
    DeptAndSub,
    #[serde(rename = "self")]
    Self_,
    Custom,
}

impl DataScopeMode {
    /// 转为字符串标识（用于日志和指标 label）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Dept => "dept",
            Self::DeptAndSub => "dept_and_sub",
            Self::Self_ => "self",
            Self::Custom => "custom",
        }
    }
}

impl std::fmt::Display for DataScopeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 数据范围规则
///
/// 每条规则绑定一张表，声明该表的数据范围模式和字段映射。
/// 规则按 `priority` 降序排列，首个匹配的规则生效。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataScopeRule {
    pub mode: DataScopeMode,
    pub dept_field: Option<String>,
    pub creator_field: Option<String>,
    pub custom_generator: Option<String>,
    pub target_table: String,
    pub priority: u32,
    pub rule_id: String,
    pub enabled: bool,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl DataScopeRule {
    /// 创建一条新规则
    pub fn new(target_table: impl Into<String>, mode: DataScopeMode) -> Self {
        let table = target_table.into();
        let seq = RULE_SEQ.fetch_add(1, Ordering::SeqCst);
        let rule_id = format!("rule_{}_{}_{}", table, mode.as_str(), seq);
        Self {
            mode,
            dept_field: None,
            creator_field: None,
            custom_generator: None,
            target_table: table,
            priority: 0,
            rule_id,
            enabled: true,
            created_at: None,
            updated_at: None,
        }
    }

    /// 设置部门字段名
    pub fn with_dept_field(mut self, field: impl Into<String>) -> Self {
        self.dept_field = Some(field.into());
        self
    }

    /// 设置创建者字段名
    pub fn with_creator_field(mut self, field: impl Into<String>) -> Self {
        self.creator_field = Some(field.into());
        self
    }

    /// 设置自定义生成器名称
    pub fn with_custom_generator(mut self, name: impl Into<String>) -> Self {
        self.custom_generator = Some(name.into());
        self
    }

    /// 设置优先级
    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_rule_id(mut self, rule_id: impl Into<String>) -> Self {
        self.rule_id = rule_id.into();
        self
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_as_str() {
        assert_eq!(DataScopeMode::All.as_str(), "all");
        assert_eq!(DataScopeMode::Dept.as_str(), "dept");
        assert_eq!(DataScopeMode::DeptAndSub.as_str(), "dept_and_sub");
        assert_eq!(DataScopeMode::Self_.as_str(), "self");
        assert_eq!(DataScopeMode::Custom.as_str(), "custom");
    }

    #[test]
    fn test_mode_serde() {
        let json = serde_json::to_string(&DataScopeMode::DeptAndSub).unwrap();
        assert_eq!(json, "\"dept_and_sub\"");
        let mode: DataScopeMode = serde_json::from_str("\"self\"").unwrap();
        assert_eq!(mode, DataScopeMode::Self_);
    }

    #[test]
    fn test_rule_builder() {
        let rule = DataScopeRule::new("order", DataScopeMode::DeptAndSub)
            .with_dept_field("dept_id")
            .with_priority(10);
        assert_eq!(rule.target_table, "order");
        assert_eq!(rule.mode, DataScopeMode::DeptAndSub);
        assert_eq!(rule.dept_field.as_deref(), Some("dept_id"));
        assert_eq!(rule.priority, 10);
    }

    #[test]
    fn test_mode_display() {
        assert_eq!(format!("{}", DataScopeMode::All), "all");
        assert_eq!(format!("{}", DataScopeMode::Dept), "dept");
        assert_eq!(format!("{}", DataScopeMode::DeptAndSub), "dept_and_sub");
        assert_eq!(format!("{}", DataScopeMode::Self_), "self");
        assert_eq!(format!("{}", DataScopeMode::Custom), "custom");
    }

    #[test]
    fn test_rule_default_fields() {
        let rule = DataScopeRule::new("order", DataScopeMode::Dept);
        assert!(rule.enabled, "new rule should be enabled by default");
        assert!(!rule.rule_id.is_empty(), "rule_id should be auto-generated");
        assert!(rule.created_at.is_none());
        assert!(rule.updated_at.is_none());
    }

    #[test]
    fn test_rule_serialize_roundtrip() {
        let rule = DataScopeRule::new("order", DataScopeMode::Dept)
            .with_dept_field("dept_id")
            .with_priority(10)
            .with_enabled(true);
        let json = serde_json::to_string(&rule).unwrap();
        let deserialized: DataScopeRule = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.target_table, rule.target_table);
        assert_eq!(deserialized.mode, rule.mode);
        assert_eq!(deserialized.priority, rule.priority);
        assert_eq!(deserialized.rule_id, rule.rule_id);
        assert_eq!(deserialized.enabled, rule.enabled);
    }

    #[test]
    fn test_rule_id_uniqueness() {
        let r1 = DataScopeRule::new("order", DataScopeMode::Dept);
        let r2 = DataScopeRule::new("order", DataScopeMode::Dept);
        assert_ne!(r1.rule_id, r2.rule_id, "rule_ids should be unique");
    }
}
