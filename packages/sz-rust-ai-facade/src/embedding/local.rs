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
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn new_pseudo_creates_instance() {
        let emb = LocalEmbedding::new_pseudo(384);
        assert_eq!(emb.dimensions(), 384);
        assert_eq!(emb.model_path(), "");
        assert!(!emb.is_model_loaded());
    }

    #[test]
    fn new_file_not_found_returns_error() {
        let result = LocalEmbedding::new("/nonexistent/path/model.onnx");
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert_eq!(err.error_code(), "AI_LOCAL_MODEL_LOAD_FAILED");
    }

    #[test]
    fn with_dimensions_changes_dim() {
        let emb = LocalEmbedding::new_pseudo(384).with_dimensions(768);
        assert_eq!(emb.dimensions(), 768);
    }

    #[test]
    fn model_path_returns_path() {
        let emb = LocalEmbedding::new_pseudo(128);
        assert_eq!(emb.model_path(), "");
    }

    #[test]
    fn load_model_empty_path_fails() {
        let mut emb = LocalEmbedding::new_pseudo(384);
        let result = emb.load_model();
        assert!(result.is_err());
        assert_eq!(
            result.err().unwrap().error_code(),
            "AI_LOCAL_MODEL_LOAD_FAILED"
        );
    }

    #[test]
    fn load_model_with_tempfile_succeeds() {
        let mut temp = tempfile::NamedTempFile::new().unwrap();
        temp.write_all(b"fake model data").unwrap();
        let path = temp.path().to_str().unwrap().to_string();
        let mut emb = LocalEmbedding::new(&path).unwrap();
        assert!(!emb.is_model_loaded());
        emb.load_model().unwrap();
        assert!(emb.is_model_loaded());
    }

    #[tokio::test]
    async fn embed_generates_vectors_with_byte_normalization() {
        let emb = LocalEmbedding::new_pseudo(4);
        let req = EmbeddingRequest::new("local", vec!["hello".to_string()]);
        let result = emb.embed(req).await.unwrap();
        assert_eq!(result.dimensions, 4);
        assert_eq!(result.embeddings.len(), 1);
        assert_eq!(result.embeddings[0].len(), 4);
        let expected_h = 104.0f32 / 255.0;
        assert!((result.embeddings[0][0] - expected_h).abs() < 1e-6);
        let expected_e = 101.0f32 / 255.0;
        assert!((result.embeddings[0][1] - expected_e).abs() < 1e-6);
    }

    #[tokio::test]
    async fn embed_multiple_texts() {
        let emb = LocalEmbedding::new_pseudo(8);
        let req = EmbeddingRequest::new("local", vec!["ab".to_string(), "cd".to_string()]);
        let result = emb.embed(req).await.unwrap();
        assert_eq!(result.embeddings.len(), 2);
        assert_eq!(result.embeddings[0].len(), 8);
        assert_eq!(result.embeddings[1].len(), 8);
    }

    #[tokio::test]
    async fn embed_text_longer_than_dimensions_truncates() {
        let emb = LocalEmbedding::new_pseudo(2);
        let req = EmbeddingRequest::new("local", vec!["abcdef".to_string()]);
        let result = emb.embed(req).await.unwrap();
        assert_eq!(result.embeddings[0].len(), 2);
        let expected_a = 97.0f32 / 255.0;
        let expected_b = 98.0f32 / 255.0;
        assert!((result.embeddings[0][0] - expected_a).abs() < 1e-6);
        assert!((result.embeddings[0][1] - expected_b).abs() < 1e-6);
    }

    #[tokio::test]
    async fn embed_empty_text() {
        let emb = LocalEmbedding::new_pseudo(4);
        let req = EmbeddingRequest::new("local", vec!["".to_string()]);
        let result = emb.embed(req).await.unwrap();
        assert_eq!(result.embeddings.len(), 1);
        assert_eq!(result.embeddings[0].len(), 4);
        for val in &result.embeddings[0] {
            assert!((val - 0.0).abs() < 1e-6);
        }
    }

    #[test]
    fn name_pseudo() {
        let emb = LocalEmbedding::new_pseudo(384);
        assert_eq!(emb.name(), "local-embedding-pseudo");
    }

    #[test]
    fn name_with_model_path_not_loaded() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let path = temp.path().to_str().unwrap().to_string();
        let emb = LocalEmbedding::new(&path).unwrap();
        assert_eq!(emb.name(), "local-embedding");
    }

    #[test]
    fn name_with_model_loaded() {
        let mut temp = tempfile::NamedTempFile::new().unwrap();
        temp.write_all(b"fake model").unwrap();
        let path = temp.path().to_str().unwrap().to_string();
        let mut emb = LocalEmbedding::new(&path).unwrap();
        emb.load_model().unwrap();
        assert_eq!(emb.name(), "local-embedding-loaded");
    }

    #[test]
    fn supported_models_returns_local() {
        let emb = LocalEmbedding::new_pseudo(384);
        assert_eq!(emb.supported_models(), &["local"]);
    }
}
