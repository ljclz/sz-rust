// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 真实 API 联调测试（需网络 + API Key，默认 ignored）
//!
//! 运行方式：
//! ```bash
//! cargo test -p sz-rust-ai-facade --features all-providers --test real_api_test -- --ignored
//! ```

#![cfg(feature = "openai")]

use std::sync::Arc;
use std::time::Duration;

use sz_rust_ai_facade::common::{AuditHttpClient, RateLimitConfig};
use sz_rust_ai_facade::llm::openai::OpenAiProvider;
use sz_rust_ai_facade::llm::provider::{ChatMessage, ChatRequest, LlmProvider, Role};

fn make_http() -> Arc<AuditHttpClient> {
    Arc::new(AuditHttpClient::new(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap(),
        RateLimitConfig::default(),
    ))
}

fn make_chat_request(model: &str, prompt: &str) -> ChatRequest {
    ChatRequest::new(
        model,
        vec![ChatMessage {
            role: Role::User,
            content: prompt.to_string().into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    )
}

#[tokio::test]
#[ignore = "需真实 DeepSeek API Key + 网络"]
async fn deepseek_chat_completion() {
    let api_key = "sk-697c8a668c084acaa8e8630bdf7d6140";
    let base_url = "https://api.deepseek.com";
    let model = "deepseek-chat";

    let provider =
        OpenAiProvider::new(api_key, base_url, make_http()).with_timeout(Duration::from_secs(30));

    let req = make_chat_request(model, "说一个字：好");
    let result = provider.chat_completion(req).await;

    match result {
        Ok(completion) => {
            assert!(!completion.id.is_empty(), "id 不应为空");
            assert!(!completion.choices.is_empty(), "choices 不应为空");
            let content = &completion.choices[0].message.content;
            assert!(!content.text_or_empty().is_empty(), "回复内容不应为空");
            println!(
                "[DeepSeek] model={}, content={}, usage={:?}",
                completion.model, content, completion.usage
            );
        }
        Err(e) => {
            panic!("DeepSeek API 调用失败: {:?}", e);
        }
    }
}

#[tokio::test]
#[ignore = "需真实 DeepSeek API Key + 网络"]
async fn deepseek_stream_completion() {
    let api_key = "sk-697c8a668c084acaa8e8630bdf7d6140";
    let base_url = "https://api.deepseek.com";
    let model = "deepseek-chat";

    let provider =
        OpenAiProvider::new(api_key, base_url, make_http()).with_timeout(Duration::from_secs(30));

    let req = make_chat_request(model, "从 1 数到 5");
    let stream = provider
        .stream_completion(req)
        .await
        .expect("stream 创建失败");

    use futures::StreamExt;
    let mut stream = stream;
    let mut collected = String::new();
    while let Some(delta) = stream.next().await {
        match delta {
            Ok(d) => {
                collected.push_str(&d.content_delta);
            }
            Err(e) => panic!("stream 错误: {:?}", e),
        }
    }
    assert!(!collected.is_empty(), "流式回复不应为空");
    println!("[DeepSeek Stream] collected: {}", collected);
}

#[tokio::test]
#[ignore = "需真实 DeepSeek API Key + 网络"]
async fn deepseek_token_count() {
    let api_key = "sk-697c8a668c084acaa8e8630bdf7d6140";
    let base_url = "https://api.deepseek.com";

    let provider = OpenAiProvider::new(api_key, base_url, make_http());

    let messages = vec![ChatMessage {
        role: Role::User,
        content: "你好，世界".to_string().into(),
        tool_call_id: None,
        tool_calls: None,
    }];
    let count = provider.token_count(&messages).await;
    println!("[DeepSeek token_count] result: {:?}", count);
}

#[tokio::test]
#[ignore = "需真实 CSDN API Key + 网络"]
async fn csdn_chat_completion() {
    let api_key = "sk-rpxbjmotzxvdcqyrinndqzogxqnkhydbyhmlmbwmytnhq";
    let base_url = "https://llm.csdn.net";
    let model = "deepseek-chat";

    let provider =
        OpenAiProvider::new(api_key, base_url, make_http()).with_timeout(Duration::from_secs(30));

    let req = make_chat_request(model, "说一个字：好");
    let result = provider.chat_completion(req).await;

    match result {
        Ok(completion) => {
            assert!(!completion.choices.is_empty());
            println!(
                "[CSDN] model={}, content={}",
                completion.model, completion.choices[0].message.content
            );
        }
        Err(e) => {
            eprintln!("[CSDN] 调用失败: {:?}", e);
        }
    }
}

#[tokio::test]
#[ignore = "需真实快手 API Key + 网络"]
async fn kuaishou_chat_completion() {
    let api_key = "fPJokxwUdNUEOeHSfGS0XRA3H0dUpt3yI0baHDodXZg";
    let base_url = "https://api.klingai.com";
    let model = "KAT-Coder-Pro-V2.5";

    let provider =
        OpenAiProvider::new(api_key, base_url, make_http()).with_timeout(Duration::from_secs(30));

    let req = make_chat_request(model, "说一个字：好");
    let result = provider.chat_completion(req).await;

    match result {
        Ok(completion) => {
            assert!(!completion.choices.is_empty());
            println!(
                "[快手] model={}, content={}",
                completion.model, completion.choices[0].message.content
            );
        }
        Err(e) => {
            eprintln!("[快手] 调用失败: {:?}", e);
        }
    }
}
