//! 回放引擎：在 PG 18 和 szrsql 上执行录制的 SQL 序列
//!
//! # 工作流程
//!
//! 1. 读取 JSONL 流量文件
//! 2. 初始化 PG 18 连接 + szrsql InMemoryTable
//! 3. 逐条执行 SQL：
//!    - 在 PG 18 上执行，记录结果 + 延迟
//!    - 在 szrsql 上执行，记录结果 + 延迟
//!    - 比对结果，记录状态
//! 4. 收集所有 `ReplayResult`，生成报告

use std::path::Path;
use std::time::Instant;

use szrsql_sql::executor::{Executor, InMemoryTable, TableStorage};
use szrsql_sql::parser::parse_sql;
use szrsql_sql::plan::{InMemoryCatalog, LogicalPlan, Planner};
use szrsql_types::value::{ColumnType, Value};

use crate::compare::{compare_results, MatchStatus, ReplayResult};
use crate::recorder::{Recorder, RecorderError, TrafficEntry};

/// 回放引擎错误
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// 录制器错误
    #[error("recorder error: {0}")]
    Recorder(#[from] RecorderError),
    /// PG 18 连接错误
    #[error("pg connection error: {0}")]
    PgConnection(String),
    /// szrsql 表初始化错误
    #[error("szrsql init error: {0}")]
    SzInit(String),
}

/// 回放引擎配置
#[derive(Debug, Clone)]
pub struct ReplayConfig {
    /// PG 18 连接串
    pub pg_url: String,
    /// PG 18 测试 schema（每轮回放前 DROP CASCADE 重建）
    pub pg_schema: String,
    /// 是否跳过 szrsql 执行错误（不 panic，仅记录）
    pub skip_sz_errors: bool,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            pg_url: "postgresql://postgres:postgres@127.0.0.1:5432/postgres".to_string(),
            pg_schema: "szrsql_shadow".to_string(),
            skip_sz_errors: true,
        }
    }
}

/// 回放引擎
pub struct ShadowReplay {
    config: ReplayConfig,
}

impl ShadowReplay {
    pub fn new(config: ReplayConfig) -> Self {
        Self { config }
    }

    /// 从 JSONL 文件回放流量
    ///
    /// # 流程
    /// 1. 加载 JSONL 流量
    /// 2. 连接 PG 18，初始化测试 schema
    /// 3. 初始化 szrsql InMemoryTable
    /// 4. 逐条执行 SQL，记录结果与延迟
    ///
    /// # 参数
    /// - `jsonl_path`: JSONL 流量文件路径
    /// - `table_name`: szrsql 表名（默认 "t"）
    /// - `columns`: 表结构 `[(列名, 列类型), ...]`
    pub fn replay_from_jsonl(
        &self,
        jsonl_path: &Path,
        table_name: &str,
        columns: Vec<(&str, ColumnType)>,
    ) -> Result<Vec<ReplayResult>, ReplayError> {
        let entries = Recorder::load_from_jsonl(jsonl_path)?;
        self.replay_entries(&entries, table_name, columns)
    }

