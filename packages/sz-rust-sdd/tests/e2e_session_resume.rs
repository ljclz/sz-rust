// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::HashSet;

use sz_rust_sdd::error::SddErrorCode;
use sz_rust_sdd::model::{AiConfig, ArtifactVersion, SddConfig, SessionToolWhitelist, Stage};
use sz_rust_sdd::orchestrator::{transaction::Transaction, Orchestrator};

use tokio::fs;

fn test_config() -> SddConfig {
    let store_root = std::env::temp_dir().join(format!("sdd-e2e-resume-{}", uuid::Uuid::new_v4()));
    SddConfig {
        store_root,
        ..Default::default()
    }
}

#[tokio::test]
async fn e2e_session_resume_after_stage() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config.clone()).unwrap();

    let target = std::env::temp_dir().join("sdd-e2e-resume-target");
    fs::create_dir_all(&target).await.unwrap();

    let session_id = orch
        .init_session(
            "resume test".to_string(),
            target,
            AiConfig::new("test", "model"),
            SessionToolWhitelist::new(HashSet::new()),
        )
        .await
        .unwrap();

    orch.run_stage(&session_id).await.unwrap();
    orch.submit_review(&session_id, sz_rust_sdd::model::ReviewDecision::Pass)
        .await
        .unwrap();

    let state = orch.resume_session(&session_id).await.unwrap();
    assert_eq!(state.current_stage, Stage::Design);
}

#[tokio::test]
async fn e2e_session_resume_under_5_seconds() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config).unwrap();

    let target = std::env::temp_dir().join("sdd-e2e-resume-time-target");
    fs::create_dir_all(&target).await.unwrap();

    let session_id = orch
        .init_session(
            "timing test".to_string(),
            target,
            AiConfig::new("test", "model"),
            SessionToolWhitelist::new(HashSet::new()),
        )
        .await
        .unwrap();

    let start = std::time::Instant::now();
    let _state = orch.resume_session(&session_id).await.unwrap();
    let duration = start.elapsed();

    assert!(
        duration.as_secs() < 5,
        "resume should complete in < 5 seconds, took {:?}",
        duration
    );
}

#[tokio::test]
async fn e2e_session_corrupted_detection() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config).unwrap();

    let target = std::env::temp_dir().join("sdd-e2e-corrupt-target");
    fs::create_dir_all(&target).await.unwrap();

    let session_id = orch
        .init_session(
            "corruption test".to_string(),
            target,
            AiConfig::new("test", "model"),
            SessionToolWhitelist::new(HashSet::new()),
        )
        .await
        .unwrap();

    let session_dir = orch.session_manager().store_root().join(&session_id.value);
    let session_file = session_dir.join("session.json");
    fs::write(&session_file, b"{\"corrupted\": true}")
        .await
        .unwrap();

    let err = orch.resume_session(&session_id).await.unwrap_err();
    assert_eq!(err.code(), Some(&SddErrorCode::SddSessionCorrupted));
}

#[tokio::test]
async fn e2e_transaction_crash_recovery() {
    let dir = std::env::temp_dir().join(format!("sdd-e2e-crash-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).await.unwrap();

    let mut state = sz_rust_sdd::model::SessionState::new(
        sz_rust_sdd::model::SessionId::new(),
        std::path::PathBuf::from("/tmp/target"),
        AiConfig::new("test", "model"),
        SessionToolWhitelist::new(HashSet::new()),
        std::path::PathBuf::from("/tmp/rules"),
    );
    state
        .artifact_version_pointers
        .insert(Stage::Spec, ArtifactVersion::initial());

    let artifact_path = dir.join("spec_v1.md");
    Transaction::two_phase_commit(&dir, &artifact_path, "# Spec\n\nContent", &state)
        .await
        .unwrap();

    let result = Transaction::verify_consistency(&dir, &state).await;
    assert!(result.is_ok());

    fs::remove_file(&artifact_path).await.unwrap();
    let err = Transaction::verify_consistency(&dir, &state)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SddErrorCode::SddSessionCorrupted));
}
