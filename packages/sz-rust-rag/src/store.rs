//! 版本化存储泛型 + 文件实现。

use crate::error::{RagError, RagResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// 版本化存储 trait。
#[async_trait]
pub trait VersionedStore<T>: Send + Sync {
    async fn add(&self, key: &str, item: T, tenant: &str, updated_by: &str) -> RagResult<T>;
    async fn update(&self, key: &str, item: T, tenant: &str, updated_by: &str) -> RagResult<T>;
    async fn delete(&self, key: &str, tenant: &str) -> RagResult<()>;
    async fn get(&self, key: &str, tenant: &str) -> RagResult<Option<T>>;
    async fn list(&self, tenant: &str) -> RagResult<Vec<T>>;
    async fn history(&self, key: &str, tenant: &str) -> RagResult<Vec<T>>;
    async fn get_version(&self, key: &str, version: u64, tenant: &str) -> RagResult<Option<T>>;
}

/// 带版本的条目。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VersionedEntry<T> {
    pub key: String,
    pub version: u64,
    pub item: T,
    pub updated_by: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// 内存 + 文件持久化版本化存储。
pub struct FileVersionedStore<T> {
    data: Arc<RwLock<HashMap<String, Vec<VersionedEntry<T>>>>>,
    file_path: PathBuf,
}

impl<T> FileVersionedStore<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync,
{
    pub async fn load(file_path: &Path) -> RagResult<Self> {
        let data = if tokio::fs::try_exists(file_path).await.unwrap_or(false) {
            let content = tokio::fs::read_to_string(file_path).await?;
            if content.trim().is_empty() {
                HashMap::new()
            } else {
                serde_json::from_str(&content).map_err(RagError::Json)?
            }
        } else {
            HashMap::new()
        };
        Ok(Self {
            data: Arc::new(RwLock::new(data)),
            file_path: file_path.to_path_buf(),
        })
    }

    pub fn new_in_memory() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            file_path: PathBuf::new(),
        }
    }

    async fn persist(&self, data: &HashMap<String, Vec<VersionedEntry<T>>>) -> RagResult<()> {
        if self.file_path.as_os_str().is_empty() {
            return Ok(());
        }
        let json = serde_json::to_string(data).map_err(RagError::Json)?;
        let tmp = self.file_path.with_extension("json.tmp");
        tokio::fs::write(&tmp, json.as_bytes()).await?;
        tokio::fs::rename(&tmp, &self.file_path).await?;
        Ok(())
    }

    fn tenant_key(key: &str, tenant: &str) -> String {
        format!("{}::{}", tenant, key)
    }
}

#[async_trait]
impl<T> VersionedStore<T> for FileVersionedStore<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
{
    async fn add(&self, key: &str, item: T, tenant: &str, updated_by: &str) -> RagResult<T> {
        let mut data = self.data.write().await;
        let tk = Self::tenant_key(key, tenant);
        if data.contains_key(&tk) {
            return Err(RagError::Internal(format!("key already exists: {}", key)));
        }
        let entry = VersionedEntry {
            key: key.to_string(),
            version: 1,
            item: item.clone(),
            updated_by: updated_by.to_string(),
            updated_at: chrono::Utc::now(),
        };
        data.insert(tk, vec![entry]);
        self.persist(&data).await?;
        Ok(item)
    }

    async fn update(&self, key: &str, item: T, tenant: &str, updated_by: &str) -> RagResult<T> {
        let mut data = self.data.write().await;
        let tk = Self::tenant_key(key, tenant);
        let versions = data
            .get_mut(&tk)
            .ok_or_else(|| RagError::Internal(format!("key not found: {}", key)))?;
        let next_version = versions.last().map(|e| e.version + 1).unwrap_or(1);
        versions.push(VersionedEntry {
            key: key.to_string(),
            version: next_version,
            item: item.clone(),
            updated_by: updated_by.to_string(),
            updated_at: chrono::Utc::now(),
        });
        self.persist(&data).await?;
        Ok(item)
    }

    async fn delete(&self, key: &str, tenant: &str) -> RagResult<()> {
        let mut data = self.data.write().await;
        let tk = Self::tenant_key(key, tenant);
        data.remove(&tk);
        self.persist(&data).await?;
        Ok(())
    }

    async fn get(&self, key: &str, tenant: &str) -> RagResult<Option<T>> {
        let data = self.data.read().await;
        let tk = Self::tenant_key(key, tenant);
        Ok(data.get(&tk).and_then(|v| v.last()).map(|e| e.item.clone()))
    }

