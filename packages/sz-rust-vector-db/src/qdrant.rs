// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Qdrant HTTP API 适配器
//!
//! 实现 [`VectorStore`] trait，通过 Qdrant REST API（默认端口 6333）进行向量存储与检索。
//!
//! ## 多租户隔离
//!
//! Collection 共享，通过 payload filter（`tenant_id` 字段）实现租户隔离。
//! - `upsert`：将 `tenant_id` 写入 payload
//! - `query`：附加 `tenant_id` must filter
//! - `delete`：附加 `tenant_id` + `record_id` must filter
//!
//! ## ID 映射
//!
//! Qdrant point ID 必须为 `uint64` 或 `UUID`。使用 UUID v5（NAMESPACE + record_id bytes）
//! 确定性映射字符串 ID → UUID，保证 upsert 幂等。
//!
//! ## 异常映射
//!
//! | Qdrant HTTP 状态码 | AiError 变体 |
//! |-------------------|-------------|
//! | 401 / 403 | `ProviderAuthFailed("qdrant")` |
//! | 404 | `Internal("collection not found")` |
//! | 429 | `RateLimited { retry_after_ms: 0 }` |
//! | 其他 4xx/5xx | `Internal("qdrant: <status> <body>")` |
//! | 网络错误 | `ProviderUnavailable("qdrant")` |

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::{SimilarityMetric, VectorHit, VectorRecord, VectorStore};
use uuid::Uuid;

/// UUID v5 命名空间（确定性映射 record_id → point_id）
const ID_NAMESPACE: Uuid = Uuid::from_bytes([
    0x73, 0x7a, 0x2d, 0x72, 0x75, 0x73, 0x74, 0x2d, 0x76, 0x65, 0x63, 0x74, 0x6f, 0x72, 0x64, 0x62,
]);

/// Qdrant HTTP API 适配器
pub struct QdrantVectorStore {
    base_url: String,
    collection: String,
    http: Client,
    api_key: Option<String>,
}

impl QdrantVectorStore {
    /// 创建 Qdrant 适配器
    ///
    /// - `base_url`：Qdrant REST API 地址（如 `http://localhost:6333`）
    /// - `collection`：collection 名称
    pub fn new(base_url: impl Into<String>, collection: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            collection: collection.into(),
            http: Client::new(),
            api_key: None,
        }
    }

    /// 设置 API Key（Qdrant Cloud 或启用了 auth 的实例）
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// 确保 collection 存在，不存在则创建（默认 Cosine 距离）
    ///
    /// - `dim`：向量维度
    pub async fn ensure_collection(&self, dim: usize) -> Result<(), AiError> {
        let url = format!("{}/collections/{}", self.base_url, self.collection);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(map_network_error)?;

        if resp.status().is_success() {
            return Ok(());
        }

        if resp.status() != StatusCode::NOT_FOUND {
            return Err(map_status_error(resp.status(), "ensure_collection: get").await);
        }

        let body = serde_json::json!({
            "vectors": {
                "size": dim,
                "distance": "Cosine"
            }
        });
        let resp = self
            .http
            .put(&url)
            .json(&body)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(map_network_error)?;

        if resp.status().is_success() || resp.status() == StatusCode::CONFLICT {
            Ok(())
        } else {
            Err(map_status_error(resp.status(), "ensure_collection: create").await)
        }
    }

    /// 构造认证 headers
    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(ref key) = self.api_key {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(key) {
                headers.insert("api-key", val);
            }
        }
        headers
    }

    /// 字符串 ID → UUID v5（确定性映射，保证 upsert 幂等）
    fn point_id(record_id: &str) -> Uuid {
        Uuid::new_v5(&ID_NAMESPACE, record_id.as_bytes())
    }

    /// SimilarityMetric → Qdrant distance 字符串
    fn distance_name(metric: SimilarityMetric) -> &'static str {
        match metric {
            SimilarityMetric::Cosine => "Cosine",
            SimilarityMetric::Dot => "Dot",
            SimilarityMetric::L2 => "Euclid",
        }
    }
}

