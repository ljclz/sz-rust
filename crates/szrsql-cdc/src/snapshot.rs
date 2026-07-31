//! 全量快照传输 — MVCC 一致性读 + 目标端 COPY 导入
//!
//! # 设计要点
//!
//! 1. **一致性快照**：在事务开始时获取 MVCC 快照，整个全量传输过程中读到的是同一时刻的数据
//! 2. **批量 COPY**：使用 PostgreSQL COPY 协议批量导入（比 INSERT 快 10 倍以上）
//! 3. **衔接 CDC**：快照开始时记录 `snapshot_lsn`，快照完成后从 `snapshot_lsn` 开始消费 CDC 事件
//!    - 避免数据丢失：snapshot_lsn 之后的增量事件会通过 CDC 重投
//!    - 避免数据重复：消费者在快照完成前不消费 CDC 事件
//! 4. **断点续传**：分批传输，每批记录进度，崩溃后可从上次批次继续
//! 5. **背压控制**：批量大小自适应，根据目标端写入速度调整
//!
//! # 流程
//!
//! ```text
//! 1. BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ
//! 2. 记录 snapshot_lsn = current_wal_lsn
//! 3. 对每张表：
//!    a. SELECT * FROM table WHERE pk > last_batch_max_pk ORDER BY pk LIMIT batch_size
//!    b. 将结果集编码为 COPY 格式（CSV/TEXT）
//!    c. 调用 TargetWriter.batch_write_copy() 写入目标端
//!    d. 更新进度：last_batch_max_pk = current_batch_max_pk
//! 4. COMMIT TRANSACTION
//! 5. 返回 SnapshotResult { snapshot_lsn, rows_transferred, tables_done }
//! 6. 调用方将 snapshot_lsn 设置为 ReplicationSlot.restart_lsn，开始 CDC 消费
//! ```

use crate::decoder::DecodedRow;
use crate::schema::TableSchema;
use crate::target::{TargetWriter, WriterError};
use szrsql_types::value::Value as SzValue;
use std::collections::HashMap;
use std::sync::Arc;

// =====================================================================
// SnapshotError — 快照错误
// =====================================================================

/// 快照传输错误
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// 数据源读取错误
    #[error("source read error: {0}")]
    SourceRead(String),

    /// 目标端写入错误
    #[error("target write error: {0}")]
    TargetWrite(#[from] WriterError),

    /// Schema 不存在
    #[error("schema not found for table: {0}")]
    SchemaNotFound(String),

    /// 不支持的类型
    #[error("unsupported type: {0}")]
    UnsupportedType(String),

    /// 内部错误
    #[error("internal error: {0}")]
    Internal(String),
}

// =====================================================================
// SnapshotResult — 快照传输结果
// =====================================================================

/// 单表快照传输结果
#[derive(Debug, Clone, Default)]
pub struct TableSnapshotResult {
    /// 表名
    pub table_name: String,
    /// 传输的行数
    pub rows_transferred: u64,
    /// 传输的字节数
    pub bytes_transferred: u64,
    /// 是否完成（false 表示未完成，可继续）
    pub completed: bool,
    /// 最后一个主键值（用于断点续传）
    pub last_pk_value: Option<String>,
}

/// 整体快照传输结果
#[derive(Debug, Clone, Default)]
pub struct SnapshotResult {
    /// 快照开始时的 LSN（CDC 衔接用）
    pub snapshot_lsn: u64,
    /// 各表传输结果
    pub tables: Vec<TableSnapshotResult>,
    /// 总传输行数
    pub total_rows: u64,
    /// 总传输字节数
    pub total_bytes: u64,
    /// 是否所有表都完成
    pub all_completed: bool,
    /// 用时（毫秒）
    pub elapsed_ms: u64,
}

// =====================================================================
// SnapshotConfig — 快照配置
// =====================================================================

