//! ClaudeProvider 单元测试
//!
//! - 构造器/name/supported_models/token_count 不依赖网络
//! - chat_completion 用 mock 服务器测试（覆盖 build_request_body 间接路径）

mod common;

use std::sync::Arc;
use sz_rust_ai_facade::common::audit::{AuditHttpClient, RateLimitConfig};
use sz_rust_ai_facade::llm::claude::ClaudeProvider;
use sz_rust_ai_facade::llm::provider::{ChatMessage, ChatRequest, LlmProvider, Role};

fn make_provider(base_url: &str) -> ClaudeProvider {
    let client = reqwest::Client::new();
    let audit = AuditHttpClient::new(client, RateLimitConfig::default());
    ClaudeProvider::new("sk-test", base_url, Arc::new(audit))
}

#[test]
fn claude_provider_name() {
    let p = make_provider("https://api.anthropic.com");
    assert_eq!(p.name(), "claude");
}

#[test]
fn claude_provider_supported_models() {
    let p = make_provider("https://api.anthropic.com");
    let models = p.supported_models();
    assert!(models.contains(&"claude-3-5-sonnet-20241022"));
    assert!(models.contains(&"claude-3-5-haiku-20241022"));
    assert!(models.contains(&"claude-3-opus-20240229"));
    assert_eq!(models.len(), 3);
}

#[test]
fn claude_provider_with_timeout() {
    let p =
        make_provider("https://api.anthropic.com").with_timeout(std::time::Duration::from_secs(60));
    let _ = p.name();
}

#[tokio::test]
async fn claude_provider_token_count_estimates() {
    let p = make_provider("https://api.anthropic.com");
    let messages = vec![
        ChatMessage {
            role: Role::User,
            content: "hello".into(),
            tool_call_id: None,
            tool_calls: None,
        },
        ChatMessage {
            role: Role::Assistant,
            content: "world!".into(),
            tool_call_id: None,
            tool_calls: None,
        },
    ];
    // 0.3 * (5 + 6) = 3.3 → 3
    let count = p.token_count(&messages).await.unwrap();
    assert_eq!(count, 3);
}

#[tokio::test]
async fn claude_provider_token_count_empty() {
    let p = make_provider("https://api.anthropic.com");
    assert_eq!(p.token_count(&[]).await.unwrap(), 0);
}

// ===== chat_completion 集成测试（mock 服务器） =====

#[tokio::test]
async fn claude_chat_completion_success_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let body = serde_json::json!({
        "id": "msg_mock",
        "model": "claude-3-5-sonnet-20241022",
        "content": [{"type": "text", "text": "Claude mock response"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 5, "output_tokens": 3}
    })
    .to_string();

    let server = MockHttpServer::start(vec![MockResponse::json(body)])
        .await
        .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new(
        "claude-3-5-sonnet-20241022",
        vec![ChatMessage {
            role: Role::User,
            content: "Hello".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    );
    let result = provider.chat_completion(req).await.unwrap();
    assert_eq!(result.id, "msg_mock");
    assert_eq!(result.choices[0].message.content, "Claude mock response");

    server.stop();
}

#[tokio::test]
async fn claude_chat_completion_401_via_mock() {
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

    let req = ChatRequest::new("claude-3-5-sonnet-20241022", vec![]);
    let err = provider.chat_completion(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_PROVIDER_AUTH_FAILED");

    server.stop();
}

#[tokio::test]
async fn claude_chat_completion_429_via_mock() {
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

    let req = ChatRequest::new("claude-3-5-sonnet-20241022", vec![]);
    let err = provider.chat_completion(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_RATE_LIMITED");

    server.stop();
}

#[tokio::test]
async fn claude_chat_completion_500_via_mock() {
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

    let req = ChatRequest::new("claude-3-5-sonnet-20241022", vec![]);
    let err = provider.chat_completion(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_PROVIDER_UNAVAILABLE");

    server.stop();
}

#[tokio::test]
async fn claude_chat_completion_with_system_message_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let body = serde_json::json!({
        "id": "msg_sys",
        "model": "claude-3-5-sonnet-20241022",
        "content": [{"type": "text", "text": "ok"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 2}
    })
    .to_string();

    let server = MockHttpServer::start(vec![MockResponse::json(body)])
        .await
        .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new(
        "claude-3-5-sonnet-20241022",
        vec![
            ChatMessage {
                role: Role::System,
                content: "You are helpful".into(),
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
    assert_eq!(result.choices[0].message.content, "ok");

    server.stop();
}

#[tokio::test]
async fn claude_chat_completion_with_tools_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};
    use sz_rust_ai_facade::llm::provider::ToolDef;

    let body = serde_json::json!({
        "id": "msg_tools",
        "model": "claude-3-5-sonnet-20241022",
        "content": [
            {"type": "text", "text": "Calling tool"},
            {"type": "tool_use", "id": "toolu_1", "name": "search", "input": {"q": "rust"}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 15, "output_tokens": 10}
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
            "claude-3-5-sonnet-20241022",
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
