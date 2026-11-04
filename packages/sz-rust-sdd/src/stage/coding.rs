// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::time::Instant;

use async_trait::async_trait;

use super::executor::{StageContext, StageExecutor};
use super::validator::{ArtifactValidator, CodingValidator};
use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::{Artifact, ArtifactMeta, ArtifactReviewStatus, ArtifactVersion, Stage};

pub struct CodingStageExecutor {
    validator: CodingValidator,
}

impl CodingStageExecutor {
    pub fn new() -> Self {
        Self {
            validator: CodingValidator::new(),
        }
    }

    fn generate_placeholder_content(task_content: &str, redo_comment: Option<&str>) -> String {
        let redo_section = redo_comment
            .map(|c| format!("\n// 修改意见: {c}\n"))
            .unwrap_or_default();

        format!(
            r#"// SDD Coding — 生成的代码变更集
// 所有代码遵守铁律：async fn Send + 'static、tokio::fs、禁止不安全代码、无 allow(dead_code)

use tokio::fs;

pub async fn execute() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {{
    let content = fs::read_to_string("config.toml").await?;
    Ok(())
}}
{redo_section}
// Task 摘要
// {task_content}
"#
        )
    }

    async fn load_task_content(ctx: &StageContext) -> String {
        let versions = ctx
            .artifact_store
            .list_versions(&ctx.session_id, Stage::Task)
            .await
            .unwrap_or_default();

        if let Some(latest) = versions.last() {
            ctx.artifact_store
                .load_artifact(&ctx.session_id, Stage::Task, *latest)
                .await
                .map(|a| a.content)
                .unwrap_or_default()
        } else {
            String::new()
        }
    }
}

impl Default for CodingStageExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StageExecutor for CodingStageExecutor {
    fn stage(&self) -> Stage {
        Stage::Coding
    }

    async fn execute(&self, ctx: &StageContext) -> SddResult<ArtifactMeta> {
        let start = Instant::now();

        let task_content = Self::load_task_content(ctx).await;
        let whitelist = ctx.session_state.whitelist.allowed_tools();
        let whitelist_str = whitelist
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        let _prompt = ctx.prompt_builder.build_coding_prompt(
            &task_content,
            &whitelist_str,
            ctx.redo_comment.as_deref(),
        )?;

        tracing::info!(session_id = %ctx.session_id, "coding stage prompt built");

        let content =
            Self::generate_placeholder_content(&task_content, ctx.redo_comment.as_deref());

        let artifact = Artifact {
            session_id: ctx.session_id.clone(),
            stage: Stage::Coding,
            version: ArtifactVersion::initial(),
            content: content.clone(),
            meta: ArtifactMeta::new(Stage::Coding),
        };

        let validation = self.validator.validate(&artifact).await?;
        if !validation.passed {
            return Err(SddError::new(
                SddErrorCode::SddStageGenFailed,
                format!(
                    "coding artifact validation failed: {:?}",
                    validation.violations
                ),
            ));
        }

        let mut meta = ArtifactMeta::new(Stage::Coding);
        meta.review_status = ArtifactReviewStatus::Pending;

        let version = ctx
            .artifact_store
            .save_artifact(&ctx.session_id, Stage::Coding, &content, meta.clone())
            .await?;

        let duration_ms = start.elapsed().as_millis() as u64;
        ctx.trace
            .record_stage_end(&ctx.session_id, Stage::Coding, duration_ms, version.value)
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
