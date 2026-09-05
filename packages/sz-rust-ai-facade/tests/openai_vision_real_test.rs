// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
#![cfg(feature = "real-api")]

use std::sync::Arc;
use sz_rust_ai_facade::common::{AuditHttpClient, RateLimitConfig};
use sz_rust_ai_facade::llm::openai::OpenAiProvider;
use sz_rust_ai_facade::llm::provider::{
    ChatMessage, ChatRequest, ContentPart, ImageDetail, LlmProvider, Role,
};

fn real_provider() -> OpenAiProvider {
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let http = Arc::new(AuditHttpClient::new(
        reqwest::Client::new(),
        RateLimitConfig::default(),
    ));
    OpenAiProvider::new(api_key, "https://api.openai.com", http)
}

#[tokio::test]
#[ignore = "需要真实 OpenAI API Key + 网络"]
async fn openai_vision_image_url_describes_content() {
    let provider = real_provider();
    let req = ChatRequest::new(
        "gpt-4o-mini",
        vec![
            ChatMessage {
                role: Role::User,
                content: "What is in this image? Describe in one sentence.".into(),
                tool_call_id: None,
                tool_calls: None,
            },
            ChatMessage {
                role: Role::User,
                content: ContentPart::Image {
                    url: "https://upload.wikimedia.org/wikipedia/commons/thumb/4/47/PNG_transparency_demonstration_1.png/280px-PNG_transparency_demonstration_1.png".to_string(),
                    detail: ImageDetail::Low,
                },
                tool_call_id: None,
                tool_calls: None,
            },
        ],
    );

    let result = provider.chat_completion(req).await;
    assert!(result.is_ok(), "vision API call failed: {:?}", result.err());
    let completion = result.unwrap();
    assert!(!completion.choices.is_empty());
    let text = completion.choices[0].message.content.text_or_empty();
    assert!(!text.is_empty(), "response text should not be empty");
    println!("Vision response: {text}");
}

#[tokio::test]
#[ignore = "需要真实 OpenAI API Key + 网络"]
async fn openai_vision_image_base64_describes_content() {
    let provider = real_provider();

    let tiny_png_base64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==";

    let req = ChatRequest::new(
        "gpt-4o-mini",
        vec![
            ChatMessage {
                role: Role::User,
                content: "What color is this image?".into(),
                tool_call_id: None,
                tool_calls: None,
            },
            ChatMessage {
                role: Role::User,
                content: ContentPart::ImageBase64 {
                    data: tiny_png_base64.to_string(),
                    mime_type: "image/png".to_string(),
                },
                tool_call_id: None,
                tool_calls: None,
            },
        ],
    );

    let result = provider.chat_completion(req).await;
    assert!(result.is_ok(), "vision API call failed: {:?}", result.err());
    let completion = result.unwrap();
    assert!(!completion.choices.is_empty());
    let text = completion.choices[0].message.content.text_or_empty();
    assert!(!text.is_empty(), "response text should not be empty");
    println!("Vision base64 response: {text}");
}

#[tokio::test]
#[ignore = "需要真实 OpenAI API Key + 网络"]
async fn openai_vision_with_all_detail_levels() {
    let provider = real_provider();
    let image_url = "https://upload.wikimedia.org/wikipedia/commons/thumb/4/47/PNG_transparency_demonstration_1.png/280px-PNG_transparency_demonstration_1.png";

    for detail in [ImageDetail::Low, ImageDetail::High, ImageDetail::Auto] {
        let req = ChatRequest::new(
            "gpt-4o-mini",
            vec![ChatMessage {
                role: Role::User,
                content: ContentPart::Image {
                    url: image_url.to_string(),
                    detail,
                },
                tool_call_id: None,
                tool_calls: None,
            }],
        );

        let result = provider.chat_completion(req).await;
        assert!(
            result.is_ok(),
            "vision API call with detail {:?} failed: {:?}",
            detail,
            result.err()
        );
    }
}
