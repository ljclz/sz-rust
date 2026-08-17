//! OpenAiProvider 单元测试
//!
//! - 构造器/name/supported_models/token_count 不依赖网络
//! - parse_completion 是 pub 静态方法，直接测试
//! - chat_completion/stream_completion 用 mock 服务器测试

mod common;

use std::sync::Arc;
use sz_rust_ai_facade::common::audit::{AuditHttpClient, RateLimitConfig};
use sz_rust_ai_facade::llm::openai::OpenAiProvider;
use sz_rust_ai_facade::llm::provider::{ChatMessage, ChatRequest, FinishReason, LlmProvider, Role};

fn make_provider(base_url: &str) -> OpenAiProvider {
    let client = reqwest::Client::new();
    let audit = AuditHttpClient::new(client, RateLimitConfig::default());
    OpenAiProvider::new("sk-test", base_url, Arc::new(audit))
}

#[test]
fn openai_provider_name() {
    let p = make_provider("https://api.openai.com");
    assert_eq!(p.name(), "openai");
}

#[test]
fn openai_provider_supported_models() {
    let p = make_provider("https://api.openai.com");
    let models = p.supported_models();
    assert!(models.contains(&"gpt-4o"));
    assert!(models.contains(&"gpt-4o-mini"));
    assert!(models.contains(&"gpt-4-turbo"));
    assert!(models.contains(&"gpt-3.5-turbo"));
}

#[test]
fn openai_provider_with_timeout() {
    let p =
        make_provider("https://api.openai.com").with_timeout(std::time::Duration::from_secs(30));
    let _ = p.name();
}

#[tokio::test]
async fn openai_provider_token_count_estimates() {
    let p = make_provider("https://api.openai.com");
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
async fn openai_provider_token_count_empty() {
    let p = make_provider("https://api.openai.com");
    let count = p.token_count(&[]).await.unwrap();
    assert_eq!(count, 0);
}

// ===== parse_completion 静态方法测试 =====

#[test]
fn openai_parse_completion_text_response() {
    let resp = serde_json::json!({
        "id": "chatcmpl-001",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "Hello!"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
    });
    let result = OpenAiProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.id, "chatcmpl-001");
    assert_eq!(result.model, "gpt-4o");
    assert_eq!(result.choices.len(), 1);
    assert_eq!(result.choices[0].message.content, "Hello!");
    assert_eq!(result.choices[0].message.role, Role::Assistant);
    assert_eq!(result.choices[0].finish_reason, Some(FinishReason::Stop));
    assert_eq!(result.usage.total_tokens, 8);
}

#[test]
fn openai_parse_completion_tool_calls() {
    let resp = serde_json::json!({
        "id": "chatcmpl-002",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_abc",
                    "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"Beijing\"}"}
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
    });
    let result = OpenAiProvider::parse_completion(&resp).unwrap();
    let tool_calls = result.choices[0].message.tool_calls.as_ref().unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].name, "get_weather");
    assert_eq!(tool_calls[0].id, "call_abc");
    assert!(tool_calls[0].arguments.contains("Beijing"));
    assert_eq!(
        result.choices[0].finish_reason,
        Some(FinishReason::ToolCalls)
    );
}

#[test]
fn openai_parse_completion_multiple_choices() {
    let resp = serde_json::json!({
        "id": "chatcmpl-003",
        "model": "gpt-4o",
        "choices": [
            {"index": 0, "message": {"role": "assistant", "content": "A"}, "finish_reason": "stop"},
            {"index": 1, "message": {"role": "assistant", "content": "B"}, "finish_reason": "length"}
        ],
        "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
    });
    let result = OpenAiProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.choices.len(), 2);
    assert_eq!(result.choices[0].message.content, "A");
    assert_eq!(result.choices[1].message.content, "B");
    assert_eq!(result.choices[1].finish_reason, Some(FinishReason::Length));
}

#[test]
fn openai_parse_completion_missing_choices_error() {
    let resp = serde_json::json!({"id": "x", "model": "gpt-4o"});
    let result = OpenAiProvider::parse_completion(&resp);
    assert!(result.is_err());
}

#[test]
fn openai_parse_completion_missing_message_in_choice_error() {
    let resp = serde_json::json!({
        "id": "x",
        "model": "gpt-4o",
        "choices": [{"index": 0}]
    });
    let result = OpenAiProvider::parse_completion(&resp);
    assert!(result.is_err());
}