    async fn list(&self, tenant: &str) -> RagResult<Vec<T>> {
        let data = self.data.read().await;
        let prefix = format!("{}::", tenant);
        let mut result = Vec::new();
        for (k, versions) in data.iter() {
            if k.starts_with(&prefix) {
                if let Some(e) = versions.last() {
                    result.push(e.item.clone());
                }
            }
        }
        Ok(result)
    }

    async fn history(&self, key: &str, tenant: &str) -> RagResult<Vec<T>> {
        let data = self.data.read().await;
        let tk = Self::tenant_key(key, tenant);
        Ok(data
            .get(&tk)
            .map(|v| v.iter().map(|e| e.item.clone()).collect())
            .unwrap_or_default())
    }

    async fn get_version(&self, key: &str, version: u64, tenant: &str) -> RagResult<Option<T>> {
        let data = self.data.read().await;
        let tk = Self::tenant_key(key, tenant);
        Ok(data
            .get(&tk)
            .and_then(|v| v.iter().find(|e| e.version == version))
            .map(|e| e.item.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_get_update_history() {
        let store: FileVersionedStore<String> = FileVersionedStore::new_in_memory();
        store.add("k1", "v1".into(), "t1", "user").await.unwrap();
        let got = store.get("k1", "t1").await.unwrap();
        assert_eq!(got, Some("v1".into()));

        store.update("k1", "v2".into(), "t1", "user").await.unwrap();
        let got = store.get("k1", "t1").await.unwrap();
        assert_eq!(got, Some("v2".into()));

        let history = store.history("k1", "t1").await.unwrap();
        assert_eq!(history, vec!["v1".to_string(), "v2".to_string()]);
    }

    #[tokio::test]
    async fn get_version() {
        let store: FileVersionedStore<i32> = FileVersionedStore::new_in_memory();
        store.add("k", 1, "t", "u").await.unwrap();
        store.update("k", 2, "t", "u").await.unwrap();
        store.update("k", 3, "t", "u").await.unwrap();
        assert_eq!(store.get_version("k", 2, "t").await.unwrap(), Some(2));
        assert_eq!(store.get_version("k", 3, "t").await.unwrap(), Some(3));
        assert_eq!(store.get_version("k", 99, "t").await.unwrap(), None);
    }

    #[tokio::test]
    async fn list_and_delete() {
        let store: FileVersionedStore<String> = FileVersionedStore::new_in_memory();
        store.add("a", "1".into(), "t", "u").await.unwrap();
        store.add("b", "2".into(), "t", "u").await.unwrap();
        store.add("c", "3".into(), "t2", "u").await.unwrap();
        let list = store.list("t").await.unwrap();
        assert_eq!(list.len(), 2);
        store.delete("a", "t").await.unwrap();
        assert_eq!(store.list("t").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn duplicate_add_fails() {
        let store: FileVersionedStore<String> = FileVersionedStore::new_in_memory();
        store.add("k", "v".into(), "t", "u").await.unwrap();
        let result = store.add("k", "v2".into(), "t", "u").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn file_persistence() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        {
            let store: FileVersionedStore<String> = FileVersionedStore::load(&path).await.unwrap();
            store.add("k", "v".into(), "t", "u").await.unwrap();
        }
        let store: FileVersionedStore<String> = FileVersionedStore::load(&path).await.unwrap();
        assert_eq!(store.get("k", "t").await.unwrap(), Some("v".into()));
    }

    #[tokio::test]
    async fn load_nonexistent_file() {
        let store: FileVersionedStore<String> =
            FileVersionedStore::load(std::path::Path::new("/nonexistent/store.json"))
                .await
                .unwrap();
        assert!(store.list("t").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn update_nonexistent_key_fails() {
        let store: FileVersionedStore<String> = FileVersionedStore::new_in_memory();
        let result = store.update("nonexistent", "v".into(), "t", "u").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_nonexistent_key_returns_none() {
        let store: FileVersionedStore<String> = FileVersionedStore::new_in_memory();
        assert_eq!(store.get("nonexistent", "t").await.unwrap(), None);
    }

    #[tokio::test]
    async fn history_nonexistent_key_returns_empty() {
        let store: FileVersionedStore<String> = FileVersionedStore::new_in_memory();
        let history = store.history("nonexistent", "t").await.unwrap();
        assert!(history.is_empty());
    }

    #[tokio::test]
    async fn list_empty_tenant() {
        let store: FileVersionedStore<String> = FileVersionedStore::new_in_memory();
        assert!(store.list("empty").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_key_succeeds() {
        let store: FileVersionedStore<String> = FileVersionedStore::new_in_memory();
        assert!(store.delete("nonexistent", "t").await.is_ok());
    }
}
