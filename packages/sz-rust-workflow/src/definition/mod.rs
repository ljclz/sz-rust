// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 流程定义模型与解析校验
//!
//! ## 模块
//!
//! - [`models`] — 核心领域模型（FlowDefinition/StateMachineDefinition/ApprovalFlowDefinition）
//! - [`node`] — 节点类型与配置
//! - [`strategy`] — 候选人/审批/容错策略与迁移
//! - [`parser`] — YAML/JSON 解析器
//! - [`validator`] — 结构/可达性/终止性/插件引用校验

pub mod models;
pub mod node;
pub mod parser;
pub mod strategy;
pub mod validator;

pub use models::{
    ApprovalFlowDefinition, DefinitionFormat, FlowDefinition, StateMachineDefinition,
};
pub use node::{ConditionBranch, Node, NodeConfig, NodeEdge, NodeType};
pub use parser::DefinitionParser;
pub use strategy::{ApprovalStrategyType, CandidateStrategy, FaultStrategy, Transition};
pub use validator::{
    DefinitionValidator, IssueSeverity, NoopPluginChecker, PluginChecker, ValidationIssue,
};
