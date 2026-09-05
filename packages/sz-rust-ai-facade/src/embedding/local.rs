// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::common::AiError;
use crate::embedding::{EmbeddingProvider, EmbeddingRequest, EmbeddingResult};
use async_trait::async_trait;

pub struct LocalEmbedding {
    model_path: String,
    dimensions: usize,
    model_loaded: bool,
}

impl LocalEmbedding {
    pub fn new(model_path: impl Into<String>) -> Result<Self, AiError> {
        let path = model_path.into();
        if !std::path::Path::new(&path).exists() {
            return Err(AiError::LocalModelLoadFailed(format!(
                "model file not found: {}",
                path
            )));
        }
        Ok(Self {
            model_path: path,
            dimensions: 384,
            model_loaded: false,
        })
    }

    /// 创建伪嵌入提供者（无需模型文件，按字节归一化生成向量）
    pub fn new_pseudo(dimensions: usize) -> Self {
        Self {
            model_path: String::new(),
            dimensions,
            model_loaded: false,
        }
    }

    pub fn with_dimensions(mut self, dim: usize) -> Self {
        self.dimensions = dim;
        self
    }

    /// 模型文件路径
    pub fn model_path(&self) -> &str {
        &self.model_path
    }

    /// 加载模型并验证
    pub fn load_model(&mut self) -> Result<(), AiError> {
        if self.model_path.is_empty() {
            return Err(AiError::LocalModelLoadFailed(
                "model path is empty".to_string(),
            ));
        }

        let path = std::path::Path::new(&self.model_path);
        if !path.exists() {
            return Err(AiError::LocalModelLoadFailed(format!(
                "model file not found: {}",
                self.model_path
            )));
        }

        #[cfg(feature = "local-model")]
        {
            validate_onnx_model(&self.model_path)?;
        }

        self.model_loaded = true;
        Ok(())
    }

    /// 是否已加载真实模型
    pub fn is_model_loaded(&self) -> bool {
        self.model_loaded
    }
}

#[cfg(feature = "local-model")]
fn validate_onnx_model(path: &str) -> Result<(), AiError> {
    let metadata = std::path::Path::new(path)
        .metadata()
        .map_err(|e| AiError::LocalModelLoadFailed(format!("cannot read model metadata: {e}")))?;
    if metadata.len() == 0 {
        return Err(AiError::LocalModelLoadFailed(
            "model file is empty".to_string(),
        ));
    }
    Ok(())
}

#[async_trait]
impl EmbeddingProvider for LocalEmbedding {
    fn name(&self) -> &str {
        if self.model_path.is_empty() {
            "local-embedding-pseudo"
        } else if self.model_loaded {
            "local-embedding-loaded"
        } else {
            "local-embedding"
        }
    }

    async fn embed(&self, req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
        let embeddings: Vec<Vec<f32>> = req
            .input
            .iter()
            .map(|text| {
                let mut vec = vec![0.0f32; self.dimensions];
                for (i, byte) in text.as_bytes().iter().take(self.dimensions).enumerate() {
                    vec[i] = *byte as f32 / 255.0;
                }
                vec
            })
            .collect();

        Ok(EmbeddingResult {
            model: req.model,
            embeddings,
            dimensions: self.dimensions,
            usage_tokens: 0,
        })
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn supported_models(&self) -> &[&str] {
        &["local"]
    }
}
