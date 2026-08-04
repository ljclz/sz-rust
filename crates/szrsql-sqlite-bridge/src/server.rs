//! SQLite TCP 服务器 — JSON 行协议。
//!
//! 由于 SQLite 本身是嵌入式文件数据库、没有标准 wire 协议，
//! 本服务器实现一个简单的 JSON 行协议（line-delimited JSON），
//! 允许远程客户端通过 TCP 执行 SQL 语句，结果以 JSON 返回。
//!
//! # 协议
//!
//! - 请求：`{"sql":"SELECT 1"}\n`（UTF-8，单行 JSON）
//! - 响应（成功）：`{"columns":["?column?"],"rows":[[1]],"tag":"SELECT 1"}\n`
//! - 响应（错误）：`{"error":"syntax error at..."}\n`
//!
//! # 设计
//!
//! - 每个连接持有独立的 `ExecutorService`，会话隔离
//! - 支持连接空闲超时（超时后主动关闭，释放资源）
//! - 使用 `tokio::BufReader` 按行读取请求
//!
//! # 用法
//!
//! ```ignore
//! use szrsql_sqlite_bridge::{SqliteConfig, SqliteServer};
//!
//! let config = SqliteConfig::new()
//!     .with_host("127.0.0.1")
//!     .with_port(9432);
//! let server = SqliteServer::new(config);
//! server.serve().await?;
//! ```

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use szrsql_protocol::pgwire::session::{ExecutorService, QueryResult, SessionError};
use szrsql_types::value::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

// =====================================================================
//  SqliteConfig
// =====================================================================

/// SQLite 服务器配置。
#[derive(Debug, Clone)]
pub struct SqliteConfig {
    /// 监听地址
    pub host: String,
    /// 监听端口
    pub port: u16,
    /// 服务器版本字符串
    pub server_version: String,
    /// 连接空闲超时（`Duration::ZERO` 表示禁用）
    pub connection_idle_timeout: Duration,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 9432,
            server_version: "3.45-szrsql".to_string(),
            connection_idle_timeout: Duration::from_secs(300),
        }
    }
}

impl SqliteConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置监听地址
    pub fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = host.into();
        self
    }

    /// 设置监听端口
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// 设置服务器版本字符串
    pub fn with_server_version(mut self, version: impl Into<String>) -> Self {
        self.server_version = version.into();
        self
    }

    /// 设置连接空闲超时
    pub fn with_connection_idle_timeout(mut self, timeout: Duration) -> Self {
        self.connection_idle_timeout = timeout;
        self
    }
}

// =====================================================================
//  SqliteServerError
// =====================================================================

/// SQLite 服务器错误。
#[derive(Debug, Error)]
pub enum SqliteServerError {
    /// IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON 序列化/反序列化错误
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

// =====================================================================
//  SqliteServer
// =====================================================================

/// SQLite TCP 服务器。
///
/// 每个客户端连接在独立的 tokio task 中处理，
/// 持有独立的 `ExecutorService` 实现会话隔离。
pub struct SqliteServer {
    config: SqliteConfig,
    connection_id_counter: AtomicI32,
}

impl SqliteServer {
    /// 创建新服务器
    pub fn new(config: SqliteConfig) -> Self {
        Self {
            config,
            connection_id_counter: AtomicI32::new(1),
        }
    }

    /// 启动监听循环。
    pub async fn serve(self) -> Result<(), SqliteServerError> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).await?;
        tracing::info!("SQLite server listening on {} (JSON line protocol)", addr);
        let self_arc = Arc::new(self);
        loop {
            let (stream, peer_addr) = listener.accept().await?;
            tracing::debug!(peer = %peer_addr, "SQLite connection accepted");
            let server = Arc::clone(&self_arc);
            tokio::spawn(async move {
                if let Err(e) = server.handle_connection(stream).await {
                    tracing::warn!(error = %e, "SQLite connection error");
                }
            });
        }
    }

    /// 处理单个客户端连接。
    pub async fn handle_connection(&self, stream: TcpStream) -> Result<(), SqliteServerError> {
        let conn_id = self.connection_id_counter.fetch_add(1, Ordering::SeqCst) as u32;
        let conn = Connection::new(self.config.clone(), conn_id);
        conn.handle(stream).await
    }
}

// =====================================================================
//  Connection
// =====================================================================

/// 单个客户端连接。
struct Connection {
    config: SqliteConfig,
    conn_id: u32,
    executor: Arc<Mutex<ExecutorService>>,
}

impl Connection {
    /// 创建新连接
    fn new(config: SqliteConfig, conn_id: u32) -> Self {
        Self {
            config,
            conn_id,
            executor: Arc::new(Mutex::new(ExecutorService::new())),
        }
    }

