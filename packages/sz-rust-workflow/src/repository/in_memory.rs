use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::definition::FlowDefinition;
use crate::error::WorkflowResult;
use crate::instance::{
    FlowInstance, HistoryEntry, InstanceStatus, PageRequest, PageResult, Task, TaskStatus,
};

use super::definition_repo::{DefinitionId, DefinitionRepository};
use super::history_repo::HistoryRepository;
use super::instance_repo::InstanceRepository;
use super::task_repo::TaskRepository;

/// InMemory 定义 Repository。
#[derive(Default)]
pub struct InMemoryDefinitionRepository {
    defs: RwLock<HashMap<String, FlowDefinition>>,
    next_id: RwLock<u64>,
}

impl InMemoryDefinitionRepository {
    fn gen_id(&self) -> String {
        let mut id = self.next_id.write();
        *id += 1;
        format!("def_{}", *id)
    }
}

#[async_trait]
impl DefinitionRepository for InMemoryDefinitionRepository {
    async fn save(&self, def: &FlowDefinition) -> WorkflowResult<DefinitionId> {
        let id = self.gen_id();
        self.defs.write().insert(id.clone(), def.clone());
        Ok(id)
    }

    async fn get(&self, id: &DefinitionId) -> WorkflowResult<Option<FlowDefinition>> {
        Ok(self.defs.read().get(id).cloned())
    }

    async fn get_active(&self, flow_key: &str) -> WorkflowResult<Option<FlowDefinition>> {
        Ok(self
            .defs
            .read()
            .values()
            .filter(|d| d.flow_key == flow_key && d.active && !d.deprecated)
            .max_by_key(|d| d.version.clone())
            .cloned())
    }

    async fn list_versions(&self, flow_key: &str) -> WorkflowResult<Vec<FlowDefinition>> {
        let mut versions: Vec<_> = self
            .defs
            .read()
            .values()
            .filter(|d| d.flow_key == flow_key)
            .cloned()
            .collect();
        versions.sort_by(|a, b| b.version.cmp(&a.version));
        Ok(versions)
    }

    async fn set_active(&self, id: &DefinitionId) -> WorkflowResult<()> {
        let mut defs = self.defs.write();
        let target = defs
            .get(id)
            .ok_or_else(|| {
                crate::error::WorkflowError::new(
                    crate::error::WorkflowErrorCode::VersionNotFound,
                    format!("定义不存在：{id}"),
                )
            })?
            .clone();
        for d in defs.values_mut() {
            if d.flow_key == target.flow_key {
                d.active = false;
            }
        }
        defs.get_mut(id).unwrap().active = true;
        Ok(())
    }

    async fn deprecate(&self, id: &DefinitionId) -> WorkflowResult<()> {
        let mut defs = self.defs.write();
        if let Some(d) = defs.get_mut(id) {
            d.deprecated = true;
            d.active = false;
        }
        Ok(())
    }
}

/// InMemory 实例 Repository。
#[derive(Default)]
pub struct InMemoryInstanceRepository {
    instances: RwLock<HashMap<String, FlowInstance>>,
}

#[async_trait]
impl InstanceRepository for InMemoryInstanceRepository {
    async fn create(&self, instance: &FlowInstance) -> WorkflowResult<()> {
        self.instances
            .write()
            .insert(instance.instance_id.clone(), instance.clone());
        Ok(())
    }

    async fn get(&self, instance_id: &str) -> WorkflowResult<Option<FlowInstance>> {
        Ok(self.instances.read().get(instance_id).cloned())
    }

    async fn update_with_version(
        &self,
        instance: &FlowInstance,
        expected_version: u64,
    ) -> WorkflowResult<bool> {
        let mut map = self.instances.write();
        let existing = match map.get(&instance.instance_id) {
            Some(e) => e,
            None => return Ok(false),
        };
        if existing.version_lock != expected_version {
            return Ok(false);
        }
        let mut updated = instance.clone();
        updated.version_lock = expected_version + 1;
        updated.updated_at = chrono::Utc::now();
        map.insert(instance.instance_id.clone(), updated);
        Ok(true)
    }

    async fn list_by_status(
        &self,
        status: InstanceStatus,
        page: PageRequest,
    ) -> WorkflowResult<PageResult<FlowInstance>> {
        let all: Vec<_> = self
            .instances
            .read()
            .values()
            .filter(|i| i.status == status)
            .cloned()
            .collect();
        let total = all.len() as u64;
        let items: Vec<_> = all
            .into_iter()
            .skip(page.offset())
            .take(page.limit())
            .collect();
        Ok(PageResult::new(items, total, &page))
    }

    async fn list_running(&self) -> WorkflowResult<Vec<FlowInstance>> {
        Ok(self
            .instances
            .read()
            .values()
            .filter(|i| i.status == InstanceStatus::Running)
            .cloned()
            .collect())
    }

    async fn list_running_batch(&self, batch_size: usize) -> WorkflowResult<Vec<FlowInstance>> {
        Ok(self
            .instances
            .read()
            .values()
            .filter(|i| i.status == InstanceStatus::Running)
            .take(batch_size)
            .cloned()
            .collect())
    }
}

/// InMemory 任务 Repository。
#[derive(Default)]
pub struct InMemoryTaskRepository {
    tasks: RwLock<HashMap<String, Task>>,
}

#[async_trait]
impl TaskRepository for InMemoryTaskRepository {
    async fn create(&self, task: &Task) -> WorkflowResult<()> {
        self.tasks
            .write()
            .insert(task.task_id.clone(), task.clone());
        Ok(())
    }

    async fn get(&self, task_id: &str) -> WorkflowResult<Option<Task>> {
        Ok(self.tasks.read().get(task_id).cloned())
    }

