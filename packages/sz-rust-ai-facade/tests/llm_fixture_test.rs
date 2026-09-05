// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
#![cfg(all(feature = "claude", feature = "gemini"))]
//! Claude/Gemini Provider 响应解析 fixture 测试
//!
//! 使用模拟的 API 响应 JSON 测试 parse_completion 方法，
//! 不需要真实 API Key 或网络请求。

use sz_rust_ai_facade::llm::claude::ClaudeProvider;
use sz_rust_ai_facade::llm::gemini::GeminiProvider;
use sz_rust_ai_facade::llm::provider::{FinishReason, Role};

// ===== Claude fixture 测试 =====

#[test]
fn claude_parse_text_response() {
    let resp = serde_json::json!({
        "id": "msg_01abc",
        "model": "claude-3-opus-20240229",
        "content": [
            {"type": "text", "text": "Hello! How can I help you?"}
        ],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 8}
    });
    let result = ClaudeProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.id, "msg_01abc");
    assert_eq!(result.model, "claude-3-opus-20240229");
    assert_eq!(result.choices.len(), 1);
    assert_eq!(
        result.choices[0].message.content.as_text(),
        Some("Hello! How can I help you?")
    );
    assert_eq!(result.choices[0].message.role, Role::Assistant);
    assert_eq!(result.choices[0].finish_reason, Some(FinishReason::Stop));
    assert_eq!(result.usage.prompt_tokens, 10);
    assert_eq!(result.usage.completion_tokens, 8);
}

#[test]
fn claude_parse_tool_use_response() {
    let resp = serde_json::json!({
        "id": "msg_02def",
        "model": "claude-3-sonnet-20240229",
        "content": [
            {"type": "text", "text": "Let me check that for you."},
            {"type": "tool_use", "id": "toolu_01", "name": "get_weather", "input": {"city": "Beijing"}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 15, "output_tokens": 20}
    });
    let result = ClaudeProvider::parse_completion(&resp).unwrap();
    assert_eq!(
        result.choices[0].finish_reason,
        Some(FinishReason::ToolCalls)
    );
    let tool_calls = result.choices[0].message.tool_calls.as_ref().unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].name, "get_weather");
    assert_eq!(tool_calls[0].id, "toolu_01");
    assert!(tool_calls[0].arguments.contains("Beijing"));
}

#[test]
fn claude_parse_max_tokens_stop() {
    let resp = serde_json::json!({
        "id": "msg_03",
        "model": "claude-3-haiku",
        "content": [{"type": "text", "text": "Partial response..."}],
        "stop_reason": "max_tokens",
        "usage": {"input_tokens": 5, "output_tokens": 100}
    });
    let result = ClaudeProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.choices[0].finish_reason, Some(FinishReason::Length));
}

#[test]
fn claude_parse_missing_content_error() {
    let resp = serde_json::json!({"id": "msg_04", "model": "claude-3"});
    let result = ClaudeProvider::parse_completion(&resp);
    assert!(result.is_err());
}

#[test]
fn claude_parse_empty_content_array() {
    let resp = serde_json::json!({
        "id": "msg_05",
        "model": "claude-3",
        "content": [],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 0, "output_tokens": 0}
    });
    let result = ClaudeProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.choices[0].message.content.as_text(), Some(""));
}

// ===== Gemini fixture 测试 =====

#[test]
fn gemini_parse_text_response() {
    let resp = serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [{"text": "Hi there!"}],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 5}
    });
    let result = GeminiProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.choices.len(), 1);
    assert_eq!(
        result.choices[0].message.content.as_text(),
        Some("Hi there!")
    );
    assert_eq!(result.choices[0].message.role, Role::Assistant);
    assert_eq!(result.choices[0].finish_reason, Some(FinishReason::Stop));
}

#[test]
fn gemini_parse_function_call_response() {
    let resp = serde_json::json!({
        "candidates": [{
            "content": {
                "parts": [
                    {"text": "Let me search."},
                    {"functionCall": {"name": "search", "args": {"query": "weather"}}}
                ],
                "role": "model"
            },
            "finishReason": "STOP",
            "index": 0
        }]
    });
    let result = GeminiProvider::parse_completion(&resp).unwrap();
    let tool_calls = result.choices[0].message.tool_calls.as_ref().unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].name, "search");
    assert!(tool_calls[0].arguments.contains("weather"));
}

#[test]
fn gemini_parse_max_tokens_stop() {
    let resp = serde_json::json!({
        "candidates": [{
            "content": {"parts": [{"text": "Truncated..."}], "role": "model"},
            "finishReason": "MAX_TOKENS",
            "index": 0
        }]
    });
    let result = GeminiProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.choices[0].finish_reason, Some(FinishReason::Length));
}

#[test]
fn gemini_parse_safety_stop() {
    let resp = serde_json::json!({
        "candidates": [{
            "content": {"parts": [{"text": ""}], "role": "model"},
            "finishReason": "SAFETY",
            "index": 0
        }]
    });
    let result = GeminiProvider::parse_completion(&resp).unwrap();
    assert_eq!(
        result.choices[0].finish_reason,
        Some(FinishReason::ContentFilter)
    );
}

#[test]
fn gemini_parse_missing_candidates_error() {
    let resp = serde_json::json!({"error": {"message": "Invalid request"}});
    let result = GeminiProvider::parse_completion(&resp);
    assert!(result.is_err());
}

#[test]
fn gemini_parse_multiple_candidates() {
    let resp = serde_json::json!({
        "candidates": [
            {"content": {"parts": [{"text": "Answer 1"}], "role": "model"}, "finishReason": "STOP", "index": 0},
            {"content": {"parts": [{"text": "Answer 2"}], "role": "model"}, "finishReason": "STOP", "index": 1}
        ]
    });
    let result = GeminiProvider::parse_completion(&resp).unwrap();
    assert_eq!(result.choices.len(), 2);
    assert_eq!(
        result.choices[0].message.content.as_text(),
        Some("Answer 1")
    );
    assert_eq!(
        result.choices[1].message.content.as_text(),
        Some("Answer 2")
    );
}
