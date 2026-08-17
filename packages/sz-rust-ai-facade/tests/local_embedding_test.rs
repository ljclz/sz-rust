//! LocalEmbedding 单元测试

use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::embedding::local::LocalEmbedding;
use sz_rust_ai_facade::embedding::{EmbeddingProvider, EmbeddingRequest};

#[test]
fn local_embedding_new_pseudo_default_dimensions() {
    let emb = LocalEmbedding::new_pseudo(384);
    assert_eq!(emb.dimensions(), 384);
    assert_eq!(emb.name(), "local-embedding");
    assert_eq!(emb.supported_models(), &["local"]);
}

#[test]
fn local_embedding_with_dimensions_changes_dim() {
    let emb = LocalEmbedding::new_pseudo(100).with_dimensions(256);
    assert_eq!(emb.dimensions(), 256);
}

#[tokio::test]
async fn local_embedding_embed_generates_vectors() {
    let emb = LocalEmbedding::new_pseudo(8);
    let req = EmbeddingRequest::new("local", vec!["hello".into(), "world".into()]);
    let result = emb.embed(req).await.unwrap();
    assert_eq!(result.embeddings.len(), 2);
    assert_eq!(result.dimensions, 8);
    assert_eq!(result.model, "local");
    assert_eq!(result.usage_tokens, 0);
    // 每个向量长度 = dimensions
    for v in &result.embeddings {
        assert_eq!(v.len(), 8);
    }
}

#[tokio::test]
async fn local_embedding_embed_empty_input() {
    let emb = LocalEmbedding::new_pseudo(4);
    let req = EmbeddingRequest::new("local", vec![]);
    let result = emb.embed(req).await.unwrap();
    assert!(result.embeddings.is_empty());
    assert_eq!(result.dimensions, 4);
}

#[tokio::test]
async fn local_embedding_embed_byte_normalization() {
    let emb = LocalEmbedding::new_pseudo(4);
    // "A" = 0x41 = 65, 65/255 ≈ 0.2549
    let req = EmbeddingRequest::new("local", vec!["A".into()]);
    let result = emb.embed(req).await.unwrap();
    let v = &result.embeddings[0];
    assert!((v[0] - (65.0f32 / 255.0)).abs() < 1e-6);
    // 其余维度为 0
    for val in v.iter().skip(1).take(3) {
        assert!((*val - 0.0).abs() < 1e-6);
    }
}

#[tokio::test]
async fn local_embedding_embed_long_text_truncated_to_dimensions() {
    let emb = LocalEmbedding::new_pseudo(3);
    // 10 字节文本，但 dimensions=3，只取前 3 字节
    let req = EmbeddingRequest::new("local", vec!["abcdefghij".into()]);
    let result = emb.embed(req).await.unwrap();
    let v = &result.embeddings[0];
    assert_eq!(v.len(), 3);
    // 前 3 字节 a,b,c = 97,98,99
    assert!((v[0] - (97.0f32 / 255.0)).abs() < 1e-6);
    assert!((v[1] - (98.0f32 / 255.0)).abs() < 1e-6);
    assert!((v[2] - (99.0f32 / 255.0)).abs() < 1e-6);
}

#[test]
fn local_embedding_new_nonexistent_path_errors() {
    let result = LocalEmbedding::new("/nonexistent/path/model.onnx");
    assert!(result.is_err());
    assert!(matches!(
        result,
        Err(AiError::LocalModelLoadFailed(ref msg)) if msg.contains("model file not found")
    ));
}