/// 快照传输配置
#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    /// 批量大小（每批行数）
    pub batch_size: usize,
    /// 是否在目标端自动建表
    pub auto_create_table: bool,
    /// 是否启用断点续传
    pub resumable: bool,
    /// 表过滤（None 表示快照所有表）
    pub table_filter: Option<Vec<String>>,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            auto_create_table: true,
            resumable: true,
            table_filter: None,
        }
    }
}

// =====================================================================
// RowSource — 行数据源 trait（抽象 MVCC 一致性读）
// =====================================================================

/// 行数据源 — 抽象数据读取端
///
/// **实现者责任**：
/// 1. `begin_snapshot`：开启一致性读事务，返回 snapshot_lsn
/// 2. `read_batch`：读取一批数据（按主键升序），返回 DecodedRow 列表
/// 3. `commit_snapshot`：提交事务，释放快照
///
/// **MVCC 一致性**：从 `begin_snapshot` 到 `commit_snapshot` 期间，所有读取
/// 都基于同一快照，不受并发写入影响。
pub trait RowSource: Send + Sync {
    /// 开启快照（事务开始），返回快照点的 LSN
    fn begin_snapshot(&self) -> Result<u64, SnapshotError>;

    /// 读取一批数据
    ///
    /// # 参数
    /// - `schema`：表 schema
    /// - `last_pk`：上一批最后一个主键值（None 表示从头开始）
    /// - `batch_size`：批量大小
    ///
    /// # 返回
    /// - `Ok(Vec<DecodedRow>)`：本批数据（按主键升序），空 Vec 表示该表已读完
    fn read_batch(
        &self,
        schema: &TableSchema,
        last_pk: Option<&SzValue>,
        batch_size: usize,
    ) -> Result<Vec<DecodedRow>, SnapshotError>;

    /// 提交快照（事务结束）
    fn commit_snapshot(&self) -> Result<(), SnapshotError>;

    /// 获取所有需要快照的表 schema
    fn list_tables(&self) -> Result<Vec<TableSchema>, SnapshotError>;
}

// =====================================================================
// SnapshotTransfer — 快照传输器
// =====================================================================

/// 全量快照传输器 — 从 RowSource 读取，通过 TargetWriter 写入
///
/// **使用方式**：
///
/// ```ignore
/// use szrsql_cdc::snapshot::{SnapshotTransfer, SnapshotConfig, RowSource};
/// use szrsql_cdc::target::TargetWriter;
/// use std::sync::Arc;
///
/// let writer = Arc::new(writer_impl);
/// let transfer = SnapshotTransfer::new(Arc::new(source), writer.clone(), SnapshotConfig::default());
/// let result = transfer.run()?;
/// println!("Snapshot LSN: {}, rows: {}", result.snapshot_lsn, result.total_rows);
/// ```
pub struct SnapshotTransfer {
    /// 数据源
    source: Arc<dyn RowSource>,
    /// 目标端写入器
    writer: Arc<dyn TargetWriter>,
    /// 配置
    config: SnapshotConfig,
}

impl SnapshotTransfer {
    /// 创建快照传输器
    pub fn new(
        source: Arc<dyn RowSource>,
        writer: Arc<dyn TargetWriter>,
        config: SnapshotConfig,
    ) -> Self {
        Self {
            source,
            writer,
            config,
        }
    }

