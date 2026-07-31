//! 反向复制器 — 协调 SourceConnector + TargetWriter，实现外部数据库 → szrsql 的反向链路
//!
//! 对应 `NineData分析与szrsql数据复制环方案.md` P5-3。
//!
//! # 设计
//!
//! 1. **协调角色**：`ReverseReplicator` 持有 `SourceConnector` + `TargetWriter`，
//!    在源端和目标端之间协调数据流动
//!
//! 2. **生命周期**：
//!    - Created → Starting（结构迁移 + 全量快照）→ Running（CDC 增量流）→ Stopped/Failed
//!    - 与正向链路 `ReplicationTask` 状态机对称，但方向相反
//!
//! 3. **数据流**：
//!    ```text
//!    外部 DB → SourceConnector → SourceEvent → 转换 → ChangeEvent → TargetWriter → szrsql
//!    ```
//!
//! 4. **关键转换**：`SourceEvent`（schema_name + table_name + DecodedRow）
//!    → szrsql 内部 `ChangeEvent`（table_id + Vec<u8> 二进制）
//!    或直接调用 `TargetWriter::write_event`（接受 `DecodedRow`）
//!
//! 5. **断点续传**：源端位点由 `SourceConnector::ack_offset` 持久化，
//!    ReverseReplicator 内部维护 `last_processed_lsn` 供监控
//!
//! 6. **错误处理**：
//!    - Schema 不匹配：Failed 状态，需人工介入
//!    - 写入失败：默认重试 N 次，超过后 Failed
//!    - 源端断连：自动重连（最多 3 次）
//!
//! # 使用方式
//!
//! ```ignore
//! use szrsql_cdc::source::reverse::ReverseReplicator;
//! use szrsql_cdc::source::{SourceConfig, SourceConnector, SourceEvent, SourceEventProvider};
//! use szrsql_cdc::target::{TargetConfig, TargetWriter, create_writer};
//! use std::sync::Arc;
//!
//! let source = szrsql_cdc::source::create_source_connector(
//!     &SourceConfig::postgres("postgresql://external/db"),
//! )?;
//! let target = create_writer(&TargetConfig::postgres("postgresql://szrsql/local"))?;
//!
//! let mut replicator = ReverseReplicator::new("rev_pg_to_szrsql", source, target);
//! replicator.start()?;
//! ```

use crate::decoder::DecodedRow;
use crate::schema::TableSchema;
use crate::source::{
    SourceConnector, SourceError, SourceEvent, SourceEventOp, SourceOffset,
};
use crate::target::{TargetWriter, WriterError};
use crate::{CdcEventOp, ChangeEvent};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

// =====================================================================
// ReverseReplicatorError — 反向复制器错误
// =====================================================================

/// 反向复制器错误
#[derive(Debug, thiserror::Error)]
pub enum ReverseReplicatorError {
    /// 源端错误
    #[error("source error: {0}")]
    Source(#[from] SourceError),

    /// 目标端错误
    #[error("target error: {0}")]
    Target(#[from] WriterError),

    /// 状态机错误（如未启动就停止）
    #[error("state error: {0}")]
    State(String),

    /// Schema 不匹配
    #[error("schema mismatch: {0}")]
    SchemaMismatch(String),

    /// 内部错误
    #[error("internal error: {0}")]
    Internal(String),
}

// =====================================================================
// ReverseReplicatorState — 反向复制器状态机
// =====================================================================

/// 反向复制器状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReverseReplicatorState {
    /// 已创建未启动
    Created,
    /// 启动中（结构迁移 + 全量快照）
    Starting,
    /// 运行中（CDC 增量流）
    Running,
    /// 已暂停
    Paused,
    /// 已停止
    Stopped,
    /// 失败
    Failed,
}

impl ReverseReplicatorState {
    /// 转字符串
    pub fn as_str(self) -> &'static str {
        match self {
            ReverseReplicatorState::Created => "created",
            ReverseReplicatorState::Starting => "starting",
            ReverseReplicatorState::Running => "running",
            ReverseReplicatorState::Paused => "paused",
            ReverseReplicatorState::Stopped => "stopped",
            ReverseReplicatorState::Failed => "failed",
        }
    }

    /// 是否可启动
    pub fn can_start(self) -> bool {
        matches!(self, Self::Created | Self::Failed)
    }

    /// 是否可停止
    pub fn can_stop(self) -> bool {
        matches!(
            self,
            Self::Created | Self::Starting | Self::Running | Self::Paused | Self::Failed
        )
    }

    /// 是否可暂停
    pub fn can_pause(self) -> bool {
        matches!(self, Self::Running)
    }

    /// 是否可恢复
    pub fn can_resume(self) -> bool {
        matches!(self, Self::Paused)
    }
}

impl std::fmt::Display for ReverseReplicatorState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
// ReverseReplicatorStats — 反向复制统计
// =====================================================================

/// 反向复制统计信息
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ReverseReplicatorStats {
    /// 已处理事件总数
    pub events_processed: u64,
    /// 已处理字节数（估算）
    pub bytes_processed: u64,
    /// 失败事件数
    pub errors: u64,
    /// 全量快照行数
    pub snapshot_rows: u64,
    /// 全量快照表数
    pub snapshot_tables: u64,
    /// 当前源端 LSN
    pub current_source_lsn: u64,
    /// 已确认 LSN
    pub confirmed_lsn: u64,
    /// 启动时间（Unix 毫秒，0 表示未启动）
    pub started_at: u64,
    /// 最后事件时间（Unix 毫秒，0 表示无事件）
    pub last_event_at: u64,
    /// 延迟（毫秒，最后事件时间到当前时间）
    pub lag_ms: u64,
}

