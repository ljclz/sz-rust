// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 市场扩展清单
//!
//! 通过 `#[serde(flatten)]` 嵌入 `AddonManifest` 基础字段，扩展市场特有字段。

use serde::{Deserialize, Serialize};

use sz_rust_addons_loader::manifest::AddonManifest;

use crate::error::{MarketplaceError, MarketplaceResult};

/// 审核状态
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// 待审核
    #[default]
    Pending,
    /// 已批准
    Approved,
    /// 已拒绝
    Rejected,
}

/// Capability 导出声明
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityExport {
    /// Capability 名称
    pub name: String,
    /// 描述
    pub description: String,
    /// JSON Schema（参数格式定义）
    pub schema: serde_json::Value,
    /// 标签
    pub tags: Vec<String>,
    /// 是否需要用户确认
    pub requires_confirmation: bool,
}

/// 依赖声明
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyDecl {
    /// 依赖插件名
    pub name: String,
    /// 版本范围（SemVer range）
    pub version_range: String,
}

/// 市场扩展清单
///
/// 通过 `#[serde(flatten)]` 嵌入 `AddonManifest` 的所有字段，
/// 并扩展市场特有的字段（描述、标签、Capability 导出、依赖、价格、签名、审核状态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceManifest {
    /// 基础清单字段（flatten 嵌入）
    #[serde(flatten)]
    pub base: AddonManifest,

    /// 插件描述
    #[serde(default)]
    pub description: Option<String>,

    /// 标签
    #[serde(default)]
    pub tags: Vec<String>,

    /// Capability 导出列表
    #[serde(default)]
    pub capabilities: Vec<CapabilityExport>,

    /// 依赖声明
    #[serde(default)]
    pub dependencies: Vec<DependencyDecl>,

    /// 许可证（SPDX 标识符）
    #[serde(default = "default_license")]
    pub license: String,

    /// 主页 URL
    #[serde(default)]
    pub homepage: Option<String>,

    /// 价格（0 = 免费）
    #[serde(default)]
    pub price: f64,

    /// Ed25519 签名（Base64 编码）
    #[serde(default)]
    pub signature: String,

    /// 审核状态
    #[serde(default, rename = "review_status")]
    pub status: ReviewStatus,
}

fn default_license() -> String {
    "Apache-2.0".to_string()
}

impl MarketplaceManifest {
    /// 从基础清单创建市场清单（扩展字段填默认值）
    pub fn from_base(base: AddonManifest) -> Self {
        Self {
            base,
            description: None,
            tags: Vec::new(),
            capabilities: Vec::new(),
            dependencies: Vec::new(),
            license: default_license(),
            homepage: None,
            price: 0.0,
            signature: String::new(),
            status: ReviewStatus::Pending,
        }
    }
}

impl From<AddonManifest> for MarketplaceManifest {
    fn from(base: AddonManifest) -> Self {
        Self::from_base(base)
    }
}

/// 校验清单标识字段（name / identifier）可安全用作存储路径组件
///
/// 插件名最终会拼入归档存储 key（`{name}/{version}/{name}.tar.gz`），
/// 必须限定字符集并排除 `..`，防止发布时路径穿越（写入 root 之外）。
pub fn validate_manifest_identity(manifest: &AddonManifest) -> MarketplaceResult<()> {
    for (label, value) in [
        ("name", manifest.name.as_str()),
        ("identifier", manifest.identifier.as_str()),
    ] {
        if value.is_empty() || value.len() > 64 {
            return Err(MarketplaceError::InvalidManifest(format!(
                "{label} 长度必须在 1..=64: {value:?}"
            )));
        }
        if !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            return Err(MarketplaceError::InvalidManifest(format!(
                "{label} 含非法字符（仅允许 ASCII 字母数字与 . _ -）: {value:?}"
            )));
        }
        if value.contains("..") {
            return Err(MarketplaceError::InvalidManifest(format!(
                "{label} 不允许包含 \"..\": {value:?}"
            )));
        }
    }
    Ok(())
}

/// 从 JSON 字符串解析市场清单
pub fn parse_manifest_json(content: &str) -> MarketplaceResult<MarketplaceManifest> {
    let manifest: MarketplaceManifest = serde_json::from_str(content)
        .map_err(|e| MarketplaceError::InvalidManifest(format!("JSON 解析失败: {e}")))?;
    validate_manifest_identity(&manifest.base)?;
    Ok(manifest)
}