    /// 连接主处理流程。
    async fn handle(&self, stream: TcpStream) -> Result<(), SqliteServerError> {
        let (reader, mut writer) = stream.into_split();
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();
        let idle_timeout = self.config.connection_idle_timeout;

        loop {
            line.clear();
            // 读取一行 JSON 请求，支持空闲超时
            let read_result = if idle_timeout.is_zero() {
                buf_reader.read_line(&mut line).await
            } else {
                match tokio::time::timeout(idle_timeout, buf_reader.read_line(&mut line)).await {
                    Ok(r) => r,
                    Err(_) => {
                        tracing::warn!(
                            conn_id = self.conn_id,
                            timeout_secs = idle_timeout.as_secs(),
                            "SQLite connection idle timeout, closing"
                        );
                        break;
                    }
                }
            };

            match read_result {
                Ok(0) => {
                    // EOF：客户端断开
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(conn_id = self.conn_id, error = %e, "read error");
                    break;
                }
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // 解析 JSON 请求
            let request: serde_json::Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    let resp = serde_json::json!({"error": format!("invalid JSON: {e}")});
                    let _ = write_json_line(&mut writer, &resp).await;
                    continue;
                }
            };

            let sql = request.get("sql").and_then(|v| v.as_str()).unwrap_or("");

            if sql.is_empty() {
                let resp = serde_json::json!({"error": "missing 'sql' field"});
                let _ = write_json_line(&mut writer, &resp).await;
                continue;
            }

            // 执行 SQL
            let response = self.execute_sql(sql).await;
            let _ = write_json_line(&mut writer, &response).await;
        }

        tracing::debug!(conn_id = self.conn_id, "SQLite connection closed");
        Ok(())
    }

    /// 执行 SQL 并构造 JSON 响应。
    async fn execute_sql(&self, sql: &str) -> serde_json::Value {
        let mut executor = self.executor.lock().await;
        let results = executor.execute_sql(sql).await;

        // 单条 SQL 可能产生多个结果（如多语句拼接），取第一个
        if let Some(result) = results.into_iter().next() {
            match result {
                Ok(QueryResult::ResultSet { columns, rows, tag }) => {
                    let col_names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
                    let json_rows: Vec<serde_json::Value> = rows
                        .into_iter()
                        .map(|row| {
                            serde_json::Value::Array(row.into_iter().map(value_to_json).collect())
                        })
                        .collect();
                    return serde_json::json!({
                        "columns": col_names,
                        "rows": json_rows,
                        "tag": tag,
                    });
                }
                Ok(QueryResult::AffectedRows { tag }) => {
                    // 从 tag 提取影响行数（如 "INSERT 0 5" → 5）
                    let affected = tag
                        .split_whitespace()
                        .last()
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0);
                    return serde_json::json!({
                        "tag": tag,
                        "affected": affected,
                    });
                }
                Ok(QueryResult::DdlComplete { tag }) => {
                    return serde_json::json!({"tag": tag, "affected": 0});
                }
                Ok(QueryResult::TransactionComplete { tag, .. }) => {
                    return serde_json::json!({"tag": tag, "affected": 0});
                }
                Ok(QueryResult::Empty) => {
                    return serde_json::json!({"tag": "EMPTY", "affected": 0});
                }
                Err(e) => {
                    return serde_json::json!({"error": format_error(&e)});
                }
            }
        }

        serde_json::json!({"tag": "EMPTY", "affected": 0})
    }
}

// =====================================================================
//  辅助函数
// =====================================================================

/// 将 `Value` 转换为 JSON 值。
fn value_to_json(v: Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Int64(n) => serde_json::json!(n),
        Value::Float64(f) => {
            if f.is_finite() {
                serde_json::json!(f)
            } else {
                serde_json::Value::Null
            }
        }
        Value::Text(s) => serde_json::Value::String(s),
        Value::Blob(b) => {
            // BLOB 以 hex 字符串返回
            let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
            serde_json::Value::String(hex)
        }
        Value::Bool(b) => serde_json::json!(b),
        Value::Date(days) => serde_json::Value::String(format!("date:{days}")),
        Value::Timestamp(us) => serde_json::Value::String(format!("ts:{us}")),
        Value::Decimal(unscaled, scale) => {
            serde_json::Value::String(format!("{unscaled}e-{scale}"))
        }
        Value::Array(arr) => serde_json::Value::Array(arr.into_iter().map(value_to_json).collect()),
        Value::Enum(s) => serde_json::Value::String(s),
        Value::Range(_) => serde_json::Value::String("[range]".to_string()),
        Value::Json(v) => v,
        Value::TsVector(_) => serde_json::Value::String("[tsvector]".to_string()),
        Value::TsQuery(_) => serde_json::Value::String("[tsquery]".to_string()),
        Value::Vector(v) => serde_json::Value::String(format!("[vector({})]", v.dims())),
        Value::Xml(_) => serde_json::Value::String("[xml]".to_string()),
    }
}

