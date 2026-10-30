// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::HashSet;

use sz_rust_sdd::error::SddErrorCode;
use sz_rust_sdd::model::{AiConfig, ReviewDecision, SddConfig, SessionToolWhitelist, Stage};
use sz_rust_sdd::orchestrator::Orchestrator;

use tokio::fs;

fn test_config() -> SddConfig {
    let store_root = std::env::temp_dir().join(format!("sdd-e2e-full-{}", uuid::Uuid::new_v4()));
    SddConfig {
        store_root,
        ..Default::default()
    }
}

#[tokio::test]
async fn e2e_full_flow_four_stages() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config.clone()).unwrap();

    let target = std::env::temp_dir().join("sdd-e2e-target");
    fs::create_dir_all(&target).await.unwrap();

    let ai_config = AiConfig::new("test", "test-model");
    let whitelist = SessionToolWhitelist::new(HashSet::new());

    let session_id = orch
        .init_session(
            "Build a hello world CLI tool".to_string(),
            target.clone(),
            ai_config,
            whitelist,
        )
        .await
        .unwrap();

    let stages = [Stage::Spec, Stage::Design, Stage::Task, Stage::Coding];
    for stage in &stages {
        let next = orch.run_stage(&session_id).await.unwrap();
        if stage != &Stage::Coding {
            assert!(
                !next.is_terminal(),
                "stage {} should not be terminal",
                stage
            );
        }

        let state = orch.resume_session(&session_id).await.unwrap();

        let versions = orch
            .artifact_store()
            .list_versions(&session_id, *stage)
            .await
            .unwrap();
        assert!(
            !versions.is_empty(),
            "stage {} should have artifacts",
            stage
        );

        let outcome = orch
            .submit_review(&session_id, ReviewDecision::Pass)
            .await
            .unwrap();
        assert_eq!(outcome.decision, ReviewDecision::Pass);

        let _ = state;
    }

    let trace = orch.query_trace(&session_id).await.unwrap();
    assert!(
        !trace.stage_records.is_empty(),
        "trace should have stage records"
    );
    assert!(
        !trace.review_decisions.is_empty(),
        "trace should have review decisions"
    );
}

#[tokio::test]
async fn e2e_full_flow_artifact_persisted() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config.clone()).unwrap();

    let target = std::env::temp_dir().join("sdd-e2e-persist-target");
    fs::create_dir_all(&target).await.unwrap();

    let session_id = orch
        .init_session(
            "test requirement".to_string(),
            target,
            AiConfig::new("test", "model"),
            SessionToolWhitelist::new(HashSet::new()),
        )
        .await
        .unwrap();

    orch.run_stage(&session_id).await.unwrap();

    let versions = orch
        .artifact_store()
        .list_versions(&session_id, Stage::Spec)
        .await
        .unwrap();
    assert!(!versions.is_empty(), "Spec artifact should be persisted");

    let artifact = orch
        .artifact_store()
        .load_artifact(&session_id, Stage::Spec, versions[0])
        .await
        .unwrap();
    assert!(
        !artifact.content.is_empty(),
        "artifact content should not be empty"
    );
}

#[tokio::test]
async fn e2e_full_flow_trace_records_reviews() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config.clone()).unwrap();

    let target = std::env::temp_dir().join("sdd-e2e-trace-target");
    fs::create_dir_all(&target).await.unwrap();

    let session_id = orch
        .init_session(
            "trace test".to_string(),
            target,
            AiConfig::new("test", "model"),
            SessionToolWhitelist::new(HashSet::new()),
        )
        .await
        .unwrap();

    orch.run_stage(&session_id).await.unwrap();
    orch.submit_review(&session_id, ReviewDecision::Pass)
        .await
        .unwrap();

    let trace = orch.query_trace(&session_id).await.unwrap();
    assert!(
        !trace.review_decisions.is_empty(),
        "trace should record review decisions"
    );
    assert_eq!(trace.review_decisions[0].decision, ReviewDecision::Pass);
}

#[tokio::test]
async fn e2e_full_flow_session_not_found() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config).unwrap();

    let fake_id = sz_rust_sdd::model::SessionId::new();
    let err = orch.resume_session(&fake_id).await.unwrap_err();
    assert_eq!(err.code(), Some(&SddErrorCode::SddSessionNotFound));
}
