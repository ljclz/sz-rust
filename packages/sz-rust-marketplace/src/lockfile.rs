// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 锁文件管理
//!
//! LockfileManager 管理 `plugins.lock`，保证可复现安装。

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{MarketplaceError, MarketplaceResult};

/// 锁文件条目（单个已安装插件）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileEntry {
    /// 插件名
    pub name: String,
    /// 版本
    pub version: String,
    /// SHA256 校验和
    pub sha256: String,
    /// Ed25519 签名
    pub signature: String,
    /// 安装时间
    pub installed_at: DateTime<Utc>,
}

/// 锁文件
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Lockfile {
    /// 锁文件版本
    pub version: u32,
    /// 已安装插件列表
    pub plugins: Vec<LockfileEntry>,
}

/// 锁文件管理器
pub struct LockfileManager;

impl LockfileManager {
    /// 读取锁文件
    pub async fn read(path: &Path) -> MarketplaceResult<Lockfile> {
        if !tokio::fs::try_exists(path).await.unwrap_or(false) {
            return Ok(Lockfile::default());
        }
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("读取锁文件失败: {e}")))?;
        let lockfile: Lockfile = serde_json::from_str(&content)
            .map_err(|e| MarketplaceError::InvalidManifest(format!("锁文件解析失败: {e}")))?;
        Ok(lockfile)
    }

    /// 写入锁文件
    pub async fn write(path: &Path, lockfile: &Lockfile) -> MarketplaceResult<()> {
        let content = serde_json::to_string_pretty(lockfile)
            .map_err(|e| MarketplaceError::InternalError(format!("锁文件序列化失败: {e}")))?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| MarketplaceError::InternalError(format!("创建目录失败: {e}")))?;
            }
        }
        tokio::fs::write(path, content)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("写入锁文件失败: {e}")))?;
        Ok(())
    }

    /// 更新锁文件中的单个插件条目（存在则替换，不存在则追加）
    pub async fn update(path: &Path, entry: LockfileEntry) -> MarketplaceResult<()> {
        let mut lockfile = Self::read(path).await?;
        if let Some(existing) = lockfile.plugins.iter_mut().find(|e| e.name == entry.name) {
            *existing = entry;
        } else {
            lockfile.plugins.push(entry);
        }
        Self::write(path, &lockfile).await
    }

    /// 移除锁文件中的插件条目
    pub async fn remove(path: &Path, name: &str) -> MarketplaceResult<()> {
        let mut lockfile = Self::read(path).await?;
        lockfile.plugins.retain(|e| e.name != name);
        Self::write(path, &lockfile).await
    }

    /// 获取锁文件路径（默认 `~/.sz-rust/plugins.lock`）
    pub fn default_path() -> MarketplaceResult<PathBuf> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_err(|_| MarketplaceError::InternalError("无法确定 HOME 目录".to_string()))?;
        Ok(PathBuf::from(home).join(".sz-rust").join("plugins.lock"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lockfile_write_read_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("plugins.lock");

        let lockfile = Lockfile {
            version: 1,
            plugins: vec![LockfileEntry {
                name: "crm".to_string(),
                version: "1.0.0".to_string(),
                sha256: "abc123".to_string(),
                signature: "sig".to_string(),
                installed_at: Utc::now(),
            }],
        };

        LockfileManager::write(&path, &lockfile).await.unwrap();
        let read = LockfileManager::read(&path).await.unwrap();
        assert_eq!(read.version, 1);
        assert_eq!(read.plugins.len(), 1);
        assert_eq!(read.plugins[0].name, "crm");
    }

    #[tokio::test]
    async fn test_lockfile_update_existing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("plugins.lock");

        let entry1 = LockfileEntry {
            name: "cms".to_string(),
            version: "1.0.0".to_string(),
            sha256: "hash1".to_string(),
            signature: "sig1".to_string(),
            installed_at: Utc::now(),
        };
        LockfileManager::update(&path, entry1).await.unwrap();

        let entry2 = LockfileEntry {
            name: "cms".to_string(),
            version: "2.0.0".to_string(),
            sha256: "hash2".to_string(),
            signature: "sig2".to_string(),
            installed_at: Utc::now(),
        };
        LockfileManager::update(&path, entry2).await.unwrap();

        let lockfile = LockfileManager::read(&path).await.unwrap();
        assert_eq!(lockfile.plugins.len(), 1);
        assert_eq!(lockfile.plugins[0].version, "2.0.0");
    }

    #[tokio::test]
    async fn test_lockfile_remove() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("plugins.lock");

        LockfileManager::update(
            &path,
            LockfileEntry {
                name: "crm".to_string(),
                version: "1.0.0".to_string(),
                sha256: "hash".to_string(),
                signature: "sig".to_string(),
                installed_at: Utc::now(),
            },
        )
        .await
        .unwrap();

        LockfileManager::remove(&path, "crm").await.unwrap();
        let lockfile = LockfileManager::read(&path).await.unwrap();
        assert!(lockfile.plugins.is_empty());
    }

    #[tokio::test]
    async fn test_lockfile_read_nonexistent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nonexistent.lock");
        let lockfile = LockfileManager::read(&path).await.unwrap();
        assert!(lockfile.plugins.is_empty());
    }
}
