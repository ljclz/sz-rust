use std::path::{Path, PathBuf};

use parking_lot::RwLock;

use crate::common::AiError;
use async_trait::async_trait;

use super::memory_store::MemoryVectorStore;
use super::{SimilarityMetric, VectorHit, VectorRecord, VectorStore};

/// 文件持久化向量存储，基于 `MemoryVectorStore` + JSON 原子写入。
///
/// - `upsert` / `delete` 先更新内存，再异步刷盘（原子写入：.tmp → rename）
/// - `query` 纯内存读取，无 I/O
/// - 适用于中小规模知识库且需要重启后恢复的场景
pub struct FileVectorStore {
    mem: MemoryVectorStore,
    file_path: RwLock<Option<PathBuf>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoreFile {
    records: Vec<VectorRecord>,
}

impl FileVectorStore {
    /// 创建纯内存模式（不持久化到文件）
    pub fn new_in_memory() -> Self {
        Self {
            mem: MemoryVectorStore::new(),
            file_path: RwLock::new(None),
        }
    }

    /// 创建文件持久化模式，从 `path` 加载已有数据
    pub async fn load(path: &Path) -> Result<Self, AiError> {
        let mem = MemoryVectorStore::new();
        let store = Self {
            mem,
            file_path: RwLock::new(Some(path.to_path_buf())),
        };
        store.load_from_disk().await?;
        Ok(store)
    }

    async fn load_from_disk(&self) -> Result<(), AiError> {
        let path = {
            let guard = self.file_path.read();
            match guard.clone() {
                Some(p) => p,
                None => return Ok(()),
            }
        };
        if !path.exists() {
            return Ok(());
        }
        let data = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| AiError::Internal(format!("读取向量存储文件失败: {e}")))?;
        if data.trim().is_empty() {
            return Ok(());
        }
        let file: StoreFile = serde_json::from_str(&data)
            .map_err(|e| AiError::Internal(format!("解析向量存储文件失败: {e}")))?;
        if !file.records.is_empty() {
            self.mem.upsert(&file.records).await?;
        }
        tracing::info!(
            "FileVectorStore 从 {} 加载 {} 条记录",
            path.display(),
            file.records.len()
        );
        Ok(())
    }

    async fn flush_to_disk(&self) -> Result<(), AiError> {
        let path = {
            let guard = self.file_path.read();
            match guard.clone() {
                Some(p) => p,
                None => return Ok(()),
            }
        };
        let records = self.mem.snapshot();
        let file = StoreFile { records };
        let json = serde_json::to_string(&file)
            .map_err(|e| AiError::Internal(format!("序列化向量存储失败: {e}")))?;

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| AiError::Internal(format!("创建目录失败: {e}")))?;
            }
        }
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, json.as_bytes())
            .await
            .map_err(|e| AiError::Internal(format!("写入临时文件失败: {e}")))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(|e| AiError::Internal(format!("原子重命名失败: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl VectorStore for FileVectorStore {
    async fn upsert(&self, records: &[VectorRecord]) -> Result<(), AiError> {
        self.mem.upsert(records).await?;
        self.flush_to_disk().await
    }

    async fn query(
        &self,
        vec: &[f32],
        topk: usize,
        metric: SimilarityMetric,
        tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError> {
        self.mem.query(vec, topk, metric, tenant).await
    }

    async fn delete(&self, ids: &[&str], tenant: &str) -> Result<(), AiError> {
        self.mem.delete(ids, tenant).await?;
        self.flush_to_disk().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(id: &str, vector: Vec<f32>, tenant: &str, text: &str) -> VectorRecord {
        VectorRecord::new(id, vector, tenant).with_metadata(serde_json::json!({"text": text}))
    }

    #[tokio::test]
    async fn file_store_persistence_roundtrip() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        {
            let store = FileVectorStore::load(&path).await.unwrap();
            store
                .upsert(&[
                    make_record("r1", vec![1.0, 0.0], "t1", "alpha"),
                    make_record("r2", vec![0.0, 1.0], "t1", "beta"),
                ])
                .await
                .unwrap();
        }

        let store = FileVectorStore::load(&path).await.unwrap();
        let hits = store
            .query(&[1.0, 0.0], 2, SimilarityMetric::Cosine, "t1")
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "r1");
    }

    #[tokio::test]
    async fn file_store_delete_persists() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        {
            let store = FileVectorStore::load(&path).await.unwrap();
            store
                .upsert(&[
                    make_record("r1", vec![1.0], "t1", "a"),
                    make_record("r2", vec![1.0], "t1", "b"),
                ])
                .await
                .unwrap();
            store.delete(&["r1"], "t1").await.unwrap();
        }

        let store = FileVectorStore::load(&path).await.unwrap();
        let hits = store
            .query(&[1.0], 10, SimilarityMetric::Cosine, "t1")
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "r2");
    }

    #[tokio::test]
    async fn in_memory_mode_no_file() {
        let store = FileVectorStore::new_in_memory();
        store
            .upsert(&[make_record("r1", vec![1.0], "t1", "x")])
            .await
            .unwrap();
        let hits = store
            .query(&[1.0], 1, SimilarityMetric::Cosine, "t1")
            .await
            .unwrap();
        assert_eq!(hits[0].id, "r1");
    }

    #[tokio::test]
    async fn load_nonexistent_file_ok() {
        let store = FileVectorStore::load(Path::new("/nonexistent/path/vec.json"))
            .await
            .unwrap();
        let hits = store
            .query(&[1.0], 1, SimilarityMetric::Cosine, "t1")
            .await
            .unwrap();
        assert!(hits.is_empty());
    }
}
