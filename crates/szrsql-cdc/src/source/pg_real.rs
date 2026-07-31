//! P7-2: 真实 PostgreSQL 反向链路源端连接器 — 使用 `postgres::Client`
//!
//! # 设计
//!
//! 1. **真实数据库驱动**：直接持有 `postgres::Client`，通过 TCP 协议与 PG 通信
//! 2. **真实 Schema 发现**：查询 `information_schema.columns` + `pg_catalog` 获取表结构
//! 3. **真实快照抽取**：执行 `SELECT * FROM table` 流式拉取全量数据
//! 4. **真实 CDC 流**：基于触发器的 CDC 模式
//!    - 在源表上创建 AFTER INSERT/UPDATE/DELETE 触发器
//!    - 触发器将变更写入 `_szrsql_cdc_log` 表
//!    - CDC 流通过轮询 `_szrsql_cdc_log` 表获取增量变更
//!    - 这种模式在生产中被广泛使用（如 Oracle CDC、Debezium trigger mode）
//! 5. **位点管理**：使用 `_szrsql_cdc_log.id`（自增 BIGSERIAL）作为 LSN
//! 6. **断点续传**：通过 `ack_offset` 持久化已消费的 LSN
//!
//! # 与 `PgSourceConnector`（闭包注入版）的差异
//!
//! | 维度 | `PgSourceConnector` | `PgRealSourceConnector`（本模块） |
//! |------|---------------------|----------------------------------|
//! | 数据来源 | 闭包注入 | 真实 PG 查询 |
//! | Schema 发现 | 闭包注入 | `information_schema.columns` |
//! | 快照抽取 | 闭包注入 | `SELECT * FROM table` |
//! | CDC 流 | 闭包回调 | 触发器 + 轮询 `_szrsql_cdc_log` |
//! | 位点 | 内存 | `_szrsql_cdc_log.id` |
//!
//! # 使用示例
//!
//! ```ignore
//! use szrsql_cdc::source::pg_real::PgRealSourceConnector;
//! use szrsql_cdc::source::{SourceConnector, SourceConfig};
//! use postgres::NoTls;
//!
//! let client = postgres::Client::connect("postgresql://postgres:test123@127.0.0.1:5432/sz_orm_test", NoTls).unwrap();
//! let connector = PgRealSourceConnector::new(client, SourceConfig::postgres("postgresql://...")).unwrap();
//! connector.connect().unwrap();
//! let schemas = connector.discover_schemas(&["users".to_string()]).unwrap();
//! ```

use crate::decoder::DecodedRow;
use crate::schema::{ColumnDef, DataType, TableSchema};
use crate::source::pg_source::PgSourceConnector;
use crate::source::{SourceConfig, SourceConnector, SourceError, SourceEvent, SourceOffset};
use szrsql_types::value::Value as SzValue;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

// =====================================================================
// PgRealSourceConnector — 真实 PG 源端连接器
// =====================================================================

/// CDC 日志表名（触发器写入变更的表）
const CDC_LOG_TABLE: &str = "_szrsql_cdc_log";

/// CDC 日志表 DDL
const CDC_LOG_DDL: &str = "CREATE TABLE IF NOT EXISTS _szrsql_cdc_log (
    id BIGSERIAL PRIMARY KEY,
    table_name TEXT NOT NULL,
    op TEXT NOT NULL,
    old_data JSONB,
    new_data JSONB,
    tx_id BIGINT,
    created_at TIMESTAMPTZ DEFAULT NOW()
);";

