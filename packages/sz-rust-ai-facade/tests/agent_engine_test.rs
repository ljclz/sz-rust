// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Agent engine 补充测试：with_model / max_tokens / timeout 终止路径

mod common;

use std::sync::Arc;
use sz_rust_ai_facade::agent::engine::{Agent, AgentOptions, AgentTask};
use sz_rust_ai_facade::agent::tool::ToolRegistry;
use sz_rust_ai_facade::agent::trace::TerminateReason;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    Usage,
};

use async_trait::async_trait;
use futures::stream::BoxStream;

/// 固定返回指定 token 数的 Provider，用于触发 max_tokens 终止
struct FixedTokenProvider {
    total_tokens: u32,
}

#[async_trait]
impl LlmProvider for FixedTokenProvider {
    fn name(&self) -> &str {
        "fixed-token"
    }
    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        Ok(ChatCompletion {
            id: "fixed".into(),
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "answer".into(),
                    tool_call_id: None,
                    tool_calls: None,
                },
                finish_reason: Some(FinishReason::Stop),
            }],
            usage: Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: self.total_tokens,
            },
        })
    }
    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        Err(AiError::Internal("not impl".into()))
    }
    async fn token_count(&self, messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(messages
            .iter()
            .map(|m| m.content.text_or_empty().len() as u32)
            .sum())
    }
    fn supported_models(&self) -> &[&str] {
        &["fixed-model"]
    }
}

#[tokio::test]
async fn agent_with_model_uses_custom_model() {
    let llm = Arc::new(FixedTokenProvider { total_tokens: 5 }) as Arc<dyn LlmProvider>;
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(llm, tools).with_model("custom-model");
    let task = AgentTask::new("test");
    let opts = AgentOptions::new("tenant");
    let result = agent.run(task, opts).await.unwrap();
    assert_eq!(result.final_answer, "answer");
    assert_eq!(result.trace.terminated_by, TerminateReason::Natural);
}

/// 返回 tool_call 的 Provider，用于让 agent 持续循环以触发 max_tokens/timeout
struct ToolCallLoopProvider {
    total_tokens: u32,
}

#[async_trait]
impl LlmProvider for ToolCallLoopProvider {
    fn name(&self) -> &str {
        "tool-call-loop"
    }
    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
        Ok(ChatCompletion {
            id: "loop".into(),
            model: req.model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "calling".into(),
                    tool_call_id: None,
                    tool_calls: Some(vec![sz_rust_ai_facade::llm::provider::ToolCall {
                        id: "tc1".into(),
                        name: "nonexistent_tool".into(),
                        arguments: "{}".into(),
                    }]),
                },
                finish_reason: Some(FinishReason::ToolCalls),
            }],
            usage: Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: self.total_tokens,
            },
        })
    }
    async fn stream_completion(
        &self,
        _req: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
        Err(AiError::Internal("not impl".into()))
    }
    async fn token_count(&self, _messages: &[ChatMessage]) -> Result<u32, AiError> {
        Ok(0)
    }
    fn supported_models(&self) -> &[&str] {
        &["loop-model"]
    }
}

#[tokio::test]
async fn agent_max_tokens_termination_path() {
    // 每步返回 tool_call（不在 allow_tools 中），累积 100 tokens，max_tokens=50
    // 第一次循环：check(0,0)=None，LLM→total_tokens=100，tool 不在白名单→continue
    // 第二次循环：check(1,100)，100>=50 → MaxTokens
    let llm = Arc::new(ToolCallLoopProvider { total_tokens: 100 }) as Arc<dyn LlmProvider>;
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(llm, tools);
    let task = AgentTask::new("test");
    let mut opts = AgentOptions::new("tenant");
    opts.max_tokens = Some(50);
    let result = agent.run(task, opts).await.unwrap();
    assert_eq!(result.trace.terminated_by, TerminateReason::MaxTokens);
}

