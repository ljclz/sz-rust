//! audit_remediation_v2 — tiktoken feature 端到端测试
//!
//! 验证 sz300 Cargo.toml 启用 tiktoken feature 后，
//! OpenAiProvider::token_count 使用 tiktoken-rs cl100k_base BPE 精确计算，
//! 非 chars * 0.25 估算。

use std::sync::Arc;
use sz_rust_ai_facade::common::{AuditHttpClient, RateLimitConfig};
use sz_rust_ai_facade::llm::openai::OpenAiProvider;
use sz_rust_ai_facade::llm::provider::{ChatMessage, LlmProvider, Role};

#[tokio::test]
async fn token_count_uses_tiktoken_bpe() {
    let http = Arc::new(AuditHttpClient::new(
        reqwest::Client::new(),
        RateLimitConfig::default(),
    ));
    let provider = OpenAiProvider::new("test-key", "https://api.openai.com", http);

    let text = "你好世界，这是一个 tiktoken 精确分词测试。";
    let messages = vec![ChatMessage {
        role: Role::User,
        content: text.into(),
        tool_call_id: None,
        tool_calls: None,
    }];

    let count = provider
        .token_count(&messages)
        .await
        .expect("token_count 应成功");
    let estimated = (text.chars().count() as f32 * 0.25) as u32;

    assert!(count > 0, "token_count 应返回非零值");
    assert_ne!(
        count, estimated,
        "tiktoken BPE 精确值 ({}) 应与 chars*0.25 估算值 ({}) 不同",
        count, estimated
    );
}
