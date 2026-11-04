// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::sync::Arc;

use crate::api::{DesignerApi, VersionManager};
use crate::config::WorkflowConfig;
use crate::definition::{DefinitionFormat, DefinitionValidator, ValidationIssue};
use crate::deps::WorkflowDeps;
use crate::engine::approval::{ApprovalFlowEngine, TaskHandleResult};
use crate::engine::history::HistoryRecorder;
use crate::engine::instance::{InstanceDetail, InstanceManager, InstanceSummary};
use crate::engine::state_machine::{StateMachineEngine, TransitionResult};
use crate::engine::task_manager::TaskManager;
use crate::error::WorkflowResult;
use crate::instance::{HistoryEntry, PageRequest, PageResult, Task, TaskAction};
use crate::repository::DefinitionId;

/// 工作流引擎统一门面，对齐 design 2.2.2.1。
pub struct WorkflowEngine {
    #[allow(dead_code)]
    config: WorkflowConfig,
    designer_api: Arc<DesignerApi>,
    instance_manager: Arc<InstanceManager>,
    state_machine: Arc<StateMachineEngine>,
    approval_flow: Arc<ApprovalFlowEngine>,
    task_manager: Arc<TaskManager>,
    #[allow(dead_code)] // 构造时移交给 instance_manager，字段仅持有引用
    history_recorder: Arc<HistoryRecorder>,
}

impl WorkflowEngine {
    pub fn new(config: WorkflowConfig, deps: WorkflowDeps) -> Self {
        let task_manager = Arc::new(TaskManager::new(deps.task_repo.clone()));
        let history_recorder = Arc::new(HistoryRecorder::new(
            deps.history_repo.clone(),
            deps.sensitive_registry.clone(),
        ));
        let version_manager = Arc::new(VersionManager::new(deps.definition_repo.clone()));
        let designer_api = Arc::new(DesignerApi::new(
            DefinitionValidator::new(Arc::new(crate::definition::NoopPluginChecker)),
            deps.definition_repo.clone(),
            version_manager,
        ));
        let instance_manager = Arc::new(InstanceManager::new(
            deps.instance_repo.clone(),
            deps.definition_repo.clone(),
            task_manager.clone(),
            deps.candidate_resolver.clone(),
            history_recorder.clone(),
            deps.audit.clone(),
            deps.event_bus.clone(),
            deps.sensitive_registry.clone(),
        ));
        let state_machine = Arc::new(StateMachineEngine::new(
            deps.instance_repo.clone(),
            deps.guard_evaluator.clone(),
            deps.event_bus.clone(),
            deps.audit.clone(),
        ));
        let approval_flow = Arc::new(ApprovalFlowEngine::new(
            deps.task_repo.clone(),
            deps.instance_repo.clone(),
            deps.candidate_resolver.clone(),
            task_manager.clone(),
            deps.audit.clone(),
            deps.event_bus.clone(),
        ));

        Self {
            config,
            designer_api,
            instance_manager,
            state_machine,
            approval_flow,
            task_manager,
            history_recorder,
        }
    }

    pub async fn validate_definition(
        &self,
        text: &str,
        format: DefinitionFormat,
    ) -> WorkflowResult<Vec<ValidationIssue>> {
        self.designer_api.validate_definition(text, format).await
    }

    pub async fn import_definition(
        &self,
        text: &str,
        format: DefinitionFormat,
    ) -> WorkflowResult<DefinitionId> {
        self.designer_api.import_definition(text, format).await
    }

    pub async fn export_definition(
        &self,
        id: &DefinitionId,
        format: DefinitionFormat,
    ) -> WorkflowResult<String> {
        self.designer_api.export_definition(id, format).await
    }

    pub async fn start_instance(
        &self,
        flow_key: &str,
        context: serde_json::Value,
        initiator: &str,
    ) -> WorkflowResult<InstanceSummary> {
        self.instance_manager
            .start(flow_key, context, initiator)
            .await
    }

    pub async fn suspend_instance(&self, instance_id: &str, actor: &str) -> WorkflowResult<()> {
        self.instance_manager.suspend(instance_id, actor).await
    }

    pub async fn resume_instance(&self, instance_id: &str, actor: &str) -> WorkflowResult<()> {
        self.instance_manager.resume(instance_id, actor).await
    }

    pub async fn terminate_instance(&self, instance_id: &str, actor: &str) -> WorkflowResult<()> {
        self.instance_manager.terminate(instance_id, actor).await
    }

    pub async fn query_instance(&self, instance_id: &str) -> WorkflowResult<InstanceDetail> {
        self.instance_manager.query(instance_id).await
    }

    pub async fn query_history(&self, instance_id: &str) -> WorkflowResult<Vec<HistoryEntry>> {
        self.instance_manager.query_history(instance_id).await
    }

    pub async fn fire_event(
        &self,
        instance_id: &str,
        event: &str,
        machine: &crate::definition::StateMachineDefinition,
        payload: serde_json::Value,
    ) -> WorkflowResult<TransitionResult> {
        self.state_machine
            .fire(instance_id, event, machine, payload)
            .await
    }

    pub async fn handle_task(
        &self,
        task_id: &str,
        action: TaskAction,
        comment: Option<String>,
        actor: &str,
        definition: &crate::definition::FlowDefinition,
    ) -> WorkflowResult<TaskHandleResult> {
        self.approval_flow
            .handle(task_id, action, comment, actor, definition)
            .await
    }

    pub async fn query_tasks(
        &self,
        candidate: &str,
        page: PageRequest,
    ) -> WorkflowResult<PageResult<Task>> {
        self.task_manager
            .list_pending_by_candidate(candidate, page)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::InstanceStatus;

    #[tokio::test]
    async fn engine_init() {
        let config = WorkflowConfig::default();
        let deps = WorkflowDeps::default_for_test();
        let engine = WorkflowEngine::new(config, deps);

        // 验证引擎可导入并校验一个合法流程定义（构造后基础能力可用）
        let yaml = r#"
flow_key: init_test
version: "1.0.0"
name: 初始化测试
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
active: true
"#;
        let result = engine.import_definition(yaml, DefinitionFormat::Yaml).await;
        assert!(
            result.is_ok(),
            "engine_init 后应能导入合法流程定义，实际: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn full_workflow() {
        let config = WorkflowConfig::default();
        let deps = WorkflowDeps::default_for_test();
        let engine = WorkflowEngine::new(config, deps);

        let yaml = r#"
flow_key: simple_flow
version: "1.0.0"
name: 简单流程
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
active: true
"#;
        let id = engine
            .import_definition(yaml, DefinitionFormat::Yaml)
            .await
            .unwrap();

        let summary = engine
            .start_instance("simple_flow", serde_json::json!({}), "user1")
            .await
            .unwrap();
        assert_eq!(summary.status, InstanceStatus::Running);

        let detail = engine.query_instance(&summary.instance_id).await.unwrap();
        assert_eq!(detail.instance.instance_id, summary.instance_id);

        let exported = engine
            .export_definition(&id, DefinitionFormat::Json)
            .await
            .unwrap();
        assert!(exported.contains("simple_flow"));
    }
}
