// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use sz_rust_capability::{
    CapError, CapResult, Capability, CapabilityRegistry, CapabilitySource, PermissionChecker,
};

use crate::error::{SddError, SddErrorCode, SddResult};

pub const SDD_CAPABILITIES: &[&str] = &[
    "sdd.session.create",
    "sdd.session.resume",
    "sdd.review.submit",
    "sdd.llm_chat",
    "sdd.mcp_call",
    "sdd.fs_write",
    "sdd.legacy_analyze",
    "sdd.cargo_check",
];

pub struct SddNoOpCapability {
    name: &'static str,
    description: &'static str,
}

impl SddNoOpCapability {
    pub fn new(name: &'static str, description: &'static str) -> Self {
        Self { name, description }
    }
}

#[async_trait]
impl Capability for SddNoOpCapability {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn tags(&self) -> &[&'static str] {
        &["sdd"]
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Service
    }

    async fn call(&self, _args: Value) -> CapResult<Value> {
        Ok(json!({ "status": "ok" }))
    }
}

pub fn register_sdd_capabilities(registry: &CapabilityRegistry) -> Vec<&'static str> {
    let caps = vec![
        Arc::new(SddNoOpCapability::new(
            "sdd.session.create",
            "Create SDD session",
        )),
        Arc::new(SddNoOpCapability::new(
            "sdd.session.resume",
            "Resume SDD session",
        )),
        Arc::new(SddNoOpCapability::new(
            "sdd.review.submit",
            "Submit review decision",
        )),
        Arc::new(SddNoOpCapability::new(
            "sdd.llm_chat",
            "Invoke LLM inference",
        )),
        Arc::new(SddNoOpCapability::new("sdd.mcp_call", "Invoke MCP tool")),
        Arc::new(SddNoOpCapability::new(
            "sdd.fs_write",
            "Write session store",
        )),
        Arc::new(SddNoOpCapability::new(
            "sdd.legacy_analyze",
            "Analyze legacy crate",
        )),
        Arc::new(SddNoOpCapability::new(
            "sdd.cargo_check",
            "Run cargo check/test/clippy",
        )),
    ];

    for cap in caps {
        registry.register(cap);
    }

    SDD_CAPABILITIES.to_vec()
}

pub struct SddCapabilityGuard {
    registry: Arc<CapabilityRegistry>,
    permission_checker: Option<Arc<dyn PermissionChecker>>,
}

impl SddCapabilityGuard {
    pub fn new(registry: Arc<CapabilityRegistry>) -> Self {
        Self {
            registry,
            permission_checker: None,
        }
    }

    pub fn with_permission_checker(
        registry: Arc<CapabilityRegistry>,
        checker: Arc<dyn PermissionChecker>,
    ) -> Self {
        Self {
            registry,
            permission_checker: Some(checker),
        }
    }

    pub fn registry(&self) -> &CapabilityRegistry {
        &self.registry
    }

    pub async fn check(
        &self,
        capability_name: &str,
        args: &Value,
        tenant_id: i64,
    ) -> SddResult<()> {
        let cap = self.registry.get(capability_name).ok_or_else(|| {
            SddError::new(
                SddErrorCode::SddCapabilityUnauthorized,
                format!("capability '{}' not registered", capability_name),
            )
        })?;

        if let Some(checker) = &self.permission_checker {
            checker
                .check(capability_name, args, tenant_id)
                .await
                .map_err(|e| match e {
                    CapError::PermissionDenied(msg) => SddError::new(
                        SddErrorCode::SddCapabilityUnauthorized,
                        format!("permission denied for '{}': {}", capability_name, msg),
                    ),
                    other => SddError::new(
                        SddErrorCode::SddCapabilityUnauthorized,
                        format!(
                            "permission check failed for '{}': {}",
                            capability_name, other
                        ),
                    ),
                })?;
        }

        let _ = cap;
        Ok(())
    }

    pub async fn check_llm_chat(&self, tenant_id: i64) -> SddResult<()> {
        self.check("sdd.llm_chat", &json!({}), tenant_id).await
    }