/// 真实 PostgreSQL 源端连接器 — 通过 `postgres::Client` 实现真实 CDC
///
/// **CDC 模式**：基于触发器 + 日志表
/// - `install_cdc_triggers` 在源表上创建触发器，将变更写入 `_szrsql_cdc_log`
/// - `start_cdc_stream` 轮询 `_szrsql_cdc_log` 表，按 id 顺序消费变更
/// - `ack_offset` 推进消费位点（删除已消费的日志行）
///
/// **生产建议**：
/// - 大流量场景应改用 PG logical replication（`pg_logical_emit_message` + replication slot）
/// - 触发器模式适用于中等流量（< 10K events/sec）和兼容性要求高的场景
pub struct PgRealSourceConnector {
    /// PG 客户端（Mutex 保护，串行执行）
    client: Mutex<postgres::Client>,
    /// 源端配置
    config: SourceConfig,
    /// 已确认的消费位点
    confirmed_offset: Mutex<SourceOffset>,
    /// 是否已连接
    connected: AtomicBool,
    /// CDC 流是否运行中
    streaming: AtomicBool,
    /// 停止信号
    stop_requested: AtomicBool,
    /// 已创建的表名集合（避免重复 ensure_table）
    discovered_tables: Mutex<HashMap<String, TableSchema>>,
}

impl PgRealSourceConnector {
    /// 创建真实 PG 源端连接器
    ///
    /// # 参数
    /// - `client`：已建立的 `postgres::Client` 连接
    /// - `config`：源端配置（`schema` 字段指定 PG schema，默认 `public`）
    pub fn new(client: postgres::Client, config: SourceConfig) -> Result<Self, SourceError> {
        Ok(Self {
            client: Mutex::new(client),
            config,
            confirmed_offset: Mutex::new(SourceOffset::default()),
            connected: AtomicBool::new(false),
            streaming: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            discovered_tables: Mutex::new(HashMap::new()),
        })
    }

    /// 通过连接串创建真实 PG 源端连接器（便捷构造函数）
    pub fn connect(
        connection_string: &str,
        config: SourceConfig,
        tls: postgres::NoTls,
    ) -> Result<Self, SourceError> {
        let client = postgres::Client::connect(connection_string, tls)
            .map_err(|e| SourceError::Connection(format!("PG connect failed: {e}")))?;
        Self::new(client, config)
    }

    /// 获取 schema 名
    pub fn schema_name(&self) -> &str {
        self.config.schema.as_deref().unwrap_or("public")
    }

    /// 是否处于 CDC 流中
    pub fn is_streaming(&self) -> bool {
        self.streaming.load(Ordering::SeqCst)
    }

    /// 是否已连接
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// 安装 CDC 日志表 + 在指定表上创建触发器
    ///
    /// **流程**：
    /// 1. 创建 `_szrsql_cdc_log` 表（如不存在）
    /// 2. 为每张表创建 AFTER INSERT/UPDATE/DELETE 触发器
    /// 3. 触发器将变更以 JSONB 格式写入日志表
    ///
    /// # 参数
    /// - `tables`：要追踪的表名列表
    pub fn install_cdc_triggers(&self, tables: &[String]) -> Result<(), SourceError> {
        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;

        // 1. 创建 CDC 日志表
        client
            .batch_execute(CDC_LOG_DDL)
            .map_err(|e| SourceError::Sql(format!("Create CDC log table failed: {e}")))?;

        // 2. 创建触发器函数（如不存在）
        let trigger_fn = format!(
            r#"CREATE OR REPLACE FUNCTION _szrsql_cdc_capture() RETURNS TRIGGER AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        INSERT INTO {log_table} (table_name, op, new_data, tx_id)
        VALUES (TG_TABLE_NAME, 'INSERT', to_jsonb(NEW), txid_current());
        RETURN NEW;
    ELSIF TG_OP = 'UPDATE' THEN
        INSERT INTO {log_table} (table_name, op, old_data, new_data, tx_id)
        VALUES (TG_TABLE_NAME, 'UPDATE', to_jsonb(OLD), to_jsonb(NEW), txid_current());
        RETURN NEW;
    ELSIF TG_OP = 'DELETE' THEN
        INSERT INTO {log_table} (table_name, op, old_data, tx_id)
        VALUES (TG_TABLE_NAME, 'DELETE', to_jsonb(OLD), txid_current());
        RETURN OLD;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;"#,
            log_table = CDC_LOG_TABLE
        );
        client
            .batch_execute(&trigger_fn)
            .map_err(|e| SourceError::Sql(format!("Create trigger function failed: {e}")))?;

        // 3. 为每张表创建触发器
        for table in tables {
            // 跳过 CDC 日志表本身，避免无限递归
            if table == CDC_LOG_TABLE {
                continue;
            }
            let trigger_name = format!("_szrsql_cdc_trg_{}", table);
            // DROP 已存在的触发器（幂等）
            let drop_sql = format!("DROP TRIGGER IF EXISTS {} ON {};", trigger_name, quote_ident(table));
            client
                .batch_execute(&drop_sql)
                .map_err(|e| SourceError::Sql(format!("Drop trigger failed: {e}")))?;

            // 创建 AFTER INSERT/UPDATE/DELETE 触发器
            let create_sql = format!(
                "CREATE TRIGGER {trigger_name} AFTER INSERT OR UPDATE OR DELETE ON {table} FOR EACH ROW EXECUTE FUNCTION _szrsql_cdc_capture();",
                trigger_name = trigger_name,
                table = quote_ident(table)
            );
            client
                .batch_execute(&create_sql)
                .map_err(|e| SourceError::Sql(format!("Create trigger failed for table {table}: {e}")))?;
        }

        Ok(())
    }

