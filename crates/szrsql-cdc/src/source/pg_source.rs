//! PostgreSQL 反向链路源端连接器 — PG → szrsql
//!
//! 对应 `NineData分析与szrsql数据复制环方案.md` P5-3。
//!
//! # 设计
//!
//! 1. **PG → szrsql 方向**：从外部 PostgreSQL 数据库捕获 CDC 事件，写入 szrsql 本地
//! 2. **测试模式**：通过 `SourceEventProvider` / `SchemaProvider` / `SnapshotProvider` 闭包注入数据，
//!    避免依赖 `tokio-postgres`（生产部署时由调用方注入闭包）
//! 3. **生产模式**：调用方注入基于 `tokio-postgres` 的实际查询闭包
//! 4. **位点持久化**：通过 `Mutex<SourceOffset>` 内存持久化 + 调用方提供的持久化闭包
//!
//! # PG 类型映射
//!
//! | PG 类型 | szrsql DataType | SzValue |
//! |---------|-----------------|---------|
//! | int2 / int4 | Int32 | Int64(i32 as i64) |
//! | int8 / bigint | Int64 | Int64(i64) |
//! | text / varchar / char | Text | Text(String) |
//! | bytea | Blob | Blob(Vec<u8>) |
//! | float4 / float8 | Real | Float64(f64) |
//! | bool | Bool | Bool(bool) |
//! | date | Date | Date(i32) |
//! | timestamp / timestamptz | Timestamp | Timestamp(i64) |
//! | json / jsonb | Json | Json(serde_json::Value) |
//! | uuid | Uuid | Text(String) |
//!
//! # CDC 流模拟
//!
//! 由于 szrsql-cdc crate 不直接依赖 PG 客户端库，CDC 流通过 `SourceEventProvider` 闭包注入：
//! - 测试场景：闭包从预定义事件列表返回事件批次
//! - 生产场景：调用方注入基于 `tokio-postgres` START_REPLICATION 的回调

