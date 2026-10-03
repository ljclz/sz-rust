// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 主题切换 API（spec §5.25 规则 3，§6.25 规则 3）
//!
//! 暗色/亮色可配置，用户主题偏好持久化。

use std::collections::HashMap;

use crate::error::AdminUiError;

/// 主题类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Theme {
    /// 亮色
    #[default]
    Light,
    /// 暗色
    Dark,
    /// 跟随系统
    Auto,
}

impl Theme {
    /// 从字符串解析
    pub fn parse(s: &str) -> Result<Self, AdminUiError> {
        match s.to_lowercase().as_str() {
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            "auto" => Ok(Self::Auto),
            _ => Err(AdminUiError::InvalidParam(format!("未知主题: {s}"))),
        }
    }

    /// 转字符串
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Auto => "auto",
        }
    }
}

/// 主题配置
#[derive(Debug, Clone)]
pub struct ThemeConfig {
    /// 当前主题
    pub current: Theme,
    /// 自定义主题变量
    pub variables: HashMap<String, String>,
}

impl ThemeConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self {
            current: Theme::default(),
            variables: HashMap::new(),
        }
    }

    /// 设置主题
    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.current = theme;
        self
    }

    /// 设置变量
    pub fn with_variable(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.variables.insert(key.into(), value.into());
        self
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// 主题服务（spec §5.25 规则 3）
pub struct ThemeService {
    /// 用户主题偏好存储
    preferences: parking_lot::RwLock<HashMap<String, ThemeConfig>>,
}

impl ThemeService {
    /// 创建主题服务
    pub fn new() -> Self {
        Self {
            preferences: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// 主题切换（持久化用户主题偏好，spec §5.25 规则 3）
    pub fn switch_theme(&self, user_id: &str, theme: Theme) -> Result<(), AdminUiError> {
        if user_id.is_empty() {
            return Err(AdminUiError::InvalidParam("用户 ID 不能为空".into()));
        }
        let mut prefs = self.preferences.write();
        let config = prefs.entry(user_id.to_string()).or_default();
        config.current = theme;
        Ok(())
    }

    /// 获取用户主题
    pub fn get_theme(&self, user_id: &str) -> Theme {
        let prefs = self.preferences.read();
        prefs.get(user_id).map(|c| c.current).unwrap_or_default()
    }

    /// 设置主题变量
    pub fn set_variable(
        &self,
        user_id: &str,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), AdminUiError> {
        if user_id.is_empty() {
            return Err(AdminUiError::InvalidParam("用户 ID 不能为空".into()));
        }
        let mut prefs = self.preferences.write();
        let config = prefs.entry(user_id.to_string()).or_default();
        config.variables.insert(key.into(), value.into());
        Ok(())
    }

    /// 获取用户主题配置
    pub fn get_config(&self, user_id: &str) -> ThemeConfig {
        let prefs = self.preferences.read();
        prefs.get(user_id).cloned().unwrap_or_default()
    }
}

impl Default for ThemeService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_parse() {
        assert_eq!(Theme::parse("light").unwrap(), Theme::Light);
        assert_eq!(Theme::parse("DARK").unwrap(), Theme::Dark);
        assert_eq!(Theme::parse("auto").unwrap(), Theme::Auto);
        assert!(Theme::parse("invalid").is_err());
    }

    #[test]
    fn test_theme_as_str() {
        assert_eq!(Theme::Light.as_str(), "light");
        assert_eq!(Theme::Dark.as_str(), "dark");
        assert_eq!(Theme::Auto.as_str(), "auto");
    }

    #[test]
    fn test_theme_service_switch() {
        let svc = ThemeService::new();
        svc.switch_theme("user1", Theme::Dark).unwrap();
        assert_eq!(svc.get_theme("user1"), Theme::Dark);
    }

    #[test]
    fn test_theme_service_default() {
        let svc = ThemeService::new();
        assert_eq!(svc.get_theme("user1"), Theme::Light);
    }

    #[test]
    fn test_theme_service_variables() {
        let svc = ThemeService::new();
        svc.set_variable("user1", "primary_color", "#1890ff")
            .unwrap();
        let config = svc.get_config("user1");
        assert_eq!(
            config.variables.get("primary_color"),
            Some(&"#1890ff".to_string())
        );
    }

    #[test]
    fn test_theme_service_empty_user() {
        let svc = ThemeService::new();
        assert!(svc.switch_theme("", Theme::Dark).is_err());
    }
}