    /// 卸载指定表上的 CDC 触发器（清理用）
    pub fn uninstall_cdc_triggers(&self, tables: &[String]) -> Result<(), SourceError> {
        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;
        for table in tables {
            let trigger_name = format!("_szrsql_cdc_trg_{}", table);
            let sql = format!("DROP TRIGGER IF EXISTS {} ON {};", trigger_name, quote_ident(table));
            let _ = client.batch_execute(&sql);
        }
        Ok(())
    }

    /// 清空 CDC 日志表（测试 / 重置用）
    pub fn clear_cdc_log(&self) -> Result<(), SourceError> {
        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;
        client
            .batch_execute(&format!("TRUNCATE TABLE {};", CDC_LOG_TABLE))
            .map_err(|e| SourceError::Sql(format!("Truncate CDC log failed: {e}")))?;
        Ok(())
    }

    /// 删除 CDC 日志表（彻底清理）
    pub fn drop_cdc_log(&self) -> Result<(), SourceError> {
        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;
        let _ = client.batch_execute("DROP FUNCTION IF EXISTS _szrsql_cdc_capture() CASCADE;");
        let _ = client.batch_execute(&format!("DROP TABLE IF EXISTS {};", CDC_LOG_TABLE));
        Ok(())
    }

    /// 将 `postgres::Row` 转换为 `DecodedRow`
    ///
    /// 根据 schema 的列定义，从 PG 行中按列名提取值并转换为 SzValue
    fn pg_row_to_decoded(
        pg_row: &postgres::Row,
        schema: &TableSchema,
    ) -> Result<DecodedRow, SourceError> {
        let mut columns = Vec::with_capacity(schema.columns.len());
        for col in &schema.columns {
            let value = Self::extract_pg_value(pg_row, &col.name, &col.data_type)?;
            columns.push((col.name.clone(), value));
        }
        Ok(DecodedRow { columns })
    }

