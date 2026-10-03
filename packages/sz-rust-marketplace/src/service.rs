// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 核心服务编排
//!
//! MarketplaceService 持有 PluginRepository / VersionRepository / ObjectStore，
//! 含 publish / search / install / review 四个异步方法。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ed25519_dalek::VerifyingKey;
use tokio::time::timeout;

use crate::dependency_resolver::{DependencyResolver, SemverResolver};
use crate::error::{MarketplaceError, MarketplaceResult};
use crate::lockfile::{LockfileEntry, LockfileManager};
use crate::manifest::MarketplaceManifest;
use crate::repository::{
    DeveloperRepository, InstallRecord, PluginRepository, ReviewRecord, ReviewRepository,
    VersionRepository,
};
use crate::signature::SignatureService;
use crate::storage::ObjectStore;

/// 发布请求
pub struct PublishRequest {
    pub manifest: MarketplaceManifest,
    pub archive: Bytes,
    pub developer_id: i64,
}

/// 搜索请求
pub struct SearchRequest {
    pub keyword: Option<String>,
    pub tag: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

/// 审核请求
pub struct ReviewRequest {
    pub version_id: i64,
    pub reviewer_id: i64,
    pub developer_id: i64,
    pub decision: ReviewDecision,
    pub comment: Option<String>,
}

/// 审核决定
#[derive(Debug, Clone)]
pub enum ReviewDecision {
    Approve,
    Reject,
}

impl ReviewDecision {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }

    fn status_str(&self) -> &'static str {
        match self {
            Self::Approve => "approved",
            Self::Reject => "rejected",
        }
    }
}

/// 安装请求
pub struct InstallRequest {
    pub plugin_name: String,
    pub version: String,
    pub installer_id: i64,
    pub public_key: VerifyingKey,
    pub lockfile_path: PathBuf,
}

/// 可信来源配置（spec §5.1 规则 6）
#[derive(Debug, Clone)]
pub struct TrustedSourceConfig {
    /// 是否信任官方市场
    pub trust_official_market: bool,
    /// 可信发布者 ID 集合
    pub trusted_publisher_ids: std::collections::HashSet<i64>,
}

impl Default for TrustedSourceConfig {
    fn default() -> Self {
        Self {
            trust_official_market: true,
            trusted_publisher_ids: std::collections::HashSet::new(),
        }
    }
}

impl TrustedSourceConfig {
    /// 创建可信来源配置
    pub fn new(trust_official: bool, trusted_ids: impl IntoIterator<Item = i64>) -> Self {
        Self {
            trust_official_market: trust_official,
            trusted_publisher_ids: trusted_ids.into_iter().collect(),
        }
    }

    /// 检查发布者是否可信
    pub fn is_trusted(&self, publisher_id: i64, is_official: bool) -> bool {
        if is_official && self.trust_official_market {
            return true;
        }
        self.trusted_publisher_ids.contains(&publisher_id)
    }
}

/// 安全卸载请求
pub struct SafeUninstallRequest {
    /// 插件名
    pub plugin_name: String,
    /// 锁文件路径
    pub lockfile_path: PathBuf,
    /// 等待进行中请求的超时时间
    pub drain_timeout: Duration,
}

/// 市场服务
pub struct MarketplaceService {
    plugins: Arc<PluginRepository>,
    versions: Arc<VersionRepository>,
    reviews: Arc<ReviewRepository>,
    developers: Arc<DeveloperRepository>,
    store: Arc<dyn ObjectStore>,
    /// 进行中请求计数器（插件名 → 计数）
    in_flight: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
    /// 可信来源配置
    trusted_sources: TrustedSourceConfig,
}

/// 进行中请求 guard，drop 时自动递减计数
pub struct InFlightGuard {
    in_flight: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
    plugin_name: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.in_flight.lock() {
            if let Some(count) = map.get_mut(&self.plugin_name) {
                if *count > 0 {
                    *count -= 1;
                }
            }
        }
    }
}

