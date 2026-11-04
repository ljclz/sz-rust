// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::HashSet;

use sz_rust_sdd::error::SddErrorCode;
use sz_rust_sdd::model::{AiConfig, ReviewDecision, SddConfig, SessionToolWhitelist, Stage};
use sz_rust_sdd::orchestrator::Orchestrator;

use tokio::fs;

fn test_config() -> SddConfig {
    let store_root = std::env::temp_dir().join(format!("sdd-e2e-rb-{}", uuid::Uuid::new_v4()));
    SddConfig {
        store_root,
        ..Default::default()
    }
}

async fn create_session(orch: &Orchestrator) -> sz_rust_sdd::model::SessionId {
    let target = std::env::temp_dir().join("sdd-e2e-rb-target");
    fs::create_dir_all(&target).await.unwrap();
    orch.init_session(
        "test requirement".to_string(),
        target,
        AiConfig::new("test", "model"),
        SessionToolWhitelist::new(HashSet::new()),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn e2e_rollback_to_spec_preserves_v1() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config).unwrap();

    let session_id = create_session(&orch).await;

    orch.run_stage(&session_id).await.unwrap();
    let v1_versions = orch
        .artifact_store()
        .list_versions(&session_id, Stage::Spec)
        .await
        .unwrap();
    assert_eq!(v1_versions.len(), 1);

    orch.submit_review(&session_id, ReviewDecision::Pass)
        .await
        .unwrap();

    orch.run_stage(&session_id).await.unwrap();
    orch.submit_review(
        &session_id,
        ReviewDecision::Rollback {
            target: Stage::Spec,
        },
    )
    .await
    .unwrap();

    let state = orch.resume_session(&session_id).await.unwrap();
    assert_eq!(state.current_stage, Stage::Spec);

    let spec_versions = orch
        .artifact_store()
        .list_versions(&session_id, Stage::Spec)
        .await
        .unwrap();
    assert!(
        !spec_versions.is_empty(),
        "Spec v1 should be preserved after rollback"
    );
}

#[tokio::test]
async fn e2e_rollback_downstream_rejected() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config).unwrap();

    let session_id = create_session(&orch).await;

    orch.run_stage(&session_id).await.unwrap();
    orch.submit_review(&session_id, ReviewDecision::Pass)
        .await
        .unwrap();

    orch.run_stage(&session_id).await.unwrap();

    let err = orch
        .submit_review(
            &session_id,
            ReviewDecision::Rollback {
                target: Stage::Coding,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SddErrorCode::SddRollbackTargetInvalid));
}

#[tokio::test]
async fn e2e_redo_with_comment() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config).unwrap();

    let session_id = create_session(&orch).await;

    orch.run_stage(&session_id).await.unwrap();
    let outcome = orch
        .submit_review(
            &session_id,
            ReviewDecision::Redo {
                comment: "add more detail to spec".to_string(),
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome.new_stage, Stage::Spec);
}

#[tokio::test]
async fn e2e_redo_empty_comment_rejected() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config).unwrap();

    let session_id = create_session(&orch).await;

    orch.run_stage(&session_id).await.unwrap();
    let err = orch
        .submit_review(
            &session_id,
            ReviewDecision::Redo {
                comment: "   ".to_string(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SddErrorCode::SddReviewDecisionInvalid));
}

#[tokio::test]
async fn e2e_review_idempotent() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config).unwrap();

    let session_id = create_session(&orch).await;

    orch.run_stage(&session_id).await.unwrap();
    orch.submit_review(&session_id, ReviewDecision::Pass)
        .await
        .unwrap();

    let err = orch
        .submit_review(&session_id, ReviewDecision::Pass)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SddErrorCode::SddReviewDecisionInvalid));
}
