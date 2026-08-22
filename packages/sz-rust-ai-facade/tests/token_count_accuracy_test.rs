#![cfg(feature = "tiktoken")]

use std::sync::Arc;
use sz_rust_ai_facade::common::{AuditHttpClient, RateLimitConfig};
use sz_rust_ai_facade::llm::openai::OpenAiProvider;
use sz_rust_ai_facade::llm::provider::{ChatMessage, LlmProvider, Role};

fn make_provider() -> OpenAiProvider {
    let http = Arc::new(AuditHttpClient::new(
        reqwest::Client::new(),
        RateLimitConfig::default(),
    ));
    OpenAiProvider::new("test-key", "https://api.openai.com", http)
}

fn msg(content: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: content.into(),
        tool_call_id: None,
        tool_calls: None,
    }
}

#[tokio::test]
async fn tiktoken_counts_simple_english_accurately() {
    let provider = make_provider();
    let messages = vec![msg("hello world")];
    let count = provider.token_count(&messages).await.unwrap();
    assert_eq!(
        count, 2,
        "'hello world' should be 2 tokens with cl100k_base"
    );
}

#[tokio::test]
async fn tiktoken_counts_empty_string_zero() {
    let provider = make_provider();
    let messages = vec![msg("")];
    let count = provider.token_count(&messages).await.unwrap();
    assert_eq!(count, 0, "empty string should be 0 tokens");
}

#[tokio::test]
async fn tiktoken_counts_single_token_words() {
    let provider = make_provider();
    let messages = vec![msg("the cat sat")];
    let count = provider.token_count(&messages).await.unwrap();
    assert_eq!(count, 3, "'the cat sat' should be 3 tokens");
}

#[tokio::test]
async fn tiktoken_counts_chinese_characters() {
    let provider = make_provider();
    let messages = vec![msg("你好世界")];
    let count = provider.token_count(&messages).await.unwrap();
    assert!(count > 0, "Chinese text should have positive token count");
    assert!(
        count <= 8,
        "4 Chinese characters should be at most 8 tokens"
    );
}

#[tokio::test]
async fn tiktoken_counts_code_snippet() {
    let provider = make_provider();
    let code = "fn main() { println!(\"hello\"); }";
    let messages = vec![msg(code)];
    let count = provider.token_count(&messages).await.unwrap();
    assert!(count > 5, "code snippet should have more than 5 tokens");
    assert!(count < 30, "code snippet should have fewer than 30 tokens");
}

#[tokio::test]
async fn tiktoken_counts_multiple_messages_sums_correctly() {
    let provider = make_provider();
    let messages = vec![msg("hello"), msg("world")];
    let count = provider.token_count(&messages).await.unwrap();
    assert_eq!(count, 2, "'hello' + 'world' should be 2 tokens total");
}

#[tokio::test]
async fn tiktoken_more_accurate_than_char_estimate() {
    let provider = make_provider();
    let text = "The quick brown fox jumps over the lazy dog. This is a longer sentence to test tokenization accuracy.";
    let messages = vec![msg(text)];
    let tiktoken_count = provider.token_count(&messages).await.unwrap();

    let char_estimate = (text.chars().count() as f32 * 0.25) as u32;

    assert_ne!(
        tiktoken_count, char_estimate,
        "tiktoken count should differ from rough char estimate for this text"
    );
    assert!(tiktoken_count > 0, "tiktoken count should be positive");
}

#[tokio::test]
async fn tiktoken_counts_long_text_reasonably() {
    let provider = make_provider();
    let long_text = "This is a test sentence. ".repeat(100);
    let messages = vec![msg(&long_text)];
    let count = provider.token_count(&messages).await.unwrap();
    assert!(
        count > 100,
        "100 sentences should have more than 100 tokens"
    );
    assert!(
        count < 1000,
        "100 sentences should have fewer than 1000 tokens"
    );
}

#[tokio::test]
async fn tiktoken_counts_special_characters() {
    let provider = make_provider();
    let messages = vec![msg("!!!@#$%^&*()")];
    let count = provider.token_count(&messages).await.unwrap();
    assert!(
        count > 0,
        "special characters should have positive token count"
    );
}

#[tokio::test]
async fn tiktoken_counts_json_string() {
    let provider = make_provider();
    let json = r#"{"name":"test","value":42,"items":["a","b","c"]}"#;
    let messages = vec![msg(json)];
    let count = provider.token_count(&messages).await.unwrap();
    assert!(count > 5, "JSON string should have more than 5 tokens");
    assert!(count < 50, "JSON string should have fewer than 50 tokens");
}
