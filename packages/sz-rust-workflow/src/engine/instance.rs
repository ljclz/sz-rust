use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};
use crate::instance::{FlowInstance, InstanceStatus, PageRequest, PageResult, Task};
use crate::integration::SensitiveFieldRegistry;
use crate::observability::{AuditAction, AuditLogger, WorkflowEvent, WorkflowEventBus};
use crate::repository::{DefinitionRepository, InstanceRepository};
use crate::scheduling::candidate_resolver::CandidateResolver;

use super::history::HistoryRecorder;
use super::task_manager::TaskManager;

/// 实例摘要。
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstanceSummary {
    pub instance_id: String,
    pub flow_key: String,
    pub status: InstanceStatus,
    pub current_nodes: Vec<String>,
}

/// 实例详情。
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstanceDetail {
    pub instance: FlowInstance,
    pub current_tasks: Vec<Task>,
}

/// 实例管理器。
pub struct InstanceManager {
    instance_repo: Arc<dyn InstanceRepository>,
    definition_repo: Arc<dyn DefinitionRepository>,
    task_manager: Arc<TaskManager>,
    candidate_resolver: Arc<dyn CandidateResolver>,
    history_recorder: Arc<HistoryRecorder>,
    audit: Arc<AuditLogger>,
    event_bus: Arc<dyn WorkflowEventBus>,
    #[allow(dead_code)] // 保留：实例历史脱敏预留
    sensitive_registry: Arc<SensitiveFieldRegistry>,
}

