use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use sz_rust_capability::{
    AllowAll, Cap, CapError, CapResult, Capability, CapabilityRegistry, CapabilitySource,
    TenantScopeChecker,
};

struct EchoCap;

#[async_trait]
impl Capability for EchoCap {
    fn name(&self) -> &'static str {
        "echo"
    }
    fn description(&self) -> &'static str {
        "回显"
    }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "message": { "type": "string" } },
            "required": ["message"]
        })
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

struct ConfirmCap;

#[async_trait]
impl Capability for ConfirmCap {
    fn name(&self) -> &'static str {
        "confirm.cap"
    }
    fn description(&self) -> &'static str {
        "需要确认的能力"
    }
    fn schema(&self) -> Value {
        json!({})
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
    fn requires_confirmation(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn test_validation_error_contains_field() {
    let registry = CapabilityRegistry::new();
    registry.register(Arc::new(EchoCap));
    let result = registry.call("echo", json!({})).await;
    match result {
        Err(CapError::ValidationError(msg)) => {
            assert!(msg.contains("message"), "错误信息应包含字段名, 实际: {msg}")
        }
        other => panic!("期望 ValidationError, 实际: {other:?}"),
    }
}

#[tokio::test]
async fn test_permission_denied() {
    let registry = CapabilityRegistry::new();
    registry.register(Arc::new(EchoCap));
    let checker = TenantScopeChecker::new();
    checker.grant("echo", 100);
    registry.set_permission_checker(Arc::new(checker));
    let result = registry
        .call_with_tenant("echo", json!({"message": "hi"}), 200)
        .await;
    match result {
        Err(CapError::PermissionDenied(msg)) => {
            assert!(msg.contains("200"), "错误信息应包含租户 ID, 实际: {msg}")
        }
        other => panic!("期望 PermissionDenied, 实际: {other:?}"),
    }
}

#[tokio::test]
async fn test_validation_and_permission_pass() {
    let registry = CapabilityRegistry::new();
    registry.register(Arc::new(EchoCap));
    let checker = TenantScopeChecker::new();
    checker.grant("echo", 100);
    registry.set_permission_checker(Arc::new(checker));
    let result = registry
        .call_with_tenant("echo", json!({"message": "hello"}), 100)
        .await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!({"message": "hello"}));
}

#[tokio::test]
async fn test_confirmation_required() {
    let registry = CapabilityRegistry::new();
    registry.register(Arc::new(ConfirmCap));
    let result = registry.call("confirm.cap", json!({})).await;
    assert!(matches!(result, Err(CapError::ConfirmationRequired)));
}

#[tokio::test]
async fn test_no_checker_default_allow() {
    let registry = CapabilityRegistry::new();
    registry.register(Arc::new(EchoCap));
    let result = registry
        .call_with_tenant("echo", json!({"message": "ok"}), 999)
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_concurrent_call_thread_safe() {
    let registry = Arc::new(CapabilityRegistry::new());
    registry.register(Arc::new(EchoCap));
    let checker = TenantScopeChecker::new();
    checker.grant("echo", 1);
    registry.set_permission_checker(Arc::new(checker));

    let mut handles = vec![];
    for _ in 0..10 {
        let reg = registry.clone();
        handles.push(tokio::spawn(async move {
            let result = reg
                .call_with_tenant("echo", json!({"message": "concurrent"}), 1)
                .await;
            assert!(result.is_ok(), "并发调用应成功");
        }));
    }
    for h in handles {
        h.await.expect("线程不应 panic");
    }
    assert_eq!(registry.metrics().call_total, 10);
}

#[tokio::test]
async fn test_facade_set_permission_checker() {
    Cap::init().ok();
    Cap::register(Arc::new(EchoCap)).ok();
    Cap::set_permission_checker(Arc::new(AllowAll)).unwrap();
    let result = Cap::call_with_tenant("echo", json!({"message": "facade"}), 1).await;
    assert!(result.is_ok());
}
