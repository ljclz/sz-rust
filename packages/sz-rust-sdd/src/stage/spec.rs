// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::time::Instant;

use async_trait::async_trait;

use super::executor::{StageContext, StageExecutor};
use super::validator::{ArtifactValidator, SpecValidator};
use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::{Artifact, ArtifactMeta, ArtifactReviewStatus, ArtifactVersion, Stage};

pub struct SpecStageExecutor {
    validator: SpecValidator,
}

impl SpecStageExecutor {
    pub fn new() -> Self {
        Self {
            validator: SpecValidator::new(),
        }
    }

    fn generate_placeholder_content(requirement: &str, redo_comment: Option<&str>) -> String {
        let redo_section = redo_comment
            .map(|c| format!("\n## 修改意见\n{c}\n"))
            .unwrap_or_default();

        format!(
            r#"# SDD Spec

## 组件定位
基于需求"{requirement}"，定义组件名称、职责与边界。

## 领域术语
定义领域核心概念与术语表。

## 角色边界
明确用户角色与权限边界。

## DFX 约束
- 性能：响应时间 < 200ms
- 安全：敏感字段脱敏
- 可用性：99.9% SLA
- 可维护性：模块化设计

## 核心能力
列举核心功能与接口定义。

## 数据约束
数据模型与持久化约束。
{redo_section}
"#
        )
    }
}

impl Default for SpecStageExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StageExecutor for SpecStageExecutor {
    fn stage(&self) -> Stage {
        Stage::Spec
    }

    async fn execute(&self, ctx: &StageContext) -> SddResult<ArtifactMeta> {
        let start = Instant::now();

        let _prompt = ctx
            .prompt_builder
            .build_spec_prompt(&ctx.requirement, ctx.redo_comment.as_deref())?;

        tracing::info!(session_id = %ctx.session_id, "spec stage prompt built");

        let content =
            Self::generate_placeholder_content(&ctx.requirement, ctx.redo_comment.as_deref());

        let artifact = Artifact {
            session_id: ctx.session_id.clone(),
            stage: Stage::Spec,
            version: ArtifactVersion::initial(),
            content: content.clone(),
            meta: ArtifactMeta::new(Stage::Spec),
        };

        let validation = self.validator.validate(&artifact).await?;
        if !validation.passed {
            return Err(SddError::new(
                SddErrorCode::SddStageGenFailed,
                format!(
                    "spec artifact validation failed: {:?}",
                    validation.violations
                ),
            ));
        }

        let mut meta = ArtifactMeta::new(Stage::Spec);
        meta.review_status = ArtifactReviewStatus::Pending;

        let version = ctx
            .artifact_store
            .save_artifact(&ctx.session_id, Stage::Spec, &content, meta.clone())
            .await?;

        let duration_ms = start.elapsed().as_millis() as u64;
        ctx.trace
            .record_stage_end(&ctx.session_id, Stage::Spec, duration_ms, version.value)
            .await?;

        meta.parent_version = Some(version);
        Ok(meta)
    }

    async fn validate_artifact(
        &self,
        artifact: &Artifact,
    ) -> SddResult<crate::model::ValidationResult> {
        self.validator.validate(artifact).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::ArtifactStore;
    use crate::model::{AiConfig, SessionId, SessionState, SessionToolWhitelist};
    use crate::prompt::PromptBuilder;
    use crate::trace::Trace;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::Arc;

    async fn setup_ctx() -> StageContext {
        let root = std::env::temp_dir().join(format!("sdd-spec-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(ArtifactStore::new(root.clone()));
        let trace = Arc::new(Trace::new(root.clone()));
        let prompt_builder = Arc::new(PromptBuilder::new().unwrap());
        let session_id = SessionId::new();
        let state = SessionState::new(
            session_id.clone(),
            PathBuf::from("/tmp/crate"),
            AiConfig::new("test", "test"),
            SessionToolWhitelist::new(HashSet::new()),
            PathBuf::from("/tmp/rules.md"),
        );
        StageContext::new(
            session_id,
            state,
            "实现用户登录".to_string(),
            PathBuf::from("/tmp/crate"),
            store,
            trace,
            prompt_builder,
        )
    }

    #[tokio::test]
    async fn spec_stage_generate() {
        let ctx = setup_ctx().await;
        let executor = SpecStageExecutor::new();
        let meta = executor.execute(&ctx).await.unwrap();
        assert_eq!(meta.generated_stage, Stage::Spec);
        assert_eq!(meta.review_status, ArtifactReviewStatus::Pending);
    }
}