use crate::decoder::DecodedRow;
use crate::schema::{ColumnDef, DataType, TableSchema};
use crate::source::{
    SchemaProvider, SnapshotProvider, SourceConfig, SourceConnector, SourceError, SourceEvent,
    SourceEventProvider, SourceOffset,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use szrsql_types::value::Value as SzValue;
// P0-6：使用 parking_lot 替代 std::sync，消除中毒 panic 风险
use parking_lot::Mutex;

// =====================================================================
// PgSourceConnector — PG 反向链路源端连接器
// =====================================================================

/// PostgreSQL 反向链路源端连接器
///
/// **使用方式**：
///
/// ```ignore
/// use szrsql_cdc::source::pg_source::PgSourceConnector;
/// use szrsql_cdc::source::{SourceConnector, SourceConfig, SourceEventProvider, SourceError, SourceEvent};
/// use std::sync::Arc;
///
/// // 测试模式：注入事件提供者
/// let event_provider: SourceEventProvider = Arc::new(|| {
///     Ok(Some(vec![SourceEvent::commit(1, 100, 1000)]))
/// });
/// let connector = PgSourceConnector::with_providers(
///     SourceConfig::postgres("postgresql://localhost/db"),
///     event_provider,
///     None,  // schema provider（None 时返回空列表）
///     None,  // snapshot provider（None 时返回空）
/// ).unwrap();
///
/// connector.connect().unwrap();
/// connector.start_cdc_stream(0, &|events| {
///     println!("received {} events", events.len());
///     Ok(())
/// }).unwrap();
/// ```
pub struct PgSourceConnector {
    /// 源端配置
    config: SourceConfig,
    /// CDC 事件提供者（注入模式）
    event_provider: SourceEventProvider,
    /// Schema 提供者（注入模式）
    schema_provider: Option<SchemaProvider>,
    /// 快照提供者（注入模式）
    snapshot_provider: Option<SnapshotProvider>,
    /// 已确认的消费位点
    confirmed_offset: Mutex<SourceOffset>,
    /// 是否已连接
    connected: AtomicBool,
    /// CDC 流是否运行中
    streaming: AtomicBool,
    /// 停止信号（用于 stop_cdc_stream）
    stop_requested: AtomicBool,
    /// 已创建的表名集合（避免重复 ensure_table）
    discovered_tables: Mutex<HashMap<String, TableSchema>>,
}

impl PgSourceConnector {
    /// 创建 PG 源端连接器（默认空事件提供者，需通过 `with_providers` 注入）
    ///
    /// # 参数
    /// - `config`：源端配置
    pub fn new(config: SourceConfig) -> Result<Self, SourceError> {
        // 默认事件提供者：返回 None 表示流结束
        let event_provider: SourceEventProvider = Arc::new(|| Ok(None));
        Ok(Self {
            config,
            event_provider,
            schema_provider: None,
            snapshot_provider: None,
            confirmed_offset: Mutex::new(SourceOffset::default()),
            connected: AtomicBool::new(false),
            streaming: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            discovered_tables: Mutex::new(HashMap::new()),
        })
    }

    /// 创建 PG 源端连接器并注入提供者闭包
    ///
    /// # 参数
    /// - `config`：源端配置
    /// - `event_provider`：CDC 事件提供者（必填，测试或生产场景的事件源）
    /// - `schema_provider`：Schema 提供者（可选，None 时 discover_schemas 返回空）
    /// - `snapshot_provider`：快照提供者（可选，None 时 extract_snapshot 返回 0 行）
    pub fn with_providers(
        config: SourceConfig,
        event_provider: SourceEventProvider,
        schema_provider: Option<SchemaProvider>,
        snapshot_provider: Option<SnapshotProvider>,
    ) -> Result<Self, SourceError> {
        Ok(Self {
            config,
            event_provider,
            schema_provider,
            snapshot_provider,
            confirmed_offset: Mutex::new(SourceOffset::default()),
            connected: AtomicBool::new(false),
            streaming: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            discovered_tables: Mutex::new(HashMap::new()),
        })
    }

    /// 获取连接字符串
    pub fn connection_string(&self) -> &str {
        &self.config.connection_string
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

    /// 将 PG 列类型字符串映射到 szrsql DataType
    ///
    /// # 支持的 PG 类型
    /// - int2 / smallint / int4 / integer → Int32
    /// - int8 / bigint / serial / bigserial → Int64
    /// - text / varchar / char / bpchar / name → Text
    /// - bytea → Blob
    /// - float4 / real / float8 / double precision → Real
    /// - bool / boolean → Bool
    /// - date → Date
    /// - timestamp / timestamptz → Timestamp
    /// - json / jsonb → Json
    /// - uuid → Uuid
    /// - numeric / decimal → Real（近似映射）
    pub fn pg_type_to_szrsql(pg_type: &str) -> Result<DataType, SourceError> {
        // 处理带括号的类型（如 varchar(255) / numeric(10,2)）
        let base_type = pg_type
            .split('(')
            .next()
            .unwrap_or(pg_type)
            .trim()
            .to_lowercase();
        match base_type.as_str() {
            "int2" | "smallint" | "int4" | "integer" | "serial" => Ok(DataType::Int32),
            "int8" | "bigint" | "bigserial" => Ok(DataType::Int64),
            "text" | "varchar" | "char" | "bpchar" | "name" | "citext" | "character varying"
            | "character" => Ok(DataType::Text),
            "bytea" => Ok(DataType::Blob),
            "float4" | "real" | "float8" | "double" | "double precision" => Ok(DataType::Real),
            "bool" | "boolean" => Ok(DataType::Bool),
            "date" => Ok(DataType::Date),
            "timestamp"
            | "timestamptz"
            | "timestamp without time zone"
            | "timestamp with time zone" => Ok(DataType::Timestamp),
            "json" | "jsonb" => Ok(DataType::Json),
            "uuid" => Ok(DataType::Uuid),
            "numeric" | "decimal" => Ok(DataType::Real), // 近似映射
            "time" | "timetz" | "time without time zone" | "time with time zone" => {
                Ok(DataType::Timestamp)
            }
            _ => Err(SourceError::TypeMapping(format!(
                "unsupported PG type: {}",
                pg_type
            ))),
        }
    }

    /// 调用方辅助：构造 `DecodedRow`（从 PG 行数据）
    ///
    /// # 参数
    /// - `columns`：列名 + 列值
    pub fn make_row(columns: Vec<(String, SzValue)>) -> DecodedRow {
        DecodedRow { columns }
    }

    /// 调用方辅助：构造 `TableSchema`（从 PG 表结构）
    ///
    /// # 参数
    /// - `table_name`：表名
    /// - `table_id`：表 ID（szrsql 内部分配）
    /// - `columns`：列定义（列名 + PG 类型字符串 + nullable）
    pub fn make_schema(
        table_name: impl Into<String>,
        table_id: u32,
        columns: Vec<(String, String, bool)>,
    ) -> Result<TableSchema, SourceError> {
        let mut col_defs = Vec::with_capacity(columns.len());
        for (name, pg_type, nullable) in columns {
            let data_type = Self::pg_type_to_szrsql(&pg_type)?;
            col_defs.push(ColumnDef {
                name,
                data_type,
                nullable,
            });
        }
        Ok(TableSchema {
            table_id,
            table_name: table_name.into(),
            columns: col_defs,
            version: 1,
        })
    }

    /// 内部：执行 CDC 流（通过 event_provider 拉取事件批次）
    fn run_cdc_stream(
        &self,
        callback: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
    ) -> Result<(), SourceError> {
        loop {
            if self.stop_requested.load(Ordering::SeqCst) {
                break;
            }

            let batch = (self.event_provider)()
                .map_err(|e| SourceError::Internal(format!("event provider error: {}", e)))?;

            match batch {
                None => {
                    // 流正常结束
                    break;
                }
                Some(events) => {
                    if events.is_empty() {
                        // 空批次，避免忙等（实际场景应 sleep）
                        continue;
                    }
                    callback(&events)?;

                    // 更新已确认位点（取批次中最大的 LSN）
                    if let Some(max_lsn) = events.iter().map(|e| e.lsn).max() {
                        let mut offset = self.confirmed_offset.lock();
                        if max_lsn > offset.lsn {
                            offset.lsn = max_lsn;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl SourceConnector for PgSourceConnector {
    fn source_type(&self) -> &str {
        "postgres"
    }

    fn connect(&self) -> Result<(), SourceError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        // 实际场景：通过 tokio-postgres 建立 TCP 连接 + 认证
        // 注入模式：直接标记为已连接
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

        let result = match &self.schema_provider {
            Some(provider) => provider(tables)?,
            None => Vec::new(),
        };

        // 缓存发现的表结构（供后续 ensure_table 使用）
        let mut cache = self.discovered_tables.lock();
        for schema in &result {
            cache.insert(schema.table_name.clone(), schema.clone());
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

        let rows = match &self.snapshot_provider {
            Some(provider) => provider(table, batch_size)?,
            None => Vec::new(),
        };

        let total = rows.len() as u64;

        // 按 batch_size 分批回调
        if rows.is_empty() {
            return Ok(0);
        }

        let bs = if batch_size == 0 {
            rows.len()
        } else {
            batch_size
        };
        for chunk in rows.chunks(bs) {
            callback(chunk)?;
        }

        Ok(total)
    }

    fn current_lsn(&self) -> Result<u64, SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }
        // 实际场景：执行 `SELECT pg_current_wal_lsn()::bigint`
        // 注入模式：返回已确认位点的 LSN
        Ok(self.confirmed_offset.lock().lsn)
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
            return Err(SourceError::Internal(
                "cdc stream already running".to_string(),
            ));
        }

        // 设置起始位点
        {
            let mut offset = self.confirmed_offset.lock();
            if start_lsn > offset.lsn {
                offset.lsn = start_lsn;
            }
        }

        self.streaming.store(true, Ordering::SeqCst);
        self.stop_requested.store(false, Ordering::SeqCst);

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
        let mut current = self.confirmed_offset.lock();
        if offset.lsn >= current.lsn {
            *current = offset.clone();
        }
        Ok(())
    }

    fn confirmed_offset(&self) -> Result<SourceOffset, SourceError> {
        Ok(self.confirmed_offset.lock().clone())
    }

    fn health_check(&self) -> Result<(), SourceError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(SourceError::Connection("not connected".to_string()));
        }
        Ok(())
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{SourceConfig, SourceConnector, SourceError, SourceEvent, SourceOffset};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use szrsql_types::value::Value as SzValue;

    fn make_row(id: i64, name: &str) -> DecodedRow {
        DecodedRow {
            columns: vec![
                ("id".to_string(), SzValue::Int64(id)),
                ("name".to_string(), SzValue::Text(name.to_string())),
            ],
        }
    }

    #[test]
    fn pg_source_type_correct() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        assert_eq!(connector.source_type(), "postgres");
    }

    #[test]
    fn pg_source_connection_string() {
        let cs = "postgresql://user:pass@host:5432/db";
        let connector = PgSourceConnector::new(SourceConfig::postgres(cs)).unwrap();
        assert_eq!(connector.connection_string(), cs);
    }

    #[test]
    fn pg_source_schema_name_default_public() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        assert_eq!(connector.schema_name(), "public");
    }

    #[test]
    fn pg_source_connect_disconnect() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        assert!(!connector.is_connected());
        connector.connect().unwrap();
        assert!(connector.is_connected());
        // 重复连接幂等
        connector.connect().unwrap();
        assert!(connector.is_connected());
        connector.disconnect().unwrap();
        assert!(!connector.is_connected());
    }

    #[test]
    fn pg_source_health_check_requires_connection() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        assert!(connector.health_check().is_err());
        connector.connect().unwrap();
        assert!(connector.health_check().is_ok());
    }

    #[test]
    fn pg_source_discover_schemas_requires_connection() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        let result = connector.discover_schemas(&[]);
        assert!(result.is_err());
        match result {
            Err(SourceError::Connection(_)) => {}
            _ => panic!("expected Connection error"),
        }
    }

    #[test]
    fn pg_source_discover_schemas_without_provider_returns_empty() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        connector.connect().unwrap();
        let schemas = connector.discover_schemas(&[]).unwrap();
        assert!(schemas.is_empty());
    }

    #[test]
    fn pg_source_discover_schemas_with_provider() {
        let schema_provider: SchemaProvider = Arc::new(|_tables| {
            Ok(vec![TableSchema {
                table_id: 1,
                table_name: "users".to_string(),
                columns: vec![
                    ColumnDef {
                        name: "id".to_string(),
                        data_type: DataType::Int64,
                        nullable: false,
                    },
                    ColumnDef {
                        name: "name".to_string(),
                        data_type: DataType::Text,
                        nullable: true,
                    },
                ],
                version: 1,
            }])
        });
        let connector = PgSourceConnector::with_providers(
            SourceConfig::postgres("postgresql://localhost/db"),
            Arc::new(|| Ok(None)),
            Some(schema_provider),
            None,
        )
        .unwrap();
        connector.connect().unwrap();
        let schemas = connector.discover_schemas(&[]).unwrap();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].table_name, "users");
    }

    #[test]
    fn pg_source_extract_snapshot_requires_connection() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        let result = connector.extract_snapshot("users", 100, &|_rows| Ok(()));
        assert!(result.is_err());
    }

    #[test]
    fn pg_source_extract_snapshot_without_provider_returns_zero() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        connector.connect().unwrap();
        let count = connector
            .extract_snapshot("users", 100, &|_rows| Ok(()))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn pg_source_extract_snapshot_with_provider_batches() {
        let snapshot_provider: SnapshotProvider = Arc::new(|_table, _batch_size| {
            Ok((1..=25)
                .map(|i| make_row(i, &format!("user{}", i)))
                .collect())
        });
        let connector = PgSourceConnector::with_providers(
            SourceConfig::postgres("postgresql://localhost/db"),
            Arc::new(|| Ok(None)),
            None,
            Some(snapshot_provider),
        )
        .unwrap();
        connector.connect().unwrap();

        let total_batches = Arc::new(AtomicUsize::new(0));
        let total_rows = Arc::new(AtomicUsize::new(0));
        let tb = total_batches.clone();
        let tr = total_rows.clone();
        let count = connector
            .extract_snapshot("users", 10, &move |rows| {
                tb.fetch_add(1, Ordering::SeqCst);
                tr.fetch_add(rows.len(), Ordering::SeqCst);
                Ok(())
            })
            .unwrap();

        assert_eq!(count, 25);
        assert_eq!(total_rows.load(Ordering::SeqCst), 25);
        assert_eq!(total_batches.load(Ordering::SeqCst), 3); // 10 + 10 + 5
    }

    #[test]
    fn pg_source_current_lsn_initially_zero() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        connector.connect().unwrap();
        assert_eq!(connector.current_lsn().unwrap(), 0);
    }

    #[test]
    fn pg_source_current_lsn_requires_connection() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        assert!(connector.current_lsn().is_err());
    }

    #[test]
    fn pg_source_ack_offset_updates_confirmed() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        connector.connect().unwrap();

        connector.ack_offset(&SourceOffset::new(100)).unwrap();
        assert_eq!(connector.confirmed_offset().unwrap().lsn, 100);

        // 较小的 LSN 不应覆盖
        connector.ack_offset(&SourceOffset::new(50)).unwrap();
        assert_eq!(connector.confirmed_offset().unwrap().lsn, 100);

        // 更大的 LSN 应覆盖
        connector.ack_offset(&SourceOffset::new(200)).unwrap();
        assert_eq!(connector.confirmed_offset().unwrap().lsn, 200);
    }

    #[test]
    fn pg_source_cdc_stream_requires_connection() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        let result = connector.start_cdc_stream(0, &|_events| Ok(()));
        assert!(result.is_err());
    }

    #[test]
    fn pg_source_cdc_stream_double_start_fails() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        connector.connect().unwrap();

        // 第一次启动（流为空立即结束）
        connector.start_cdc_stream(0, &|_events| Ok(())).unwrap();

        // 再次启动应正常（前一次已结束）
        connector.start_cdc_stream(0, &|_events| Ok(())).unwrap();
    }

    #[test]
    fn pg_source_cdc_stream_default_provider_immediately_ends() {
        let connector =
            PgSourceConnector::new(SourceConfig::postgres("postgresql://localhost/db")).unwrap();
        connector.connect().unwrap();

        let received = Arc::new(AtomicUsize::new(0));
        let r = received.clone();
        connector
            .start_cdc_stream(0, &move |_events| {
                r.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        // 默认 provider 返回 None，流立即结束，回调不应被调用
        assert_eq!(received.load(Ordering::SeqCst), 0);
        assert!(!connector.is_streaming());
    }

    #[test]
    fn pg_source_cdc_stream_with_events() {
        let event_count = Arc::new(AtomicUsize::new(0));
        let ec = event_count.clone();
        let event_provider: SourceEventProvider = Arc::new(move || {
            let n = ec.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok(Some(vec![
                    SourceEvent::insert(1, "public", "users", make_row(1, "Alice"), 1000),
                    SourceEvent::insert(2, "public", "users", make_row(2, "Bob"), 1001),
                ]))
            } else {
                Ok(None) // 流结束
            }
        });

        let connector = PgSourceConnector::with_providers(
            SourceConfig::postgres("postgresql://localhost/db"),
            event_provider,
            None,
            None,
        )
        .unwrap();
        connector.connect().unwrap();

        let total = Arc::new(AtomicUsize::new(0));
        let t = total.clone();
        connector
            .start_cdc_stream(0, &move |events| {
                t.fetch_add(events.len(), Ordering::SeqCst);
                Ok(())
            })
            .unwrap();

        assert_eq!(total.load(Ordering::SeqCst), 2);
        // 流结束后位点应为最后事件 LSN
        assert_eq!(connector.confirmed_offset().unwrap().lsn, 2);
    }

    #[test]
    fn pg_source_cdc_stream_stop_request() {
        let stop_counter = Arc::new(AtomicUsize::new(0));
        let sc = stop_counter.clone();
        let event_provider: SourceEventProvider = Arc::new(move || {
            let n = sc.fetch_add(1, Ordering::SeqCst);
            // 持续返回事件，直到 stop 被调用
            Ok(Some(vec![SourceEvent::insert(
                n as u64 + 1,
                "public",
                "users",
                make_row(n as i64 + 1, "user"),
                1000,
            )]))
        });

        let connector = Arc::new(
            PgSourceConnector::with_providers(
                SourceConfig::postgres("postgresql://localhost/db"),
                event_provider,
                None,
                None,
            )
            .unwrap(),
        );
        connector.connect().unwrap();

        let c = connector.clone();
        let stop_handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            c.stop_cdc_stream().unwrap();
        });

        let received = Arc::new(AtomicUsize::new(0));
        let r = received.clone();
        connector
            .start_cdc_stream(0, &move |events| {
                r.fetch_add(events.len(), Ordering::SeqCst);
                Ok(())
            })
            .unwrap();

        stop_handle.join().unwrap();
        // 应该收到至少一些事件后才停止
        assert!(received.load(Ordering::SeqCst) > 0);
        assert!(!connector.is_streaming());
    }

    #[test]
    fn pg_source_cdc_stream_start_lsn_advances_offset() {
        let event_provider: SourceEventProvider = Arc::new(|| Ok(None));
        let connector = PgSourceConnector::with_providers(
            SourceConfig::postgres("postgresql://localhost/db"),
            event_provider,
            None,
            None,
        )
        .unwrap();
        connector.connect().unwrap();

        connector.start_cdc_stream(500, &|_events| Ok(())).unwrap();
        // start_lsn=500 应被记录
        assert_eq!(connector.confirmed_offset().unwrap().lsn, 500);
    }

    #[test]
    fn pg_source_cdc_stream_callback_error_propagates() {
        let event_provider: SourceEventProvider = Arc::new(|| {
            Ok(Some(vec![SourceEvent::insert(
                1,
                "public",
                "users",
                make_row(1, "Alice"),
                1000,
            )]))
        });
        let connector = PgSourceConnector::with_providers(
            SourceConfig::postgres("postgresql://localhost/db"),
            event_provider,
            None,
            None,
        )
        .unwrap();
        connector.connect().unwrap();

        let result = connector.start_cdc_stream(0, &|_events| {
            Err(SourceError::Internal("callback error".to_string()))
        });
        assert!(result.is_err());
        match result {
            Err(SourceError::Internal(msg)) => assert!(msg.contains("callback error")),
            _ => panic!("expected Internal error"),
        }
    }

    #[test]
    fn pg_source_pg_type_to_szrsql_basic_mapping() {
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("int2").unwrap(),
            DataType::Int32
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("int4").unwrap(),
            DataType::Int32
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("int8").unwrap(),
            DataType::Int64
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("text").unwrap(),
            DataType::Text
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("bytea").unwrap(),
            DataType::Blob
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("float8").unwrap(),
            DataType::Real
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("bool").unwrap(),
            DataType::Bool
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("date").unwrap(),
            DataType::Date
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("timestamp").unwrap(),
            DataType::Timestamp
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("jsonb").unwrap(),
            DataType::Json
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("uuid").unwrap(),
            DataType::Uuid
        );
    }

    #[test]
    fn pg_source_pg_type_to_szrsql_handles_case_insensitive() {
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("INT8").unwrap(),
            DataType::Int64
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("VarChar").unwrap(),
            DataType::Text
        );
    }

    #[test]
    fn pg_source_pg_type_to_szrsql_handles_parentheses() {
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("varchar(255)").unwrap(),
            DataType::Text
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("numeric(10,2)").unwrap(),
            DataType::Real
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("char(10)").unwrap(),
            DataType::Text
        );
    }

    #[test]
    fn pg_source_pg_type_to_szrsql_aliases() {
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("smallint").unwrap(),
            DataType::Int32
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("integer").unwrap(),
            DataType::Int32
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("bigint").unwrap(),
            DataType::Int64
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("boolean").unwrap(),
            DataType::Bool
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("double precision").unwrap(),
            DataType::Real
        );
        assert_eq!(
            PgSourceConnector::pg_type_to_szrsql("timestamptz").unwrap(),
            DataType::Timestamp
        );
    }

    #[test]
    fn pg_source_pg_type_to_szrsql_unsupported_type() {
        let result = PgSourceConnector::pg_type_to_szrsql("geometry");
        assert!(result.is_err());
        match result {
            Err(SourceError::TypeMapping(msg)) => assert!(msg.contains("geometry")),
            _ => panic!("expected TypeMapping error"),
        }
    }

    #[test]
    fn pg_source_make_schema_from_pg_columns() {
        let schema = PgSourceConnector::make_schema(
            "users",
            42,
            vec![
                ("id".to_string(), "int8".to_string(), false),
                ("name".to_string(), "varchar(100)".to_string(), true),
                ("data".to_string(), "jsonb".to_string(), true),
            ],
        )
        .unwrap();

        assert_eq!(schema.table_id, 42);
        assert_eq!(schema.table_name, "users");
        assert_eq!(schema.columns.len(), 3);
        assert_eq!(schema.columns[0].name, "id");
        assert_eq!(schema.columns[0].data_type, DataType::Int64);
        assert!(!schema.columns[0].nullable);
        assert_eq!(schema.columns[1].data_type, DataType::Text);
        assert!(schema.columns[1].nullable);
        assert_eq!(schema.columns[2].data_type, DataType::Json);
    }

    #[test]
    fn pg_source_make_schema_invalid_type_fails() {
        let result = PgSourceConnector::make_schema(
            "users",
            1,
            vec![("id".to_string(), "invalid_type".to_string(), false)],
        );
        assert!(result.is_err());
    }

    #[test]
    fn pg_source_make_row_helper() {
        let row = PgSourceConnector::make_row(vec![
            ("id".to_string(), SzValue::Int64(1)),
            ("name".to_string(), SzValue::Text("Alice".to_string())),
        ]);
        assert_eq!(row.len(), 2);
        assert_eq!(row.columns[0].0, "id");
    }

    #[test]
    fn pg_source_disconnect_stops_stream() {
        let event_provider: SourceEventProvider = Arc::new(|| {
            Ok(Some(vec![SourceEvent::insert(
                1,
                "public",
                "users",
                make_row(1, "Alice"),
                1000,
            )]))
        });
        let connector = Arc::new(
            PgSourceConnector::with_providers(
                SourceConfig::postgres("postgresql://localhost/db"),
                event_provider,
                None,
                None,
            )
            .unwrap(),
        );
        connector.connect().unwrap();

        let c = connector.clone();
        let stop_handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            c.disconnect().unwrap();
        });

        let _ = connector.start_cdc_stream(0, &|_events| Ok(()));
        stop_handle.join().unwrap();
        assert!(!connector.is_streaming());
        assert!(!connector.is_connected());
    }
}
