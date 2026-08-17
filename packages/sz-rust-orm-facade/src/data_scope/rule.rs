//! 数据范围规则定义 — `DataScopeMode` 枚举与 `DataScopeRule` 结构体

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone)]
pub struct DataScopeRule {
    /// 数据范围模式
    pub mode: DataScopeMode,
    /// 部门字段名（DEPT / DEPT_AND_SUB 模式必填，如 `"dept_id"`）
    pub dept_field: Option<String>,
    /// 创建者字段名（SELF 模式必填，如 `"creator_id"`）
    pub creator_field: Option<String>,
    /// 自定义条件生成器名称（CUSTOM 模式必填）
    pub custom_generator: Option<String>,
    /// 目标表名（如 `"order"`）
    pub target_table: String,
    /// 优先级（数值越大优先级越高，同表多规则时取最高优先级）
    pub priority: u32,
}

impl DataScopeRule {
    /// 创建一条新规则
    pub fn new(target_table: impl Into<String>, mode: DataScopeMode) -> Self {
        Self {
            mode,
            dept_field: None,
            creator_field: None,
            custom_generator: None,
            target_table: target_table.into(),
            priority: 0,
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
}