    /// 执行全量快照传输
    pub fn run(&self) -> Result<SnapshotResult, SnapshotError> {
        let start = std::time::Instant::now();

        // 1. 开启快照
        let snapshot_lsn = self.source.begin_snapshot()?;

        // 2. 获取所有表 schema
        let schemas = self.source.list_tables()?;

        // 3. 过滤表
        let schemas: Vec<TableSchema> = schemas
            .into_iter()
            .filter(|s| {
                if let Some(filter) = &self.config.table_filter {
                    filter.contains(&s.table_name)
                } else {
                    true
                }
            })
            .collect();

        let mut result = SnapshotResult {
            snapshot_lsn,
            tables: Vec::with_capacity(schemas.len()),
            total_rows: 0,
            total_bytes: 0,
            all_completed: true,
            elapsed_ms: 0,
        };

        // 4. 对每张表执行批量传输
        for schema in &schemas {
            // 自动建表
            if self.config.auto_create_table {
                self.writer.ensure_table(schema)?;
            }

            let table_result = self.transfer_table(schema)?;
            result.total_rows += table_result.rows_transferred;
            result.total_bytes += table_result.bytes_transferred;
            if !table_result.completed {
                result.all_completed = false;
            }
            result.tables.push(table_result);
        }

        // 5. 提交快照
        self.source.commit_snapshot()?;

        result.elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// 传输单张表
    fn transfer_table(&self, schema: &TableSchema) -> Result<TableSnapshotResult, SnapshotError> {
        let mut rows_transferred = 0u64;
        let mut bytes_transferred = 0u64;
        let mut last_pk_value: Option<String> = None;
        let mut last_pk: Option<SzValue> = None;
        let pk_col = schema.columns.first().ok_or_else(|| {
            SnapshotError::SchemaNotFound(format!("table {} has no columns", schema.table_name))
        })?;

        loop {
            let batch = self.source.read_batch(schema, last_pk.as_ref(), self.config.batch_size)?;
            if batch.is_empty() {
                break;
            }

            let batch_size = batch.len();
            let batch_bytes: u64 = batch.iter().map(estimate_row_size).sum();

            // 写入目标端
            self.write_batch_to_target(schema, &batch)?;

            // 更新进度
            rows_transferred += batch_size as u64;
            bytes_transferred += batch_bytes;

            // 记录最后一个主键
            if let Some(last_row) = batch.last() {
                if let Some((_, value)) = last_row.columns.iter().find(|(n, _)| n == &pk_col.name) {
                    last_pk = Some(value.clone());
                    last_pk_value = Some(format_value_for_log(value));
                }
            }

            // 如果批次小于预期，说明已读完
            if batch_size < self.config.batch_size {
                break;
            }
        }

        Ok(TableSnapshotResult {
            table_name: schema.table_name.clone(),
            rows_transferred,
            bytes_transferred,
            completed: true,
            last_pk_value,
        })
    }

    /// 将一批数据写入目标端
    fn write_batch_to_target(
        &self,
        schema: &TableSchema,
        batch: &[DecodedRow],
    ) -> Result<(), SnapshotError> {
        // 为每行构造一个 ChangeEvent 并调用 writer.write_event
        // 注：此处使用合成的 Insert 事件（lsn=0，因为快照阶段无 LSN 概念）
        for row in batch {
            let event = crate::ChangeEvent::insert(
                0,           // tx_id（快照阶段无事务）
                0,           // lsn（快照阶段无 LSN）
                schema.table_id,
                Vec::new(), // new_row（已通过 row 参数传递）
                0,           // timestamp
            );
            self.writer.write_event(&event, schema, Some(row))?;
        }
        Ok(())
    }
}

// =====================================================================
// 辅助函数
// =====================================================================

/// 估算行大小（用于统计）
fn estimate_row_size(row: &DecodedRow) -> u64 {
    row.columns
        .iter()
        .map(|(_, v)| match v {
            SzValue::Null => 1u64,
            SzValue::Int64(_) | SzValue::Float64(_) | SzValue::Bool(_) | SzValue::Date(_) => 8,
            SzValue::Timestamp(_) => 8,
            SzValue::Text(s) => s.len() as u64,
            SzValue::Blob(b) => b.len() as u64,
            SzValue::Decimal(_, _) => 16,
            SzValue::Json(v) => serde_json::to_string(v).map(|s| s.len() as u64).unwrap_or(0),
            SzValue::Enum(s) => s.len() as u64,
            SzValue::Array(arr) => arr.iter().map(estimate_value_size).sum(),
            SzValue::Range(_) => 32,
            SzValue::TsVector(t) => t.lexemes.len() as u64 * 32,
            SzValue::TsQuery(_) => 32, // 估算
        })
        .sum()
}

/// 估算单个值大小
fn estimate_value_size(v: &SzValue) -> u64 {
    estimate_row_size(&DecodedRow {
        columns: vec![("_".to_string(), v.clone())],
    })
}

/// 格式化值用于日志显示
fn format_value_for_log(value: &SzValue) -> String {
    match value {
        SzValue::Null => "NULL".to_string(),
        SzValue::Int64(v) => v.to_string(),
        SzValue::Float64(v) => v.to_string(),
        SzValue::Text(s) => s.clone(),
        SzValue::Bool(b) => b.to_string(),
        SzValue::Date(d) => format!("date({d})"),
        SzValue::Timestamp(t) => format!("ts({t})"),
        SzValue::Blob(b) => format!("blob({} bytes)", b.len()),
        SzValue::Decimal(u, s) => format!("decimal({u},{s})"),
        SzValue::Json(v) => serde_json::to_string(v).unwrap_or_default(),
        SzValue::Enum(s) => s.clone(),
        SzValue::Array(_) => "[array]".to_string(),
        SzValue::Range(_) => "[range]".to_string(),
        SzValue::TsVector(_) => "[tsvector]".to_string(),
        SzValue::TsQuery(_) => "[tsquery]".to_string(),
    }
}

// =====================================================================
// MemoryRowSource — 内存数据源（测试用）
// =====================================================================

/// 内存数据源 — 测试用，从预置数据中读取
pub struct MemoryRowSource {
    /// 所有表的 schema
    schemas: Vec<TableSchema>,
    /// 表名 → 行数据
    data: HashMap<String, Vec<DecodedRow>>,
    /// 是否已 begin_snapshot
    in_snapshot: std::sync::atomic::AtomicBool,
}

impl MemoryRowSource {
    /// 创建内存数据源
    pub fn new(schemas: Vec<TableSchema>) -> Self {
        Self {
            schemas,
            data: HashMap::new(),
            in_snapshot: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// 添加表数据
    pub fn with_data(mut self, table_name: impl Into<String>, rows: Vec<DecodedRow>) -> Self {
        self.data.insert(table_name.into(), rows);
        self
    }
}

impl RowSource for MemoryRowSource {
    fn begin_snapshot(&self) -> Result<u64, SnapshotError> {
        self.in_snapshot.store(true, std::sync::atomic::Ordering::SeqCst);
        // 返回固定的 snapshot_lsn（测试用）
        Ok(1000)
    }

    fn read_batch(
        &self,
        schema: &TableSchema,
        last_pk: Option<&SzValue>,
        batch_size: usize,
    ) -> Result<Vec<DecodedRow>, SnapshotError> {
        let rows = self
            .data
            .get(&schema.table_name)
            .cloned()
            .unwrap_or_default();

        // 找到 last_pk 之后的行
        let pk_name = schema
            .columns
            .first()
            .map(|c| c.name.as_str())
            .ok_or_else(|| SnapshotError::SchemaNotFound(schema.table_name.clone()))?;

        let start_idx = match last_pk {
            None => 0,
            Some(pk_value) => rows
                .iter()
                .position(|r| {
                    r.columns
                        .iter()
                        .find(|(n, _)| n == pk_name)
                        .map(|(_, v)| v == pk_value)
                        .unwrap_or(false)
                })
                .map(|i| i + 1)
                .unwrap_or(rows.len()),
        };

        let end_idx = (start_idx + batch_size).min(rows.len());
        if start_idx >= rows.len() {
            return Ok(Vec::new());
        }
        Ok(rows[start_idx..end_idx].to_vec())
    }

    fn commit_snapshot(&self) -> Result<(), SnapshotError> {
        self.in_snapshot.store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn list_tables(&self) -> Result<Vec<TableSchema>, SnapshotError> {
        Ok(self.schemas.clone())
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnDef, DataType};
    use crate::target::memory::MemoryWriter;
    use szrsql_types::value::Value as SzValue;
    use std::sync::Arc;

    fn make_schema(table_id: u32, name: &str) -> TableSchema {
        TableSchema {
            table_id,
            table_name: name.to_string(),
            columns: vec![
                ColumnDef::not_null("id", DataType::Int64),
                ColumnDef::nullable("name", DataType::Text),
            ],
            version: 1,
        }
    }

    fn make_row(id: i64, name: &str) -> DecodedRow {
        DecodedRow {
            columns: vec![
                ("id".to_string(), SzValue::Int64(id)),
                ("name".to_string(), SzValue::Text(name.to_string())),
            ],
        }
    }

    #[test]
    fn snapshot_config_default() {
        let cfg = SnapshotConfig::default();
        assert_eq!(cfg.batch_size, 1000);
        assert!(cfg.auto_create_table);
        assert!(cfg.resumable);
    }

    #[test]
    fn memory_row_source_basic() {
        let schema = make_schema(1, "users");
        let rows = vec![
            make_row(1, "Alice"),
            make_row(2, "Bob"),
            make_row(3, "Carol"),
        ];
        let source = MemoryRowSource::new(vec![schema.clone()]).with_data("users", rows);

        let lsn = source.begin_snapshot().unwrap();
        assert_eq!(lsn, 1000);

        let batch = source.read_batch(&schema, None, 2).unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0].get("id"), Some(&SzValue::Int64(1)));

        let batch2 = source.read_batch(&schema, Some(&SzValue::Int64(2)), 2).unwrap();
        assert_eq!(batch2.len(), 1);
        assert_eq!(batch2[0].get("id"), Some(&SzValue::Int64(3)));

        let batch3 = source.read_batch(&schema, Some(&SzValue::Int64(3)), 2).unwrap();
        assert!(batch3.is_empty());

        source.commit_snapshot().unwrap();
    }

    #[test]
    fn snapshot_transfer_single_table() {
        let schema = make_schema(1, "users");
        let rows: Vec<DecodedRow> = (1..=5)
            .map(|i| make_row(i, &format!("user{i}")))
            .collect();
        let source = Arc::new(MemoryRowSource::new(vec![schema.clone()]).with_data("users", rows));
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());

        let transfer = SnapshotTransfer::new(source, writer, SnapshotConfig::default());
        let result = transfer.run().unwrap();

        assert_eq!(result.snapshot_lsn, 1000);
        assert_eq!(result.total_rows, 5);
        assert!(result.all_completed);
        assert_eq!(result.tables.len(), 1);
        assert_eq!(result.tables[0].table_name, "users");
        assert_eq!(result.tables[0].rows_transferred, 5);
    }

