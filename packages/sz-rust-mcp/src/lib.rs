//! sz-rust-mcp — SZ-Rust 框架能力 MCP server（4.3 竞争力深化：MCP 工具集成）
//!
//! 将 sz-rust 框架能力封装为 [Model Context Protocol](https://modelcontextprotocol.io)
//! tools，供 AI Agent 通过 stdio transport 调用。
//!
//! ## 支持的工具
//!
//! | 工具 | 输入 | 输出 | 来源 |
//! |------|------|------|------|
//! | `parse_path` | `uri` | 路由三元组 (app/controller/action) | router-facade |
//! | `build_select_query` | `table` / `columns` / `where_eq` | 参数化 SQL + 绑定参数 | orm-facade |
//! | `openapi_spec` | `routes` (JSON) | OpenAPI 3.0 spec | router-facade |
//! | `redaction_check` | `config` (JSON) | Debug 输出脱敏检查结果 | pay-facade |
//! | `url_decode` | `value` | URL 解码结果 | http-facade |
//! | `sql_validate` | `sql` | SQL 注入防护校验 | orm-facade (sql-validator) |
//! | `route_conflicts` | `routes` (JSON) | 路由冲突检测 | router-facade |
//!
//! ## 协议
//!
//! JSON-RPC 2.0 over stdio（MCP 标准 transport）：
//! - `initialize` → server 能力声明
//! - `tools/list` → 工具列表（name/description/inputSchema）
//! - `tools/call` → 工具调用（params: {name, arguments}）
//!
//! ## 运行
//!
//! ```bash
//! cargo run -p sz-rust-mcp
//! echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"parse_path","arguments":{"uri":"/oapc/customer/index"}}}' | cargo run -p sz-rust-mcp
//! ```

use serde_json::{json, Value};

pub mod tool;
pub mod tools;
pub mod whitelist;

/// 返回所有扩展 MCP 工具（基于 McpTool trait 的新工具）。
pub fn extended_tools() -> Vec<Box<dyn tool::McpTool>> {
    vec![
        Box::new(tools::crud::McpCreate),
        Box::new(tools::crud::McpRead),
        Box::new(tools::crud::McpUpdate),
        Box::new(tools::crud::McpDelete),
        Box::new(tools::migrate::McpMigrateCreate),
        Box::new(tools::migrate::McpMigrateRun),
        Box::new(tools::test_tool::McpTestRun),
        Box::new(tools::deploy::McpDeployRun),
        Box::new(tools::plugin_tool::McpPluginInstall),
        Box::new(tools::plugin_tool::McpPluginUninstall),
    ]
}

/// 工具调用错误
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// 工具不存在
    #[error("tool not found: {0}")]
    ToolNotFound(String),
    /// 参数解析失败
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    /// 内部执行错误
    #[error("execution failed: {0}")]
    Execution(String),
}

/// 校验 SQL 标识符（表名 / 列名），防止 SQL 注入。
///
/// 规则：
/// - 非空，长度 ≤ 64（MySQL 标识符上限）
/// - 仅允许字母、数字、下划线
/// - 必须以字母或下划线开头（不能以数字开头）
fn validate_identifier(name: &str) -> Result<(), McpError> {
    if name.is_empty() {
        return Err(McpError::InvalidArguments("标识符不能为空".into()));
    }
    if name.len() > 64 {
        return Err(McpError::InvalidArguments(format!(
            "标识符过长（>64）：{name}"
        )));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(McpError::InvalidArguments(format!(
            "标识符必须以字母或下划线开头：{name}"
        )));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(McpError::InvalidArguments(format!(
            "标识符含非法字符（仅允许字母/数字/下划线）：{name}"
        )));
    }
    Ok(())
}

/// 处理一条 JSON-RPC 请求，返回响应 JSON
pub fn handle_request(raw: &str) -> Value {
    let req: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {"code": -32700, "message": format!("parse error: {e}")},
            })
        }
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

    match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "sz-rust-mcp", "version": env!("CARGO_PKG_VERSION")},
            }
        }),
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"tools": tool_definitions()}
        }),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(name, &args) {
                Ok(content) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"content": [{"type": "text", "text": content}]}
                }),
                Err(e) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32603, "message": e.to_string()}
                }),
            }
        }
        "notifications/initialized" => {
            // 通知类请求无需响应
            Value::Null
        }
        _ => {
            if id.is_null() {
                // 通知类请求无需响应
                Value::Null
            } else {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32601, "message": format!("method not found: {method}")}
                })
            }
        }
    }
}

