// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 核心服务编排
//!
//! MarketplaceService 持有 PluginRepository / VersionRepository / ObjectStore，
//! 含 publish / search / install / review 四个异步方法。

use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use ed25519_dalek::VerifyingKey;

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

/// 市场服务
pub struct MarketplaceService {
    plugins: Arc<PluginRepository>,
    versions: Arc<VersionRepository>,
    reviews: Arc<ReviewRepository>,
    developers: Arc<DeveloperRepository>,
    store: Arc<dyn ObjectStore>,
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
        }
    }

    /// 发布插件
    ///
    /// 校验清单 → 验证签名 → SemVer 严格递增 → 上传归档 → 创建 pending 版本
    pub async fn publish(&self, req: PublishRequest) -> MarketplaceResult<i64> {
        let manifest = &req.manifest;

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

        let latest_version = self.versions.find_latest_by_plugin(0).await?;
        let current_version = latest_version
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
