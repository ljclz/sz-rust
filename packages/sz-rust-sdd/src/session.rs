// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::fs;
use tokio::io;

use crate::error::{SddError, SddErrorCode, SddResult};
use crate::model::{AiConfig, SessionId, SessionState, SessionSummary, SessionToolWhitelist};

const SESSION_FILE: &str = "session.json";
const TMP_SUFFIX: &str = ".tmp";

pub struct SessionManager {
    store_root: PathBuf,
    active_count: AtomicU64,
    max_concurrent: u64,
}

impl SessionManager {
    pub fn new(store_root: PathBuf, max_concurrent: u64) -> Self {
        Self {
            store_root,
            active_count: AtomicU64::new(0),
            max_concurrent,
        }
    }

    pub fn from_config(config: &crate::model::SddConfig) -> Self {
        Self::new(config.store_root.clone(), config.max_concurrent_sessions)
    }

    pub fn active_count(&self) -> u64 {
        self.active_count.load(Ordering::Relaxed)
    }

    pub fn store_root(&self) -> &Path {
        &self.store_root
    }

    pub async fn create_session(
        &self,
        requirement: String,
        target_crate: PathBuf,
        ai_config: AiConfig,
        mut whitelist: SessionToolWhitelist,
        rules_path: PathBuf,
    ) -> SddResult<SessionState> {
        let current = self.active_count.load(Ordering::Relaxed);
        if current >= self.max_concurrent {
            return Err(SddError::new(
                SddErrorCode::SddSessionLimitExceeded,
                format!(
                    "concurrent sessions {current} >= max {}",
                    self.max_concurrent
                ),
            ));
        }

        whitelist.lock();

        let id = SessionId::new();
        let mut state =
            SessionState::new(id.clone(), target_crate, ai_config, whitelist, rules_path);
        state.id = id;

        let session_dir = self.session_dir(&state.id);
        fs::create_dir_all(&session_dir).await?;

        self.save_session(&state).await?;

        let _req_path = session_dir.join("requirement.txt");
        fs::write(&_req_path, requirement).await?;

        self.active_count.fetch_add(1, Ordering::Relaxed);
        Ok(state)
    }

