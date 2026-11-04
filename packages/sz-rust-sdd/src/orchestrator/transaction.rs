// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::{Path, PathBuf};

use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::{SessionState, Stage};

pub struct Transaction;

impl Transaction {
    pub async fn write_artifact_atomic(path: &Path, content: &str) -> SddResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        let tmp_path = Self::tmp_path(path);

        let mut file = fs::File::create(&tmp_path).await?;
        file.write_all(content.as_bytes()).await?;
        file.sync_all().await?;
        file.flush().await?;
        drop(file);

        fs::rename(&tmp_path, path).await?;
        Ok(())
    }

    pub async fn write_session_atomic(session_dir: &Path, state: &SessionState) -> SddResult<()> {
        fs::create_dir_all(session_dir).await?;

        let path = session_dir.join("session.json");
        let tmp_path = session_dir.join("session.json.tmp");

        let data = serde_json::to_vec_pretty(state)?;

        let mut file = fs::File::create(&tmp_path).await?;
        file.write_all(&data).await?;
        file.sync_all().await?;
        file.flush().await?;
        drop(file);

        fs::rename(&tmp_path, &path).await?;
        Ok(())
    }

    pub async fn two_phase_commit(
        session_dir: &Path,
        artifact_path: &Path,
        artifact_content: &str,
        state: &SessionState,
    ) -> SddResult<()> {
        Self::write_artifact_atomic(artifact_path, artifact_content).await?;
        Self::write_session_atomic(session_dir, state).await?;
        Ok(())
    }

    pub async fn verify_consistency(session_dir: &Path, state: &SessionState) -> SddResult<()> {
        for (stage, version) in &state.artifact_version_pointers {
            let filename = stage.artifact_filename(version.value);
            let artifact_path = session_dir.join(&filename);
            if !artifact_path.exists() {
                return Err(SddError::new(
                    SddErrorCode::SddSessionCorrupted,
                    format!(
                        "session corrupted: {} version {} (file {}) does not exist",
                        stage, version.value, filename
                    ),
                ));
            }
        }
        Ok(())
    }

    pub async fn check_artifact_exists(session_dir: &Path, stage: Stage, version: u32) -> bool {
        let filename = stage.artifact_filename(version);
        session_dir.join(filename).exists()
    }

    fn tmp_path(path: &Path) -> PathBuf {
        let mut tmp = path.as_os_str().to_os_string();
        tmp.push(".tmp");
        PathBuf::from(tmp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AiConfig, ArtifactVersion, SessionId, SessionState, SessionToolWhitelist};
    use std::collections::HashSet;

    async fn setup_session_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sdd-tx-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).await.unwrap();
        dir
    }

    fn make_session_state() -> SessionState {
        let id = SessionId::new();
        let ai_config = AiConfig::new("test", "test-model");
        let whitelist = SessionToolWhitelist::new(HashSet::new());
        SessionState::new(
            id,
            PathBuf::from("/tmp/target"),
            ai_config,
            whitelist,
            PathBuf::from("/tmp/rules"),
        )
    }

    #[tokio::test]
    async fn transaction_atomic_write() {
        let dir = setup_session_dir().await;
        let path = dir.join("artifact.md");

        Transaction::write_artifact_atomic(&path, "test content")
            .await
            .unwrap();

        let content = fs::read_to_string(&path).await.unwrap();
        assert_eq!(content, "test content");

        assert!(!dir.join("artifact.md.tmp").exists());
    }

    #[tokio::test]
    async fn transaction_session_atomic_write() {
        let dir = setup_session_dir().await;
        let state = make_session_state();

        Transaction::write_session_atomic(&dir, &state)
            .await
            .unwrap();

        let session_path = dir.join("session.json");
        assert!(session_path.exists());

        let data = fs::read_to_string(&session_path).await.unwrap();
        let _: SessionState = serde_json::from_str(&data).unwrap();
    }

    #[tokio::test]
    async fn transaction_two_phase_commit() {
        let dir = setup_session_dir().await;
        let state = make_session_state();
        let artifact_path = dir.join("spec_v1.md");

        Transaction::two_phase_commit(&dir, &artifact_path, "# Spec\n\nContent", &state)
            .await
            .unwrap();

        assert!(artifact_path.exists());
        assert!(dir.join("session.json").exists());
    }

    #[tokio::test]
    async fn transaction_crash_recovery_consistent() {
        let dir = setup_session_dir().await;
        let mut state = make_session_state();
        state
            .artifact_version_pointers
            .insert(crate::model::Stage::Spec, ArtifactVersion::initial());

        let artifact_path = dir.join("spec_v1.md");
        Transaction::two_phase_commit(&dir, &artifact_path, "# Spec", &state)
            .await
            .unwrap();

        let result = Transaction::verify_consistency(&dir, &state).await;
        assert!(
            result.is_ok(),
            "consistent state should verify: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn transaction_crash_recovery_corrupted() {
        let dir = setup_session_dir().await;
        let mut state = make_session_state();
        state
            .artifact_version_pointers
            .insert(crate::model::Stage::Spec, ArtifactVersion::initial());

        let session_path = dir.join("session.json");
        let data = serde_json::to_vec_pretty(&state).unwrap();
        fs::write(&session_path, &data).await.unwrap();

        let err = Transaction::verify_consistency(&dir, &state)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddSessionCorrupted));
    }

    #[tokio::test]
    async fn transaction_check_artifact_exists() {
        let dir = setup_session_dir().await;
        let path = dir.join("spec_v1.md");
        fs::write(&path, "content").await.unwrap();

        assert!(Transaction::check_artifact_exists(&dir, Stage::Spec, 1).await);
        assert!(!Transaction::check_artifact_exists(&dir, Stage::Spec, 2).await);
        assert!(!Transaction::check_artifact_exists(&dir, Stage::Design, 1).await);
    }
}
