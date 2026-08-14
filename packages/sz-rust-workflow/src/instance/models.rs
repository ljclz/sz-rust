use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 实例状态，对齐 spec 6.4.4。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStatus {
    Created,
    Running,
    Suspended,
    Terminated,
    Completed,
    Withdrawn,
    Error,
}

impl InstanceStatus {
    /// 是否可办理（接受事件与任务办理）。
    pub fn is_handleable(&self) -> bool {
        matches!(self, Self::Running)
    }
}

impl std::fmt::Display for InstanceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Suspended => "suspended",
            Self::Terminated => "terminated",
            Self::Completed => "completed",
            Self::Withdrawn => "withdrawn",
            Self::Error => "error",
        })
    }
}

/// 任务状态，对齐 spec 6.5。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Completed,
    Rejected,
    Transferred,
    Invalidated,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Rejected => "rejected",
            Self::Transferred => "transferred",
            Self::Invalidated => "invalidated",
        })
    }
}

/// 任务办理动作，对齐 spec 6.5。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Approve,
    Reject,
    Transfer,
    AddSign,
    Withdraw,
}

/// 流程实例，对齐 design 2.3.2 类图。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowInstance {
    pub instance_id: String,
    pub flow_key: String,
    pub version: semver::Version,
    pub status: InstanceStatus,
    /// 当前所在节点（并行网关可能多节点）
    pub current_nodes: Vec<String>,
    /// 流程上下文
    pub context: serde_json::Value,
    /// 乐观锁版本号
    pub version_lock: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub initiator: String,
}

impl FlowInstance {
    pub fn new(
        instance_id: impl Into<String>,
        flow_key: impl Into<String>,
        version: semver::Version,
        initiator: impl Into<String>,
        context: serde_json::Value,
        start_node: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            instance_id: instance_id.into(),
            flow_key: flow_key.into(),
            version,
            status: InstanceStatus::Running,
            current_nodes: vec![start_node.into()],
            context,
            version_lock: 0,
            created_at: now,
            updated_at: now,
            initiator: initiator.into(),
        }
    }

    /// 自增版本锁并更新时间戳。
    pub fn bump_version(&mut self) {
        self.version_lock += 1;
        self.updated_at = Utc::now();
    }
}

/// 任务，对齐 spec 6.5。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub task_id: String,
    pub instance_id: String,
    pub node_id: String,
    pub candidates: Vec<String>,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<TaskAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Task {
    pub fn new_pending(
        task_id: impl Into<String>,
        instance_id: impl Into<String>,
        node_id: impl Into<String>,
        candidates: Vec<String>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            instance_id: instance_id.into(),
            node_id: node_id.into(),
            candidates,
            status: TaskStatus::Pending,
            assignee: None,
            action: None,
            handled_at: None,
            created_at: Utc::now(),
        }
    }
}

/// 审批记录，对齐 spec 6.6。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub record_id: String,
    pub instance_id: String,
    pub task_id: String,
    pub node_id: String,
    pub actor: String,
    pub action: TaskAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_user: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// 历史条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryEntryType {
    Transition,
    NodeEnter,
    NodeLeave,
    TaskHandled,
    InstanceLifecycle,
}

/// 历史条目。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub entry_id: String,
    pub instance_id: String,
    pub entry_type: HistoryEntryType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_state: Option<String>,
    pub context_snapshot: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

impl HistoryEntry {
    pub fn transition(
        entry_id: impl Into<String>,
        instance_id: impl Into<String>,
        from_state: impl Into<String>,
        to_state: impl Into<String>,
        context_snapshot: serde_json::Value,
    ) -> Self {
        Self {
            entry_id: entry_id.into(),
            instance_id: instance_id.into(),
            entry_type: HistoryEntryType::Transition,
            from_node: None,
            to_node: None,
            from_state: Some(from_state.into()),
            to_state: Some(to_state.into()),
            context_snapshot,
            timestamp: Utc::now(),
        }
    }