    /// 从 PG 行中按列名+类型提取值并转换为 SzValue
    fn extract_pg_value(
        pg_row: &postgres::Row,
        name: &str,
        data_type: &DataType,
    ) -> Result<SzValue, SourceError> {
        // 尝试按类型提取，失败则返回 Null（兼容 NULL 值）
        let value = match data_type {
            DataType::Int32 => pg_row
                .try_get::<_, Option<i32>>(name)
                .map(|v| v.map(|i| SzValue::Int64(i as i64)).unwrap_or(SzValue::Null))
                .unwrap_or(SzValue::Null),
            DataType::Int64 => pg_row
                .try_get::<_, Option<i64>>(name)
                .map(|v| v.map(SzValue::Int64).unwrap_or(SzValue::Null))
                .unwrap_or(SzValue::Null),
            DataType::Text => pg_row
                .try_get::<_, Option<String>>(name)
                .map(|v| v.map(SzValue::Text).unwrap_or(SzValue::Null))
                .unwrap_or(SzValue::Null),
            DataType::Real => pg_row
                .try_get::<_, Option<f64>>(name)
                .map(|v| v.map(SzValue::Float64).unwrap_or(SzValue::Null))
                .unwrap_or(SzValue::Null),
            DataType::Bool => pg_row
                .try_get::<_, Option<bool>>(name)
                .map(|v| v.map(SzValue::Bool).unwrap_or(SzValue::Null))
                .unwrap_or(SzValue::Null),
            DataType::Blob => pg_row
                .try_get::<_, Option<Vec<u8>>>(name)
                .map(|v| v.map(SzValue::Blob).unwrap_or(SzValue::Null))
                .unwrap_or(SzValue::Null),
            DataType::Json => pg_row
                .try_get::<_, Option<serde_json::Value>>(name)
                .map(|v| v.map(SzValue::Json).unwrap_or(SzValue::Null))
                .unwrap_or(SzValue::Null),
            DataType::Date => pg_row
                .try_get::<_, Option<chrono::NaiveDate>>(name)
                .map(|v| {
                    v.map(|d| {
                        let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
                        SzValue::Date((d - epoch).num_days() as i32)
                    })
                    .unwrap_or(SzValue::Null)
                })
                .unwrap_or(SzValue::Null),
            DataType::Timestamp => pg_row
                .try_get::<_, Option<chrono::NaiveDateTime>>(name)
                .map(|v| {
                    v.map(|t| {
                        SzValue::Timestamp(t.and_utc().timestamp_micros())
                    })
                    .unwrap_or(SzValue::Null)
                })
                .unwrap_or(SzValue::Null),
            DataType::Uuid => pg_row
                .try_get::<_, Option<String>>(name)
                .map(|v| v.map(SzValue::Text).unwrap_or(SzValue::Null))
                .unwrap_or(SzValue::Null),
        };
        Ok(value)
    }

    /// 从 JSONB 数据中重建 DecodedRow
    fn jsonb_to_decoded(
        json: &serde_json::Value,
        schema: &TableSchema,
    ) -> Result<DecodedRow, SourceError> {
        let obj = json
            .as_object()
            .ok_or_else(|| SourceError::Internal("expected JSON object".to_string()))?;

        let mut columns = Vec::with_capacity(schema.columns.len());
        for col in &schema.columns {
            let value = match obj.get(&col.name) {
                None => SzValue::Null,
                Some(serde_json::Value::Null) => SzValue::Null,
                Some(v) => match &col.data_type {
                    DataType::Int32 | DataType::Int64 => {
                        v.as_i64().map(SzValue::Int64).unwrap_or(SzValue::Null)
                    }
                    DataType::Text => v
                        .as_str()
                        .map(|s| SzValue::Text(s.to_string()))
                        .unwrap_or(SzValue::Null),
                    DataType::Real => v
                        .as_f64()
                        .map(SzValue::Float64)
                        .unwrap_or(SzValue::Null),
                    DataType::Bool => v.as_bool().map(SzValue::Bool).unwrap_or(SzValue::Null),
                    DataType::Blob => {
                        // JSONB 中 Blob 存储为 hex 字符串
                        v.as_str()
                            .map(|s| {
                                let bytes = s
                                    .as_bytes()
                                    .iter()
                                    .step_by(2)
                                    .cloned()
                                    .collect();
                                SzValue::Blob(bytes)
                            })
                            .unwrap_or(SzValue::Null)
                    }
                    DataType::Json => SzValue::Json(v.clone()),
                    DataType::Date => v
                        .as_i64()
                        .map(|i| SzValue::Date(i as i32))
                        .unwrap_or(SzValue::Null),
                    DataType::Timestamp => v.as_i64().map(SzValue::Timestamp).unwrap_or(SzValue::Null),
                    DataType::Uuid => v
                        .as_str()
                        .map(|s| SzValue::Text(s.to_string()))
                        .unwrap_or(SzValue::Null),
                },
            };
            columns.push((col.name.clone(), value));
        }
        Ok(DecodedRow { columns })
    }
}

