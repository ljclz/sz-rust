//! 反向链路源端抽象 — 外部数据库 CDC 源 → szrsql
//!
//! 对应 `NineData分析与szrsql数据复制环方案.md` P5-3。
//!
//! # 设计要点
//!
//! 1. **反向链路定义**：与正向链路（szrsql → 外部数据库）相反，
//!    反向链路从外部数据库捕获变更，写入 szrsql 本地。
//!    场景：PG/MySQL 主库 → szrsql 作为分析副本。
//!
//! 2. **SourceConnector trait**：屏蔽源端数据库差异
//!    - `connect` / `disconnect`：源端连接管理
//!    - `discover_schemas`：源端表结构发现
//!    - `extract_snapshot`：全量快照抽取（用于初次同步）
//!    - `start_cdc_stream` / `stop_cdc_stream`：CDC 增量流控制
//!    - `current_lsn`：源端当前位点（用于断点续传）
//!
//! 3. **SourceEvent 注入模式**：与 `TargetWriter::SqlExecutor` 对称，
//!    使用 `SourceEventProvider` 闭包注入源端事件，避免直接依赖外部客户端库。
//!    生产部署时由调用方注入（基于 tokio-postgres / mysql_async 等）。
//!
//! 4. **断点续传**：通过 `SourceOffset` 持久化消费位点，崩溃后从上次位点恢复。
//!
//! 5. **类型映射**：源端列类型 → szrsql `DataType`，统一内部表示。
//!
//! # 模块组织
//!
//! - `SourceConnector` trait + `SourceConfig` + `SourceOffset`
//! - `SourceEvent`：源端变更事件（独立于 szrsql 的 `ChangeEvent`，因为源端 schema 不同）
//! - `SourceError`：源端错误类型
//! - `PgSourceConnector`：PostgreSQL 反向链路实现（P5-3）

use crate::decoder::DecodedRow;
use crate::schema::TableSchema;
use std::sync::Arc;

pub mod logical_replication;
pub mod pg_real;
pub mod pg_source;
pub mod reverse;

// =====================================================================
// SourceError — 源端错误
// =====================================================================

/// 源端错误
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// 连接错误（网络/认证）
    #[error("source connection error: {0}")]
    Connection(String),

    /// SQL 执行错误
    #[error("source sql error: {0}")]
    Sql(String),

    /// Schema 发现失败
    #[error("schema discovery error: {0}")]
    SchemaDiscovery(String),

    /// 类型映射失败（源端类型无法映射到 szrsql DataType）
    #[error("type mapping error: {0}")]
    TypeMapping(String),

    /// 位点越界（请求的 LSN 大于源端当前 LSN）
    #[error("lsn out of range: requested={requested}, current={current}")]
    LsnOutOfRange { requested: u64, current: u64 },

    /// 不支持的操作
    #[error("unsupported operation: {0}")]
    Unsupported(String),

    /// 内部错误
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<crate::source::reverse::ReverseReplicatorError> for SourceError {
    fn from(e: crate::source::reverse::ReverseReplicatorError) -> Self {
        SourceError::Internal(e.to_string())
    }
}

// =====================================================================
// SourceOffset — 源端消费位点
// =====================================================================

/// 源端消费位点 — 用于断点续传
///
/// **设计**：
/// - `lsn`：源端日志序列号（PG LSN / MySQL binlog position / Oracle SCN）
/// - `tx_id`：最近提交的事务 ID（可选，用于去重）
/// - `event_offset`：同一事务内的事件偏移（可选，用于事务内断点）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceOffset {
    /// 源端 LSN（日志序列号）
    pub lsn: u64,
    /// 最近提交的事务 ID（可选）
    pub tx_id: Option<u64>,
    /// 事务内事件偏移（可选）
    pub event_offset: Option<u32>,
}

impl SourceOffset {
    /// 创建新位点
    pub fn new(lsn: u64) -> Self {
        Self {
            lsn,
            tx_id: None,
            event_offset: None,
        }
    }