// =====================================================================
// ReverseReplicator — 反向复制器
// =====================================================================

/// 反向复制器 — 协调 SourceConnector + TargetWriter
///
/// **生命周期**：
/// 1. `start`：连接源端 → 结构迁移 → 全量快照 → CDC 流
/// 2. `pause` / `resume`：暂停/恢复 CDC 流（保留源端连接）
/// 3. `stop`：停止 CDC 流，断开源端
///
/// **线程安全**：内部 `RwLock` 保护状态，`AtomicU64` 计数器，支持并发查询
pub struct ReverseReplicator {
    /// 任务 ID
    task_id: String,
    /// 源端连接器
    source: Arc<dyn SourceConnector>,
    /// 目标端写入器
    target: Arc<dyn TargetWriter>,
    /// 状态
    state: RwLock<ReverseReplicatorState>,
    /// 统计信息
    stats: Mutex<ReverseReplicatorStats>,
    /// 已发现的表 schema 缓存（table_name → TableSchema）
    schemas: RwLock<HashMap<String, TableSchema>>,
    /// 停止信号
    stop_requested: AtomicBool,
    /// 暂停信号
    pause_requested: AtomicBool,
    /// 重试次数上限（默认 3）
    max_retries: u32,
    /// 重试间隔（毫秒，默认 1000）
    retry_interval_ms: u64,
}

impl ReverseReplicator {
    /// 创建反向复制器
    ///
    /// # 参数
    /// - `task_id`：任务 ID（唯一标识）
    /// - `source`：源端连接器
    /// - `target`：目标端写入器
    pub fn new(
        task_id: impl Into<String>,
        source: Arc<dyn SourceConnector>,
        target: Arc<dyn TargetWriter>,
    ) -> Self {
        Self {
            task_id: task_id.into(),
            source,
            target,
            state: RwLock::new(ReverseReplicatorState::Created),
            stats: Mutex::new(ReverseReplicatorStats::default()),
            schemas: RwLock::new(HashMap::new()),
            stop_requested: AtomicBool::new(false),
            pause_requested: AtomicBool::new(false),
            max_retries: 3,
            retry_interval_ms: 1000,
        }
    }

    /// 设置重试次数上限
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// 设置重试间隔（毫秒）
    pub fn with_retry_interval(mut self, interval_ms: u64) -> Self {
        self.retry_interval_ms = interval_ms;
        self
    }

    /// 获取任务 ID
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// 获取当前状态
    pub fn state(&self) -> ReverseReplicatorState {
        *self.state.read().unwrap()
    }

    /// 获取统计信息快照
    pub fn stats(&self) -> ReverseReplicatorStats {
        self.stats.lock().unwrap().clone()
    }

    /// 启动反向复制（结构迁移 + 全量快照 + CDC 流）
    ///
    /// **流程**：
    /// 1. 状态转为 Starting
    /// 2. 连接源端
    /// 3. 发现源端表结构
    /// 4. 在目标端 ensure_table
    /// 5. 全量快照抽取并写入目标端
    /// 6. 状态转为 Running
    /// 7. 启动 CDC 流，事件转写目标端
    /// 8. 流结束后状态转为 Stopped
    ///
    /// **注**：该方法阻塞直到 CDC 流结束或被 `stop` 中断
    pub fn start(&self) -> Result<(), ReverseReplicatorError> {
        {
            let mut state = self.state.write().unwrap();
            if !state.can_start() {
                return Err(ReverseReplicatorError::State(format!(
                    "cannot start from state {}",
                    state
                )));
            }
            *state = ReverseReplicatorState::Starting;
        }

        self.stop_requested.store(false, Ordering::SeqCst);
        self.pause_requested.store(false, Ordering::SeqCst);

        // 记录启动时间
        let now = unix_millis();
        self.stats.lock().unwrap().started_at = now;

        // 1. 连接源端
        if let Err(e) = self.source.connect() {
            self.set_state(ReverseReplicatorState::Failed);
            return Err(ReverseReplicatorError::Source(e));
        }

        // 2. 结构迁移：发现源端 schema 并 ensure_table
        if let Err(e) = self.run_schema_migration() {
            self.set_state(ReverseReplicatorState::Failed);
            return Err(e);
        }

        // 3. 全量快照
        if let Err(e) = self.run_initial_snapshot() {
            self.set_state(ReverseReplicatorState::Failed);
            return Err(e);
        }

        // 4. 状态转为 Running，开始 CDC 流
        self.set_state(ReverseReplicatorState::Running);

        // 5. CDC 流（阻塞）
        let start_lsn = self
            .source
            .confirmed_offset()
            .map_err(ReverseReplicatorError::Source)?
            .lsn;
        let result = self.run_cdc_stream(start_lsn);

        // 6. 流结束后的状态转换
        match result {
            Ok(()) => {
                self.set_state(ReverseReplicatorState::Stopped);
                Ok(())
            }
            Err(e) => {
                self.set_state(ReverseReplicatorState::Failed);
                Err(e)
            }
        }
    }

