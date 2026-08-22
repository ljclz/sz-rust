use crate::agent::tool::Tool;
use crate::common::AiError;
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;

pub struct McpToolBridge {
    allowed_tools: HashSet<String>,
}

impl McpToolBridge {
    pub fn new(allowed_tools: &[&str]) -> Self {
        Self {
            allowed_tools: allowed_tools.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn list_tools(&self) -> Vec<String> {
        self.allowed_tools.iter().cloned().collect()
    }

    pub async fn call(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, AiError> {
        if !self.allowed_tools.contains(name) {
            return Err(AiError::ToolNotAuthorized(name.to_string()));
        }
        let result = sz_rust_mcp::call_tool(name, args)
            .map_err(|e| AiError::ToolExecution(e.to_string()))?;
        serde_json::from_str(&result)
            .map_err(|e| AiError::ToolExecution(format!("mcp result parse error: {}", e)))
    }

    pub fn adapters(&self) -> Vec<McpToolAdapter> {
        self.allowed_tools
            .iter()
            .map(|name| McpToolAdapter {
                name: name.clone(),
                bridge: Arc::new(McpToolBridge {
                    allowed_tools: HashSet::from([name.clone()]),
                }),
            })
            .collect()
    }
}

pub struct McpToolAdapter {
    name: String,
    bridge: Arc<McpToolBridge>,
}

impl McpToolAdapter {
    pub fn new(name: impl Into<String>, bridge: Arc<McpToolBridge>) -> Self {
        Self {
            name: name.into(),
            bridge,
        }
    }
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn schema(&self) -> serde_json::Value {
        let defs = sz_rust_mcp::tool_definitions();
        for def in &defs {
            if def.get("name").and_then(|v| v.as_str()) == Some(&self.name) {
                return def
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
            }
        }
        serde_json::Value::Null
    }

    async fn call(&self, args: &serde_json::Value) -> Result<serde_json::Value, AiError> {
        self.bridge.call(&self.name, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_whitelist() {
        let bridge = McpToolBridge::new(&["parse_path", "build_select_query"]);
        let tools = bridge.list_tools();
        assert_eq!(tools.len(), 2);
    }

    #[test]
    fn list_tools_returns_all_allowed() {
        let bridge = McpToolBridge::new(&["a", "b", "c"]);
        let tools = bridge.list_tools();
        assert_eq!(tools.len(), 3);
    }

    #[tokio::test]
    async fn call_unauthorized_tool_errors() {
        let bridge = McpToolBridge::new(&["parse_path"]);
        let err = bridge
            .call("not_allowed", &serde_json::Value::Null)
            .await
            .unwrap_err();
        assert_eq!(err.error_code(), "AI_TOOL_NOT_AUTHORIZED");
    }

    #[test]
    fn adapters_returns_one_per_tool() {
        let bridge = McpToolBridge::new(&["parse_path", "build_select_query"]);
        let adapters = bridge.adapters();
        assert_eq!(adapters.len(), 2);
    }

    #[test]
    fn adapter_name_matches() {
        let bridge = McpToolBridge::new(&["parse_path"]);
        let adapters = bridge.adapters();
        assert_eq!(adapters[0].name(), "parse_path");
    }

    #[tokio::test]
    async fn adapter_call_unauthorized_errors() {
        let bridge = McpToolBridge::new(&["parse_path"]);
        let adapter = McpToolAdapter::new("other_tool", Arc::new(bridge));
        let err = adapter.call(&serde_json::Value::Null).await.unwrap_err();
        assert_eq!(err.error_code(), "AI_TOOL_NOT_AUTHORIZED");
    }
}
