// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

mod common;

use sz_rust_migration::analyzer::Tp6AnalysisInput;
use sz_rust_migration::analyzer::Tp6Analyzer;

#[tokio::test]
async fn analyze_full_tp6_project() {
    let fixture = common::tp6_fixture::Tp6Fixture::create_minimal().await;
    let analyzer = Tp6Analyzer::new();
    let input = Tp6AnalysisInput {
        project_path: fixture.root().to_path_buf(),
        exclude_paths: vec![],
        llm_assist: false,
        trace_id: "test-trace".into(),
    };
    let report = analyzer.analyze(&input).await.unwrap();

    assert_eq!(report.version, "6.0.13");
    assert_eq!(report.routes.len(), 2);
    assert_eq!(report.models.len(), 1);
    assert_eq!(report.controllers.len(), 1);

    assert!(report.routes.iter().any(|r| r.path_pattern == "user/info"));
    assert!(report
        .routes
        .iter()
        .any(|r| r.path_pattern == "user/create"));

    assert!(report
        .models
        .iter()
        .any(|m| m.class_name == "User" && m.table_name == "users"));

    assert!(report
        .controllers
        .iter()
        .any(|c| c.class_name == "UserController" && c.actions.len() == 2));

    assert!(!report.dependency_graph.has_cycle);
}

#[tokio::test]
async fn analyze_nonexistent_path() {
    let analyzer = Tp6Analyzer::new();
    let input = Tp6AnalysisInput {
        project_path: std::path::PathBuf::from("/nonexistent/path"),
        exclude_paths: vec![],
        llm_assist: false,
        trace_id: "test-trace".into(),
    };
    let result = analyzer.analyze(&input).await;
    assert!(result.is_err());
}
