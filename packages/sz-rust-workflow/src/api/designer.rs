use std::sync::Arc;

use crate::definition::{DefinitionFormat, DefinitionParser, DefinitionValidator, ValidationIssue};
use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};
use crate::repository::{DefinitionId, DefinitionRepository};

use super::version_manager::VersionManager;

/// 设计器 API，对齐 spec 5.6。
pub struct DesignerApi {
    parser: DefinitionParser,
    validator: DefinitionValidator,
    definition_repo: Arc<dyn DefinitionRepository>,
    version_manager: Arc<VersionManager>,
}

impl DesignerApi {
    pub fn new(
        validator: DefinitionValidator,
        definition_repo: Arc<dyn DefinitionRepository>,
        version_manager: Arc<VersionManager>,
    ) -> Self {
        Self {
            parser: DefinitionParser::new(),
            validator,
            definition_repo,
            version_manager,
        }
    }

    /// 校验定义（不持久化）。
    pub async fn validate_definition(
        &self,
        text: &str,
        format: DefinitionFormat,
    ) -> WorkflowResult<Vec<ValidationIssue>> {
        let def = self.parser.parse(text, format)?;
        self.validator.validate(&def).await
    }

    /// 导入定义（校验 + 持久化）。
    pub async fn import_definition(
        &self,
        text: &str,
        format: DefinitionFormat,
    ) -> WorkflowResult<DefinitionId> {
        let def = self.parser.parse(text, format)?;
        let issues = self.validator.validate(&def).await?;
        let has_errors = issues
            .iter()
            .any(|i| i.severity == crate::definition::IssueSeverity::Error);
        if has_errors {
            return Err(WorkflowError::new(
                WorkflowErrorCode::StructureIncomplete,
                "定义校验失败，存在 Error 级问题",
            )
            .with_details(serde_json::to_value(&issues).unwrap_or_default()));
        }
        self.definition_repo.save(&def).await
    }

    /// 导出定义。
    pub async fn export_definition(
        &self,
        id: &DefinitionId,
        format: DefinitionFormat,
    ) -> WorkflowResult<String> {
        let def = self.definition_repo.get(id).await?.ok_or_else(|| {
            WorkflowError::with_field(
                WorkflowErrorCode::DefinitionNotFound,
                "定义不存在",
                "id",
                id,
            )
        })?;
        match format {
            DefinitionFormat::Json => serde_json::to_string(&def).map_err(|e| {
                WorkflowError::new(
                    WorkflowErrorCode::FormatUnsupported,
                    format!("序列化失败：{e}"),
                )
            }),
            DefinitionFormat::Yaml => serde_yaml::to_string(&def).map_err(|e| {
                WorkflowError::new(
                    WorkflowErrorCode::FormatUnsupported,
                    format!("序列化失败：{e}"),
                )
            }),
        }
    }

    /// 获取版本管理器引用。
    pub fn version_manager(&self) -> &VersionManager {
        &self.version_manager
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::InMemoryDefinitionRepository;

    const VALID_YAML: &str = r#"
flow_key: leave_req
version: "1.0.0"
name: 请假
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
"#;

    #[tokio::test]
    async fn validate_definition() {
        let repo = Arc::new(InMemoryDefinitionRepository::default());
        let vm = Arc::new(VersionManager::new(repo.clone()));
        let api = DesignerApi::new(DefinitionValidator::new_noop(), repo, vm);
        let issues = api
            .validate_definition(VALID_YAML, DefinitionFormat::Yaml)
            .await
            .unwrap();
        let errors: Vec<_> = issues
            .iter()
            .filter(|i| i.severity == crate::definition::IssueSeverity::Error)
            .collect();
        assert!(errors.is_empty());
    }

    #[tokio::test]
    async fn import_and_export() {
        let repo = Arc::new(InMemoryDefinitionRepository::default());
        let vm = Arc::new(VersionManager::new(repo.clone()));
        let api = DesignerApi::new(DefinitionValidator::new_noop(), repo, vm);
        let id = api
            .import_definition(VALID_YAML, DefinitionFormat::Yaml)
            .await
            .unwrap();
        let exported = api
            .export_definition(&id, DefinitionFormat::Json)
            .await
            .unwrap();
        assert!(exported.contains("leave_req"));
    }

    #[tokio::test]
    async fn export_not_found() {
        let repo = Arc::new(InMemoryDefinitionRepository::default());
        let vm = Arc::new(VersionManager::new(repo.clone()));
        let api = DesignerApi::new(DefinitionValidator::new_noop(), repo, vm);
        let result = api
            .export_definition(&"nonexistent".to_string(), DefinitionFormat::Json)
            .await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code,
            WorkflowErrorCode::DefinitionNotFound
        );
    }
}
