use sz_rust_workflow::{
    DefinitionFormat, TaskAction, WorkflowConfig, WorkflowDeps, WorkflowEngine,
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
