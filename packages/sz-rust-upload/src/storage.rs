// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! 存储抽象（spec §5.23 规则 5，§6.23 规则 5）
//!
//! 统一本地/S3/MinIO/OSS 接口，应用层无感知存储后端。

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::UploadError;

/// 存储后端配置（spec §6.23 规则 5）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum StorageBackend {
    /// 本地存储
    Local { root: PathBuf },
    /// AWS S3
    S3 { endpoint: String, bucket: String },
    /// MinIO
    MinIO { endpoint: String, bucket: String },
    /// 阿里云 OSS
    OSS { endpoint: String, bucket: String },
}

impl StorageBackend {
    /// 创建本地存储后端
    pub fn local(root: impl Into<PathBuf>) -> Self {
        Self::Local { root: root.into() }
    }

    /// 创建 S3 存储后端
    pub fn s3(endpoint: impl Into<String>, bucket: impl Into<String>) -> Self {
        Self::S3 {
            endpoint: endpoint.into(),
            bucket: bucket.into(),
        }
    }

    /// 创建 MinIO 存储后端
    pub fn minio(endpoint: impl Into<String>, bucket: impl Into<String>) -> Self {
        Self::MinIO {
            endpoint: endpoint.into(),
            bucket: bucket.into(),
        }
    }

    /// 创建 OSS 存储后端
    pub fn oss(endpoint: impl Into<String>, bucket: impl Into<String>) -> Self {
        Self::OSS {
            endpoint: endpoint.into(),
            bucket: bucket.into(),
        }
    }

    /// 获取后端名称
    pub fn backend_name(&self) -> &str {
        match self {
            Self::Local { .. } => "local",
            Self::S3 { .. } => "s3",
            Self::MinIO { .. } => "minio",
            Self::OSS { .. } => "oss",
        }
    }
}

/// 存储抽象 trait（spec §5.23 规则 5）
#[async_trait]
pub trait StorageAdapter: Send + Sync {
    /// 存储分片
    async fn store_chunk(
        &self,
        upload_id: &str,
        chunk_index: u32,
        data: &[u8],
    ) -> Result<(), UploadError>;

    /// 合并分片
    async fn merge_chunks(
        &self,
        upload_id: &str,
        total_chunks: u32,
    ) -> Result<PathBuf, UploadError>;

    /// 删除文件
    async fn delete(&self, path: &Path) -> Result<(), UploadError>;

    /// 检查文件是否存在
    async fn exists(&self, path: &Path) -> bool;
}

/// 本地存储适配器
pub struct LocalStorage {
    root: PathBuf,
}

impl LocalStorage {
    /// 创建本地存储
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 分片路径
    fn chunk_path(&self, upload_id: &str, chunk_index: u32) -> PathBuf {
        self.root
            .join(upload_id)
            .join(format!("chunk_{chunk_index}"))
    }

    /// 合并后文件路径
    fn merged_path(&self, upload_id: &str) -> PathBuf {
        self.root.join(upload_id).join("merged")
    }
}

#[async_trait]
impl StorageAdapter for LocalStorage {
    async fn store_chunk(
        &self,
        upload_id: &str,
        chunk_index: u32,
        data: &[u8],
    ) -> Result<(), UploadError> {
        let path = self.chunk_path(upload_id, chunk_index);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| UploadError::StorageError(e.to_string()))?;
        }
        tokio::fs::write(&path, data)
            .await
            .map_err(|e| UploadError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn merge_chunks(
        &self,
        upload_id: &str,
        total_chunks: u32,
    ) -> Result<PathBuf, UploadError> {
        let merged_path = self.merged_path(upload_id);
        let mut file = tokio::fs::File::create(&merged_path)
            .await
            .map_err(|e| UploadError::StorageError(e.to_string()))?;
        use tokio::io::AsyncWriteExt;
        for i in 0..total_chunks {
            let chunk_path = self.chunk_path(upload_id, i);
            let data = tokio::fs::read(&chunk_path)
                .await
                .map_err(|e| UploadError::StorageError(e.to_string()))?;
            file.write_all(&data)
                .await
                .map_err(|e| UploadError::StorageError(e.to_string()))?;
        }
        file.flush()
            .await
            .map_err(|e| UploadError::StorageError(e.to_string()))?;
        Ok(merged_path)
    }

    async fn delete(&self, path: &Path) -> Result<(), UploadError> {
        tokio::fs::remove_file(path)
            .await
            .map_err(|e| UploadError::StorageError(e.to_string()))?;
        Ok(())
    }

    async fn exists(&self, path: &Path) -> bool {
        tokio::fs::metadata(path).await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_backend_names() {
        assert_eq!(StorageBackend::local("/tmp").backend_name(), "local");
        assert_eq!(
            StorageBackend::s3("endpoint", "bucket").backend_name(),
            "s3"
        );
        assert_eq!(
            StorageBackend::minio("endpoint", "bucket").backend_name(),
            "minio"
        );
        assert_eq!(
            StorageBackend::oss("endpoint", "bucket").backend_name(),
            "oss"
        );
    }

    #[tokio::test]
    async fn test_local_storage_store_and_merge() {
        let temp_dir = std::env::temp_dir().join(format!(
            "sz-rust-upload-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = LocalStorage::new(&temp_dir);

        storage.store_chunk("upload1", 0, b"hello ").await.unwrap();
        storage.store_chunk("upload1", 1, b"world").await.unwrap();

        let merged = storage.merge_chunks("upload1", 2).await.unwrap();
        let content = tokio::fs::read_to_string(&merged).await.unwrap();
        assert_eq!(content, "hello world");

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_local_storage_exists() {
        let temp_dir = std::env::temp_dir().join(format!(
            "sz-rust-upload-test-exists-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = LocalStorage::new(&temp_dir);
        storage.store_chunk("upload1", 0, b"data").await.unwrap();
        let chunk_path = temp_dir.join("upload1").join("chunk_0");
        assert!(storage.exists(&chunk_path).await);
        let nonexistent = temp_dir.join("nonexistent");
        assert!(!storage.exists(&nonexistent).await);
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_local_storage_delete() {
        let temp_dir = std::env::temp_dir().join(format!(
            "sz-rust-upload-test-delete-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage = LocalStorage::new(&temp_dir);
        storage.store_chunk("upload1", 0, b"data").await.unwrap();
        let chunk_path = temp_dir.join("upload1").join("chunk_0");
        assert!(storage.exists(&chunk_path).await);
        storage.delete(&chunk_path).await.unwrap();
        assert!(!storage.exists(&chunk_path).await);
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
