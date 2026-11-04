// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::time::Instant;

use async_trait::async_trait;

use super::executor::{StageContext, StageExecutor};
use super::validator::{ArtifactValidator, TaskValidator};
use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::{Artifact, ArtifactMeta, ArtifactReviewStatus, ArtifactVersion, Stage};

pub struct TaskStageExecutor {
    validator: TaskValidator,
}

impl TaskStageExecutor {
    pub fn new() -> Self {
        Self {
            validator: TaskValidator::new(),
        }
    }

    fn generate_placeholder_content(design_content: &str, redo_comment: Option<&str>) -> String {
        let redo_section = redo_comment
            .map(|c| format!("\n## 修改意见\n{c}\n"))
            .unwrap_or_default();

        format!(
            r#"# SDD Task

## 任务清单

### 组1：脚手架
- [ ] 任务1.1：创建 crate 骨架
  - 命令：cargo new --lib packages/new-crate
  - 验证：cargo check -p new-crate
  - 依赖：无

### 组2：核心实现
- [ ] 任务2.1：实现核心逻辑
  - 命令：cargo build -p new-crate
  - 验证：cargo test -p new-crate
  - 依赖：任务1.1

### 组3：测试
- [ ] 任务3.1：编写单元测试
  - 命令：cargo test -p new-crate --lib
  - 验证：cargo test -p new-crate --lib
  - 依赖：任务2.1

## Design 摘要
{design_content}
{redo_section}
"#
        )
    }

    async fn load_design_content(ctx: &StageContext) -> String {
        let versions = ctx
            .artifact_store
            .list_versions(&ctx.session_id, Stage::Design)
            .await
            .unwrap_or_default();

        if let Some(latest) = versions.last() {
            ctx.artifact_store
                .load_artifact(&ctx.session_id, Stage::Design, *latest)
                .await
                .map(|a| a.content)
                .unwrap_or_default()
        } else {
            String::new()
        }
    }
}

impl Default for TaskStageExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StageExecutor for TaskStageExecutor {
    fn stage(&self) -> Stage {
        Stage::Task
    }

    async fn execute(&self, ctx: &StageContext) -> SddResult<ArtifactMeta> {
        let start = Instant::now();

        let design_content = Self::load_design_content(ctx).await;

        let _prompt = ctx
            .prompt_builder
            .build_task_prompt(&design_content, ctx.redo_comment.as_deref())?;

        tracing::info!(session_id = %ctx.session_id, "task stage prompt built");

        let content =
            Self::generate_placeholder_content(&design_content, ctx.redo_comment.as_deref());

        let artifact = Artifact {
            session_id: ctx.session_id.clone(),
            stage: Stage::Task,
            version: ArtifactVersion::initial(),
            content: content.clone(),
            meta: ArtifactMeta::new(Stage::Task),
        };

        let validation = self.validator.validate(&artifact).await?;
        if !validation.passed {
            return Err(SddError::new(
                SddErrorCode::SddStageGenFailed,
                format!(
                    "task artifact validation failed: {:?}",
                    validation.violations
                ),
            ));
        }

        let mut meta = ArtifactMeta::new(Stage::Task);
        meta.review_status = ArtifactReviewStatus::Pending;

        let version = ctx
            .artifact_store
            .save_artifact(&ctx.session_id, Stage::Task, &content, meta.clone())
            .await?;

        let duration_ms = start.elapsed().as_millis() as u64;
        ctx.trace
            .record_stage_end(&ctx.session_id, Stage::Task, duration_ms, version.value)
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
