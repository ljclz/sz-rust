use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::capability::Capability;
use crate::error::{CapError, CapResult};
use crate::registry::CapabilityRegistry;
use crate::source::CapabilitySource;

pub struct McpCapabilityAdapter {
    tool_name: &'static str,
    cap_name: &'static str,
    description: &'static str,
    input_schema: Value,
    tags: &'static [&'static str],
}

impl McpCapabilityAdapter {
    pub fn new(tool_name: &'static str, input_schema: Value) -> Self {
        Self {
            tool_name,
            cap_name: static_cap_name(tool_name),
            description: static_description(tool_name),
            input_schema,
            tags: static_tags(tool_name),
        }
    }
}

#[async_trait]
impl Capability for McpCapabilityAdapter {
    fn name(&self) -> &'static str {
        self.cap_name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn tags(&self) -> &[&'static str] {
        self.tags
    }

    fn source(&self) -> CapabilitySource {
        CapabilitySource::Service
    }

    async fn call(&self, args: Value) -> CapResult<Value> {
        let result = sz_rust_mcp::call_tool(self.tool_name, &args);
        match result {
            Ok(json_str) => serde_json::from_str(&json_str)
                .map_err(|e| CapError::ExecutionError(format!("MCP 返回值 JSON 解析失败: {e}"))),
            Err(sz_rust_mcp::McpError::ToolNotFound(name)) => Err(CapError::NotFound(name)),
            Err(sz_rust_mcp::McpError::InvalidArguments(msg)) => {
                Err(CapError::ValidationError(msg))
            }
            Err(sz_rust_mcp::McpError::Execution(msg)) => Err(CapError::ExecutionError(msg)),
        }
    }
}

fn static_cap_name(tool_name: &str) -> &'static str {
    match tool_name {
        "parse_path" => "mcp.parse_path",
        "build_select_query" => "mcp.build_select_query",
        "openapi_spec" => "mcp.openapi_spec",
        "redaction_check" => "mcp.redaction_check",
        "url_decode" => "mcp.url_decode",
        "sql_validate" => "mcp.sql_validate",
        "route_conflicts" => "mcp.route_conflicts",
        "build_insert_query" => "mcp.build_insert_query",
        "build_update_query" => "mcp.build_update_query",
        "build_delete_query" => "mcp.build_delete_query",
        "crud_read" => "mcp.crud_read",
        "migrate_create" => "mcp.migrate_create",
        "migrate_status" => "mcp.migrate_status",
        "migrate_run" => "mcp.migrate_run",
        "test_run" => "mcp.test_run",
        "test_coverage" => "mcp.test_coverage",
        "deploy_check" => "mcp.deploy_check",
        "deploy_status" => "mcp.deploy_status",
        "plugin_list" => "mcp.plugin_list",
        "plugin_install" => "mcp.plugin_install",
        "plugin_uninstall" => "mcp.plugin_uninstall",
        _ => "mcp.unknown",
    }
}