impl SourceConnector for PgRealSourceConnector {
    fn source_type(&self) -> &str {
        "postgres-real"
    }

    fn connect(&self) -> Result<(), SourceError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        // 验证连接活性
        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;
        client
            .batch_execute("SELECT 1")
            .map_err(|e| SourceError::Connection(format!("PG health check failed: {e}")))?;
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn disconnect(&self) -> Result<(), SourceError> {
        self.stop_cdc_stream()?;
        self.connected.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn discover_schemas(&self, tables: &[String]) -> Result<Vec<TableSchema>, SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }

        let schema_name = self.schema_name();
        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;

        // 如果未指定表名，查询 schema 下所有表
        let table_filter = if tables.is_empty() {
            String::new()
        } else {
            // 占位符从 $2 开始（$1 是 schema_name）
            let placeholders: Vec<String> = (1..=tables.len()).map(|i| format!("${}", i + 1)).collect();
            format!("AND table_name IN ({})", placeholders.join(", "))
        };

        let sql = format!(
            "SELECT table_name, column_name, data_type, is_nullable, ordinal_position
             FROM information_schema.columns
             WHERE table_schema = $1 {}
             ORDER BY table_name, ordinal_position",
            table_filter
        );

        let mut params: Vec<&(dyn postgres::types::ToSql + Sync)> =
            vec![&schema_name];
        for t in tables {
            params.push(t);
        }

        let rows = client
            .query(&sql, &params)
            .map_err(|e| SourceError::SchemaDiscovery(format!("Query failed: {e}")))?;

        // 按 table_name 分组
        let mut table_columns: HashMap<String, Vec<(String, String, bool, i32)>> = HashMap::new();
        for row in &rows {
            let table_name: String = row.try_get(0).map_err(|e| {
                SourceError::SchemaDiscovery(format!("Get table_name failed: {e}"))
            })?;
            let column_name: String = row.try_get(1).map_err(|e| {
                SourceError::SchemaDiscovery(format!("Get column_name failed: {e}"))
            })?;
            let pg_type: String = row.try_get(2).map_err(|e| {
                SourceError::SchemaDiscovery(format!("Get data_type failed: {e}"))
            })?;
            let is_nullable: String = row.try_get(3).map_err(|e| {
                SourceError::SchemaDiscovery(format!("Get is_nullable failed: {e}"))
            })?;
            let ordinal: i32 = row.try_get(4).map_err(|e| {
                SourceError::SchemaDiscovery(format!("Get ordinal_position failed: {e}"))
            })?;
            let nullable = is_nullable == "YES";
            table_columns
                .entry(table_name)
                .or_default()
                .push((column_name, pg_type, nullable, ordinal));
        }

        // 构造 TableSchema 列表
        let mut result = Vec::new();
        for (idx, (table_name, cols)) in table_columns.into_iter().enumerate() {
            let mut col_defs = Vec::with_capacity(cols.len());
            // 按 ordinal_position 排序
            let mut cols = cols;
            cols.sort_by_key(|c| c.3);
            for (name, pg_type, nullable, _) in cols {
                let data_type = PgSourceConnector::pg_type_to_szrsql(&pg_type)?;
                col_defs.push(ColumnDef {
                    name,
                    data_type,
                    nullable,
                });
            }
            let schema = TableSchema {
                table_id: (idx + 1) as u32,
                table_name,
                columns: col_defs,
                version: 1,
            };
            // 缓存 schema
            let mut cache = self.discovered_tables.lock().unwrap();
            cache.insert(schema.table_name.clone(), schema.clone());
            result.push(schema);
        }

