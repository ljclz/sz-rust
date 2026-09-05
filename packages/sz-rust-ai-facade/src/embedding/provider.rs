// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::common::AiError;
use async_trait::async_trait;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmbeddingRequest {
    pub model: String,
    pub input: Vec<String>,
}

impl EmbeddingRequest {
    pub fn new(model: impl Into<String>, input: Vec<String>) -> Self {
        Self {
            model: model.into(),
            input,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmbeddingResult {
    pub model: String,
    pub embeddings: Vec<Vec<f32>>,
    pub dimensions: usize,
    pub usage_tokens: u32,
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn embed(&self, req: EmbeddingRequest) -> Result<EmbeddingResult, AiError>;

    fn dimensions(&self) -> usize;

    fn supported_models(&self) -> &[&str];
}