    /// 创建带事务 ID 的位点
    pub fn with_tx(lsn: u64, tx_id: u64) -> Self {
        Self {
            lsn,
            tx_id: Some(tx_id),
            event_offset: None,
        }
    }

    /// 是否比另一个位点更新（基于 LSN 比较）
    pub fn is_after(&self, other: &Self) -> bool {
        self.lsn > other.lsn
    }
}

impl Default for SourceOffset {
    fn default() -> Self {
        Self::new(0)
    }
}

impl std::fmt::Display for SourceOffset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "lsn={}", self.lsn)?;
        if let Some(tx) = self.tx_id {
            write!(f, ", tx={}", tx)?;
        }
        if let Some(off) = self.event_offset {
            write!(f, ", off={}", off)?;
        }
        Ok(())
    }
}

// =====================================================================
// SourceEvent — 源端变更事件
// =====================================================================

/// 源端变更事件操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceEventOp {
    /// 插入
    Insert,
    /// 更新
    Update,
    /// 删除
    Delete,
    /// 事务提交（标记事务边界，本身不携带行数据）
    Commit,
    /// 事务回滚
    Abort,
}

impl SourceEventOp {
    /// 转字符串
    pub fn as_str(self) -> &'static str {
        match self {
            SourceEventOp::Insert => "insert",
            SourceEventOp::Update => "update",
            SourceEventOp::Delete => "delete",
            SourceEventOp::Commit => "commit",
            SourceEventOp::Abort => "abort",
        }
    }

    /// 是否为 DML 操作（携带行数据）
    pub fn is_dml(self) -> bool {
        matches!(
            self,
            SourceEventOp::Insert | SourceEventOp::Update | SourceEventOp::Delete
        )
    }
}

/// 源端变更事件 — 外部数据库的 CDC 事件
///
/// **与 szrsql 内部 `ChangeEvent` 的差异**：
/// - `table_name`：源端表名（字符串），而非 `table_id`（szrsql 内部 ID）
/// - `schema_name`：源端 schema/库名（PG schema / MySQL database）
/// - `before` / `after`：使用 `DecodedRow`（列名+列值），而非 `Vec<u8>` 二进制
/// - `lsn`：源端 LSN（PG LSN / MySQL binlog position）
#[derive(Debug, Clone, PartialEq)]
pub struct SourceEvent {
    /// 源端 LSN
    pub lsn: u64,
    /// 操作类型
    pub op: SourceEventOp,
    /// 源端 schema 名（PG: public, MySQL: database name）
    pub schema_name: String,
    /// 源端表名
    pub table_name: String,
    /// 前镜像（Update/Delete 携带，Insert 为 None）
    pub before: Option<DecodedRow>,
    /// 后镜像（Insert/Update 携带，Delete 为 None）
    pub after: Option<DecodedRow>,
    /// 事务 ID（可选，用于事务边界识别）
    pub tx_id: Option<u64>,
    /// 事件时间戳（Unix 毫秒）
    pub timestamp: u64,
}

impl SourceEvent {
    /// 创建 Insert 事件
    pub fn insert(
        lsn: u64,
        schema_name: impl Into<String>,
        table_name: impl Into<String>,
        after: DecodedRow,
        timestamp: u64,
    ) -> Self {
        Self {
            lsn,
            op: SourceEventOp::Insert,
            schema_name: schema_name.into(),
            table_name: table_name.into(),
            before: None,
            after: Some(after),
            tx_id: None,
            timestamp,
        }
    }

    /// 创建 Update 事件
    pub fn update(
        lsn: u64,
        schema_name: impl Into<String>,
        table_name: impl Into<String>,
        before: DecodedRow,
        after: DecodedRow,
        timestamp: u64,
    ) -> Self {
        Self {
            lsn,
            op: SourceEventOp::Update,
            schema_name: schema_name.into(),
            table_name: table_name.into(),
            before: Some(before),
            after: Some(after),
            tx_id: None,
            timestamp,
        }
    }

