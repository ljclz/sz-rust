use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::definition::NodeConfig;
use crate::error::{WorkflowError, WorkflowErrorCode, WorkflowResult};
use crate::integration::SensitiveFieldRegistry;
use crate::scheduling::fault_strategy::{FaultDecision, FaultStrategyHandler, PluginNodeOutcome};

/// 插件节点执行器，对齐 design 2.2.2.8。
pub struct PluginNodeExecutor {
    capability_registry: Arc<sz_rust_capability::CapabilityRegistry>,
    fault_handler: Arc<dyn FaultStrategyHandler>,
    sensitive_registry: Arc<SensitiveFieldRegistry>,
    timeout: Duration,
}

impl PluginNodeExecutor {
    pub fn new(
        capability_registry: Arc<sz_rust_capability::CapabilityRegistry>,
        fault_handler: Arc<dyn FaultStrategyHandler>,
        sensitive_registry: Arc<SensitiveFieldRegistry>,
        timeout: Duration,
    ) -> Self {
        Self {
            capability_registry,
            fault_handler,
            sensitive_registry,
            timeout,
        }
    }

    /// 执行插件节点。
    pub async fn execute(
        &self,
        node: &NodeConfig,
        context: &mut Value,
    ) -> WorkflowResult<PluginNodeOutcome> {
        let (capability_name, version_range, args_mapping, fault_strategy, _output_schema, next) =
            match node {
                NodeConfig::Plugin {
                    capability_name,
                    capability_version_range,
                    args_mapping,
                    fault_strategy,
                    output_schema,
                    next,
                } => (
                    capability_name,
                    capability_version_range,
                    args_mapping,
                    *fault_strategy,
                    output_schema,
                    next,
                ),
                _ => {
                    return Err(WorkflowError::new(
                        WorkflowErrorCode::CapabilityNotFound,
                        "非插件节点",
                    ))
                }
            };

        let _ = next;
        let args = self.resolve_args(args_mapping, context);

        let capability = match self.capability_registry.get(capability_name) {
            Some(c) => c,
            None => {
                let wf_error = WorkflowError::with_field(
                    WorkflowErrorCode::CapabilityNotFound,
                    "能力不存在",
                    "capability",
                    capability_name,
                );
                let decision = self.fault_handler.decide(fault_strategy, &wf_error, 0);
                return match decision {
                    FaultDecision::Terminate => Ok(PluginNodeOutcome::InstanceTerminated),
                    FaultDecision::Skip => Ok(PluginNodeOutcome::Skipped),
                    FaultDecision::Retry { .. } => Ok(PluginNodeOutcome::InstanceTerminated),
                };
            }
        };

        let req = semver::VersionReq::parse(version_range).map_err(|e| {
            WorkflowError::with_field(
                WorkflowErrorCode::CapabilityNotFound,
                format!("非法版本范围：{e}"),
                "range",
                version_range,
            )
        })?;
        let cap_version =
            semver::Version::parse(capability.version()).unwrap_or(semver::Version::new(0, 0, 0));
        if !req.matches(&cap_version) {
            return Err(WorkflowError::with_field(
                WorkflowErrorCode::CapabilityNotFound,
                format!(
                    "能力版本 {} 不满足范围 {}",
                    capability.version(),
                    version_range
                ),
                "capability",
                capability_name,
            ));
        }

        let mut attempt = 0u32;
        loop {
            let call_result = tokio::time::timeout(
                self.timeout,
                self.capability_registry.call(capability_name, args.clone()),
            )
            .await;

            match call_result {
                Ok(Ok(result)) => {
                    self.sensitive_registry
                        .merge_capability_result(context, result);
                    return Ok(PluginNodeOutcome::Completed);
                }
                Ok(Err(e)) => {
                    let wf_error = WorkflowError::new(
                        WorkflowErrorCode::CapabilityNotFound,
                        format!("能力调用失败：{e}"),
                    );
                    let decision = self
                        .fault_handler
                        .decide(fault_strategy, &wf_error, attempt);
                    match decision {
                        FaultDecision::Terminate => {
                            return Ok(PluginNodeOutcome::InstanceTerminated)
                        }
                        FaultDecision::Skip => return Ok(PluginNodeOutcome::Skipped),
                        FaultDecision::Retry { backoff, .. } => {
                            attempt += 1;
                            tokio::time::sleep(backoff).await;
                            continue;
                        }
                    }
                }
                Err(_) => {
                    let wf_error = WorkflowError::with_field(
                        WorkflowErrorCode::CapabilityTimeout,
                        "能力调用超时",
                        "capability",
                        capability_name,
                    );
                    let decision = self
                        .fault_handler
                        .decide(fault_strategy, &wf_error, attempt);
                    match decision {
                        FaultDecision::Terminate => {
                            return Ok(PluginNodeOutcome::InstanceTerminated)
                        }
                        FaultDecision::Skip => return Ok(PluginNodeOutcome::Skipped),
                        FaultDecision::Retry { backoff, .. } => {
                            attempt += 1;
                            tokio::time::sleep(backoff).await;
                            continue;
                        }
                    }
                }
            }
        }
    }

