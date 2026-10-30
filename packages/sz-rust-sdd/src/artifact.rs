// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::{Path, PathBuf};

use tokio::fs;

use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::{
    Artifact, ArtifactDiff, ArtifactMeta, ArtifactReviewStatus, ArtifactSummary, ArtifactVersion,
    SessionId, Stage,
};

pub struct ArtifactStore {
    store_root: PathBuf,
}

impl ArtifactStore {
    pub fn new(store_root: PathBuf) -> Self {
        Self { store_root }
    }

    pub fn from_config(config: &crate::model::SddConfig) -> Self {
        Self::new(config.store_root.clone())
    }

    pub fn store_root(&self) -> &Path {
        &self.store_root
    }

    pub async fn save_artifact(
        &self,
        session_id: &SessionId,
        stage: Stage,
        content: &str,
        mut meta: ArtifactMeta,
    ) -> SddResult<ArtifactVersion> {
        let session_dir = self.store_root.join(&session_id.value);
        fs::create_dir_all(&session_dir).await?;

        let versions = self.list_versions(session_id, stage).await?;
        let version = match versions.last() {
            Some(v) => v.next(),
            None => ArtifactVersion::initial(),
        };

        meta.generated_stage = stage;
        if meta.parent_version.is_none() {
            if let Some(parent_val) = version.parent {
                meta.parent_version = Some(ArtifactVersion {
                    value: parent_val,
                    parent: None,
                });
            }
        }

        let content_path = session_dir.join(stage.artifact_filename(version.value));
        let meta_path = session_dir.join(self.meta_filename(stage, version.value));

        if content_path.exists() {
            let existing_meta = self.load_meta(session_id, stage, version).await;
            if let Ok(existing) = existing_meta {
                if existing.review_status == ArtifactReviewStatus::Pending {
                    return Err(SddError::new(
                        SddErrorCode::SddStorePermissionDenied,
                        format!(
                            "artifact {stage} v{} is pending review, cannot overwrite",
                            version.value
                        ),
                    ));
                }
            }
        }

        let meta_data = serde_json::to_vec_pretty(&meta)?;
        fs::write(&content_path, content).await?;
        fs::write(&meta_path, meta_data).await?;

        Ok(version)
    }