#[test]
fn openai_parse_completion_no_usage_defaults_zero() {
    let resp = serde_json::json!({
        "id": "x",
        "model": "gpt-4o",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}]
    });
    let result = OpenAiProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.usage.prompt_tokens, 0);
    assert_eq!(result.usage.completion_tokens, 0);
    assert_eq!(result.usage.total_tokens, 0);
}

#[test]
fn openai_parse_completion_all_finish_reasons() {
    let cases = [
        ("stop", FinishReason::Stop),
        ("length", FinishReason::Length),
        ("tool_calls", FinishReason::ToolCalls),
        ("content_filter", FinishReason::ContentFilter),
    ];
    for (s, expected) in cases {
        let resp = serde_json::json!({
            "id": "x", "model": "gpt-4o",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": ""}, "finish_reason": s}],
        });
        let result = OpenAiProvider::parse_completion(&resp).unwrap();
        assert_eq!(result.choices[0].finish_reason, Some(expected));
    }
}

#[test]
fn openai_parse_completion_unknown_finish_reason_none() {
    let resp = serde_json::json!({
        "id": "x", "model": "gpt-4o",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": ""}, "finish_reason": "unknown_reason"}],
    });
    let result = OpenAiProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.choices[0].finish_reason, None);
}

#[test]
fn openai_parse_completion_role_mapping() {
    let roles = [
        ("system", Role::System),
        ("user", Role::User),
        ("tool", Role::Tool),
        ("assistant", Role::Assistant),
        ("custom", Role::Assistant),
    ];
    for (s, expected) in roles {
        let resp = serde_json::json!({
            "id": "x", "model": "gpt-4o",
            "choices": [{"index": 0, "message": {"role": s, "content": "c"}, "finish_reason": "stop"}],
        });
        let result = OpenAiProvider::parse_completion(&resp).unwrap();
        assert_eq!(result.choices[0].message.role, expected);
    }
}

#[test]
fn openai_parse_completion_empty_content_defaults_empty() {
    let resp = serde_json::json!({
        "id": "x", "model": "gpt-4o",
        "choices": [{"index": 0, "message": {"role": "assistant"}, "finish_reason": "stop"}],
    });
    let result = OpenAiProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.choices[0].message.content, "");
}

// ===== chat_completion 集成测试（mock 服务器） =====

#[tokio::test]
async fn openai_chat_completion_success_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let body = serde_json::json!({
        "id": "chatcmpl-mock",
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "Mock response"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
    })
    .to_string();

    let server = MockHttpServer::start(vec![MockResponse::json(body)])
        .await
        .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new(
        "gpt-4o",
        vec![ChatMessage {
            role: Role::User,
            content: "Hi".into(),
            tool_call_id: None,
            tool_calls: None,
        }],
    );
    let result = provider.chat_completion(req).await.unwrap();
    assert_eq!(result.id, "chatcmpl-mock");
    assert_eq!(result.choices[0].message.content, "Mock response");

    server.stop();
}

#[tokio::test]
async fn openai_chat_completion_401_auth_error_via_mock() {
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

    let req = ChatRequest::new("gpt-4o", vec![]);
    let err = provider.chat_completion(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_PROVIDER_AUTH_FAILED");

    server.stop();
}

#[tokio::test]
async fn openai_chat_completion_429_rate_limited_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let server = MockHttpServer::start(vec![MockResponse {
        status: 429,
        body: "rate limited".into(),
        delay_ms: 0,
        headers: vec![],
    }])
    .await
    .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new("gpt-4o", vec![]);
    let err = provider.chat_completion(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_RATE_LIMITED");

    server.stop();
}

#[tokio::test]
async fn openai_chat_completion_500_provider_unavailable_via_mock() {
    use common::mock_server::{MockHttpServer, MockResponse};

    let server = MockHttpServer::start(vec![MockResponse {
        status: 500,
        body: "internal error".into(),
        delay_ms: 0,
        headers: vec![],
    }])
    .await
    .unwrap();
    let provider = make_provider(server.base_url());

    let req = ChatRequest::new("gpt-4o", vec![]);
    let err = provider.chat_completion(req).await.unwrap_err();
    assert_eq!(err.error_code(), "AI_PROVIDER_UNAVAILABLE");

    server.stop();
}