/// 工具定义列表（MCP tools/list 返回）
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "parse_path",
            "description": "解析 URI 为 (app, controller, action) 路由三元组（对齐 PHP auto_multi_app 规则）",
            "inputSchema": {"type": "object", "properties": {"uri": {"type": "string"}}, "required": ["uri"]}
        }),
        json!({
            "name": "build_select_query",
            "description": "构建参数化 SELECT 查询（显式列投影 + WHERE 绑定参数，防 SQL 注入）",
            "inputSchema": {"type": "object", "properties": {
                "table": {"type": "string"}, "columns": {"type": "array", "items": {"type": "string"}},
                "where_eq": {"type": "object"}
            }, "required": ["table", "columns"]}
        }),
        json!({
            "name": "openapi_spec",
            "description": "从路由配置自动生成 OpenAPI 3.0 spec",
            "inputSchema": {"type": "object", "properties": {
                "routes": {"type": "array", "items": {"type": "object"}},
                "title": {"type": "string"}, "version": {"type": "string"}
            }, "required": ["routes"]}
        }),
        json!({
            "name": "redaction_check",
            "description": "检查配置对象的 Debug 输出是否泄漏敏感字段（merchant_private_key 等应显示 <redacted>）",
            "inputSchema": {"type": "object", "properties": {"config": {"type": "object"}}, "required": ["config"]}
        }),
        json!({
            "name": "url_decode",
            "description": "URL 百分比解码（支持 UTF-8 多字节，对齐 PHP urldecode）",
            "inputSchema": {"type": "object", "properties": {"value": {"type": "string"}}, "required": ["value"]}
        }),
        json!({
            "name": "sql_validate",
            "description": "SQL 安全校验（注入防护：语句类型、危险模式、表名列名白名单校验）",
            "inputSchema": {"type": "object", "properties": {"sql": {"type": "string"}}, "required": ["sql"]}
        }),
        json!({
            "name": "route_conflicts",
            "description": "路由冲突检测：检查路由规则集合是否存在歧义/冲突",
            "inputSchema": {"type": "object", "properties": {
                "routes": {"type": "array", "items": {"type": "object"}}
            }, "required": ["routes"]}
        }),
        json!({
            "name": "build_insert_query",
            "description": "构建参数化 INSERT 查询（防 SQL 注入）",
            "inputSchema": {"type": "object", "properties": {
                "table": {"type": "string"}, "data": {"type": "object"}
            }, "required": ["table", "data"]}
        }),
        json!({
            "name": "build_update_query",
            "description": "构建参数化 UPDATE 查询（WHERE 绑定，防 SQL 注入）",
            "inputSchema": {"type": "object", "properties": {
                "table": {"type": "string"}, "set": {"type": "object"}, "where_eq": {"type": "object"}
            }, "required": ["table", "set"]}
        }),
        json!({
            "name": "build_delete_query",
            "description": "构建参数化 DELETE 查询（WHERE 绑定，防 SQL 注入）",
            "inputSchema": {"type": "object", "properties": {
                "table": {"type": "string"}, "where_eq": {"type": "object"}
            }, "required": ["table", "where_eq"]}
        }),
        json!({
            "name": "crud_read",
            "description": "CRUD 读操作：构建参数化 SELECT 并返回 SQL + 参数数",
            "inputSchema": {"type": "object", "properties": {
                "table": {"type": "string"}, "columns": {"type": "array", "items": {"type": "string"}},
                "where_eq": {"type": "object"}, "limit": {"type": "integer"}
            }, "required": ["table", "columns"]}
        }),
        json!({
            "name": "migrate_create",
            "description": "生成迁移脚本模板（UP/DOWN SQL）",
            "inputSchema": {"type": "object", "properties": {
                "name": {"type": "string"}, "description": {"type": "string"}
            }, "required": ["name"]}
        }),
        json!({
            "name": "migrate_status",
            "description": "检查迁移状态：返回已执行/待执行迁移列表",
            "inputSchema": {"type": "object", "properties": {
                "migrations_dir": {"type": "string"}
            }}
        }),
        json!({
            "name": "migrate_run",
            "description": "生成执行迁移的命令（cargo run -p sz-rust-migration）",
            "inputSchema": {"type": "object", "properties": {
                "direction": {"type": "string", "enum": ["up", "down"]}, "steps": {"type": "integer"}
            }}
        }),
        json!({
            "name": "test_run",
            "description": "生成测试运行命令（cargo test）",
            "inputSchema": {"type": "object", "properties": {
                "package": {"type": "string"}, "test_name": {"type": "string"}, "flags": {"type": "string"}
            }}
        }),
        json!({
            "name": "test_coverage",
            "description": "生成覆盖率分析命令（cargo tarpaulin / cargo llvm-cov）",
            "inputSchema": {"type": "object", "properties": {
                "package": {"type": "string"}, "output_format": {"type": "string"}
            }}
        }),
        json!({
            "name": "deploy_check",
            "description": "检查部署配置完整性（Docker/K8s 配置校验）",
            "inputSchema": {"type": "object", "properties": {
                "config_path": {"type": "string"}, "target": {"type": "string", "enum": ["docker", "k8s"]}
            }, "required": ["config_path"]}
        }),
        json!({
            "name": "deploy_status",
            "description": "生成部署状态查询命令",
            "inputSchema": {"type": "object", "properties": {
                "target": {"type": "string", "enum": ["docker", "k8s", "bare_metal"]}
            }}
        }),
        json!({
            "name": "plugin_list",
            "description": "列出已注册的 Capability（按 source/tags 过滤）",
            "inputSchema": {"type": "object", "properties": {
                "source": {"type": "string", "enum": ["skill", "plugin", "service"]},
                "tags": {"type": "array", "items": {"type": "string"}}
            }}
        }),
        json!({
            "name": "plugin_install",
            "description": "生成插件安装命令（cargo add + 配置注册）",
            "inputSchema": {"type": "object", "properties": {
                "plugin_name": {"type": "string"}, "version": {"type": "string"}
            }, "required": ["plugin_name"]}
        }),
        json!({
            "name": "plugin_uninstall",
            "description": "生成插件卸载命令（cargo remove + 清理配置）",
            "inputSchema": {"type": "object", "properties": {
                "plugin_name": {"type": "string"}
            }, "required": ["plugin_name"]}
        }),
    ]
}

