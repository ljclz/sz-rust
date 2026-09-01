#![forbid(unsafe_code)]
#![allow(missing_docs)]
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

// ============================================================================
// Addon 接线：WorkflowState + register_routes
// ============================================================================

use axum::response::Json;
use serde_json::json;
use sz_rust_core::router::RouterBuilder;

/// workflow addon 状态
#[derive(Clone)]
pub struct WorkflowState {
    pub version: &'static str,
}

impl Default for WorkflowState {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

fn create_engine() -> WorkflowEngine {
    let config = WorkflowConfig::default();
    let deps = WorkflowDeps::default_for_test();
    WorkflowEngine::new(config, deps)
}

/// 注册 workflow addon 路由到 sz300 RouterBuilder
pub fn register_routes<S>(builder: RouterBuilder<S>, state: WorkflowState) -> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    let builder = builder.get("/api/workflow/health", {
        let v = state.version;
        move || async move {
            let _engine = create_engine();
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "plugin": "workflow",
                    "status": "active",
                    "engine": "WorkflowEngine",
                    "version": v
                }
            }))
        }
    });

    let builder = builder.get("/api/workflow/definitions", {
        move || async move {
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "definitions": [],
                    "total": 0
                }
            }))
        }
    });

    let builder = builder.get("/api/workflow/instances", {
        move || async move {
            let engine = create_engine();
            let page = PageRequest::default();
            let pending_tasks = engine
                .query_tasks("", page)
                .await
                .map(|r| r.total)
                .unwrap_or(0);
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "instances": [],
                    "total": 0,
                    "pending_tasks": pending_tasks
                }
            }))
        }
    });

    builder
}

pub mod capability;
pub use capability::WorkflowPlugin;