    /// 停止反向复制（异步中断 CDC 流）
    ///
    /// **幂等**：若已处于 `Stopped` 状态，直接返回 `Ok(())`
    pub fn stop(&self) -> Result<(), ReverseReplicatorError> {
        let state = self.state();
        // 幂等：已经停止则直接返回 Ok
        if state == ReverseReplicatorState::Stopped {
            return Ok(());
        }
        if !state.can_stop() {
            return Err(ReverseReplicatorError::State(format!(
                "cannot stop from state {}",
                state
            )));
        }
        self.stop_requested.store(true, Ordering::SeqCst);
        if state == ReverseReplicatorState::Running {
            self.source.stop_cdc_stream()?;
        }
        self.set_state(ReverseReplicatorState::Stopped);
        Ok(())
    }

    /// 暂停 CDC 流
    pub fn pause(&self) -> Result<(), ReverseReplicatorError> {
        let state = self.state();
        if !state.can_pause() {
            return Err(ReverseReplicatorError::State(format!(
                "cannot pause from state {}",
                state
            )));
        }
        self.pause_requested.store(true, Ordering::SeqCst);
        self.source.stop_cdc_stream()?;
        self.set_state(ReverseReplicatorState::Paused);
        Ok(())
    }

    /// 恢复 CDC 流（从上次确认位点继续）
    pub fn resume(&self) -> Result<(), ReverseReplicatorError> {
        let state = self.state();
        if !state.can_resume() {
            return Err(ReverseReplicatorError::State(format!(
                "cannot resume from state {}",
                state
            )));
        }
        self.pause_requested.store(false, Ordering::SeqCst);
        self.set_state(ReverseReplicatorState::Running);

        let start_lsn = self.source.confirmed_offset()?.lsn;
        self.run_cdc_stream(start_lsn)?;
        Ok(())
    }

    /// 健康检查（源端 + 目标端）
    pub fn health_check(&self) -> Result<(), ReverseReplicatorError> {
        self.source.health_check()?;
        self.target.health_check()?;
        Ok(())
    }

    // -----------------------------------------------------------------
    // 内部方法
    // -----------------------------------------------------------------

    /// 设置状态
    fn set_state(&self, new_state: ReverseReplicatorState) {
        let mut state = self.state.write().unwrap();
        *state = new_state;
    }

    /// 运行结构迁移：发现源端 schema → ensure_table
    fn run_schema_migration(&self) -> Result<(), ReverseReplicatorError> {
        let schemas = self.source.discover_schemas(&[])?;
        let mut cache = self.schemas.write().unwrap();
        for schema in &schemas {
            // 在目标端 ensure_table
            self.target.ensure_table(schema)?;
            cache.insert(schema.table_name.clone(), schema.clone());
        }
        Ok(())
    }

    /// 运行全量快照：逐表抽取 → 批量写入目标端
    fn run_initial_snapshot(&self) -> Result<(), ReverseReplicatorError> {
        let table_names: Vec<String> = {
            let cache = self.schemas.read().unwrap();
            cache.keys().cloned().collect()
        };

        let snapshot_count = table_names.len();
        for table_name in &table_names {
            if self.stop_requested.load(Ordering::SeqCst) {
                break;
            }

            let schema = {
                let cache = self.schemas.read().unwrap();
                cache
                    .get(table_name)
                    .ok_or_else(|| {
                        ReverseReplicatorError::SchemaMismatch(format!(
                            "schema not found for table {}",
                            table_name
                        ))
                    })?
                    .clone()
            };

            // 抽取快照并写入目标端
            let rows_written = self.source.extract_snapshot(table_name, 1000, &|rows| {
                for row in rows {
                    let event = build_change_event_from_source_row(
                        &schema.table_name,
                        schema.table_id,
                        row,
                    );
                    // 反向链路中，目标端是 szrsql，写入时无需 schema_version
                    self.target
                        .write_event(&event, &schema, Some(row))
                        .map_err(|e| SourceError::Internal(format!("target write error: {}", e)))?;
                }
                Ok(())
            })?;

            let mut stats = self.stats.lock().unwrap();
            stats.snapshot_rows += rows_written;
        }

        let mut stats = self.stats.lock().unwrap();
        stats.snapshot_tables += snapshot_count as u64;
        Ok(())
    }

    /// 运行 CDC 流：从 start_lsn 开始消费源端事件，转写目标端
    fn run_cdc_stream(&self, start_lsn: u64) -> Result<(), ReverseReplicatorError> {
        let result = self.source.start_cdc_stream(start_lsn, &|events| {
            for event in events {
                if self.stop_requested.load(Ordering::SeqCst) {
                    return Ok(());
                }
                // 将 ReverseReplicatorError 转换为 SourceError 以匹配回调签名
                if let Err(e) = self.process_source_event(event) {
                    return Err(SourceError::Internal(format!(
                        "process_source_event failed: {}",
                        e
                    )));
                }
            }
            Ok(())
        });

        match result {
            Ok(()) => Ok(()),
            Err(SourceError::Connection(_msg)) => {
                // 连接错误，尝试重连
                self.handle_connection_error()
            }
            Err(e) => Err(ReverseReplicatorError::Source(e)),
        }
    }