/// 将 serde_json::Value 转换为 sz-orm Value（WHERE 绑定参数）
fn json_to_orm_value(v: &Value) -> sz_rust_orm_facade::Value {
    match v {
        Value::Null => sz_rust_orm_facade::Value::Null,
        Value::Bool(b) => sz_rust_orm_facade::Value::Bool(*b),
        Value::Number(n) => n
            .as_i64()
            .map(sz_rust_orm_facade::Value::I64)
            .or_else(|| n.as_f64().map(sz_rust_orm_facade::Value::F64))
            .unwrap_or(sz_rust_orm_facade::Value::Null),
        Value::String(s) => sz_rust_orm_facade::Value::String(s.clone()),
        Value::Array(_) | Value::Object(_) => sz_rust_orm_facade::Value::String(v.to_string()),
    }
}

/// 执行工具调用，返回文本结果
pub fn call_tool(name: &str, args: &Value) -> Result<String, McpError> {
    match name {
        "parse_path" => {
            let uri = args
                .get("uri")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("uri 必填".into()))?;
            let parsed = sz_rust_router_facade::router::parse_path(uri);
            Ok(json!({"app": parsed.app, "controller": parsed.controller, "action": parsed.action}).to_string())
        }
        "build_select_query" => {
            let table = args
                .get("table")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("table 必填".into()))?;
            let columns: Vec<&str> = args
                .get("columns")
                .and_then(|v| v.as_array())
                .ok_or_else(|| McpError::InvalidArguments("columns 必填".into()))?
                .iter()
                .filter_map(|c| c.as_str())
                .collect();
            if columns.is_empty() {
                return Err(McpError::InvalidArguments(
                    "columns 不能为空（禁止 SELECT *）".into(),
                ));
            }
            validate_identifier(table)?;
            for col in &columns {
                validate_identifier(col)?;
            }
            let mut q = sz_rust_orm_facade::SelectQuery::new()
                .columns(&columns)
                .from(table);
            // where_eq: {"id": 1, "status": "active"}
            if let Some(where_map) = args.get("where_eq").and_then(|v| v.as_object()) {
                for (col, val) in where_map {
                    validate_identifier(col)?;
                    q = q.where_eq(col.as_str(), json_to_orm_value(val));
                }
            }
            let built = q.build_with_params(sz_rust_orm_facade::DbType::MySQL);
            Ok(json!({"sql": built.sql, "params": built.params.len()}).to_string())
        }
        "openapi_spec" => {
            let routes = args
                .get("routes")
                .and_then(|v| v.as_array())
                .ok_or_else(|| McpError::InvalidArguments("routes 必填".into()))?;
            let mut config = sz_rust_router_facade::routing::RouteConfig::new();
            for r in routes {
                let method = r.get("method").and_then(|m| m.as_str()).unwrap_or("GET");
                let path = r.get("path").and_then(|p| p.as_str()).unwrap_or("/");
                let handler = r
                    .get("handler")
                    .and_then(|h| h.as_str())
                    .unwrap_or("Index@index");
                let http_method = match method {
                    "POST" => sz_rust_router_facade::routing::HttpMethod::POST,
                    "PUT" => sz_rust_router_facade::routing::HttpMethod::PUT,
                    "DELETE" => sz_rust_router_facade::routing::HttpMethod::DELETE,
                    _ => sz_rust_router_facade::routing::HttpMethod::GET,
                };
                config.add_route(sz_rust_router_facade::routing::RouteRule::new(
                    http_method,
                    path,
                    handler,
                ));
            }
            let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("API");
            let version = args
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("1.0.0");
            let spec = sz_rust_router_facade::openapi::spec_from_route_config(
                sz_rust_router_facade::openapi::OpenApiBuilder::new(title, version),
                &config,
            );
            Ok(spec)
        }
        "redaction_check" => {
            let config = args
                .get("config")
                .ok_or_else(|| McpError::InvalidArguments("config 必填".into()))?;
            let app_id = config
                .get("app_id")
                .and_then(|v| v.as_str())
                .unwrap_or("app");
            let private_key = config
                .get("merchant_private_key")
                .and_then(|v| v.as_str())
                .unwrap_or("dummy");
            let pay_config =
                sz_rust_pay_facade::PayConfig::new(sz_rust_pay_facade::PayPlatform::Alipay, app_id)
                    .with_merchant_private_key(private_key)
                    .with_platform_public_key("MIIPUBLIC")
                    .with_notify_url("https://example.com/notify");
            let debug_out = format!("{pay_config:?}");
            let leaked = private_key.len() > 8 && debug_out.contains(private_key);
            Ok(json!({
                "redacted": !leaked,
                "debug_output": debug_out,
                "note": if leaked { "❌ 泄漏：Debug 输出包含私钥明文" } else { "✅ 安全：Debug 输出已脱敏" }
            }).to_string())
        }
        "url_decode" => {
            let value = args
                .get("value")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("value 必填".into()))?;
            Ok(json!({"decoded": sz_rust_http_facade::request::url_decode(value)}).to_string())
        }
        "sql_validate" => {
            let sql = args
                .get("sql")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("sql 必填".into()))?;
            let result = sz_rust_orm_facade::validate_sql(sql);
            Ok(json!({
                "valid": result.is_ok(),
                "message": match &result {
                    Ok(()) => "SQL 校验通过".to_string(),
                    Err(e) => format!("SQL 校验失败: {e}"),
                }
            })
            .to_string())
        }
        "route_conflicts" => {
            let routes = args
                .get("routes")
                .and_then(|v| v.as_array())
                .ok_or_else(|| McpError::InvalidArguments("routes 必填".into()))?;
            let mut config = sz_rust_router_facade::routing::RouteConfig::new();
            for r in routes {
                let method = r.get("method").and_then(|m| m.as_str()).unwrap_or("GET");
                let path = r.get("path").and_then(|p| p.as_str()).unwrap_or("/");
                let handler = r
                    .get("handler")
                    .and_then(|h| h.as_str())
                    .unwrap_or("Index@index");
                let http_method = match method {
                    "POST" => sz_rust_router_facade::routing::HttpMethod::POST,
                    "PUT" => sz_rust_router_facade::routing::HttpMethod::PUT,
                    "DELETE" => sz_rust_router_facade::routing::HttpMethod::DELETE,
                    _ => sz_rust_router_facade::routing::HttpMethod::GET,
                };
                config.add_route(sz_rust_router_facade::routing::RouteRule::new(
                    http_method,
                    path,
                    handler,
                ));
            }
            let conflicts = config.find_conflicts();
            Ok(json!({
                "conflict_count": conflicts.len(),
                "conflicts": conflicts.iter().map(|(a, b)| json!({
                    "a": format!("{} {} -> {}", a.method, a.path, a.handler),
                    "b": format!("{} {} -> {}", b.method, b.path, b.handler),
                })).collect::<Vec<_>>(),
            })
            .to_string())
        }
        "build_insert_query" => {
            let table = args
                .get("table")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("table 必填".into()))?;
            let data = args
                .get("data")
                .and_then(|v| v.as_object())
                .ok_or_else(|| McpError::InvalidArguments("data 必填".into()))?;
            if data.is_empty() {
                return Err(McpError::InvalidArguments("data 不能为空".into()));
            }
            validate_identifier(table)?;
            for k in data.keys() {
                validate_identifier(k)?;
            }
            let columns: Vec<&str> = data.keys().map(|k| k.as_str()).collect();
            let placeholders: Vec<&str> = columns.iter().map(|_| "?").collect();
            let sql = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                table,
                columns.join(", "),
                placeholders.join(", ")
            );
            Ok(json!({"sql": sql, "params": data.len()}).to_string())
        }
        "build_update_query" => {
            let table = args
                .get("table")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("table 必填".into()))?;
            let set_map = args
                .get("set")
                .and_then(|v| v.as_object())
                .ok_or_else(|| McpError::InvalidArguments("set 必填".into()))?;
            if set_map.is_empty() {
                return Err(McpError::InvalidArguments("set 不能为空".into()));
            }
            validate_identifier(table)?;
            for k in set_map.keys() {
                validate_identifier(k)?;
            }
            let set_clause: Vec<String> = set_map.keys().map(|k| format!("{} = ?", k)).collect();
            let mut sql = format!("UPDATE {} SET {}", table, set_clause.join(", "));
            let mut param_count = set_map.len();
            if let Some(where_map) = args.get("where_eq").and_then(|v| v.as_object()) {
                if !where_map.is_empty() {
                    for k in where_map.keys() {
                        validate_identifier(k)?;
                    }
                    let where_clause: Vec<String> =
                        where_map.keys().map(|k| format!("{} = ?", k)).collect();
                    sql.push_str(&format!(" WHERE {}", where_clause.join(" AND ")));
                    param_count += where_map.len();
                }
            }
            Ok(json!({"sql": sql, "params": param_count}).to_string())
        }
        "build_delete_query" => {
            let table = args
                .get("table")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("table 必填".into()))?;
            let where_map = args
                .get("where_eq")
                .and_then(|v| v.as_object())
                .ok_or_else(|| McpError::InvalidArguments("where_eq 必填".into()))?;
            if where_map.is_empty() {
                return Err(McpError::InvalidArguments(
                    "where_eq 不能为空（禁止无条件删除）".into(),
                ));
            }
            validate_identifier(table)?;
            for k in where_map.keys() {
                validate_identifier(k)?;
            }
            let where_clause: Vec<String> =
                where_map.keys().map(|k| format!("{} = ?", k)).collect();
            let sql = format!("DELETE FROM {} WHERE {}", table, where_clause.join(" AND "));
            Ok(json!({"sql": sql, "params": where_map.len()}).to_string())
        }
        "crud_read" => {
            let table = args
                .get("table")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("table 必填".into()))?;
            let columns: Vec<&str> = args
                .get("columns")
                .and_then(|v| v.as_array())
                .ok_or_else(|| McpError::InvalidArguments("columns 必填".into()))?
                .iter()
                .filter_map(|c| c.as_str())
                .collect();
            if columns.is_empty() {
                return Err(McpError::InvalidArguments(
                    "columns 不能为空（禁止 SELECT *）".into(),
                ));
            }
            validate_identifier(table)?;
            for col in &columns {
                validate_identifier(col)?;
            }
            let mut sql = format!("SELECT {} FROM {}", columns.join(", "), table);
            let mut param_count = 0;
            if let Some(where_map) = args.get("where_eq").and_then(|v| v.as_object()) {
                if !where_map.is_empty() {
                    for k in where_map.keys() {
                        validate_identifier(k)?;
                    }
                    let where_clause: Vec<String> =
                        where_map.keys().map(|k| format!("{} = ?", k)).collect();
                    sql.push_str(&format!(" WHERE {}", where_clause.join(" AND ")));
                    param_count = where_map.len();
                }
            }
            if let Some(limit) = args.get("limit").and_then(|v| v.as_u64()) {
                sql.push_str(&format!(" LIMIT {}", limit));
            }
            Ok(json!({"sql": sql, "params": param_count}).to_string())
        }
        "migrate_create" => {
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("name 必填".into()))?;
            let description = args
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let template = format!(
                "-- Migration: {}\n-- Description: {}\n\n-- UP\n\n\n-- DOWN\n\n",
                name, description
            );
            Ok(json!({"template": template, "filename": format!("{}.sql", name)}).to_string())
        }
        "migrate_status" => {
            let migrations_dir = args
                .get("migrations_dir")
                .and_then(|v| v.as_str())
                .unwrap_or("migrations");
            Ok(json!({
                "migrations_dir": migrations_dir,
                "status": "placeholder — 实际使用时连接 sz-rust-migration crate 查询",
                "applied": [],
                "pending": []
            })
            .to_string())
        }
        "migrate_run" => {
            let direction = args
                .get("direction")
                .and_then(|v| v.as_str())
                .unwrap_or("up");
            let steps = args.get("steps").and_then(|v| v.as_u64()).unwrap_or(0);
            let mut cmd = "cargo run -p sz-rust-migration --".to_string();
            if direction == "down" {
                cmd.push_str(" --down");
            }
            if steps > 0 {
                cmd.push_str(&format!(" --steps {}", steps));
            }
            Ok(json!({"command": cmd, "direction": direction, "steps": steps}).to_string())
        }
        "test_run" => {
            let package = args.get("package").and_then(|v| v.as_str());
            let test_name = args.get("test_name").and_then(|v| v.as_str());
            let flags = args.get("flags").and_then(|v| v.as_str()).unwrap_or("");
            let mut cmd = "cargo test".to_string();
            if let Some(pkg) = package {
                cmd.push_str(&format!(" -p {}", pkg));
            }
            if let Some(tn) = test_name {
                cmd.push_str(&format!(" -- {}", tn));
            }
            if !flags.is_empty() {
                cmd.push_str(&format!(" {}", flags));
            }
            Ok(json!({"command": cmd}).to_string())
        }
        "test_coverage" => {
            let package = args.get("package").and_then(|v| v.as_str());
            let output_format = args
                .get("output_format")
                .and_then(|v| v.as_str())
                .unwrap_or("html");
            let mut cmd = "cargo llvm-cov".to_string();
            if let Some(pkg) = package {
                cmd.push_str(&format!(" -p {}", pkg));
            }
            cmd.push_str(&format!(" --{}", output_format));
            Ok(json!({"command": cmd, "note": "需安装 cargo-llvm-cov: cargo install cargo-llvm-cov"}).to_string())
        }
        "deploy_check" => {
            let config_path = args
                .get("config_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("config_path 必填".into()))?;
            let target = args
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("docker");
            Ok(json!({
                "config_path": config_path,
                "target": target,
                "checks": ["配置文件存在性", "端口冲突检测", "环境变量完整性", "依赖服务可达性"],
                "status": "placeholder — 实际使用时读取配置文件并校验"
            })
            .to_string())
        }
        "deploy_status" => {
            let target = args
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("docker");
            let cmd = match target {
                "k8s" => "kubectl get pods -o wide",
                "bare_metal" => "systemctl status sz-rust",
                _ => "docker ps --filter name=sz-rust",
            };
            Ok(json!({"command": cmd, "target": target}).to_string())
        }
        "plugin_list" => {
            let source = args.get("source").and_then(|v| v.as_str());
            let tags: Vec<&str> = args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|t| t.as_str()).collect())
                .unwrap_or_default();
            Ok(json!({
                "source": source,
                "tags": tags,
                "note": "实际使用时查询 CapabilityRegistry::list_all() / find_by_tags()",
                "plugins": []
            })
            .to_string())
        }
        "plugin_install" => {
            let plugin_name = args
                .get("plugin_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("plugin_name 必填".into()))?;
            let version = args.get("version").and_then(|v| v.as_str());
            let mut cmd = format!("cargo add {}", plugin_name);
            if let Some(v) = version {
                cmd.push_str(&format!(" --version {}", v));
            }
            Ok(json!({
                "command": cmd,
                "post_install": format!("在 Cargo.toml [dependencies] 追加 {} 并调用 CapabilityRegistry::register", plugin_name)
            }).to_string())
        }
        "plugin_uninstall" => {
            let plugin_name = args
                .get("plugin_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::InvalidArguments("plugin_name 必填".into()))?;
            Ok(json!({
                "command": format!("cargo remove {}", plugin_name),
                "post_uninstall": format!("调用 CapabilityRegistry::unregister(\"{}\") 清理注册", plugin_name)
            }).to_string())
        }
        _ => Err(McpError::ToolNotFound(name.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_capabilities() {
        let resp = handle_request(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        assert_eq!(resp["result"]["serverInfo"]["name"], "sz-rust-mcp");
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn tools_list_returns_seven_tools() {
        let resp = handle_request(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 21);
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"parse_path"));
        assert!(names.contains(&"openapi_spec"));
        assert!(names.contains(&"sql_validate"));
        assert!(names.contains(&"route_conflicts"));
        assert!(names.contains(&"build_insert_query"));
        assert!(names.contains(&"migrate_create"));
        assert!(names.contains(&"test_run"));
        assert!(names.contains(&"deploy_check"));
        assert!(names.contains(&"plugin_list"));
    }

    #[test]
    fn parse_path_tool() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"parse_path","arguments":{"uri":"/oapc/customer/index"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["controller"], "Customer");
        assert_eq!(parsed["action"], "index");
    }

    #[test]
    fn build_query_tool_parameterized() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"build_select_query","arguments":{"table":"users","columns":["id","name"],"where_eq":{"id":1}}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["sql"].as_str().unwrap().contains('?'));
        assert_eq!(out["params"], 1);
        // 防注入：显式列投影
        assert!(!out["sql"].as_str().unwrap().contains('*'));
    }

    #[test]
    fn redaction_check_tool_detects_leak() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"redaction_check","arguments":{"config":{"app_id":"app1","merchant_private_key":"SUPER_SECRET_KEY_12345"}}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert_eq!(out["redacted"], true);
    }

    #[test]
    fn unknown_tool_returns_error() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
        );
        assert!(resp["error"]["code"].as_i64().unwrap() < 0);
    }

    #[test]
    fn sql_validate_tool_rejects_injection() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"sql_validate","arguments":{"sql":"SELECT * FROM users WHERE id = 1 OR 1=1"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        // 危险模式（SELECT * + OR 恒真）应被识别
        assert_eq!(out["valid"], false);
    }

    #[test]
    fn sql_validate_tool_accepts_parameterized() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"sql_validate","arguments":{"sql":"SELECT id, name FROM users WHERE id = ?"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert_eq!(out["valid"], true);
    }

    #[test]
    fn route_conflicts_tool_detects_duplicate() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"route_conflicts","arguments":{"routes":[
                {"method":"GET","path":"/user/list","handler":"User@list"},
                {"method":"GET","path":"/user/list","handler":"Admin@list"}
            ]}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(
            out["conflict_count"]
                .as_i64()
                .expect("conflict_count 字段缺失")
                >= 1
        );
    }

    #[test]
    fn validate_identifier_accepts_valid() {
        assert!(validate_identifier("users").is_ok());
        assert!(validate_identifier("_users").is_ok());
        assert!(validate_identifier("user_orders").is_ok());
        assert!(validate_identifier("t1").is_ok());
    }

    #[test]
    fn validate_identifier_rejects_empty() {
        assert!(validate_identifier("").is_err());
    }

    #[test]
    fn validate_identifier_rejects_leading_digit() {
        assert!(validate_identifier("1table").is_err());
    }

    #[test]
    fn validate_identifier_rejects_special_chars() {
        assert!(validate_identifier("user; DROP TABLE users; --").is_err());
        assert!(validate_identifier("user' OR '1'='1").is_err());
        assert!(validate_identifier("user--").is_err());
        assert!(validate_identifier("user name").is_err());
        assert!(validate_identifier("user.name").is_err());
    }

    #[test]
    fn build_insert_rejects_injection_in_table() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"build_insert_query","arguments":{"table":"users; DROP TABLE orders; --","data":{"name":"test"}}}}"#,
        );
        assert!(resp["error"]["code"].as_i64().unwrap() < 0);
    }

    #[test]
    fn build_insert_rejects_injection_in_column() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"build_insert_query","arguments":{"table":"users","data":{"name' OR '1'='1":"test"}}}}"#,
        );
        assert!(resp["error"]["code"].as_i64().unwrap() < 0);
    }

    #[test]
    fn crud_read_rejects_injection_in_table() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"crud_read","arguments":{"table":"users UNION SELECT password FROM admin","columns":["id"]}}}"#,
        );
        assert!(resp["error"]["code"].as_i64().unwrap() < 0);
    }

    #[test]
    fn build_delete_rejects_injection_in_where_key() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"build_delete_query","arguments":{"table":"users","where_eq":{"id = 1 OR 1=1; --":1}}}}"#,
        );
        assert!(resp["error"]["code"].as_i64().unwrap() < 0);
    }

    // ===== 新增覆盖率测试 =====

    #[test]
    fn validate_identifier_rejects_too_long() {
        let long_name = "a".repeat(65);
        assert!(validate_identifier(&long_name).is_err());
    }

    #[test]
    fn handle_request_parse_error_returns_error() {
        let resp = handle_request("not a valid json");
        assert_eq!(resp["error"]["code"], -32700);
        assert!(resp["id"].is_null());
    }

    #[test]
    fn handle_request_notifications_initialized_returns_null() {
        let resp = handle_request(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(resp.is_null());
    }

    #[test]
    fn handle_request_unknown_method_with_id_returns_error() {
        let resp = handle_request(r#"{"jsonrpc":"2.0","id":99,"method":"foo/bar"}"#);
        assert_eq!(resp["error"]["code"], -32601);
        assert_eq!(resp["id"], 99);
    }

    #[test]
    fn handle_request_unknown_method_without_id_returns_null() {
        let resp = handle_request(r#"{"jsonrpc":"2.0","method":"foo/bar"}"#);
        assert!(resp.is_null());
    }

    #[test]
    fn openapi_spec_tool_generates_spec() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"openapi_spec","arguments":{"routes":[{"method":"GET","path":"/users","handler":"User@list"},{"method":"POST","path":"/users","handler":"User@create"}],"title":"MyAPI","version":"2.0.0"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        assert!(text.contains("openapi"));
    }

    #[test]
    fn url_decode_tool_decodes_percent_encoding() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"url_decode","arguments":{"value":"%E4%B8%AD%E6%96%87"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert_eq!(out["decoded"], "中文");
    }

    #[test]
    fn build_update_query_tool_generates_sql() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"build_update_query","arguments":{"table":"users","set":{"name":"alice"},"where_eq":{"id":1}}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["sql"].as_str().unwrap().contains("UPDATE"));
        assert_eq!(out["params"], 2);
    }

    #[test]
    fn build_update_query_without_where_eq() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":23,"method":"tools/call","params":{"name":"build_update_query","arguments":{"table":"users","set":{"name":"alice"}}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert_eq!(out["params"], 1);
    }

    #[test]
    fn build_update_query_rejects_empty_set() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":24,"method":"tools/call","params":{"name":"build_update_query","arguments":{"table":"users","set":{}}}}"#,
        );
        assert!(resp["error"]["code"].as_i64().unwrap() < 0);
    }

    #[test]
    fn migrate_status_tool_returns_placeholder() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{"name":"migrate_status","arguments":{"migrations_dir":"custom_migrations"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert_eq!(out["migrations_dir"], "custom_migrations");
    }

    #[test]
    fn migrate_run_tool_up_direction() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":26,"method":"tools/call","params":{"name":"migrate_run","arguments":{"direction":"up","steps":3}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["command"].as_str().unwrap().contains("--steps 3"));
    }

    #[test]
    fn migrate_run_tool_down_direction() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":27,"method":"tools/call","params":{"name":"migrate_run","arguments":{"direction":"down"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["command"].as_str().unwrap().contains("--down"));
    }

    #[test]
    fn test_run_tool_generates_command() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":28,"method":"tools/call","params":{"name":"test_run","arguments":{"package":"sz-rust-mcp","test_name":"parse_path_tool","flags":"--nocapture"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["command"].as_str().unwrap().contains("cargo test"));
        assert!(out["command"].as_str().unwrap().contains("-p sz-rust-mcp"));
    }

    #[test]
    fn test_coverage_tool_generates_command() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":29,"method":"tools/call","params":{"name":"test_coverage","arguments":{"package":"sz-rust-mcp","output_format":"text"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["command"].as_str().unwrap().contains("cargo llvm-cov"));
    }

    #[test]
    fn deploy_check_tool_returns_checks() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":30,"method":"tools/call","params":{"name":"deploy_check","arguments":{"config_path":"/etc/sz-rust/config.toml","target":"k8s"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert_eq!(out["target"], "k8s");
        assert!(out["checks"].is_array());
    }

    #[test]
    fn deploy_status_tool_k8s_target() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":31,"method":"tools/call","params":{"name":"deploy_status","arguments":{"target":"k8s"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["command"].as_str().unwrap().contains("kubectl"));
    }

    #[test]
    fn deploy_status_tool_bare_metal_target() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":32,"method":"tools/call","params":{"name":"deploy_status","arguments":{"target":"bare_metal"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["command"].as_str().unwrap().contains("systemctl"));
    }

    #[test]
    fn deploy_status_tool_default_docker_target() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":33,"method":"tools/call","params":{"name":"deploy_status","arguments":{}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["command"].as_str().unwrap().contains("docker ps"));
    }

    #[test]
    fn plugin_list_tool_returns_empty_list() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":34,"method":"tools/call","params":{"name":"plugin_list","arguments":{"source":"skill","tags":["auth","cache"]}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert_eq!(out["source"], "skill");
        assert!(out["plugins"].is_array());
    }

    #[test]
    fn plugin_install_tool_generates_command() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":35,"method":"tools/call","params":{"name":"plugin_install","arguments":{"plugin_name":"sz-rust-addon-cache","version":"1.0.0"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["command"].as_str().unwrap().contains("cargo add"));
        assert!(out["command"].as_str().unwrap().contains("--version 1.0.0"));
    }

    #[test]
    fn plugin_uninstall_tool_generates_command() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":36,"method":"tools/call","params":{"name":"plugin_uninstall","arguments":{"plugin_name":"sz-rust-addon-cache"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["command"].as_str().unwrap().contains("cargo remove"));
    }

    #[test]
    fn build_select_query_rejects_empty_columns() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":37,"method":"tools/call","params":{"name":"build_select_query","arguments":{"table":"users","columns":[]}}}"#,
        );
        assert!(resp["error"]["code"].as_i64().unwrap() < 0);
    }

    #[test]
    fn build_insert_query_rejects_empty_data() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":38,"method":"tools/call","params":{"name":"build_insert_query","arguments":{"table":"users","data":{}}}}"#,
        );
        assert!(resp["error"]["code"].as_i64().unwrap() < 0);
    }

    #[test]
    fn build_delete_query_rejects_empty_where_eq() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":39,"method":"tools/call","params":{"name":"build_delete_query","arguments":{"table":"users","where_eq":{}}}}"#,
        );
        assert!(resp["error"]["code"].as_i64().unwrap() < 0);
    }

    #[test]
    fn crud_read_rejects_empty_columns() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":40,"method":"tools/call","params":{"name":"crud_read","arguments":{"table":"users","columns":[]}}}"#,
        );
        assert!(resp["error"]["code"].as_i64().unwrap() < 0);
    }

    #[test]
    fn crud_read_with_limit_generates_sql() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":41,"method":"tools/call","params":{"name":"crud_read","arguments":{"table":"users","columns":["id","name"],"where_eq":{"status":"active"},"limit":10}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert!(out["sql"].as_str().unwrap().contains("LIMIT 10"));
        assert_eq!(out["params"], 1);
    }

    #[test]
    fn crud_read_without_where_eq() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"crud_read","arguments":{"table":"users","columns":["id","name"]}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert_eq!(out["params"], 0);
    }

    #[test]
    fn json_to_orm_value_null() {
        let v = json_to_orm_value(&Value::Null);
        assert!(matches!(v, sz_rust_orm_facade::Value::Null));
    }

    #[test]
    fn json_to_orm_value_bool() {
        let v = json_to_orm_value(&json!(true));
        assert!(matches!(v, sz_rust_orm_facade::Value::Bool(true)));
    }

    #[test]
    fn json_to_orm_value_string() {
        let v = json_to_orm_value(&json!("hello"));
        assert!(matches!(v, sz_rust_orm_facade::Value::String(_)));
    }

    #[test]
    fn json_to_orm_value_array() {
        let v = json_to_orm_value(&json!([1, 2, 3]));
        assert!(matches!(v, sz_rust_orm_facade::Value::String(_)));
    }

    #[test]
    fn json_to_orm_value_object() {
        let v = json_to_orm_value(&json!({"key": "val"}));
        assert!(matches!(v, sz_rust_orm_facade::Value::String(_)));
    }

    #[test]
    fn json_to_orm_value_f64() {
        let v = json_to_orm_value(&json!(3.5));
        assert!(matches!(v, sz_rust_orm_facade::Value::F64(_)));
    }

    #[test]
    fn build_select_query_with_where_eq_string_value() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":43,"method":"tools/call","params":{"name":"build_select_query","arguments":{"table":"users","columns":["id"],"where_eq":{"name":"alice"}}}}"#,
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("响应 text 字段缺失");
        let out: Value = serde_json::from_str(text).expect("响应 JSON 解析失败");
        assert_eq!(out["params"], 1);
    }
}