    pub async fn load_session(&self, id: &SessionId) -> SddResult<SessionState> {
        let path = self.session_dir(id).join(SESSION_FILE);
        let data = fs::read(&path).await.map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                SddError::new(
                    SddErrorCode::SddSessionNotFound,
                    format!("session {} not found", id),
                )
            } else {
                SddError::from(e)
            }
        })?;

        let state: SessionState = serde_json::from_slice(&data).map_err(|e| {
            SddError::new(
                SddErrorCode::SddSessionCorrupted,
                format!("session {} corrupted: {e}", id),
            )
        })?;

        if state.id != *id {
            return Err(SddError::new(
                SddErrorCode::SddSessionCorrupted,
                format!("session id mismatch: expected {}, got {}", id, state.id),
            ));
        }

        Ok(state)
    }

    pub async fn save_session(&self, state: &SessionState) -> SddResult<()> {
        let dir = self.session_dir(&state.id);
        fs::create_dir_all(&dir).await?;

        let path = dir.join(SESSION_FILE);
        let tmp_path = dir.join(format!("{SESSION_FILE}{TMP_SUFFIX}"));

        let data = serde_json::to_vec_pretty(state)?;
        fs::write(&tmp_path, &data).await?;
        fs::rename(&tmp_path, &path).await?;

        Ok(())
    }

    pub async fn list_sessions(&self) -> SddResult<Vec<SessionSummary>> {
        let mut entries = fs::read_dir(&self.store_root).await?;
        let mut summaries = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };

            let session_file = path.join(SESSION_FILE);
            if !session_file.exists() {
                continue;
            }

            let id = SessionId { value: dir_name };
            match self.load_session(&id).await {
                Ok(state) => summaries.push(SessionSummary::from(&state)),
                Err(_) => continue,
            }
        }

        summaries.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        Ok(summaries)
    }

    pub fn release_session(&self) {
        let prev = self.active_count.fetch_sub(1, Ordering::Relaxed);
        if prev == 0 {
            self.active_count.store(0, Ordering::Relaxed);
        }
    }

    fn session_dir(&self, id: &SessionId) -> PathBuf {
        self.store_root.join(&id.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn test_config() -> crate::model::SddConfig {
        crate::model::SddConfig {
            store_root: std::env::temp_dir().join(format!("sdd-test-{}", uuid::Uuid::new_v4())),
            ..Default::default()
        }
    }

    async fn setup_mgr() -> SessionManager {
        let config = test_config();
        fs::create_dir_all(&config.store_root).await.unwrap();
        SessionManager::from_config(&config)
    }

    #[tokio::test]
    async fn session_lifecycle() {
        let mgr = setup_mgr().await;
        let target = PathBuf::from("/tmp/crate");
        let ai_config = AiConfig::new("claude", "claude-3");
        let whitelist = SessionToolWhitelist::new(HashSet::new());

        let state = mgr
            .create_session(
                "test requirement".to_string(),
                target.clone(),
                ai_config.clone(),
                whitelist,
                PathBuf::from("/tmp/rules.md"),
            )
            .await
            .unwrap();

        assert_eq!(state.current_stage, crate::model::Stage::Spec);
        assert!(state.whitelist.is_locked());
        assert_eq!(mgr.active_count(), 1);

        let loaded = mgr.load_session(&state.id).await.unwrap();
        assert_eq!(loaded.id, state.id);
        assert_eq!(loaded.target_crate, target);

        let summaries = mgr.list_sessions().await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, state.id);
    }

    #[tokio::test]
    async fn session_concurrent_limit() {
        let config = test_config();
        fs::create_dir_all(&config.store_root).await.unwrap();

        let mgr = SessionManager::new(config.store_root.clone(), 2);
        let target = PathBuf::from("/tmp/crate");
        let ai_config = AiConfig::new("claude", "claude-3");
        let whitelist = SessionToolWhitelist::new(HashSet::new());
        let rules = PathBuf::from("/tmp/rules.md");

        let _s1 = mgr
            .create_session(
                "req1".into(),
                target.clone(),
                ai_config.clone(),
                whitelist.clone(),
                rules.clone(),
            )
            .await
            .unwrap();
        let _s2 = mgr
            .create_session(
                "req2".into(),
                target.clone(),
                ai_config.clone(),
                whitelist.clone(),
                rules.clone(),
            )
            .await
            .unwrap();

        let err = mgr
            .create_session("req3".into(), target, ai_config, whitelist, rules)
            .await
            .unwrap_err();
        assert!(err.is_session_limit_exceeded());
    }

    #[tokio::test]
    async fn session_corrupted() {
        let mgr = setup_mgr().await;
        let target = PathBuf::from("/tmp/crate");
        let ai_config = AiConfig::new("claude", "claude-3");
        let whitelist = SessionToolWhitelist::new(HashSet::new());

        let state = mgr
            .create_session(
                "req".into(),
                target,
                ai_config,
                whitelist,
                PathBuf::from("/tmp/rules.md"),
            )
            .await
            .unwrap();

        let session_file = mgr.session_dir(&state.id).join(SESSION_FILE);
        fs::write(&session_file, b"not valid json").await.unwrap();

        let err = mgr.load_session(&state.id).await.unwrap_err();
        assert!(err.is_session_corrupted());
    }

    #[tokio::test]
    async fn session_not_found() {
        let mgr = setup_mgr().await;
        let fake_id = SessionId::new();
        let err = mgr.load_session(&fake_id).await.unwrap_err();
        assert!(err.is_session_not_found());
    }

    #[tokio::test]
    async fn session_atomic_save() {
        let mgr = setup_mgr().await;
        let target = PathBuf::from("/tmp/crate");
        let ai_config = AiConfig::new("claude", "claude-3");
        let whitelist = SessionToolWhitelist::new(HashSet::new());

        let mut state = mgr
            .create_session(
                "req".into(),
                target,
                ai_config,
                whitelist,
                PathBuf::from("/tmp/rules.md"),
            )
            .await
            .unwrap();

        state.current_stage = crate::model::Stage::Design;
        mgr.save_session(&state).await.unwrap();

        let loaded = mgr.load_session(&state.id).await.unwrap();
        assert_eq!(loaded.current_stage, crate::model::Stage::Design);

        let tmp_file = mgr
            .session_dir(&state.id)
            .join(format!("{SESSION_FILE}{TMP_SUFFIX}"));
        assert!(!tmp_file.exists());
    }
}
