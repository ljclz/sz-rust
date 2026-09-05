// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use sz_rust_workflow::{DefinitionFormat, WorkflowConfig, WorkflowDeps, WorkflowEngine};

#[tokio::test]
async fn designer_validate_no_persist() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());

    let yaml = r#"
flow_key: designer_test
version: "1.0.0"
name: 设计器测试
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
"#;

    let issues = engine
        .validate_definition(yaml, DefinitionFormat::Yaml)
        .await
        .unwrap();
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == sz_rust_workflow::definition::IssueSeverity::Error)
        .collect();
    assert!(errors.is_empty());

    let detail = engine.query_instance("nonexistent").await;
    assert!(detail.is_err());
    assert_eq!(
        detail.unwrap_err().code,
        sz_rust_workflow::WorkflowErrorCode::InstanceNotFound
    );
}

#[tokio::test]
async fn designer_import_and_export() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());

    let yaml = r#"
flow_key: import_export_test
version: "1.0.0"
name: 导入导出测试
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
active: true
"#;

    let id = engine
        .import_definition(yaml, DefinitionFormat::Yaml)
        .await
        .unwrap();

    let json = engine
        .export_definition(&id, DefinitionFormat::Json)
        .await
        .unwrap();
    assert!(json.contains("import_export_test"));

    let yaml_out = engine
        .export_definition(&id, DefinitionFormat::Yaml)
        .await
        .unwrap();
    assert!(yaml_out.contains("import_export_test"));
}

#[tokio::test]
async fn designer_export_not_found() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());

    let result = engine
        .export_definition(&"nonexistent".to_string(), DefinitionFormat::Json)
        .await;
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().code,
        sz_rust_workflow::WorkflowErrorCode::DefinitionNotFound
    );
}

#[tokio::test]
async fn instance_lifecycle() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());

    let yaml = r#"
flow_key: lifecycle_test
version: "1.0.0"
name: 生命周期测试
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
active: true
"#;
    engine
        .import_definition(yaml, DefinitionFormat::Yaml)
        .await
        .unwrap();

    let summary = engine
        .start_instance("lifecycle_test", serde_json::json!({}), "user1")
        .await
        .unwrap();

    engine
        .suspend_instance(&summary.instance_id, "admin")
        .await
        .unwrap();

    let detail = engine.query_instance(&summary.instance_id).await.unwrap();
    assert_eq!(
        detail.instance.status,
        sz_rust_workflow::instance::InstanceStatus::Suspended
    );

    engine
        .resume_instance(&summary.instance_id, "admin")
        .await
        .unwrap();

    let detail = engine.query_instance(&summary.instance_id).await.unwrap();
    assert_eq!(
        detail.instance.status,
        sz_rust_workflow::instance::InstanceStatus::Running
    );

    engine
        .terminate_instance(&summary.instance_id, "admin")
        .await
        .unwrap();

    let detail = engine.query_instance(&summary.instance_id).await.unwrap();
    assert_eq!(
        detail.instance.status,
        sz_rust_workflow::instance::InstanceStatus::Terminated
    );
}