    fn resolve_args(&self, mapping: &Value, context: &Value) -> Value {
        if mapping.is_null() {
            return context.clone();
        }
        if let Value::Object(map) = mapping {
            let mut result = serde_json::Map::new();
            for (k, v) in map {
                if let Some(s) = v.as_str() {
                    if s.starts_with("$.") {
                        if let Some(val) = lookup(context, &s[2..]) {
                            result.insert(k.clone(), val.clone());
                            continue;
                        }
                    }
                }
                result.insert(k.clone(), v.clone());
            }
            return Value::Object(result);
        }
        mapping.clone()
    }
}

fn lookup<'a>(ctx: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = ctx;
    for part in path.split('.') {
        if let Value::Object(obj) = current {
            current = obj.get(part)?;
        } else {
            return None;
        }
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::FaultStrategy;
    use crate::scheduling::fault_strategy::DefaultFaultStrategyHandler;
    use async_trait::async_trait;
    use sz_rust_capability::{CapResult, Capability, CapabilitySource};

    struct EchoCap;
    #[async_trait]
    impl Capability for EchoCap {
        fn name(&self) -> &'static str {
            "test.echo"
        }
        fn description(&self) -> &'static str {
            "回显"
        }
        fn schema(&self) -> Value {
            Value::Object(serde_json::Map::new())
        }
        fn tags(&self) -> &[&'static str] {
            &["test"]
        }
        fn source(&self) -> CapabilitySource {
            CapabilitySource::Skill
        }
        async fn call(&self, args: Value) -> CapResult<Value> {
            Ok(args)
        }
    }

    #[tokio::test]
    async fn execute_success() {
        let registry = Arc::new(sz_rust_capability::CapabilityRegistry::new());
        registry.register(Arc::new(EchoCap));
        let executor = PluginNodeExecutor::new(
            registry,
            Arc::new(DefaultFaultStrategyHandler::default()),
            Arc::new(SensitiveFieldRegistry::new()),
            Duration::from_secs(5),
        );
        let node = NodeConfig::Plugin {
            capability_name: "test.echo".into(),
            capability_version_range: "*".into(),
            args_mapping: serde_json::json!({"key": "value"}),
            fault_strategy: FaultStrategy::Fail,
            output_schema: None,
            next: "end".into(),
        };
        let mut ctx = serde_json::json!({});
        let result = executor.execute(&node, &mut ctx).await.unwrap();
        assert_eq!(result, PluginNodeOutcome::Completed);
        assert_eq!(ctx["key"], "value");
    }

    #[tokio::test]
    async fn execute_capability_not_found() {
        let registry = Arc::new(sz_rust_capability::CapabilityRegistry::new());
        let executor = PluginNodeExecutor::new(
            registry,
            Arc::new(DefaultFaultStrategyHandler::default()),
            Arc::new(SensitiveFieldRegistry::new()),
            Duration::from_secs(5),
        );
        let node = NodeConfig::Plugin {
            capability_name: "nonexistent.cap".into(),
            capability_version_range: "*".into(),
            args_mapping: Value::Null,
            fault_strategy: FaultStrategy::Fail,
            output_schema: None,
            next: "end".into(),
        };
        let mut ctx = serde_json::json!({});
        let result = executor.execute(&node, &mut ctx).await.unwrap();
        assert_eq!(result, PluginNodeOutcome::InstanceTerminated);
    }
}
