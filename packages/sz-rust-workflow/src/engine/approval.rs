use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use crate::definition::{FlowDefinition, NodeConfig, NodeType};
use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};
use crate::instance::{ApprovalRecord, FlowInstance, InstanceStatus, Task, TaskAction, TaskStatus};
use crate::observability::{AuditLogger, WorkflowEvent, WorkflowEventBus};
use crate::repository::{InstanceRepository, TaskRepository};
use crate::scheduling::approval_strategy::{select_strategy, NodeCompletion};
use crate::scheduling::candidate_resolver::CandidateResolver;

use super::task_manager::TaskManager;

/// 任务办理结果。
#[derive(Debug, Clone)]
pub struct TaskHandleResult {
    pub task_id: String,
    pub action: TaskAction,
    pub node_completion: NodeCompletion,
    pub advanced: bool,
}

/// 审批流引擎，对齐 design 2.2.2.4。
pub struct ApprovalFlowEngine {
    task_repo: Arc<dyn TaskRepository>,
    instance_repo: Arc<dyn InstanceRepository>,
    candidate_resolver: Arc<dyn CandidateResolver>,
    task_manager: Arc<TaskManager>,
    audit: Arc<AuditLogger>,
    event_bus: Arc<dyn WorkflowEventBus>,
}

impl ApprovalFlowEngine {
    pub fn new(
        task_repo: Arc<dyn TaskRepository>,
        instance_repo: Arc<dyn InstanceRepository>,
        candidate_resolver: Arc<dyn CandidateResolver>,
        task_manager: Arc<TaskManager>,
        audit: Arc<AuditLogger>,
        event_bus: Arc<dyn WorkflowEventBus>,
    ) -> Self {
        Self {
            task_repo,
            instance_repo,
            candidate_resolver,
            task_manager,
            audit,
            event_bus,
        }
    }

