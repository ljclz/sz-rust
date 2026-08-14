use std::sync::Arc;

use chrono::Utc;

use crate::definition::StateMachineDefinition;
use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};
use crate::guard::GuardEvaluator;
use crate::instance::InstanceStatus;
use crate::observability::{AuditLogger, WorkflowEvent, WorkflowEventBus};
use crate::repository::InstanceRepository;

/// 迁移结果。
#[derive(Debug, Clone)]
pub struct TransitionResult {
    pub from_state: String,
    pub to_state: String,
    pub event: String,
    pub migrated: bool,
}

/// 状态机引擎。
pub struct StateMachineEngine {
    instance_repo: Arc<dyn InstanceRepository>,
    guard_evaluator: Arc<dyn GuardEvaluator>,
    event_bus: Arc<dyn WorkflowEventBus>,
    audit: Arc<AuditLogger>,
}

impl StateMachineEngine {
    pub fn new(
        instance_repo: Arc<dyn InstanceRepository>,
        guard_evaluator: Arc<dyn GuardEvaluator>,
        event_bus: Arc<dyn WorkflowEventBus>,
        audit: Arc<AuditLogger>,
    ) -> Self {
        Self {
            instance_repo,
            guard_evaluator,
            event_bus,
            audit,
        }
    }

    /// 触发事件，执行状态迁移。
    pub async fn fire(
        &self,
        instance_id: &str,
        event: &str,
        machine: &StateMachineDefinition,
        payload: serde_json::Value,
    ) -> WorkflowResult<TransitionResult> {
        let instance = self.instance_repo.get(instance_id).await?.ok_or_else(|| {
            WorkflowError::with_field(
                WorkflowErrorCode::InstanceNotFound,
                "实例不存在",
                "instance_id",
                instance_id,
            )
        })?;

        if !instance.status.is_handleable() {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::InstanceNotHandleable,
                format!("实例状态 {} 不可办理", instance.status),
                "status",
                &instance.status.to_string(),
            ));
        }

        let current_state = instance
            .context
            .get("current_state")
            .and_then(|v| v.as_str())
            .unwrap_or(&machine.initial_state)
            .to_string();

        let transition = machine
            .transitions
            .iter()
            .find(|t| t.from == current_state && t.event == event)
            .ok_or_else(|| {
                WorkflowError::with_field(
                    WorkflowErrorCode::NoMatchingTransition,
                    format!("状态 {} 不接受事件 {}", current_state, event),
                    "state",
                    &current_state,
                )
            })?;

        if let Some(ref guard) = transition.guard {
            let mut ctx = instance.context.clone();
            if let serde_json::Value::Object(ref mut obj) = ctx {
                if let serde_json::Value::Object(ref payload_obj) = payload {
                    for (k, v) in payload_obj {
                        obj.insert(k.clone(), v.clone());
                    }
                }
                obj.insert("payload".into(), payload.clone());
            }
            let guard_result = self.guard_evaluator.evaluate(guard, &ctx).await;
            match guard_result {
                Ok(true) => {}
                Ok(false) => {
                    return Ok(TransitionResult {
                        from_state: current_state.clone(),
                        to_state: current_state,
                        event: event.into(),
                        migrated: false,
                    });
                }
                Err(e) => return Err(e),
            }
        }

        let expected_version = instance.version_lock;
        let mut updated = instance.clone();
        if let serde_json::Value::Object(ref mut obj) = updated.context {
            obj.insert(
                "current_state".into(),
                serde_json::Value::String(transition.to.clone()),
            );
            obj.insert("payload".into(), payload);
        }
        updated.bump_version();

        let success = self
            .instance_repo
            .update_with_version(&updated, expected_version)
            .await?;
        if !success {
            return Err(WorkflowError::new(
                WorkflowErrorCode::OptimisticLockConflict,
                "乐观锁冲突：实例版本号不匹配",
            ));
        }

        self.event_bus
            .publish(WorkflowEvent::TransitionFired {
                instance_id: instance_id.into(),
                from: current_state.clone(),
                to: transition.to.clone(),
                event: event.into(),
                timestamp: Utc::now(),
            })
            .await
            .ok();

        self.audit.log_transition(
            "system",
            instance_id,
            "",
            event,
            InstanceStatus::Running,
            InstanceStatus::Running,
        );

        Ok(TransitionResult {
            from_state: current_state,
            to_state: transition.to.clone(),
            event: event.into(),
            migrated: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guard::DefaultGuardEvaluator;
    use crate::instance::FlowInstance;
    use crate::integration::SensitiveFieldRegistry;
    use crate::observability::{AuditLogger, NoopEventBus};
    use crate::repository::InMemoryInstanceRepository;

    fn machine() -> StateMachineDefinition {
        StateMachineDefinition {
            initial_state: "draft".into(),
            states: vec!["draft".into(), "review".into(), "done".into()],
            transitions: vec![
                crate::definition::Transition {
                    from: "draft".into(),
                    to: "review".into(),
                    event: "submit".into(),
                    guard: None,
                },
                crate::definition::Transition {
                    from: "review".into(),
                    to: "done".into(),
                    event: "approve".into(),
                    guard: Some("$.amount > 100".into()),
                },
            ],
        }
    }

    fn setup() -> (StateMachineEngine, Arc<InMemoryInstanceRepository>) {
        let repo = Arc::new(InMemoryInstanceRepository::default());
        let engine = StateMachineEngine::new(
            repo.clone(),
            Arc::new(DefaultGuardEvaluator::default()),
            Arc::new(NoopEventBus),
            Arc::new(AuditLogger::new(Arc::new(SensitiveFieldRegistry::new()))),
        );
        (engine, repo)
    }

    #[tokio::test]
    async fn fire_success() {
        let (engine, repo) = setup();
        let inst = FlowInstance::new(
            "i1",
            "test",
            semver::Version::new(1, 0, 0),
            "u1",
            serde_json::json!({"current_state": "draft"}),
            "start",
        );
        repo.create(&inst).await.unwrap();

        let result = engine
            .fire("i1", "submit", &machine(), serde_json::json!({}))
            .await
            .unwrap();
        assert!(result.migrated);
        assert_eq!(result.from_state, "draft");
        assert_eq!(result.to_state, "review");
    }

    #[tokio::test]
    async fn fire_no_matching_transition() {
        let (engine, repo) = setup();
        let inst = FlowInstance::new(
            "i1",
            "test",
            semver::Version::new(1, 0, 0),
            "u1",
            serde_json::json!({"current_state": "draft"}),
            "start",
        );
        repo.create(&inst).await.unwrap();

        let result = engine
            .fire("i1", "approve", &machine(), serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code,
            WorkflowErrorCode::NoMatchingTransition
        );
    }

    #[tokio::test]
    async fn fire_instance_not_found() {
        let (engine, _) = setup();
        let result = engine
            .fire("nonexistent", "submit", &machine(), serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code,
            WorkflowErrorCode::InstanceNotFound
        );
    }

    #[tokio::test]
    async fn fire_guard_false_drops_event() {
        let (engine, repo) = setup();
        let inst = FlowInstance::new(
            "i1",
            "test",
            semver::Version::new(1, 0, 0),
            "u1",
            serde_json::json!({"current_state": "review"}),
            "start",
        );
        repo.create(&inst).await.unwrap();

        let result = engine
            .fire(
                "i1",
                "approve",
                &machine(),
                serde_json::json!({"amount": 50}),
            )
            .await
            .unwrap();
        assert!(!result.migrated);
    }

    #[tokio::test]
    async fn fire_guard_true_migrates() {
        let (engine, repo) = setup();
        let inst = FlowInstance::new(
            "i1",
            "test",
            semver::Version::new(1, 0, 0),
            "u1",
            serde_json::json!({"current_state": "review"}),
            "start",
        );
        repo.create(&inst).await.unwrap();

        let result = engine
            .fire(
                "i1",
                "approve",
                &machine(),
                serde_json::json!({"amount": 200}),
            )
            .await
            .unwrap();
        assert!(result.migrated);
        assert_eq!(result.to_state, "done");
    }
}
