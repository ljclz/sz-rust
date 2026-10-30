// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::time::Instant;

use async_trait::async_trait;

use super::executor::{StageContext, StageExecutor};
use super::validator::{ArtifactValidator, DesignValidator};
use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::{Artifact, ArtifactMeta, ArtifactReviewStatus, ArtifactVersion, Stage};

pub struct DesignStageExecutor {
    validator: DesignValidator,
}

impl DesignStageExecutor {
    pub fn new() -> Self {
        Self {
            validator: DesignValidator::new(),
        }
    }

    fn generate_placeholder_content(
        spec_content: &str,
        legacy_report: &str,
        redo_comment: Option<&str>,
    ) -> String {
        let redo_section = redo_comment
            .map(|c| format!("\n## 修改意见\n{c}\n"))
            .unwrap_or_default();

        format!(
            r#"# SDD Design

## 需求与存量功能关系分析
基于 spec.md 和存量分析，对比需求与现有代码。

### Spec 摘要
{spec_content}

### 存量分析报告
{legacy_report}

## 架构设计
模块划分与依赖关系图。

## 接口设计
API 签名与数据流定义。

## 数据模型
持久化策略与版本化方案。
{redo_section}
"#
        )
    }

    async fn load_spec_content(ctx: &StageContext) -> String {
        let versions = ctx
            .artifact_store
            .list_versions(&ctx.session_id, Stage::Spec)
            .await
            .unwrap_or_default();

        if let Some(latest) = versions.last() {
            ctx.artifact_store
                .load_artifact(&ctx.session_id, Stage::Spec, *latest)
                .await
                .map(|a| a.content)
                .unwrap_or_default()
        } else {
            String::new()
        }
    }
}

impl Default for DesignStageExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StageExecutor for DesignStageExecutor {
    fn stage(&self) -> Stage {
        Stage::Design
    }

    async fn execute(&self, ctx: &StageContext) -> SddResult<ArtifactMeta> {
        let start = Instant::now();

        let spec_content = Self::load_spec_content(ctx).await;
        let legacy_report = "# 存量分析报告\n（占位 — 后续集成 LegacyAnalyzer）";

        let _prompt = ctx.prompt_builder.build_design_prompt(
            &spec_content,
            legacy_report,
            ctx.redo_comment.as_deref(),
        )?;

        tracing::info!(session_id = %ctx.session_id, "design stage prompt built");

        let content = Self::generate_placeholder_content(
            &spec_content,
            legacy_report,
            ctx.redo_comment.as_deref(),
        );

        let artifact = Artifact {
            session_id: ctx.session_id.clone(),
            stage: Stage::Design,
            version: ArtifactVersion::initial(),
            content: content.clone(),
            meta: ArtifactMeta::new(Stage::Design),
        };

        let validation = self.validator.validate(&artifact).await?;
        if !validation.passed {
            return Err(SddError::new(
                SddErrorCode::SddStageGenFailed,
                format!(
                    "design artifact validation failed: {:?}",
                    validation.violations
                ),
            ));
        }

        let mut meta = ArtifactMeta::new(Stage::Design);
        meta.review_status = ArtifactReviewStatus::Pending;

        let version = ctx
            .artifact_store
            .save_artifact(&ctx.session_id, Stage::Design, &content, meta.clone())
            .await?;

        let duration_ms = start.elapsed().as_millis() as u64;
        ctx.trace
            .record_stage_end(&ctx.session_id, Stage::Design, duration_ms, version.value)
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