#[async_trait]
impl VectorStore for QdrantVectorStore {
    async fn upsert(&self, records: &[VectorRecord]) -> Result<(), AiError> {
        if records.is_empty() {
            return Ok(());
        }

        let url = format!(
            "{}/collections/{}/points?wait=true",
            self.base_url, self.collection
        );

        let points: Vec<QdrantPoint> = records
            .iter()
            .map(|rec| QdrantPoint {
                id: Self::point_id(&rec.id),
                vector: rec.vector.clone(),
                payload: QdrantPayload {
                    record_id: rec.id.clone(),
                    tenant_id: rec.tenant_id.clone(),
                    text: rec
                        .metadata
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    metadata: rec.metadata.clone(),
                },
            })
            .collect();

        let body = serde_json::json!({ "points": points });
        let resp = self
            .http
            .put(&url)
            .json(&body)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(map_network_error)?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(map_status_error(resp.status(), "upsert").await)
        }
    }

    async fn query(
        &self,
        vec: &[f32],
        topk: usize,
        metric: SimilarityMetric,
        tenant: &str,
    ) -> Result<Vec<VectorHit>, AiError> {
        let url = format!(
            "{}/collections/{}/points/search",
            self.base_url, self.collection
        );

        let body = serde_json::json!({
            "vector": vec,
            "limit": topk,
            "with_payload": true,
            "filter": {
                "must": [
                    {"key": "tenant_id", "match": {"value": tenant}}
                ]
            }
        });

        let resp = self
            .http
            .post(&url)
            .json(&body)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(map_network_error)?;

        if !resp.status().is_success() {
            return Err(map_status_error(resp.status(), "query").await);
        }

        let search_resp: QdrantSearchResponse = resp
            .json()
            .await
            .map_err(|e| AiError::Internal(format!("qdrant query decode failed: {e}")))?;

        let hits = search_resp
            .result
            .into_iter()
            .map(|r| VectorHit {
                id: r.payload.record_id,
                score: r.score,
                metadata: r.payload.metadata,
                text: r.payload.text,
            })
            .collect();

        let _ = Self::distance_name(metric);
        Ok(hits)
    }

    async fn delete(&self, ids: &[&str], tenant: &str) -> Result<(), AiError> {
        if ids.is_empty() {
            return Ok(());
        }

        let url = format!(
            "{}/collections/{}/points/delete?wait=true",
            self.base_url, self.collection
        );

        let point_ids: Vec<Uuid> = ids.iter().map(|id| Self::point_id(id)).collect();
        let body = serde_json::json!({
            "points": point_ids,
            "filter": {
                "must": [
                    {"key": "tenant_id", "match": {"value": tenant}}
                ]
            }
        });

        let resp = self
            .http
            .post(&url)
            .json(&body)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(map_network_error)?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(map_status_error(resp.status(), "delete").await)
        }
    }
}

// ── Qdrant API 数据结构 ──

#[derive(Serialize)]
struct QdrantPoint {
    id: Uuid,
    vector: Vec<f32>,
    payload: QdrantPayload,
}

#[derive(Serialize, Deserialize)]
struct QdrantPayload {
    record_id: String,
    tenant_id: String,
    text: String,
    metadata: serde_json::Value,
}

#[derive(Deserialize)]
struct QdrantSearchResponse {
    result: Vec<QdrantSearchResult>,
}

#[derive(Deserialize)]
struct QdrantSearchResult {
    score: f32,
    payload: QdrantPayload,
}

// ── 异常映射 ──

fn map_network_error(e: reqwest::Error) -> AiError {
    AiError::ProviderUnavailable(format!("qdrant: {e}"))
}

async fn map_status_error(status: StatusCode, op: &str) -> AiError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            AiError::ProviderAuthFailed("qdrant".into())
        }
        StatusCode::NOT_FOUND => AiError::Internal(format!("qdrant {op}: collection not found")),
        StatusCode::TOO_MANY_REQUESTS => AiError::RateLimited { retry_after_ms: 0 },
        _ => AiError::Internal(format!("qdrant {op}: HTTP {status}")),
    }
}

