use crate::definition::ApprovalStrategyType;
use crate::instance::{Task, TaskAction, TaskStatus};

/// 节点完成状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeCompletion {
    Completed,
    Rejected,
    InProgress,
}

/// 审批策略 trait，对齐 design 2.2.2.7。
pub trait ApprovalStrategy: Send + Sync + 'static {
    fn check_completion(&self, tasks: &[Task], action: TaskAction) -> NodeCompletion;
}

/// 会签策略：所有候选人均 approve 才 Completed，任一 reject 则 Rejected。
pub struct AndSignStrategy;

impl ApprovalStrategy for AndSignStrategy {
    fn check_completion(&self, tasks: &[Task], _action: TaskAction) -> NodeCompletion {
        let relevant: Vec<&Task> = tasks
            .iter()
            .filter(|t| {
                t.status == TaskStatus::Completed
                    || t.status == TaskStatus::Rejected
                    || t.status == TaskStatus::Pending
            })
            .collect();
        if relevant.is_empty() {
            return NodeCompletion::InProgress;
        }
        if relevant.iter().any(|t| t.status == TaskStatus::Rejected) {
            return NodeCompletion::Rejected;
        }
        if relevant.iter().all(|t| t.status == TaskStatus::Completed) {
            return NodeCompletion::Completed;
        }
        NodeCompletion::InProgress
    }
}

/// 或签策略：任一候选人 approve 即 Completed，任一 reject 是否 Rejected 由配置决定。
pub struct OrSignStrategy;

impl ApprovalStrategy for OrSignStrategy {
    fn check_completion(&self, tasks: &[Task], _action: TaskAction) -> NodeCompletion {
        if tasks.iter().any(|t| t.status == TaskStatus::Completed) {
            return NodeCompletion::Completed;
        }
        if tasks.iter().all(|t| t.status == TaskStatus::Rejected) && !tasks.is_empty() {
            return NodeCompletion::Rejected;
        }
        NodeCompletion::InProgress
    }
}

/// 按类型选择策略。
pub fn select_strategy(strategy_type: ApprovalStrategyType) -> Box<dyn ApprovalStrategy> {
    match strategy_type {
        ApprovalStrategyType::AndSign => Box::new(AndSignStrategy),
        ApprovalStrategyType::OrSign => Box::new(OrSignStrategy),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_task(id: &str, status: TaskStatus) -> Task {
        Task {
            task_id: id.into(),
            instance_id: "i1".into(),
            node_id: "n1".into(),
            candidates: vec!["u1".into()],
            status,
            assignee: None,
            action: None,
            handled_at: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn and_sign_in_progress() {
        let tasks = vec![
            make_task("t1", TaskStatus::Completed),
            make_task("t2", TaskStatus::Pending),
            make_task("t3", TaskStatus::Pending),
        ];
        let s = AndSignStrategy;
        assert_eq!(
            s.check_completion(&tasks, TaskAction::Approve),
            NodeCompletion::InProgress
        );
    }

    #[test]
    fn and_sign_completed() {
        let tasks = vec![
            make_task("t1", TaskStatus::Completed),
            make_task("t2", TaskStatus::Completed),
            make_task("t3", TaskStatus::Completed),
        ];
        let s = AndSignStrategy;
        assert_eq!(
            s.check_completion(&tasks, TaskAction::Approve),
            NodeCompletion::Completed
        );
    }

    #[test]
    fn and_sign_rejected() {
        let tasks = vec![
            make_task("t1", TaskStatus::Completed),
            make_task("t2", TaskStatus::Rejected),
        ];
        let s = AndSignStrategy;
        assert_eq!(
            s.check_completion(&tasks, TaskAction::Reject),
            NodeCompletion::Rejected
        );
    }

    #[test]
    fn or_sign_completed() {
        let tasks = vec![
            make_task("t1", TaskStatus::Completed),
            make_task("t2", TaskStatus::Pending),
        ];
        let s = OrSignStrategy;
        assert_eq!(
            s.check_completion(&tasks, TaskAction::Approve),
            NodeCompletion::Completed
        );
    }

    #[test]
    fn or_sign_rejected() {
        let tasks = vec![
            make_task("t1", TaskStatus::Rejected),
            make_task("t2", TaskStatus::Rejected),
        ];
        let s = OrSignStrategy;
        assert_eq!(
            s.check_completion(&tasks, TaskAction::Reject),
            NodeCompletion::Rejected
        );
    }

    #[test]
    fn or_sign_in_progress() {
        let tasks = vec![
            make_task("t1", TaskStatus::Pending),
            make_task("t2", TaskStatus::Pending),
        ];
        let s = OrSignStrategy;
        assert_eq!(
            s.check_completion(&tasks, TaskAction::Approve),
            NodeCompletion::InProgress
        );
    }
}