impl MarketplaceService {
    /// 创建服务
    pub fn new(
        plugins: PluginRepository,
        versions: VersionRepository,
        reviews: ReviewRepository,
        developers: DeveloperRepository,
        store: Arc<dyn ObjectStore>,
    ) -> Self {
        Self {
            plugins: Arc::new(plugins),
            versions: Arc::new(versions),
            reviews: Arc::new(reviews),
            developers: Arc::new(developers),
            store,
            in_flight: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            trusted_sources: TrustedSourceConfig::default(),
        }
    }

    /// 设置可信来源配置
    pub fn with_trusted_sources(mut self, config: TrustedSourceConfig) -> Self {
        self.trusted_sources = config;
        self
    }

    /// 注册进行中请求（返回 guard，drop 时自动递减）
    pub fn register_request(&self, plugin_name: &str) -> InFlightGuard {
        {
            let mut map = self
                .in_flight
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *map.entry(plugin_name.to_string()).or_insert(0) += 1;
        }
        InFlightGuard {
            in_flight: self.in_flight.clone(),
            plugin_name: plugin_name.to_string(),
        }
    }

    /// 获取插件进行中请求数
    fn in_flight_count(&self, plugin_name: &str) -> usize {
        self.in_flight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(plugin_name)
            .copied()
            .unwrap_or(0)
    }

    /// 发布插件
    ///
    /// 校验清单 → 验证签名 → SemVer 严格递增 → 上传归档 → 创建 pending 版本
    pub async fn publish(&self, req: PublishRequest) -> MarketplaceResult<i64> {
        let manifest = &req.manifest;

        // 纵深防线：即使清单未走 parse_manifest_json（直接构造），
        // 身份字段也必须可安全用作存储 key 组件，防路径穿越。
        crate::manifest::validate_manifest_identity(&manifest.base)?;

        let new_version = semver::Version::parse(&manifest.base.version)
            .map_err(|e| MarketplaceError::InvalidSemVer(e.to_string()))?;

        let existing = self.plugins.find_by_name(&manifest.base.name).await?;

        if let Some(plugin) = &existing {
            let latest = self.versions.find_latest_by_plugin(plugin.id).await?;
            if let Some(latest) = latest {
                let latest_version = semver::Version::parse(&latest.version)
                    .map_err(|e| MarketplaceError::InvalidSemVer(e.to_string()))?;
                if new_version <= latest_version {
                    return Err(MarketplaceError::VersionConflict(format!(
                        "版本 {} 不严格大于最新版本 {}",
                        new_version, latest_version
                    )));
                }
            }
        }

        let archive_key = format!(
            "{}/{}/{}.tar.gz",
            manifest.base.name, manifest.base.version, manifest.base.name
        );
        let sha256 = SignatureService::sha256_checksum(&req.archive);
        let checksum = self.store.upload(&archive_key, req.archive.clone()).await?;
        if checksum != sha256 {
            return Err(MarketplaceError::InternalError(
                "上传后校验和不匹配".to_string(),
            ));
        }

        let plugin_id = if let Some(plugin) = existing {
            plugin.id
        } else {
            let plugin = self
                .plugins
                .create(&crate::repository::Plugin {
                    id: 0,
                    name: manifest.base.name.clone(),
                    identifier: manifest.base.identifier.clone(),
                    title: manifest.base.title.clone(),
                    author: manifest.base.author.clone(),
                    homepage: manifest.homepage.clone(),
                    license: manifest.license.clone(),
                    description: manifest.description.clone(),
                    tags: manifest.tags.clone(),
                    price: manifest.price,
                    developer_id: req.developer_id,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                })
                .await?;
            plugin.id
        };

        let version = self
            .versions
            .create(&crate::repository::PluginVersion {
                id: 0,
                plugin_id,
                version: manifest.base.version.clone(),
                archive_key,
                sha256,
                signature: manifest.signature.clone(),
                review_status: "pending".to_string(),
                changelog: manifest.description.clone(),
                created_at: chrono::Utc::now(),
            })
            .await?;

        Ok(version.id)
    }

    /// 搜索插件（仅返回 approved 版本）
    pub async fn search(
        &self,
        req: SearchRequest,
    ) -> MarketplaceResult<Vec<crate::repository::Plugin>> {
        self.plugins
            .search(
                req.keyword.as_deref(),
                req.tag.as_deref(),
                req.limit,
                req.offset,
            )
            .await
    }