impl InstanceManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        instance_repo: Arc<dyn InstanceRepository>,
        definition_repo: Arc<dyn DefinitionRepository>,
        task_manager: Arc<TaskManager>,
        candidate_resolver: Arc<dyn CandidateResolver>,
        history_recorder: Arc<HistoryRecorder>,
        audit: Arc<AuditLogger>,
        event_bus: Arc<dyn WorkflowEventBus>,
        #[allow(dead_code)] // 保留：实例历史脱敏预留
        sensitive_registry: Arc<SensitiveFieldRegistry>,
    ) -> Self {
        Self {
            instance_repo,
            definition_repo,
            task_manager,
            candidate_resolver,
            history_recorder,
            audit,
            event_bus,
            sensitive_registry,
        }
    }

    /// 启动实例。
    pub async fn start(
        &self,
        flow_key: &str,
        context: serde_json::Value,
        initiator: &str,
    ) -> WorkflowResult<InstanceSummary> {
        let def = self
            .definition_repo
            .get_active(flow_key)
            .await?
            .ok_or_else(|| {
                WorkflowError::with_field(
                    WorkflowErrorCode::DefinitionNotFound,
                    "流程定义不存在或无生效版本",
                    "flow_key",
                    flow_key,
                )
            })?;

        let instance_id = Uuid::new_v4().to_string();
        let instance = FlowInstance::new(
            instance_id.clone(),
            flow_key,
            def.version.clone(),
            initiator,
            context,
            &def.start_node,
        );
        self.instance_repo.create(&instance).await?;

        self.history_recorder
            .record_node_event(
                &instance_id,
                None,
                &def.start_node,
                crate::instance::HistoryEntryType::NodeEnter,
            )
            .await
            .ok();

        if let Some(start_node) = def.find_node(&def.start_node) {
            if let crate::definition::NodeConfig::Start { next } = &start_node.config {
                if let Some(next_node) = def.find_node(next) {
                    if let crate::definition::NodeConfig::Approval {
                        candidate_strategy, ..
                    } = &next_node.config
                    {
                        let candidates = self
                            .candidate_resolver
                            .resolve(candidate_strategy, &instance.context)
                            .await?;
                        self.task_manager
                            .create_tasks(&instance_id, next, candidates)
                            .await
                            .ok();
                    }
                }
            }
        }

        self.event_bus
            .publish(WorkflowEvent::InstanceStarted {
                instance_id: instance_id.clone(),
                flow_key: flow_key.into(),
                initiator: initiator.into(),
                timestamp: Utc::now(),
            })
            .await
            .ok();

        self.audit.log_action(
            AuditAction::Start,
            initiator,
            &instance_id,
            InstanceStatus::Created,
            InstanceStatus::Running,
            serde_json::json!({"flow_key": flow_key}),
        );

        Ok(InstanceSummary {
            instance_id,
            flow_key: flow_key.into(),
            status: InstanceStatus::Running,
            current_nodes: instance.current_nodes.clone(),
        })
    }

    /// 挂起实例。
    pub async fn suspend(&self, instance_id: &str, actor: &str) -> WorkflowResult<()> {
        let instance = self.load_instance(instance_id).await?;
        if instance.status != InstanceStatus::Running {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::IllegalStatusTransition,
                format!("非 running 状态不可挂起：{}", instance.status),
                "status",
                &instance.status.to_string(),
            ));
        }
        let mut updated = instance.clone();
        updated.status = InstanceStatus::Suspended;
        updated.bump_version();
        self.save_with_lock(&updated, instance.version_lock).await?;

        self.event_bus
            .publish(WorkflowEvent::InstanceSuspended {
                instance_id: instance_id.into(),
                actor: actor.into(),
                timestamp: Utc::now(),
            })
            .await
            .ok();
        self.audit.log_action(
            AuditAction::Suspend,
            actor,
            instance_id,
            InstanceStatus::Running,
            InstanceStatus::Suspended,
            serde_json::json!({}),
        );
        Ok(())
    }

    /// 恢复实例。
    pub async fn resume(&self, instance_id: &str, actor: &str) -> WorkflowResult<()> {
        let instance = self.load_instance(instance_id).await?;
        if instance.status != InstanceStatus::Suspended {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::IllegalStatusTransition,
                format!("非 suspended 状态不可恢复：{}", instance.status),
                "status",
                &instance.status.to_string(),
            ));
        }
        let mut updated = instance.clone();
        updated.status = InstanceStatus::Running;
        updated.bump_version();
        self.save_with_lock(&updated, instance.version_lock).await?;

        self.event_bus
            .publish(WorkflowEvent::InstanceResumed {
                instance_id: instance_id.into(),
                actor: actor.into(),
                timestamp: Utc::now(),
            })
            .await
            .ok();
        self.audit.log_action(
            AuditAction::Resume,
            actor,
            instance_id,
            InstanceStatus::Suspended,
            InstanceStatus::Running,
            serde_json::json!({}),
        );
        Ok(())
    }

    /// 终止实例。
    pub async fn terminate(&self, instance_id: &str, actor: &str) -> WorkflowResult<()> {
        let instance = self.load_instance(instance_id).await?;
        let mut updated = instance.clone();
        updated.status = InstanceStatus::Terminated;
        updated.bump_version();
        self.save_with_lock(&updated, instance.version_lock).await?;
        self.task_manager
            .invalidate_by_instance(instance_id)
            .await?;

        self.event_bus
            .publish(WorkflowEvent::InstanceTerminated {
                instance_id: instance_id.into(),
                actor: actor.into(),
                timestamp: Utc::now(),
            })
            .await
            .ok();
        self.audit.log_action(
            AuditAction::Terminate,
            actor,
            instance_id,
            instance.status,
            InstanceStatus::Terminated,
            serde_json::json!({}),
        );
        Ok(())
    }

    /// 查询实例详情。
    pub async fn query(&self, instance_id: &str) -> WorkflowResult<InstanceDetail> {
        let instance = self.load_instance(instance_id).await?;
        let tasks = self.task_manager.list_by_instance(instance_id).await?;
        Ok(InstanceDetail {
            instance,
            current_tasks: tasks,
        })
    }

    /// 按状态分页查询。
    pub async fn list_by_status(
        &self,
        status: InstanceStatus,
        page: PageRequest,
    ) -> WorkflowResult<PageResult<FlowInstance>> {
        self.instance_repo.list_by_status(status, page).await
    }

    /// 查询历史轨迹（敏感字段脱敏）。
    pub async fn query_history(
        &self,
        instance_id: &str,
    ) -> WorkflowResult<Vec<crate::instance::HistoryEntry>> {
        let entries = self
            .history_recorder
            .history_repo
            .list_by_instance(instance_id)
            .await?;
        Ok(entries)
    }

    async fn load_instance(&self, instance_id: &str) -> WorkflowResult<FlowInstance> {
        self.instance_repo.get(instance_id).await?.ok_or_else(|| {
            WorkflowError::with_field(
                WorkflowErrorCode::InstanceNotFound,
                "实例不存在",
                "instance_id",
                instance_id,
            )
        })
    }

    async fn save_with_lock(&self, instance: &FlowInstance, expected: u64) -> WorkflowResult<()> {
        let success = self
            .instance_repo
            .update_with_version(instance, expected)
            .await?;
        if !success {
            return Err(WorkflowError::new(
                WorkflowErrorCode::OptimisticLockConflict,
                "乐观锁冲突",
            ));
        }
        Ok(())
    }
}
