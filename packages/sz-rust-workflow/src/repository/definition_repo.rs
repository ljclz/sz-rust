// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use async_trait::async_trait;

use crate::definition::FlowDefinition;
use crate::error::WorkflowResult;

/// 定义 ID 类型别名。
pub type DefinitionId = String;

/// 流程定义持久化 Repository，对齐 design 2.2.2.9。
#[async_trait]
pub trait DefinitionRepository: Send + Sync + 'static {
    /// 保存定义（返回分配的 ID）。
    async fn save(&self, def: &FlowDefinition) -> WorkflowResult<DefinitionId>;

    /// 按 ID 获取定义。
    async fn get(&self, id: &DefinitionId) -> WorkflowResult<Option<FlowDefinition>>;

    /// 获取 flow_key 的当前生效版本。
    async fn get_active(&self, flow_key: &str) -> WorkflowResult<Option<FlowDefinition>>;

    /// 列出 flow_key 的所有版本。
    async fn list_versions(&self, flow_key: &str) -> WorkflowResult<Vec<FlowDefinition>>;

    /// 设置生效版本（同 flow_key 仅一个 active）。
    async fn set_active(&self, id: &DefinitionId) -> WorkflowResult<()>;

    /// 弃用版本。
    async fn deprecate(&self, id: &DefinitionId) -> WorkflowResult<()>;
}