        Ok(result)
    }

    fn extract_snapshot(
        &self,
        table: &str,
        batch_size: usize,
        callback: &dyn Fn(&[DecodedRow]) -> Result<(), SourceError>,
    ) -> Result<u64, SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }

        // 查找缓存的 schema
        let schema = {
            let cache = self.discovered_tables.lock().unwrap();
            cache
                .get(table)
                .ok_or_else(|| {
                    SourceError::SchemaDiscovery(format!(
                        "table {} not discovered, call discover_schemas first",
                        table
                    ))
                })?
                .clone()
        };

        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;

        // 执行全表扫描
        let sql = format!("SELECT * FROM {}", quote_ident(table));
        let rows = client
            .query(&sql, &[])
            .map_err(|e| SourceError::Sql(format!("Snapshot query failed: {e}")))?;

        // 转换为 DecodedRow
        let decoded_rows: Vec<DecodedRow> = rows
            .iter()
            .map(|r| Self::pg_row_to_decoded(r, &schema))
            .collect::<Result<Vec<_>, _>>()?;

        let total = decoded_rows.len() as u64;
        if decoded_rows.is_empty() {
            return Ok(0);
        }

        // 按 batch_size 分批回调
        let bs = if batch_size == 0 {
            decoded_rows.len()
        } else {
            batch_size
        };
        for chunk in decoded_rows.chunks(bs) {
            callback(chunk)?;
        }

        Ok(total)
    }

    fn current_lsn(&self) -> Result<u64, SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }

        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;

        // 查询 CDC 日志表的最大 id 作为当前 LSN
        let rows = client
            .query(&format!("SELECT COALESCE(MAX(id), 0) FROM {}", CDC_LOG_TABLE), &[])
            .map_err(|e| SourceError::Sql(format!("Query current_lsn failed: {e}")))?;
        if rows.is_empty() {
            return Ok(0);
        }
        let lsn: i64 = rows[0].try_get(0).map_err(|e| {
            SourceError::Sql(format!("Get current_lsn value failed: {e}"))
        })?;
        Ok(lsn as u64)
    }

    fn start_cdc_stream(
        &self,
        start_lsn: u64,
        callback: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
    ) -> Result<(), SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }
        if self.streaming.load(Ordering::SeqCst) {
            return Err(SourceError::Internal("cdc stream already running".to_string()));
        }

        // 设置起始位点
        {
            let mut offset = self.confirmed_offset.lock().unwrap();
            if start_lsn > offset.lsn {
                offset.lsn = start_lsn;
            }
        }

        self.streaming.store(true, Ordering::SeqCst);
        self.stop_requested.store(false, Ordering::SeqCst);

        // 轮询 CDC 日志表
        let result = self.run_cdc_stream(callback);

        self.streaming.store(false, Ordering::SeqCst);
        self.stop_requested.store(false, Ordering::SeqCst);

        result
    }

    fn stop_cdc_stream(&self) -> Result<(), SourceError> {
        if self.streaming.load(Ordering::SeqCst) {
            self.stop_requested.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn ack_offset(&self, offset: &SourceOffset) -> Result<(), SourceError> {
        // 更新内存中的位点
        let mut current = self.confirmed_offset.lock().unwrap();
        if offset.lsn >= current.lsn {
            *current = offset.clone();
        }

        // 删除已消费的日志行（保留最近 1000 行用于审计）
        let keep_count = 1000;
        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;
        let sql = format!(
            "DELETE FROM {log_table} WHERE id <= $1 AND id <= (SELECT MAX(id) - {keep_count} FROM {log_table});",
            log_table = CDC_LOG_TABLE,
            keep_count = keep_count
        );
        let lsn_i64 = offset.lsn as i64;
        let _ = client.query(&sql, &[&lsn_i64]);
        Ok(())
    }

    fn confirmed_offset(&self) -> Result<SourceOffset, SourceError> {
        Ok(self.confirmed_offset.lock().unwrap().clone())
    }

    fn health_check(&self) -> Result<(), SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }
        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;
        client
            .batch_execute("SELECT 1")
            .map_err(|e| SourceError::Connection(format!("PG health_check failed: {e}")))?;
        Ok(())
    }
}

