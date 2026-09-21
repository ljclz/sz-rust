// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
use crate::common::AiError;
use async_trait::async_trait;
use std::collections::HashMap;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    fn schema(&self) -> serde_json::Value;

    async fn call(&self, args: &serde_json::Value) -> Result<serde_json::Value, AiError>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub async fn call(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| AiError::ToolNotAuthorized(name.to_string()))?;
        tool.call(args).await
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn call(&self, args: &serde_json::Value) -> Result<serde_json::Value, AiError> {
            Ok(args.clone())
        }
    }

    struct ErrorTool;
    #[async_trait]
    impl Tool for ErrorTool {
        fn name(&self) -> &str {
            "error_tool"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::Value::Null
        }
        async fn call(&self, _args: &serde_json::Value) -> Result<serde_json::Value, AiError> {
            Err(AiError::ToolExecution("always fails".into()))
        }
    }

    #[tokio::test]
    async fn register_and_call_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let result = registry
            .call("echo", &serde_json::json!({"msg": "hi"}))
            .await
            .unwrap();
        assert_eq!(result["msg"], "hi");
    }

    #[tokio::test]
    async fn call_unknown_tool_errors() {
        let registry = ToolRegistry::new();
        let err = registry
            .call("nonexistent", &serde_json::Value::Null)
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "AI_TOOL_NOT_AUTHORIZED");
    }

    #[tokio::test]
    async fn call_failing_tool_propagates_error() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ErrorTool));
        let err = registry
            .call("error_tool", &serde_json::Value::Null)
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "AI_TOOL_EXECUTION");
    }

    #[test]
    fn list_returns_all_registered() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry.register(Box::new(ErrorTool));
        let names = registry.list();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"echo".to_string()));
        assert!(names.contains(&"error_tool".to_string()));
    }

    #[test]
    fn get_returns_tool_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        assert!(registry.get("echo").is_some());
        assert!(registry.get("unknown").is_none());
    }
}
