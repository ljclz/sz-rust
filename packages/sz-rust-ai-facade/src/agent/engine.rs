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