    /// 创建 Delete 事件
    pub fn delete(
        lsn: u64,
        schema_name: impl Into<String>,
        table_name: impl Into<String>,
        before: DecodedRow,
        timestamp: u64,
    ) -> Self {
        Self {
            lsn,
            op: SourceEventOp::Delete,
            schema_name: schema_name.into(),
            table_name: table_name.into(),
            before: Some(before),
            after: None,
            tx_id: None,
            timestamp,
        }
    }

    /// 创建 Commit 事件
    pub fn commit(lsn: u64, tx_id: u64, timestamp: u64) -> Self {
        Self {
            lsn,
            op: SourceEventOp::Commit,
            schema_name: String::new(),
            table_name: String::new(),
            before: None,
            after: None,
            tx_id: Some(tx_id),
            timestamp,
        }
    }

    /// 创建 Abort 事件
    pub fn abort(lsn: u64, tx_id: u64, timestamp: u64) -> Self {
        Self {
            lsn,
            op: SourceEventOp::Abort,
            schema_name: String::new(),
            table_name: String::new(),
            before: None,
            after: None,
            tx_id: Some(tx_id),
            timestamp,
        }
    }

    /// 设置事务 ID（用于事务边界识别）
    pub fn with_tx_id(mut self, tx_id: u64) -> Self {
        self.tx_id = Some(tx_id);
        self
    }

    /// 获取该事件的源端表全限定名（schema.table）
    pub fn qualified_table(&self) -> String {
        if self.schema_name.is_empty() {
            self.table_name.clone()
        } else {
            format!("{}.{}", self.schema_name, self.table_name)
        }
    }
}

// =====================================================================
// SourceConfig — 源端配置
// =====================================================================

/// 源端配置 — 描述外部源端数据库的连接信息
#[derive(Debug, Clone)]
pub struct SourceConfig {
    /// 源端类型（"postgres" / "mysql" / "oracle" / "sqlserver"）
    pub source_type: String,
    /// 连接字符串
    pub connection_string: String,
    /// 源端 schema/库名（PG: public, MySQL: database, Oracle: schema）
    pub schema: Option<String>,
    /// 表过滤白名单（空表示所有表）
    pub table_filter: Vec<String>,
    /// 起始 LSN（None 表示从最新位置开始；Some(0) 表示从最早开始）
    pub start_lsn: Option<u64>,
    /// 是否启用全量快照（首次同步）
    pub initial_snapshot: bool,
}

impl SourceConfig {
    /// 创建 PG 源端配置
    pub fn postgres(connection_string: impl Into<String>) -> Self {
        Self {
            source_type: "postgres".to_string(),
            connection_string: connection_string.into(),
            schema: Some("public".to_string()),
            table_filter: Vec::new(),
            start_lsn: None,
            initial_snapshot: true,
        }
    }

    /// 创建 MySQL 源端配置（预留，P5+ 阶段实现）
    pub fn mysql(connection_string: impl Into<String>) -> Self {
        Self {
            source_type: "mysql".to_string(),
            connection_string: connection_string.into(),
            schema: None,
            table_filter: Vec::new(),
            start_lsn: None,
            initial_snapshot: true,
        }
    }

    /// 设置表过滤
    pub fn with_tables(mut self, tables: Vec<String>) -> Self {
        self.table_filter = tables;
        self
    }

    /// 设置起始 LSN
    pub fn with_start_lsn(mut self, lsn: u64) -> Self {
        self.start_lsn = Some(lsn);
        self
    }

    /// 禁用初始全量快照
    pub fn without_initial_snapshot(mut self) -> Self {
        self.initial_snapshot = false;
        self
    }
}

// =====================================================================
// SourceConnector — 源端连接器 trait
// =====================================================================

