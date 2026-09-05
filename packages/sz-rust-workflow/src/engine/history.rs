// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use crate::error::WorkflowResult;
use crate::instance::{ApprovalRecord, HistoryEntry, HistoryEntryType};
use crate::integration::SensitiveFieldRegistry;
use crate::repository::HistoryRepository;

/// 历史记录器，对齐 spec 5.5.1 规则 5。
pub struct HistoryRecorder {
    pub history_repo: Arc<dyn HistoryRepository>,
    sensitive_registry: Arc<SensitiveFieldRegistry>,
}

impl HistoryRecorder {
    pub fn new(
        history_repo: Arc<dyn HistoryRepository>,
        sensitive_registry: Arc<SensitiveFieldRegistry>,
    ) -> Self {
        Self {
            history_repo,
            sensitive_registry,
        }
    }

    pub async fn record_transition(
        &self,
        instance_id: &str,
        from_state: &str,
        to_state: &str,
        event: &str,
        context: &serde_json::Value,
    ) -> WorkflowResult<HistoryEntry> {
        let masked = self.sensitive_registry.mask(context);
        let entry = HistoryEntry {
            entry_id: Uuid::new_v4().to_string(),
            instance_id: instance_id.into(),
            entry_type: HistoryEntryType::Transition,
            from_node: None,
            to_node: None,
            from_state: Some(from_state.into()),
            to_state: Some(to_state.into()),
            context_snapshot: serde_json::json!({"event": event, "context": masked}),
            timestamp: Utc::now(),
        };
        self.history_repo.append(&entry).await?;
        Ok(entry)
    }

    pub async fn record_node_event(
        &self,
        instance_id: &str,
        from_node: Option<&str>,
        to_node: &str,
        entry_type: HistoryEntryType,
    ) -> WorkflowResult<HistoryEntry> {
        let entry = HistoryEntry::node_event(
            Uuid::new_v4().to_string(),
            instance_id,
            from_node.map(|s| s.to_string()),
            to_node,
            entry_type,
        );
        self.history_repo.append(&entry).await?;
        Ok(entry)
    }

    pub async fn record_task_handled(
        &self,
        record: ApprovalRecord,
    ) -> WorkflowResult<HistoryEntry> {
        let entry = HistoryEntry {
            entry_id: Uuid::new_v4().to_string(),
            instance_id: record.instance_id.clone(),
            entry_type: HistoryEntryType::TaskHandled,
            from_node: Some(record.node_id.clone()),
            to_node: None,
            from_state: None,
            to_state: None,
            context_snapshot: serde_json::to_value(&record).unwrap_or_default(),
            timestamp: record.timestamp,
        };
        self.history_repo.append(&entry).await?;
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::TaskAction;
    use crate::repository::InMemoryHistoryRepository;

    #[tokio::test]
    async fn record_transition_masks_sensitive() {
        let registry = Arc::new(SensitiveFieldRegistry::new());
        registry.register("phone");
        let repo = Arc::new(InMemoryHistoryRepository::default());
        let recorder = HistoryRecorder::new(repo.clone(), registry);

        let entry = recorder
            .record_transition(
                "i1",
                "draft",
                "review",
                "submit",
                &serde_json::json!({"phone": "138"}),
            )
            .await
            .unwrap();
        assert_eq!(entry.from_state.as_deref(), Some("draft"));
        assert_eq!(entry.context_snapshot["context"]["phone"], "***");

        let list = repo.list_by_instance("i1").await.unwrap();
        assert_eq!(list.len(), 1);
    }

    #[tokio::test]
    async fn record_node_event() {
        let registry = Arc::new(SensitiveFieldRegistry::new());
        let repo = Arc::new(InMemoryHistoryRepository::default());
        let recorder = HistoryRecorder::new(repo, registry);

        recorder
            .record_node_event("i1", Some("n0"), "n1", HistoryEntryType::NodeEnter)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn record_task_handled() {
        let registry = Arc::new(SensitiveFieldRegistry::new());
        let repo = Arc::new(InMemoryHistoryRepository::default());
        let recorder = HistoryRecorder::new(repo, registry);

        let record = ApprovalRecord {
            record_id: "r1".into(),
            instance_id: "i1".into(),
            task_id: "t1".into(),
            node_id: "n1".into(),
            actor: "u1".into(),
            action: TaskAction::Approve,
            comment: Some("ok".into()),
            target_user: None,
            timestamp: Utc::now(),
        };
        recorder.record_task_handled(record).await.unwrap();
    }
}