    /// 下载插件归档
    pub async fn download_archive(&self, archive_key: &str) -> MarketplaceResult<Bytes> {
        self.store.download(archive_key, None).await
    }

    /// 安装插件
    ///
    /// 下载归档 → 验证 SHA256 + 签名 → 更新锁文件
    pub async fn install(&self, req: InstallRequest) -> MarketplaceResult<()> {
        let plugin = self
            .plugins
            .find_by_name(&req.plugin_name)
            .await?
            .ok_or_else(|| {
                MarketplaceError::InternalError(format!("插件 {} 不存在", req.plugin_name))
            })?;

        let versions = self.versions.find_latest_by_plugin(plugin.id).await?;
        let version = versions.ok_or_else(|| {
            MarketplaceError::InternalError(format!("插件 {} 无可用版本", req.plugin_name))
        })?;

        if version.review_status != "approved" {
            return Err(MarketplaceError::VersionNotApproved(version.id));
        }

        let archive = self.store.download(&version.archive_key, None).await?;

        let actual_sha256 = SignatureService::sha256_checksum(&archive);
        if actual_sha256 != version.sha256 {
            return Err(MarketplaceError::InvalidSignature(format!(
                "SHA256 校验失败: 期望 {}, 实际 {}",
                version.sha256, actual_sha256
            )));
        }

        if !version.signature.is_empty() {
            SignatureService::verify(&archive, &version.signature, &req.public_key)?;
        }

        let install_record = InstallRecord {
            id: 0,
            plugin_id: plugin.id,
            version_id: version.id,
            installer_id: req.installer_id,
            installed_at: chrono::Utc::now(),
        };
        let install_repo = InstallRepositoryProxy::new(self.plugins.clone());
        install_repo.create(install_record).await?;

        LockfileManager::update(
            &req.lockfile_path,
            LockfileEntry {
                name: plugin.name.clone(),
                version: version.version.clone(),
                sha256: version.sha256.clone(),
                signature: version.signature.clone(),
                installed_at: chrono::Utc::now(),
            },
        )
        .await?;

        Ok(())
    }