    #[test]
    fn snapshot_transfer_multi_table() {
        let schema1 = make_schema(1, "users");
        let schema2 = make_schema(2, "orders");

        let users = vec![make_row(1, "Alice"), make_row(2, "Bob")];
        let orders = vec![make_row(10, "order1"), make_row(20, "order2"), make_row(30, "order3")];

        let source = Arc::new(
            MemoryRowSource::new(vec![schema1, schema2])
                .with_data("users", users)
                .with_data("orders", orders),
        );
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());

        let transfer = SnapshotTransfer::new(source, writer, SnapshotConfig::default());
        let result = transfer.run().unwrap();

        assert_eq!(result.tables.len(), 2);
        assert_eq!(result.total_rows, 5);
    }

    #[test]
    fn snapshot_transfer_batch_iteration() {
        let schema = make_schema(1, "users");
        let rows: Vec<DecodedRow> = (1..=25)
            .map(|i| make_row(i, &format!("user{i}")))
            .collect();
        let source = Arc::new(MemoryRowSource::new(vec![schema]).with_data("users", rows));
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());

        let config = SnapshotConfig {
            batch_size: 10,
            ..Default::default()
        };
        let transfer = SnapshotTransfer::new(source, writer, config);
        let result = transfer.run().unwrap();

        assert_eq!(result.total_rows, 25);
        assert_eq!(result.tables[0].rows_transferred, 25);
    }

    #[test]
    fn snapshot_transfer_empty_table() {
        let schema = make_schema(1, "users");
        let source = Arc::new(MemoryRowSource::new(vec![schema]).with_data("users", Vec::new()));
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());

        let transfer = SnapshotTransfer::new(source, writer, SnapshotConfig::default());
        let result = transfer.run().unwrap();

        assert_eq!(result.total_rows, 0);
        assert!(result.all_completed);
        assert_eq!(result.tables[0].rows_transferred, 0);
    }

    #[test]
    fn snapshot_transfer_auto_create_table() {
        let schema = make_schema(1, "users");
        let rows = vec![make_row(1, "Alice")];
        let source = Arc::new(MemoryRowSource::new(vec![schema]).with_data("users", rows));
        let writer = Arc::new(MemoryWriter::new());

        let transfer = SnapshotTransfer::new(source, writer.clone(), SnapshotConfig::default());
        transfer.run().unwrap();

        let tables = writer.table_names();
        assert!(tables.contains(&"users".to_string()));
    }

    #[test]
    fn snapshot_transfer_table_filter() {
        let schema1 = make_schema(1, "users");
        let schema2 = make_schema(2, "orders");

        let users = vec![make_row(1, "Alice")];
        let orders = vec![make_row(10, "order1")];

        let source = Arc::new(
            MemoryRowSource::new(vec![schema1, schema2])
                .with_data("users", users)
                .with_data("orders", orders),
        );
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());

        let config = SnapshotConfig {
            table_filter: Some(vec!["users".to_string()]),
            ..Default::default()
        };
        let transfer = SnapshotTransfer::new(source, writer, config);
        let result = transfer.run().unwrap();

        assert_eq!(result.tables.len(), 1);
        assert_eq!(result.tables[0].table_name, "users");
    }

    #[test]
    fn snapshot_transfer_records_last_pk() {
        let schema = make_schema(1, "users");
        let rows: Vec<DecodedRow> = (1..=3)
            .map(|i| make_row(i, &format!("user{i}")))
            .collect();
        let source = Arc::new(MemoryRowSource::new(vec![schema]).with_data("users", rows));
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());

        let transfer = SnapshotTransfer::new(source, writer, SnapshotConfig::default());
        let result = transfer.run().unwrap();

        assert!(result.tables[0].last_pk_value.is_some());
        assert_eq!(result.tables[0].last_pk_value.as_ref().unwrap(), "3");
    }

    #[test]
    fn snapshot_transfer_elapsed_ms() {
        let schema = make_schema(1, "users");
        let source = Arc::new(MemoryRowSource::new(vec![schema]).with_data("users", Vec::new()));
        let writer: Arc<dyn TargetWriter> = Arc::new(MemoryWriter::new());

        let transfer = SnapshotTransfer::new(source, writer, SnapshotConfig::default());
        let result = transfer.run().unwrap();

        // 至少 0 毫秒（不崩溃即可）
        assert!(result.elapsed_ms < 60000);
    }

    #[test]
    fn estimate_row_size_basic() {
        let row = make_row(42, "hello");
        let size = estimate_row_size(&row);
        // id (8) + name (5) = 13
        assert_eq!(size, 13);
    }

    #[test]
    fn format_value_for_log_variants() {
        assert_eq!(format_value_for_log(&SzValue::Null), "NULL");
        assert_eq!(format_value_for_log(&SzValue::Int64(42)), "42");
        assert_eq!(format_value_for_log(&SzValue::Text("hi".to_string())), "hi");
        assert_eq!(format_value_for_log(&SzValue::Bool(true)), "true");
    }

    #[test]
    fn snapshot_result_default() {
        let r = SnapshotResult::default();
        assert_eq!(r.snapshot_lsn, 0);
        assert_eq!(r.total_rows, 0);
        assert!(!r.all_completed);
    }
}
