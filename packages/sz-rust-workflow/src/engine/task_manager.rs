use std::sync::Arc;

use uuid::Uuid;

use crate::error::WorkflowResult;
use crate::instance::{PageRequest, PageResult, Task};
use crate::repository::TaskRepository;

/// 任务生命周期管理器。
pub struct TaskManager {
    task_repo: Arc<dyn TaskRepository>,
}

impl TaskManager {
    pub fn new(task_repo: Arc<dyn TaskRepository>) -> Self {
        Self { task_repo }
    }

    /// 为每个候选人生成待办任务。
    pub async fn create_tasks(
        &self,
        instance_id: &str,
        node_id: &str,
        candidates: Vec<String>,
    ) -> WorkflowResult<Vec<Task>> {
        let mut tasks = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let task = Task::new_pending(
                Uuid::new_v4().to_string(),
                instance_id,
                node_id,
                vec![candidate],
            );
            self.task_repo.create(&task).await?;
            tasks.push(task);
        }
        Ok(tasks)
    }

    /// 失效实例所有未完成任务。
    pub async fn invalidate_by_instance(&self, instance_id: &str) -> WorkflowResult<u64> {
        self.task_repo.invalidate_by_instance(instance_id).await
    }

    /// 分页查询候选人待办。
    pub async fn list_pending_by_candidate(
        &self,
        candidate: &str,
        page: PageRequest,
    ) -> WorkflowResult<PageResult<Task>> {
        self.task_repo
            .list_pending_by_candidate(candidate, page)
            .await
    }

    /// 获取任务。
    pub async fn get(&self, task_id: &str) -> WorkflowResult<Option<Task>> {
        self.task_repo.get(task_id).await
    }

    /// 更新任务。
    pub async fn update(&self, task: &Task) -> WorkflowResult<()> {
        self.task_repo.update(task).await
    }

    /// 列出实例所有任务。
    pub async fn list_by_instance(&self, instance_id: &str) -> WorkflowResult<Vec<Task>> {
        self.task_repo.list_by_instance(instance_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::TaskStatus;
    use crate::repository::InMemoryTaskRepository;

    #[tokio::test]
    async fn create_tasks() {
        let repo = Arc::new(InMemoryTaskRepository::default());
        let mgr = TaskManager::new(repo);
        let tasks = mgr
            .create_tasks("i1", "n1", vec!["u1".into(), "u2".into()])
            .await
            .unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].status, TaskStatus::Pending);
        assert_ne!(tasks[0].task_id, tasks[1].task_id);
    }

    #[tokio::test]
    async fn invalidate() {
        let repo = Arc::new(InMemoryTaskRepository::default());
        let mgr = TaskManager::new(repo);
        mgr.create_tasks("i1", "n1", vec!["u1".into(), "u2".into()])
            .await
            .unwrap();
        let count = mgr.invalidate_by_instance("i1").await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn list_pending() {
        let repo = Arc::new(InMemoryTaskRepository::default());
        let mgr = TaskManager::new(repo);
        mgr.create_tasks("i1", "n1", vec!["u1".into(), "u2".into()])
            .await
            .unwrap();
        let result = mgr
            .list_pending_by_candidate("u1", PageRequest::default())
            .await
            .unwrap();
        assert_eq!(result.total, 1);
    }
}
