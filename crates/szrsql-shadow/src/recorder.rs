//! 流量录制器：将 SQL 序列序列化为 JSONL 格式
//!
//! # JSONL 格式
//!
//! 每行一个 `TrafficEntry`：
//!
//! ```json
//! {"timestamp":"2026-07-25T10:00:00Z","session_id":"sess-001","sql":"SELECT 1","params":[]}
//! ```
//!
//! # 录制源
//!
//! 1. **SQL 文件**：每行一条 SQL（`--` 开头为注释，空行跳过）
//! 2. **pg_stat_statements**：（未来扩展）从 PG 18 查询已执行的 SQL
//! 3. **慢查询日志**：（未来扩展）从 PG 18 log 解析慢查询

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use thiserror::Error;

/// 录制/回放过程中的错误
#[derive(Debug, Error)]
pub enum RecorderError {
    /// 文件 IO 错误
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// JSON 序列化/反序列化错误
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// SQL 文件解析错误（如缺少终止分号）
    #[error("sql parse error at line {line}: {reason}")]
    SqlParse { line: usize, reason: String },
}

/// 一条录制的流量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficEntry {
    /// 时间戳（ISO 8601）
    pub timestamp: String,
    /// 会话 ID（用于关联同一会话的 SQL）
    pub session_id: String,
    /// SQL 文本
    pub sql: String,
    /// 绑定参数（未来扩展，当前为空）
    #[serde(default)]
    pub params: Vec<String>,
}

impl TrafficEntry {
    /// 创建一条新的流量记录
    pub fn new(session_id: impl Into<String>, sql: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now().to_rfc3339(),
            session_id: session_id.into(),
            sql: sql.into(),
            params: Vec::new(),
        }
    }
}

/// 流量录制器
pub struct Recorder;

impl Recorder {
    /// 从 SQL 文件录制流量到 JSONL 文件
    ///
    /// SQL 文件格式：
    /// - 每行一条 SQL（以 `;` 结尾）
    /// - `--` 开头为注释，跳过
    /// - 空行跳过
    /// - 多行 SQL 用 `;` 分隔
    ///
    /// # 参数
    /// - `sql_path`: SQL 文件路径
    /// - `output_path`: 输出 JSONL 文件路径
    /// - `session_id`: 会话 ID（所有 SQL 共用一个 session_id）
    pub fn record_from_sql_file(
        sql_path: &Path,
        output_path: &Path,
        session_id: &str,
    ) -> Result<usize, RecorderError> {
        let file = std::fs::File::open(sql_path)?;
        let reader = BufReader::new(file);
        let mut output = std::fs::File::create(output_path)?;

        let mut count = 0usize;
        let mut buffer = String::new();
        for (idx, line) in reader.lines().enumerate() {
            let line = line?;
            let trimmed = line.trim();
            // 跳过空行与注释
            if trimmed.is_empty() || trimmed.starts_with("--") {
                continue;
            }
            buffer.push_str(trimmed);
            buffer.push(' ');

            // 遇到 `;` 表示一条 SQL 结束
            if buffer.contains(';') {
                let sql = buffer.trim().to_string();
                let entry = TrafficEntry::new(session_id, sql);
                let json = serde_json::to_string(&entry)?;
                writeln!(output, "{json}")?;
                count += 1;
                buffer.clear();
            }
            // 防止 lint 警告：idx 暂未使用，未来可用于错误定位
            let _ = idx;
        }

        // 处理文件末尾未以 `;` 结尾的 SQL
        if !buffer.trim().is_empty() {
            let sql = buffer.trim().to_string();
            let entry = TrafficEntry::new(session_id, sql);
            let json = serde_json::to_string(&entry)?;
            writeln!(output, "{json}")?;
            count += 1;
        }

        tracing::info!("recorded {count} SQL entries from {sql_path:?} to {output_path:?}");
        Ok(count)
    }

    /// 从 JSONL 文件读取流量
    pub fn load_from_jsonl(jsonl_path: &Path) -> Result<Vec<TrafficEntry>, RecorderError> {
        let file = std::fs::File::open(jsonl_path)?;
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: TrafficEntry = serde_json::from_str(&line)?;
            entries.push(entry);
        }
        Ok(entries)
    }

    /// 将流量列表写入 JSONL 文件
    pub fn save_to_jsonl(
        entries: &[TrafficEntry],
        output_path: &Path,
    ) -> Result<usize, RecorderError> {
        let mut output = std::fs::File::create(output_path)?;
        for entry in entries {
            let json = serde_json::to_string(entry)?;
            writeln!(output, "{json}")?;
        }
        Ok(entries.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn record_from_sql_file_basic() {
        // 准备临时 SQL 文件
        let mut sql_file = NamedTempFile::new().unwrap();
        writeln!(sql_file, "-- comment").unwrap();
        writeln!(sql_file, "SELECT 1;").unwrap();
        writeln!(sql_file, "").unwrap();
        writeln!(sql_file, "SELECT 2;").unwrap();
        writeln!(sql_file, "INSERT INTO t VALUES (1, 'a');").unwrap();
        sql_file.flush().unwrap();

        let output_file = NamedTempFile::new().unwrap();
        let count = Recorder::record_from_sql_file(
            sql_file.path(),
            output_file.path(),
            "test-session",
        )
        .unwrap();

        assert_eq!(count, 3);

        let entries = Recorder::load_from_jsonl(output_file.path()).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].sql, "SELECT 1;");
        assert_eq!(entries[1].sql, "SELECT 2;");
        assert_eq!(entries[2].sql, "INSERT INTO t VALUES (1, 'a');");
        assert_eq!(entries[0].session_id, "test-session");
    }

    #[test]
    fn record_handles_multiline_sql() {
        let mut sql_file = NamedTempFile::new().unwrap();
        writeln!(sql_file, "SELECT *").unwrap();
        writeln!(sql_file, "FROM t").unwrap();
        writeln!(sql_file, "WHERE id = 1;").unwrap();
        sql_file.flush().unwrap();

        let output_file = NamedTempFile::new().unwrap();
        let count =
            Recorder::record_from_sql_file(sql_file.path(), output_file.path(), "s1").unwrap();

        assert_eq!(count, 1);
        let entries = Recorder::load_from_jsonl(output_file.path()).unwrap();
        assert!(entries[0].sql.contains("SELECT *"));
        assert!(entries[0].sql.contains("FROM t"));
        assert!(entries[0].sql.contains("WHERE id = 1;"));
    }

    #[test]
    fn load_empty_jsonl() {
        let file = NamedTempFile::new().unwrap();
        let entries = Recorder::load_from_jsonl(file.path()).unwrap();
        assert!(entries.is_empty());
    }
}
