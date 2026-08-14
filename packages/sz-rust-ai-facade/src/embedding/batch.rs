use crate::common::AiError;
use crate::embedding::{EmbeddingProvider, EmbeddingRequest, EmbeddingResult};

pub struct BatchChunker;

impl BatchChunker {
    pub fn chunk(texts: &[String], chunk_size: usize) -> Vec<Vec<String>> {
        if chunk_size == 0 || texts.is_empty() {
            return vec![];
        }
        texts.chunks(chunk_size).map(|c| c.to_vec()).collect()
    }

    pub async fn embed_chunks<P: EmbeddingProvider>(
        provider: &P,
        texts: &[String],
        chunk_size: usize,
        model: &str,
    ) -> Result<EmbeddingResult, AiError> {
        let chunks = Self::chunk(texts, chunk_size);
        let mut all_embeddings = Vec::with_capacity(texts.len());
        let mut total_usage = 0u32;

        for chunk in chunks {
            let req = EmbeddingRequest::new(model, chunk);
            let result = provider.embed(req).await?;
            all_embeddings.extend(result.embeddings);
            total_usage += result.usage_tokens;
        }

        let dimensions = all_embeddings.first().map(|v| v.len()).unwrap_or(0);
        Ok(EmbeddingResult {
            model: model.to_string(),
            embeddings: all_embeddings,
            dimensions,
            usage_tokens: total_usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct MockEmbedding;
    #[async_trait]
    impl EmbeddingProvider for MockEmbedding {
        fn name(&self) -> &str {
            "mock"
        }
        async fn embed(&self, req: EmbeddingRequest) -> Result<EmbeddingResult, AiError> {
            let embeddings: Vec<Vec<f32>> = req.input.iter().map(|_| vec![0.1, 0.2, 0.3]).collect();
            let usage = embeddings.len() as u32;
            Ok(EmbeddingResult {
                model: req.model,
                dimensions: 3,
                embeddings,
                usage_tokens: usage,
            })
        }
        fn dimensions(&self) -> usize {
            3
        }
        fn supported_models(&self) -> &[&str] {
            &[]
        }
    }

    #[test]
    fn chunk_empty_returns_empty() {
        let result = BatchChunker::chunk(&[], 64);
        assert!(result.is_empty());
    }

    #[test]
    fn chunk_zero_size_returns_empty() {
        let texts = vec!["a".to_string()];
        let result = BatchChunker::chunk(&texts, 0);
        assert!(result.is_empty());
    }

    #[test]
    fn chunk_exact_division() {
        let texts: Vec<String> = (0..200).map(|i| format!("text{}", i)).collect();
        let chunks = BatchChunker::chunk(&texts, 64);
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks[0].len(), 64);
        assert_eq!(chunks[3].len(), 8);
    }

    #[test]
    fn chunk_preserves_order() {
        let texts: Vec<String> = (0..10).map(|i| format!("t{}", i)).collect();
        let chunks = BatchChunker::chunk(&texts, 3);
        let flat: Vec<String> = chunks.into_iter().flatten().collect();
        assert_eq!(flat, texts);
    }

    #[tokio::test]
    async fn embed_chunks_merges_all_embeddings() {
        let texts: Vec<String> = (0..200).map(|i| format!("text{}", i)).collect();
        let provider = MockEmbedding;
        let result = BatchChunker::embed_chunks(&provider, &texts, 64, "mock-model")
            .await
            .unwrap();
        assert_eq!(result.embeddings.len(), 200);
        assert_eq!(result.dimensions, 3);
        assert_eq!(result.usage_tokens, 200);
    }

    #[tokio::test]
    async fn embed_chunks_single_chunk() {
        let texts = vec!["a".to_string(), "b".to_string()];
        let provider = MockEmbedding;
        let result = BatchChunker::embed_chunks(&provider, &texts, 64, "mock-model")
            .await
            .unwrap();
        assert_eq!(result.embeddings.len(), 2);
    }
}