/// 源端连接器 — 抽象外部数据库 CDC 源
///
/// **实现者责任**：
/// 1. `connect`：建立源端连接（认证、心跳）
/// 2. `discover_schemas`：发现源端表结构（用于目标端 ensure_table）
/// 3. `extract_snapshot`：全量快照抽取（按表批量拉取）
/// 4. `start_cdc_stream`：启动 CDC 增量流（基于 start_lsn）
/// 5. `current_lsn`：获取源端当前 LSN（用于位点对齐）
/// 6. `ack_offset`：确认消费位点（用于断点续传）
///
/// **同步语义**：
/// - trait 为同步接口，与 `TargetWriter` 对称
/// - 实现可在内部通过线程池处理 IO
///
/// **错误处理**：
/// - 临时错误（网络抖动）应返回 `SourceError::Connection`，调用方可重试
/// - 不可恢复错误返回 `SourceError::Internal`
pub trait SourceConnector: Send + Sync {
    /// 源端类型名（"postgres" / "mysql" / ...）
    fn source_type(&self) -> &str;

    /// 连接源端
    fn connect(&self) -> Result<(), SourceError>;

    /// 断开源端
    fn disconnect(&self) -> Result<(), SourceError>;

    /// 发现源端表结构
    ///
    /// # 参数
    /// - `tables`：表名列表；空 Vec 表示发现所有表
    ///
    /// # 返回
    /// - `Ok(Vec<TableSchema>)`：表结构列表
    fn discover_schemas(&self, tables: &[String]) -> Result<Vec<TableSchema>, SourceError>;

    /// 全量快照抽取
    ///
    /// # 参数
    /// - `table`：表名
    /// - `batch_size`：每批拉取行数
    /// - `callback`：每批回调（行数据列表）
    ///
    /// # 返回
    /// - `Ok(u64)`：抽取的总行数
    /// - `Err`：抽取失败
    ///
    /// # 注意
    /// - 调用方需在 callback 中将行写入目标端
    /// - 抽取过程中源端可能继续写入，CDC 增量会处理增量部分
    fn extract_snapshot(
        &self,
        table: &str,
        batch_size: usize,
        callback: &dyn Fn(&[DecodedRow]) -> Result<(), SourceError>,
    ) -> Result<u64, SourceError>;

    /// 获取源端当前 LSN
    fn current_lsn(&self) -> Result<u64, SourceError>;

    /// 启动 CDC 增量流
    ///
    /// # 参数
    /// - `start_lsn`：起始 LSN
    /// - `callback`：事件回调（每批事件）
    ///
    /// # 返回
    /// - `Ok(())`：流正常结束
    /// - `Err`：流异常中断
    ///
    /// # 注意
    /// - 该方法阻塞直到流结束或被 `stop_cdc_stream` 中断
    /// - 实现需支持 backpressure：callback 慢时源端不能 OOM
    fn start_cdc_stream(
        &self,
        start_lsn: u64,
        callback: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
    ) -> Result<(), SourceError>;

    /// 停止 CDC 增量流（异步中断）
    fn stop_cdc_stream(&self) -> Result<(), SourceError>;

    /// 确认消费位点（持久化）
    fn ack_offset(&self, offset: &SourceOffset) -> Result<(), SourceError>;

    /// 获取已确认的消费位点
    fn confirmed_offset(&self) -> Result<SourceOffset, SourceError>;

    /// 健康检查
    fn health_check(&self) -> Result<(), SourceError>;
}

// =====================================================================
// SourceEventProvider — 事件提供者闭包（测试用注入模式）
// =====================================================================

/// 源端事件提供者 — 注入此闭包以模拟源端 CDC 流（测试用）
///
/// **设计**：与 `TargetWriter::SqlExecutor` 对称，使用闭包注入事件，
/// 避免直接依赖 tokio-postgres / mysql_async 等客户端库。
///
/// 闭包签名：`Fn() -> Result<Option<Vec<SourceEvent>>, SourceError>`
/// - `Ok(Some(events))`：返回一批事件
/// - `Ok(None)`：流结束（无更多事件）
/// - `Err`：流错误
pub type SourceEventProvider =
    Arc<dyn Fn() -> Result<Option<Vec<SourceEvent>>, SourceError> + Send + Sync>;