fn static_description(tool_name: &str) -> &'static str {
    match tool_name {
        "parse_path" => {
            "解析 URI 为 (app, controller, action) 路由三元组（对齐 PHP auto_multi_app 规则）"
        }
        "build_select_query" => {
            "构建参数化 SELECT 查询（显式列投影 + WHERE 绑定参数，防 SQL 注入）"
        }
        "openapi_spec" => "从路由配置自动生成 OpenAPI 3.0 spec",
        "redaction_check" => {
            "检查配置对象的 Debug 输出是否泄漏敏感字段（merchant_private_key 等应显示 <redacted>）"
        }
        "url_decode" => "URL 百分比解码（支持 UTF-8 多字节，对齐 PHP urldecode）",
        "sql_validate" => "SQL 安全校验（注入防护：语句类型、危险模式、表名列名白名单校验）",
        "route_conflicts" => "路由冲突检测：检查路由规则集合是否存在歧义/冲突",
        "build_insert_query" => "构建参数化 INSERT 查询（防 SQL 注入）",
        "build_update_query" => "构建参数化 UPDATE 查询（WHERE 绑定，防 SQL 注入）",
        "build_delete_query" => "构建参数化 DELETE 查询（WHERE 绑定，防 SQL 注入）",
        "crud_read" => "CRUD 读操作：构建参数化 SELECT 并返回 SQL + 参数数",
        "migrate_create" => "生成迁移脚本模板（UP/DOWN SQL）",
        "migrate_status" => "检查迁移状态：返回已执行/待执行迁移列表",
        "migrate_run" => "生成执行迁移的命令（cargo run -p sz-rust-migration）",
        "test_run" => "生成测试运行命令（cargo test）",
        "test_coverage" => "生成覆盖率分析命令（cargo tarpaulin / cargo llvm-cov）",
        "deploy_check" => "检查部署配置完整性（Docker/K8s 配置校验）",
        "deploy_status" => "生成部署状态查询命令",
        "plugin_list" => "列出已注册的 Capability（按 source/tags 过滤）",
        "plugin_install" => "生成插件安装命令（cargo add + 配置注册）",
        "plugin_uninstall" => "生成插件卸载命令（cargo remove + 清理配置）",
        _ => "",
    }
}

fn static_tags(tool_name: &str) -> &'static [&'static str] {
    match tool_name {
        "parse_path" => &["mcp", "router", "parse", "read"],
        "build_select_query" => &["mcp", "orm", "query", "build"],
        "openapi_spec" => &["mcp", "router", "openapi", "read"],
        "redaction_check" => &["mcp", "security", "redaction", "read"],
        "url_decode" => &["mcp", "http", "decode", "read"],
        "sql_validate" => &["mcp", "orm", "security", "validate"],
        "route_conflicts" => &["mcp", "router", "validate", "read"],
        "build_insert_query" => &["mcp", "orm", "query", "write"],
        "build_update_query" => &["mcp", "orm", "query", "write"],
        "build_delete_query" => &["mcp", "orm", "query", "write"],
        "crud_read" => &["mcp", "orm", "crud", "read"],
        "migrate_create" => &["mcp", "migration", "create", "write"],
        "migrate_status" => &["mcp", "migration", "status", "read"],
        "migrate_run" => &["mcp", "migration", "run", "write"],

        "test_run" => &["mcp", "test", "run", "read"],
        "test_coverage" => &["mcp", "test", "coverage", "read"],
        "deploy_check" => &["mcp", "deploy", "check", "read"],
        "deploy_status" => &["mcp", "deploy", "status", "read"],
        "plugin_list" => &["mcp", "plugin", "list", "read"],
        "plugin_install" => &["mcp", "plugin", "install", "write"],
        "plugin_uninstall" => &["mcp", "plugin", "uninstall", "write"],
        _ => &["mcp"],
    }
}

pub fn register_mcp_tools(registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
    let definitions = sz_rust_mcp::tool_definitions();
    let mut registered = Vec::with_capacity(definitions.len());

    for def in definitions {
        let tool_name = def
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CapError::ExecutionError("MCP 工具定义缺少 name 字段".into()))?;

        let input_schema = def
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let static_name = match tool_name {
            "parse_path" => "parse_path",
            "build_select_query" => "build_select_query",
            "openapi_spec" => "openapi_spec",
            "redaction_check" => "redaction_check",
            "url_decode" => "url_decode",
            "sql_validate" => "sql_validate",
            "route_conflicts" => "route_conflicts",
            "build_insert_query" => "build_insert_query",
            "build_update_query" => "build_update_query",
            "build_delete_query" => "build_delete_query",
            "crud_read" => "crud_read",
            "migrate_create" => "migrate_create",
            "migrate_status" => "migrate_status",
            "migrate_run" => "migrate_run",
            "test_run" => "test_run",
            "test_coverage" => "test_coverage",
            "deploy_check" => "deploy_check",
            "deploy_status" => "deploy_status",
            "plugin_list" => "plugin_list",
            "plugin_install" => "plugin_install",
            "plugin_uninstall" => "plugin_uninstall",
            other => return Err(CapError::ExecutionError(format!("未知 MCP 工具: {other}"))),
        };

        let adapter =
            Arc::new(McpCapabilityAdapter::new(static_name, input_schema)) as Arc<dyn Capability>;
        registry.register(adapter);

        registered.push(format!("mcp.{static_name}"));
    }

    Ok(registered)
}

