// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::llm::provider::ToolCall;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentTrace {
    pub steps: Vec<AgentStep>,
    pub total_tokens: u32,
    pub total_duration_ms: u64,
    pub terminated_by: TerminateReason,
}

impl AgentTrace {
    pub fn new() -> Self {
        Self {
            steps: Vec::new(),
            total_tokens: 0,
            total_duration_ms: 0,
            terminated_by: TerminateReason::Natural,
        }
    }
}

impl Default for AgentTrace {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentStep {
    pub thought: String,
    pub tool_call: Option<ToolCall>,
    pub tool_result: Option<serde_json::Value>,
    pub observation: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminateReason {
    Natural,
    MaxSteps,
    MaxTokens,
    Timeout,
    Error,
}
