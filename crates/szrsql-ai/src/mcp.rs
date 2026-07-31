//! MCP Server — Phase 7b.6
//!
//! 对应 `SzRSQL技术实现方案.md` 9.9 节。
//!
//! # 设计
//!
//! Model Context Protocol (MCP) Server — 让外部 LLM 通过标准化协议调用
//! SzRSQL 工具，实现 LLM 与数据库的互操作。
//!
//! ## 协议
//!
//! - **传输层** — JSON-RPC 2.0 over stdio（每行一条 JSON 消息）
//! - **方法** — `initialize` / `tools/list` / `tools/call` / `shutdown`
//! - **工具** — `list_tables` / `describe_table` / `execute_sql` / `get_stats`
//!
//! ## 工作流程
//!
//! 1. Client 发送 `initialize` → Server 返回协议版本 + 能力
//! 2. Client 发送 `tools/list` → Server 返回工具清单
//! 3. Client 发送 `tools/call` (name + args) → Server 执行工具 → 返回结构化结果
//! 4. Client 发送 `shutdown` → Server 关闭
//!
//! # 验证标准
//!
//! - LLM 通过 MCP 协议调用 SzRSQL 工具（查询表结构/执行 SQL/获取统计信息）
//! - 返回结构化结果
//!
//! 对应 `SzRSQL实施进度.md` Phase 7b.6。

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// =====================================================================
//  错误类型
// =====================================================================

/// MCP 错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    /// JSON 解析错误
    #[error("parse error: {0}")]
    ParseError(String),
    /// 无效请求
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// 方法不存在
    #[error("method not found: {0}")]
    MethodNotFound(String),
    /// 工具不存在
    #[error("tool not found: {0}")]
    ToolNotFound(String),
    /// 工具参数无效
    #[error("invalid tool params: {0}")]
    InvalidToolParams(String),
    /// 工具执行错误
    #[error("tool execution error: {0}")]
    ToolExecutionError(String),
    /// 后端错误
    #[error("backend error: {0}")]
    BackendError(String),
}

impl McpError {
    /// 转换为 JSON-RPC 错误码
    fn code(&self) -> i32 {
        match self {
            Self::ParseError(_) => -32700,
            Self::InvalidRequest(_) => -32600,
            Self::MethodNotFound(_) => -32601,
            Self::InvalidToolParams(_) => -32602,
            Self::ToolNotFound(_) | Self::ToolExecutionError(_) => -32603,
            Self::BackendError(_) => -32000,
        }
    }
}

// =====================================================================
//  JSON-RPC 2.0 消息
// =====================================================================

/// JSON-RPC 2.0 请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 错误
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    /// 成功响应
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// 错误响应
    pub fn error(id: Option<Value>, code: i32, message: String, data: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data,
            }),
        }
    }
}

// =====================================================================
//  MCP 协议常量
// =====================================================================

/// MCP 协议版本
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// MCP Server 名称
pub const MCP_SERVER_NAME: &str = "szrsql-mcp-server";

/// MCP Server 版本（与 crate 版本一致）
pub const MCP_SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// =====================================================================
//  后端接口 — 工具实际执行的后端
// =====================================================================

/// MCP 后端 — 提供实际数据库操作能力
///
/// 工具通过后端执行实际操作，便于测试时注入 Mock 后端。
pub trait McpBackend {
    /// 列出所有表
    fn list_tables(&self) -> Result<Vec<TableInfo>, McpError>;

    /// 描述表结构
    fn describe_table(&self, table: &str) -> Result<TableSchema, McpError>;

    /// 执行 SQL（返回行数据）
    fn execute_sql(&self, sql: &str) -> Result<QueryResult, McpError>;

    /// 获取数据库统计信息
    fn get_stats(&self) -> Result<DbStats, McpError>;
}

/// 表信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub row_count: u64,
    pub size_bytes: u64,
}

/// 列定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    /// 列注释 — Phase TDengine-P2
    pub comment: Option<String>,
}

