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
            let mut q = sz_rust_orm_facade::Query::select()
                .columns(&columns)
                .from(table);
            // where_eq: {"id": 1, "status": "active"}
            if let Some(where_map) = args.get("where_eq").and_then(|v| v.as_object()) {
                for (col, val) in where_map {
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
        assert_eq!(tools.len(), 7);
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"parse_path"));
        assert!(names.contains(&"openapi_spec"));
        assert!(names.contains(&"sql_validate"));
        assert!(names.contains(&"route_conflicts"));
    }

    #[test]
    fn parse_path_tool() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"parse_path","arguments":{"uri":"/oapc/customer/index"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["controller"], "Customer");
        assert_eq!(parsed["action"], "index");
    }

    #[test]
    fn build_query_tool_parameterized() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"build_select_query","arguments":{"table":"users","columns":["id","name"],"where_eq":{"id":1}}}}"#,
        );
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let out: Value = serde_json::from_str(text).unwrap();
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
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let out: Value = serde_json::from_str(text).unwrap();
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
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let out: Value = serde_json::from_str(text).unwrap();
        // 危险模式（SELECT * + OR 恒真）应被识别
        assert_eq!(out["valid"], false);
    }

    #[test]
    fn sql_validate_tool_accepts_parameterized() {
        let resp = handle_request(
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"sql_validate","arguments":{"sql":"SELECT id, name FROM users WHERE id = ?"}}}"#,
        );
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let out: Value = serde_json::from_str(text).unwrap();
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
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let out: Value = serde_json::from_str(text).unwrap();
        assert!(out["conflict_count"].as_i64().unwrap() >= 1);
    }
}
