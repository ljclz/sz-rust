//! Capability Registry 端到端测试
//!
//! 验证幻影交付修复 2：Capability Registry 已接入生产。
//! 测试内容：
//! 1. MCP 工具可注册为 Capability
//! 2. find_by_tags / list_all 查询正常
//! 3. call 调用链路完整（参数校验 → 能力调用）

use std::sync::Arc;

use serde_json::json;
use sz_rust_capability::builtin::McpCapabilityAdapter;
use sz_rust_capability::CapabilityRegistry;

/// 注册 5 个 MCP 工具能力后，list_all 返回 5 条
#[test]
fn test_register_mcp_tools_and_list_all() {
    let registry = CapabilityRegistry::new();
    let tools: &[(&str, serde_json::Value)] = &[
        ("parse_path", json!({"path": "string"})),
        ("build_select_query", json!({"table": "string"})),
        ("sql_validate", json!({"sql": "string"})),
        ("redaction_check", json!({"text": "string"})),
        ("crud_read", json!({"table": "string", "id": "integer"})),
    ];
    for (name, schema) in tools {
        let cap = Arc::new(McpCapabilityAdapter::new(name, schema.clone()));
        registry.register(cap);
    }
    let all = registry.list_all();
    assert_eq!(all.len(), 5, "应注册 5 个 MCP 工具能力");
}

/// find_by_tags 按 tag 过滤能力
#[test]
fn test_find_by_tags_filters_correctly() {
    let registry = CapabilityRegistry::new();
    let cap = Arc::new(McpCapabilityAdapter::new("parse_path", json!({})));
    registry.register(cap);
    let found = registry.find_by_tags(&["mcp"], None);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name(), "mcp.parse_path");
    let not_found = registry.find_by_tags(&["nonexistent_tag"], None);
    assert_eq!(not_found.len(), 0);
}

/// get 按名称查询单个能力
#[test]
fn test_get_by_name_returns_capability() {
    let registry = CapabilityRegistry::new();
    let cap = Arc::new(McpCapabilityAdapter::new("sql_validate", json!({})));
    registry.register(cap);
    let found = registry.get("mcp.sql_validate");
    assert!(found.is_some());
    assert_eq!(found.unwrap().name(), "mcp.sql_validate");
    assert!(registry.get("mcp.nonexistent").is_none());
}

/// call 调用 MCP 工具能力，返回 JSON 结果
#[tokio::test]
async fn test_call_mcp_capability_executes() {
    let registry = CapabilityRegistry::new();
    let cap = Arc::new(McpCapabilityAdapter::new("parse_path", json!({})));
    registry.register(cap);
    let args = json!({"uri": "/api/v1/product/list"});
    let result = registry.call("mcp.parse_path", args).await;
    assert!(result.is_ok(), "MCP parse_path 调用应成功");
    let resp: serde_json::Value = result.unwrap();
    assert!(resp.is_object(), "返回值应为 JSON 对象");
    assert!(resp.get("app").is_some(), "返回值应包含 app 字段");
}

/// unregister 移除已注册能力
#[test]
fn test_unregister_removes_capability() {
    let registry = CapabilityRegistry::new();
    let cap = Arc::new(McpCapabilityAdapter::new("crud_read", json!({})));
    registry.register(cap);
    assert_eq!(registry.list_all().len(), 1);
    let removed = registry.unregister("mcp.crud_read");
    assert!(removed.is_some());
    assert_eq!(registry.list_all().len(), 0);
}
