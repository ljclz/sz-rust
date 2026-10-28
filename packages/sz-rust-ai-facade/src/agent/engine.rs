// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::agent::memory::ShortTermMemory;
use crate::agent::termination::TerminationPolicy;
use crate::agent::tool::ToolRegistry;
use crate::agent::trace::{AgentStep, AgentTrace, TerminateReason};
use crate::common::AiError;
use crate::llm::provider::{ChatMessage, ChatRequest, LlmProvider, Role};
use crate::rag::citation::Citation;
use crate::rag::pipeline::RagPipeline;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct AgentTask {
    pub instruction: String,
    pub context: Vec<ChatMessage>,
}

impl AgentTask {
    pub fn new(instruction: impl Into<String>) -> Self {
        Self {
            instruction: instruction.into(),
            context: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentOptions {
    pub max_steps: Option<u32>,
    pub max_tokens: Option<u32>,
    pub timeout: Option<Duration>,
    pub allow_tools: Vec<String>,
    pub tenant_id: String,
}

impl AgentOptions {
    pub fn new(tenant_id: impl Into<String>) -> Self {
        Self {
            max_steps: Some(25),
            max_tokens: None,
            timeout: None,
            allow_tools: Vec::new(),
            tenant_id: tenant_id.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub final_answer: String,
    pub trace: AgentTrace,
    pub citations: Vec<Citation>,
}

pub struct Agent {
    llm: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    model: String,
    rag_pipeline: Option<Arc<RagPipeline>>,
}

impl Agent {
    pub fn new(llm: Arc<dyn LlmProvider>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            llm,
            tools,
            model: "gpt-4o".to_string(),
            rag_pipeline: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_rag_pipeline(mut self, rag: Arc<RagPipeline>) -> Self {
        self.rag_pipeline = Some(rag);
        self
    }

    pub async fn run(&self, task: AgentTask, opts: AgentOptions) -> Result<AgentResult, AiError> {
        let max_steps = opts.max_steps.unwrap_or(25);
        let mut policy = TerminationPolicy::new(max_steps);
        if let Some(max_tokens) = opts.max_tokens {
            policy = policy.with_max_tokens(max_tokens);
        }
        if let Some(timeout) = opts.timeout {
            policy = policy.with_timeout(timeout);
        }

        let mut citations: Vec<Citation> = Vec::new();

        let mut memory = ShortTermMemory::new(100);
        memory.push(ChatMessage {
            role: Role::System,
            content: task.instruction.clone().into(),
            tool_call_id: None,
            tool_calls: None,
        });

        if let Some(ref rag) = self.rag_pipeline {
            match rag.retrieve(&task.instruction, 5).await {
                Ok(hits) => {
                    citations = hits
                        .iter()
                        .enumerate()
                        .map(|(i, hit)| Citation {
                            doc_id: hit.id.clone(),
                            offset: i as u32,
                            length: hit.text.len() as u32,
                            score: hit.score,
                            text: hit.text.clone(),
                        })
                        .collect();
                    if !hits.is_empty() {
                        let context = rag.assemble(&hits, 2000).await.unwrap_or_default();
                        if !context.is_empty() {
                            memory.push(ChatMessage {
                                role: Role::System,
                                content: format!("Retrieved context:\n{context}").into(),
                                tool_call_id: None,
                                tool_calls: None,
                            });
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(target: "ai_agent", "RAG retrieval failed: {e}");
                }
            }
        }

        for msg in &task.context {
            memory.push(msg.clone());
        }

        let mut trace = AgentTrace::new();
        let start = Instant::now();
        let mut total_tokens = 0u32;
        let mut final_answer = String::new();

        loop {
            let step_start = Instant::now();

            if let Some(reason) =
                policy.check(trace.steps.len() as u32, total_tokens, start.elapsed())
            {
                trace.terminated_by = reason;
                trace.total_duration_ms = start.elapsed().as_millis() as u64;
                trace.total_tokens = total_tokens;
                return Ok(AgentResult {
                    final_answer,
                    trace,
                    citations,
                });
            }

            let messages = memory.messages().to_vec();
            let req = ChatRequest::new(&self.model, messages);
            let completion = self.llm.chat_completion(req).await?;
            total_tokens += completion.usage.total_tokens;

            let choice = completion
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| AiError::Internal("LLM returned no choices".to_string()))?;

            let thought = choice.message.content.clone();
            let tool_calls = choice.message.tool_calls.clone();

            if let Some(ref calls) = tool_calls {
                for tool_call in calls {
                    if !opts.allow_tools.iter().any(|t| t == &tool_call.name) {
                        let step = AgentStep {
                            thought: String::new(),
                            tool_call: Some(tool_call.clone()),
                            tool_result: Some(serde_json::json!({"error": "tool not authorized"})),
                            observation: format!(
                                "Tool '{}' not in allow_tools whitelist",
                                tool_call.name
                            ),
                            duration_ms: step_start.elapsed().as_millis() as u64,
                        };
                        trace.steps.push(step);
                        continue;
                    }

                    let args: serde_json::Value = serde_json::from_str(&tool_call.arguments)
                        .unwrap_or(serde_json::Value::Null);
                    let tool_result = self.tools.call(&tool_call.name, &args).await;

                    let (result_val, observation) = match tool_result {
                        Ok(v) => (
                            v.clone(),
                            format!("Tool '{}' executed successfully", tool_call.name),
                        ),
                        Err(e) => (
                            serde_json::json!({"error": e.to_string()}),
                            format!("Tool '{}' failed: {}", tool_call.name, e),
                        ),
                    };

                    let step = AgentStep {
                        thought: thought.to_string(),
                        tool_call: Some(tool_call.clone()),
                        tool_result: Some(result_val.clone()),
                        observation,
                        duration_ms: step_start.elapsed().as_millis() as u64,
                    };
                    trace.steps.push(step);

                    memory.push(ChatMessage {
                        role: Role::Assistant,
                        content: thought.clone(),
                        tool_call_id: None,
                        tool_calls: Some(vec![tool_call.clone()]),
                    });
                    memory.push(ChatMessage {
                        role: Role::Tool,
                        content: result_val.to_string().into(),
                        tool_call_id: Some(tool_call.id.clone()),
                        tool_calls: None,
                    });
                }
            } else {
                final_answer = thought.to_string();
                memory.push(ChatMessage {
                    role: Role::Assistant,
                    content: thought,
                    tool_call_id: None,
                    tool_calls: None,
                });
                trace.terminated_by = TerminateReason::Natural;
                trace.total_duration_ms = start.elapsed().as_millis() as u64;
                trace.total_tokens = total_tokens;
                return Ok(AgentResult {
                    final_answer,
                    trace,
                    citations,
                });
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{
        ChatCompletion, Choice, FinishReason, LlmProvider, StreamDelta, ToolCall, Usage,
    };
    use async_trait::async_trait;
    use futures::stream::BoxStream;

    struct MockLlmProvider {
        response: String,
        tool_calls: Option<Vec<ToolCall>>,
    }

    #[async_trait]
    impl LlmProvider for MockLlmProvider {
        fn name(&self) -> &str {
            "mock-llm"
        }
        async fn chat_completion(&self, req: ChatRequest) -> Result<ChatCompletion, AiError> {
            let message = ChatMessage {
                role: Role::Assistant,
                content: self.response.clone().into(),
                tool_call_id: None,
                tool_calls: self.tool_calls.clone(),
            };
            Ok(ChatCompletion {
                id: "mock-id".into(),
                model: req.model,
                choices: vec![Choice {
                    index: 0,
                    message,
                    finish_reason: Some(FinishReason::Stop),
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
            Err(AiError::Internal("not implemented".into()))
        }
        async fn token_count(&self, _messages: &[ChatMessage]) -> Result<u32, AiError> {
            Ok(0)
        }
        fn supported_models(&self) -> &[&str] {
            &["mock"]
        }
    }

    fn make_agent(response: &str) -> Agent {
        let llm = Arc::new(MockLlmProvider {
            response: response.into(),
            tool_calls: None,
        });
        let tools = Arc::new(ToolRegistry::new());
        Agent::new(llm, tools)
    }

    #[test]
    fn agent_task_new() {
        let task = AgentTask::new("do something");
        assert_eq!(task.instruction, "do something");
        assert!(task.context.is_empty());
    }

    #[test]
    fn agent_options_new_defaults() {
        let opts = AgentOptions::new("tenant-1");
        assert_eq!(opts.max_steps, Some(25));
        assert_eq!(opts.max_tokens, None);
        assert_eq!(opts.timeout, None);
        assert!(opts.allow_tools.is_empty());
        assert_eq!(opts.tenant_id, "tenant-1");
    }

    #[tokio::test]
    async fn agent_run_natural_termination() {
        let agent = make_agent("final answer");
        let task = AgentTask::new("answer the question");
        let opts = AgentOptions::new("tenant-1");
        let result = agent.run(task, opts).await.unwrap();
        assert_eq!(result.final_answer, "final answer");
        assert_eq!(result.trace.terminated_by, TerminateReason::Natural);
        assert!(result.citations.is_empty());
        assert!(result.trace.steps.is_empty());
    }

    #[tokio::test]
    async fn agent_run_max_steps_termination() {
        let llm = Arc::new(MockLlmProvider {
            response: "thinking".into(),
            tool_calls: Some(vec![ToolCall {
                id: "call-1".into(),
                name: "some_tool".into(),
                arguments: "{}".into(),
            }]),
        });
        let tools = Arc::new(ToolRegistry::new());
        let agent = Agent::new(llm, tools);
        let task = AgentTask::new("do something");
        let opts = AgentOptions {
            max_steps: Some(1),
            max_tokens: None,
            timeout: None,
            allow_tools: vec![],
            tenant_id: "tenant-1".to_string(),
        };
        let result = agent.run(task, opts).await.unwrap();
        assert_eq!(result.trace.terminated_by, TerminateReason::MaxSteps);
        assert!(!result.trace.steps.is_empty());
    }

    #[tokio::test]
    async fn agent_run_unauthorized_tool_recorded() {
        let llm = Arc::new(MockLlmProvider {
            response: "thinking".into(),
            tool_calls: Some(vec![ToolCall {
                id: "call-1".into(),
                name: "unauthorized_tool".into(),
                arguments: "{}".into(),
            }]),
        });
        let tools = Arc::new(ToolRegistry::new());
        let agent = Agent::new(llm, tools);
        let task = AgentTask::new("do something");
        let opts = AgentOptions {
            max_steps: Some(1),
            max_tokens: None,
            timeout: None,
            allow_tools: vec![],
            tenant_id: "tenant-1".to_string(),
        };
        let result = agent.run(task, opts).await.unwrap();
        assert_eq!(result.trace.terminated_by, TerminateReason::MaxSteps);
        assert_eq!(result.trace.steps.len(), 1);
        assert!(result.trace.steps[0].tool_call.is_some());
        assert!(result.trace.steps[0]
            .observation
            .contains("not in allow_tools"));
    }

    #[tokio::test]
    async fn agent_run_with_context_messages() {
        let agent = make_agent("answer with context");

        let mut task = AgentTask::new("question");
        task.context.push(ChatMessage {
            role: Role::User,
            content: "additional context".into(),
            tool_call_id: None,
            tool_calls: None,
        });
        let opts = AgentOptions::new("tenant");
        let result = agent.run(task, opts).await.unwrap();
        assert_eq!(result.final_answer, "answer with context");
    }

    #[tokio::test]
    async fn agent_with_model() {
        let llm = Arc::new(MockLlmProvider {
            response: "answer".into(),
            tool_calls: None,
        });
        let tools = Arc::new(ToolRegistry::new());
        let agent = Agent::new(llm, tools).with_model("claude-3");
        let task = AgentTask::new("question");
        let opts = AgentOptions::new("tenant");
        let result = agent.run(task, opts).await.unwrap();
        assert_eq!(result.final_answer, "answer");
    }

    #[tokio::test]
    async fn agent_run_max_tokens_termination() {
        let llm = Arc::new(MockLlmProvider {
            response: "thinking".into(),
            tool_calls: Some(vec![ToolCall {
                id: "call-1".into(),
                name: "some_tool".into(),
                arguments: "{}".into(),
            }]),
        });
        let tools = Arc::new(ToolRegistry::new());
        let agent = Agent::new(llm, tools);
        let task = AgentTask::new("do something");
        let opts = AgentOptions {
            max_steps: Some(100),
            max_tokens: Some(10),
            timeout: None,
            allow_tools: vec![],
            tenant_id: "tenant-1".to_string(),
        };
        let result = agent.run(task, opts).await.unwrap();
        // mock LLM 每次返回 total_tokens=15，第一次就超过 max_tokens=10
        assert_eq!(result.trace.terminated_by, TerminateReason::MaxTokens);
    }

    #[tokio::test]
    async fn agent_run_authorized_tool_executes_successfully() {
        use crate::agent::tool::Tool;
        struct EchoTool;
        #[async_trait]
        impl Tool for EchoTool {
            fn name(&self) -> &str {
                "echo"
            }
            fn schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object"})
            }
            async fn call(&self, args: &serde_json::Value) -> Result<serde_json::Value, AiError> {
                Ok(args.clone())
            }
        }

        let llm = Arc::new(MockLlmProvider {
            response: "using tool".into(),
            tool_calls: Some(vec![ToolCall {
                id: "call-1".into(),
                name: "echo".into(),
                arguments: r#"{"msg":"hello"}"#.into(),
            }]),
        });
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let tools = Arc::new(registry);
        let agent = Agent::new(llm, tools);
        let task = AgentTask::new("echo something");
        let opts = AgentOptions {
            max_steps: Some(1),
            max_tokens: None,
            timeout: None,
            allow_tools: vec!["echo".to_string()],
            tenant_id: "tenant-1".to_string(),
        };
        let result = agent.run(task, opts).await.unwrap();
        assert_eq!(result.trace.terminated_by, TerminateReason::MaxSteps);
        assert_eq!(result.trace.steps.len(), 1);
        assert!(result.trace.steps[0].tool_result.is_some());
        assert!(result.trace.steps[0]
            .observation
            .contains("executed successfully"));
    }

    #[tokio::test]
    async fn agent_run_authorized_tool_fails_gracefully() {
        use crate::agent::tool::Tool;
        struct FailTool;
        #[async_trait]
        impl Tool for FailTool {
            fn name(&self) -> &str {
                "fail"
            }
            fn schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object"})
            }
            async fn call(&self, _args: &serde_json::Value) -> Result<serde_json::Value, AiError> {
                Err(AiError::ToolExecution("always fails".into()))
            }
        }

        let llm = Arc::new(MockLlmProvider {
            response: "using failing tool".into(),
            tool_calls: Some(vec![ToolCall {
                id: "call-1".into(),
                name: "fail".into(),
                arguments: "{}".into(),
            }]),
        });
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(FailTool));
        let tools = Arc::new(registry);
        let agent = Agent::new(llm, tools);
        let task = AgentTask::new("use failing tool");
        let opts = AgentOptions {
            max_steps: Some(1),
            max_tokens: None,
            timeout: None,
            allow_tools: vec!["fail".to_string()],
            tenant_id: "tenant-1".to_string(),
        };
        let result = agent.run(task, opts).await.unwrap();
        assert_eq!(result.trace.terminated_by, TerminateReason::MaxSteps);
        assert_eq!(result.trace.steps.len(), 1);
        assert!(result.trace.steps[0].observation.contains("failed"));
    }
}
