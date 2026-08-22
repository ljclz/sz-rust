use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::common::AiError;
use async_trait::async_trait;

use super::{SimilarityMetric, VectorHit, VectorRecord, VectorStore};

/// 内存向量存储，支持多租户隔离与三种相似度度量。
///
/// 数据结构：`tenant_id → (record_id → VectorRecord)`，
/// 使用 `parking_lot::RwLock` 保证并发安全。
/// 适用于中小规模知识库（< 10 万条），无需外部依赖。
pub struct MemoryVectorStore {
    inner: Arc<RwLock<HashMap<String, HashMap<String, VectorRecord>>>>,
}

impl MemoryVectorStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn compute_similarity(query: &[f32], candidate: &[f32], metric: SimilarityMetric) -> f32 {
        match metric {
            SimilarityMetric::Cosine => cosine_similarity(query, candidate),
            SimilarityMetric::Dot => dot_product(query, candidate),
            SimilarityMetric::L2 => -l2_distance(query, candidate),
        }
    }

    /// 导出所有记录的快照（用于文件持久化）
    pub fn snapshot(&self) -> Vec<VectorRecord> {
        let guard = self.inner.read();
        let mut records = Vec::new();
        for tenant_map in guard.values() {
            for rec in tenant_map.values() {
                records.push(rec.clone());
            }
        }
        records
    }
}

impl Default for MemoryVectorStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VectorStore for MemoryVectorStore {
    async fn upsert(&self, records: &[VectorRecord]) -> Result<(), AiError> {
        let mut guard = self.inner.write();
        for rec in records {
            let tenant_map = guard.entry(rec.tenant_id.clone()).or_default();
            tenant_map.insert(rec.id.clone(), rec.clone());
        }
        Ok(())
    }

    async fn query(
        &self,
        vec: &[f32],
        topk: usize,
        metric: SimilarityMetric,
        tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError> {
        let guard = self.inner.read();
        let tenant_map = match guard.get(tenant) {
            Some(m) => m,
            None => return Ok(vec![]),
        };

        let mut hits: Vec<VectorHit> = tenant_map
            .values()
            .map(|rec| {
                let score = Self::compute_similarity(vec, &rec.vector, metric);
                let text = rec
                    .metadata
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                VectorHit {
                    id: rec.id.clone(),
                    score,
                    metadata: rec.metadata.clone(),
                    text,
                }
            })
            .collect();

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(topk);
        Ok(hits)
    }

    async fn delete(&self, ids: &[&str], tenant: &str) -> Result<(), AiError> {
        let mut guard = self.inner.write();
        if let Some(tenant_map) = guard.get_mut(tenant) {
            for id in ids {
                tenant_map.remove(*id);
            }
        }
        Ok(())
    }
}

fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn l2_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot = dot_product(a, b);
    let norm_a = dot_product(a, a).sqrt();
    let norm_b = dot_product(b, b).sqrt();
    if norm_a < 1e-10 || norm_b < 1e-10 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(id: &str, vector: Vec<f32>, tenant: &str, text: &str) -> VectorRecord {
        VectorRecord::new(id, vector, tenant).with_metadata(serde_json::json!({"text": text}))
    }

    #[tokio::test]
    async fn upsert_and_query_cosine() {
        let store = MemoryVectorStore::new();
        let records = vec![
            make_record("r1", vec![1.0, 0.0, 0.0], "t1", "alpha"),
            make_record("r2", vec![0.0, 1.0, 0.0], "t1", "beta"),
            make_record("r3", vec![1.0, 0.1, 0.0], "t1", "gamma"),
        ];
        store.upsert(&records).await.unwrap();

        let hits = store
            .query(&[1.0, 0.0, 0.0], 2, SimilarityMetric::Cosine, "t1")
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "r1");
        assert!((hits[0].score - 1.0).abs() < 1e-6);
        assert_eq!(hits[1].id, "r3");
    }

    #[tokio::test]
    async fn upsert_overwrites_existing() {
        let store = MemoryVectorStore::new();
        store
            .upsert(&[make_record("r1", vec![1.0, 0.0], "t1", "old")])
            .await
            .unwrap();
        store
            .upsert(&[make_record("r1", vec![0.0, 1.0], "t1", "new")])
            .await
            .unwrap();

        let hits = store
            .query(&[0.0, 1.0], 1, SimilarityMetric::Cosine, "t1")
            .await
            .unwrap();
        assert_eq!(hits[0].id, "r1");
        assert_eq!(hits[0].text, "new");
    }

    #[tokio::test]
    async fn tenant_isolation() {
        let store = MemoryVectorStore::new();
        store
            .upsert(&[make_record("r1", vec![1.0], "tenant-a", "a")])
            .await
            .unwrap();
        store
            .upsert(&[make_record("r2", vec![1.0], "tenant-b", "b")])
            .await
            .unwrap();

        let hits_a = store
            .query(&[1.0], 10, SimilarityMetric::Cosine, "tenant-a")
            .await
            .unwrap();
        assert_eq!(hits_a.len(), 1);
        assert_eq!(hits_a[0].id, "r1");

        let hits_b = store
            .query(&[1.0], 10, SimilarityMetric::Cosine, "tenant-b")
            .await
            .unwrap();
        assert_eq!(hits_b.len(), 1);
        assert_eq!(hits_b[0].id, "r2");
    }

    #[tokio::test]
    async fn delete_records() {
        let store = MemoryVectorStore::new();
        store
            .upsert(&[
                make_record("r1", vec![1.0], "t1", "a"),
                make_record("r2", vec![1.0], "t1", "b"),
            ])
            .await
            .unwrap();
        store.delete(&["r1"], "t1").await.unwrap();

        let hits = store
            .query(&[1.0], 10, SimilarityMetric::Cosine, "t1")
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "r2");
    }

    #[tokio::test]
    async fn query_empty_tenant() {
        let store = MemoryVectorStore::new();
        let hits = store
            .query(&[1.0], 10, SimilarityMetric::Cosine, "nonexistent")
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn dot_metric() {
        let store = MemoryVectorStore::new();
        store
            .upsert(&[make_record("r1", vec![3.0, 4.0], "t1", "x")])
            .await
            .unwrap();
        let hits = store
            .query(&[1.0, 1.0], 1, SimilarityMetric::Dot, "t1")
            .await
            .unwrap();
        assert!((hits[0].score - 7.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn l2_metric() {
        let store = MemoryVectorStore::new();
        store
            .upsert(&[make_record("r1", vec![1.0, 1.0], "t1", "x")])
            .await
            .unwrap();
        let hits = store
            .query(&[4.0, 1.0], 1, SimilarityMetric::L2, "t1")
            .await
            .unwrap();
        assert!((hits[0].score - (-3.0)).abs() < 1e-6);
    }

    #[tokio::test]
    async fn topk_truncation() {
        let store = MemoryVectorStore::new();
        let records: Vec<VectorRecord> = (0..5)
            .map(|i| make_record(&format!("r{i}"), vec![i as f32, 0.0], "t1", "x"))
            .collect();
        store.upsert(&records).await.unwrap();
        let hits = store
            .query(&[4.0, 0.0], 2, SimilarityMetric::Cosine, "t1")
            .await
            .unwrap();
        assert_eq!(hits.len(), 2);
    }
}
