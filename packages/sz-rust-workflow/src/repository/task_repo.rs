// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use async_trait::async_trait;

use crate::error::WorkflowResult;
use crate::instance::{PageRequest, PageResult, Task};

/// 任务持久化 Repository。
#[async_trait]
pub trait TaskRepository: Send + Sync + 'static {
    /// 创建任务。
    async fn create(&self, task: &Task) -> WorkflowResult<()>;

    /// 获取任务。
    async fn get(&self, task_id: &str) -> WorkflowResult<Option<Task>>;

    /// 更新任务。
    async fn update(&self, task: &Task) -> WorkflowResult<()>;

    /// 失效实例所有未完成任务，返回失效数。
    async fn invalidate_by_instance(&self, instance_id: &str) -> WorkflowResult<u64>;

    /// 按候选人分页查询待办。
    async fn list_pending_by_candidate(
        &self,
        candidate: &str,
        page: PageRequest,
    ) -> WorkflowResult<PageResult<Task>>;

    /// 列出实例所有任务。
    async fn list_by_instance(&self, instance_id: &str) -> WorkflowResult<Vec<Task>>;
}