    /// 在 PG 18 + szrsql 上回放流量条目
    pub fn replay_entries(
        &self,
        entries: &[TrafficEntry],
        table_name: &str,
        columns: Vec<(&str, ColumnType)>,
    ) -> Result<Vec<ReplayResult>, ReplayError> {
        // 1. 连接 PG 18
        let mut pg_client = postgres::Client::connect(&self.config.pg_url, postgres::NoTls)
            .map_err(|e| ReplayError::PgConnection(e.to_string()))?;

        // 2. 初始化 PG 18 测试 schema
        let schema = &self.config.pg_schema;
        pg_client
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE;
                 CREATE SCHEMA {schema};
                 SET search_path TO {schema};"
            ))
            .map_err(|e| ReplayError::PgConnection(e.to_string()))?;

        // 在 PG 18 创建表
        let pg_create_sql = build_pg_create_table_sql(table_name, &columns);
        pg_client
            .execute(&pg_create_sql, &[])
            .map_err(|e| ReplayError::PgConnection(e.to_string()))?;

        // 3. 初始化 szrsql 表
        let mut sz_table = InMemoryTable::with_columns(table_name, columns.clone());
        let sz_catalog = build_szrsql_catalog(table_name, columns);

        // 4. 逐条执行 SQL
        let mut results = Vec::with_capacity(entries.len());
        for entry in entries {
            let result = self.replay_one(entry, &mut pg_client, &mut sz_table, &sz_catalog);
            results.push(result);
        }

        Ok(results)
    }

    /// 执行单条 SQL
    ///
    /// 在 PG 18 和 szrsql 上各执行一次，记录延迟并比对结果。
    /// 由于 `exec_pg`/`exec_szrsql` 返回 `Vec<Vec<String>>`，行数直接从结果长度取。
    fn replay_one(
        &self,
        entry: &TrafficEntry,
        pg_client: &mut postgres::Client,
        sz_table: &mut InMemoryTable,
        sz_catalog: &InMemoryCatalog,
    ) -> ReplayResult {
        let sql = &entry.sql;

        // 1. PG 18 执行
        let pg_start = Instant::now();
        let pg_result = exec_pg(pg_client, sql);
        let pg_latency_ms = pg_start.elapsed().as_secs_f64() * 1000.0;

        // 2. szrsql 执行
        let sz_start = Instant::now();
        let sz_result = exec_szrsql(sql, sz_catalog, sz_table);
        let sz_latency_ms = sz_start.elapsed().as_secs_f64() * 1000.0;

        // 3. 比对（同时获取行数）
        let (status, pg_row_count, sz_row_count) = match (pg_result, sz_result) {
            (Err(_pg_err), Err(_sz_err)) => (MatchStatus::BothError, 0i64, 0i64),
            (Err(pg_err), Ok(_)) => (MatchStatus::PgError(pg_err), 0, 0),
            (Ok(_), Err(sz_err)) => (MatchStatus::SzError(sz_err), 0, 0),
            (Ok(pg_rows), Ok(sz_rows)) => {
                let pg_count = pg_rows.len() as i64;
                let sz_count = sz_rows.len() as i64;
                let status = compare_results(sql, &sz_rows, &pg_rows);
                (status, pg_count, sz_count)
            }
        };

        ReplayResult {
            sql: sql.clone(),
            pg_rows: pg_row_count,
            sz_rows: sz_row_count,
            pg_latency_ms,
            sz_latency_ms,
            status,
        }
    }
}

// =====================================================================
//  PG 18 执行辅助
// =====================================================================

/// 在 PG 18 上执行 SQL，返回结果集（字符串矩阵）
fn exec_pg(client: &mut postgres::Client, sql: &str) -> Result<Vec<Vec<String>>, String> {
    let trimmed = sql.trim().to_uppercase();
    if trimmed.starts_with("SELECT") {
        let rows = client
            .query(sql, &[])
            .map_err(|e| format!("pg query: {e}"))?;
        let result: Vec<Vec<String>> = rows
            .iter()
            .map(|row| (0..row.len()).map(|i| pg_cell_to_string(row, i)).collect())
            .collect();
        Ok(result)
    } else {
        client
            .execute(sql, &[])
            .map_err(|e| format!("pg execute: {e}"))?;
        Ok(Vec::new())
    }
}

