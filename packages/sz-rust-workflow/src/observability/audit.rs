use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::instance::InstanceStatus;
use crate::integration::SensitiveFieldRegistry;

/// 审计操作类型，对齐 spec 4.3.4 六类操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Start,
    Transition,
    Handle,
    Terminate,
    Suspend,
    Resume,
}

/// 结构化审计日志器，对齐 spec 4.3.4。
pub struct AuditLogger {
    sensitive_registry: Arc<SensitiveFieldRegistry>,
}

impl AuditLogger {
    pub fn new(sensitive_registry: Arc<SensitiveFieldRegistry>) -> Self {
        Self { sensitive_registry }
    }

    /// 记录审计动作，输出结构化 `tracing` 日志。
    pub fn log_action(
        &self,
        action: AuditAction,
        actor: &str,
        instance_id: &str,
        pre_status: InstanceStatus,
        post_status: InstanceStatus,
        details: serde_json::Value,
    ) {
        let masked_details = self.sensitive_registry.mask(&details);
        let span = tracing::info_span!(
            "workflow_audit",
            instance_id = %instance_id,
            action = ?action,
        );
        let _enter = span.enter();
        tracing::info!(
            action = ?action,
            actor = %actor,
            instance_id = %instance_id,
            pre_status = %pre_status,
            post_status = %post_status,
            timestamp = %Utc::now().to_rfc3339(),
            details = %masked_details,
            "审计日志",
        );
    }

    /// 记录迁移审计（含 node_id 与 transition 信息）。
    pub fn log_transition(
        &self,
        actor: &str,
        instance_id: &str,
        node_id: &str,
        transition: &str,
        pre_status: InstanceStatus,
        post_status: InstanceStatus,
    ) {
        let span = tracing::info_span!(
            "workflow_transition",
            instance_id = %instance_id,
            node_id = %node_id,
            transition = %transition,
        );
        let _enter = span.enter();
        tracing::info!(
            action = ?AuditAction::Transition,
            actor = %actor,
            instance_id = %instance_id,
            node_id = %node_id,
            transition = %transition,
            pre_status = %pre_status,
            post_status = %post_status,
            timestamp = %Utc::now().to_rfc3339(),
            "迁移审计",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_log_does_not_panic() {
        let registry = Arc::new(SensitiveFieldRegistry::new());
        registry.register("phone");
        let logger = AuditLogger::new(registry);
        logger.log_action(
            AuditAction::Start,
            "user1",
            "i1",
            InstanceStatus::Created,
            InstanceStatus::Running,
            serde_json::json!({"phone": "13800138000", "name": "test"}),
        );
        logger.log_transition(
            "user1",
            "i1",
            "n1",
            "submit",
            InstanceStatus::Running,
            InstanceStatus::Running,
        );
    }

    #[test]
    fn audit_action_serde() {
        assert_eq!(
            serde_json::to_string(&AuditAction::Start).unwrap(),
            "\"start\""
        );
        assert_eq!(
            serde_json::to_string(&AuditAction::Transition).unwrap(),
            "\"transition\""
        );
    }
}
