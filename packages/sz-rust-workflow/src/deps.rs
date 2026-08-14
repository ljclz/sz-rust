use std::sync::Arc;

use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};
use crate::guard::{DefaultGuardEvaluator, GuardEvaluator};
use crate::integration::SensitiveFieldRegistry;
use crate::observability::{AuditLogger, NoopEventBus, WorkflowEventBus, WorkflowMetrics};
use crate::repository::{
    DefinitionRepository, HistoryRepository, InMemoryDefinitionRepository,
    InMemoryHistoryRepository, InMemoryInstanceRepository, InMemoryTaskRepository,
    InstanceRepository, TaskRepository,
};
use crate::scheduling::candidate_resolver::{CandidateResolver, DefaultCandidateResolver};
use sz_rust_capability::CapabilityRegistry;

/// 工作流依赖注入容器。
pub struct WorkflowDeps {
    pub capability_registry: Arc<CapabilityRegistry>,
    pub definition_repo: Arc<dyn DefinitionRepository>,
    pub instance_repo: Arc<dyn InstanceRepository>,
    pub task_repo: Arc<dyn TaskRepository>,
    pub history_repo: Arc<dyn HistoryRepository>,
    pub event_bus: Arc<dyn WorkflowEventBus>,
    pub metrics: Arc<WorkflowMetrics>,
    pub audit: Arc<AuditLogger>,
    pub sensitive_registry: Arc<SensitiveFieldRegistry>,
    pub guard_evaluator: Arc<dyn GuardEvaluator>,
    pub candidate_resolver: Arc<dyn CandidateResolver>,
}

impl WorkflowDeps {
    /// 测试用默认依赖（InMemory Repository + NoopEventBus）。
    pub fn default_for_test() -> Self {
        let capability_registry = Arc::new(CapabilityRegistry::new());
        let sensitive_registry = Arc::new(SensitiveFieldRegistry::new());
        let guard_evaluator: Arc<dyn GuardEvaluator> = Arc::new(DefaultGuardEvaluator::default());
        let candidate_resolver: Arc<dyn CandidateResolver> = Arc::new(
            DefaultCandidateResolver::new(guard_evaluator.clone(), capability_registry.clone()),
        );
        Self {
            capability_registry,
            definition_repo: Arc::new(InMemoryDefinitionRepository::default()),
            instance_repo: Arc::new(InMemoryInstanceRepository::default()),
            task_repo: Arc::new(InMemoryTaskRepository::default()),
            history_repo: Arc::new(InMemoryHistoryRepository::default()),
            event_bus: Arc::new(NoopEventBus),
            metrics: Arc::new(WorkflowMetrics::new()),
            audit: Arc::new(AuditLogger::new(sensitive_registry.clone())),
            sensitive_registry,
            guard_evaluator,
            candidate_resolver,
        }
    }
}

/// Builder 模式装配依赖。
pub struct WorkflowDepsBuilder {
    capability_registry: Option<Arc<CapabilityRegistry>>,
    definition_repo: Option<Arc<dyn DefinitionRepository>>,
    instance_repo: Option<Arc<dyn InstanceRepository>>,
    task_repo: Option<Arc<dyn TaskRepository>>,
    history_repo: Option<Arc<dyn HistoryRepository>>,
    event_bus: Option<Arc<dyn WorkflowEventBus>>,
    sensitive_registry: Option<Arc<SensitiveFieldRegistry>>,
    guard_evaluator: Option<Arc<dyn GuardEvaluator>>,
}

impl Default for WorkflowDepsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkflowDepsBuilder {
    pub fn new() -> Self {
        Self {
            capability_registry: None,
            definition_repo: None,
            instance_repo: None,
            task_repo: None,
            history_repo: None,
            event_bus: None,
            sensitive_registry: None,
            guard_evaluator: None,
        }
    }

    pub fn capability_registry(mut self, v: Arc<CapabilityRegistry>) -> Self {
        self.capability_registry = Some(v);
        self
    }
    pub fn definition_repo(mut self, v: Arc<dyn DefinitionRepository>) -> Self {
        self.definition_repo = Some(v);
        self
    }
    pub fn instance_repo(mut self, v: Arc<dyn InstanceRepository>) -> Self {
        self.instance_repo = Some(v);
        self
    }
    pub fn task_repo(mut self, v: Arc<dyn TaskRepository>) -> Self {
        self.task_repo = Some(v);
        self
    }
    pub fn history_repo(mut self, v: Arc<dyn HistoryRepository>) -> Self {
        self.history_repo = Some(v);
        self
    }
    pub fn event_bus(mut self, v: Arc<dyn WorkflowEventBus>) -> Self {
        self.event_bus = Some(v);
        self
    }
    pub fn sensitive_registry(mut self, v: Arc<SensitiveFieldRegistry>) -> Self {
        self.sensitive_registry = Some(v);
        self
    }
    pub fn guard_evaluator(mut self, v: Arc<dyn GuardEvaluator>) -> Self {
        self.guard_evaluator = Some(v);
        self
    }

    pub fn build(self) -> WorkflowResult<WorkflowDeps> {
        let capability_registry = self.capability_registry.ok_or_else(|| {
            WorkflowError::new(
                WorkflowErrorCode::StructureIncomplete,
                "缺少 capability_registry",
            )
        })?;
        let sensitive_registry = self
            .sensitive_registry
            .unwrap_or_else(|| Arc::new(SensitiveFieldRegistry::new()));
        let guard_evaluator = self
            .guard_evaluator
            .unwrap_or_else(|| Arc::new(DefaultGuardEvaluator::default()));
        let candidate_resolver: Arc<dyn CandidateResolver> = Arc::new(
            DefaultCandidateResolver::new(guard_evaluator.clone(), capability_registry.clone()),
        );
        Ok(WorkflowDeps {
            capability_registry,
            definition_repo: self.definition_repo.ok_or_else(|| {
                WorkflowError::new(
                    WorkflowErrorCode::StructureIncomplete,
                    "缺少 definition_repo",
                )
            })?,
            instance_repo: self.instance_repo.ok_or_else(|| {
                WorkflowError::new(WorkflowErrorCode::StructureIncomplete, "缺少 instance_repo")
            })?,
            task_repo: self.task_repo.ok_or_else(|| {
                WorkflowError::new(WorkflowErrorCode::StructureIncomplete, "缺少 task_repo")
            })?,
            history_repo: self.history_repo.ok_or_else(|| {
                WorkflowError::new(WorkflowErrorCode::StructureIncomplete, "缺少 history_repo")
            })?,
            event_bus: self.event_bus.unwrap_or_else(|| Arc::new(NoopEventBus)),
            metrics: Arc::new(WorkflowMetrics::new()),
            audit: Arc::new(AuditLogger::new(sensitive_registry.clone())),
            sensitive_registry,
            guard_evaluator,
            candidate_resolver,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_for_test() {
        let deps = WorkflowDeps::default_for_test();
        assert_eq!(deps.capability_registry.len(), 0);
    }

    #[test]
    fn builder_missing_required() {
        let result = WorkflowDepsBuilder::new().build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_complete() {
        let deps = WorkflowDepsBuilder::new()
            .capability_registry(Arc::new(CapabilityRegistry::new()))
            .definition_repo(Arc::new(InMemoryDefinitionRepository::default()))
            .instance_repo(Arc::new(InMemoryInstanceRepository::default()))
            .task_repo(Arc::new(InMemoryTaskRepository::default()))
            .history_repo(Arc::new(InMemoryHistoryRepository::default()))
            .build()
            .unwrap();
        assert_eq!(deps.capability_registry.len(), 0);
    }
}
