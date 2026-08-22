//! GeminiProvider 单元测试
//!
//! - 构造器/name/supported_models/token_count 不依赖网络
//! - chat_completion 用 mock 服务器测试

mod common;

use std::sync::Arc;
use sz_rust_ai_facade::common::audit::{AuditHttpClient, RateLimitConfig};
use sz_rust_ai_facade::llm::gemini::GeminiProvider;
use sz_rust_ai_facade::llm::provider::{ChatMessage, ChatRequest, LlmProvider, Role};

fn make_provider(base_url: &str) -> GeminiProvider {
    let client = reqwest::Client::new();
    let audit = AuditHttpClient::new(client, RateLimitConfig::default());
    GeminiProvider::new("test-key", base_url, Arc::new(audit))
}

#[test]
fn gemini_provider_name() {
    let p = make_provider("https://generativelanguage.googleapis.com");
    assert_eq!(p.name(), "gemini");
}

#[test]
fn gemini_provider_supported_models() {
    let p = make_provider("https://generativelanguage.googleapis.com");
    let models = p.supported_models();
    assert!(models.contains(&"gemini-2.0-flash"));
    assert!(models.contains(&"gemini-1.5-pro"));
    assert!(models.contains(&"gemini-1.5-flash"));
    assert_eq!(models.len(), 3);
}

#[test]
fn gemini_provider_with_timeout() {
    let p = make_provider("https://generativelanguage.googleapis.com")
        .with_timeout(std::time::Duration::from_secs(45));
    let _ = p.name();
}

#[tokio::test]
async fn gemini_provider_token_count_estimates() {
    let p = make_provider("https://generativelanguage.googleapis.com");
    let messages = vec![
        ChatMessage {
            role: Role::User,
            content: "hello world".into(),
            tool_call_id: None,
            tool_calls: None,
        },
        ChatMessage {
            role: Role::Assistant,
            content: "hi".into(),
            tool_call_id: None,
            tool_calls: None,
        },
    ];
    // 0.25 * (11 + 2) = 3.25 → 3
    let count = p.token_count(&messages).await.unwrap();
    assert_eq!(count, 3);
}

#[tokio::test]
async fn gemini_provider_token_count_empty() {
    let p = make_provider("https://generativelanguage.googleapis.com");
    assert_eq!(p.token_count(&[]).await.unwrap(), 0);
}

// ===== chat_completion 集成测试（mock 服务器） =====

#[tokio::test]
async fn gemini_chat_completion_success_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let body = serde_json::json!({
        "candidates": [{
            "content": {"parts": [{"text": "Gemini mock"}], "role": "model"},
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 2, "totalTokenCount": 5}
    })
    .to_string();

    let server = MockHttpServer::start(vec![MockResponse::json(body)])
        .await
        .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new(
        "gemini-2.0-flash",
        vec![ChatMessage {
            role: Role::User,
            content: "Hi".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    );
    let result = provider.chat_completion(req).await.unwrap();
    assert_eq!(
        result.choices[0].message.content.as_text(),
        Some("Gemini mock")
    );
    assert_eq!(result.model, "gemini-2.0-flash");

    server.stop();
}

#[tokio::test]
async fn gemini_chat_completion_401_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let server = MockHttpServer::start(vec![MockResponse {
        status: 401,
        body: "unauthorized".into(),
        delay_ms: 0,
        headers: vec![],
    }])
    .await
    .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new("gemini-2.0-flash", vec![]);
    let err = provider.chat_completion(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_PROVIDER_AUTH_FAILED");

    server.stop();
}

#[tokio::test]
async fn gemini_chat_completion_403_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let server = MockHttpServer::start(vec![MockResponse {
        status: 403,
        body: "forbidden".into(),
        delay_ms: 0,
        headers: vec![],
    }])
    .await
    .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new("gemini-2.0-flash", vec![]);
    let err = provider.chat_completion(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_PROVIDER_AUTH_FAILED");

    server.stop();
}

#[tokio::test]
async fn gemini_chat_completion_429_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let server = MockHttpServer::start(vec![MockResponse {
        status: 429,
        body: "rate".into(),
        delay_ms: 0,
        headers: vec![],
    }])
    .await
    .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new("gemini-2.0-flash", vec![]);
    let err = provider.chat_completion(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_RATE_LIMITED");

    server.stop();
}

#[tokio::test]
async fn gemini_chat_completion_500_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let server = MockHttpServer::start(vec![MockResponse {
        status: 500,
        body: "err".into(),
        delay_ms: 0,
        headers: vec![],
    }])
    .await
    .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new("gemini-2.0-flash", vec![]);
    let err = provider.chat_completion(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_PROVIDER_UNAVAILABLE");

    server.stop();
}

#[tokio::test]
async fn gemini_chat_completion_with_system_instruction_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let body = serde_json::json!({
        "candidates": [{
            "content": {"parts": [{"text": "ok"}], "role": "model"},
            "finishReason": "STOP",
            "index": 0
        }]
    })
    .to_string();

    let server = MockHttpServer::start(vec![MockResponse::json(body)])
        .await
        .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new(
        "gemini-2.0-flash",
        vec![
            ChatMessage {
                role: Role::System,
                content: "Be helpful".into(),
                tool_call_id: None,
                tool_calls: None,
            },
            ChatMessage {
                role: Role::User,
                content: "Hi".into(),
                tool_call_id: None,
                tool_calls: None,
            },
        ],
    );
    let result = provider.chat_completion(req).await.unwrap();
    assert_eq!(result.choices[0].message.content.as_text(), Some("ok"));

    server.stop();
}

#[tokio::test]
async fn gemini_chat_completion_with_tools_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};
    use sz_rust_ai_facade::llm::provider::ToolDef;

    let body = serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [
                    {"text": "calling"},
                    {"functionCall": {"name": "search", "args": {"q": "rust"}}}
                ],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }]
    })
    .to_string();

    let server = MockHttpServer::start(vec![MockResponse::json(body)])
        .await
        .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest {
        tools: Some(vec![ToolDef {
            name: "search".into(),
            description: "Search".into(),
            parameters: serde_json::json!({"type": "object"}),
        }]),
        ..ChatRequest::new(
            "gemini-2.0-flash",
            vec![ChatMessage {
                role: Role::User,
                content: "search rust".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
        )
    };
    let result = provider.chat_completion(req).await.unwrap();
    let tc = result.choices[0].message.tool_calls.as_ref().unwrap();
    assert_eq!(tc[0].name, "search");

    server.stop();
}
