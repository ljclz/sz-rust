// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use async_trait::async_trait;
use serde_json::Value;
use sz_rust_capability::{CapError, CapResult, Capability, CapabilitySource};

use crate::facade::Ai;
use crate::llm::provider::{ChatCompletion, ChatRequest};

/// LLM 对话能力，委托 [`Ai::chat`] 执行。
///
/// 实现 [`Capability`] trait，注册到 Capability Registry 后，
/// AI Agent 可通过 `Cap::call("ai.llm_chat", args)` 调用 LLM 对话。
///
/// # 参数格式
///
/// `args` 为 [`ChatRequest`] 的 JSON 序列化形式：
///
/// ```json
/// {
///   "model": "gpt-4",
///   "messages": [{"role": "user", "content": "你好"}],
///   "max_tokens": 1000,
///   "temperature": 0.7
/// }
/// ```
///
/// # 返回格式
///
/// 返回 [`ChatCompletion`] 的 JSON 序列化形式。
pub struct LlmChatCapability;

#[async_trait]
impl Capability for LlmChatCapability {
    fn name(&self) -> &'static str {
        "ai.llm_chat"
    }

    fn description(&self) -> &'static str {
        "LLM 对话能力，委托 ai-facade LlmProvider 执行"
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "model": { "type": "string", "description": "模型名称" },
                "messages": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "role": { "type": "string", "enum": ["system", "user", "assistant", "tool"] },
                            "content": { "type": "string" }
                        },
                        "required": ["role", "content"]
                    }
                },
                "max_tokens": { "type": "number", "description": "最大生成 token 数" },
                "temperature": { "type": "number", "description": "采样温度 0-2" }
            },
            "required": ["model", "messages"]
        })
    }

    fn tags(&self) -> &[&'static str] {
        &["ai", "llm", "chat", "reasoning"]
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Skill
    }

    async fn call(&self, args: Value) -> CapResult<Value> {
        let req: ChatRequest = serde_json::from_value(args)
            .map_err(|e| CapError::ValidationError(format!("ChatRequest 解析失败: {e}")))?;

        let completion: ChatCompletion = Ai::chat(req).await.map_err(map_ai_error)?;

        serde_json::to_value(completion)
            .map_err(|e| CapError::ExecutionError(format!("ChatCompletion 序列化失败: {e}")))
    }
}

fn map_ai_error(e: crate::common::AiError) -> CapError {
    use crate::common::AiError;
    match e {
        AiError::ToolNotAuthorized(msg) => CapError::PermissionDenied(msg),
        AiError::ProviderAuthFailed(msg) => CapError::PermissionDenied(msg),
        AiError::ToolExecution(msg) => CapError::ExecutionError(msg),
        other => CapError::ExecutionError(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_metadata() {
        let cap = LlmChatCapability;
        assert_eq!(cap.name(), "ai.llm_chat");
        assert_eq!(cap.source(), CapabilitySource::Skill);
        assert_eq!(cap.tags(), &["ai", "llm", "chat", "reasoning"]);
        assert!(!cap.requires_confirmation());
        assert_eq!(cap.version(), "1.0.0");
    }

    #[test]
    fn test_schema_structure() {
        let cap = LlmChatCapability;
        let schema = cap.schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["model"].is_object());
        assert!(schema["properties"]["messages"].is_object());
        assert!(schema["required"].as_array().unwrap().len() >= 2);
    }
}