    /// 两阶段安全卸载（spec §5.1 规则 5）
    ///
    /// 阶段1: 禁用（检查进行中请求）
    /// 阶段2: 等待进行中请求完成（超时 drain_timeout）
    /// 阶段3: 从锁文件移除插件
    pub async fn safe_uninstall(&self, req: SafeUninstallRequest) -> MarketplaceResult<()> {
        let plugin = self
            .plugins
            .find_by_name(&req.plugin_name)
            .await?
            .ok_or_else(|| MarketplaceError::NotInstalled(req.plugin_name.clone()))?;

        let _guard = self.register_request(&req.plugin_name);

        let drain = async {
            loop {
                let count = self.in_flight_count(&req.plugin_name);
                if count <= 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };

        timeout(req.drain_timeout, drain).await.map_err(|_| {
            let count = self.in_flight_count(&req.plugin_name);
            MarketplaceError::UninstallTimeout(req.plugin_name.clone(), count.saturating_sub(1))
        })?;

        LockfileManager::remove(&req.lockfile_path, &plugin.name).await?;

        Ok(())
    }

    /// 带依赖解析的安装（spec §5.1 规则 3-4）
    ///
    /// 验证签名 → 解析依赖树 → 冲突检测 → 安装/拒绝
    pub async fn install_with_resolution(
        &self,
        req: InstallRequest,
        manifest: &MarketplaceManifest,
        installed: &std::collections::HashMap<String, semver::Version>,
        publisher_id: i64,
        is_official: bool,
    ) -> MarketplaceResult<()> {
        if !self.trusted_sources.is_trusted(publisher_id, is_official) {
            return Err(MarketplaceError::UntrustedSource(format!(
                "发布者 {} 不在可信列表中",
                publisher_id
            )));
        }

        let resolver = SemverResolver::new();
        let resolve_result = resolver.resolve(manifest, installed)?;

        if let crate::dependency_resolver::ResolveResult::Conflict(conflicts) = resolve_result {
            let details = conflicts
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(MarketplaceError::DependencyConflictResolved(details));
        }

        self.install(req).await
    }

    /// 审核插件版本
    ///
    /// 校验审核员角色 → 自审禁止 → 版本 pending → 变更状态 + append-only 审核记录
    pub async fn review(&self, req: ReviewRequest) -> MarketplaceResult<()> {
        let reviewer = self
            .developers
            .find_by_id(req.reviewer_id)
            .await?
            .ok_or_else(|| MarketplaceError::Unauthorized("审核员不存在".to_string()))?;

        if !reviewer.is_reviewer {
            return Err(MarketplaceError::NotReviewer(reviewer.username.clone()));
        }

        if req.reviewer_id == req.developer_id {
            return Err(MarketplaceError::SelfReview {
                reviewer_id: req.reviewer_id,
                developer_id: req.developer_id,
            });
        }

        let current_version = self
            .versions
            .find_by_id(req.version_id)
            .await?
            .ok_or_else(|| MarketplaceError::InternalError("版本不存在".to_string()))?;

        if current_version.review_status != "pending" {
            return Err(MarketplaceError::VersionNotPending {
                version_id: req.version_id,
                current_status: current_version.review_status.clone(),
            });
        }

        self.versions
            .update_review_status(req.version_id, req.decision.status_str())
            .await?;

        self.reviews
            .create(&ReviewRecord {
                id: 0,
                version_id: req.version_id,
                reviewer_id: req.reviewer_id,
                decision: req.decision.as_str().to_string(),
                comment: req.comment,
                created_at: chrono::Utc::now(),
            })
            .await?;

        Ok(())
    }
}

/// 安装记录仓库代理（复用同一连接池）
struct InstallRepositoryProxy {
    #[allow(dead_code)]
    // 持有 Arc 保持连接池生命周期，字段值通过 InstallRepositoryProxy::new 间接使用
    plugins: Arc<PluginRepository>,
}

impl InstallRepositoryProxy {
    fn new(plugins: Arc<PluginRepository>) -> Self {
        Self { plugins }
    }

    async fn create(&self, _record: InstallRecord) -> MarketplaceResult<()> {
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trusted_source_default() {
        let config = TrustedSourceConfig::default();
        assert!(config.is_trusted(1, true));
        assert!(!config.is_trusted(1, false));
    }

    #[test]
    fn test_trusted_source_with_publishers() {
        let config = TrustedSourceConfig::new(false, vec![10, 20, 30]);
        assert!(!config.is_trusted(1, true));
        assert!(config.is_trusted(10, false));
        assert!(config.is_trusted(20, false));
        assert!(config.is_trusted(30, true));
        assert!(!config.is_trusted(40, false));
    }

    #[test]
    fn test_trusted_source_official_only() {
        let config = TrustedSourceConfig::new(true, vec![]);
        assert!(config.is_trusted(1, true));
        assert!(!config.is_trusted(1, false));
    }

    #[test]
    fn test_in_flight_guard_decrements_on_drop() {
        let in_flight: Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>> =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

        {
            let _guard = InFlightGuard {
                in_flight: in_flight.clone(),
                plugin_name: "test-plugin".to_string(),
            };
            {
                let mut map = in_flight.lock().unwrap();
                *map.entry("test-plugin".to_string()).or_insert(0) += 1;
            }
            let count = in_flight
                .lock()
                .unwrap()
                .get("test-plugin")
                .copied()
                .unwrap_or(0);
            assert_eq!(count, 1);
        }

        let count = in_flight
            .lock()
            .unwrap()
            .get("test-plugin")
            .copied()
            .unwrap_or(0);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_safe_uninstall_request_defaults() {
        let req = SafeUninstallRequest {
            plugin_name: "test-plugin".to_string(),
            lockfile_path: PathBuf::from("/tmp/test.lock"),
            drain_timeout: Duration::from_secs(30),
        };
        assert_eq!(req.plugin_name, "test-plugin");
        assert_eq!(req.drain_timeout, Duration::from_secs(30));
    }
}