    pub async fn check_mcp_call(&self, tool_name: &str, tenant_id: i64) -> SddResult<()> {
        self.check("sdd.mcp_call", &json!({ "tool": tool_name }), tenant_id)
            .await
    }

    pub async fn check_fs_write(&self, path: &str, tenant_id: i64) -> SddResult<()> {
        self.check("sdd.fs_write", &json!({ "path": path }), tenant_id)
            .await
    }

    pub async fn check_session_create(&self, tenant_id: i64) -> SddResult<()> {
        self.check("sdd.session.create", &json!({}), tenant_id)
            .await
    }

    pub async fn check_session_resume(&self, tenant_id: i64) -> SddResult<()> {
        self.check("sdd.session.resume", &json!({}), tenant_id)
            .await
    }

    pub async fn check_review_submit(&self, tenant_id: i64) -> SddResult<()> {
        self.check("sdd.review.submit", &json!({}), tenant_id).await
    }

    pub async fn check_legacy_analyze(&self, tenant_id: i64) -> SddResult<()> {
        self.check("sdd.legacy_analyze", &json!({}), tenant_id)
            .await
    }

    pub async fn check_cargo_check(&self, tenant_id: i64) -> SddResult<()> {
        self.check("sdd.cargo_check", &json!({}), tenant_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sz_rust_capability::AllowAll;

    fn setup_guard() -> SddCapabilityGuard {
        let registry = Arc::new(CapabilityRegistry::new());
        register_sdd_capabilities(&registry);
        SddCapabilityGuard::with_permission_checker(registry, Arc::new(AllowAll))
    }

    #[tokio::test]
    async fn capability_guard_check() {
        let guard = setup_guard();
        let result = guard.check_llm_chat(1).await;
        assert!(result.is_ok(), "AllowAll should pass: {:?}", result);
    }

    #[tokio::test]
    async fn capability_unauthorized_blocked() {
        let registry = Arc::new(CapabilityRegistry::new());
        let guard = SddCapabilityGuard::new(registry);

        let err = guard.check_llm_chat(1).await.unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddCapabilityUnauthorized));
    }

    #[tokio::test]
    async fn capability_all_sdd_registered() {
        let registry = Arc::new(CapabilityRegistry::new());
        let names = register_sdd_capabilities(&registry);
        assert_eq!(names.len(), 8);

        for name in SDD_CAPABILITIES {
            assert!(
                registry.get(name).is_some(),
                "capability '{}' should be registered",
                name
            );
        }
    }

    #[tokio::test]
    async fn capability_guard_check_all_methods() {
        let guard = setup_guard();
        assert!(guard.check_llm_chat(1).await.is_ok());
        assert!(guard.check_mcp_call("fs_read", 1).await.is_ok());
        assert!(guard.check_fs_write("/tmp/test", 1).await.is_ok());
        assert!(guard.check_session_create(1).await.is_ok());
        assert!(guard.check_session_resume(1).await.is_ok());
        assert!(guard.check_review_submit(1).await.is_ok());
        assert!(guard.check_legacy_analyze(1).await.is_ok());
        assert!(guard.check_cargo_check(1).await.is_ok());
    }

    #[tokio::test]
    async fn capability_guard_permission_denied() {
        use sz_rust_capability::TenantScopeChecker;

        let registry = Arc::new(CapabilityRegistry::new());
        register_sdd_capabilities(&registry);

        let checker = TenantScopeChecker::new();
        checker.grant("sdd.llm_chat", 100);

        let guard = SddCapabilityGuard::with_permission_checker(registry, Arc::new(checker));

        assert!(guard.check_llm_chat(100).await.is_ok());
        let err = guard.check_llm_chat(200).await.unwrap_err();
        assert_eq!(err.code(), Some(&SddErrorCode::SddCapabilityUnauthorized));
    }

    #[tokio::test]
    async fn capability_noop_call_returns_ok() {
        let cap = SddNoOpCapability::new("sdd.test", "test capability");
        let result = cap.call(json!({})).await.unwrap();
        assert_eq!(result["status"], "ok");
    }
}