/// 格式化 SessionError 为用户可读字符串。
fn format_error(e: &SessionError) -> String {
    e.to_string()
}

/// 写入一行 JSON 响应（追加换行符并 flush）。
async fn write_json_line(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    value: &serde_json::Value,
) -> Result<(), SqliteServerError> {
    let mut json = serde_json::to_string(value)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = SqliteConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9432);
        assert_eq!(config.server_version, "3.45-szrsql");
        assert_eq!(config.connection_idle_timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_config_builder() {
        let config = SqliteConfig::new()
            .with_host("0.0.0.0")
            .with_port(8080)
            .with_server_version("3.46-test")
            .with_connection_idle_timeout(Duration::from_secs(60));
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(config.server_version, "3.46-test");
        assert_eq!(config.connection_idle_timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_value_to_json_null() {
        assert_eq!(value_to_json(Value::Null), serde_json::Value::Null);
    }

    #[test]
    fn test_value_to_json_int64() {
        assert_eq!(value_to_json(Value::Int64(42)), serde_json::json!(42));
        assert_eq!(value_to_json(Value::Int64(-7)), serde_json::json!(-7));
    }

    #[test]
    fn test_value_to_json_float64() {
        assert_eq!(
            value_to_json(Value::Float64(std::f64::consts::PI)),
            serde_json::json!(std::f64::consts::PI)
        );
    }

    #[test]
    fn test_value_to_json_text() {
        assert_eq!(
            value_to_json(Value::Text("hello".to_string())),
            serde_json::Value::String("hello".to_string())
        );
    }

    #[test]
    fn test_value_to_json_bool() {
        assert_eq!(value_to_json(Value::Bool(true)), serde_json::json!(true));
        assert_eq!(value_to_json(Value::Bool(false)), serde_json::json!(false));
    }

    #[test]
    fn test_value_to_json_blob() {
        let blob = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let json = value_to_json(Value::Blob(blob));
        assert_eq!(json, serde_json::Value::String("deadbeef".to_string()));
    }

    #[test]
    fn test_value_to_json_decimal() {
        let json = value_to_json(Value::Decimal(12345, 2));
        assert_eq!(json, serde_json::Value::String("12345e-2".to_string()));
    }

    #[test]
    fn test_value_to_json_array() {
        let arr = Value::Array(vec![Value::Int64(1), Value::Int64(2), Value::Int64(3)]);
        let json = value_to_json(arr);
        assert_eq!(json, serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn test_value_to_json_float_nan() {
        // NaN / Infinity 应返回 Null（JSON 不支持）
        let json = value_to_json(Value::Float64(f64::NAN));
        assert_eq!(json, serde_json::Value::Null);
        let json = value_to_json(Value::Float64(f64::INFINITY));
        assert_eq!(json, serde_json::Value::Null);
    }

    #[tokio::test]
    async fn test_execute_sql_select() {
        let conn = Connection::new(SqliteConfig::default(), 1);
        let resp = conn.execute_sql("SELECT 1").await;
        assert!(resp.get("columns").is_some());
        assert!(resp.get("rows").is_some());
        assert_eq!(resp.get("tag").and_then(|t| t.as_str()), Some("SELECT 1"));
    }

    #[tokio::test]
    async fn test_execute_sql_create_and_insert() {
        let conn = Connection::new(SqliteConfig::default(), 1);
        let _ = conn.execute_sql("CREATE TABLE t (id INT)").await;
        let resp = conn.execute_sql("INSERT INTO t VALUES (1)").await;
        assert_eq!(resp.get("tag").and_then(|t| t.as_str()), Some("INSERT 0 1"));
        assert_eq!(resp.get("affected").and_then(|a| a.as_i64()), Some(1));
    }

    #[tokio::test]
    async fn test_execute_sql_error() {
        let conn = Connection::new(SqliteConfig::default(), 1);
        let resp = conn.execute_sql("SELECT FROM").await;
        assert!(resp.get("error").is_some());
    }

    #[tokio::test]
    async fn test_execute_sql_empty() {
        let conn = Connection::new(SqliteConfig::default(), 1);
        let resp = conn.execute_sql("").await;
        assert_eq!(resp.get("tag").and_then(|t| t.as_str()), Some("EMPTY"));
    }
}
