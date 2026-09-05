// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use sz_rust_ai_facade::agent::engine::{Agent, AgentOptions, AgentResult, AgentTask};
use sz_rust_ai_facade::agent::tool::ToolRegistry;
use sz_rust_ai_facade::agent::trace::TerminateReason;
use sz_rust_ai_facade::common::AiError;
use sz_rust_ai_facade::llm::provider::{
    ChatCompletion, ChatMessage, ChatRequest, Choice, FinishReason, LlmProvider, Role, StreamDelta,
    ToolCall, Usage,
};

struct ScriptedLlm {
    responses: Vec<ChatCompletion>,
    call_index: Arc<AtomicU32>,
}
#[async_trait]
impl LlmProvider for ScriptedLlm {
    fn name(&self) -> &str {
        "scripted"
    }
    async fn chat_completion(&self, _req: ChatRequest) -> Result<ChatCompletion, AiError> {
        let idx = self.call_index.fetch_add(1, Ordering::SeqCst) as usize;
        if idx < self.responses.len() {
            Ok(self.responses[idx].clone())
        } else {
            Ok(ChatCompletion {
                id: "default".into(),
                model: "gpt-4o".into(),
                choices: vec![Choice {
                    index: 0,
                    message: ChatMessage {
                        role: Role::Assistant,
                        content: "Done".into(),
                        tool_call_id: None,
                        tool_calls: None,
                    },
                    finish_reason: Some(FinishReason::Stop),
                }],
                usage: Usage {
                    prompt_tokens: 5,
                    completion_tokens: 5,
                    total_tokens: 10,
                },
            })
        }
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

fn make_completion(content: &str, tool_calls: Option<Vec<ToolCall>>) -> ChatCompletion {
    ChatCompletion {
        id: "chatcmpl-test".into(),
        model: "gpt-4o".into(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: Role::Assistant,
                content: content.into(),
                tool_call_id: None,
                tool_calls,
            },
            finish_reason: Some(FinishReason::Stop),
        }],
        usage: Usage {
            prompt_tokens: 10,
            completion_tokens: 10,
            total_tokens: 20,
        },
    }
}

#[tokio::test]
async fn agent_natural_termination_no_tools() {
    let llm = ScriptedLlm {
        responses: vec![make_completion("The answer is 42", None)],
        call_index: Arc::new(AtomicU32::new(0)),
    };
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(Arc::new(llm) as Arc<dyn LlmProvider>, tools);
    let task = AgentTask::new("What is the answer?");
    let opts = AgentOptions::new("tenant-1");
    let result: AgentResult = agent.run(task, opts).await.unwrap();
    assert_eq!(result.final_answer, "The answer is 42");
    assert_eq!(result.trace.terminated_by, TerminateReason::Natural);
    assert_eq!(result.trace.steps.len(), 0);
}

#[tokio::test]
async fn agent_max_steps_termination() {
    let tool_call = ToolCall {
        id: "call_1".into(),
        name: "echo".into(),
        arguments: "{}".into(),
    };
    let llm = ScriptedLlm {
        responses: vec![
            make_completion("thinking", Some(vec![tool_call.clone()])),
            make_completion("thinking", Some(vec![tool_call.clone()])),
            make_completion("thinking", Some(vec![tool_call.clone()])),
            make_completion("thinking", Some(vec![tool_call])),
        ],
        call_index: Arc::new(AtomicU32::new(0)),
    };
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(Arc::new(llm) as Arc<dyn LlmProvider>, tools);
    let task = AgentTask::new("Keep calling tools");
    let mut opts = AgentOptions::new("tenant-1");
    opts.max_steps = Some(3);
    let result = agent.run(task, opts).await.unwrap();
    assert_eq!(result.trace.terminated_by, TerminateReason::MaxSteps);
}

#[tokio::test]
async fn agent_tool_whitelist_rejection() {
    let tool_call = ToolCall {
        id: "call_1".into(),
        name: "forbidden_tool".into(),
        arguments: "{}".into(),
    };
    let llm = ScriptedLlm {
        responses: vec![
            make_completion("calling forbidden", Some(vec![tool_call])),
            make_completion("final answer", None),
        ],
        call_index: Arc::new(AtomicU32::new(0)),
    };
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(Arc::new(llm) as Arc<dyn LlmProvider>, tools);
    let task = AgentTask::new("Call a forbidden tool");
    let mut opts = AgentOptions::new("tenant-1");
    opts.max_steps = Some(5);
    let result = agent.run(task, opts).await.unwrap();
    assert!(result
        .trace
        .steps
        .iter()
        .any(|s| s.observation.contains("not in allow_tools")));
}

#[tokio::test]
async fn agent_trace_records_steps() {
    let tool_call = ToolCall {
        id: "call_1".into(),
        name: "echo".into(),
        arguments: "{}".into(),
    };
    let llm = ScriptedLlm {
        responses: vec![
            make_completion("thinking", Some(vec![tool_call])),
            make_completion("final answer", None),
        ],
        call_index: Arc::new(AtomicU32::new(0)),
    };
    let mut registry = ToolRegistry::new();
    struct EchoTool;
    #[async_trait]
    impl sz_rust_ai_facade::agent::tool::Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::Value::Null
        }
        async fn call(&self, args: &serde_json::Value) -> Result<serde_json::Value, AiError> {
            Ok(args.clone())
        }
    }
    registry.register(Box::new(EchoTool));
    let tools = Arc::new(registry);
    let agent = Agent::new(Arc::new(llm) as Arc<dyn LlmProvider>, tools);
    let task = AgentTask::new("Use echo tool");
    let mut opts = AgentOptions::new("tenant-1");
    opts.allow_tools = vec!["echo".into()];
    opts.max_steps = Some(5);
    let result = agent.run(task, opts).await.unwrap();
    assert!(!result.trace.steps.is_empty());
    for step in &result.trace.steps {
        assert!(step.tool_call.is_some());
        assert!(step.tool_result.is_some());
        assert!(!step.observation.is_empty());
    }
}

#[tokio::test]
async fn agent_with_context_messages() {
    let llm = ScriptedLlm {
        responses: vec![make_completion("answer with context", None)],
        call_index: Arc::new(AtomicU32::new(0)),
    };
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(Arc::new(llm) as Arc<dyn LlmProvider>, tools);
    let mut task = AgentTask::new("Answer based on context");
    task.context.push(ChatMessage {
        role: Role::User,
        content: "Context: Rust is great".into(),
        tool_call_id: None,
        tool_calls: None,
    });
    let opts = AgentOptions::new("tenant-1");
    let result = agent.run(task, opts).await.unwrap();
    assert_eq!(result.final_answer, "answer with context");
}

#[tokio::test]
async fn agent_total_tokens_accumulated() {
    let llm = ScriptedLlm {
        responses: vec![make_completion("answer", None)],
        call_index: Arc::new(AtomicU32::new(0)),
    };
    let tools = Arc::new(ToolRegistry::new());
    let agent = Agent::new(Arc::new(llm) as Arc<dyn LlmProvider>, tools);
    let task = AgentTask::new("query");
    let opts = AgentOptions::new("tenant-1");
    let result = agent.run(task, opts).await.unwrap();
    assert!(result.trace.total_tokens > 0);
}
