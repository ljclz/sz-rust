// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

mod common;

use sz_rust_migration::orchestrator::{MigrationOrchestrator, MigrationStartRequest};
use sz_rust_migration::validator::HttpRequestSample;

#[tokio::test]
async fn orchestrator_full_migration_flow() {
    let fixture = common::tp6_fixture::Tp6Fixture::create_minimal().await;
    let output_dir = tempfile::TempDir::new().unwrap();

    let config_content = r#"
version = "1.0.0"

[model]
analysis_model = "deepseek-chat"
generation_model = "deepseek-coder"

[target_crate]
path = ""
allow_overwrite = true

[validation]
tp6_service_url = ""
sz_rust_service_url = ""
timeout_secs = 5
max_retries = 1
"#;
    let config_path = output_dir.path().join("migration.toml");
    tokio::fs::write(&config_path, config_content)
        .await
        .unwrap();

    let request = MigrationStartRequest {
        tp6_project_path: fixture.root().to_path_buf(),
        config_path,
        output_dir: output_dir.path().to_path_buf(),
        validation_samples: vec![
            HttpRequestSample::get("/user/info"),
            HttpRequestSample::post(
                "/user/create",
                serde_json::json!({"name": "test", "email": "test@test.com"}),
            ),
        ],
        tenant_id: "test-tenant".into(),
    };

    let orchestrator = MigrationOrchestrator::new();
    let handle = orchestrator.start(request).await.unwrap();

    let migration_id = handle.migration_id().await;
    assert!(!migration_id.is_empty());

    for _ in 0..20 {
        let status = handle.status().await;
        match status {
            sz_rust_migration::phase::MigrationStatus::Completed => break,
            sz_rust_migration::phase::MigrationStatus::Failed { error_code } => {
                panic!("Migration failed: {error_code}");
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
        }
    }

    let analysis_path = output_dir.path().join("analysis_report.json");
    assert!(analysis_path.exists(), "analysis_report.json should exist");

    let report_path = output_dir.path().join("migration_report.md");
    assert!(report_path.exists(), "migration_report.md should exist");
}