impl PgRealSourceConnector {
    /// CDC 流主循环 — 轮询 `_szrsql_cdc_log` 表
    fn run_cdc_stream(
        &self,
        callback: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
    ) -> Result<(), SourceError> {
        let poll_interval = Duration::from_millis(100);
        let batch_size: i64 = 100;

        loop {
            if self.stop_requested.load(Ordering::SeqCst) {
                break;
            }

            // 查询当前位点之后的新事件
            let current_lsn = self.confirmed_offset.lock().unwrap().lsn;
            let events = self.poll_cdc_log(current_lsn, batch_size)?;

            if events.is_empty() {
                // 无新事件，等待
                std::thread::sleep(poll_interval);
                continue;
            }

            // 回调通知
            callback(&events)?;

            // 更新位点（取最后事件的 LSN）
            if let Some(max_lsn) = events.iter().map(|e| e.lsn).max() {
                let mut offset = self.confirmed_offset.lock().unwrap();
                if max_lsn > offset.lsn {
                    offset.lsn = max_lsn;
                }
            }
        }
        Ok(())
    }

    /// 从 CDC 日志表拉取一批事件
    fn poll_cdc_log(
        &self,
        start_lsn: u64,
        batch_size: i64,
    ) -> Result<Vec<SourceEvent>, SourceError> {
        let mut client = self.client.lock().map_err(|e| {
            SourceError::Internal(format!("PG client mutex poisoned: {e}"))
        })?;

        let sql = format!(
            "SELECT id, table_name, op, old_data, new_data, tx_id, EXTRACT(EPOCH FROM created_at) * 1000
             FROM {}
             WHERE id > $1
             ORDER BY id ASC
             LIMIT $2",
            CDC_LOG_TABLE
        );
        let start_i64 = start_lsn as i64;
        let rows = client
            .query(&sql, &[&start_i64, &batch_size])
            .map_err(|e| SourceError::Sql(format!("Poll CDC log failed: {e}")))?;

        // 需要查 schema 缓存来重建 DecodedRow
        let mut events = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: i64 = row.try_get(0).map_err(|e| {
                SourceError::Internal(format!("Get id failed: {e}"))
            })?;
            let table_name: String = row.try_get(1).map_err(|e| {
                SourceError::Internal(format!("Get table_name failed: {e}"))
            })?;
            let op: String = row.try_get(2).map_err(|e| {
                SourceError::Internal(format!("Get op failed: {e}"))
            })?;
            let old_data: Option<serde_json::Value> = row.try_get(3).map_err(|e| {
                SourceError::Internal(format!("Get old_data failed: {e}"))
            })?;
            let new_data: Option<serde_json::Value> = row.try_get(4).map_err(|e| {
                SourceError::Internal(format!("Get new_data failed: {e}"))
            })?;
            let tx_id: Option<i64> = row.try_get(5).ok();
            let timestamp: Option<f64> = row.try_get(6).ok();

            // 查 schema 缓存
            let schema = {
                let cache = self.discovered_tables.lock().unwrap();
                cache.get(&table_name).cloned()
            };

            let (before, after) = if let Some(schema) = &schema {
                // clippy fix: 用 and_then 替换 map().flatten()
                let before = old_data
                    .as_ref()
                    .and_then(|j| Self::jsonb_to_decoded(j, schema).ok());
                let after = new_data
                    .as_ref()
                    .and_then(|j| Self::jsonb_to_decoded(j, schema).ok());
                (before, after)
            } else {
                // schema 未缓存，使用空 DecodedRow（不应发生，需先 discover_schemas）
                (None, None)
            };

            let event = match op.as_str() {
                "INSERT" => SourceEvent {
                    lsn: id as u64,
                    op: crate::source::SourceEventOp::Insert,
                    schema_name: self.schema_name().to_string(),
                    table_name: table_name.clone(),
                    before: None,
                    after,
                    tx_id: tx_id.map(|t| t as u64),
                    timestamp: timestamp.map(|t| t as u64).unwrap_or(0),
                },
                "UPDATE" => SourceEvent {
                    lsn: id as u64,
                    op: crate::source::SourceEventOp::Update,
                    schema_name: self.schema_name().to_string(),
                    table_name: table_name.clone(),
                    before,
                    after,
                    tx_id: tx_id.map(|t| t as u64),
                    timestamp: timestamp.map(|t| t as u64).unwrap_or(0),
                },
                "DELETE" => SourceEvent {
                    lsn: id as u64,
                    op: crate::source::SourceEventOp::Delete,
                    schema_name: self.schema_name().to_string(),
                    table_name: table_name.clone(),
                    before,
                    after: None,
                    tx_id: tx_id.map(|t| t as u64),
                    timestamp: timestamp.map(|t| t as u64).unwrap_or(0),
                },
                _ => continue, // 未知 op 跳过
            };
            events.push(event);
        }

