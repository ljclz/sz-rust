// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::collections::HashSet;

use sz_rust_sdd::error::SddErrorCode;
use sz_rust_sdd::model::{AiConfig, SddConfig, SessionToolWhitelist};
use sz_rust_sdd::orchestrator::Orchestrator;

use tokio::fs;

fn test_config() -> SddConfig {
    let store_root = std::env::temp_dir().join(format!("sdd-e2e-conc-{}", uuid::Uuid::new_v4()));
    SddConfig {
        store_root,
        max_concurrent_sessions: 8,
        ..Default::default()
    }
}

#[tokio::test]
async fn e2e_concurrent_8_sessions_success() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config).unwrap();

    let target = std::env::temp_dir().join("sdd-e2e-conc-target");
    fs::create_dir_all(&target).await.unwrap();

    for i in 0..8 {
        let result = orch
            .init_session(
                format!("requirement {}", i),
                target.clone(),
                AiConfig::new("test", "model"),
                SessionToolWhitelist::new(HashSet::new()),
            )
            .await;
        assert!(
            result.is_ok(),
            "session {} creation should succeed: {:?}",
            i,
            result
        );
    }
}

#[tokio::test]
async fn e2e_concurrent_9th_session_rejected() {
    let config = test_config();
    fs::create_dir_all(&config.store_root).await.unwrap();
    let orch = Orchestrator::new(config).unwrap();

    let target = std::env::temp_dir().join("sdd-e2e-conc-limit-target");
    fs::create_dir_all(&target).await.unwrap();

    for i in 0..8 {
        orch.init_session(
            format!("req {}", i),
            target.clone(),
            AiConfig::new("test", "model"),
            SessionToolWhitelist::new(HashSet::new()),
        )
        .await
        .unwrap();
    }

    let err = orch
        .init_session(
            "9th session".to_string(),
            target,
            AiConfig::new("test", "model"),
            SessionToolWhitelist::new(HashSet::new()),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some(&SddErrorCode::SddSessionLimitExceeded));
}

#[tokio::test]
async fn e2e_capability_unauthorized_blocked() {
    use sz_rust_capability::CapabilityRegistry;
    use sz_rust_sdd::capability::guard::SddCapabilityGuard;

    let registry = std::sync::Arc::new(CapabilityRegistry::new());
    let guard = SddCapabilityGuard::new(registry);

    let err = guard.check_llm_chat(1).await.unwrap_err();
    assert_eq!(err.code(), Some(&SddErrorCode::SddCapabilityUnauthorized));
}

#[tokio::test]
async fn e2e_whitelist_blocks_unlisted_tool() {
    use sz_rust_sdd::model::SessionToolWhitelist;

    let empty_set = HashSet::new();
    let whitelist = SessionToolWhitelist::new(empty_set);

    assert!(!whitelist.is_allowed("fs_write"));
    assert!(!whitelist.is_allowed("git_commit"));
    assert!(!whitelist.is_allowed("unknown_tool"));
}

#[tokio::test]
async fn e2e_whitelist_allows_listed_tool() {
    use sz_rust_sdd::model::SessionToolWhitelist;

    let mut tools = HashSet::new();
    tools.insert("fs_read".to_string());
    tools.insert("fs_write".to_string());
    let whitelist = SessionToolWhitelist::new(tools);

    assert!(whitelist.is_allowed("fs_read"));
    assert!(whitelist.is_allowed("fs_write"));
    assert!(!whitelist.is_allowed("git_commit"));
}

#[tokio::test]
async fn e2e_whitelist_locked_after_creation() {
    use sz_rust_sdd::model::SessionToolWhitelist;

    let mut tools = HashSet::new();
    tools.insert("fs_read".to_string());
    let mut whitelist = SessionToolWhitelist::new(tools);

    assert!(!whitelist.is_locked());
    whitelist.lock();
    assert!(whitelist.is_locked());
}