    pub fn node_event(
        entry_id: impl Into<String>,
        instance_id: impl Into<String>,
        from_node: Option<String>,
        to_node: impl Into<String>,
        entry_type: HistoryEntryType,
    ) -> Self {
        Self {
            entry_id: entry_id.into(),
            instance_id: instance_id.into(),
            entry_type,
            from_node,
            to_node: Some(to_node.into()),
            from_state: None,
            to_state: None,
            context_snapshot: serde_json::Value::Null,
            timestamp: Utc::now(),
        }
    }
}

/// 分页请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRequest {
    pub page: u32,
    pub page_size: u32,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 20,
        }
    }
}

impl PageRequest {
    pub fn offset(&self) -> usize {
        ((self.page.saturating_sub(1)) as usize) * (self.page_size as usize)
    }
    pub fn limit(&self) -> usize {
        self.page_size as usize
    }
}

/// 分页结果。
#[derive(Debug, Clone, Serialize)]
pub struct PageResult<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

impl<T> PageResult<T> {
    pub fn new(items: Vec<T>, total: u64, req: &PageRequest) -> Self {
        Self {
            items,
            total,
            page: req.page,
            page_size: req.page_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_status_is_handleable() {
        assert!(InstanceStatus::Running.is_handleable());
        assert!(!InstanceStatus::Suspended.is_handleable());
        assert!(!InstanceStatus::Created.is_handleable());
        assert!(!InstanceStatus::Terminated.is_handleable());
    }

    #[test]
    fn flow_instance_new() {
        let inst = FlowInstance::new(
            "i1",
            "leave",
            semver::Version::new(1, 0, 0),
            "user1",
            serde_json::json!({}),
            "start",
        );
        assert_eq!(inst.instance_id, "i1");
        assert_eq!(inst.status, InstanceStatus::Running);
        assert_eq!(inst.current_nodes, vec!["start"]);
        assert_eq!(inst.version_lock, 0);
    }

    #[test]
    fn flow_instance_bump_version() {
        let mut inst = FlowInstance::new(
            "i1",
            "leave",
            semver::Version::new(1, 0, 0),
            "u1",
            serde_json::json!({}),
            "s",
        );
        let old = inst.updated_at;
        std::thread::sleep(std::time::Duration::from_millis(1));
        inst.bump_version();
        assert_eq!(inst.version_lock, 1);
        assert!(inst.updated_at > old);
    }

    #[test]
    fn task_new_pending() {
        let t = Task::new_pending("t1", "i1", "n1", vec!["u1".into(), "u2".into()]);
        assert_eq!(t.status, TaskStatus::Pending);
        assert_eq!(t.candidates, vec!["u1", "u2"]);
        assert!(t.assignee.is_none());
    }

    #[test]
    fn page_request_offset() {
        let r = PageRequest {
            page: 3,
            page_size: 10,
        };
        assert_eq!(r.offset(), 20);
        assert_eq!(r.limit(), 10);

        let r2 = PageRequest::default();
        assert_eq!(r2.offset(), 0);
        assert_eq!(r2.limit(), 20);
    }

    #[test]
    fn history_entry_transition() {
        let e = HistoryEntry::transition("e1", "i1", "draft", "review", serde_json::json!({}));
        assert_eq!(e.entry_type, HistoryEntryType::Transition);
        assert_eq!(e.from_state.as_deref(), Some("draft"));
        assert_eq!(e.to_state.as_deref(), Some("review"));
    }

    #[test]
    fn history_entry_node_event() {
        let e = HistoryEntry::node_event(
            "e1",
            "i1",
            Some("n0".into()),
            "n1",
            HistoryEntryType::NodeEnter,
        );
        assert_eq!(e.entry_type, HistoryEntryType::NodeEnter);
        assert_eq!(e.from_node.as_deref(), Some("n0"));
        assert_eq!(e.to_node.as_deref(), Some("n1"));
    }

    #[test]
    fn enums_serde() {
        assert_eq!(
            serde_json::to_string(&InstanceStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&TaskAction::Approve).unwrap(),
            "\"approve\""
        );
    }
}