pub fn register_builtin_skills(
    registry: &CapabilityRegistry,
    skills: Vec<Arc<dyn Capability>>,
) -> CapResult<Vec<String>> {
    let mut registered = Vec::with_capacity(skills.len());
    for skill in skills {
        let name = skill.name().to_string();
        registry.register(skill);
        registered.push(name);
    }
    Ok(registered)
}

/// 扩展 MCP 工具适配器 — 将 `McpTool` trait 适配为 `Capability`。
pub struct ExtendedMcpAdapter {
    tool: Box<dyn sz_rust_mcp::tool::McpTool>,
}

impl ExtendedMcpAdapter {
    pub fn new(tool: Box<dyn sz_rust_mcp::tool::McpTool>) -> Self {
        Self { tool }
    }
}

#[async_trait]
impl Capability for ExtendedMcpAdapter {
    fn name(&self) -> &'static str {
        extended_cap_name(self.tool.name())
    }
    fn description(&self) -> &'static str {
        extended_description(self.tool.name())
    }
    fn schema(&self) -> Value {
        self.tool.input_schema()
    }
    fn tags(&self) -> &[&'static str] {
        extended_tags(self.tool.name())
    }
    fn source(&self) -> CapabilitySource {
        CapabilitySource::Skill
    }
    async fn call(&self, args: Value) -> CapResult<Value> {
        self.tool.execute(args).await.map_err(|e| match e {
            sz_rust_mcp::tool::ToolError::InvalidArgs(msg) => CapError::ValidationError(msg),
            sz_rust_mcp::tool::ToolError::ExecutionFailed(msg) => CapError::ExecutionError(msg),
            sz_rust_mcp::tool::ToolError::PermissionDenied(msg) => CapError::ValidationError(msg),
            sz_rust_mcp::tool::ToolError::ConfirmationRequired => {
                CapError::ValidationError("需要人工确认".into())
            }
            sz_rust_mcp::tool::ToolError::Timeout(msg) => CapError::ExecutionError(msg),
        })
    }
    fn requires_confirmation(&self) -> bool {
        self.tool.requires_confirmation()
    }
}

fn extended_cap_name(tool_name: &str) -> &'static str {
    match tool_name {
        "crud_create" => "mcp.crud_create",
        "crud_read" => "mcp.crud_read",
        "crud_update" => "mcp.crud_update",
        "crud_delete" => "mcp.crud_delete",
        "migrate_create" => "mcp.migrate_create",
        "migrate_run" => "mcp.migrate_run",
        "test_run" => "mcp.test_run",
        "deploy_run" => "mcp.deploy_run",
        "plugin_install" => "mcp.plugin_install",
        "plugin_uninstall" => "mcp.plugin_uninstall",
        _ => "mcp.unknown",
    }
}