// ── 单元测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_id_deterministic() {
        let id1 = QdrantVectorStore::point_id("rec-001");
        let id2 = QdrantVectorStore::point_id("rec-001");
        assert_eq!(id1, id2, "same string must map to same UUID");
    }

    #[test]
    fn point_id_different_inputs() {
        let id1 = QdrantVectorStore::point_id("rec-001");
        let id2 = QdrantVectorStore::point_id("rec-002");
        assert_ne!(id1, id2, "different strings must map to different UUIDs");
    }

    #[test]
    fn point_id_is_valid_uuid() {
        let id = QdrantVectorStore::point_id("any-string-id");
        assert_eq!(id.get_version_num(), 5, "must be UUID v5");
    }

    #[test]
    fn distance_name_mapping() {
        assert_eq!(
            QdrantVectorStore::distance_name(SimilarityMetric::Cosine),
            "Cosine"
        );
        assert_eq!(
            QdrantVectorStore::distance_name(SimilarityMetric::Dot),
            "Dot"
        );
        assert_eq!(
            QdrantVectorStore::distance_name(SimilarityMetric::L2),
            "Euclid"
        );
    }

    #[test]
    fn new_trims_trailing_slash() {
        let store = QdrantVectorStore::new("http://localhost:6333/", "test_col");
        assert_eq!(store.base_url, "http://localhost:6333");
        assert_eq!(store.collection, "test_col");
    }

    #[test]
    fn with_api_key_sets_key() {
        let store =
            QdrantVectorStore::new("http://localhost:6333", "col").with_api_key("secret-key");
        assert_eq!(store.api_key.as_deref(), Some("secret-key"));
    }

    #[test]
    fn auth_headers_empty_without_key() {
        let store = QdrantVectorStore::new("http://localhost:6333", "col");
        let headers = store.auth_headers();
        assert!(headers.get("api-key").is_none());
    }

    #[test]
    fn auth_headers_present_with_key() {
        let store = QdrantVectorStore::new("http://localhost:6333", "col").with_api_key("test-key");
        let headers = store.auth_headers();
        assert_eq!(headers.get("api-key").unwrap(), "test-key");
    }

    #[test]
    fn qdrant_point_serializes_correctly() {
        let point = QdrantPoint {
            id: Uuid::new_v4(),
            vector: vec![1.0, 2.0, 3.0],
            payload: QdrantPayload {
                record_id: "r1".into(),
                tenant_id: "t1".into(),
                text: "hello".into(),
                metadata: serde_json::json!({"page": 1}),
            },
        };
        let json = serde_json::to_string(&point).unwrap();
        assert!(json.contains("\"vector\":[1.0,2.0,3.0]"));
        assert!(json.contains("\"record_id\":\"r1\""));
        assert!(json.contains("\"tenant_id\":\"t1\""));
    }

    #[test]
    fn qdrant_search_response_deserializes() {
        let json = r#"{
            "result": [
                {
                    "id": "550e8400-e29b-41d4-a716-446655440000",
                    "version": 0,
                    "score": 0.95,
                    "payload": {
                        "record_id": "rec-1",
                        "tenant_id": "t1",
                        "text": "sample text",
                        "metadata": {"source": "doc1"}
                    },
                    "vector": null
                }
            ]
        }"#;
        let resp: QdrantSearchResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.result.len(), 1);
        assert!((resp.result[0].score - 0.95).abs() < 1e-6);
        assert_eq!(resp.result[0].payload.record_id, "rec-1");
        assert_eq!(resp.result[0].payload.text, "sample text");
    }

    #[tokio::test]
    async fn upsert_empty_records_is_noop() {
        let store = QdrantVectorStore::new("http://localhost:6333", "col");
        let result = store.upsert(&[]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_empty_ids_is_noop() {
        let store = QdrantVectorStore::new("http://localhost:6333", "col");
        let result = store.delete(&[], "t1").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn network_error_maps_to_provider_unavailable() {
        let store = QdrantVectorStore::new("http://127.0.0.1:1", "col");
        let result = store
            .upsert(&[VectorRecord::new("r1", vec![1.0], "t1")])
            .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, AiError::ProviderUnavailable(_)),
            "got: {err:?}"
        );
        assert!(err.is_retryable());
    }

    #[tokio::test]
    async fn status_error_mapping() {
        let r = map_status_error(StatusCode::UNAUTHORIZED, "test").await;
        assert!(matches!(r, AiError::ProviderAuthFailed(_)));

        let r = map_status_error(StatusCode::FORBIDDEN, "test").await;
        assert!(matches!(r, AiError::ProviderAuthFailed(_)));

        let r = map_status_error(StatusCode::NOT_FOUND, "test").await;
        assert!(matches!(r, AiError::Internal(_)));

        let r = map_status_error(StatusCode::TOO_MANY_REQUESTS, "test").await;
        assert!(matches!(r, AiError::RateLimited { .. }));

        let r = map_status_error(StatusCode::INTERNAL_SERVER_ERROR, "test").await;
        assert!(matches!(r, AiError::Internal(_)));
    }
}