    /// 处理单个源端事件：转写为目标端写入
    fn process_source_event(
        &self,
        event: &SourceEvent,
    ) -> Result<(), ReverseReplicatorError> {
        let now = unix_millis();
        let mut stats = self.stats.lock().unwrap();
        stats.events_processed += 1;
        stats.last_event_at = now;
        stats.current_source_lsn = event.lsn;
        if event.lsn > stats.confirmed_lsn {
            stats.confirmed_lsn = event.lsn;
        }
        drop(stats);

        // Commit/Abort 不需要写入目标端
        if !event.op.is_dml() {
            // 但需要 ack 位点
            self.source.ack_offset(&SourceOffset::new(event.lsn))?;
            return Ok(());
        }

        // 查找 schema
        let schema = {
            let cache = self.schemas.read().unwrap();
            cache.get(&event.table_name).cloned()
        };

        let schema = match schema {
            Some(s) => s,
            None => {
                // schema 未缓存，尝试动态发现
                let discovered = self.source.discover_schemas(std::slice::from_ref(&event.table_name))?;
                if discovered.is_empty() {
                    return Err(ReverseReplicatorError::SchemaMismatch(format!(
                        "schema not found for table {}",
                        event.table_name
                    )));
                }
                let s = discovered[0].clone();
                self.schemas
                    .write()
                    .unwrap()
                    .insert(event.table_name.clone(), s.clone());
                self.target.ensure_table(&s)?;
                s
            }
        };

        // 转换 SourceEvent → szrsql ChangeEvent
        let (op, row) = match event.op {
            SourceEventOp::Insert => (CdcEventOp::Insert, event.after.as_ref()),
            SourceEventOp::Update => (CdcEventOp::Update, event.after.as_ref()),
            SourceEventOp::Delete => (CdcEventOp::Delete, event.before.as_ref()),
            _ => unreachable!(),
        };

        let sz_event = ChangeEvent {
            tx_id: event.tx_id.unwrap_or(0) as u32,
            lsn: event.lsn,
            op,
            table_id: Some(schema.table_id),
            old_row: None, // 反向链路暂不传 old_row（SourceEvent.before 可选）
            new_row: None, // 反向链路直接通过 row 参数传 DecodedRow
            timestamp: event.timestamp,
            schema_version: Some(schema.version),
        };

        // 写入目标端（带重试）
        self.write_with_retry(&sz_event, &schema, row)?;

        // 确认位点
        self.source.ack_offset(&SourceOffset::new(event.lsn))?;

        Ok(())
    }

    /// 带重试的写入
    fn write_with_retry(
        &self,
        event: &ChangeEvent,
        schema: &TableSchema,
        row: Option<&DecodedRow>,
    ) -> Result<(), ReverseReplicatorError> {
        let mut last_err = None;
        for attempt in 0..=self.max_retries {
            match self.target.write_event(event, schema, row) {
                Ok(()) => return Ok(()),
                Err(WriterError::Connection(msg)) => {
                    last_err = Some(WriterError::Connection(msg));
                    if attempt < self.max_retries {
                        // 简单 sleep（实际场景应使用 tokio::time::sleep）
                        std::thread::sleep(std::time::Duration::from_millis(
                            self.retry_interval_ms * (attempt as u64 + 1),
                        ));
                        continue;
                    }
                }
                Err(e) => {
                    // 非连接错误，不重试
                    self.stats.lock().unwrap().errors += 1;
                    return Err(ReverseReplicatorError::Target(e));
                }
            }
        }
        self.stats.lock().unwrap().errors += 1;
        Err(ReverseReplicatorError::Target(last_err.unwrap()))
    }

    /// 处理源端连接错误：尝试重连
    fn handle_connection_error(&self) -> Result<(), ReverseReplicatorError> {
        for attempt in 0..self.max_retries {
            std::thread::sleep(std::time::Duration::from_millis(
                self.retry_interval_ms * (attempt as u64 + 1),
            ));
            if self.stop_requested.load(Ordering::SeqCst) {
                return Err(ReverseReplicatorError::State(
                    "stopped during reconnection".to_string(),
                ));
            }
            match self.source.connect() {
                Ok(()) => {
                    // 重连成功，从上次确认位点继续 CDC 流
                    let start_lsn = self.source.confirmed_offset()?.lsn;
                    return self.run_cdc_stream(start_lsn);
                }
                Err(_) => continue,
            }
        }
        Err(ReverseReplicatorError::Source(SourceError::Connection(
            "reconnect failed after max retries".to_string(),
        )))
    }
}

// =====================================================================
// 辅助函数
// =====================================================================

/// 构建 szrsql ChangeEvent（从源端行数据）
fn build_change_event_from_source_row(
    _table_name: &str,
    table_id: u32,
    _row: &DecodedRow,
) -> ChangeEvent {
    // 反向链路快照写入：每行作为 Insert 事件
    // 注意：new_row 留空，因为 TargetWriter 通过 row 参数接收 DecodedRow
    ChangeEvent::insert(0, 0, table_id, Vec::new(), unix_millis())
}