    /// 办理任务。
    pub async fn handle(
        &self,
        task_id: &str,
        action: TaskAction,
        comment: Option<String>,
        actor: &str,
        definition: &FlowDefinition,
    ) -> WorkflowResult<TaskHandleResult> {
        let task = self.task_repo.get(task_id).await?.ok_or_else(|| {
            WorkflowError::with_field(
                WorkflowErrorCode::InstanceNotFound,
                "任务不存在",
                "task_id",
                task_id,
            )
        })?;

        if task.status != TaskStatus::Pending {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::TaskNotHandleable,
                format!("任务状态 {} 不可办理", task.status),
                "task_id",
                task_id,
            ));
        }

        let instance = self
            .instance_repo
            .get(&task.instance_id)
            .await?
            .ok_or_else(|| {
                WorkflowError::with_field(
                    WorkflowErrorCode::InstanceNotFound,
                    "实例不存在",
                    "instance_id",
                    &task.instance_id,
                )
            })?;

        if !instance.status.is_handleable() {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::InstanceNotHandleable,
                format!("实例状态 {} 不可办理", instance.status),
                "instance_id",
                &instance.instance_id,
            ));
        }

        if !task.candidates.contains(&actor.to_string()) {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::UnauthorizedHandle,
                "越权办理：actor 不属于候选人集合",
                "actor",
                actor,
            ));
        }

        let mut updated_task = task.clone();
        updated_task.status = match action {
            TaskAction::Approve => TaskStatus::Completed,
            TaskAction::Reject => TaskStatus::Rejected,
            TaskAction::Transfer => TaskStatus::Transferred,
            _ => TaskStatus::Completed,
        };
        updated_task.assignee = Some(actor.into());
        updated_task.action = Some(action);
        updated_task.handled_at = Some(Utc::now());
        self.task_repo.update(&updated_task).await?;

        let record = ApprovalRecord {
            record_id: Uuid::new_v4().to_string(),
            instance_id: task.instance_id.clone(),
            task_id: task.task_id.clone(),
            node_id: task.node_id.clone(),
            actor: actor.into(),
            action,
            comment,
            target_user: None,
            timestamp: Utc::now(),
        };

        self.event_bus
            .publish(WorkflowEvent::TaskHandled {
                instance_id: task.instance_id.clone(),
                task_id: task.task_id.clone(),
                actor: actor.into(),
                action: format!("{:?}", action),
                timestamp: Utc::now(),
            })
            .await
            .ok();

        self.audit.log_action(
            crate::observability::AuditAction::Handle,
            actor,
            &task.instance_id,
            instance.status,
            instance.status,
            serde_json::to_value(&record).unwrap_or_default(),
        );

        let node = definition.find_node(&task.node_id).ok_or_else(|| {
            WorkflowError::with_field(
                WorkflowErrorCode::InstanceNotFound,
                "节点不存在",
                "node_id",
                &task.node_id,
            )
        })?;

        let (strategy_type, next) = match &node.config {
            NodeConfig::Approval {
                approval_strategy,
                next,
                ..
            } => (*approval_strategy, next.clone()),
            _ => {
                return Ok(TaskHandleResult {
                    task_id: task_id.into(),
                    action,
                    node_completion: NodeCompletion::Completed,
                    advanced: false,
                })
            }
        };

        let all_tasks = self
            .task_manager
            .list_by_instance(&task.instance_id)
            .await?;
        let node_tasks: Vec<Task> = all_tasks
            .iter()
            .filter(|t| t.node_id == task.node_id)
            .cloned()
            .collect();
        let strategy = select_strategy(strategy_type);
        let completion = strategy.check_completion(&node_tasks, action);

        let advanced = if completion == NodeCompletion::Completed {
            self.advance_node(&instance, &task.node_id, &next, definition)
                .await?;
            true
        } else if completion == NodeCompletion::Rejected {
            self.advance_node(&instance, &task.node_id, &definition.start_node, definition)
                .await?;
            true
        } else {
            false
        };

        Ok(TaskHandleResult {
            task_id: task_id.into(),
            action,
            node_completion: completion,
            advanced,
        })
    }

    async fn advance_node(
        &self,
        instance: &FlowInstance,
        current_node: &str,
        next_node: &str,
        definition: &FlowDefinition,
    ) -> WorkflowResult<()> {
        self.event_bus
            .publish(WorkflowEvent::NodeLeft {
                instance_id: instance.instance_id.clone(),
                node_id: current_node.into(),
                timestamp: Utc::now(),
            })
            .await
            .ok();

        if let Some(next) = definition.find_node(next_node) {
            if next.node_type == NodeType::End {
                let mut updated = instance.clone();
                updated.status = InstanceStatus::Completed;
                updated.current_nodes = vec![next_node.into()];
                updated.bump_version();
                let _ = self
                    .instance_repo
                    .update_with_version(&updated, instance.version_lock)
                    .await?;
                self.event_bus
                    .publish(WorkflowEvent::InstanceCompleted {
                        instance_id: instance.instance_id.clone(),
                        timestamp: Utc::now(),
                    })
                    .await
                    .ok();
            } else if let NodeConfig::Approval {
                candidate_strategy, ..
            } = &next.config
            {
                let candidates = self
                    .candidate_resolver
                    .resolve(candidate_strategy, &instance.context)
                    .await?;
                let tasks = self
                    .task_manager
                    .create_tasks(&instance.instance_id, next_node, candidates.clone())
                    .await?;
                for t in &tasks {
                    self.event_bus
                        .publish(WorkflowEvent::TaskCreated {
                            instance_id: instance.instance_id.clone(),
                            task_id: t.task_id.clone(),
                            node_id: next_node.into(),
                            candidates: candidates.clone(),
                            timestamp: Utc::now(),
                        })
                        .await
                        .ok();
                }
                let mut updated = instance.clone();
                updated.current_nodes = vec![next_node.into()];
                updated.bump_version();
                let _ = self
                    .instance_repo
                    .update_with_version(&updated, instance.version_lock)
                    .await?;
            }
            self.event_bus
                .publish(WorkflowEvent::NodeEntered {
                    instance_id: instance.instance_id.clone(),
                    node_id: next_node.into(),
                    timestamp: Utc::now(),
                })
                .await
                .ok();
        }
        Ok(())
    }
}
