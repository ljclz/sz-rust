use sz_rust_workflow::{DefinitionFormat, WorkflowConfig, WorkflowDeps, WorkflowEngine};

const STATE_MACHINE_YAML: &str = r#"
flow_key: state_machine_test
version: "1.0.0"
name: 状态机测试
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
machine:
  initial_state: draft
  states:
    - draft
    - review
    - done
  transitions:
    - from: draft
      to: review
      event: submit
    - from: review
      to: done
      event: approve
      guard: "$.amount > 100"
"#;

#[tokio::test]
async fn state_machine_end_to_end() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());

    engine
        .import_definition(STATE_MACHINE_YAML, DefinitionFormat::Yaml)
        .await
        .unwrap();

    let summary = engine
        .start_instance(
            "state_machine_test",
            serde_json::json!({"current_state": "draft"}),
            "user1",
        )
        .await
        .unwrap();
    assert_eq!(summary.flow_key, "state_machine_test");
}

#[tokio::test]
async fn state_machine_instance_not_found() {
    let engine = WorkflowEngine::new(WorkflowConfig::default(), WorkflowDeps::default_for_test());

    let machine = sz_rust_workflow::definition::StateMachineDefinition {
        initial_state: "draft".into(),
        states: vec!["draft".into(), "done".into()],
        transitions: vec![sz_rust_workflow::definition::Transition {
            from: "draft".into(),
            to: "done".into(),
            event: "submit".into(),
            guard: None,
        }],
    };

    let result = engine
        .fire_event("nonexistent", "submit", &machine, serde_json::json!({}))
        .await;
    assert!(result.is_err());
    assert_eq!(
        result.unwrap_err().code,
        sz_rust_workflow::WorkflowErrorCode::InstanceNotFound
    );
}