    async fn update(&self, task: &Task) -> WorkflowResult<()> {
        self.tasks
            .write()
            .insert(task.task_id.clone(), task.clone());
        Ok(())
    }

    async fn invalidate_by_instance(&self, instance_id: &str) -> WorkflowResult<u64> {
        let mut map = self.tasks.write();
        let mut count = 0u64;
        for t in map.values_mut() {
            if t.instance_id == instance_id && t.status == TaskStatus::Pending {
                t.status = TaskStatus::Invalidated;
                count += 1;
            }
        }
        Ok(count)
    }

    async fn list_pending_by_candidate(
        &self,
        candidate: &str,
        page: PageRequest,
    ) -> WorkflowResult<PageResult<Task>> {
        let all: Vec<_> = self
            .tasks
            .read()
            .values()
            .filter(|t| {
                t.status == TaskStatus::Pending && t.candidates.contains(&candidate.to_string())
            })
            .cloned()
            .collect();
        let total = all.len() as u64;
        let items: Vec<_> = all
            .into_iter()
            .skip(page.offset())
            .take(page.limit())
            .collect();
        Ok(PageResult::new(items, total, &page))
    }

    async fn list_by_instance(&self, instance_id: &str) -> WorkflowResult<Vec<Task>> {
        Ok(self
            .tasks
            .read()
            .values()
            .filter(|t| t.instance_id == instance_id)
            .cloned()
            .collect())
    }
}

/// InMemory 历史 Repository。
#[derive(Default)]
pub struct InMemoryHistoryRepository {
    entries: RwLock<Vec<HistoryEntry>>,
}

#[async_trait]
impl HistoryRepository for InMemoryHistoryRepository {
    async fn append(&self, entry: &HistoryEntry) -> WorkflowResult<()> {
        self.entries.write().push(entry.clone());
        Ok(())
    }

    async fn list_by_instance(&self, instance_id: &str) -> WorkflowResult<Vec<HistoryEntry>> {
        Ok(self
            .entries
            .read()
            .iter()
            .filter(|e| e.instance_id == instance_id)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn definition_repo_save_and_get() {
        let repo = InMemoryDefinitionRepository::default();
        let def = FlowDefinition {
            flow_key: "test".into(),
            version: semver::Version::new(1, 0, 0),
            name: "test".into(),
            nodes: vec![],
            start_node: "start".into(),
            active: true,
            deprecated: false,
            machine: None,
            flow: None,
        };
        let id = repo.save(&def).await.unwrap();
        let got = repo.get(&id).await.unwrap().unwrap();
        assert_eq!(got.flow_key, "test");
    }

    #[tokio::test]
    async fn definition_repo_set_active() {
        let repo = InMemoryDefinitionRepository::default();
        let def1 = FlowDefinition {
            flow_key: "test".into(),
            version: semver::Version::new(1, 0, 0),
            name: "v1".into(),
            nodes: vec![],
            start_node: "start".into(),
            active: true,
            deprecated: false,
            machine: None,
            flow: None,
        };
        let mut def2 = def1.clone();
        def2.version = semver::Version::new(2, 0, 0);
        def2.active = false;

        let id1 = repo.save(&def1).await.unwrap();
        let id2 = repo.save(&def2).await.unwrap();
        repo.set_active(&id2).await.unwrap();
        let got1 = repo.get(&id1).await.unwrap().unwrap();
        let got2 = repo.get(&id2).await.unwrap().unwrap();
        assert!(!got1.active);
        assert!(got2.active);
    }

    #[tokio::test]
    async fn instance_repo_optimistic_lock() {
        let repo = InMemoryInstanceRepository::default();
        let inst = FlowInstance::new(
            "i1",
            "test",
            semver::Version::new(1, 0, 0),
            "u1",
            serde_json::json!({}),
            "start",
        );
        repo.create(&inst).await.unwrap();

        let mut updated = inst.clone();
        updated.status = InstanceStatus::Suspended;
        let ok = repo.update_with_version(&updated, 0).await.unwrap();
        assert!(ok);

        let got = repo.get("i1").await.unwrap().unwrap();
        assert_eq!(got.status, InstanceStatus::Suspended);
        assert_eq!(got.version_lock, 1);

        let conflict = repo.update_with_version(&updated, 0).await.unwrap();
        assert!(!conflict);
    }

    #[tokio::test]
    async fn task_repo_invalidate() {
        let repo = InMemoryTaskRepository::default();
        let t1 = Task::new_pending("t1", "i1", "n1", vec!["u1".into()]);
        let t2 = Task::new_pending("t2", "i1", "n2", vec!["u2".into()]);
        let t3 = Task::new_pending("t3", "i2", "n3", vec!["u3".into()]);
        repo.create(&t1).await.unwrap();
        repo.create(&t2).await.unwrap();
        repo.create(&t3).await.unwrap();

        let count = repo.invalidate_by_instance("i1").await.unwrap();
        assert_eq!(count, 2);
        let got = repo.get("t1").await.unwrap().unwrap();
        assert_eq!(got.status, TaskStatus::Invalidated);
    }

    #[tokio::test]
    async fn history_repo_append_and_list() {
        let repo = InMemoryHistoryRepository::default();
        let e1 = HistoryEntry::transition("e1", "i1", "draft", "review", serde_json::json!({}));
        let e2 = HistoryEntry::transition("e2", "i1", "review", "done", serde_json::json!({}));
        let e3 = HistoryEntry::transition("e3", "i2", "a", "b", serde_json::json!({}));
        repo.append(&e1).await.unwrap();
        repo.append(&e2).await.unwrap();
        repo.append(&e3).await.unwrap();

        let list = repo.list_by_instance("i1").await.unwrap();
        assert_eq!(list.len(), 2);
    }
}