/// 源端 Schema 提供者 — 注入此闭包以返回源端表结构（测试用）
pub type SchemaProvider =
    Arc<dyn Fn(&[String]) -> Result<Vec<TableSchema>, SourceError> + Send + Sync>;

/// 源端快照提供者 — 注入此闭包以返回全量快照数据（测试用）
pub type SnapshotProvider = Arc<
    dyn Fn(&str, usize) -> Result<Vec<DecodedRow>, SourceError> + Send + Sync,
>;

// =====================================================================
// SourceConnectorFactory — 工厂函数
// =====================================================================

/// 根据配置创建 SourceConnector
///
/// # 参数
/// - `config`：源端配置
///
/// # 返回
/// - `Ok(Arc<dyn SourceConnector>)`：创建成功
/// - `Err(SourceError)`：不支持的类型或连接失败
pub fn create_source_connector(
    config: &SourceConfig,
) -> Result<Arc<dyn SourceConnector>, SourceError> {
    match config.source_type.as_str() {
        "postgres" | "postgresql" | "pg" => {
            Ok(Arc::new(crate::source::pg_source::PgSourceConnector::new(
                config.clone(),
            )?))
        }
        _ => Err(SourceError::Unsupported(format!(
            "unsupported source type: {}",
            config.source_type
        ))),
    }
}

/// P7-2: 创建真实 PG 源端连接器（基于 `postgres::Client`，无闭包注入）
///
/// 与 `create_source_connector` 不同，本函数返回的 `PgRealSourceConnector` 真实连接
/// PostgreSQL 数据库，使用触发器 + 日志表模式实现 CDC。
///
/// # 参数
/// - `config`：源端配置
///
/// # 返回
/// - `Ok(Arc<PgRealSourceConnector>)`：创建成功（已建立 PG 连接）
/// - `Err(SourceError)`：连接失败
pub fn create_real_pg_source_connector(
    config: &SourceConfig,
) -> Result<Arc<crate::source::pg_real::PgRealSourceConnector>, SourceError> {
    // 使用 `::postgres::` 显式引用外部 crate
    let client = ::postgres::Client::connect(&config.connection_string, ::postgres::NoTls)
        .map_err(|e| SourceError::Connection(format!("PG connect failed: {e}")))?;
    Ok(Arc::new(crate::source::pg_real::PgRealSourceConnector::new(
        client,
        config.clone(),
    )?))
}

