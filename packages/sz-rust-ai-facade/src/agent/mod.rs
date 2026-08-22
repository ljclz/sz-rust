//! Agent 编排引擎：工具选择循环 + 短期/长期记忆 + 多步推理

pub mod engine;
pub mod memory;
pub mod termination;
pub mod tool;
pub mod trace;

pub use engine::{Agent, AgentOptions, AgentResult, AgentTask};
pub use memory::{LongTermMemory, ShortTermMemory};
pub use termination::TerminationPolicy;
pub use tool::{Tool, ToolRegistry};
pub use trace::{AgentStep, AgentTrace, TerminateReason};