/// 表结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    pub table: String,
    pub columns: Vec<ColumnDef>,
}

/// 查询结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub affected_rows: u64,
    pub elapsed_ms: u64,
}

/// 数据库统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbStats {
    pub table_count: usize,
    pub total_rows: u64,
    pub total_size_bytes: u64,
    pub cache_hit_rate: f64,
    pub active_connections: u32,
}

// =====================================================================
//  内置 Mock 后端 — 用于测试和演示
// =====================================================================

/// 内存 Mock 后端 — 用于测试
pub struct MockBackend {
    tables: HashMap<String, TableSchema>,
    row_counts: HashMap<String, u64>,
}

impl Default for MockBackend {
    fn default() -> Self {
        let mut tables = HashMap::new();
        let mut row_counts = HashMap::new();

        // 内置示例表：products
        tables.insert(
            "products".to_string(),
            TableSchema {
                table: "products".to_string(),
                columns: vec![
                    ColumnDef {
                        name: "id".to_string(),
                        data_type: "BIGINT".to_string(),
                        nullable: false,
                        primary_key: true,
                        comment: None,
                    },
                    ColumnDef {
                        name: "name".to_string(),
                        data_type: "VARCHAR(255)".to_string(),
                        nullable: false,
                        primary_key: false,
                        comment: None,
                    },
                    ColumnDef {
                        name: "price".to_string(),
                        data_type: "DECIMAL(10,2)".to_string(),
                        nullable: true,
                        primary_key: false,
                        comment: None,
                    },
                    ColumnDef {
                        name: "stock".to_string(),
                        data_type: "INT".to_string(),
                        nullable: true,
                        primary_key: false,
                        comment: None,
                    },
                ],
            },
        );
        row_counts.insert("products".to_string(), 1000);

        // 内置示例表：orders
        tables.insert(
            "orders".to_string(),
            TableSchema {
                table: "orders".to_string(),
                columns: vec![
                    ColumnDef {
                        name: "order_id".to_string(),
                        data_type: "BIGINT".to_string(),
                        nullable: false,
                        primary_key: true,
                        comment: None,
                    },
                    ColumnDef {
                        name: "customer_id".to_string(),
                        data_type: "BIGINT".to_string(),
                        nullable: false,
                        primary_key: false,
                        comment: None,
                    },
                    ColumnDef {
                        name: "total".to_string(),
                        data_type: "DECIMAL(10,2)".to_string(),
                        nullable: false,
                        primary_key: false,
                        comment: None,
                    },
                ],
            },
        );
        row_counts.insert("orders".to_string(), 5000);

        Self { tables, row_counts }
    }
}

impl McpBackend for MockBackend {
    fn list_tables(&self) -> Result<Vec<TableInfo>, McpError> {
        let tables: Vec<TableInfo> = self
            .tables
            .values()
            .map(|schema| TableInfo {
                name: schema.table.clone(),
                row_count: *self.row_counts.get(&schema.table).unwrap_or(&0),
                size_bytes: schema.columns.len() as u64 * 1024,
            })
            .collect();
        Ok(tables)
    }

