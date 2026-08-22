//! OpenAiEmbedding 单元测试（不依赖真实网络）

use sz_rust_ai_facade::common::audit::{AuditHttpClient, RateLimitConfig};
use sz_rust_ai_facade::embedding::openai::OpenAiEmbedding;
use sz_rust_ai_facade::embedding::EmbeddingProvider;

fn make_embedding() -> OpenAiEmbedding {
    let client = reqwest::Client::new();
    let audit = AuditHttpClient::new(client, RateLimitConfig::default());
    OpenAiEmbedding::new(
        "sk-test",
        "https://api.openai.com",
        std::sync::Arc::new(audit),
    )
}

#[test]
fn openai_embedding_new_default_dimensions() {
    let emb = make_embedding();
    assert_eq!(emb.dimensions(), 1536);
    assert_eq!(emb.name(), "openai-embedding");
}

#[test]
fn openai_embedding_with_dimensions() {
    let emb = make_embedding().with_dimensions(3072);
    assert_eq!(emb.dimensions(), 3072);
}

#[test]
fn openai_embedding_supported_models() {
    let emb = make_embedding();
    let models = emb.supported_models();
    assert!(models.contains(&"text-embedding-3-small"));
    assert!(models.contains(&"text-embedding-3-large"));
    assert!(models.contains(&"text-embedding-ada-002"));
    assert_eq!(models.len(), 3);
}
