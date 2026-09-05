// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::common::{AiError, AuditHttpClient};
use crate::embedding::{EmbeddingProvider, EmbeddingRequest, EmbeddingResult};
use async_trait::async_trait;
use std::sync::Arc;

pub struct OpenAiEmbedding {
    api_key: String,
    base_url: String,
    http: Arc<AuditHttpClient>,
    dimensions: usize,
}

impl OpenAiEmbedding {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        http: Arc<AuditHttpClient>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            http,
            dimensions: 1536,
        }
    }

    pub fn with_dimensions(mut self, dim: usize) -> Self {
        self.dimensions = dim;
        self
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbedding {
    fn name(&self) -> &str {
        "openai-embedding"
    }

    async fn embed(&self, req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
        let url = format!("{}/v1/embeddings", self.base_url);
        let body = serde_json::json!({
            "model": req.model,
            "input": req.input,
        });

        let http_req = self
            .http
            .client()
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .build()
            .map_err(AiError::from)?;

        let resp = self
            .http
            .send_with_audit(http_req, "openai-embedding", &req.model)
            .await?;
        let status = resp.status();

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 401 {
                return Err(AiError::ProviderAuthFailed(format!(
                    "openai-embedding: {}",
                    text
                )));
            }
            if status.as_u16() == 429 {
                return Err(AiError::RateLimited {
                    retry_after_ms: 1000,
                });
            }
            return Err(AiError::ProviderUnavailable(format!(
                "openai-embedding {}: {}",
                status, text
            )));
        }

        let json: serde_json::Value = resp.json().await.map_err(AiError::from)?;

        let embeddings: Vec<Vec<f32>> = json
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| AiError::Internal("OpenAI embedding response missing data".to_string()))?
            .iter()
            .map(|d| {
                d.get("embedding")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|f| f.as_f64().map(|v| v as f32))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();

        let usage_tokens = json
            .get("usage")
            .and_then(|u| u.get("total_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        let dimensions = embeddings
            .first()
            .map(|v| v.len())
            .unwrap_or(self.dimensions);

        Ok(EmbeddingResult {
            model: req.model,
            embeddings,
            dimensions,
            usage_tokens,
        })
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn supported_models(&self) -> &[&str] {
        &[
            "text-embedding-3-small",
            "text-embedding-3-large",
            "text-embedding-ada-002",
        ]
    }
}