    fn describe_table(&self, table: &str) -> Result<TableSchema, McpError> {
        self.tables
            .get(table)
            .cloned()
            .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))
    }

    fn execute_sql(&self, sql: &str) -> Result<QueryResult, McpError> {
        let sql_lower = sql.to_lowercase();
        if sql_lower.contains("select") {
            // 模拟 SELECT 查询
            if sql_lower.contains("from products") {
                return Ok(QueryResult {
                    columns: vec!["id".to_string(), "name".to_string(), "price".to_string()],
                    rows: vec![
                        vec![json!(1), json!("苹果汁"), json!(5.5)],
                        vec![json!(2), json!("橙汁"), json!(6.0)],
                        vec![json!(3), json!("面包"), json!(8.0)],
                    ],
                    affected_rows: 0,
                    elapsed_ms: 2,
                });
            }
            if sql_lower.contains("from orders") {
                return Ok(QueryResult {
                    columns: vec![
                        "order_id".to_string(),
                        "customer_id".to_string(),
                        "total".to_string(),
                    ],
                    rows: vec![
                        vec![json!(1001), json!(1), json!(55.5)],
                        vec![json!(1002), json!(2), json!(120.0)],
                    ],
                    affected_rows: 0,
                    elapsed_ms: 3,
                });
            }
            return Ok(QueryResult {
                columns: vec![],
                rows: vec![],
                affected_rows: 0,
                elapsed_ms: 1,
            });
        }
        if sql_lower.contains("insert")
            || sql_lower.contains("update")
            || sql_lower.contains("delete")
        {
            return Ok(QueryResult {
                columns: vec![],
                rows: vec![],
                affected_rows: 1,
                elapsed_ms: 1,
            });
        }
        Err(McpError::BackendError(format!("unsupported SQL: {sql}")))
    }

    fn get_stats(&self) -> Result<DbStats, McpError> {
        let total_rows: u64 = self.row_counts.values().sum();
        Ok(DbStats {
            table_count: self.tables.len(),
            total_rows,
            total_size_bytes: self.tables.len() as u64 * 1024 * 1024,
            cache_hit_rate: 0.85,
            active_connections: 3,
        })
    }
}

// =====================================================================
//  工具定义
// =====================================================================

/// 工具定义
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// 工具调用结果
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallResult {
    pub content: Vec<ToolContent>,
    pub is_error: bool,
}

/// 工具内容块
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
}

impl ToolContent {
    /// 创建文本内容块
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

impl ToolCallResult {
    /// 成功结果（文本）
    pub fn text_success(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::text(text)],
            is_error: false,
        }
    }

    /// 错误结果
    pub fn text_error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ToolContent::text(text)],
            is_error: true,
        }
    }
}

// =====================================================================
//  McpServer — MCP 服务器主体
// =====================================================================

/// MCP Server — JSON-RPC 2.0 over stdio
pub struct McpServer {
    backend: Box<dyn McpBackend>,
    initialized: bool,
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new(Box::new(MockBackend::default()))
    }
}

impl McpServer {
    /// 创建 MCP Server
    pub fn new(backend: Box<dyn McpBackend>) -> Self {
        Self {
            backend,
            initialized: false,
        }
    }

