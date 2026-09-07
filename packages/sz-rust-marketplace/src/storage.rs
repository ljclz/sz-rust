// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 对象存储抽象

use async_trait::async_trait;
use bytes::Bytes;

use crate::error::{MarketplaceError, MarketplaceResult};

/// 对象存储 trait
#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// 上传对象，返回 SHA256 校验和
    async fn upload(&self, key: &str, data: Bytes) -> MarketplaceResult<String>;

    /// 下载对象（可选 Range 支持）
    async fn download(&self, key: &str, range: Option<(u64, u64)>) -> MarketplaceResult<Bytes>;

    /// 删除对象
    async fn delete(&self, key: &str) -> MarketplaceResult<()>;
}

/// 本地对象存储
pub struct LocalObjectStore {
    root: std::path::PathBuf,
}

impl LocalObjectStore {
    /// 创建本地存储（root 目录自动创建）
    pub fn new(root: std::path::PathBuf) -> Self {
        Self { root }
    }

    fn full_path(&self, key: &str) -> std::path::PathBuf {
        self.root.join(key)
    }
}

#[async_trait]
impl ObjectStore for LocalObjectStore {
    async fn upload(&self, key: &str, data: Bytes) -> MarketplaceResult<String> {
        let path = self.full_path(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| MarketplaceError::InternalError(format!("创建目录失败: {e}")))?;
        }
        let checksum = crate::signature::SignatureService::sha256_checksum(&data);
        tokio::fs::write(&path, &data)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("写入文件失败: {e}")))?;
        Ok(checksum)
    }

    async fn download(&self, key: &str, range: Option<(u64, u64)>) -> MarketplaceResult<Bytes> {
        let path = self.full_path(key);
        if range.is_some() {
            return Err(MarketplaceError::InternalError(
                "LocalObjectStore 暂不支持 Range 下载".to_string(),
            ));
        }
        let data = tokio::fs::read(&path)
            .await
            .map_err(|e| MarketplaceError::DownloadFailed(format!("读取文件失败: {e}")))?;
        Ok(Bytes::from(data))
    }

    async fn delete(&self, key: &str) -> MarketplaceResult<()> {
        let path = self.full_path(key);
        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("删除文件失败: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_store_upload_download_delete() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalObjectStore::new(temp.path().to_path_buf());

        let data = Bytes::from(b"hello world".to_vec());
        let checksum = store.upload("test/file.txt", data.clone()).await.unwrap();
        assert_eq!(checksum.len(), 64);

        let downloaded = store.download("test/file.txt", None).await.unwrap();
        assert_eq!(downloaded, data);

        store.delete("test/file.txt").await.unwrap();
        let result = store.download("test/file.txt", None).await;
        assert!(result.is_err());
    }
}