fn extended_description(tool_name: &str) -> &'static str {
    match tool_name {
        "crud_create" => "通过 CapabilityRegistry 创建资源",
        "crud_read" => "通过 CapabilityRegistry 查询资源",
        "crud_update" => "通过 CapabilityRegistry 更新资源",
        "crud_delete" => "通过 CapabilityRegistry 删除资源（需要确认）",
        "migrate_create" => "生成迁移脚本模板（UP/DOWN SQL），使用 tokio::fs 写文件",
        "migrate_run" => "执行迁移（cargo run -p sz-rust-migration）",
        "test_run" => "异步执行 cargo test，返回 passed/failed/skipped 数量",
        "deploy_run" => "通过 Node.js ssh2 包执行远程部署（需要确认）",
        "plugin_install" => "从插件市场安装插件（cargo add + 注册到 CapabilityRegistry）",
        "plugin_uninstall" => "卸载插件（cargo remove + 清理注册，需要确认）",
        _ => "",
    }
}

fn extended_tags(tool_name: &str) -> &'static [&'static str] {
    match tool_name {
        "crud_create" => &["mcp", "crud", "create", "write"],
        "crud_read" => &["mcp", "crud", "read", "read"],
        "crud_update" => &["mcp", "crud", "update", "write"],
        "crud_delete" => &["mcp", "crud", "delete", "write"],
        "migrate_create" => &["mcp", "migration", "create", "write"],
        "migrate_run" => &["mcp", "migration", "run", "write"],
        "test_run" => &["mcp", "test", "run", "read"],
        "deploy_run" => &["mcp", "deploy", "run", "write"],
        "plugin_install" => &["mcp", "plugin", "install", "write"],
        "plugin_uninstall" => &["mcp", "plugin", "uninstall", "write"],
        _ => &["mcp"],
    }
}

