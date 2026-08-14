use crate::common::AiError;
use crate::embedding::{EmbeddingProvider, EmbeddingRequest, EmbeddingResult};
use async_trait::async_trait;

pub struct LocalEmbedding {
    #[allow(dead_code)]
    model_path: String,
    dimensions: usize,
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
        })
    }

    pub fn with_dimensions(mut self, dim: usize) -> Self {
        self.dimensions = dim;
        self
    }
}

#[async_trait]
impl EmbeddingProvider for LocalEmbedding {
    fn name(&self) -> &str {
        "local-embedding"
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
