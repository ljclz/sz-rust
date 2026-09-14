// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! T1.6 Agent 多轮工具调用轨迹端到端测试

mod common;

use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use sz_rust_ai_facade::agent::engine::{Agent, AgentOptions, AgentTask};
use sz_rust_ai_facade::agent::tool::{Tool, ToolRegistry};
use sz_rust_ai_facade::agent::trace::TerminateReason;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    ToolCall, Usage,
};

struct ScriptedLlm {
    call_index: Arc<AtomicU32>,
}

#[async_trait]
impl LlmProvider for ScriptedLlm {
    fn name(&self) -> &str {
        "scripted"
    }
    async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
        let idx = self.call_index.fetch_add(1, Ordering::SeqCst);
        let (content, tool_calls) = match idx {
            0 => (
                "Let me check.".into(),
                Some(vec![ToolCall {
                    id: "call_1".into(),
                    name: "tool_a".into(),
                    arguments: "{\"q\": \"test\"}".into(),
                }]),
            ),
            1 => (
                "Checking tool_b.".into(),
                Some(vec![ToolCall {
                    id: "call_2".into(),
                    name: "tool_b".into(),
                    arguments: "{}".into(),
                }]),
            ),
            _ => ("Done".into(), None),
        };
        Ok(ChatCompletion {
            id: format!("resp-{}", idx),
            model: "gpt-4o".into(),
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content,
                    tool_call_id: None,
                    tool_calls,
                },
                finish_reason: Some(if idx >= 2 {
                    FinishReason::Stop
                } else {
                    FinishReason::ToolCalls
                }),
            }],
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            },
        })
    }
    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        Err(AiError::Internal("not supported".into()))
    }
    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages
            .iter()
            .map(|m| m.content.text_or_empty().len() as u32)
            .sum())
    }
    fn supported_models(&self) -> &[&str] {
        &["gpt-4o"]
    }
}

struct MockTool {
    name: String,
}

#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    async fn call(&self, _args: &serde_json::Value) -> Result<serde_json::Value, AiError> {
        Ok(serde_json::json!({"result": format!("from {}", self.name)}))
    }
}

#[tokio::test]
async fn it_agent_multi_round_tool_trace() {
    let llm = Arc::new(ScriptedLlm {
        call_index: Arc::new(AtomicU32::new(0)),
    });
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool {
        name: "tool_a".into(),
    }));
    registry.register(Box::new(MockTool {
        name: "tool_b".into(),
    }));

    let agent = Agent::new(llm, Arc::new(registry));
    let task = AgentTask::new("Test task");
    let mut opts = AgentOptions::new("tenant-1");
    opts.max_steps = Some(5);
    opts.allow_tools = vec!["tool_a".into(), "tool_b".into()];

    let result = agent.run(task, opts).await.unwrap();

    assert_eq!(result.trace.terminated_by, TerminateReason::Natural);
    assert_eq!(result.trace.steps.len(), 2);
    for step in &result.trace.steps {
        // duration_ms 为无符号类型（u64），恒 >= 0；此断言验证字段可访问（编译期）
        let _ = step.duration_ms;
    }
}