/// P1-2: 创建 PG logical replication 源端连接器（基于 replication slot + START_REPLICATION）
///
/// 与 `create_real_pg_source_connector`（触发器模式）不同，本函数返回的
/// `LogicalReplicationSource` 使用 PG 原生 logical replication 协议，
/// 性能更高、对源端侵入更小。
///
/// # 参数
/// - `config`：源端配置（连接串需包含 `replication=database` 参数）
/// - `slot_name`：replication slot 名称（需在 PG 端唯一）
/// - `publication_name`：publication 名称（需在 PG 端唯一）
///
/// # 返回
/// - `Ok(Arc<LogicalReplicationSource>)`：创建成功（已建立 PG 连接）
/// - `Err(SourceError)`：连接失败
///
/// # 使用示例
///
/// ```ignore
/// use szrsql_cdc::source::create_logical_replication_source;
/// use szrsql_cdc::source::{SourceConfig, SourceConnector};
///
/// let conn_str = "postgres://postgres:test123@127.0.0.1:5432/sz_orm_test?replication=database";
/// let connector = create_logical_replication_source(
///     &SourceConfig::postgres(conn_str),
///     "szrsql_slot",
///     "szrsql_pub",
/// ).unwrap();
/// connector.connect().unwrap();
/// ```
pub fn create_logical_replication_source(
    config: &SourceConfig,
    slot_name: &str,
    publication_name: &str,
) -> Result<Arc<crate::source::logical_replication::LogicalReplicationSource>, SourceError> {
    // 使用 `::postgres::` 显式引用外部 crate
    let client = ::postgres::Client::connect(&config.connection_string, ::postgres::NoTls)
        .map_err(|e| SourceError::Connection(format!("PG connect failed: {e}")))?;
    Ok(Arc::new(
        crate::source::logical_replication::LogicalReplicationSource::new(
            client,
            config.clone(),
            slot_name,
            publication_name,
        )?,
    ))
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use szrsql_types::value::Value as SzValue;

    #[test]
    fn source_offset_default_is_zero() {
        let off = SourceOffset::default();
        assert_eq!(off.lsn, 0);
        assert_eq!(off.tx_id, None);
        assert_eq!(off.event_offset, None);
    }

    #[test]
    fn source_offset_is_after_compares_lsn() {
        let a = SourceOffset::new(100);
        let b = SourceOffset::new(200);
        assert!(!a.is_after(&b));
        assert!(b.is_after(&a));
        assert!(!a.is_after(&a));
    }

    #[test]
    fn source_offset_with_tx_carries_tx_id() {
        let off = SourceOffset::with_tx(500, 42);
        assert_eq!(off.lsn, 500);
        assert_eq!(off.tx_id, Some(42));
    }

    #[test]
    fn source_offset_display_format() {
        let off = SourceOffset {
            lsn: 100,
            tx_id: Some(42),
            event_offset: Some(3),
        };
        let s = format!("{}", off);
        assert!(s.contains("lsn=100"));
        assert!(s.contains("tx=42"));
        assert!(s.contains("off=3"));
    }

    #[test]
    fn source_event_op_is_dml_correct() {
        assert!(SourceEventOp::Insert.is_dml());
        assert!(SourceEventOp::Update.is_dml());
        assert!(SourceEventOp::Delete.is_dml());
        assert!(!SourceEventOp::Commit.is_dml());
        assert!(!SourceEventOp::Abort.is_dml());
    }

    #[test]
    fn source_event_op_as_str() {
        assert_eq!(SourceEventOp::Insert.as_str(), "insert");
        assert_eq!(SourceEventOp::Update.as_str(), "update");
        assert_eq!(SourceEventOp::Delete.as_str(), "delete");
        assert_eq!(SourceEventOp::Commit.as_str(), "commit");
        assert_eq!(SourceEventOp::Abort.as_str(), "abort");
    }

    #[test]
    fn source_event_insert_constructor() {
        let row = DecodedRow {
            columns: vec![("id".to_string(), SzValue::Int64(1))],
        };
        let event = SourceEvent::insert(100, "public", "users", row.clone(), 1000);
        assert_eq!(event.lsn, 100);
        assert_eq!(event.op, SourceEventOp::Insert);
        assert_eq!(event.schema_name, "public");
        assert_eq!(event.table_name, "users");
        assert_eq!(event.before, None);
        assert_eq!(event.after, Some(row));
        assert_eq!(event.tx_id, None);
        assert_eq!(event.timestamp, 1000);
    }

    #[test]
    fn source_event_update_constructor() {
        let before = DecodedRow {
            columns: vec![("id".to_string(), SzValue::Int64(1))],
        };
        let after = DecodedRow {
            columns: vec![("id".to_string(), SzValue::Int64(1))],
        };
        let event = SourceEvent::update(100, "public", "users", before.clone(), after.clone(), 1000);
        assert_eq!(event.op, SourceEventOp::Update);
        assert_eq!(event.before, Some(before));
        assert_eq!(event.after, Some(after));
    }

    #[test]
    fn source_event_delete_constructor() {
        let before = DecodedRow {
            columns: vec![("id".to_string(), SzValue::Int64(1))],
        };
        let event = SourceEvent::delete(100, "public", "users", before.clone(), 1000);
        assert_eq!(event.op, SourceEventOp::Delete);
        assert_eq!(event.before, Some(before));
        assert_eq!(event.after, None);
    }

    #[test]
    fn source_event_commit_abort_constructors() {
        let commit = SourceEvent::commit(100, 42, 1000);
        assert_eq!(commit.op, SourceEventOp::Commit);
        assert_eq!(commit.tx_id, Some(42));
        assert_eq!(commit.table_name, "");

        let abort = SourceEvent::abort(101, 43, 1001);
        assert_eq!(abort.op, SourceEventOp::Abort);
        assert_eq!(abort.tx_id, Some(43));
    }

    #[test]
    fn source_event_with_tx_id_chain() {
        let row = DecodedRow {
            columns: vec![("id".to_string(), SzValue::Int64(1))],
        };
        let event = SourceEvent::insert(100, "public", "users", row, 1000)
            .with_tx_id(42);
        assert_eq!(event.tx_id, Some(42));
    }

    #[test]
    fn source_event_qualified_table_with_schema() {
        let event = SourceEvent::commit(100, 1, 1000);
        let _ = event; // unused warning prevention

        let row = DecodedRow {
            columns: vec![("id".to_string(), SzValue::Int64(1))],
        };
        let e = SourceEvent::insert(100, "public", "users", row, 1000);
        assert_eq!(e.qualified_table(), "public.users");
    }

    #[test]
    fn source_event_qualified_table_without_schema() {
        let row = DecodedRow {
            columns: vec![("id".to_string(), SzValue::Int64(1))],
        };
        let mut e = SourceEvent::insert(100, "", "users", row, 1000);
        e.schema_name = String::new();
        assert_eq!(e.qualified_table(), "users");
    }

    #[test]
    fn source_config_postgres_factory() {
        let cfg = SourceConfig::postgres("postgresql://user:pass@host/db");
        assert_eq!(cfg.source_type, "postgres");
        assert_eq!(cfg.connection_string, "postgresql://user:pass@host/db");
        assert_eq!(cfg.schema.as_deref(), Some("public"));
        assert!(cfg.table_filter.is_empty());
        assert!(cfg.initial_snapshot);
    }

    #[test]
    fn source_config_mysql_factory() {
        let cfg = SourceConfig::mysql("mysql://user:pass@host/db");
        assert_eq!(cfg.source_type, "mysql");
        assert!(cfg.schema.is_none());
    }

    #[test]
    fn source_config_builder_chains() {
        let cfg = SourceConfig::postgres("postgresql://localhost/db")
            .with_tables(vec!["users".to_string(), "orders".to_string()])
            .with_start_lsn(12345)
            .without_initial_snapshot();
        assert_eq!(cfg.table_filter, vec!["users", "orders"]);
        assert_eq!(cfg.start_lsn, Some(12345));
        assert!(!cfg.initial_snapshot);
    }

    #[test]
    fn create_source_connector_unsupported_type() {
        let mut cfg = SourceConfig::postgres("postgresql://localhost/db");
        cfg.source_type = "redis".to_string();
        let result = create_source_connector(&cfg);
        assert!(result.is_err());
        match result {
            Err(SourceError::Unsupported(msg)) => {
                assert!(msg.contains("redis"));
            }
            _ => panic!("expected Unsupported error"),
        }
    }

    #[test]
    fn create_source_connector_postgres_alias() {
        for alias in &["postgres", "postgresql", "pg"] {
            let mut cfg = SourceConfig::postgres("postgresql://localhost/db");
            cfg.source_type = alias.to_string();
            let result = create_source_connector(&cfg);
            assert!(result.is_ok(), "alias {} should be supported", alias);
        }
    }

    #[test]
    fn source_error_display() {
        let e = SourceError::Connection("network down".to_string());
        assert!(format!("{}", e).contains("network down"));

        let e = SourceError::LsnOutOfRange {
            requested: 100,
            current: 50,
        };
        let s = format!("{}", e);
        assert!(s.contains("requested=100"));
        assert!(s.contains("current=50"));
    }
}