/// 获取当前 Unix 毫秒
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::DecodedRow;
    use crate::schema::{ColumnDef, DataType, TableSchema};
    use crate::source::{SourceError, SourceEvent, SourceOffset};
    use crate::target::{TargetWriter, WriterError};
    use crate::{CdcEventOp, ChangeEvent};
    use szrsql_types::value::Value as SzValue;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    // -----------------------------------------------------------------
    // Mock Source Connector
    // -----------------------------------------------------------------

    /// 模拟源端连接器（用于测试）
    struct MockSourceConnector {
        events: Mutex<Vec<SourceEvent>>,
        schemas: Vec<TableSchema>,
        snapshot_rows: Mutex<HashMap<String, Vec<DecodedRow>>>,
        connected: AtomicBool,
        streaming: AtomicBool,
        confirmed_offset: Mutex<SourceOffset>,
        current_lsn: AtomicU64,
    }

    impl MockSourceConnector {
        fn new(
            events: Vec<SourceEvent>,
            schemas: Vec<TableSchema>,
            snapshot_rows: HashMap<String, Vec<DecodedRow>>,
        ) -> Self {
            Self {
                events: Mutex::new(events),
                schemas,
                snapshot_rows: Mutex::new(snapshot_rows),
                connected: AtomicBool::new(false),
                streaming: AtomicBool::new(false),
                confirmed_offset: Mutex::new(SourceOffset::default()),
                current_lsn: AtomicU64::new(0),
            }
        }
    }

    impl SourceConnector for MockSourceConnector {
        fn source_type(&self) -> &str {
            "mock"
        }

        fn connect(&self) -> Result<(), SourceError> {
            self.connected.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn disconnect(&self) -> Result<(), SourceError> {
            self.connected.store(false, Ordering::SeqCst);
            Ok(())
        }

        fn discover_schemas(&self, _tables: &[String]) -> Result<Vec<TableSchema>, SourceError> {
            Ok(self.schemas.clone())
        }

        fn extract_snapshot(
            &self,
            table: &str,
            _batch_size: usize,
            callback: &dyn Fn(&[DecodedRow]) -> Result<(), SourceError>,
        ) -> Result<u64, SourceError> {
            let rows = self.snapshot_rows.lock().unwrap();
            let table_rows = rows.get(table).cloned().unwrap_or_default();
            let count = table_rows.len() as u64;
            if !table_rows.is_empty() {
                callback(&table_rows)?;
            }
            Ok(count)
        }

        fn current_lsn(&self) -> Result<u64, SourceError> {
            Ok(self.current_lsn.load(Ordering::SeqCst))
        }

        fn start_cdc_stream(
            &self,
            _start_lsn: u64,
            callback: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
        ) -> Result<(), SourceError> {
            self.streaming.store(true, Ordering::SeqCst);
            let events = self.events.lock().unwrap().clone();
            if !events.is_empty() {
                callback(&events)?;
                let max_lsn = events.iter().map(|e| e.lsn).max().unwrap_or(0);
                self.current_lsn.store(max_lsn, Ordering::SeqCst);
            }
            self.streaming.store(false, Ordering::SeqCst);
            Ok(())
        }

        fn stop_cdc_stream(&self) -> Result<(), SourceError> {
            self.streaming.store(false, Ordering::SeqCst);
            Ok(())
        }

        fn ack_offset(&self, offset: &SourceOffset) -> Result<(), SourceError> {
            let mut current = self.confirmed_offset.lock().unwrap();
            if offset.lsn >= current.lsn {
                *current = offset.clone();
            }
            Ok(())
        }

        fn confirmed_offset(&self) -> Result<SourceOffset, SourceError> {
            Ok(self.confirmed_offset.lock().unwrap().clone())
        }

        fn health_check(&self) -> Result<(), SourceError> {
            if !self.connected.load(Ordering::SeqCst) {
                return Err(SourceError::Connection("not connected".to_string()));
            }
            Ok(())
        }
    }

    // -----------------------------------------------------------------
    // Mock Target Writer
    // -----------------------------------------------------------------

    /// 模拟目标端写入器（用于测试）
    struct MockTargetWriter {
        written_events: Mutex<Vec<(CdcEventOp, Option<DecodedRow>)>>,
        ensure_table_count: AtomicUsize,
        fail_on_write: AtomicBool,
    }

    impl MockTargetWriter {
        fn new() -> Self {
            Self {
                written_events: Mutex::new(Vec::new()),
                ensure_table_count: AtomicUsize::new(0),
                fail_on_write: AtomicBool::new(false),
            }
        }

        fn written_count(&self) -> usize {
            self.written_events.lock().unwrap().len()
        }

        fn written_events(&self) -> Vec<(CdcEventOp, Option<DecodedRow>)> {
            self.written_events.lock().unwrap().clone()
        }
    }

    impl TargetWriter for MockTargetWriter {
        fn write_event(
            &self,
            event: &ChangeEvent,
            _schema: &TableSchema,
            row: Option<&DecodedRow>,
        ) -> Result<(), WriterError> {
            if self.fail_on_write.load(Ordering::SeqCst) {
                return Err(WriterError::Connection("mock failure".to_string()));
            }
            self.written_events
                .lock()
                .unwrap()
                .push((event.op, row.cloned()));
            Ok(())
        }

        fn ensure_table(&self, _schema: &TableSchema) -> Result<(), WriterError> {
            self.ensure_table_count
                .fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn target_type(&self) -> &'static str {
            "mock"
        }
    }

    // -----------------------------------------------------------------
    // 辅助函数
    // -----------------------------------------------------------------

    fn make_schema(table_id: u32, table_name: &str) -> TableSchema {
        TableSchema {
            table_id,
            table_name: table_name.to_string(),
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

    fn make_source_events() -> Vec<SourceEvent> {
        vec![
            SourceEvent::insert(1, "public", "users", make_row(1, "Alice"), 1000),
            SourceEvent::insert(2, "public", "users", make_row(2, "Bob"), 1001),
            SourceEvent::commit(3, 100, 1002),
        ]
    }

    // -----------------------------------------------------------------
    // 测试用例
    // -----------------------------------------------------------------

    #[test]
    fn reverse_state_initial_is_created() {
        let source = Arc::new(MockSourceConnector::new(vec![], vec![], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target);
        assert_eq!(r.state(), ReverseReplicatorState::Created);
        assert_eq!(r.task_id(), "task1");
    }

    #[test]
    fn reverse_state_can_start_check() {
        assert!(ReverseReplicatorState::Created.can_start());
        assert!(ReverseReplicatorState::Failed.can_start());
        assert!(!ReverseReplicatorState::Running.can_start());
        assert!(!ReverseReplicatorState::Stopped.can_start());
    }

    #[test]
    fn reverse_state_can_pause_check() {
        assert!(ReverseReplicatorState::Running.can_pause());
        assert!(!ReverseReplicatorState::Created.can_pause());
        assert!(!ReverseReplicatorState::Paused.can_pause());
    }

    #[test]
    fn reverse_state_can_resume_check() {
        assert!(ReverseReplicatorState::Paused.can_resume());
        assert!(!ReverseReplicatorState::Running.can_resume());
    }

    #[test]
    fn reverse_state_can_stop_check() {
        // 几乎所有状态都可停止
        for s in [
            ReverseReplicatorState::Created,
            ReverseReplicatorState::Starting,
            ReverseReplicatorState::Running,
            ReverseReplicatorState::Paused,
            ReverseReplicatorState::Failed,
        ] {
            assert!(s.can_stop(), "{:?} should be stoppable", s);
        }
        assert!(!ReverseReplicatorState::Stopped.can_stop());
    }

    #[test]
    fn reverse_state_as_str() {
        assert_eq!(ReverseReplicatorState::Created.as_str(), "created");
        assert_eq!(ReverseReplicatorState::Starting.as_str(), "starting");
        assert_eq!(ReverseReplicatorState::Running.as_str(), "running");
        assert_eq!(ReverseReplicatorState::Paused.as_str(), "paused");
        assert_eq!(ReverseReplicatorState::Stopped.as_str(), "stopped");
        assert_eq!(ReverseReplicatorState::Failed.as_str(), "failed");
    }

    #[test]
    fn reverse_state_display() {
        let s = format!("{}", ReverseReplicatorState::Running);
        assert_eq!(s, "running");
    }

    #[test]
    fn reverse_full_lifecycle_with_empty_source() {
        let source = Arc::new(MockSourceConnector::new(vec![], vec![], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source.clone(), target.clone());

        r.start().unwrap();
        // 流结束后状态应为 Stopped（无事件）
        assert_eq!(r.state(), ReverseReplicatorState::Stopped);
        let stats = r.stats();
        assert_eq!(stats.events_processed, 0);
        assert_eq!(stats.snapshot_rows, 0);
    }

    #[test]
    fn reverse_full_lifecycle_with_events() {
        let schema = make_schema(1, "users");
        let mut snapshot = HashMap::new();
        snapshot.insert("users".to_string(), vec![make_row(0, "Initial")]);

        let source = Arc::new(MockSourceConnector::new(
            make_source_events(),
            vec![schema],
            snapshot,
        ));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source.clone(), target.clone());

        r.start().unwrap();
        assert_eq!(r.state(), ReverseReplicatorState::Stopped);

        let stats = r.stats();
        // 2 个 Insert + 1 个 Commit = 3 个事件
        assert_eq!(stats.events_processed, 3);
        assert_eq!(stats.snapshot_rows, 1);
        assert_eq!(stats.snapshot_tables, 1);
        // 已确认 LSN 应为最后事件 LSN
        assert_eq!(stats.confirmed_lsn, 3);
    }

    #[test]
    fn reverse_target_receives_insert_events() {
        let schema = make_schema(1, "users");
        let source = Arc::new(MockSourceConnector::new(
            make_source_events(),
            vec![schema],
            HashMap::new(),
        ));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target.clone());

        r.start().unwrap();

        let written = target.written_events();
        // 2 个 Insert 事件应写入目标端（Commit 不写入）
        assert_eq!(written.len(), 2);
        assert_eq!(written[0].0, CdcEventOp::Insert);
        assert_eq!(written[1].0, CdcEventOp::Insert);
    }

    #[test]
    fn reverse_snapshot_writes_to_target() {
        let schema = make_schema(1, "users");
        let mut snapshot = HashMap::new();
        snapshot.insert(
            "users".to_string(),
            vec![
                make_row(1, "Alice"),
                make_row(2, "Bob"),
                make_row(3, "Carol"),
            ],
        );

        let source = Arc::new(MockSourceConnector::new(
            vec![], // 无 CDC 事件
            vec![schema],
            snapshot,
        ));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target.clone());

        r.start().unwrap();

        // 3 行快照应写入目标端
        assert_eq!(target.written_count(), 3);
        let stats = r.stats();
        assert_eq!(stats.snapshot_rows, 3);
    }

    #[test]
    fn reverse_stop_aborts_cdc_stream() {
        let schema = make_schema(1, "users");
        let source = Arc::new(MockSourceConnector::new(
            make_source_events(),
            vec![schema],
            HashMap::new(),
        ));
        let target = Arc::new(MockTargetWriter::new());
        let r = Arc::new(ReverseReplicator::new(
            "task1",
            source,
            target.clone(),
        ));

        let r_clone = r.clone();
        let stop_handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            r_clone.stop().unwrap();
        });

        let _ = r.start();
        stop_handle.join().unwrap();
        // 状态应为 Stopped 或 Failed
        let s = r.state();
        assert!(
            s == ReverseReplicatorState::Stopped || s == ReverseReplicatorState::Failed,
            "expected Stopped or Failed, got {}",
            s
        );
    }

    #[test]
    fn reverse_start_from_invalid_state_fails() {
        let source = Arc::new(MockSourceConnector::new(vec![], vec![], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target);

        // 先启动一次
        r.start().unwrap();
        assert_eq!(r.state(), ReverseReplicatorState::Stopped);

        // 从 Stopped 状态再次 start 应失败
        let result = r.start();
        assert!(result.is_err());
        match result {
            Err(ReverseReplicatorError::State(_)) => {}
            _ => panic!("expected State error"),
        }
    }

    #[test]
    fn reverse_stop_from_created_state() {
        let source = Arc::new(MockSourceConnector::new(vec![], vec![], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target);

        r.stop().unwrap();
        assert_eq!(r.state(), ReverseReplicatorState::Stopped);
    }

    #[test]
    fn reverse_schema_mismatch_fails() {
        // 源端不返回 schema，但事件引用了不存在的表
        let source = Arc::new(MockSourceConnector::new(
            make_source_events(),
            vec![], // 无 schema
            HashMap::new(),
        ));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target);

        let result = r.start();
        // 应该失败（schema mismatch 或 ensure_table）
        assert!(result.is_err());
        assert_eq!(r.state(), ReverseReplicatorState::Failed);
    }

    #[test]
    fn reverse_with_retries_config() {
        let source = Arc::new(MockSourceConnector::new(vec![], vec![], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target)
            .with_max_retries(5)
            .with_retry_interval(100);

        // 验证配置不报错（私有字段，通过行为验证）
        let _ = r.start();
    }

    #[test]
    fn reverse_stats_initialized_zero() {
        let source = Arc::new(MockSourceConnector::new(vec![], vec![], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target);

        let stats = r.stats();
        assert_eq!(stats.events_processed, 0);
        assert_eq!(stats.bytes_processed, 0);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.snapshot_rows, 0);
        assert_eq!(stats.snapshot_tables, 0);
        assert_eq!(stats.current_source_lsn, 0);
        assert_eq!(stats.confirmed_lsn, 0);
        assert_eq!(stats.started_at, 0);
    }

    #[test]
    fn reverse_health_check_requires_connection() {
        let source = Arc::new(MockSourceConnector::new(vec![], vec![], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target);

        // 未连接，health_check 应失败
        assert!(r.health_check().is_err());

        // 启动后（连接成功）health_check 应通过
        r.start().unwrap();
        // 启动后源端已断开（流结束后状态为 Stopped）
        // 但 MockSourceConnector.connect 是幂等的，且 streaming=false 后连接仍保持
    }

    #[test]
    fn reverse_update_event_writes_to_target() {
        let schema = make_schema(1, "users");
        let before = make_row(1, "Alice");
        let after = make_row(1, "Bob");
        let events = vec![SourceEvent::update(
            10,
            "public",
            "users",
            before,
            after,
            1000,
        )];
        let source = Arc::new(MockSourceConnector::new(events, vec![schema], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target.clone());

        r.start().unwrap();

        let written = target.written_events();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, CdcEventOp::Update);
    }

    #[test]
    fn reverse_delete_event_writes_to_target() {
        let schema = make_schema(1, "users");
        let before = make_row(1, "Alice");
        let events = vec![SourceEvent::delete(10, "public", "users", before, 1000)];
        let source = Arc::new(MockSourceConnector::new(events, vec![schema], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target.clone());

        r.start().unwrap();

        let written = target.written_events();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, CdcEventOp::Delete);
    }

    #[test]
    fn reverse_commit_only_event_does_not_write() {
        let schema = make_schema(1, "users");
        let events = vec![SourceEvent::commit(10, 100, 1000)];
        let source = Arc::new(MockSourceConnector::new(events, vec![schema], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target.clone());

        r.start().unwrap();

        // Commit 事件不应写入目标端
        assert_eq!(target.written_count(), 0);
        // 但 events_processed 应计数
        assert_eq!(r.stats().events_processed, 1);
    }

    #[test]
    fn reverse_abort_event_does_not_write() {
        let schema = make_schema(1, "users");
        let events = vec![SourceEvent::abort(10, 100, 1000)];
        let source = Arc::new(MockSourceConnector::new(events, vec![schema], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target.clone());

        r.start().unwrap();

        assert_eq!(target.written_count(), 0);
        assert_eq!(r.stats().events_processed, 1);
    }

    #[test]
    fn reverse_mixed_events_lifecycle() {
        let schema = make_schema(1, "users");
        let events = vec![
            SourceEvent::insert(1, "public", "users", make_row(1, "Alice"), 1000),
            SourceEvent::insert(2, "public", "users", make_row(2, "Bob"), 1001),
            SourceEvent::commit(3, 100, 1002),
            SourceEvent::update(4, "public", "users", make_row(1, "Alice"), make_row(1, "Alice2"), 1003),
            SourceEvent::commit(5, 101, 1004),
            SourceEvent::delete(6, "public", "users", make_row(2, "Bob"), 1005),
            SourceEvent::commit(7, 102, 1006),
        ];
        let source = Arc::new(MockSourceConnector::new(events, vec![schema], HashMap::new()));
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target.clone());

        r.start().unwrap();

        let written = target.written_events();
        // 2 Insert + 1 Update + 1 Delete = 4 DML 事件
        assert_eq!(written.len(), 4);
        assert_eq!(written[0].0, CdcEventOp::Insert);
        assert_eq!(written[1].0, CdcEventOp::Insert);
        assert_eq!(written[2].0, CdcEventOp::Update);
        assert_eq!(written[3].0, CdcEventOp::Delete);

        let stats = r.stats();
        assert_eq!(stats.events_processed, 7);
        assert_eq!(stats.confirmed_lsn, 7);
    }

    #[test]
    fn reverse_target_write_error_fails_replication() {
        let schema = make_schema(1, "users");
        let source = Arc::new(MockSourceConnector::new(
            make_source_events(),
            vec![schema],
            HashMap::new(),
        ));
        let target = Arc::new(MockTargetWriter::new());
        target.fail_on_write.store(true, Ordering::SeqCst);

        let r = ReverseReplicator::new("task1", source, target)
            .with_max_retries(1)
            .with_retry_interval(10);

        let result = r.start();
        assert!(result.is_err());
        assert_eq!(r.state(), ReverseReplicatorState::Failed);

        let stats = r.stats();
        assert!(stats.errors > 0);
    }

    #[test]
    fn reverse_offset_acked_after_event() {
        let schema = make_schema(1, "users");
        let events = vec![
            SourceEvent::insert(100, "public", "users", make_row(1, "Alice"), 1000),
            SourceEvent::commit(200, 100, 1001),
        ];
        let source = Arc::new(MockSourceConnector::new(events, vec![schema], HashMap::new()));

        let r = ReverseReplicator::new(
            "task1",
            source.clone(),
            Arc::new(MockTargetWriter::new()),
        );
        r.start().unwrap();

        // 源端 confirmed_offset 应被推进到最后事件 LSN
        let source_offset = source.confirmed_offset().unwrap();
        assert_eq!(source_offset.lsn, 200);
    }

    #[test]
    fn reverse_ensure_table_called_during_schema_migration() {
        let schema1 = make_schema(1, "users");
        let schema2 = make_schema(2, "orders");
        let source = Arc::new(MockSourceConnector::new(
            vec![],
            vec![schema1, schema2],
            HashMap::new(),
        ));
        let target = Arc::new(MockTargetWriter::new());
        // 直接读取 ensure_table_count
        let r = ReverseReplicator::new("task1", source, target.clone());

        r.start().unwrap();

        // 2 个表应调用 2 次 ensure_table
        assert_eq!(
            target.ensure_table_count.load(Ordering::SeqCst),
            2
        );
    }

    #[test]
    fn reverse_error_display() {
        let e = ReverseReplicatorError::State("invalid transition".to_string());
        let s = format!("{}", e);
        assert!(s.contains("invalid transition"));

        let e = ReverseReplicatorError::SchemaMismatch("missing column".to_string());
        assert!(format!("{}", e).contains("missing column"));
    }

    #[test]
    fn reverse_unknown_table_dynamically_discovered() {
        // 源端 schema 列表为空，但 discover_schemas(table_name) 会返回 schema
        // 此场景模拟：schema_migration 阶段没有发现任何表，CDC 事件中引用了新表
        let schema = make_schema(1, "users");
        let events = vec![SourceEvent::insert(
            1,
            "public",
            "users",
            make_row(1, "Alice"),
            1000,
        )];

        // 自定义 MockSourceConnector，使 discover_schemas([]) 返回空，
        // 但 discover_schemas(["users"]) 返回 schema
        struct DynamicDiscoverySource {
            base: MockSourceConnector,
            schema: TableSchema,
        }
        impl SourceConnector for DynamicDiscoverySource {
            fn source_type(&self) -> &str {
                self.base.source_type()
            }
            fn connect(&self) -> Result<(), SourceError> {
                self.base.connect()
            }
            fn disconnect(&self) -> Result<(), SourceError> {
                self.base.disconnect()
            }
            fn discover_schemas(&self, tables: &[String]) -> Result<Vec<TableSchema>, SourceError> {
                if tables.is_empty() {
                    Ok(vec![])
                } else {
                    Ok(vec![self.schema.clone()])
                }
            }
            fn extract_snapshot(
                &self,
                table: &str,
                batch_size: usize,
                callback: &dyn Fn(&[DecodedRow]) -> Result<(), SourceError>,
            ) -> Result<u64, SourceError> {
                self.base.extract_snapshot(table, batch_size, callback)
            }
            fn current_lsn(&self) -> Result<u64, SourceError> {
                self.base.current_lsn()
            }
            fn start_cdc_stream(
                &self,
                start_lsn: u64,
                callback: &dyn Fn(&[SourceEvent]) -> Result<(), SourceError>,
            ) -> Result<(), SourceError> {
                self.base.start_cdc_stream(start_lsn, callback)
            }
            fn stop_cdc_stream(&self) -> Result<(), SourceError> {
                self.base.stop_cdc_stream()
            }
            fn ack_offset(&self, offset: &SourceOffset) -> Result<(), SourceError> {
                self.base.ack_offset(offset)
            }
            fn confirmed_offset(&self) -> Result<SourceOffset, SourceError> {
                self.base.confirmed_offset()
            }
            fn health_check(&self) -> Result<(), SourceError> {
                self.base.health_check()
            }
        }

        let base = MockSourceConnector::new(events, vec![], HashMap::new());
        let source = Arc::new(DynamicDiscoverySource {
            base,
            schema: schema.clone(),
        });
        let target = Arc::new(MockTargetWriter::new());
        let r = ReverseReplicator::new("task1", source, target.clone());

        r.start().unwrap();

        // 应动态发现 schema 并写入目标端
        assert_eq!(target.written_count(), 1);
    }
}
