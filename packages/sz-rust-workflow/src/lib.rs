#![forbid(unsafe_code)]
#![doc = "SZ-Rust 工作流引擎 — 状态机/审批流/插件节点编排"]
//!
//! ## 核心组件
//!
//! - [`config::WorkflowConfig`] — 引擎配置
//! - [`error::WorkflowError`] / [`error::WorkflowErrorCode`] — 错误类型（26 错误码）
//! - [`definition`] — 流程定义模型与解析校验
//! - [`engine`] — 状态机/审批流/实例/插件节点引擎
//! - [`guard`] — 守卫条件求值
//! - [`scheduling`] — 审批策略/候选人解析/任务动作/容错策略
//! - [`instance`] — 实例/任务/历史领域模型
//! - [`repository`] — 持久化 Repository trait + InMemory 实现
//! - [`observability`] — 事件总线/指标/审计
//! - [`integration`] — 插件卸载联动/敏感字段脱敏
//! - [`api`] — 设计器 HTTP API

pub mod api;
pub mod config;
pub mod definition;
pub mod deps;
pub mod engine;
pub mod error;
pub mod guard;
pub mod instance;
pub mod integration;
pub mod observability;
pub mod repository;
pub mod scheduling;

pub use config::WorkflowConfig;
pub use definition::{
    ApprovalStrategyType, CandidateStrategy, DefinitionFormat, DefinitionParser,
    DefinitionValidator, FaultStrategy, FlowDefinition, IssueSeverity, NodeConfig, NodeType,
    StateMachineDefinition, Transition, ValidationIssue,
};
pub use deps::{WorkflowDeps, WorkflowDepsBuilder};
pub use engine::WorkflowEngine;
pub use error::{WorkflowError, WorkflowErrorCode, WorkflowResult};
pub use instance::{
    FlowInstance, HistoryEntry, InstanceStatus, PageRequest, PageResult, Task, TaskAction,
    TaskStatus,
};