#[tokio::test]
async fn agent_timeout_termination_path() {
    // 每步返回 tool_call，timeout=1ns，第二步 check 触发 Timeout
    let llm = Arc::new(ToolCallLoopProvider { total_tokens: 5 }) as Arc<dyn LlmProvider>;
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(llm, tools);
    let task = AgentTask::new("test");
    let mut opts = AgentOptions::new("tenant");
    opts.max_steps = Some(100);
    opts.timeout = Some(std::time::Duration::from_nanos(1));
    let result = agent.run(task, opts).await.unwrap();
    assert_eq!(result.trace.terminated_by, TerminateReason::Timeout);
}

#[tokio::test]
async fn agent_max_steps_termination_path() {
    let llm = Arc::new(FixedTokenProvider { total_tokens: 5 }) as Arc<dyn LlmProvider>;
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(llm, tools);
    let task = AgentTask::new("test");
    let mut opts = AgentOptions::new("tenant");
    opts.max_steps = Some(0); // 立即触发 max_steps
    let result = agent.run(task, opts).await.unwrap();
    assert_eq!(result.trace.terminated_by, TerminateReason::MaxSteps);
    assert!(result.final_answer.is_empty());
}

#[tokio::test]
async fn agent_with_context_messages() {
    let llm = Arc::new(FixedTokenProvider { total_tokens: 5 }) as Arc<dyn LlmProvider>;
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(llm, tools);
    let mut task = AgentTask::new("system instruction");
    task.context.push(ChatMessage {
        role: Role::User,
        content: "context msg".into(),
        tool_call_id: None,
        tool_calls: None,
    });
    let opts = AgentOptions::new("tenant");
    let result = agent.run(task, opts).await.unwrap();
    assert_eq!(result.final_answer, "answer");
}

#[tokio::test]
async fn agent_tool_failure_records_error_in_trace() {
    use sz_rust_ai_facade::agent::tool::Tool;

    struct FailTool;
    #[async_trait]
    impl Tool for FailTool {
        fn name(&self) -> &str {
            "fail_tool"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::Value::Null
        }
        async fn call(&self, _: &serde_json::Value) -> Result<serde_json::Value, AiError> {
            Err(AiError::ToolExecution("boom".into()))
        }
    }

    // Provider 返回 tool_call
    struct ToolCallProvider;
    #[async_trait]
    impl LlmProvider for ToolCallProvider {
        fn name(&self) -> &str {
            "tool-call"
        }
        async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
            Ok(ChatCompletion {
                id: "tc".into(),
                model: req.model,
                choices: vec![Choice {
                    index: 0,
                    message: ChatMessage {
                        role: Role::Assistant,
                        content: "calling tool".into(),
                        tool_call_id: None,
                        tool_calls: Some(vec![sz_rust_ai_facade::llm::provider::ToolCall {
                            id: "tc1".into(),
                            name: "fail_tool".into(),
                            arguments: "{}".into(),
                        }]),
                    },
                    finish_reason: Some(FinishReason::ToolCalls),
                }],
                usage: Usage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    total_tokens: 0,
                },
            })
        }
        async fn stream_completion(
            &self,
            _req: ChatRequest,
        ) -> Result<BoxStream<'static, Result<StreamDelta, AiError>>, AiError> {
            Err(AiError::Internal("not impl".into()))
        }
        async fn token_count(&self, _messages: &[ChatMessage]) -> Result<u32, AiError> {
            Ok(0)
        }
        fn supported_models(&self) -> &[&str] {
            &["tc-model"]
        }
    }

    let llm = Arc::new(ToolCallProvider) as Arc<dyn LlmProvider>;
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FailTool));
    let tools = Arc::new(registry);
    let agent = Agent::new(llm, tools);

    let task = AgentTask::new("use tool");
    let mut opts = AgentOptions::new("tenant");
    opts.allow_tools = vec!["fail_tool".into()];
    opts.max_steps = Some(2);
    let result = agent.run(task, opts).await.unwrap();
    // 第一步工具失败，第二步无 tool_call 自然终止
    assert!(!result.trace.steps.is_empty());
    // 第一步的 observation 应包含 "failed"
    assert!(result.trace.steps[0].observation.contains("failed"));
}
