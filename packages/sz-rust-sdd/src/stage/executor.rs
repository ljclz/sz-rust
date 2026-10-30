// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::artifact::ArtifactStore;
use crate::error::SddResult;
use crate::model::{Artifact, ArtifactMeta, SessionId, SessionState, Stage, ValidationResult};
use crate::prompt::PromptBuilder;
use crate::trace::Trace;

pub struct StageContext {
    pub session_id: SessionId,
    pub session_state: SessionState,
    pub requirement: String,
    pub target_crate: PathBuf,
    pub artifact_store: Arc<ArtifactStore>,
    pub trace: Arc<Trace>,
    pub prompt_builder: Arc<PromptBuilder>,
    pub redo_comment: Option<String>,
}

impl StageContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: SessionId,
        session_state: SessionState,
        requirement: String,
        target_crate: PathBuf,
        artifact_store: Arc<ArtifactStore>,
        trace: Arc<Trace>,
        prompt_builder: Arc<PromptBuilder>,
    ) -> Self {
        Self {
            session_id,
            session_state,
            requirement,
            target_crate,
            artifact_store,
            trace,
            prompt_builder,
            redo_comment: None,
        }
    }

    pub fn with_redo_comment(mut self, comment: String) -> Self {
        self.redo_comment = Some(comment);
        self
    }
}

#[async_trait]
pub trait StageExecutor: Send + Sync + 'static {
    fn stage(&self) -> Stage;

    async fn execute(&self, ctx: &StageContext) -> SddResult<ArtifactMeta>;

    async fn validate_artifact(&self, artifact: &Artifact) -> SddResult<ValidationResult>;
}

pub struct StageExecutorRegistry {
    executors: Vec<Box<dyn StageExecutor>>,
}

impl StageExecutorRegistry {
    pub fn new() -> Self {
        Self {
            executors: Vec::new(),
        }
    }

    pub fn register(&mut self, executor: Box<dyn StageExecutor>) {
        self.executors.push(executor);
    }

    pub fn get(&self, stage: Stage) -> Option<&dyn StageExecutor> {
        self.executors
            .iter()
            .find(|e| e.stage() == stage)
            .map(|e| e.as_ref())
    }

    pub fn stages(&self) -> Vec<Stage> {
        self.executors.iter().map(|e| e.stage()).collect()
    }
}

impl Default for StageExecutorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AiConfig, SessionToolWhitelist};
    use std::collections::HashSet;

    struct DummyExecutor {
        stage: Stage,
    }

    #[async_trait]
    impl StageExecutor for DummyExecutor {
        fn stage(&self) -> Stage {
            self.stage
        }

        async fn execute(&self, _ctx: &StageContext) -> SddResult<ArtifactMeta> {
            Ok(ArtifactMeta::new(self.stage))
        }

        async fn validate_artifact(&self, _artifact: &Artifact) -> SddResult<ValidationResult> {
            Ok(ValidationResult::passed())
        }
    }

    fn dummy_context() -> StageContext {
        let root = std::env::temp_dir().join(format!("sdd-ctx-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(ArtifactStore::new(root.clone()));
        let trace = Arc::new(Trace::new(root.clone()));
        let prompt_builder = Arc::new(crate::prompt::PromptBuilder::new().unwrap());
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
            "test requirement".to_string(),
            PathBuf::from("/tmp/crate"),
            store,
            trace,
            prompt_builder,
        )
    }

    #[tokio::test]
    async fn stage_executor_registry_dispatch() {
        let mut registry = StageExecutorRegistry::new();
        registry.register(Box::new(DummyExecutor { stage: Stage::Spec }));
        registry.register(Box::new(DummyExecutor {
            stage: Stage::Design,
        }));
        registry.register(Box::new(DummyExecutor { stage: Stage::Task }));
        registry.register(Box::new(DummyExecutor {
            stage: Stage::Coding,
        }));

        assert_eq!(registry.stages().len(), 4);

        let executor = registry.get(Stage::Spec).unwrap();
        assert_eq!(executor.stage(), Stage::Spec);

        let ctx = dummy_context();
        let meta = executor.execute(&ctx).await.unwrap();
        assert_eq!(meta.generated_stage, Stage::Spec);

        let artifact = Artifact {
            session_id: ctx.session_id.clone(),
            stage: Stage::Spec,
            version: crate::model::ArtifactVersion::initial(),
            content: "# Spec".to_string(),
            meta: ArtifactMeta::new(Stage::Spec),
        };
        let result = executor.validate_artifact(&artifact).await.unwrap();
        assert!(result.passed);
    }

    #[tokio::test]
    async fn stage_context_redo_comment() {
        let ctx = dummy_context();
        assert!(ctx.redo_comment.is_none());

        let ctx_with_redo = ctx.with_redo_comment("fix this".to_string());
        assert_eq!(ctx_with_redo.redo_comment.as_deref(), Some("fix this"));
    }
}