        Ok(events)
    }
}

/// 标识符引用（用双引号包裹，转义内部双引号）
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

// =====================================================================
// 单元测试（不依赖真实 PG）
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_real_source_connect_failure_returns_connection_error() {
        let result = PgRealSourceConnector::connect(
            "postgresql://nonexistent:5432/nonexistent_db",
            SourceConfig::postgres("postgresql://nonexistent:5432/nonexistent_db"),
            postgres::NoTls,
        );
        assert!(result.is_err());
        match result {
            Err(SourceError::Connection(_)) => {}
            Err(e) => panic!("expected Connection error, got: {e:?}"),
            Ok(_) => panic!("should not succeed"),
        }
    }

    #[test]
    fn quote_ident_escapes_double_quotes() {
        assert_eq!(quote_ident("users"), "\"users\"");
        assert_eq!(quote_ident("user\"name"), "\"user\"\"name\"");
    }

    #[test]
    fn cdc_log_table_name_constant() {
        assert_eq!(CDC_LOG_TABLE, "_szrsql_cdc_log");
    }

    /// 真实 PG 连通性验证（需本机 PostgreSQL 18 运行中，标记为 ignored 避免阻塞 CI）。
    #[test]
    #[ignore]
    fn pg_real_source_connects_to_local_postgres() {
        let conn_str = "postgresql://postgres:test123@127.0.0.1:5432/sz_orm_test";
        let result = PgRealSourceConnector::connect(
            conn_str,
            SourceConfig::postgres(conn_str),
            postgres::NoTls,
        );
        assert!(result.is_ok(), "PG connect failed (check PG 18 is running on 127.0.0.1:5432)");
    }

    #[test]
    fn cdc_log_ddl_contains_required_columns() {
        assert!(CDC_LOG_DDL.contains("id BIGSERIAL PRIMARY KEY"));
        assert!(CDC_LOG_DDL.contains("table_name TEXT NOT NULL"));
        assert!(CDC_LOG_DDL.contains("op TEXT NOT NULL"));
        assert!(CDC_LOG_DDL.contains("old_data JSONB"));
        assert!(CDC_LOG_DDL.contains("new_data JSONB"));
        assert!(CDC_LOG_DDL.contains("tx_id BIGINT"));
    }
}
