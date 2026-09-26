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

/// 校验存储 key 可安全拼入 root 路径（词法防线）
///
/// 拒绝空 key、反斜杠、NUL、绝对路径以及 `..`/`.` 等非常规组件，
/// 防止 key 逃逸存储根目录（路径穿越）。
fn validate_key(key: &str) -> MarketplaceResult<()> {
    if key.is_empty() || key.len() > 512 {
        return Err(MarketplaceError::InvalidManifest(format!(
            "存储 key 长度必须在 1..=512: len={}",
            key.len()
        )));
    }
    if key.contains('\\') || key.contains('\0') {
        return Err(MarketplaceError::InvalidManifest(format!(
            "存储 key 含非法字符: {key:?}"
        )));
    }
    let path = std::path::Path::new(key);
    for comp in path.components() {
        if !matches!(comp, std::path::Component::Normal(_)) {
            return Err(MarketplaceError::InvalidManifest(format!(
                "存储 key 含非法路径组件（拒绝 .. / . / 绝对路径）: {key:?}"
            )));
        }
    }
    Ok(())
}

impl LocalObjectStore {
    /// 创建本地存储（root 目录自动创建）
    pub fn new(root: std::path::PathBuf) -> Self {
        Self { root }
    }

    fn full_path(&self, key: &str) -> std::path::PathBuf {
        self.root.join(key)
    }

    /// 规范化路径校验（纵深防线）：path 必须位于规范 root 之内
    ///
    /// 拦截词法校验无法覆盖的符号链接/大小写变体逃逸。
    async fn ensure_within_root(&self, path: &std::path::Path) -> MarketplaceResult<()> {
        let target = tokio::fs::canonicalize(path)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("路径规范化失败: {e}")))?;
        let root = tokio::fs::canonicalize(&self.root)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("存储根目录规范化失败: {e}")))?;
        if !target.starts_with(&root) {
            return Err(MarketplaceError::InternalError(format!(
                "存储 key 越界: {key_trace} 逃逸 root",
                key_trace = path.display()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl ObjectStore for LocalObjectStore {
    async fn upload(&self, key: &str, data: Bytes) -> MarketplaceResult<String> {
        validate_key(key)?;
        let path = self.full_path(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| MarketplaceError::InternalError(format!("创建目录失败: {e}")))?;
            self.ensure_within_root(parent).await?;
        }
        let checksum = crate::signature::SignatureService::sha256_checksum(&data);
        tokio::fs::write(&path, &data)
            .await
            .map_err(|e| MarketplaceError::InternalError(format!("写入文件失败: {e}")))?;
        Ok(checksum)
    }

    async fn download(&self, key: &str, range: Option<(u64, u64)>) -> MarketplaceResult<Bytes> {
        validate_key(key)?;
        let path = self.full_path(key);
        self.ensure_within_root(&path).await?;
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
        validate_key(key)?;
        let path = self.full_path(key);
        self.ensure_within_root(&path).await?;
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

    #[tokio::test]
    async fn test_local_store_range_unsupported() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalObjectStore::new(temp.path().to_path_buf());
        store
            .upload("f.txt", Bytes::from(b"data".to_vec()))
            .await
            .unwrap();
        let result = store.download("f.txt", Some((0, 2))).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_local_store_upload_empty() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalObjectStore::new(temp.path().to_path_buf());
        let checksum = store.upload("empty.txt", Bytes::new()).await.unwrap();
        assert_eq!(checksum.len(), 64);
        let downloaded = store.download("empty.txt", None).await.unwrap();
        assert!(downloaded.is_empty());
    }

    #[tokio::test]
    async fn test_local_store_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalObjectStore::new(temp.path().to_path_buf());
        store
            .upload("f.txt", Bytes::from(b"v1".to_vec()))
            .await
            .unwrap();
        store
            .upload("f.txt", Bytes::from(b"v2".to_vec()))
            .await
            .unwrap();
        let downloaded = store.download("f.txt", None).await.unwrap();
        assert_eq!(downloaded, Bytes::from(b"v2".to_vec()));
    }

    #[tokio::test]
    async fn test_local_store_delete_nonexistent() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalObjectStore::new(temp.path().to_path_buf());
        let result = store.delete("no_such_file.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_local_store_download_nonexistent() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalObjectStore::new(temp.path().to_path_buf());
        let result = store.download("no_such_file.txt", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_local_store_nested_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalObjectStore::new(temp.path().to_path_buf());
        store
            .upload("a/b/c/deep.txt", Bytes::from(b"deep".to_vec()))
            .await
            .unwrap();
        let downloaded = store.download("a/b/c/deep.txt", None).await.unwrap();
        assert_eq!(downloaded, Bytes::from(b"deep".to_vec()));
    }

    #[tokio::test]
    async fn test_local_store_rejects_traversal_keys() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let store = LocalObjectStore::new(root.clone());
        let evil_keys = [
            "../evil_outside.txt",
            "../../evil.txt",
            "a/../../evil.txt",
            "..\\evil.txt",
            "/abs/path.txt",
            "a/../b.txt",
            ".",
            "..",
        ];
        for key in evil_keys {
            assert!(
                store.upload(key, Bytes::from(b"x".to_vec())).await.is_err(),
                "upload 应拒绝穿越 key: {key}"
            );
            assert!(
                store.download(key, None).await.is_err(),
                "download 应拒绝穿越 key: {key}"
            );
            assert!(
                store.delete(key).await.is_err(),
                "delete 应拒绝穿越 key: {key}"
            );
        }
        // 穿越 key 若未被拦截，会写到 root 之外的确定位置 —— 断言其不存在
        assert!(!temp.path().join("evil_outside.txt").exists());
        assert!(!root.join("a").exists());
    }

    #[tokio::test]
    async fn test_local_store_rejects_empty_and_nul_keys() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalObjectStore::new(temp.path().to_path_buf());
        assert!(store.upload("", Bytes::new()).await.is_err());
        assert!(store.upload("a\0b.txt", Bytes::new()).await.is_err());
    }

    #[tokio::test]
    async fn test_local_store_allows_normal_keys_after_hardening() {
        let temp = tempfile::tempdir().unwrap();
        let store = LocalObjectStore::new(temp.path().to_path_buf());
        let checksum = store
            .upload("plug/1.0.0/plug.tar.gz", Bytes::from(b"ok".to_vec()))
            .await
            .unwrap();
        assert_eq!(checksum.len(), 64);
    }
}
