// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 版本兼容声明与 API 稳定性标注（spec §5.5 规则 2-3、§6.5 规则 3）
//!
//! 不兼容版本加载拒绝；API 稳定性标注（stable/experimental/deprecated）。

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::error::SdkError;

/// API 稳定性级别（spec §6.5 规则 3）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StabilityLevel {
    /// 稳定（minor 版本不破坏）
    Stable,
    /// 实验性（可能变更）
    Experimental,
    /// 废弃（将移除）
    Deprecated,
}

impl StabilityLevel {
    /// 获取稳定性标签
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Experimental => "experimental",
            Self::Deprecated => "deprecated",
        }
    }
}

/// 插件清单（SDK 视角）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDescriptor {
    /// 插件名称
    pub name: String,
    /// 插件版本
    pub version: String,
    /// 兼容宿主版本范围（semver range）
    pub host_version_req: VersionReq,
    /// API 稳定性级别
    pub stability: StabilityLevel,
    /// 权限声明
    pub permissions: Vec<String>,
}

impl PluginDescriptor {
    /// 创建插件描述符
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        host_version_req: impl Into<String>,
        stability: StabilityLevel,
    ) -> Result<Self, SdkError> {
        let req = VersionReq::parse(host_version_req.into().as_str())
            .map_err(|e| SdkError::InvalidParam(format!("版本范围解析失败: {e}")))?;
        Ok(Self {
            name: name.into(),
            version: version.into(),
            host_version_req: req,
            stability,
            permissions: Vec::new(),
        })
    }

    /// 添加权限
    pub fn with_permission(mut self, permission: impl Into<String>) -> Self {
        self.permissions.push(permission.into());
        self
    }

    /// 校验宿主版本兼容性
    ///
    /// # 后置条件
    /// - 兼容 → Ok
    /// - 不兼容 → Err(VersionIncompatible)
    pub fn check_compatibility(&self, host_version: &str) -> Result<(), SdkError> {
        let version = Version::parse(host_version).map_err(|e| {
            SdkError::InvalidParam(format!("宿主版本 `{host_version}` 解析失败: {e}"))
        })?;

        if self.host_version_req.matches(&version) {
            Ok(())
        } else {
            Err(SdkError::VersionIncompatible(
                self.host_version_req.to_string(),
                host_version.to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_descriptor_creation() {
        let desc = PluginDescriptor::new("test-plugin", "1.0.0", ">=1.7.0", StabilityLevel::Stable)
            .unwrap();
        assert_eq!(desc.name, "test-plugin");
        assert_eq!(desc.version, "1.0.0");
        assert_eq!(desc.stability, StabilityLevel::Stable);
    }

    #[test]
    fn test_descriptor_with_permissions() {
        let desc = PluginDescriptor::new("test", "1.0.0", ">=1.0.0", StabilityLevel::Experimental)
            .unwrap()
            .with_permission("read:files")
            .with_permission("write:config");
        assert_eq!(desc.permissions.len(), 2);
    }

    #[test]
    fn test_check_compatible() {
        let desc =
            PluginDescriptor::new("test", "1.0.0", ">=1.7.0", StabilityLevel::Stable).unwrap();
        assert!(desc.check_compatibility("1.7.0").is_ok());
        assert!(desc.check_compatibility("2.0.0").is_ok());
    }

    #[test]
    fn test_check_incompatible() {
        let desc =
            PluginDescriptor::new("test", "1.0.0", ">=1.7.0", StabilityLevel::Stable).unwrap();
        let result = desc.check_compatibility("1.6.0");
        assert!(matches!(result, Err(SdkError::VersionIncompatible(_, _))));
    }

    #[test]
    fn test_check_invalid_host_version() {
        let desc =
            PluginDescriptor::new("test", "1.0.0", ">=1.0.0", StabilityLevel::Stable).unwrap();
        let result = desc.check_compatibility("not-a-version");
        assert!(matches!(result, Err(SdkError::InvalidParam(_))));
    }

    #[test]
    fn test_invalid_version_range() {
        let result = PluginDescriptor::new("test", "1.0.0", "not-a-range", StabilityLevel::Stable);
        assert!(result.is_err());
    }

    #[test]
    fn test_stability_as_str() {
        assert_eq!(StabilityLevel::Stable.as_str(), "stable");
        assert_eq!(StabilityLevel::Experimental.as_str(), "experimental");
        assert_eq!(StabilityLevel::Deprecated.as_str(), "deprecated");
    }

    #[test]
    fn test_stability_serialization() {
        let level = StabilityLevel::Experimental;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, "\"Experimental\"");
    }
}