    pub async fn load_artifact(
        &self,
        session_id: &SessionId,
        stage: Stage,
        version: ArtifactVersion,
    ) -> SddResult<Artifact> {
        let content_path = self
            .store_root
            .join(&session_id.value)
            .join(stage.artifact_filename(version.value));

        let content = fs::read_to_string(&content_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SddError::new(
                    SddErrorCode::SddArtifactVersionNotFound,
                    format!("artifact {stage} v{} not found", version.value),
                )
            } else {
                SddError::from(e)
            }
        })?;

        let meta = self.load_meta(session_id, stage, version).await?;

        Ok(Artifact {
            session_id: session_id.clone(),
            stage,
            version,
            content,
            meta,
        })
    }

    pub async fn list_versions(
        &self,
        session_id: &SessionId,
        stage: Stage,
    ) -> SddResult<Vec<ArtifactVersion>> {
        let session_dir = self.store_root.join(&session_id.value);
        if !session_dir.exists() {
            return Ok(Vec::new());
        }

        let prefix = match stage {
            Stage::Spec => "spec_v",
            Stage::Design => "design_v",
            Stage::Task => "task_v",
            Stage::Coding => "coding_v",
            Stage::Completed => "completed_v",
            Stage::Failed => "failed_v",
        };

        let mut entries = fs::read_dir(&session_dir).await?;
        let mut versions = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let fname = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };

            if !fname.starts_with(prefix) || !fname.ends_with(".md") {
                continue;
            }

            let num_str = &fname[prefix.len()..fname.len() - 3];
            if let Ok(value) = num_str.parse::<u32>() {
                versions.push(ArtifactVersion {
                    value,
                    parent: if value > 1 { Some(value - 1) } else { None },
                });
            }
        }

        versions.sort_by_key(|v| v.value);
        Ok(versions)
    }

    pub async fn diff_artifacts(
        &self,
        session_id: &SessionId,
        stage: Stage,
        v1: ArtifactVersion,
        v2: ArtifactVersion,
    ) -> SddResult<ArtifactDiff> {
        let art1 = self.load_artifact(session_id, stage, v1).await?;
        let art2 = self.load_artifact(session_id, stage, v2).await?;

        let diff = similar::TextDiff::from_lines(&art1.content, &art2.content);
        let mut added_lines = 0usize;
        let mut removed_lines = 0usize;

        for change in diff.iter_all_changes() {
            match change.tag() {
                similar::ChangeTag::Insert => added_lines += 1,
                similar::ChangeTag::Delete => removed_lines += 1,
                _ => {}
            }
        }

        let modified_lines = added_lines.min(removed_lines);
        let added_lines = added_lines.saturating_sub(modified_lines);
        let removed_lines = removed_lines.saturating_sub(modified_lines);

        let diff_text = diff.unified_diff().context_radius(3).to_string();

        Ok(ArtifactDiff {
            stage,
            v1,
            v2,
            added_lines,
            removed_lines,
            modified_lines,
            diff_text,
        })
    }

    pub async fn export_artifact(
        &self,
        session_id: &SessionId,
        stage: Stage,
        version: ArtifactVersion,
        output: &Path,
    ) -> SddResult<()> {
        let artifact = self.load_artifact(session_id, stage, version).await?;

        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).await?;
        }

        fs::write(output, artifact.content).await?;
        Ok(())
    }

    pub async fn compute_summary(
        &self,
        session_id: &SessionId,
        stage: Stage,
        version: ArtifactVersion,
    ) -> SddResult<ArtifactSummary> {
        let artifact = self.load_artifact(session_id, stage, version).await?;
        let content = &artifact.content;

        let section_titles: Vec<String> = content
            .lines()
            .filter(|line| line.starts_with("# "))
            .map(|line| line.trim_start_matches("# ").to_string())
            .collect();

        let task_count = if stage == Stage::Task {
            content
                .lines()
                .filter(|line| line.starts_with("- [ ]") || line.starts_with("- [x]"))
                .count()
                .into()
        } else {
            None
        };

        let code_change_lines = if stage == Stage::Coding {
            content
                .lines()
                .filter(|line| line.starts_with("+") || line.starts_with("-"))
                .count()
                .into()
        } else {
            None
        };

        Ok(ArtifactSummary {
            stage,
            version,
            content_length: content.len(),
            section_titles,
            task_count,
            code_change_lines,
        })
    }

    fn meta_filename(&self, stage: Stage, version: u32) -> String {
        let name = match stage {
            Stage::Spec => "spec",
            Stage::Design => "design",
            Stage::Task => "task",
            Stage::Coding => "coding",
            Stage::Completed => "completed",
            Stage::Failed => "failed",
        };
        format!("{name}_v{version}.meta.json")
    }

    async fn load_meta(
        &self,
        session_id: &SessionId,
        stage: Stage,
        version: ArtifactVersion,
    ) -> SddResult<ArtifactMeta> {
        let meta_path = self
            .store_root
            .join(&session_id.value)
            .join(self.meta_filename(stage, version.value));

        let data = fs::read(&meta_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SddError::new(
                    SddErrorCode::SddArtifactVersionNotFound,
                    format!("meta for {stage} v{} not found", version.value),
                )
            } else {
                SddError::from(e)
            }
        })?;

        let meta: ArtifactMeta = serde_json::from_slice(&data)?;
        Ok(meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn test_store_root() -> PathBuf {
        std::env::temp_dir().join(format!("sdd-artifact-test-{}", uuid::Uuid::new_v4()))
    }

    async fn setup_store() -> (ArtifactStore, SessionId) {
        let root = test_store_root();
        fs::create_dir_all(&root).await.unwrap();
        let store = ArtifactStore::new(root);
        let session_id = SessionId::new();
        fs::create_dir_all(store.store_root.join(&session_id.value))
            .await
            .unwrap();
        (store, session_id)
    }

    fn sample_meta(stage: Stage) -> ArtifactMeta {
        ArtifactMeta {
            generated_at: Utc::now(),
            generated_stage: stage,
            review_status: ArtifactReviewStatus::Pending,
            ai_call_records: Vec::new(),
            parent_version: None,
        }
    }

    #[tokio::test]
    async fn artifact_version_increment() {
        let (store, session_id) = setup_store().await;

        let v1 = store
            .save_artifact(
                &session_id,
                Stage::Spec,
                "# Spec v1",
                sample_meta(Stage::Spec),
            )
            .await
            .unwrap();
        assert_eq!(v1.value, 1);

        let v2 = store
            .save_artifact(
                &session_id,
                Stage::Spec,
                "# Spec v2",
                sample_meta(Stage::Spec),
            )
            .await
            .unwrap();
        assert_eq!(v2.value, 2);
        assert_eq!(v2.parent, Some(1));

        let v3 = store
            .save_artifact(
                &session_id,
                Stage::Spec,
                "# Spec v3",
                sample_meta(Stage::Spec),
            )
            .await
            .unwrap();
        assert_eq!(v3.value, 3);

        let art1 = store
            .load_artifact(&session_id, Stage::Spec, v1)
            .await
            .unwrap();
        assert_eq!(art1.content, "# Spec v1");

        let art3 = store
            .load_artifact(&session_id, Stage::Spec, v3)
            .await
            .unwrap();
        assert_eq!(art3.content, "# Spec v3");
    }

    #[tokio::test]
    async fn artifact_diff() {
        let (store, session_id) = setup_store().await;

        let old_content = "line1\nline2\nline3\n";
        let new_content = "line1\nline2_modified\nline3\nline4\n";

        let v1 = store
            .save_artifact(
                &session_id,
                Stage::Design,
                old_content,
                sample_meta(Stage::Design),
            )
            .await
            .unwrap();
        let v2 = store
            .save_artifact(
                &session_id,
                Stage::Design,
                new_content,
                sample_meta(Stage::Design),
            )
            .await
            .unwrap();

        let diff = store
            .diff_artifacts(&session_id, Stage::Design, v1, v2)
            .await
            .unwrap();

        assert_eq!(diff.stage, Stage::Design);
        assert!(diff.added_lines > 0 || diff.modified_lines > 0);
        assert!(!diff.diff_text.is_empty());
    }

    #[tokio::test]
    async fn artifact_export() {
        let (store, session_id) = setup_store().await;

        let content = "# Exported spec\n";
        let v1 = store
            .save_artifact(&session_id, Stage::Spec, content, sample_meta(Stage::Spec))
            .await
            .unwrap();

        let export_path =
            std::env::temp_dir().join(format!("sdd-export-{}-spec.md", uuid::Uuid::new_v4()));
        store
            .export_artifact(&session_id, Stage::Spec, v1, &export_path)
            .await
            .unwrap();

        let exported = fs::read_to_string(&export_path).await.unwrap();
        assert_eq!(exported, content);
    }

    #[tokio::test]
    async fn artifact_not_overwrite_unreviewed() {
        let (store, session_id) = setup_store().await;

        let mut meta = sample_meta(Stage::Task);
        meta.review_status = ArtifactReviewStatus::Pending;
        let v1 = store
            .save_artifact(&session_id, Stage::Task, "# Task v1", meta)
            .await
            .unwrap();

        let mut pending_meta = sample_meta(Stage::Task);
        pending_meta.review_status = ArtifactReviewStatus::Pending;

        let result = store
            .save_artifact(&session_id, Stage::Task, "# overwritten", pending_meta)
            .await;

        let v2 = result.unwrap();
        assert_ne!(v2.value, v1.value);

        let original = store
            .load_artifact(&session_id, Stage::Task, v1)
            .await
            .unwrap();
        assert_eq!(original.content, "# Task v1");
    }

    #[tokio::test]
    async fn artifact_list_versions() {
        let (store, session_id) = setup_store().await;

        for i in 1..=3 {
            store
                .save_artifact(
                    &session_id,
                    Stage::Coding,
                    &format!("# Coding v{i}"),
                    sample_meta(Stage::Coding),
                )
                .await
                .unwrap();
        }

        let versions = store
            .list_versions(&session_id, Stage::Coding)
            .await
            .unwrap();
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].value, 1);
        assert_eq!(versions[1].value, 2);
        assert_eq!(versions[2].value, 3);
    }

    #[tokio::test]
    async fn artifact_version_not_found() {
        let (store, session_id) = setup_store().await;

        let err = store
            .load_artifact(&session_id, Stage::Spec, ArtifactVersion::initial())
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddArtifactVersionNotFound));
    }
}
