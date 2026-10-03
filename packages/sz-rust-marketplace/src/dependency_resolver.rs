// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//! 插件依赖解析器 — semver range 冲突检测（spec §5.1 规则 3）
//!
//! v1.7.0 新增模块，通过 Cargo feature `plugin-market` 控制。

use std::collections::HashMap;

use semver::{Version, VersionReq};

use crate::error::MarketplaceError;
use crate::manifest::MarketplaceManifest;

/// 依赖冲突报告
#[derive(Debug, Clone, thiserror::Error)]
pub enum DependencyConflict {
    /// 版本 range 求交为空：已安装版本不满足待安装 range
    #[error("依赖 `{name}` 版本冲突：已安装 {installed} 不满足 {required}")]
    VersionConflict {
        /// 依赖名
        name: String,
        /// 已安装版本
        installed: Version,
        /// 待安装版本范围
        required: VersionReq,
    },
    /// 依赖缺失：声明的依赖未安装
    #[error("依赖 `{0}` 未安装")]
    Missing(String),
}

/// 依赖解析结果
#[derive(Debug, Clone)]
pub enum ResolveResult {
    /// 依赖兼容，可安装
    Compatible,
    /// 存在冲突，拒绝安装
    Conflict(Vec<DependencyConflict>),
}

/// 依赖解析器 trait
pub trait DependencyResolver: Send + Sync {
    /// 解析插件依赖树，检测与已安装插件的版本冲突
    ///
    /// # 前置条件
    /// - `manifest` 已通过签名验证
    /// - `installed` 为当前已安装插件的清单快照（名称 → 版本）
    ///
    /// # 后置条件
    /// - 返回 `Compatible` 表示可安全安装
    /// - 返回 `Conflict` 包含所有冲突详情
    fn resolve(
        &self,
        manifest: &MarketplaceManifest,
        installed: &HashMap<String, Version>,
    ) -> Result<ResolveResult, MarketplaceError>;
}

/// 默认 semver range 解析器实现
pub struct SemverResolver;

impl SemverResolver {
    /// 创建解析器
    pub fn new() -> Self {
        Self
    }
}

