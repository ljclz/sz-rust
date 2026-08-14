use crate::common::AiError;
use async_trait::async_trait;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorRecord {
    pub id: String,
    pub vector: Vec<f32>,
    pub metadata: serde_json::Value,
    pub tenant_id: String,
}

impl VectorRecord {
    pub fn new(id: impl Into<String>, vector: Vec<f32>, tenant_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            vector,
            metadata: serde_json::Value::Null,
            tenant_id: tenant_id.into(),
        }
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorHit {
    pub id: String,
    pub score: f32,
    pub metadata: serde_json::Value,
    pub text: String,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SimilarityMetric {
    Cosine,
    Dot,
    L2,
}

#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert(&self, records: &[VectorRecord]) -> Result<(), AiError>;

    async fn query(
        &self,
        vec: &[f32],
        topk: usize,
        metric: SimilarityMetric,
        tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError>;

    async fn delete(&self, ids: &[&str], tenant: &str) -> Result<(), AiError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_record_new() {
        let rec = VectorRecord::new("v1", vec![1.0, 2.0, 3.0], "tenant-a");
        assert_eq!(rec.id, "v1");
        assert_eq!(rec.vector, vec![1.0, 2.0, 3.0]);
        assert_eq!(rec.tenant_id, "tenant-a");
        assert!(rec.metadata.is_null());
    }

    #[test]
    fn vector_record_with_metadata() {
        let rec = VectorRecord::new("v1", vec![1.0], "t1")
            .with_metadata(serde_json::json!({"source": "doc1"}));
        assert_eq!(rec.metadata["source"], "doc1");
    }

    #[test]
    fn similarity_metric_serde() {
        let json = serde_json::to_string(&SimilarityMetric::Cosine).unwrap();
        assert_eq!(json, "\"cosine\"");
        let json = serde_json::to_string(&SimilarityMetric::Dot).unwrap();
        assert_eq!(json, "\"dot\"");
        let json = serde_json::to_string(&SimilarityMetric::L2).unwrap();
        assert_eq!(json, "\"l2\"");
    }

    #[test]
    fn vector_hit_serde_roundtrip() {
        let hit = VectorHit {
            id: "h1".into(),
            score: 0.95,
            metadata: serde_json::json!({"page": 1}),
            text: "sample".into(),
        };
        let json = serde_json::to_string(&hit).unwrap();
        let de: VectorHit = serde_json::from_str(&json).unwrap();
        assert_eq!(de.id, "h1");
        assert!((de.score - 0.95).abs() < 1e-6);
    }

    #[test]
    fn vector_record_serde_roundtrip() {
        let rec = VectorRecord::new("v1", vec![1.0, 2.0], "t1")
            .with_metadata(serde_json::json!({"k": "v"}));
        let json = serde_json::to_string(&rec).unwrap();
        let de: VectorRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(de.id, "v1");
        assert_eq!(de.vector, vec![1.0, 2.0]);
        assert_eq!(de.tenant_id, "t1");
    }
}
