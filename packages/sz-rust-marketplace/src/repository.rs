// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据访问层
//!
//! 五个 Repository：Plugin / Version / Review / Install / Developer
//! 所有 WHERE 条件参数化绑定，禁止 SELECT *（显式列投影）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};

use crate::error::{MarketplaceError, MarketplaceResult};

// ── 实体结构体 ──

/// 插件实体
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Plugin {
    pub id: i64,
    pub name: String,
    pub identifier: String,
    pub title: String,
    pub author: String,
    pub homepage: Option<String>,
    pub license: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub price: f64,
    pub developer_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 插件版本实体
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PluginVersion {
    pub id: i64,
    pub plugin_id: i64,
    pub version: String,
    pub archive_key: String,
    pub sha256: String,
    pub signature: String,
    pub review_status: String,
    pub changelog: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 审核记录实体
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ReviewRecord {
    pub id: i64,
    pub version_id: i64,
    pub reviewer_id: i64,
    pub decision: String,
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 安装记录实体
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct InstallRecord {
    pub id: i64,
    pub plugin_id: i64,
    pub version_id: i64,
    pub installer_id: i64,
    pub installed_at: DateTime<Utc>,
}

/// 开发者实体
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Developer {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub public_key: String,
    pub is_reviewer: bool,
    pub created_at: DateTime<Utc>,
}

// ── PluginRepository ──

/// 插件仓库
pub struct PluginRepository {
    pool: PgPool,
}

impl PluginRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, plugin: &Plugin) -> MarketplaceResult<Plugin> {
        let row = sqlx::query_as::<_, Plugin>(
            r#"INSERT INTO plugins (name, identifier, title, author, homepage, license, description, tags, price, developer_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING id, name, identifier, title, author, homepage, license, description, tags, price, developer_id, created_at, updated_at"#,
        )
        .bind(&plugin.name)
        .bind(&plugin.identifier)
        .bind(&plugin.title)
        .bind(&plugin.author)
        .bind(&plugin.homepage)
        .bind(&plugin.license)
        .bind(&plugin.description)
        .bind(&plugin.tags)
        .bind(plugin.price)
        .bind(plugin.developer_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Plugin create: {e}")))?;
        Ok(row)
    }

    pub async fn find_by_id(&self, id: i64) -> MarketplaceResult<Option<Plugin>> {
        let row = sqlx::query_as::<_, Plugin>(
            r#"SELECT id, name, identifier, title, author, homepage, license, description, tags, price, developer_id, created_at, updated_at
            FROM plugins WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Plugin find_by_id: {e}")))?;
        Ok(row)
    }

    pub async fn find_by_name(&self, name: &str) -> MarketplaceResult<Option<Plugin>> {
        let row = sqlx::query_as::<_, Plugin>(
            r#"SELECT id, name, identifier, title, author, homepage, license, description, tags, price, developer_id, created_at, updated_at
            FROM plugins WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Plugin find_by_name: {e}")))?;
        Ok(row)
    }

    pub async fn search(
        &self,
        keyword: Option<&str>,
        tag: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> MarketplaceResult<Vec<Plugin>> {
        let rows = sqlx::query_as::<_, Plugin>(
            r#"SELECT id, name, identifier, title, author, homepage, license, description, tags, price, developer_id, created_at, updated_at
            FROM plugins
            WHERE ($1::text IS NULL OR title ILIKE '%' || $1 || '%' OR description ILIKE '%' || $1 || '%')
              AND ($2::text IS NULL OR $2 = ANY(tags))
            ORDER BY id
            LIMIT $3 OFFSET $4"#,
        )
        .bind(keyword)
        .bind(tag)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Plugin search: {e}")))?;
        Ok(rows)
    }
}

// ── VersionRepository ──

/// 版本仓库
pub struct VersionRepository {
    pool: PgPool,
}

impl VersionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, version: &PluginVersion) -> MarketplaceResult<PluginVersion> {
        let row = sqlx::query_as::<_, PluginVersion>(
            r#"INSERT INTO plugin_versions (plugin_id, version, archive_key, sha256, signature, review_status, changelog)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, plugin_id, version, archive_key, sha256, signature, review_status, changelog, created_at"#,
        )
        .bind(version.plugin_id)
        .bind(&version.version)
        .bind(&version.archive_key)
        .bind(&version.sha256)
        .bind(&version.signature)
        .bind(&version.review_status)
        .bind(&version.changelog)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Version create: {e}")))?;
        Ok(row)
    }

    pub async fn find_latest_by_plugin(
        &self,
        plugin_id: i64,
    ) -> MarketplaceResult<Option<PluginVersion>> {
        let row = sqlx::query_as::<_, PluginVersion>(
            r#"SELECT id, plugin_id, version, archive_key, sha256, signature, review_status, changelog, created_at
            FROM plugin_versions WHERE plugin_id = $1 ORDER BY created_at DESC LIMIT 1"#,
        )
        .bind(plugin_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Version find_latest: {e}")))?;
        Ok(row)
    }

    pub async fn update_review_status(
        &self,
        version_id: i64,
        status: &str,
    ) -> MarketplaceResult<PluginVersion> {
        let row = sqlx::query_as::<_, PluginVersion>(
            r#"UPDATE plugin_versions SET review_status = $2 WHERE id = $1
            RETURNING id, plugin_id, version, archive_key, sha256, signature, review_status, changelog, created_at"#,
        )
        .bind(version_id)
        .bind(status)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Version update_status: {e}")))?;
        Ok(row)
    }
}

// ── ReviewRepository ──

/// 审核仓库
pub struct ReviewRepository {
    pool: PgPool,
}

impl ReviewRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, review: &ReviewRecord) -> MarketplaceResult<ReviewRecord> {
        let row = sqlx::query_as::<_, ReviewRecord>(
            r#"INSERT INTO review_records (version_id, reviewer_id, decision, comment)
            VALUES ($1, $2, $3, $4)
            RETURNING id, version_id, reviewer_id, decision, comment, created_at"#,
        )
        .bind(review.version_id)
        .bind(review.reviewer_id)
        .bind(&review.decision)
        .bind(&review.comment)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Review create: {e}")))?;
        Ok(row)
    }

    pub async fn find_by_version(&self, version_id: i64) -> MarketplaceResult<Vec<ReviewRecord>> {
        let rows = sqlx::query_as::<_, ReviewRecord>(
            r#"SELECT id, version_id, reviewer_id, decision, comment, created_at
            FROM review_records WHERE version_id = $1 ORDER BY created_at"#,
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Review find_by_version: {e}")))?;
        Ok(rows)
    }
}

// ── InstallRepository ──

/// 安装仓库
pub struct InstallRepository {
    pool: PgPool,
}

impl InstallRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, install: &InstallRecord) -> MarketplaceResult<InstallRecord> {
        let row = sqlx::query_as::<_, InstallRecord>(
            r#"INSERT INTO install_records (plugin_id, version_id, installer_id)
            VALUES ($1, $2, $3)
            RETURNING id, plugin_id, version_id, installer_id, installed_at"#,
        )
        .bind(install.plugin_id)
        .bind(install.version_id)
        .bind(install.installer_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Install create: {e}")))?;
        Ok(row)
    }
}

// ── DeveloperRepository ──

/// 开发者仓库
pub struct DeveloperRepository {
    pool: PgPool,
}

impl DeveloperRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn find_by_id(&self, id: i64) -> MarketplaceResult<Option<Developer>> {
        let row = sqlx::query_as::<_, Developer>(
            r#"SELECT id, username, email, public_key, is_reviewer, created_at
            FROM developers WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Developer find_by_id: {e}")))?;
        Ok(row)
    }

    pub async fn find_by_username(&self, username: &str) -> MarketplaceResult<Option<Developer>> {
        let row = sqlx::query_as::<_, Developer>(
            r#"SELECT id, username, email, public_key, is_reviewer, created_at
            FROM developers WHERE username = $1"#,
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MarketplaceError::InternalError(format!("Developer find_by_username: {e}")))?;
        Ok(row)
    }
}