/// 从 TOML 字符串解析市场清单
pub fn parse_manifest_toml(content: &str) -> MarketplaceResult<MarketplaceManifest> {
    let manifest: MarketplaceManifest = toml::from_str(content)
        .map_err(|e| MarketplaceError::InvalidManifest(format!("TOML 解析失败: {e}")))?;
    validate_manifest_identity(&manifest.base)?;
    Ok(manifest)
}

/// 从 PHP Plugin.php `$info` 数组字符串解析市场清单
///
/// 复用 `sz_rust_addons_loader::manifest::parse_manifest` 的 PHP 解析逻辑，
/// 需要传入插件目录路径（包含 Plugin.php 和 info.ini）。
pub async fn parse_manifest_php(
    addon_path: &std::path::Path,
) -> MarketplaceResult<MarketplaceManifest> {
    let base = sz_rust_addons_loader::manifest::parse_manifest(addon_path)
        .await
        .map_err(|e| MarketplaceError::InvalidManifest(format!("PHP 清单解析失败: {e}")))?;
    Ok(MarketplaceManifest::from_base(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_json_roundtrip() {
        let json = r#"{
            "name": "crm",
            "title": "CRM 插件",
            "identifier": "com.szrust.crm",
            "icon": "icon.png",
            "author": "SZ-Rust Team",
            "version": "1.0.0",
            "admin": "crm/admin",
            "status": 1,
            "description": "客户关系管理",
            "tags": ["crm", "business"],
            "license": "Apache-2.0",
            "price": 0.0,
            "signature": "",
            "review_status": "pending"
        }"#;

        let manifest = parse_manifest_json(json).unwrap();
        assert_eq!(manifest.base.name, "crm");
        assert_eq!(manifest.base.version, "1.0.0");
        assert_eq!(manifest.description.as_deref(), Some("客户关系管理"));
        assert_eq!(manifest.tags, vec!["crm", "business"]);
    }

    #[test]
    fn test_manifest_invalid_json() {
        let result = parse_manifest_json("{ invalid json }");
        assert!(result.is_err());
    }

    #[test]
    fn test_manifest_rejects_traversal_identity() {
        for evil in ["../../evil", "a/../b", "foo/bar", "foo\\bar", "..", "..a.."] {
            let json = format!(
                r#"{{ "name": "{evil}", "title": "x", "identifier": "id", "author": "a", "version": "1.0.0" }}"#
            );
            let result = parse_manifest_json(&json);
            assert!(result.is_err(), "name={evil:?} 应被拒绝");
        }
        for evil in ["com/../evil", "a b", "com:evil", "插件"] {
            let json = format!(
                r#"{{ "name": "ok-name", "title": "x", "identifier": "{evil}", "author": "a", "version": "1.0.0" }}"#
            );
            let result = parse_manifest_json(&json);
            assert!(result.is_err(), "identifier={evil:?} 应被拒绝");
        }
    }

    #[test]
    fn test_manifest_accepts_normal_identity() {
        let json = r#"{
            "name": "crm-pro_v2",
            "title": "x",
            "identifier": "com.szrust.crm",
            "icon": "icon.png",
            "author": "a",
            "version": "1.0.0",
            "admin": "",
            "status": 1
        }"#;
        assert!(parse_manifest_json(json).is_ok());
    }

    #[test]
    fn test_manifest_toml_parse() {
        let toml_str = r#"
name = "cms"
title = "CMS 插件"
identifier = "com.szrust.cms"
icon = ""
author = "SZ-Rust"
version = "2.1.0"
admin = ""
status = 1
description = "内容管理系统"
license = "MIT"
price = 9.99
"#;
        let manifest = parse_manifest_toml(toml_str).unwrap();
        assert_eq!(manifest.base.name, "cms");
        assert_eq!(manifest.base.version, "2.1.0");
        assert_eq!(manifest.license, "MIT");
        assert!((manifest.price - 9.99).abs() < f64::EPSILON);
    }

    #[test]
    fn test_from_addon_manifest() {
        let base = AddonManifest::new("test-plugin");
        let manifest = MarketplaceManifest::from(base);
        assert_eq!(manifest.base.name, "test-plugin");
        assert_eq!(manifest.license, "Apache-2.0");
        assert_eq!(manifest.status, ReviewStatus::Pending);
        assert!((manifest.price - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_review_status_serde() {
        let json = serde_json::to_string(&ReviewStatus::Approved).unwrap();
        assert_eq!(json, "\"approved\"");

        let status: ReviewStatus = serde_json::from_str("\"rejected\"").unwrap();
        assert_eq!(status, ReviewStatus::Rejected);
    }
}
