// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use std::sync::Arc;

use serde::Serialize;

use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};
use crate::repository::{DefinitionId, DefinitionRepository};

/// 定义摘要。
#[derive(Debug, Clone, Serialize)]
pub struct DefinitionSummary {
    pub id: DefinitionId,
    pub flow_key: String,
    pub version: String,
    pub name: String,
    pub active: bool,
    pub deprecated: bool,
}

/// 版本管理器，对齐 spec 5.6。
pub struct VersionManager {
    definition_repo: Arc<dyn DefinitionRepository>,
}

impl VersionManager {
    pub fn new(definition_repo: Arc<dyn DefinitionRepository>) -> Self {
        Self { definition_repo }
    }

    /// 列出某流程标识所有版本。
    pub async fn list_versions(&self, flow_key: &str) -> WorkflowResult<Vec<DefinitionSummary>> {
        let defs = self.definition_repo.list_versions(flow_key).await?;
        Ok(defs
            .into_iter()
            .map(|d| DefinitionSummary {
                id: String::new(),
                flow_key: d.flow_key,
                version: d.version.to_string(),
                name: d.name,
                active: d.active,
                deprecated: d.deprecated,
            })
            .collect())
    }

    /// 设置生效版本。
    pub async fn set_active(&self, id: &DefinitionId) -> WorkflowResult<()> {
        let def = self.definition_repo.get(id).await?.ok_or_else(|| {
            WorkflowError::with_field(WorkflowErrorCode::VersionNotFound, "版本不存在", "id", id)
        })?;
        if def.deprecated {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::VersionNotFound,
                "已弃用版本不可设为生效",
                "id",
                id,
            ));
        }
        self.definition_repo.set_active(id).await
    }

    /// 弃用版本。
    pub async fn deprecate(&self, id: &DefinitionId) -> WorkflowResult<()> {
        self.definition_repo.deprecate(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::FlowDefinition;
    use crate::repository::InMemoryDefinitionRepository;

    fn make_def(key: &str, ver: &str, active: bool) -> FlowDefinition {
        FlowDefinition {
            flow_key: key.into(),
            version: semver::Version::parse(ver).unwrap(),
            name: "test".into(),
            nodes: vec![],
            start_node: "start".into(),
            active,
            deprecated: false,
            machine: None,
            flow: None,
        }
    }

    #[tokio::test]
    async fn list_versions() {
        let repo = Arc::new(InMemoryDefinitionRepository::default());
        repo.save(&make_def("test", "1.0.0", true)).await.unwrap();
        repo.save(&make_def("test", "2.0.0", false)).await.unwrap();
        let mgr = VersionManager::new(repo);
        let versions = mgr.list_versions("test").await.unwrap();
        assert_eq!(versions.len(), 2);
    }

    #[tokio::test]
    async fn set_active_not_found() {
        let repo = Arc::new(InMemoryDefinitionRepository::default());
        let mgr = VersionManager::new(repo);
        let result = mgr.set_active(&"nonexistent".to_string()).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, WorkflowErrorCode::VersionNotFound);
    }
}
