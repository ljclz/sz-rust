// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use async_trait::async_trait;

use crate::error::WorkflowResult;
use crate::instance::{FlowInstance, InstanceStatus, PageRequest, PageResult};

/// 流程实例持久化 Repository，对齐 design 2.2.2.9。
///
/// `update_with_version` 实现乐观锁语义。
#[async_trait]
pub trait InstanceRepository: Send + Sync + 'static {
    /// 创建实例。
    async fn create(&self, instance: &FlowInstance) -> WorkflowResult<()>;

    /// 获取实例。
    async fn get(&self, instance_id: &str) -> WorkflowResult<Option<FlowInstance>>;

    /// 乐观锁更新：仅当 `expected_version` 匹配时更新，成功返回 `true` 并自增 `version_lock`。
    async fn update_with_version(
        &self,
        instance: &FlowInstance,
        expected_version: u64,
    ) -> WorkflowResult<bool>;

    /// 按状态分页查询。
    async fn list_by_status(
        &self,
        status: InstanceStatus,
        page: PageRequest,
    ) -> WorkflowResult<PageResult<FlowInstance>>;

    /// 列出所有 running 实例。
    async fn list_running(&self) -> WorkflowResult<Vec<FlowInstance>>;

    /// 批量列出 running 实例（故障恢复用）。
    async fn list_running_batch(&self, batch_size: usize) -> WorkflowResult<Vec<FlowInstance>>;
}
