//! P1-4 MCP 工具扩展测试。
//!
//! 覆盖：
//! 1. 10 个扩展工具的 name/description/schema
//! 2. McpDelete/McpDeployRun/McpPluginUninstall 的 requires_confirmation = true
//! 3. 白名单鉴权（白名单外返回 PermissionDenied，敏感操作返回 ConfirmationRequired）
//! 4. extended_tools() 返回 10 个工具

use sz_rust_mcp::tool::McpTool;
use sz_rust_mcp::tools::crud::{McpCreate, McpDelete, McpRead, McpUpdate};
use sz_rust_mcp::tools::deploy::McpDeployRun;
use sz_rust_mcp::tools::migrate::{McpMigrateCreate, McpMigrateRun};
use sz_rust_mcp::tools::plugin_tool::{McpPluginInstall, McpPluginUninstall};
use sz_rust_mcp::tools::test_tool::McpTestRun;
use sz_rust_mcp::whitelist::ToolWhitelist;

#[test]
fn test_extended_tools_count() {
    let tools = sz_rust_mcp::extended_tools();
    assert_eq!(tools.len(), 10, "应有 10 个扩展工具");
}

#[test]
fn test_crud_tool_names() {
    assert_eq!(McpCreate.name(), "crud_create");
    assert_eq!(McpRead.name(), "crud_read");
    assert_eq!(McpUpdate.name(), "crud_update");
    assert_eq!(McpDelete.name(), "crud_delete");
}

#[test]
fn test_migrate_tool_names() {
    assert_eq!(McpMigrateCreate.name(), "migrate_create");
    assert_eq!(McpMigrateRun.name(), "migrate_run");
}

#[test]
fn test_test_tool_name() {
    assert_eq!(McpTestRun.name(), "test_run");
}

#[test]
fn test_deploy_tool_name() {
    assert_eq!(McpDeployRun.name(), "deploy_run");
}

#[test]
fn test_plugin_tool_names() {
    assert_eq!(McpPluginInstall.name(), "plugin_install");
    assert_eq!(McpPluginUninstall.name(), "plugin_uninstall");
}

#[test]
fn test_crud_delete_requires_confirmation() {
    assert!(McpDelete.requires_confirmation(), "McpDelete 应需要确认");
}

#[test]
fn test_deploy_run_requires_confirmation() {
    assert!(
        McpDeployRun.requires_confirmation(),
        "McpDeployRun 应需要确认"
    );
}

#[test]
fn test_plugin_uninstall_requires_confirmation() {
    assert!(
        McpPluginUninstall.requires_confirmation(),
        "McpPluginUninstall 应需要确认"
    );
}

#[test]
fn test_non_sensitive_tools_no_confirmation() {
    assert!(!McpCreate.requires_confirmation());
    assert!(!McpRead.requires_confirmation());
    assert!(!McpUpdate.requires_confirmation());
    assert!(!McpMigrateCreate.requires_confirmation());
    assert!(!McpMigrateRun.requires_confirmation());
    assert!(!McpTestRun.requires_confirmation());
    assert!(!McpPluginInstall.requires_confirmation());
}

#[test]
fn test_all_tools_have_input_schema() {
    let tools = sz_rust_mcp::extended_tools();
    for tool in &tools {
        let schema = tool.input_schema();
        assert!(schema.is_object(), "{} 应有 input_schema", tool.name());
        assert!(
            schema.get("type").is_some(),
            "{} 的 input_schema 应有 type 字段",
            tool.name()
        );
    }
}

#[test]
fn test_whitelist_allow_all() {
    let wl = ToolWhitelist::allow_all();
    assert!(wl.check("crud_create").is_ok());
    assert!(wl.check("crud_read").is_ok());
    assert!(wl.check("any_tool").is_ok());
}

#[test]
fn test_whitelist_rejects_unlisted() {
    let wl = ToolWhitelist::new().allow("crud_read").allow("crud_create");
    assert!(wl.check("crud_read").is_ok());
    assert!(wl.check("crud_create").is_ok());
    let err = wl.check("crud_delete").unwrap_err();
    assert!(
        matches!(err, sz_rust_mcp::tool::ToolError::PermissionDenied(_)),
        "白名单外工具应返回 PermissionDenied"
    );
}

#[test]
fn test_whitelist_sensitive_returns_confirmation_required() {
    let wl = ToolWhitelist::allow_all()
        .mark_sensitive("crud_delete")
        .mark_sensitive("deploy_run")
        .mark_sensitive("plugin_uninstall");
    let err = wl.check("crud_delete").unwrap_err();
    assert!(
        matches!(err, sz_rust_mcp::tool::ToolError::ConfirmationRequired),
        "敏感操作应返回 ConfirmationRequired"
    );
    let err = wl.check("deploy_run").unwrap_err();
    assert!(matches!(
        err,
        sz_rust_mcp::tool::ToolError::ConfirmationRequired
    ));
    let err = wl.check("plugin_uninstall").unwrap_err();
    assert!(matches!(
        err,
        sz_rust_mcp::tool::ToolError::ConfirmationRequired
    ));
}

#[test]
fn test_whitelist_default() {
    let wl = ToolWhitelist::default();
    assert!(wl.check("crud_read").is_ok());
    assert!(wl.is_sensitive("crud_delete"));
    assert!(wl.is_sensitive("deploy_run"));
    assert!(wl.is_sensitive("plugin_uninstall"));
    assert!(!wl.is_sensitive("crud_create"));
}

#[tokio::test]
async fn test_mcp_create_execute() {
    let tool = McpCreate;
    let result = tool
        .execute(serde_json::json!({"capability": "test_cap", "data": {}, "tenant_id": 1}))
        .await;
    assert!(result.is_ok());
    let out = result.unwrap();
    assert_eq!(out["status"], "created");
    assert_eq!(out["capability"], "test_cap");
}

#[tokio::test]
async fn test_mcp_read_execute() {
    let tool = McpRead;
    let result = tool
        .execute(serde_json::json!({"capability": "test_cap", "tenant_id": 1}))
        .await;
    assert!(result.is_ok());
    let out = result.unwrap();
    assert_eq!(out["status"], "ok");
}

#[tokio::test]
async fn test_mcp_migrate_create_execute() {
    let tool = McpMigrateCreate;
    let result = tool
        .execute(serde_json::json!({"name": "test_migration", "description": "test", "output_dir": "test_migrations_tmp"}))
        .await;
    assert!(result.is_ok(), "migrate_create 应成功: {:?}", result);
    let out = result.unwrap();
    assert_eq!(out["status"], "created");

    let _ = tokio::fs::remove_dir_all("test_migrations_tmp").await;
}

#[test]
fn test_tool_info_from_tool() {
    use sz_rust_mcp::tool::ToolInfo;
    let tool = McpDelete;
    let info = ToolInfo::from_tool(&tool);
    assert_eq!(info.name, "crud_delete");
    assert!(info.requires_confirmation);
}
