// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Agent 编排引擎：工具选择循环 + 短期/长期记忆 + 多步推理

pub mod delegate;
pub mod engine;
pub mod hitl;
pub mod memory;
pub mod multi_step;
pub mod persistence;
pub mod recovery;
pub mod termination;
pub mod tool;
pub mod trace;

pub use delegate::{create_delegate_step, AgentDelegator, FnSubAgent, SubAgent};
pub use engine::{Agent, AgentOptions, AgentResult, AgentTask};
pub use hitl::{
    AutoApproveCallback, AutoRejectCallback, HitlController, ReviewCallback, ReviewRequest,
    ReviewStatus,
};
pub use memory::{LongTermMemory, ShortTermMemory};
pub use multi_step::{
    FnStep, MultiStepOrchestrator, OrchestrationResult, OrchestrationStep, StepAction, StepContext,
    StepExecutionResult,
};
pub use persistence::{
    AgentExecution, ExecutionState, ExecutionStore, ExecutionTracker, InMemoryExecutionStore,
    StepTrace,
};
pub use recovery::{RecoveryConfig, RecoveryDecision, RecoveryHandler, RecoveryStrategy};
pub use termination::TerminationPolicy;
pub use tool::{Tool, ToolRegistry};
pub use trace::{AgentStep, AgentTrace, TerminateReason};