/// 将 PG 行的某列转换为字符串（与 szrsql 规范一致）
fn pg_cell_to_string(row: &postgres::Row, idx: usize) -> String {
    use postgres::types::Type;
    let col_type = row.columns()[idx].type_();
    match *col_type {
        Type::INT8 => {
            let v: Option<i64> = row.get(idx);
            v.map(|n| n.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        }
        Type::INT4 => {
            let v: Option<i32> = row.get(idx);
            v.map(|n| n.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        }
        Type::TEXT | Type::VARCHAR => {
            let v: Option<String> = row.get(idx);
            v.unwrap_or_else(|| "NULL".to_string())
        }
        Type::FLOAT8 => {
            let v: Option<f64> = row.get(idx);
            v.map(|f| format!("{f:.6}"))
                .unwrap_or_else(|| "NULL".to_string())
        }
        Type::BOOL => {
            let v: Option<bool> = row.get(idx);
            v.map(|b| b.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        }
        _ => {
            let v: Option<String> = row.get(idx);
            v.unwrap_or_else(|| "NULL".to_string())
        }
    }
}

// =====================================================================
//  szrsql 执行辅助
// =====================================================================

/// 在 szrsql 上执行 SQL，返回结果集（字符串矩阵）
fn exec_szrsql(
    sql: &str,
    catalog: &InMemoryCatalog,
    table: &mut InMemoryTable,
) -> Result<Vec<Vec<String>>, String> {
    let stmts = parse_sql(sql).map_err(|e| format!("parse: {e}"))?;
    if stmts.len() != 1 {
        return Err(format!("expected 1 statement, got {}", stmts.len()));
    }
    let planner = Planner::new(catalog);
    let plan = planner
        .plan_statement(stmts.into_iter().next().unwrap())
        .map_err(|e| format!("plan: {e}"))?;

    match &plan {
        LogicalPlan::Insert { table: t, .. } if t.name == table.name() => {
            let exec = Executor::new();
            exec.execute_insert(&plan, table)
                .map_err(|e| format!("execute_insert: {e}"))?;
            Ok(Vec::new())
        }
        LogicalPlan::Update { table: t, .. } if t.name == table.name() => {
            let exec = Executor::new();
            exec.execute_update(&plan, table)
                .map_err(|e| format!("execute_update: {e}"))?;
            Ok(Vec::new())
        }
        LogicalPlan::Delete { table: t, .. } if t.name == table.name() => {
            let exec = Executor::new();
            exec.execute_delete(&plan, table)
                .map_err(|e| format!("execute_delete: {e}"))?;
            Ok(Vec::new())
        }
        _ => {
            let exec = Executor::new();
            let mut exec = exec;
            exec.register_table(table);
            let rows = exec.execute(&plan).map_err(|e| format!("execute: {e}"))?;
            let result: Vec<Vec<String>> = rows
                .iter()
                .map(|row| row.iter().map(value_to_compare_string).collect())
                .collect();
            Ok(result)
        }
    }
}

/// 将 Value 转换为比对字符串（规范化）
fn value_to_compare_string(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Int64(n) => n.to_string(),
        Value::Float64(f) => format!("{f:.6}"),
        Value::Text(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

// =====================================================================
//  建表 SQL 生成
// =====================================================================

/// 生成 PG 18 CREATE TABLE SQL
fn build_pg_create_table_sql(table_name: &str, columns: &[(&str, ColumnType)]) -> String {
    let cols: Vec<String> = columns
        .iter()
        .map(|(name, ct)| format!("{name} {}", pg_type_name(ct)))
        .collect();
    format!("CREATE TABLE {table_name} ({})", cols.join(", "))
}

/// 将 szrsql ColumnType 转为 PG 类型名
fn pg_type_name(ct: &ColumnType) -> &'static str {
    match ct {
        ColumnType::Int64 => "BIGINT",
        ColumnType::Float64 => "DOUBLE PRECISION",
        ColumnType::Text => "TEXT",
        ColumnType::Bool => "BOOLEAN",
        ColumnType::Date => "DATE",
        ColumnType::Timestamp => "TIMESTAMP",
        ColumnType::Decimal { .. } => "DECIMAL(38, 18)",
        ColumnType::Blob => "BYTEA",
        ColumnType::Json => "JSON",
        _ => "TEXT",
    }
}

/// 构建 szrsql catalog
fn build_szrsql_catalog(table_name: &str, columns: Vec<(&str, ColumnType)>) -> InMemoryCatalog {
    let mut catalog = InMemoryCatalog::new();
    catalog.add_simple_table(table_name, columns);
    catalog
}
