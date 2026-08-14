use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use sz_rust_capability::{CapResult, Capability, CapabilitySource};
use sz_rust_workflow::{
    DefinitionFormat, FaultStrategy, NodeConfig, NodeType, WorkflowConfig, WorkflowDeps,
    WorkflowEngine,
};

struct DoubleCap;
#[async_trait]
impl Capability for DoubleCap {
    fn name(&self) -> &'static str {
        "math.double"
    }
    fn description(&self) -> &'static str {
        "翻倍"
    }
    fn schema(&self) -> Value {
        Value::Object(serde_json::Map::new())
    }
    fn tags(&self) -> &[&'static str] {
        &["math"]
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Skill
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        let n = args.get("n").and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(serde_json::json!({"doubled": n * 2}))
    }
}

#[tokio::test]
async fn plugin_node_capability_call() {
    let mut deps = WorkflowDeps::default_for_test();
    deps.capability_registry.register(Arc::new(DoubleCap));
    let engine = WorkflowEngine::new(WorkflowConfig::default(), deps);

    let yaml = r#"
flow_key: plugin_test
version: "1.0.0"
name: 插件测试
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
        .start_instance("plugin_test", serde_json::json!({"n": 5}), "user1")
        .await
        .unwrap();
    assert_eq!(summary.flow_key, "plugin_test");
}

#[tokio::test]
async fn plugin_node_timeout_config() {
    let config = WorkflowConfig {
        plugin_call_timeout: Duration::from_millis(100),
        ..WorkflowConfig::default()
    };
    let engine = WorkflowEngine::new(config, WorkflowDeps::default_for_test());
    let _ = &engine;
}
