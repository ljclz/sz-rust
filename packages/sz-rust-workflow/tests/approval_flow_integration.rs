// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use sz_rust_workflow::{
    DefinitionFormat, DefinitionParser, TaskAction, WorkflowConfig, WorkflowDeps, WorkflowEngine,
};

const APPROVAL_YAML: &str = r#"
flow_key: approval_test
version: "1.0.0"
name: 审批流测试
nodes:
  - node_id: start
    node_type: start
    kind: start
    next: approve1
  - node_id: approve1
    node_type: approval
    kind: approval
    approval_strategy: and_sign
    candidate_strategy:
      type: static
      users:
        - user1
        - user2
    next: end
  - node_id: end
    node_type: end
    kind: end
start_node: start
active: true
"#;

#[tokio::test]
async fn approval_flow_start_and_query() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());

    let id = engine
        .import_definition(APPROVAL_YAML, DefinitionFormat::Yaml)
        .await
        .unwrap();

    let summary = engine
        .start_instance("approval_test", serde_json::json!({}), "initiator")
        .await
        .unwrap();
    assert_eq!(summary.flow_key, "approval_test");

    let detail = engine.query_instance(&summary.instance_id).await.unwrap();
    assert!(!detail.current_tasks.is_empty());

    let exported = engine
        .export_definition(&id, DefinitionFormat::Json)
        .await
        .unwrap();
    assert!(exported.contains("approval_test"));
}

#[tokio::test]
async fn approval_flow_pending_tasks() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());

    engine
        .import_definition(APPROVAL_YAML, DefinitionFormat::Yaml)
        .await
        .unwrap();

    let summary = engine
        .start_instance("approval_test", serde_json::json!({}), "initiator")
        .await
        .unwrap();

    let tasks = engine
        .query_tasks("user1", sz_rust_workflow::instance::PageRequest::default())
        .await
        .unwrap();
    assert!(tasks.total >= 1);

    let _ = summary;
}

#[tokio::test]
async fn approval_flow_handle_task_approve() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());
    engine
        .import_definition(APPROVAL_YAML, DefinitionFormat::Yaml)
        .await
        .unwrap();

    let summary = engine
        .start_instance("approval_test", serde_json::json!({}), "initiator")
        .await
        .unwrap();

    let tasks = engine
        .query_tasks("user1", sz_rust_workflow::instance::PageRequest::default())
        .await
        .unwrap();
    assert!(tasks.total >= 1);
    let task = &tasks.items[0];

    let def = DefinitionParser::new()
        .parse(APPROVAL_YAML, DefinitionFormat::Yaml)
        .unwrap();

    let result = engine
        .handle_task(&task.task_id, TaskAction::Approve, None, "user1", &def)
        .await;
    assert!(result.is_ok(), "handle_task should succeed: {:?}", result);
    let handle_result = result.unwrap();
    assert_eq!(handle_result.task_id, task.task_id);
    assert_eq!(handle_result.action, TaskAction::Approve);

    let _ = summary;
}

#[tokio::test]
async fn approval_flow_handle_task_not_found() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());
    engine
        .import_definition(APPROVAL_YAML, DefinitionFormat::Yaml)
        .await
        .unwrap();

    let def = DefinitionParser::new()
        .parse(APPROVAL_YAML, DefinitionFormat::Yaml)
        .unwrap();

    let result = engine
        .handle_task("nonexistent", TaskAction::Approve, None, "user1", &def)
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        sz_rust_workflow::WorkflowErrorCode::InstanceNotFound
    );
}

#[tokio::test]
async fn approval_flow_handle_task_unauthorized() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());
    engine
        .import_definition(APPROVAL_YAML, DefinitionFormat::Yaml)
        .await
        .unwrap();

    let summary = engine
        .start_instance("approval_test", serde_json::json!({}), "initiator")
        .await
        .unwrap();

    let tasks = engine
        .query_tasks("user1", sz_rust_workflow::instance::PageRequest::default())
        .await
        .unwrap();
    assert!(tasks.total >= 1);
    let task = &tasks.items[0];

    let def = DefinitionParser::new()
        .parse(APPROVAL_YAML, DefinitionFormat::Yaml)
        .unwrap();

    let result = engine
        .handle_task(&task.task_id, TaskAction::Approve, None, "intruder", &def)
        .await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        sz_rust_workflow::WorkflowErrorCode::UnauthorizedHandle
    );

    let _ = summary;
}

#[tokio::test]
async fn approval_flow_handle_task_reject() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());
    engine
        .import_definition(APPROVAL_YAML, DefinitionFormat::Yaml)
        .await
        .unwrap();

    let summary = engine
        .start_instance("approval_test", serde_json::json!({}), "initiator")
        .await
        .unwrap();

    let tasks = engine
        .query_tasks("user1", sz_rust_workflow::instance::PageRequest::default())
        .await
        .unwrap();
    assert!(tasks.total >= 1);
    let task = &tasks.items[0];

    let def = DefinitionParser::new()
        .parse(APPROVAL_YAML, DefinitionFormat::Yaml)
        .unwrap();

    let result = engine
        .handle_task(
            &task.task_id,
            TaskAction::Reject,
            Some("rejected by user1".into()),
            "user1",
            &def,
        )
        .await;
    assert!(
        result.is_ok(),
        "handle_task reject should succeed: {:?}",
        result
    );
    let handle_result = result.unwrap();
    assert_eq!(handle_result.action, TaskAction::Reject);

    let _ = summary;
}