impl Default for SemverResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl DependencyResolver for SemverResolver {
    fn resolve(
        &self,
        manifest: &MarketplaceManifest,
        installed: &HashMap<String, Version>,
    ) -> Result<ResolveResult, MarketplaceError> {
        let mut conflicts = Vec::new();

        for dep in &manifest.dependencies {
            let req = VersionReq::parse(&dep.version_range).map_err(|e| {
                MarketplaceError::InvalidSemVer(format!(
                    "依赖 `{}` 版本范围 `{}` 解析失败: {}",
                    dep.name, dep.version_range, e
                ))
            })?;

            match installed.get(&dep.name) {
                Some(installed_version) => {
                    if !req.matches(installed_version) {
                        conflicts.push(DependencyConflict::VersionConflict {
                            name: dep.name.clone(),
                            installed: installed_version.clone(),
                            required: req,
                        });
                    }
                }
                None => {
                    conflicts.push(DependencyConflict::Missing(dep.name.clone()));
                }
            }
        }

        if conflicts.is_empty() {
            Ok(ResolveResult::Compatible)
        } else {
            Ok(ResolveResult::Conflict(conflicts))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::MarketplaceManifest;
    use sz_rust_addons_loader::manifest::AddonManifest;

    fn make_manifest(deps: Vec<(&str, &str)>) -> MarketplaceManifest {
        let mut base = AddonManifest::new("test-plugin");
        base.identifier = "com.szrust.test".to_string();
        base.version = "1.0.0".to_string();
        let mut manifest = MarketplaceManifest::from_base(base);
        manifest.dependencies = deps
            .into_iter()
            .map(|(name, version_range)| crate::manifest::DependencyDecl {
                name: name.to_string(),
                version_range: version_range.to_string(),
            })
            .collect();
        manifest
    }

    #[test]
    fn test_resolve_compatible() {
        let manifest = make_manifest(vec![("dep-a", "^1.0.0"), ("dep-b", ">=2.0.0")]);
        let mut installed = HashMap::new();
        installed.insert("dep-a".to_string(), Version::parse("1.2.0").unwrap());
        installed.insert("dep-b".to_string(), Version::parse("2.1.0").unwrap());

        let resolver = SemverResolver::new();
        let result = resolver.resolve(&manifest, &installed).unwrap();
        assert!(matches!(result, ResolveResult::Compatible));
    }

    #[test]
    fn test_resolve_version_conflict() {
        let manifest = make_manifest(vec![("dep-a", "^1.0.0")]);
        let mut installed = HashMap::new();
        installed.insert("dep-a".to_string(), Version::parse("2.0.0").unwrap());

        let resolver = SemverResolver::new();
        let result = resolver.resolve(&manifest, &installed).unwrap();
        match result {
            ResolveResult::Conflict(conflicts) => {
                assert_eq!(conflicts.len(), 1);
                assert!(matches!(
                    &conflicts[0],
                    DependencyConflict::VersionConflict { name, .. } if name == "dep-a"
                ));
            }
            _ => panic!("期望 Conflict"),
        }
    }

    #[test]
    fn test_resolve_missing_dependency() {
        let manifest = make_manifest(vec![("dep-x", "^1.0.0")]);
        let installed = HashMap::new();

        let resolver = SemverResolver::new();
        let result = resolver.resolve(&manifest, &installed).unwrap();
        match result {
            ResolveResult::Conflict(conflicts) => {
                assert_eq!(conflicts.len(), 1);
                assert!(matches!(
                    &conflicts[0],
                    DependencyConflict::Missing(name) if name == "dep-x"
                ));
            }
            _ => panic!("期望 Conflict"),
        }
    }

    #[test]
    fn test_resolve_multiple_conflicts() {
        let manifest = make_manifest(vec![
            ("dep-a", "^1.0.0"),
            ("dep-b", "^3.0.0"),
            ("dep-c", "^1.0.0"),
        ]);
        let mut installed = HashMap::new();
        installed.insert("dep-a".to_string(), Version::parse("2.0.0").unwrap());
        installed.insert("dep-b".to_string(), Version::parse("2.5.0").unwrap());

        let resolver = SemverResolver::new();
        let result = resolver.resolve(&manifest, &installed).unwrap();
        match result {
            ResolveResult::Conflict(conflicts) => {
                assert_eq!(conflicts.len(), 3);
            }
            _ => panic!("期望 Conflict"),
        }
    }

    #[test]
    fn test_resolve_no_dependencies() {
        let manifest = make_manifest(vec![]);
        let installed = HashMap::new();

        let resolver = SemverResolver::new();
        let result = resolver.resolve(&manifest, &installed).unwrap();
        assert!(matches!(result, ResolveResult::Compatible));
    }

    #[test]
    fn test_resolve_invalid_version_range() {
        let manifest = make_manifest(vec![("dep-a", "not-a-range")]);
        let installed = HashMap::new();

        let resolver = SemverResolver::new();
        let result = resolver.resolve(&manifest, &installed);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_exact_version_match() {
        let manifest = make_manifest(vec![("dep-a", "=1.2.3")]);
        let mut installed = HashMap::new();
        installed.insert("dep-a".to_string(), Version::parse("1.2.3").unwrap());

        let resolver = SemverResolver::new();
        let result = resolver.resolve(&manifest, &installed).unwrap();
        assert!(matches!(result, ResolveResult::Compatible));
    }

    #[test]
    fn test_resolve_exact_version_mismatch() {
        let manifest = make_manifest(vec![("dep-a", "=1.2.3")]);
        let mut installed = HashMap::new();
        installed.insert("dep-a".to_string(), Version::parse("1.2.4").unwrap());

        let resolver = SemverResolver::new();
        let result = resolver.resolve(&manifest, &installed).unwrap();
        assert!(matches!(result, ResolveResult::Conflict(_)));
    }
}