/// 将扩展 MCP 工具（基于 McpTool trait）注册到 CapabilityRegistry。
pub fn register_extended_mcp_tools(registry: &CapabilityRegistry) -> CapResult<Vec<String>> {
    let tools = sz_rust_mcp::extended_tools();
    let mut registered = Vec::with_capacity(tools.len());

    for tool in tools {
        let cap_name = extended_cap_name(tool.name()).to_string();
        let adapter = Arc::new(ExtendedMcpAdapter::new(tool)) as Arc<dyn Capability>;
        registry.register(adapter);
        registered.push(cap_name);
    }

    Ok(registered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_register_mcp_tools() {
        let registry = CapabilityRegistry::new();
        let result = register_mcp_tools(&registry);
        assert!(result.is_ok());
        let names = result.unwrap();
        assert_eq!(names.len(), 21);
        assert!(names.contains(&"mcp.parse_path".to_string()));
        assert!(names.contains(&"mcp.sql_validate".to_string()));
        assert!(names.contains(&"mcp.build_insert_query".to_string()));
        assert!(names.contains(&"mcp.plugin_list".to_string()));
        assert_eq!(registry.list_by_source(CapabilitySource::Service).len(), 21);
    }

    #[test]
    fn test_mcp_adapter_tags() {
        let registry = CapabilityRegistry::new();
        register_mcp_tools(&registry).unwrap();
        let caps = registry.find_by_tags(&["mcp", "router"], None);
        assert_eq!(caps.len(), 3);
        let caps = registry.find_by_tags(&["mcp", "orm"], None);
        assert_eq!(caps.len(), 6);
        let caps = registry.find_by_tags(&["mcp", "security"], None);
        assert_eq!(caps.len(), 2);
        let caps = registry.find_by_tags(&["mcp", "migration"], None);
        assert_eq!(caps.len(), 3);
        let caps = registry.find_by_tags(&["mcp", "plugin"], None);
        assert_eq!(caps.len(), 3);
    }

    #[tokio::test]
    async fn test_mcp_adapter_call_url_decode() {
        let registry = CapabilityRegistry::new();
        register_mcp_tools(&registry).unwrap();
        let result = registry
            .call("mcp.url_decode", json!({"value": "%E4%BD%A0%E5%A5%BD"}))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mcp_adapter_call_not_found() {
        let registry = CapabilityRegistry::new();
        register_mcp_tools(&registry).unwrap();
        let result = registry.call("mcp.nonexistent", json!({})).await;
        assert!(matches!(result, Err(CapError::NotFound(_))));
    }

    #[test]
    fn test_register_builtin_skills() {
        struct DummySkill;
        #[async_trait]
        impl Capability for DummySkill {
            fn name(&self) -> &'static str {
                "dummy_skill"
            }
            fn description(&self) -> &'static str {
                "测试 Skill"
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
            async fn call(&self, _args: Value) -> CapResult<Value> {
                Ok(json!({}))
            }
        }

        let registry = CapabilityRegistry::new();
        let skills = vec![Arc::new(DummySkill) as Arc<dyn Capability>];
        let result = register_builtin_skills(&registry, skills);
        assert!(result.is_ok());
        let names = result.unwrap();
        assert_eq!(names, vec!["dummy_skill"]);
        assert_eq!(registry.list_by_source(CapabilitySource::Skill).len(), 1);
    }

    #[test]
    fn test_register_extended_mcp_tools() {
        let registry = CapabilityRegistry::new();
        let result = register_extended_mcp_tools(&registry);
        assert!(result.is_ok());
        let names = result.unwrap();
        assert_eq!(names.len(), 10, "应注册 10 个扩展工具");
        assert!(names.contains(&"mcp.crud_create".to_string()));
        assert!(names.contains(&"mcp.crud_read".to_string()));
        assert!(names.contains(&"mcp.crud_update".to_string()));
        assert!(names.contains(&"mcp.crud_delete".to_string()));
        assert!(names.contains(&"mcp.migrate_create".to_string()));
        assert!(names.contains(&"mcp.migrate_run".to_string()));
        assert!(names.contains(&"mcp.test_run".to_string()));
        assert!(names.contains(&"mcp.deploy_run".to_string()));
        assert!(names.contains(&"mcp.plugin_install".to_string()));
        assert!(names.contains(&"mcp.plugin_uninstall".to_string()));
        assert_eq!(registry.list_by_source(CapabilitySource::Skill).len(), 10);
    }

    #[test]
    fn test_extended_mcp_tags() {
        let registry = CapabilityRegistry::new();
        register_extended_mcp_tools(&registry).unwrap();
        let caps = registry.find_by_tags(&["mcp", "crud"], None);
        assert_eq!(caps.len(), 4, "应有 4 个 CRUD 工具");
        let caps = registry.find_by_tags(&["mcp", "migration"], None);
        assert_eq!(caps.len(), 2, "应有 2 个迁移工具");
        let caps = registry.find_by_tags(&["mcp", "deploy"], None);
        assert_eq!(caps.len(), 1, "应有 1 个部署工具");
        let caps = registry.find_by_tags(&["mcp", "plugin"], None);
        assert_eq!(caps.len(), 2, "应有 2 个插件工具");
    }

    #[test]
    fn test_extended_mcp_confirmation() {
        let registry = CapabilityRegistry::new();
        register_extended_mcp_tools(&registry).unwrap();
        let delete_cap = registry.get("mcp.crud_delete").unwrap();
        assert!(delete_cap.requires_confirmation());
        let deploy_cap = registry.get("mcp.deploy_run").unwrap();
        assert!(deploy_cap.requires_confirmation());
        let uninstall_cap = registry.get("mcp.plugin_uninstall").unwrap();
        assert!(uninstall_cap.requires_confirmation());
        let create_cap = registry.get("mcp.crud_create").unwrap();
        assert!(!create_cap.requires_confirmation());
    }

    #[tokio::test]
    async fn test_extended_mcp_call_crud_create() {
        let registry = CapabilityRegistry::new();
        register_extended_mcp_tools(&registry).unwrap();
        let result = registry
            .call(
                "mcp.crud_create",
                json!({"capability": "test_cap", "data": {}, "tenant_id": 1}),
            )
            .await;
        assert!(result.is_ok());
        let out = result.unwrap();
        assert_eq!(out["status"], "created");
    }
}