    /// 所有工具定义
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "list_tables".to_string(),
                description: "列出数据库中所有表".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: "describe_table".to_string(),
                description: "描述指定表的结构（列名、类型、约束）".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "table": {
                            "type": "string",
                            "description": "表名"
                        }
                    },
                    "required": ["table"],
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: "execute_sql".to_string(),
                description: "执行 SQL 语句（SELECT/INSERT/UPDATE/DELETE）并返回结果".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "sql": {
                            "type": "string",
                            "description": "SQL 语句"
                        }
                    },
                    "required": ["sql"],
                    "additionalProperties": false
                }),
            },
            ToolDefinition {
                name: "get_stats".to_string(),
                description: "获取数据库统计信息（表数、总行数、缓存命中率等）".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            },
        ]
    }

    /// 处理 JSON-RPC 请求，返回 JSON-RPC 响应
    pub fn handle_request(&mut self, req: &JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id.clone();

        // 解析错误请求（jsonrpc 字段不是 "2.0"）
        if req.jsonrpc != "2.0" {
            return JsonRpcResponse::error(
                id,
                -32600,
                "Invalid Request: jsonrpc must be \"2.0\"".to_string(),
                None,
            );
        }

        let result = match req.method.as_str() {
            "initialize" => self.handle_initialize(req.params.as_ref()),
            "initialized" => {
                // 通知 — 无需响应
                return JsonRpcResponse::success(id, json!({}));
            }
            "tools/list" => self.handle_tools_list(),
            "tools/call" => self.handle_tools_call(req.params.as_ref()),
            "shutdown" => {
                self.initialized = false;
                Ok(json!({}))
            }
            _ => Err(McpError::MethodNotFound(req.method.clone())),
        };

        match result {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(err) => JsonRpcResponse::error(id, err.code(), err.to_string(), None),
        }
    }

    /// 处理 initialize 方法
    fn handle_initialize(&mut self, _params: Option<&Value>) -> Result<Value, McpError> {
        self.initialized = true;
        Ok(json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "serverInfo": {
                "name": MCP_SERVER_NAME,
                "version": MCP_SERVER_VERSION
            },
            "capabilities": {
                "tools": {
                    "listChanged": false
                }
            }
        }))
    }

    /// 处理 tools/list 方法
    fn handle_tools_list(&self) -> Result<Value, McpError> {
        let tools = self.tool_definitions();
        Ok(json!({
            "tools": tools
        }))
    }

    /// 处理 tools/call 方法
    fn handle_tools_call(&self, params: Option<&Value>) -> Result<Value, McpError> {
        let params = params.ok_or_else(|| {
            McpError::InvalidToolParams("missing params for tools/call".to_string())
        })?;

        let tool_name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'name' field".to_string()))?;

        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let result = match tool_name {
            "list_tables" => self.tool_list_tables(&args)?,
            "describe_table" => self.tool_describe_table(&args)?,
            "execute_sql" => self.tool_execute_sql(&args)?,
            "get_stats" => self.tool_get_stats(&args)?,
            _ => {
                return Err(McpError::ToolNotFound(tool_name.to_string()));
            }
        };

        serde_json::to_value(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize result failed: {e}")))
    }

    // -----------------------------------------------------------------
    //  工具实现
    // -----------------------------------------------------------------

    /// list_tables 工具
    fn tool_list_tables(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let tables = self.backend.list_tables()?;
        let text = serde_json::to_string_pretty(&tables)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    /// describe_table 工具
    fn tool_describe_table(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let table = args
            .get("table")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'table' argument".to_string()))?;

        let schema = self.backend.describe_table(table)?;
        let text = serde_json::to_string_pretty(&schema)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    /// execute_sql 工具
    fn tool_execute_sql(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let sql = args
            .get("sql")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'sql' argument".to_string()))?;

        if sql.trim().is_empty() {
            return Err(McpError::InvalidToolParams("sql is empty".to_string()));
        }

        let result = self.backend.execute_sql(sql)?;
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    /// get_stats 工具
    fn tool_get_stats(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let stats = self.backend.get_stats()?;
        let text = serde_json::to_string_pretty(&stats)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    // -----------------------------------------------------------------
    //  stdio 主循环
    // -----------------------------------------------------------------

    /// 运行 stdio 主循环（每行一条 JSON-RPC 消息）
    ///
    /// 从 stdin 读取请求，向 stdout 写入响应。
    /// 遇到 EOF 或 shutdown 后退出。
    pub fn run_stdio(&mut self) -> Result<(), McpError> {
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut stdout = stdout.lock();

        for line in stdin.lock().lines() {
            let line = line.map_err(|e| McpError::ParseError(format!("read line: {e}")))?;
            if line.trim().is_empty() {
                continue;
            }

            let req: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let resp =
                        JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"), None);
                    let json = serde_json::to_string(&resp)
                        .map_err(|e| McpError::ParseError(format!("serialize: {e}")))?;
                    writeln!(stdout, "{json}")
                        .map_err(|e| McpError::BackendError(format!("write: {e}")))?;
                    stdout
                        .flush()
                        .map_err(|e| McpError::BackendError(format!("flush: {e}")))?;
                    continue;
                }
            };

            let is_shutdown = req.method == "shutdown";
            let resp = self.handle_request(&req);
            let json = serde_json::to_string(&resp)
                .map_err(|e| McpError::ParseError(format!("serialize: {e}")))?;
            writeln!(stdout, "{json}")
                .map_err(|e| McpError::BackendError(format!("write: {e}")))?;
            stdout
                .flush()
                .map_err(|e| McpError::BackendError(format!("flush: {e}")))?;

            if is_shutdown {
                break;
            }
        }

        Ok(())
    }
}

// =====================================================================
//  便捷函数 — 解析单条请求
// =====================================================================

/// 解析并处理单条 JSON-RPC 请求字符串
///
/// 用于测试和集成。返回响应的 JSON 字符串。
pub fn handle_request_json(server: &mut McpServer, request_json: &str) -> String {
    let req: JsonRpcRequest = match serde_json::from_str(request_json) {
        Ok(r) => r,
        Err(e) => {
            let resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"), None);
            return serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string());
        }
    };
    let resp = server.handle_request(&req);
    serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string())
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]
    use super::*;

    // -----------------------------------------------------------------
    //  JSON-RPC 基础测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_jsonrpc_success_response() {
        let resp = JsonRpcResponse::success(Some(json!(1)), json!({"ok": true}));
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"jsonrpc\":\"2.0\""));
        assert!(s.contains("\"id\":1"));
        assert!(s.contains("\"result\""));
        assert!(!s.contains("\"error\""));
    }

    #[test]
    fn test_7b6_jsonrpc_error_response() {
        let resp = JsonRpcResponse::error(Some(json!(2)), -32601, "not found".to_string(), None);
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"code\":-32601"));
        assert!(s.contains("\"message\":\"not found\""));
        assert!(!s.contains("\"result\""));
    }

    #[test]
    fn test_7b6_jsonrpc_parse_request() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "initialize");
        assert_eq!(req.id, Some(json!(1)));
    }

    // -----------------------------------------------------------------
    //  错误码测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_error_codes() {
        assert_eq!(McpError::ParseError("x".to_string()).code(), -32700);
        assert_eq!(McpError::InvalidRequest("x".to_string()).code(), -32600);
        assert_eq!(McpError::MethodNotFound("x".to_string()).code(), -32601);
        assert_eq!(McpError::InvalidToolParams("x".to_string()).code(), -32602);
        assert_eq!(McpError::ToolNotFound("x".to_string()).code(), -32603);
        assert_eq!(McpError::ToolExecutionError("x".to_string()).code(), -32603);
        assert_eq!(McpError::BackendError("x".to_string()).code(), -32000);
    }

    // -----------------------------------------------------------------
    //  initialize 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_initialize() {
        let mut server = McpServer::default();
        assert!(!server.initialized);

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "initialize".to_string(),
            params: Some(json!({})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], MCP_SERVER_NAME);
        assert_eq!(result["serverInfo"]["version"], MCP_SERVER_VERSION);
        assert!(result["capabilities"]["tools"]["listChanged"].is_boolean());
        assert!(server.initialized);
    }

    #[test]
    fn test_7b6_invalid_jsonrpc_version() {
        let mut server = McpServer::default();
        let req = JsonRpcRequest {
            jsonrpc: "1.0".to_string(),
            id: Some(json!(1)),
            method: "initialize".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32600);
    }

    // -----------------------------------------------------------------
    //  tools/list 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_tools_list() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/list".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 4);

        let names: Vec<String> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"list_tables".to_string()));
        assert!(names.contains(&"describe_table".to_string()));
        assert!(names.contains(&"execute_sql".to_string()));
        assert!(names.contains(&"get_stats".to_string()));
    }

    #[test]
    fn test_7b6_tool_definitions_have_schema() {
        let server = McpServer::default();
        for def in server.tool_definitions() {
            assert!(def.input_schema.is_object());
            assert!(!def.description.is_empty());
        }
    }

    // -----------------------------------------------------------------
    //  tools/call — list_tables 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_call_list_tables() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "list_tables",
                "arguments": {}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["is_error"], false);
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");

        let text = content[0]["text"].as_str().unwrap();
        assert!(text.contains("products"));
        assert!(text.contains("orders"));
    }

    // -----------------------------------------------------------------
    //  tools/call — describe_table 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_call_describe_table() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "describe_table",
                "arguments": {
                    "table": "products"
                }
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["is_error"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("products"));
        assert!(text.contains("id"));
        assert!(text.contains("name"));
        assert!(text.contains("price"));
    }

    #[test]
    fn test_7b6_call_describe_table_not_found() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(5)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "describe_table",
                "arguments": {
                    "table": "nonexistent"
                }
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32000); // BackendError
    }

    #[test]
    fn test_7b6_call_describe_table_missing_arg() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(6)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "describe_table",
                "arguments": {}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602); // InvalidToolParams
    }

    // -----------------------------------------------------------------
    //  tools/call — execute_sql 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_call_execute_sql_select() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(7)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "execute_sql",
                "arguments": {
                    "sql": "SELECT id, name, price FROM products"
                }
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["is_error"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("columns"));
        assert!(text.contains("rows"));
        assert!(text.contains("苹果汁"));
    }

    #[test]
    fn test_7b6_call_execute_sql_insert() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(8)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "execute_sql",
                "arguments": {
                    "sql": "INSERT INTO products VALUES (4, '牛奶', 7.5)"
                }
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("affected_rows"));
    }

    #[test]
    fn test_7b6_call_execute_sql_empty() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(9)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "execute_sql",
                "arguments": {
                    "sql": "   "
                }
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602); // InvalidToolParams
    }

    #[test]
    fn test_7b6_call_execute_sql_missing_arg() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(10)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "execute_sql",
                "arguments": {}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    // -----------------------------------------------------------------
    //  tools/call — get_stats 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_call_get_stats() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(11)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "get_stats",
                "arguments": {}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["is_error"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("table_count"));
        assert!(text.contains("total_rows"));
        assert!(text.contains("cache_hit_rate"));
    }

    // -----------------------------------------------------------------
    //  tools/call — 错误情况测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_call_unknown_tool() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(12)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "nonexistent_tool",
                "arguments": {}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32603); // ToolNotFound
    }

    #[test]
    fn test_7b6_call_missing_name() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(13)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "arguments": {}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[test]
    fn test_7b6_call_missing_params() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(14)),
            method: "tools/call".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    // -----------------------------------------------------------------
    //  方法不存在测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_method_not_found() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(15)),
            method: "nonexistent/method".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // -----------------------------------------------------------------
    //  shutdown 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_shutdown() {
        let mut server = McpServer::default();
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(16)),
            method: "shutdown".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        assert!(!server.initialized);
    }

    // -----------------------------------------------------------------
    //  handle_request_json 便捷函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_handle_request_json_initialize() {
        let mut server = McpServer::default();
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let response = handle_request_json(&mut server, request);
        assert!(response.contains("\"protocolVersion\""));
        assert!(response.contains("\"name\":\"szrsql-mcp-server\""));
    }

    #[test]
    fn test_7b6_handle_request_json_invalid_json() {
        let mut server = McpServer::default();
        let response = handle_request_json(&mut server, "not valid json");
        assert!(response.contains("-32700"));
    }

    #[test]
    fn test_7b6_handle_request_json_full_flow() {
        let mut server = McpServer::default();

        // 1. initialize
        let r1 = handle_request_json(
            &mut server,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );
        assert!(r1.contains("protocolVersion"));

        // 2. tools/list
        let r2 = handle_request_json(
            &mut server,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        );
        assert!(r2.contains("list_tables"));
        assert!(r2.contains("execute_sql"));

        // 3. tools/call - list_tables
        let r3 = handle_request_json(
            &mut server,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_tables","arguments":{}}}"#,
        );
        assert!(r3.contains("products"));

        // 4. tools/call - execute_sql
        let r4 = handle_request_json(
            &mut server,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"execute_sql","arguments":{"sql":"SELECT * FROM products"}}}"#,
        );
        assert!(r4.contains("苹果汁"));

        // 5. shutdown
        let r5 = handle_request_json(
            &mut server,
            r#"{"jsonrpc":"2.0","id":5,"method":"shutdown"}"#,
        );
        assert!(r5.contains("result"));
    }

    // -----------------------------------------------------------------
    //  MockBackend 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_mock_backend_list_tables() {
        let backend = MockBackend::default();
        let tables = backend.list_tables().unwrap();
        assert!(tables.len() >= 2);
        let names: Vec<String> = tables.iter().map(|t| t.name.clone()).collect();
        assert!(names.contains(&"products".to_string()));
        assert!(names.contains(&"orders".to_string()));
    }

    #[test]
    fn test_7b6_mock_backend_describe_table() {
        let backend = MockBackend::default();
        let schema = backend.describe_table("products").unwrap();
        assert_eq!(schema.table, "products");
        assert!(schema.columns.len() >= 3);
        assert!(schema
            .columns
            .iter()
            .any(|c| c.primary_key && c.name == "id"));
    }

    #[test]
    fn test_7b6_mock_backend_describe_table_not_found() {
        let backend = MockBackend::default();
        let result = backend.describe_table("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_7b6_mock_backend_execute_sql_select() {
        let backend = MockBackend::default();
        let result = backend
            .execute_sql("SELECT id, name FROM products")
            .unwrap();
        assert!(!result.columns.is_empty());
        assert!(!result.rows.is_empty());
        assert_eq!(result.affected_rows, 0);
    }

    #[test]
    fn test_7b6_mock_backend_execute_sql_insert() {
        let backend = MockBackend::default();
        let result = backend
            .execute_sql("INSERT INTO products VALUES (10, 'test', 1.0)")
            .unwrap();
        assert_eq!(result.affected_rows, 1);
    }

    #[test]
    fn test_7b6_mock_backend_execute_sql_unsupported() {
        let backend = MockBackend::default();
        let result = backend.execute_sql("DROP TABLE unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_7b6_mock_backend_get_stats() {
        let backend = MockBackend::default();
        let stats = backend.get_stats().unwrap();
        assert!(stats.table_count >= 2);
        assert!(stats.total_rows > 0);
        assert!(stats.cache_hit_rate > 0.0);
    }

    // -----------------------------------------------------------------
    //  ToolContent / ToolCallResult 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_tool_content_text() {
        let c = ToolContent::text("hello");
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"type\":\"text\""));
        assert!(s.contains("\"text\":\"hello\""));
    }

    #[test]
    fn test_7b6_tool_call_result_success() {
        let r = ToolCallResult::text_success("ok");
        assert!(!r.is_error);
        assert_eq!(r.content.len(), 1);
    }

    #[test]
    fn test_7b6_tool_call_result_error() {
        let r = ToolCallResult::text_error("failed");
        assert!(r.is_error);
        assert_eq!(r.content.len(), 1);
    }

    // -----------------------------------------------------------------
    //  协议常量测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_protocol_constants() {
        assert_eq!(MCP_PROTOCOL_VERSION, "2024-11-05");
        assert_eq!(MCP_SERVER_NAME, "szrsql-mcp-server");
        assert!(!MCP_SERVER_VERSION.is_empty());
    }

    // -----------------------------------------------------------------
    //  自定义后端测试 — 验证 McpBackend trait 可被外部实现
    // -----------------------------------------------------------------

    struct CustomBackend {
        tables: Vec<String>,
    }

    impl McpBackend for CustomBackend {
        fn list_tables(&self) -> Result<Vec<TableInfo>, McpError> {
            Ok(self
                .tables
                .iter()
                .map(|t| TableInfo {
                    name: t.clone(),
                    row_count: 0,
                    size_bytes: 0,
                })
                .collect())
        }

        fn describe_table(&self, table: &str) -> Result<TableSchema, McpError> {
            Ok(TableSchema {
                table: table.to_string(),
                columns: vec![ColumnDef {
                    name: "id".to_string(),
                    data_type: "INT".to_string(),
                    nullable: false,
                    primary_key: true,
                    comment: None,
                }],
            })
        }

        fn execute_sql(&self, _sql: &str) -> Result<QueryResult, McpError> {
            Ok(QueryResult {
                columns: vec!["id".to_string()],
                rows: vec![vec![json!(1)]],
                affected_rows: 0,
                elapsed_ms: 0,
            })
        }

        fn get_stats(&self) -> Result<DbStats, McpError> {
            Ok(DbStats {
                table_count: self.tables.len(),
                total_rows: 0,
                total_size_bytes: 0,
                cache_hit_rate: 0.0,
                active_connections: 1,
            })
        }
    }

    #[test]
    fn test_7b6_custom_backend() {
        let backend = Box::new(CustomBackend {
            tables: vec!["users".to_string(), "posts".to_string()],
        });
        let mut server = McpServer::new(backend);
        server.initialized = true;

        // list_tables
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "list_tables",
                "arguments": {}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let binding = resp.result.unwrap();
        let text = binding["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("users"));
        assert!(text.contains("posts"));
        assert!(!text.contains("products"));
    }

    // -----------------------------------------------------------------
    //  完整 LLM 调用流程模拟
    // -----------------------------------------------------------------

    #[test]
    fn test_7b6_full_llm_workflow_simulation() {
        let mut server = McpServer::default();

        // Step 1: LLM 发起 initialize
        let init_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!("init-1")),
            method: "initialize".to_string(),
            params: Some(json!({})),
        };
        let init_resp = server.handle_request(&init_req);
        assert!(init_resp.error.is_none());
        assert_eq!(
            init_resp.result.unwrap()["protocolVersion"],
            MCP_PROTOCOL_VERSION
        );

        // Step 2: LLM 查询可用工具
        let list_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!("list-1")),
            method: "tools/list".to_string(),
            params: None,
        };
        let list_resp = server.handle_request(&list_req);
        let tools = list_resp.result.unwrap()["tools"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(tools.len(), 4);

        // Step 3: LLM 调用 list_tables 了解数据库
        let call_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!("call-1")),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "list_tables",
                "arguments": {}
            })),
        };
        let call_resp = server.handle_request(&call_req);
        let result_text = call_resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(result_text.contains("products"));
        assert!(result_text.contains("orders"));

        // Step 4: LLM 调用 describe_table 了解 products 表结构
        let desc_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!("call-2")),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "describe_table",
                "arguments": {
                    "table": "products"
                }
            })),
        };
        let desc_resp = server.handle_request(&desc_req);
        let desc_text = desc_resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(desc_text.contains("price"));
        assert!(desc_text.contains("stock"));

        // Step 5: LLM 调用 execute_sql 查询商品
        let sql_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!("call-3")),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "execute_sql",
                "arguments": {
                    "sql": "SELECT id, name, price FROM products"
                }
            })),
        };
        let sql_resp = server.handle_request(&sql_req);
        let sql_text = sql_resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(sql_text.contains("苹果汁"));
        assert!(sql_text.contains("columns"));

        // Step 6: LLM 调用 get_stats 了解数据库整体状态
        let stats_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!("call-4")),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "get_stats",
                "arguments": {}
            })),
        };
        let stats_resp = server.handle_request(&stats_req);
        let stats_text = stats_resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(stats_text.contains("table_count"));
        assert!(stats_text.contains("total_rows"));

        // Step 7: LLM 发起 shutdown
        let shutdown_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!("shutdown-1")),
            method: "shutdown".to_string(),
            params: None,
        };
        let shutdown_resp = server.handle_request(&shutdown_req);
        assert!(shutdown_resp.error.is_none());
        assert!(!server.initialized);
    }
}
