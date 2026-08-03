//! MCP Server 详细实现 — Phase 7d.22
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.22 MCP Server 详细实现设计。
//!
//! # 设计
//!
//! 在 Phase 7b.6 基础 MCP Server（4 工具）之上，扩展为 30 个 LLM 工具，
//! 覆盖数据库运维全生命周期的 8 大类别：
//!
//! ## 8 个类别 × 30 个工具
//!
//! | # | 类别 | 工具 | 说明 |
//! |---|------|------|------|
//! | 1 | Schema | list_tables / describe_table / list_indexes / list_views | 表结构与元数据 |
//! | 2 | Query | execute_sql / explain_query / prepare_statement / cancel_query | 查询执行 |
//! | 3 | SlowQuery | slow_queries / top_queries / query_stats / reset_stats | 慢查询与统计 |
//! | 4 | TxLock | list_transactions / list_locks / kill_transaction / deadlock_history | 事务与锁 |
//! | 5 | Perf | wait_events / ash_report / active_sessions / pprof_dump | 性能与等待事件 |
//! | 6 | Maintenance | vacuum_table / analyze_table / autovacuum_status | 维护与自动清理 |
//! | 7 | Alerting | list_alerts / db_stats / capacity_predict | 告警与容量预测 |
//! | 8 | Insight | summarize_table / ask_data / explain_root_cause / get_lineage | 数据洞察（TDengine 启发） |
//!
//! ## 协议
//!
//! 复用 Phase 7b.6 的 JSON-RPC 2.0 over stdio 协议层：
//! - `initialize` / `tools/list` / `tools/call` / `shutdown`
//! - 工具定义包含 `category` 自定义字段，便于 LLM 按类别检索
//!
//! ## 验证标准
//!
//! - MCP Server 启动 → list_tools 返回 30 个工具
//! - query 工具执行 SQL → 返回结果
//! - slow_queries 返回慢查询
//! - 8 个类别全覆盖

use crate::mcp::{
    JsonRpcRequest, JsonRpcResponse, McpError, ToolCallResult, ToolDefinition,
    MCP_PROTOCOL_VERSION, MCP_SERVER_NAME, MCP_SERVER_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

// =====================================================================
//  ToolCategory — 工具类别（7 个类别）
// =====================================================================

/// MCP 工具类别 — 9 个类别覆盖数据库运维全生命周期
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolCategory {
    /// 类别 1：表结构与元数据
    Schema,
    /// 类别 2：查询执行
    Query,
    /// 类别 3：慢查询与统计
    SlowQuery,
    /// 类别 4：事务与锁
    TxLock,
    /// 类别 5：性能与等待事件
    Perf,
    /// 类别 6：维护与自动清理
    Maintenance,
    /// 类别 7：告警与容量预测
    Alerting,
    /// 类别 8：数据洞察（TDengine 启发 — 从"展示"到"洞察"）
    Insight,
    /// 类别 9：数据复制（NineData 启发 — CDC 任务管理）
    Replication,
}

impl ToolCategory {
    /// 类别名称（英文标识）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::Query => "query",
            Self::SlowQuery => "slow_query",
            Self::TxLock => "tx_lock",
            Self::Perf => "perf",
            Self::Maintenance => "maintenance",
            Self::Alerting => "alerting",
            Self::Insight => "insight",
            Self::Replication => "replication",
        }
    }

    /// 类别中文描述
    pub fn description(&self) -> &'static str {
        match self {
            Self::Schema => "表结构与元数据",
            Self::Query => "查询执行",
            Self::SlowQuery => "慢查询与统计",
            Self::TxLock => "事务与锁",
            Self::Perf => "性能与等待事件",
            Self::Maintenance => "维护与自动清理",
            Self::Alerting => "告警与容量预测",
            Self::Insight => "数据洞察",
            Self::Replication => "数据复制（CDC 任务管理）",
        }
    }

    /// 所有类别
    pub fn all() -> &'static [ToolCategory] {
        &[
            Self::Schema,
            Self::Query,
            Self::SlowQuery,
            Self::TxLock,
            Self::Perf,
            Self::Maintenance,
            Self::Alerting,
            Self::Insight,
            Self::Replication,
        ]
    }
}

// =====================================================================
//  DTO 类型 — 22 个新增数据传输对象
// =====================================================================

// --- 类别 1: Schema ---

/// 索引信息（对应 pg_indexes + pg_stat_user_indexes）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexInfo {
    pub name: String,
    pub table: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub is_primary: bool,
}

/// 视图信息（对应 pg_views）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewInfo {
    pub name: String,
    pub definition: String,
    pub owner: String,
}

// --- 类别 2: Query ---

/// 执行计划（对应 EXPLAIN 输出）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainPlan {
    pub sql: String,
    pub cost: f64,
    pub rows: u64,
    pub operators: Vec<String>,
}

/// 预处理语句结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareResult {
    pub name: String,
    pub parameter_count: usize,
}

/// 取消查询结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelResult {
    pub query_id: u64,
    pub cancelled: bool,
}

// --- 类别 3: SlowQuery ---

/// 慢查询记录（对应 MySQL slow_query_log / pg_stat_statements）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowQueryRecord {
    pub sql: String,
    pub elapsed_ms: u64,
    pub timestamp: u64,
    pub rows_scanned: u64,
    pub plan_operator: String,
}

/// 高频查询记录（Top N by calls）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopQueryRecord {
    pub sql: String,
    pub calls: u64,
    pub total_time_ms: f64,
    pub mean_time_ms: f64,
    pub rows: u64,
}

/// 查询统计摘要（pg_stat_statements 聚合）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryStatsSummary {
    pub total_queries: u64,
    pub total_time_ms: f64,
    pub unique_queries: usize,
    pub avg_time_ms: f64,
}

/// 重置统计结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetResult {
    pub reset: bool,
}

// --- 类别 4: TxLock ---

/// 事务信息（对应 pg_stat_activity）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionInfo {
    pub txn_id: u32,
    pub state: String,
    pub started_at: u64,
    pub sql: String,
    pub wait_event: Option<String>,
    /// 隔离级别（P3-Tx-Enhancement）— 仅 ExecutorBackend 提供
    pub isolation: Option<String>,
    /// 快照中活跃事务数（P3-Tx-Enhancement）— 仅 ExecutorBackend 提供
    pub snapshot_active_count: Option<u32>,
    /// 快照 xmax（P3-Tx-Enhancement）— 仅 ExecutorBackend 提供
    pub snapshot_xmax: Option<u32>,
}

/// 锁信息（对应 pg_locks）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    pub txn_id: u32,
    pub table: String,
    pub mode: String,
    pub granted: bool,
    pub wait_start: Option<u64>,
}

/// 终止事务结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KillResult {
    pub txn_id: u32,
    pub killed: bool,
}

/// 死锁历史记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadlockRecord {
    pub timestamp: u64,
    pub txn_ids: Vec<u32>,
    pub resource: String,
}

// --- 类别 5: Perf ---

/// 等待事件摘要（对应 pg_stat_wait）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitEventSummary {
    pub event: String,
    pub total_waits: u64,
    pub total_wait_ms: u64,
    pub avg_wait_ms: f64,
}

/// ASH 报告（Active Session History）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AshReport {
    pub duration_secs: u64,
    pub sample_count: usize,
    pub top_sql: Vec<String>,
    pub top_wait_events: Vec<String>,
}

/// 会话信息（对应 pg_stat_activity）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: u32,
    pub state: String,
    pub sql: String,
    pub wait_event: Option<String>,
    pub user: String,
}

/// pprof 性能剖析结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PprofResult {
    pub sample_count: usize,
    pub duration_secs: u64,
    pub top_functions: Vec<String>,
}

// --- 类别 6: Maintenance ---

/// VACUUM 结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VacuumResult {
    pub table: String,
    pub dead_tuples_reclaimed: u64,
    pub elapsed_ms: u64,
}

/// ANALYZE 结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzeResult {
    pub table: String,
    pub rows_analyzed: u64,
    pub columns_analyzed: usize,
}

/// Autovacuum 状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutovacuumStatus {
    pub enabled: bool,
    pub last_run: u64,
    pub tables_vacuumed: usize,
    pub tables_analyzed: usize,
}

// --- 类别 7: Alerting ---

/// 告警信息（对应 AlertManager alerts）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertInfo {
    pub level: String,
    pub rule_id: String,
    pub message: String,
    pub timestamp: u64,
    pub value: f64,
    pub threshold: f64,
}

/// 容量预测结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityForecast {
    pub metric: String,
    pub current_value: f64,
    pub predicted_value: f64,
    pub days_ahead: u32,
    pub confidence: f64,
    /// 当前存储大小（字节）（P3-Capacity-Enhanced）— 基于 live/dead tuples 估算
    pub storage_bytes_current: Option<f64>,
    /// 预测存储大小（字节）（P3-Capacity-Enhanced）
    pub storage_bytes_predicted: Option<f64>,
    /// 每天净增长率（行数/天）（P3-Capacity-Enhanced）— INSERT - DELETE
    pub net_growth_rate_per_day: Option<f64>,
    /// 按表分解预测（P3-Capacity-Enhanced）— None 表示未启用按表分解
    pub table_breakdown: Option<Vec<TableForecast>>,
}

/// 单表容量预测（P3-Capacity-Enhanced）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableForecast {
    /// 表名
    pub table: String,
    /// 当前行数
    pub current_rows: f64,
    /// 预测行数
    pub predicted_rows: f64,
    /// 当前存储大小（字节）
    pub current_bytes: f64,
    /// 预测存储大小（字节）
    pub predicted_bytes: f64,
    /// 每天净增长率（行数/天）
    pub growth_rate_per_day: f64,
}

// --- 类别 8: Insight（TDengine 启发：从"展示"到"洞察"） ---

/// 列级统计摘要 — 让 LLM 理解数据分布而非仅看类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSummary {
    pub name: String,
    pub data_type: String,
    pub null_count: u64,
    pub distinct_count: u64,
    pub min_value: Option<String>,
    pub max_value: Option<String>,
    pub top_values: Vec<(String, u64)>,
}

/// 表级数据摘要 — 自动生成，LLM 无需写 SQL 即可理解数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSummary {
    pub table: String,
    pub row_count: u64,
    pub columns: Vec<ColumnSummary>,
}

/// 自然语言问答引用 — 数据来源追溯
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskCitation {
    pub table: String,
    pub row_id: u64,
    pub snippet: String,
    pub score: f32,
}

/// 自然语言问答结果 — Agent Interface 统一入口
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskAnswer {
    pub answer: String,
    pub sql: Option<String>,
    pub citations: Vec<AskCitation>,
}

/// 根因类型（TDengine 启发 — 不仅告诉"发生了什么"，还能分析"为什么"）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CauseType {
    MissingIndex,
    LockContention,
    HighQps,
    StatsStale,
    Deadlock,
    /// 资源竞争（CPU/IO/内存等系统资源瓶颈）— P3-RootCause-Enhanced
    ResourceContention,
}

/// 单条根因推断
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CauseEntry {
    pub cause_type: CauseType,
    pub description: String,
    pub confidence: f64,
}

/// 证据（关联多源数据）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub source: String,
    pub detail: String,
}

/// 根因分析报告 — Agent Interface 洞察层
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootCauseReport {
    pub alert: AlertInfo,
    pub likely_causes: Vec<CauseEntry>,
    pub evidence: Vec<Evidence>,
}

// --- P5: 数据血缘追踪 DTO（TDengine 启发 — 字段级血缘暴露给 LLM） ---

/// 血缘边来源类型 — 标记血缘如何产生
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineageEdgeSource {
    /// CTAS（CREATE TABLE AS SELECT）
    Ctas,
    /// 视图（CREATE VIEW AS SELECT）
    View,
    /// CDC 演化（ALTER TABLE 列迁移）
    Cdc,
    /// 手动标注
    Manual,
}

impl LineageEdgeSource {
    /// 转字符串
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ctas => "ctas",
            Self::View => "view",
            Self::Cdc => "cdc",
            Self::Manual => "manual",
        }
    }
}

/// 字段引用 — 表名 + 列名（字段级血缘最小单元）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnRefDto {
    /// 表名
    pub table: String,
    /// 列名
    pub column: String,
}

/// 血缘边 — 一条"target ← source + transform"的有向边
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageEdgeDto {
    /// 上游来源（字段级）
    pub source: ColumnRefDto,
    /// 下游目标（字段级）
    pub target: ColumnRefDto,
    /// 转换描述（如 "SUM(price)" / "direct" / "CAST AS BIGINT"）
    pub transform: String,
    /// 血缘来源类型
    pub source_type: LineageEdgeSource,
}

/// 血缘查询结果 — 包含上游 + 下游 + 全量边
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageInfo {
    /// 查询的表名（None 表示查询全量血缘）
    pub table: Option<String>,
    /// 该表的上游血缘（target = table）
    pub upstream: Vec<LineageEdgeDto>,
    /// 该表的下游血缘（source = table）
    pub downstream: Vec<LineageEdgeDto>,
    /// 血缘涉及的所有表（去重排序）
    pub tables: Vec<String>,
    /// 全量血缘边数（即使按表过滤，也报告总量供 LLM 参考）
    pub total_edges: usize,
}

// --- 类别 9: Replication ---

/// 复制任务信息 — 用于 list/monitor 返回
///
/// 与 `szrsql_cdc::task::TaskInfo` 字段对齐，但作为独立的 DTO 用于 MCP 协议序列化
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationTaskInfo {
    /// 任务 ID
    pub task_id: String,
    /// 任务描述
    pub description: String,
    /// 任务状态（created/starting/running/paused/stopped/failed）
    pub state: String,
    /// 目标端类型（postgres/mysql/kafka/memory）
    pub target_type: String,
    /// 目标端连接串
    pub target_connection: String,
    /// 创建时间（Unix 毫秒）
    pub created_at: u64,
    /// 表过滤（None 表示复制所有表）
    pub table_filter: Option<Vec<String>>,
    /// 已接收事件数
    pub events_received: u64,
    /// 已写入事件数
    pub events_written: u64,
    /// 已处理字节数
    pub bytes_processed: u64,
    /// 已处理事务数
    pub transactions_processed: u64,
    /// 错误次数
    pub error_count: u64,
    /// 最后一次错误消息
    pub last_error: Option<String>,
    /// 最后写入时间戳
    pub last_write_at: u64,
    /// 最后接收事件的 LSN
    pub last_lsn: u64,
    /// 已确认 flush 的 LSN
    pub confirmed_flush_lsn: u64,
    /// 当前滞后量（last_lsn - confirmed_flush_lsn）
    pub lag: u64,
    /// 全量快照点 LSN（P4-1）— 0 表示未启用快照+增量衔接
    pub snapshot_lsn: u64,
}

/// 创建复制任务请求参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateReplicationTaskParams {
    /// 任务 ID（唯一）
    pub task_id: String,
    /// 任务描述
    pub description: String,
    /// 目标端类型（postgres/mysql/kafka/memory）
    pub target_type: String,
    /// 目标端连接串
    pub target_connection: String,
    /// 表过滤（None 表示复制所有表）
    pub table_filter: Option<Vec<String>>,
    /// 是否在全量同步完成后才开启增量
    pub snapshot_first: bool,
}

/// 创建复制任务结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateReplicationTaskResult {
    pub task_id: String,
    pub state: String,
    pub created: bool,
}

/// 停止复制任务结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopReplicationTaskResult {
    pub task_id: String,
    pub state: String,
    pub stopped: bool,
}

/// 复制管理器统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationManagerStats {
    pub total_tasks: usize,
    pub total_created: u64,
    pub total_started: u64,
    pub total_stopped: u64,
    pub total_failed: u64,
    pub running_tasks: usize,
}

// =====================================================================
//  McpBackendV2 — 扩展后端接口（30 个工具方法）
// =====================================================================

/// MCP 扩展后端 — 提供 30 个工具的实际执行能力
///
/// 工具通过后端执行实际操作，便于测试时注入 Mock 后端。
/// 实现方可以桥接到真实的 szrsql-ops / szrsql-tx 模块。
pub trait McpBackendV2: Send {
    // --- 类别 1: Schema ---
    /// 列出所有表
    fn list_tables(&self) -> Result<Vec<crate::mcp::TableInfo>, McpError>;
    /// 描述表结构
    fn describe_table(&self, table: &str) -> Result<crate::mcp::TableSchema, McpError>;
    /// 列出表的索引
    fn list_indexes(&self, table: &str) -> Result<Vec<IndexInfo>, McpError>;
    /// 列出所有视图
    fn list_views(&self) -> Result<Vec<ViewInfo>, McpError>;

    // --- 类别 2: Query ---
    /// 执行 SQL
    fn execute_sql(&self, sql: &str) -> Result<crate::mcp::QueryResult, McpError>;
    /// 获取执行计划
    fn explain_query(&self, sql: &str) -> Result<ExplainPlan, McpError>;
    /// 预处理语句
    fn prepare_statement(&self, name: &str, sql: &str) -> Result<PrepareResult, McpError>;
    /// 取消查询
    fn cancel_query(&self, query_id: u64) -> Result<CancelResult, McpError>;

    // --- 类别 3: SlowQuery ---
    /// 慢查询列表
    fn slow_queries(&self, limit: usize) -> Result<Vec<SlowQueryRecord>, McpError>;
    /// 高频查询
    fn top_queries(&self, limit: usize) -> Result<Vec<TopQueryRecord>, McpError>;
    /// 查询统计
    fn query_stats(&self) -> Result<QueryStatsSummary, McpError>;
    /// 重置统计
    fn reset_stats(&self) -> Result<ResetResult, McpError>;

    // --- 类别 4: TxLock ---
    /// 活跃事务列表
    fn list_transactions(&self) -> Result<Vec<TransactionInfo>, McpError>;
    /// 锁信息列表
    fn list_locks(&self) -> Result<Vec<LockInfo>, McpError>;
    /// 终止事务
    fn kill_transaction(&self, txn_id: u32) -> Result<KillResult, McpError>;
    /// 死锁历史
    fn deadlock_history(&self) -> Result<Vec<DeadlockRecord>, McpError>;

    // --- 类别 5: Perf ---
    /// 等待事件统计
    fn wait_events(&self) -> Result<Vec<WaitEventSummary>, McpError>;
    /// ASH 报告
    fn ash_report(&self, duration_secs: u64) -> Result<AshReport, McpError>;
    /// 活跃会话
    fn active_sessions(&self) -> Result<Vec<SessionInfo>, McpError>;
    /// pprof 性能剖析
    fn pprof_dump(&self, duration_secs: u64) -> Result<PprofResult, McpError>;

    // --- 类别 6: Maintenance ---
    /// VACUUM 表
    fn vacuum_table(&self, table: &str) -> Result<VacuumResult, McpError>;
    /// ANALYZE 表
    fn analyze_table(&self, table: &str) -> Result<AnalyzeResult, McpError>;
    /// Autovacuum 状态
    fn autovacuum_status(&self) -> Result<AutovacuumStatus, McpError>;

    // --- 类别 7: Alerting ---
    /// 告警列表
    fn list_alerts(&self) -> Result<Vec<AlertInfo>, McpError>;
    /// 数据库统计
    fn db_stats(&self) -> Result<crate::mcp::DbStats, McpError>;
    /// 容量预测
    fn capacity_predict(&self, days: u32) -> Result<CapacityForecast, McpError>;

    // --- 类别 8: Insight（TDengine 启发） ---
    /// 表数据摘要 — 自动统计各列基数/分布/top 值
    fn summarize_table(&self, table: &str) -> Result<TableSummary, McpError>;
    /// 自然语言问答 — Agent Interface 统一入口
    fn ask_data(&self, question: &str) -> Result<AskAnswer, McpError>;
    /// 根因分析 — 关联 alerts + slow_queries + wait_events 三源数据
    fn explain_root_cause(&self, alert_id: &str) -> Result<RootCauseReport, McpError>;
    /// 数据血缘查询 — 输入表名返回上下游字段级血缘；None 返回全量血缘
    fn get_lineage(&self, table: Option<&str>) -> Result<LineageInfo, McpError>;

    // --- 类别 9: Replication（NineData 启发 — CDC 任务管理） ---
    /// 创建复制任务 — 创建一个源端→目标端的 CDC 复制链路
    ///
    /// 默认实现返回 `BackendError`，需要后端注入 `ReplicationTaskManager` 才能使用。
    fn create_replication_task(
        &self,
        params: CreateReplicationTaskParams,
    ) -> Result<CreateReplicationTaskResult, McpError> {
        Err(McpError::BackendError(format!(
            "create_replication_task not available: no ReplicationTaskManager attached (params={:?})",
            params.task_id
        )))
    }
    /// 列出所有复制任务
    fn list_replication_tasks(&self) -> Result<Vec<ReplicationTaskInfo>, McpError> {
        Err(McpError::BackendError(
            "list_replication_tasks not available: no ReplicationTaskManager attached".to_string(),
        ))
    }
    /// 监控指定复制任务（详细统计）
    fn monitor_replication_task(&self, task_id: &str) -> Result<ReplicationTaskInfo, McpError> {
        Err(McpError::BackendError(format!(
            "monitor_replication_task not available: no ReplicationTaskManager attached (task_id={})",
            task_id
        )))
    }
    /// 停止复制任务
    fn stop_replication_task(&self, task_id: &str) -> Result<StopReplicationTaskResult, McpError> {
        Err(McpError::BackendError(format!(
            "stop_replication_task not available: no ReplicationTaskManager attached (task_id={})",
            task_id
        )))
    }
    /// 获取复制管理器统计（默认实现返回 Err）
    fn replication_manager_stats(&self) -> Result<ReplicationManagerStats, McpError> {
        Err(McpError::BackendError(
            "replication_manager_stats not available: no ReplicationTaskManager attached"
                .to_string(),
        ))
    }
}

// =====================================================================
//  ToolDefinitionV2 — 带类别的工具定义
// =====================================================================

/// 带类别的工具定义
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinitionV2 {
    #[serde(flatten)]
    pub base: ToolDefinition,
    /// 工具所属类别
    pub category: ToolCategory,
}

// =====================================================================
//  MockBackendV2 — 内存 Mock 后端（用于测试）
// =====================================================================

/// 内存 Mock 后端 — 模拟 30 个工具的返回数据，用于测试和演示
pub struct MockBackendV2 {
    tables: HashMap<String, crate::mcp::TableSchema>,
    row_counts: HashMap<String, u64>,
    indexes: HashMap<String, Vec<IndexInfo>>,
    views: Vec<ViewInfo>,
    slow_query_log: Vec<SlowQueryRecord>,
    query_stats_collector: HashMap<String, (u64, f64)>,
    transactions: Vec<TransactionInfo>,
    locks: Vec<LockInfo>,
    deadlocks: Vec<DeadlockRecord>,
    wait_events: Vec<WaitEventSummary>,
    sessions: Vec<SessionInfo>,
    alerts: Vec<AlertInfo>,
    prepared_statements: HashMap<String, String>,
    stats_reset: bool,
}

impl Default for MockBackendV2 {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBackendV2 {
    /// 构造带示例数据的 Mock 后端
    pub fn new() -> Self {
        let mut tables = HashMap::new();
        let mut row_counts = HashMap::new();
        let mut indexes = HashMap::new();

        // 示例表：products
        tables.insert(
            "products".to_string(),
            crate::mcp::TableSchema {
                table: "products".to_string(),
                columns: vec![
                    crate::mcp::ColumnDef {
                        name: "id".to_string(),
                        data_type: "BIGINT".to_string(),
                        nullable: false,
                        primary_key: true,
                        comment: None,
                    },
                    crate::mcp::ColumnDef {
                        name: "name".to_string(),
                        data_type: "VARCHAR(255)".to_string(),
                        nullable: false,
                        primary_key: false,
                        comment: None,
                    },
                    crate::mcp::ColumnDef {
                        name: "price".to_string(),
                        data_type: "DECIMAL(10,2)".to_string(),
                        nullable: true,
                        primary_key: false,
                        comment: None,
                    },
                ],
            },
        );
        row_counts.insert("products".to_string(), 1000);
        indexes.insert(
            "products".to_string(),
            vec![IndexInfo {
                name: "idx_products_id".to_string(),
                table: "products".to_string(),
                columns: vec!["id".to_string()],
                unique: true,
                is_primary: true,
            }],
        );

        // 示例表：orders
        tables.insert(
            "orders".to_string(),
            crate::mcp::TableSchema {
                table: "orders".to_string(),
                columns: vec![
                    crate::mcp::ColumnDef {
                        name: "order_id".to_string(),
                        data_type: "BIGINT".to_string(),
                        nullable: false,
                        primary_key: true,
                        comment: None,
                    },
                    crate::mcp::ColumnDef {
                        name: "customer_id".to_string(),
                        data_type: "BIGINT".to_string(),
                        nullable: false,
                        primary_key: false,
                        comment: None,
                    },
                    crate::mcp::ColumnDef {
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
        indexes.insert(
            "orders".to_string(),
            vec![IndexInfo {
                name: "idx_orders_customer".to_string(),
                table: "orders".to_string(),
                columns: vec!["customer_id".to_string()],
                unique: false,
                is_primary: false,
            }],
        );

        let slow_query_log = vec![SlowQueryRecord {
            sql: "SELECT * FROM orders WHERE total > 100".to_string(),
            elapsed_ms: 350,
            timestamp: 1700000000,
            rows_scanned: 5000,
            plan_operator: "Seq Scan".to_string(),
        }];

        let query_stats_collector = HashMap::from([
            ("SELECT * FROM products".to_string(), (150u64, 320.5f64)),
            (
                "SELECT * FROM orders WHERE customer_id = ?".to_string(),
                (80u64, 210.0f64),
            ),
        ]);

        let transactions = vec![TransactionInfo {
            txn_id: 1001,
            state: "active".to_string(),
            started_at: 1700000000,
            sql: "UPDATE products SET price = 9.9 WHERE id = 1".to_string(),
            wait_event: None,
            isolation: None,
            snapshot_active_count: None,
            snapshot_xmax: None,
        }];

        let locks = vec![LockInfo {
            txn_id: 1001,
            table: "products".to_string(),
            mode: "Exclusive".to_string(),
            granted: true,
            wait_start: None,
        }];

        let deadlocks = vec![DeadlockRecord {
            timestamp: 1700000100,
            txn_ids: vec![1001, 1002],
            resource: "products:row:1".to_string(),
        }];

        let wait_events = vec![
            WaitEventSummary {
                event: "db file sequential read".to_string(),
                total_waits: 500,
                total_wait_ms: 2500,
                avg_wait_ms: 5.0,
            },
            WaitEventSummary {
                event: "log file sync".to_string(),
                total_waits: 100,
                total_wait_ms: 1500,
                avg_wait_ms: 15.0,
            },
        ];

        let sessions = vec![SessionInfo {
            session_id: 1,
            state: "ACTIVE".to_string(),
            sql: "SELECT * FROM products".to_string(),
            wait_event: None,
            user: "admin".to_string(),
        }];

        let alerts = vec![AlertInfo {
            level: "warning".to_string(),
            rule_id: "high_qps".to_string(),
            message: "QPS exceeds threshold".to_string(),
            timestamp: 1700000200,
            value: 12000.0,
            threshold: 10000.0,
        }];

        Self {
            tables,
            row_counts,
            indexes,
            views: vec![ViewInfo {
                name: "v_product_summary".to_string(),
                definition: "SELECT name, price FROM products".to_string(),
                owner: "admin".to_string(),
            }],
            slow_query_log,
            query_stats_collector,
            transactions,
            locks,
            deadlocks,
            wait_events,
            sessions,
            alerts,
            prepared_statements: HashMap::new(),
            stats_reset: false,
        }
    }
}

impl McpBackendV2 for MockBackendV2 {
    // --- 类别 1: Schema ---

    fn list_tables(&self) -> Result<Vec<crate::mcp::TableInfo>, McpError> {
        let tables: Vec<crate::mcp::TableInfo> = self
            .tables
            .values()
            .map(|schema| crate::mcp::TableInfo {
                name: schema.table.clone(),
                row_count: *self.row_counts.get(&schema.table).unwrap_or(&0),
                size_bytes: schema.columns.len() as u64 * 1024,
            })
            .collect();
        Ok(tables)
    }

    fn describe_table(&self, table: &str) -> Result<crate::mcp::TableSchema, McpError> {
        self.tables
            .get(table)
            .cloned()
            .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))
    }

    fn list_indexes(&self, table: &str) -> Result<Vec<IndexInfo>, McpError> {
        if !self.tables.contains_key(table) {
            return Err(McpError::BackendError(format!("table not found: {table}")));
        }
        Ok(self.indexes.get(table).cloned().unwrap_or_default())
    }

    fn list_views(&self) -> Result<Vec<ViewInfo>, McpError> {
        Ok(self.views.clone())
    }

    // --- 类别 2: Query ---

    fn execute_sql(&self, sql: &str) -> Result<crate::mcp::QueryResult, McpError> {
        let sql_lower = sql.to_lowercase();
        if sql_lower.contains("select") {
            if sql_lower.contains("from products") {
                return Ok(crate::mcp::QueryResult {
                    columns: vec!["id".to_string(), "name".to_string(), "price".to_string()],
                    rows: vec![
                        vec![json!(1), json!("苹果汁"), json!(5.5)],
                        vec![json!(2), json!("橙汁"), json!(6.0)],
                    ],
                    affected_rows: 0,
                    elapsed_ms: 2,
                });
            }
            if sql_lower.contains("from orders") {
                return Ok(crate::mcp::QueryResult {
                    columns: vec![
                        "order_id".to_string(),
                        "customer_id".to_string(),
                        "total".to_string(),
                    ],
                    rows: vec![vec![json!(1001), json!(1), json!(55.5)]],
                    affected_rows: 0,
                    elapsed_ms: 3,
                });
            }
            return Ok(crate::mcp::QueryResult {
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
            return Ok(crate::mcp::QueryResult {
                columns: vec![],
                rows: vec![],
                affected_rows: 1,
                elapsed_ms: 1,
            });
        }
        Err(McpError::BackendError(format!("unsupported SQL: {sql}")))
    }

    fn explain_query(&self, sql: &str) -> Result<ExplainPlan, McpError> {
        if sql.trim().is_empty() {
            return Err(McpError::InvalidToolParams("sql is empty".to_string()));
        }
        let sql_lower = sql.to_lowercase();
        let (cost, rows, operators) = if sql_lower.contains("where") {
            (
                15.5,
                100,
                vec!["Index Scan".to_string(), "Filter".to_string()],
            )
        } else {
            (50.0, 1000, vec!["Seq Scan".to_string()])
        };
        Ok(ExplainPlan {
            sql: sql.to_string(),
            cost,
            rows,
            operators,
        })
    }

    fn prepare_statement(&self, name: &str, sql: &str) -> Result<PrepareResult, McpError> {
        if name.trim().is_empty() {
            return Err(McpError::InvalidToolParams("name is empty".to_string()));
        }
        if sql.trim().is_empty() {
            return Err(McpError::InvalidToolParams("sql is empty".to_string()));
        }
        let param_count = sql.matches('?').count();
        Ok(PrepareResult {
            name: name.to_string(),
            parameter_count: param_count,
        })
    }

    fn cancel_query(&self, query_id: u64) -> Result<CancelResult, McpError> {
        Ok(CancelResult {
            query_id,
            cancelled: true,
        })
    }

    // --- 类别 3: SlowQuery ---

    fn slow_queries(&self, limit: usize) -> Result<Vec<SlowQueryRecord>, McpError> {
        let result: Vec<SlowQueryRecord> =
            self.slow_query_log.iter().take(limit).cloned().collect();
        Ok(result)
    }

    fn top_queries(&self, limit: usize) -> Result<Vec<TopQueryRecord>, McpError> {
        let mut records: Vec<TopQueryRecord> = self
            .query_stats_collector
            .iter()
            .map(|(sql, (calls, total_time))| TopQueryRecord {
                sql: sql.clone(),
                calls: *calls,
                total_time_ms: *total_time,
                mean_time_ms: *total_time / *calls as f64,
                rows: 0,
            })
            .collect();
        records.sort_by_key(|r| std::cmp::Reverse(r.calls));
        records.truncate(limit);
        Ok(records)
    }

    fn query_stats(&self) -> Result<QueryStatsSummary, McpError> {
        let total_queries: u64 = self.query_stats_collector.values().map(|(c, _)| c).sum();
        let total_time: f64 = self.query_stats_collector.values().map(|(_, t)| t).sum();
        let unique = self.query_stats_collector.len();
        let avg = if total_queries > 0 {
            total_time / total_queries as f64
        } else {
            0.0
        };
        Ok(QueryStatsSummary {
            total_queries,
            total_time_ms: total_time,
            unique_queries: unique,
            avg_time_ms: avg,
        })
    }

    fn reset_stats(&self) -> Result<ResetResult, McpError> {
        Ok(ResetResult { reset: true })
    }

    // --- 类别 4: TxLock ---

    fn list_transactions(&self) -> Result<Vec<TransactionInfo>, McpError> {
        Ok(self.transactions.clone())
    }

    fn list_locks(&self) -> Result<Vec<LockInfo>, McpError> {
        Ok(self.locks.clone())
    }

    fn kill_transaction(&self, txn_id: u32) -> Result<KillResult, McpError> {
        Ok(KillResult {
            txn_id,
            killed: true,
        })
    }

    fn deadlock_history(&self) -> Result<Vec<DeadlockRecord>, McpError> {
        Ok(self.deadlocks.clone())
    }

    // --- 类别 5: Perf ---

    fn wait_events(&self) -> Result<Vec<WaitEventSummary>, McpError> {
        Ok(self.wait_events.clone())
    }

    fn ash_report(&self, duration_secs: u64) -> Result<AshReport, McpError> {
        Ok(AshReport {
            duration_secs,
            sample_count: (duration_secs as usize) * 10,
            top_sql: vec!["SELECT * FROM products".to_string()],
            top_wait_events: vec!["db file sequential read".to_string()],
        })
    }

    fn active_sessions(&self) -> Result<Vec<SessionInfo>, McpError> {
        Ok(self.sessions.clone())
    }

    fn pprof_dump(&self, duration_secs: u64) -> Result<PprofResult, McpError> {
        Ok(PprofResult {
            sample_count: (duration_secs as usize) * 100,
            duration_secs,
            top_functions: vec![
                "szrsql_storage::btree::BTree::insert".to_string(),
                "szrsql_sql::executor::execute_select".to_string(),
            ],
        })
    }

    // --- 类别 6: Maintenance ---

    fn vacuum_table(&self, table: &str) -> Result<VacuumResult, McpError> {
        if !self.tables.contains_key(table) {
            return Err(McpError::BackendError(format!("table not found: {table}")));
        }
        Ok(VacuumResult {
            table: table.to_string(),
            dead_tuples_reclaimed: 50,
            elapsed_ms: 15,
        })
    }

    fn analyze_table(&self, table: &str) -> Result<AnalyzeResult, McpError> {
        if !self.tables.contains_key(table) {
            return Err(McpError::BackendError(format!("table not found: {table}")));
        }
        let col_count = self.tables.get(table).map(|s| s.columns.len()).unwrap_or(0);
        let row_count = *self.row_counts.get(table).unwrap_or(&0);
        Ok(AnalyzeResult {
            table: table.to_string(),
            rows_analyzed: row_count,
            columns_analyzed: col_count,
        })
    }

    fn autovacuum_status(&self) -> Result<AutovacuumStatus, McpError> {
        Ok(AutovacuumStatus {
            enabled: true,
            last_run: 1700000300,
            tables_vacuumed: 2,
            tables_analyzed: 2,
        })
    }

    // --- 类别 7: Alerting ---

    fn list_alerts(&self) -> Result<Vec<AlertInfo>, McpError> {
        Ok(self.alerts.clone())
    }

    fn db_stats(&self) -> Result<crate::mcp::DbStats, McpError> {
        let total_rows: u64 = self.row_counts.values().sum();
        Ok(crate::mcp::DbStats {
            table_count: self.tables.len(),
            total_rows,
            total_size_bytes: self.tables.len() as u64 * 1024 * 1024,
            cache_hit_rate: 0.85,
            active_connections: 3,
        })
    }

    fn capacity_predict(&self, days: u32) -> Result<CapacityForecast, McpError> {
        Ok(CapacityForecast {
            metric: "disk_usage_gb".to_string(),
            current_value: 50.0,
            predicted_value: 50.0 + (days as f64) * 0.5,
            days_ahead: days,
            confidence: 0.92,
            storage_bytes_current: None,
            storage_bytes_predicted: None,
            net_growth_rate_per_day: None,
            table_breakdown: None,
        })
    }

    fn summarize_table(&self, table: &str) -> Result<TableSummary, McpError> {
        let schema = self
            .tables
            .get(table)
            .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))?;
        let row_count = self.row_counts.get(table).copied().unwrap_or(0);
        let columns = schema
            .columns
            .iter()
            .map(|col| {
                let distinct_count = if col.primary_key {
                    row_count
                } else if col.data_type.starts_with("VARCHAR") {
                    row_count / 10
                } else {
                    row_count / 100
                };
                let null_count = if col.nullable {
                    row_count / 20
                } else {
                    0
                };
                let min_value = if col.data_type.starts_with("BIGINT") {
                    Some("1".to_string())
                } else if col.data_type.starts_with("DECIMAL") {
                    Some("0.01".to_string())
                } else {
                    None
                };
                let max_value = if col.data_type.starts_with("BIGINT") {
                    Some(row_count.to_string())
                } else if col.data_type.starts_with("DECIMAL") {
                    Some("999.99".to_string())
                } else {
                    None
                };
                let top_values = if col.data_type.starts_with("VARCHAR") {
                    vec![("示例值A".to_string(), 120u64), ("示例值B".to_string(), 80)]
                } else {
                    vec![]
                };
                ColumnSummary {
                    name: col.name.clone(),
                    data_type: col.data_type.clone(),
                    null_count,
                    distinct_count,
                    min_value,
                    max_value,
                    top_values,
                }
            })
            .collect();
        Ok(TableSummary {
            table: table.to_string(),
            row_count,
            columns,
        })
    }

    fn ask_data(&self, question: &str) -> Result<AskAnswer, McpError> {
        // 模拟 NL2SQL + RAG：基于关键词匹配生成示例回答
        let q_lower = question.to_lowercase();
        let (answer, sql) = if q_lower.contains("商品") || q_lower.contains("product") {
            (
                "products 表共有 1000 行商品数据，价格区间 0.01 ~ 999.99 元。".to_string(),
                Some("SELECT count(*), min(price), max(price) FROM products".to_string()),
            )
        } else if q_lower.contains("订单") || q_lower.contains("order") {
            (
                "orders 表共有 5000 行订单数据。".to_string(),
                Some("SELECT count(*) FROM orders".to_string()),
            )
        } else if q_lower.contains("慢查询") || q_lower.contains("slow") {
            (
                "检测到 1 条慢查询：SELECT * FROM orders WHERE total > 100，耗时 350ms。"
                    .to_string(),
                None,
            )
        } else {
            (
                format!("问题「{question}」暂无法直接回答，建议使用 execute_sql 工具查询。"),
                None,
            )
        };
        Ok(AskAnswer {
            answer,
            sql,
            citations: vec![AskCitation {
                table: "products".to_string(),
                row_id: 1,
                snippet: "id=1, name=苹果汁, price=3.50".to_string(),
                score: 0.92,
            }],
        })
    }

    fn explain_root_cause(&self, alert_id: &str) -> Result<RootCauseReport, McpError> {
        // 根据 alert_id（用 rule_id 匹配）找到对应告警
        let alert = self
            .alerts
            .iter()
            .find(|a| a.rule_id == alert_id)
            .ok_or_else(|| McpError::BackendError(format!("alert not found: {alert_id}")))?
            .clone();

        // 根因推理规则：基于 rule_id + 关联 slow_queries + wait_events
        let (causes, evidence) = match alert.rule_id.as_str() {
            "high_qps" => {
                let mut causes = vec![CauseEntry {
                    cause_type: CauseType::HighQps,
                    description: "QPS 超过阈值，可能由突发流量或缺失索引导致全表扫描放大"
                        .to_string(),
                    confidence: 0.7,
                }];
                let mut evidence = vec![Evidence {
                    source: "alert".to_string(),
                    detail: format!("QPS={:.0}, threshold={:.0}", alert.value, alert.threshold),
                }];
                // 关联慢查询作为证据
                if let Some(sq) = self.slow_query_log.first() {
                    if sq.plan_operator == "Seq Scan" {
                        causes.push(CauseEntry {
                            cause_type: CauseType::MissingIndex,
                            description: format!(
                                "慢查询使用 Seq Scan，扫描 {} 行，建议添加索引",
                                sq.rows_scanned
                            ),
                            confidence: 0.8,
                        });
                    }
                    evidence.push(Evidence {
                        source: "slow_query".to_string(),
                        detail: format!("SQL={}, elapsed={}ms", sq.sql, sq.elapsed_ms),
                    });
                }
                (causes, evidence)
            }
            "full_table_scan" => {
                let causes = vec![CauseEntry {
                    cause_type: CauseType::MissingIndex,
                    description: "检测到全表扫描，建议为 WHERE 条件列添加索引".to_string(),
                    confidence: 0.85,
                }];
                let evidence = self
                    .slow_query_log
                    .iter()
                    .filter(|sq| sq.plan_operator == "Seq Scan")
                    .map(|sq| Evidence {
                        source: "slow_query".to_string(),
                        detail: format!("SQL={}, rows_scanned={}", sq.sql, sq.rows_scanned),
                    })
                    .collect();
                (causes, evidence)
            }
            "deadlock" => {
                let causes = vec![CauseEntry {
                    cause_type: CauseType::Deadlock,
                    description: "检测到死锁，建议检查事务锁顺序".to_string(),
                    confidence: 0.9,
                }];
                let evidence = self
                    .deadlocks
                    .iter()
                    .map(|dl| Evidence {
                        source: "deadlock_history".to_string(),
                        detail: format!("txn_ids={:?} resource={}", dl.txn_ids, dl.resource),
                    })
                    .collect();
                (causes, evidence)
            }
            "timeout" => {
                let mut causes = vec![];
                let mut evidence = vec![];
                let lock_wait = self.wait_events.iter().any(|w| w.event.contains("lock"));
                if lock_wait {
                    causes.push(CauseEntry {
                        cause_type: CauseType::LockContention,
                        description: "等待事件中锁等待占比高，存在锁竞争".to_string(),
                        confidence: 0.6,
                    });
                    evidence.push(Evidence {
                        source: "wait_events".to_string(),
                        detail: "lock wait detected".to_string(),
                    });
                }
                if let Some(sq) = self.slow_query_log.first() {
                    if sq.rows_scanned > 10000 {
                        causes.push(CauseEntry {
                            cause_type: CauseType::MissingIndex,
                            description: format!("慢查询扫描 {} 行，可能导致超时", sq.rows_scanned),
                            confidence: 0.7,
                        });
                    }
                }
                if causes.is_empty() {
                    causes.push(CauseEntry {
                        cause_type: CauseType::StatsStale,
                        description: "统计信息可能过期，建议执行 ANALYZE".to_string(),
                        confidence: 0.5,
                    });
                }
                (causes, evidence)
            }
            _ => {
                let causes = vec![CauseEntry {
                    cause_type: CauseType::StatsStale,
                    description: "未知告警类型，建议执行 ANALYZE 更新统计信息".to_string(),
                    confidence: 0.3,
                }];
                let evidence = vec![Evidence {
                    source: "alert".to_string(),
                    detail: format!("rule_id={}, message={}", alert.rule_id, alert.message),
                }];
                (causes, evidence)
            }
        };

        Ok(RootCauseReport {
            alert,
            likely_causes: causes,
            evidence,
        })
    }

    /// P5: 数据血缘查询 — 基于内置 Mock 血缘数据
    ///
    /// Mock 血缘拓扑（与 MockBackendV2 的示例表对齐）：
    /// - products.price → orders.total_price (CTAS, SUM)
    /// - products.id → orders.product_id (CTAS, direct)
    /// - products.name → order_items.product_name (View, direct)
    fn get_lineage(&self, table: Option<&str>) -> Result<LineageInfo, McpError> {
        let all_edges = mock_lineage_edges();
        let total_edges = all_edges.len();

        // 涉及的所有表（去重 + 排序）
        let mut tables_set = std::collections::HashSet::new();
        for e in &all_edges {
            tables_set.insert(e.source.table.clone());
            tables_set.insert(e.target.table.clone());
        }
        let mut tables: Vec<String> = tables_set.into_iter().collect();
        tables.sort();

        match table {
            None => Ok(LineageInfo {
                table: None,
                upstream: all_edges.clone(),
                downstream: vec![],
                tables,
                total_edges,
            }),
            Some(t) => {
                let upstream: Vec<LineageEdgeDto> = all_edges
                    .iter()
                    .filter(|e| e.target.table == t)
                    .cloned()
                    .collect();
                let downstream: Vec<LineageEdgeDto> = all_edges
                    .iter()
                    .filter(|e| e.source.table == t)
                    .cloned()
                    .collect();
                Ok(LineageInfo {
                    table: Some(t.to_string()),
                    upstream,
                    downstream,
                    tables,
                    total_edges,
                })
            }
        }
    }
}

/// 构造 Mock 血缘边（与 MockBackendV2 示例表对齐）
fn mock_lineage_edges() -> Vec<LineageEdgeDto> {
    vec![
        LineageEdgeDto {
            source: ColumnRefDto {
                table: "products".to_string(),
                column: "price".to_string(),
            },
            target: ColumnRefDto {
                table: "orders".to_string(),
                column: "total_price".to_string(),
            },
            transform: "SUM(price)".to_string(),
            source_type: LineageEdgeSource::Ctas,
        },
        LineageEdgeDto {
            source: ColumnRefDto {
                table: "products".to_string(),
                column: "id".to_string(),
            },
            target: ColumnRefDto {
                table: "orders".to_string(),
                column: "product_id".to_string(),
            },
            transform: "direct".to_string(),
            source_type: LineageEdgeSource::Ctas,
        },
        LineageEdgeDto {
            source: ColumnRefDto {
                table: "products".to_string(),
                column: "name".to_string(),
            },
            target: ColumnRefDto {
                table: "order_items".to_string(),
                column: "product_name".to_string(),
            },
            transform: "direct".to_string(),
            source_type: LineageEdgeSource::View,
        },
    ]
}

// =====================================================================
//  CatalogBackend — 连接真实 catalog 的 MCP 后端（Phase TDengine-P3-MVP）
// =====================================================================

/// 连接真实 catalog 的 MCP 后端 — Phase TDengine-P3-MVP
///
/// # 设计
///
/// - 持有 `Box<dyn MutableCatalog>`，仅调用 `&self` 只读方法
///   （`list_tables` / `get_table` / `list_indexes_for_table` / `get_column_comment`）
/// - 4 个 Schema 类工具返回**真实元数据**：`list_tables` / `describe_table` / `list_indexes` / `list_views`
/// - 其余 26 个方法返回空 `Vec` 或 `Err`（等待后续接入执行器/ops/lineage）
///
/// # 用途
///
/// 让 LLM 通过 MCP 协议看到真实的表结构、列注释、索引信息，
/// 覆盖"看库看表"这一最高频场景。修复差距 1（MCP 全 Mock）的子集。
///
/// # 兼容性
///
/// `MockBackendV2` 保持不动，`CatalogBackend` 作为独立实现。
/// 通过 `McpServerV2::new_with_catalog` 便捷注入。
pub struct CatalogBackend {
    catalog: Box<dyn szrsql_catalog::MutableCatalog>,
    /// 可选的执行器后端 — 注入后启用 Query/Runtime/Maintenance/Insight 等 26 个方法
    ///
    /// 通过 `with_executor` 构造器注入。当为 `None` 时，这 26 个方法返回
    /// "no executor attached" 错误或空数据（与原 MVP limit 行为一致）。
    executor: Option<ExecutorBackend>,
    /// 层次化数据目录树（P5）— 在扁平 catalog 之上叠加路径组织层
    ///
    /// 默认为空树（仅根节点）。可通过 `catalog_tree()` 访问进行挂载/卸载/移动操作。
    /// 不影响现有 30 个 MCP 工具行为，仅作为数据资产组织能力提供。
    catalog_tree: szrsql_catalog::catalog_tree::CatalogTree,
    /// 可选的复制任务管理器 — 注入后启用 5 个 Replication 类 MCP 工具
    ///
    /// 通过 `with_replication` 构造器注入。当为 `None` 时，Replication 类方法
    /// 返回 "no replication manager attached" 错误（继承 trait 默认实现）。
    /// 持有 `Arc<ReplicationTaskManager>` 以便多个 MCP 会话共享同一管理器。
    replication: Option<std::sync::Arc<szrsql_cdc::task::ReplicationTaskManager>>,
}

impl CatalogBackend {
    /// 创建 CatalogBackend，接管 catalog 所有权
    ///
    /// catalog 实现了 `MutableCatalog` trait（如 `ManagedCatalog`）。
    /// CatalogBackend 仅调用只读方法，不会修改 catalog。
    ///
    /// 此构造器仅启用 4 个 Schema 方法（list_tables/describe_table/list_indexes/list_views）。
    /// 其余 26 个方法返回 "no executor attached" — 使用 `with_executor` 启用完整功能。
    pub fn new(catalog: Box<dyn szrsql_catalog::MutableCatalog>) -> Self {
        Self {
            catalog,
            executor: None,
            catalog_tree: szrsql_catalog::catalog_tree::CatalogTree::new(),
            replication: None,
        }
    }

    /// 创建带执行器的 CatalogBackend — 启用全部 30 个 MCP 方法
    ///
    /// `executor` 提供数据存储 + SQL 执行 + 运行时统计 + 血缘追踪。
    /// 调用方需保证 `catalog` 与 `executor` 的表 schema 一致
    /// （通常先构建 executor，再从其 catalog 构建外部 catalog 引用）。
    pub fn with_executor(
        catalog: Box<dyn szrsql_catalog::MutableCatalog>,
        executor: ExecutorBackend,
    ) -> Self {
        Self {
            catalog,
            executor: Some(executor),
            catalog_tree: szrsql_catalog::catalog_tree::CatalogTree::new(),
            replication: None,
        }
    }

    /// 注入复制任务管理器 — 启用 5 个 Replication 类 MCP 工具
    ///
    /// `replication` 提供源端→目标端的 CDC 复制任务生命周期管理。
    /// 调用方需先构建 `CdcEngine` + `SlotManager` + `RowDecoder` +
    /// `SchemaRegistry`，再用它们构造 `ReplicationTaskManager`。
    ///
    /// 典型用法：
    /// ```ignore
    /// let cdc_engine = Arc::new(CdcEngine::new(...));
    /// let slot_mgr = Arc::new(SlotManager::in_memory());
    /// let schema_reg = Arc::new(SchemaRegistry::new());
    /// let decoder = Arc::new(RowDecoder::new(schema_reg.clone()));
    /// let task_mgr = Arc::new(ReplicationTaskManager::new(
    ///     slot_mgr, decoder, schema_reg, cdc_engine,
    /// ));
    /// let backend = CatalogBackend::new(catalog).with_replication(task_mgr);
    /// ```
    pub fn with_replication(
        mut self,
        replication: std::sync::Arc<szrsql_cdc::task::ReplicationTaskManager>,
    ) -> Self {
        self.replication = Some(replication);
        self
    }

    /// 访问层次化数据目录树（P5）
    ///
    /// 提供树状数据资产组织能力：
    /// - `create_dir(path)` — 创建目录节点
    /// - `mount_table(path, table_name)` — 在路径挂载表
    /// - `mount_view(path, view_name)` — 在路径挂载视图
    /// - `unmount(path)` — 卸载节点
    /// - `list_children(path)` — 列出子节点
    /// - `move_node(src, dst_parent)` — 移动节点
    /// - `tree_view()` — 整树 BFS 视图
    /// - `find_path_by_table_name(name)` — 反向查找表路径
    pub fn catalog_tree(&self) -> &szrsql_catalog::catalog_tree::CatalogTree {
        &self.catalog_tree
    }

    /// 可变访问层次化数据目录树（P5）
    pub fn catalog_tree_mut(&mut self) -> &mut szrsql_catalog::catalog_tree::CatalogTree {
        &mut self.catalog_tree
    }

    /// 从 `list_tables()` 结果中查找简单表名匹配的 `TableName`
    ///
    /// MVP 阶段避免直接构造 `TableName`（来自 `szrsql_sql::ast`，szrsql-ai 生产代码不直接依赖）。
    /// 通过 `list_tables()` 返回的 `Vec<TableName>` 线性查找，O(n) 复杂度对 MVP 够用。
    fn find_table_name(&self, table: &str) -> Option<szrsql_sql::ast::TableName> {
        self.catalog
            .list_tables()
            .into_iter()
            .find(|n| n.name == table)
    }
}

impl McpBackendV2 for CatalogBackend {
    // --- 类别 1: Schema（4 个真实方法） ---

    /// 列出所有表 — 返回真实表清单
    ///
    /// `row_count` 和 `size_bytes` 暂返回 0（MVP 未连接 storage 层）。
    fn list_tables(&self) -> Result<Vec<crate::mcp::TableInfo>, McpError> {
        let tables = self
            .catalog
            .list_tables()
            .into_iter()
            .map(|name| crate::mcp::TableInfo {
                name: name.name.clone(),
                row_count: 0,  // MVP 未连接 storage
                size_bytes: 0, // MVP 未连接 storage
            })
            .collect();
        Ok(tables)
    }

    /// 描述表结构 — 返回真实 schema + 真实列注释
    ///
    /// 注释优先级（复用 `information_schema::columns_with_catalog` 模式）：
    /// 1. `catalog.get_column_comment()`（COMMENT ON COLUMN 设置的）
    /// 2. `ColumnDefinition.comment`（CREATE TABLE 时内联指定的）
    fn describe_table(&self, table: &str) -> Result<crate::mcp::TableSchema, McpError> {
        let name = self
            .find_table_name(table)
            .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))?;
        let schema = self
            .catalog
            .get_table(&name)
            .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))?;

        let columns = schema
            .columns
            .iter()
            .map(|col| {
                let comment = self
                    .catalog
                    .get_column_comment(&name, &col.name)
                    .or_else(|| col.comment.clone());
                crate::mcp::ColumnDef {
                    name: col.name.clone(),
                    data_type: column_type_to_string(&col.data_type),
                    nullable: !(col.not_null || col.primary_key),
                    primary_key: col.primary_key,
                    comment,
                }
            })
            .collect();

        Ok(crate::mcp::TableSchema {
            table: name.name.clone(),
            columns,
        })
    }

    /// 列出表的索引 — 返回真实索引元数据
    ///
    /// `is_primary` 通过索引名是否以 `_pkey` 结尾判断（与 PG 命名约定一致）。
    fn list_indexes(&self, table: &str) -> Result<Vec<IndexInfo>, McpError> {
        let name = self
            .find_table_name(table)
            .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))?;
        let indexes = self.catalog.list_indexes_for_table(&name);
        let result = indexes
            .into_iter()
            .map(|idx| {
                // 先借用 idx.column_names()，再 move idx.name/idx.table，避免 partial move
                let columns: Vec<String> =
                    idx.column_names().into_iter().map(String::from).collect();
                let is_primary = idx.name.ends_with("_pkey");
                IndexInfo {
                    is_primary,
                    name: idx.name,
                    table: idx.table.name,
                    columns,
                    unique: idx.unique,
                }
            })
            .collect();
        Ok(result)
    }

    /// 列出所有视图 — SzRSQL 不支持 VIEW，返回空 Vec（语义正确）
    fn list_views(&self) -> Result<Vec<ViewInfo>, McpError> {
        Ok(vec![])
    }

    // --- 类别 2: Query — 委托到 executor（未注入时返回 Err） ---

    fn execute_sql(&self, sql: &str) -> Result<crate::mcp::QueryResult, McpError> {
        match &self.executor {
            Some(exec) => exec.execute_sql(sql),
            None => Err(McpError::BackendError(
                "CatalogBackend has no executor attached (use with_executor to enable execute_sql)"
                    .to_string(),
            )),
        }
    }
    fn explain_query(&self, sql: &str) -> Result<ExplainPlan, McpError> {
        match &self.executor {
            Some(exec) => exec.explain_query(sql),
            None => Err(McpError::BackendError(
                "CatalogBackend has no executor attached (use with_executor to enable explain_query)"
                    .to_string(),
            )),
        }
    }
    fn prepare_statement(&self, name: &str, sql: &str) -> Result<PrepareResult, McpError> {
        match &self.executor {
            Some(exec) => exec.prepare_statement(name, sql),
            None => Err(McpError::BackendError(
                "CatalogBackend has no executor attached (use with_executor to enable prepare_statement)"
                    .to_string(),
            )),
        }
    }
    fn cancel_query(&self, query_id: u64) -> Result<CancelResult, McpError> {
        match &self.executor {
            Some(exec) => exec.cancel_query(query_id),
            None => Ok(CancelResult {
                query_id,
                cancelled: false,
            }),
        }
    }

    // --- 类别 3: SlowQuery — 委托到 executor（未注入时返回空） ---

    fn slow_queries(&self, limit: usize) -> Result<Vec<SlowQueryRecord>, McpError> {
        match &self.executor {
            Some(exec) => exec.slow_queries(limit),
            None => Ok(vec![]),
        }
    }
    fn top_queries(&self, limit: usize) -> Result<Vec<TopQueryRecord>, McpError> {
        match &self.executor {
            Some(exec) => exec.top_queries(limit),
            None => Ok(vec![]),
        }
    }
    fn query_stats(&self) -> Result<QueryStatsSummary, McpError> {
        match &self.executor {
            Some(exec) => exec.query_stats(),
            None => Ok(QueryStatsSummary {
                total_queries: 0,
                total_time_ms: 0.0,
                unique_queries: 0,
                avg_time_ms: 0.0,
            }),
        }
    }
    fn reset_stats(&self) -> Result<ResetResult, McpError> {
        match &self.executor {
            Some(exec) => exec.reset_stats(),
            None => Ok(ResetResult { reset: false }),
        }
    }

    // --- 类别 4: TxLock — 委托到 executor（未注入时返回空） ---

    fn list_transactions(&self) -> Result<Vec<TransactionInfo>, McpError> {
        match &self.executor {
            Some(exec) => exec.list_transactions(),
            None => Ok(vec![]),
        }
    }
    fn list_locks(&self) -> Result<Vec<LockInfo>, McpError> {
        match &self.executor {
            Some(exec) => exec.list_locks(),
            None => Ok(vec![]),
        }
    }
    fn kill_transaction(&self, txn_id: u32) -> Result<KillResult, McpError> {
        match &self.executor {
            Some(exec) => exec.kill_transaction(txn_id),
            None => Ok(KillResult {
                txn_id,
                killed: false,
            }),
        }
    }
    fn deadlock_history(&self) -> Result<Vec<DeadlockRecord>, McpError> {
        match &self.executor {
            Some(exec) => exec.deadlock_history(),
            None => Ok(vec![]),
        }
    }

    // --- 类别 5: Perf — 委托到 executor（未注入时返回空） ---

    fn wait_events(&self) -> Result<Vec<WaitEventSummary>, McpError> {
        match &self.executor {
            Some(exec) => exec.wait_events(),
            None => Ok(vec![]),
        }
    }
    fn ash_report(&self, duration_secs: u64) -> Result<AshReport, McpError> {
        match &self.executor {
            Some(exec) => exec.ash_report(duration_secs),
            None => Ok(AshReport {
                duration_secs,
                sample_count: 0,
                top_sql: vec![],
                top_wait_events: vec![],
            }),
        }
    }
    fn active_sessions(&self) -> Result<Vec<SessionInfo>, McpError> {
        match &self.executor {
            Some(exec) => exec.active_sessions(),
            None => Ok(vec![]),
        }
    }
    fn pprof_dump(&self, duration_secs: u64) -> Result<PprofResult, McpError> {
        match &self.executor {
            Some(exec) => exec.pprof_dump(duration_secs),
            None => Ok(PprofResult {
                sample_count: 0,
                duration_secs,
                top_functions: vec![],
            }),
        }
    }

    // --- 类别 6: Maintenance — 委托到 executor（未注入时返回 Err/默认） ---

    fn vacuum_table(&self, table: &str) -> Result<VacuumResult, McpError> {
        match &self.executor {
            Some(exec) => exec.vacuum_table(table),
            None => Err(McpError::BackendError(
                "CatalogBackend has no executor attached (use with_executor to enable vacuum_table)"
                    .to_string(),
            )),
        }
    }
    fn analyze_table(&self, table: &str) -> Result<AnalyzeResult, McpError> {
        match &self.executor {
            Some(exec) => exec.analyze_table(table),
            None => Err(McpError::BackendError(
                "CatalogBackend has no executor attached (use with_executor to enable analyze_table)"
                    .to_string(),
            )),
        }
    }
    fn autovacuum_status(&self) -> Result<AutovacuumStatus, McpError> {
        match &self.executor {
            Some(exec) => exec.autovacuum_status(),
            None => Ok(AutovacuumStatus {
                enabled: false,
                last_run: 0,
                tables_vacuumed: 0,
                tables_analyzed: 0,
            }),
        }
    }

    // --- 类别 7: Alerting — 委托到 executor（未注入时返回空/默认） ---

    fn list_alerts(&self) -> Result<Vec<AlertInfo>, McpError> {
        match &self.executor {
            Some(exec) => exec.list_alerts(),
            None => Ok(vec![]),
        }
    }
    fn db_stats(&self) -> Result<crate::mcp::DbStats, McpError> {
        // table_count 始终从 catalog 获取（真实），其余字段委托到 executor
        let table_count = self.catalog.list_tables().len();
        match &self.executor {
            Some(exec) => {
                let mut stats = exec.db_stats()?;
                stats.table_count = table_count;
                Ok(stats)
            }
            None => Ok(crate::mcp::DbStats {
                table_count,
                total_rows: 0,
                total_size_bytes: 0,
                cache_hit_rate: 0.0,
                active_connections: 0,
            }),
        }
    }
    fn capacity_predict(&self, days: u32) -> Result<CapacityForecast, McpError> {
        match &self.executor {
            Some(exec) => exec.capacity_predict(days),
            None => Ok(CapacityForecast {
                metric: "none".to_string(),
                current_value: 0.0,
                predicted_value: 0.0,
                days_ahead: days,
                confidence: 0.0,
                storage_bytes_current: None,
                storage_bytes_predicted: None,
                net_growth_rate_per_day: None,
                table_breakdown: None,
            }),
        }
    }

    // --- 类别 8: Insight — 委托到 executor（未注入时返回 Err/空） ---

    fn summarize_table(&self, table: &str) -> Result<TableSummary, McpError> {
        match &self.executor {
            Some(exec) => exec.summarize_table(table),
            None => Err(McpError::BackendError(
                "CatalogBackend has no executor attached (use with_executor to enable summarize_table)"
                    .to_string(),
            )),
        }
    }
    fn ask_data(&self, question: &str) -> Result<AskAnswer, McpError> {
        match &self.executor {
            Some(exec) => exec.ask_data(question),
            None => Err(McpError::BackendError(
                "CatalogBackend has no executor attached (use with_executor to enable ask_data)"
                    .to_string(),
            )),
        }
    }
    fn explain_root_cause(&self, alert_id: &str) -> Result<RootCauseReport, McpError> {
        match &self.executor {
            Some(exec) => exec.explain_root_cause(alert_id),
            None => Err(McpError::BackendError(
                "CatalogBackend has no executor attached (use with_executor to enable explain_root_cause)"
                    .to_string(),
            )),
        }
    }
    fn get_lineage(&self, table: Option<&str>) -> Result<LineageInfo, McpError> {
        match &self.executor {
            Some(exec) => exec.get_lineage(table),
            None => Ok(LineageInfo {
                table: None,
                upstream: vec![],
                downstream: vec![],
                tables: vec![],
                total_edges: 0,
            }),
        }
    }

    // --- 类别 9: Replication — 委托到 replication manager（未注入时返回 Err） ---

    /// 创建复制任务 — 构造 TargetWriter + TaskConfig，调用管理器创建并启动
    ///
    /// 根据 `target_type` 分派 writer 工厂：
    /// - `memory` / `postgres` / `mysql`：使用 `target::create_writer`
    /// - `kafka`：使用 `MockKafkaProducer`（MVP；生产环境应注入真实 producer）
    ///
    /// 任务创建后立即调用 `start_task` 进入 Running 状态，并注册为 CdcEngine observer。
    fn create_replication_task(
        &self,
        params: CreateReplicationTaskParams,
    ) -> Result<CreateReplicationTaskResult, McpError> {
        let mgr = self.replication.as_ref().ok_or_else(|| {
            McpError::BackendError(
                "CatalogBackend has no replication manager attached (use with_replication to enable create_replication_task)"
                    .to_string(),
            )
        })?;

        // 1. 构造 TargetWriter
        let writer: std::sync::Arc<dyn szrsql_cdc::target::TargetWriter> =
            match params.target_type.as_str() {
                "memory" => {
                    let cfg = szrsql_cdc::target::TargetConfig::memory();
                    szrsql_cdc::target::create_writer(&cfg).map_err(|e| {
                        McpError::BackendError(format!("create memory writer failed: {e}"))
                    })?
                }
                "postgres" => {
                    let cfg = szrsql_cdc::target::TargetConfig::postgres(&params.target_connection);
                    szrsql_cdc::target::create_writer(&cfg).map_err(|e| {
                        McpError::BackendError(format!("create postgres writer failed: {e}"))
                    })?
                }
                "mysql" => {
                    let cfg = szrsql_cdc::target::TargetConfig::mysql(&params.target_connection);
                    szrsql_cdc::target::create_writer(&cfg).map_err(|e| {
                        McpError::BackendError(format!("create mysql writer failed: {e}"))
                    })?
                }
                "kafka" => {
                    // MVP：使用 MockKafkaProducer（将消息记录到内存）
                    // 生产环境应在注入 ReplicationTaskManager 前预构造 KafkaSink
                    // 并通过自定义 TaskConfig 直接调用 create_task
                    let producer =
                        std::sync::Arc::new(szrsql_cdc::target::kafka::MockKafkaProducer::new());
                    // target_connection 格式："brokers|topic"
                    let (brokers, topic) = params
                        .target_connection
                        .split_once('|')
                        .unwrap_or(("localhost:9092", "cdc-events"));
                    let kafka_cfg = szrsql_cdc::target::kafka::KafkaConfig::new(topic, brokers);
                    std::sync::Arc::new(szrsql_cdc::target::kafka::KafkaSink::new(
                        kafka_cfg, producer,
                    ))
                }
                other => {
                    return Err(McpError::BackendError(format!(
                        "unsupported target_type: {other} (supported: memory/postgres/mysql/kafka)"
                    )));
                }
            };

        // 2. 构造 TaskConfig
        let table_filter = params.table_filter.as_ref().map(|v| {
            v.iter()
                .cloned()
                .collect::<std::collections::HashSet<String>>()
        });

        // 根据目标端类型推断方言（P4-2）
        let dialect = match params.target_type.as_str() {
            "postgres" => szrsql_cdc::migration::Dialect::Postgres,
            "mysql" => szrsql_cdc::migration::Dialect::MySQL,
            "oracle" => szrsql_cdc::migration::Dialect::Oracle,
            "sqlserver" | "mssql" => szrsql_cdc::migration::Dialect::SqlServer,
            _ => szrsql_cdc::migration::Dialect::Postgres, // 默认 Postgres
        };

        let config = szrsql_cdc::task::TaskConfig {
            task_id: params.task_id.clone(),
            description: params.description.clone(),
            table_filter,
            writer,
            target_type: params.target_type.clone(),
            target_connection: params.target_connection.clone(),
            snapshot_first: params.snapshot_first,
            dialect,
            backpressure_config: szrsql_cdc::backpressure::BackpressureConfig::default(),
        };

        // 3. 创建并启动任务
        let task = mgr
            .create_task(config)
            .map_err(|e| McpError::BackendError(format!("create_task failed: {e}")))?;
        mgr.start_task(&params.task_id)
            .map_err(|e| McpError::BackendError(format!("start_task failed: {e}")))?;
        let final_state = task.state();

        Ok(CreateReplicationTaskResult {
            task_id: params.task_id,
            state: final_state.as_str().to_string(),
            created: true,
        })
    }

    /// 列出所有复制任务 — 委托到管理器，映射 TaskInfo → ReplicationTaskInfo
    fn list_replication_tasks(&self) -> Result<Vec<ReplicationTaskInfo>, McpError> {
        let mgr = self.replication.as_ref().ok_or_else(|| {
            McpError::BackendError(
                "CatalogBackend has no replication manager attached (use with_replication to enable list_replication_tasks)"
                    .to_string(),
            )
        })?;
        Ok(mgr.list_tasks().into_iter().map(task_info_to_dto).collect())
    }

    /// 监控指定复制任务 — 返回详细状态和统计
    fn monitor_replication_task(&self, task_id: &str) -> Result<ReplicationTaskInfo, McpError> {
        let mgr = self.replication.as_ref().ok_or_else(|| {
            McpError::BackendError(format!(
                "CatalogBackend has no replication manager attached (use with_replication to enable monitor_replication_task, task_id={})",
                task_id
            ))
        })?;
        let info = mgr
            .monitor_task(task_id)
            .map_err(|e| McpError::BackendError(format!("monitor_task failed: {e}")))?;
        Ok(task_info_to_dto(info))
    }

    /// 停止复制任务 — 注销 observer 并转入 Stopped 终态
    fn stop_replication_task(&self, task_id: &str) -> Result<StopReplicationTaskResult, McpError> {
        let mgr = self.replication.as_ref().ok_or_else(|| {
            McpError::BackendError(format!(
                "CatalogBackend has no replication manager attached (use with_replication to enable stop_replication_task, task_id={})",
                task_id
            ))
        })?;
        mgr.stop_task(task_id)
            .map_err(|e| McpError::BackendError(format!("stop_task failed: {e}")))?;
        // 获取停止后状态
        let info = mgr
            .monitor_task(task_id)
            .map_err(|e| McpError::BackendError(format!("post-stop monitor failed: {e}")))?;
        Ok(StopReplicationTaskResult {
            task_id: task_id.to_string(),
            state: info.state.as_str().to_string(),
            stopped: info.state == szrsql_cdc::task::TaskState::Stopped,
        })
    }

    /// 复制管理器统计 — 返回管理器级别的聚合统计
    fn replication_manager_stats(&self) -> Result<ReplicationManagerStats, McpError> {
        let mgr = self.replication.as_ref().ok_or_else(|| {
            McpError::BackendError(
                "CatalogBackend has no replication manager attached (use with_replication to enable replication_manager_stats)"
                    .to_string(),
            )
        })?;
        let stats = mgr.manager_stats();
        Ok(ReplicationManagerStats {
            total_tasks: stats.total_tasks,
            total_created: stats.total_created,
            total_started: stats.total_started,
            total_stopped: stats.total_stopped,
            total_failed: stats.total_failed,
            running_tasks: stats.running_tasks,
        })
    }
}

/// 将 `szrsql_cdc::task::TaskInfo` 转换为 MCP 协议 DTO `ReplicationTaskInfo`
///
/// 该转换层隔离了 CDC 内部数据结构与 MCP 协议暴露的 DTO，便于两者独立演进。
fn task_info_to_dto(info: szrsql_cdc::task::TaskInfo) -> ReplicationTaskInfo {
    // 先计算 lag（借用 stats），再 move 字段，避免 partial move 借用冲突
    let lag = info.stats.lag();
    let stats = info.stats;
    ReplicationTaskInfo {
        task_id: info.task_id,
        description: info.description,
        state: info.state.as_str().to_string(),
        target_type: info.target_type,
        target_connection: info.target_connection,
        created_at: info.created_at,
        table_filter: info.table_filter,
        events_received: stats.events_received,
        events_written: stats.events_written,
        bytes_processed: stats.bytes_processed,
        transactions_processed: stats.transactions_processed,
        error_count: stats.error_count,
        last_error: stats.last_error,
        last_write_at: stats.last_write_at,
        last_lsn: stats.last_lsn,
        confirmed_flush_lsn: stats.confirmed_flush_lsn,
        lag,
        snapshot_lsn: info.snapshot_lsn,
    }
}

/// 截断 SQL 文本到指定长度（用于告警消息）
fn truncate_sql(sql: &str, max_len: usize) -> String {
    if sql.len() <= max_len {
        sql.to_string()
    } else {
        format!("{}...", &sql[..max_len])
    }
}

/// 将表名映射为稳定的 resource_id (u64)（P3-Deadlock-Detection）
///
/// 使用 `DefaultHasher` 对表名（小写）进行哈希，确保同一表名在多次调用中
/// 产生相同的 resource_id，从而让 LockManager 正确识别同一资源的锁冲突。
/// 不同表名产生不同的 resource_id（哈希碰撞概率极低，可接受）。
fn table_resource_id(table: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    table.to_lowercase().hash(&mut hasher);
    hasher.finish()
}

/// 从 SELECT 语句中提取所有源表名（递归遍历 FROM/JOIN/子查询/集合操作）
fn extract_source_tables(select: &szrsql_sql::ast::Select) -> Vec<String> {
    let mut tables = Vec::new();
    collect_tables_from_select(select, &mut tables);
    // 去重
    tables.sort();
    tables.dedup();
    tables
}

/// 递归收集 SELECT 中的源表
fn collect_tables_from_select(select: &szrsql_sql::ast::Select, out: &mut Vec<String>) {
    use szrsql_sql::ast::{JoinCondition, SelectItem};

    // WITH 子句（CTE）— 递归收集 CTE 查询中的表
    if let Some(with) = &select.with {
        for cte in &with.ctes {
            collect_tables_from_select(&cte.query, out);
        }
    }

    // FROM 子句
    for tj in &select.from {
        collect_tables_from_table_factor(&tj.relation, out);
        for join in &tj.joins {
            collect_tables_from_table_factor(&join.relation, out);
            if let JoinCondition::Using(_cols) = &join.condition {
                // USING 列不产生新表；ON 条件中的子查询可忽略（简化）
            }
        }
    }

    // projection 中的子查询
    for item in &select.projection {
        if let SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } = item {
            collect_tables_from_expr(e, out);
        }
    }

    // WHERE 子查询
    if let Some(w) = &select.where_clause {
        collect_tables_from_expr(w, out);
    }

    // 集合操作的右侧
    if let Some(set_op) = &select.set_op {
        collect_tables_from_select(&set_op.right, out);
    }
}

/// 从 TableFactor 收集表名
fn collect_tables_from_table_factor(tf: &szrsql_sql::ast::TableFactor, out: &mut Vec<String>) {
    use szrsql_sql::ast::TableFactor;
    match tf {
        TableFactor::Table { name, .. } => {
            out.push(name.name.clone());
        }
        TableFactor::Derived { subquery, .. } => {
            collect_tables_from_select(subquery, out);
        }
        TableFactor::TableFunction { .. } => {}
    }
}

/// 从 Expr 收集子查询中的表
fn collect_tables_from_expr(expr: &szrsql_sql::ast::Expr, out: &mut Vec<String>) {
    use szrsql_sql::ast::Expr;
    match expr {
        Expr::Subquery(sel) | Expr::Exists { subquery: sel, .. } => {
            collect_tables_from_select(sel, out);
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_tables_from_expr(left, out);
            collect_tables_from_expr(right, out);
        }
        Expr::UnaryOp { expr: e, .. } => collect_tables_from_expr(e, out),
        Expr::Function { args, .. } | Expr::WindowFunction { args, .. } => {
            for a in args {
                collect_tables_from_expr(a, out);
            }
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            if let Some(o) = operand {
                collect_tables_from_expr(o, out);
            }
            for (w, t) in when_then {
                collect_tables_from_expr(w, out);
                collect_tables_from_expr(t, out);
            }
            if let Some(e) = else_expr {
                collect_tables_from_expr(e, out);
            }
        }
        Expr::InList { expr: e, list, .. } => {
            collect_tables_from_expr(e, out);
            for item in list {
                collect_tables_from_expr(item, out);
            }
        }
        Expr::InSubquery {
            expr: e, subquery, ..
        } => {
            collect_tables_from_expr(e, out);
            collect_tables_from_select(subquery, out);
        }
        Expr::Between {
            expr: e, low, high, ..
        } => {
            collect_tables_from_expr(e, out);
            collect_tables_from_expr(low, out);
            collect_tables_from_expr(high, out);
        }
        Expr::Like {
            expr: e, pattern, ..
        }
        | Expr::SimilarTo {
            expr: e, pattern, ..
        } => {
            collect_tables_from_expr(e, out);
            collect_tables_from_expr(pattern, out);
        }
        Expr::IsNull { expr: e, .. } => collect_tables_from_expr(e, out),
        Expr::IsDistinctFrom { left, right, .. } => {
            collect_tables_from_expr(left, out);
            collect_tables_from_expr(right, out);
        }
        Expr::Cast { expr: e, .. } => collect_tables_from_expr(e, out),
        Expr::Substring {
            expr: e,
            from,
            for_len,
        } => {
            collect_tables_from_expr(e, out);
            if let Some(f) = from {
                collect_tables_from_expr(f, out);
            }
            if let Some(fl) = for_len {
                collect_tables_from_expr(fl, out);
            }
        }
        Expr::Tuple(exprs) => {
            for e in exprs {
                collect_tables_from_expr(e, out);
            }
        }
        // 叶子节点：Literal/Identifier/Parameter/Wildcard/Array
        _ => {}
    }
}

/// ColumnType → 字符串（MCP 暴露给 LLM 的类型名）
///
/// 与 `information_schema::sql_data_type` 类似但使用简短大写形式，
/// 便于 LLM 在 prompt 中理解。
fn column_type_to_string(ct: &szrsql_types::value::ColumnType) -> String {
    use szrsql_types::value::ColumnType;
    match ct {
        ColumnType::Int64 => "BIGINT".to_string(),
        ColumnType::Float64 => "DOUBLE".to_string(),
        ColumnType::Text => "TEXT".to_string(),
        ColumnType::Bool => "BOOLEAN".to_string(),
        ColumnType::Date => "DATE".to_string(),
        ColumnType::Timestamp => "TIMESTAMP".to_string(),
        ColumnType::Decimal { precision, scale } => {
            format!("DECIMAL({precision},{scale})")
        }
        ColumnType::Enum(_) => "TEXT".to_string(),
        ColumnType::Null => "NULL".to_string(),
        ColumnType::Blob => "BLOB".to_string(),
        ColumnType::Array(_) => "ARRAY".to_string(),
        ColumnType::Range(_) => "RANGE".to_string(),
        ColumnType::Json => "JSON".to_string(),
        ColumnType::TsVector => "TSVECTOR".to_string(),
        ColumnType::TsQuery => "TSQUERY".to_string(),
    }
}

/// 将 `szrsql_types::value::ColumnType` 转换为 `nl2sql::ColType`（用于 NL2SQL 引擎注册）
fn column_type_to_coltype(ct: &szrsql_types::value::ColumnType) -> crate::nl2sql::ColType {
    use szrsql_types::value::ColumnType;
    match ct {
        ColumnType::Int64 => crate::nl2sql::ColType::Integer,
        ColumnType::Float64 | ColumnType::Decimal { .. } => crate::nl2sql::ColType::Float,
        ColumnType::Text | ColumnType::Enum(_) | ColumnType::Blob => crate::nl2sql::ColType::Text,
        ColumnType::Bool => crate::nl2sql::ColType::Bool,
        ColumnType::Date => crate::nl2sql::ColType::Date,
        ColumnType::Timestamp => crate::nl2sql::ColType::Timestamp,
        _ => crate::nl2sql::ColType::Text,
    }
}

// =====================================================================
//  P3-Prepare：参数占位符计数 — 遍历 AST 收集 Expr::Parameter(idx)
//
//  支持 PG 风格 $1/$2/...（1-based 索引）和 ? 占位符（解析器转为 Parameter(1)）。
//  返回最大索引值，作为 parameter_count。
// =====================================================================

/// 遍历语句列表，返回最大参数索引（0 表示无参数占位符）
fn count_parameters(stmts: &[szrsql_sql::ast::Statement]) -> usize {
    stmts.iter().map(count_params_in_stmt).max().unwrap_or(0)
}

/// 遍历单条语句，返回其中出现的最大参数索引
fn count_params_in_stmt(stmt: &szrsql_sql::ast::Statement) -> usize {
    use szrsql_sql::ast::{InsertSource, Statement};
    match stmt {
        Statement::Select(select) => count_params_in_select(select),
        Statement::Insert { source, .. } | Statement::Replace { source, .. } => match source {
            InsertSource::Values(rows) => rows
                .iter()
                .flat_map(|row| row.iter())
                .map(count_params_in_expr)
                .max()
                .unwrap_or(0),
            InsertSource::Select(sel) => count_params_in_select(sel),
            InsertSource::DefaultValues => 0,
        },
        Statement::Update {
            assignments,
            where_clause,
            from,
            returning,
            ..
        } => {
            let mut max_idx = 0;
            for a in assignments {
                max_idx = max_idx.max(count_params_in_expr(&a.value));
            }
            if let Some(w) = where_clause {
                max_idx = max_idx.max(count_params_in_expr(w));
            }
            for tf in from {
                max_idx = max_idx.max(count_params_in_table_factor(tf));
            }
            if let Some(ret) = returning {
                for item in ret {
                    max_idx = max_idx.max(count_params_in_select_item(item));
                }
            }
            max_idx
        }
        Statement::Delete {
            where_clause,
            using,
            returning,
            ..
        } => {
            let mut max_idx = 0;
            if let Some(w) = where_clause {
                max_idx = max_idx.max(count_params_in_expr(w));
            }
            for tf in using {
                max_idx = max_idx.max(count_params_in_table_factor(tf));
            }
            if let Some(ret) = returning {
                for item in ret {
                    max_idx = max_idx.max(count_params_in_select_item(item));
                }
            }
            max_idx
        }
        // 其他语句类型（DDL 等）通常不含参数占位符
        _ => 0,
    }
}

/// 遍历 SELECT 语句，返回最大参数索引
fn count_params_in_select(select: &szrsql_sql::ast::Select) -> usize {
    use szrsql_sql::ast::JoinCondition;
    let mut max_idx = 0;

    // WITH 子句（CTE）
    if let Some(with) = &select.with {
        for cte in &with.ctes {
            max_idx = max_idx.max(count_params_in_select(&cte.query));
        }
    }

    // projection
    for item in &select.projection {
        max_idx = max_idx.max(count_params_in_select_item(item));
    }

    // FROM 子句（含 JOIN）
    for tj in &select.from {
        max_idx = max_idx.max(count_params_in_table_factor(&tj.relation));
        for join in &tj.joins {
            max_idx = max_idx.max(count_params_in_table_factor(&join.relation));
            if let JoinCondition::On(e) = &join.condition {
                max_idx = max_idx.max(count_params_in_expr(e));
            }
        }
    }

    // WHERE / GROUP BY / HAVING
    if let Some(w) = &select.where_clause {
        max_idx = max_idx.max(count_params_in_expr(w));
    }
    for e in &select.group_by {
        max_idx = max_idx.max(count_params_in_expr(e));
    }
    if let Some(h) = &select.having {
        max_idx = max_idx.max(count_params_in_expr(h));
    }

    // ORDER BY / LIMIT / OFFSET
    for ob in &select.order_by {
        max_idx = max_idx.max(count_params_in_expr(&ob.expr));
    }
    if let Some(l) = &select.limit {
        max_idx = max_idx.max(count_params_in_expr(l));
    }
    if let Some(o) = &select.offset {
        max_idx = max_idx.max(count_params_in_expr(o));
    }

    // 集合操作（UNION / INTERSECT / EXCEPT）
    if let Some(set_op) = &select.set_op {
        max_idx = max_idx.max(count_params_in_select(&set_op.left));
        max_idx = max_idx.max(count_params_in_select(&set_op.right));
    }

    max_idx
}

/// 遍历 TableFactor，处理派生表和表函数中的参数占位符
fn count_params_in_table_factor(tf: &szrsql_sql::ast::TableFactor) -> usize {
    use szrsql_sql::ast::TableFactor;
    match tf {
        TableFactor::Table { .. } => 0,
        TableFactor::Derived { subquery, .. } => count_params_in_select(subquery),
        TableFactor::TableFunction { args, .. } => {
            args.iter().map(count_params_in_expr).max().unwrap_or(0)
        }
    }
}

/// 遍历 SelectItem，返回其中的最大参数索引
fn count_params_in_select_item(item: &szrsql_sql::ast::SelectItem) -> usize {
    use szrsql_sql::ast::SelectItem;
    match item {
        SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
            count_params_in_expr(e)
        }
        SelectItem::QualifiedWildcard(_) | SelectItem::Wildcard => 0,
    }
}

/// 递归遍历表达式，返回其中出现的最大参数索引
fn count_params_in_expr(expr: &szrsql_sql::ast::Expr) -> usize {
    use szrsql_sql::ast::Expr;
    match expr {
        Expr::Parameter(idx) => *idx,
        Expr::Literal(_) | Expr::Identifier(_) | Expr::Wildcard | Expr::Array(_) => 0,
        Expr::BinaryOp { left, right, .. } => {
            count_params_in_expr(left).max(count_params_in_expr(right))
        }
        Expr::UnaryOp { expr: e, .. } => count_params_in_expr(e),
        Expr::Function { args, .. } | Expr::WindowFunction { args, .. } => {
            args.iter().map(count_params_in_expr).max().unwrap_or(0)
        }
        Expr::Case {
            operand,
            when_then,
            else_expr,
        } => {
            let mut max_idx = 0;
            if let Some(o) = operand {
                max_idx = max_idx.max(count_params_in_expr(o));
            }
            for (w, t) in when_then {
                max_idx = max_idx.max(count_params_in_expr(w));
                max_idx = max_idx.max(count_params_in_expr(t));
            }
            if let Some(e) = else_expr {
                max_idx = max_idx.max(count_params_in_expr(e));
            }
            max_idx
        }
        Expr::Cast { expr: e, .. } => count_params_in_expr(e),
        Expr::InList { expr: e, list, .. } => {
            let mut max_idx = count_params_in_expr(e);
            for item in list {
                max_idx = max_idx.max(count_params_in_expr(item));
            }
            max_idx
        }
        Expr::InSubquery {
            expr: e, subquery, ..
        } => count_params_in_expr(e).max(count_params_in_select(subquery)),
        Expr::Between {
            expr: e, low, high, ..
        } => count_params_in_expr(e)
            .max(count_params_in_expr(low))
            .max(count_params_in_expr(high)),
        Expr::Like {
            expr: e, pattern, ..
        }
        | Expr::SimilarTo {
            expr: e, pattern, ..
        } => count_params_in_expr(e).max(count_params_in_expr(pattern)),
        Expr::IsNull { expr: e, .. } => count_params_in_expr(e),
        Expr::IsDistinctFrom { left, right, .. } => {
            count_params_in_expr(left).max(count_params_in_expr(right))
        }
        Expr::Subquery(sel) => count_params_in_select(sel),
        Expr::Exists { subquery, .. } => count_params_in_select(subquery),
        Expr::Substring {
            expr: e,
            from,
            for_len,
        } => {
            let mut max_idx = count_params_in_expr(e);
            if let Some(f) = from {
                max_idx = max_idx.max(count_params_in_expr(f));
            }
            if let Some(fl) = for_len {
                max_idx = max_idx.max(count_params_in_expr(fl));
            }
            max_idx
        }
        Expr::Tuple(exprs) => exprs.iter().map(count_params_in_expr).max().unwrap_or(0),
        Expr::AnyOp { left, right, .. } | Expr::AllOp { left, right, .. } => {
            count_params_in_expr(left).max(count_params_in_expr(right))
        }
        // P3-1: GROUP BY constructs — no parameter placeholders
        Expr::GroupingSets(sets) => sets
            .iter()
            .flat_map(|s| s.iter())
            .map(count_params_in_expr)
            .max()
            .unwrap_or(0),
        Expr::Cube(cols) | Expr::Rollup(cols) => {
            cols.iter().map(count_params_in_expr).max().unwrap_or(0)
        }
    }
}

// =====================================================================
//  ExecutorBackend — 连接真实执行器的 MCP 后端（Phase TDengine-P3-Full）
// =====================================================================

/// 执行单条 Statement 的结果类型：(columns, rows, affected_rows)
type ExecResult = Result<(Vec<String>, Vec<Vec<Value>>, u64), McpError>;

/// 连接真实执行器的 MCP 后端 — Phase TDengine-P3-Full
///
/// # 设计
///
/// 在 `CatalogBackend`（P3-MVP）基础上进一步接入真实 SQL 执行器，
/// 让 `execute_sql` / `explain_query` / `prepare_statement` 三个 Query 类工具
/// 返回真实执行结果。
///
/// ## 持有资源
///
/// - `catalog: RefCell<InMemoryCatalog>` — owned schema 存储（实现 `Catalog` trait）
/// - `tables: RefCell<HashMap<String, InMemoryTable>>` — owned 数据存储
///
/// ## 内部可变性
///
/// `McpBackendV2` trait 要求 `execute_sql(&self, ...)`，但 SQL 执行需要修改
/// catalog 和 tables。使用 `RefCell` 实现内部可变性，因 MCP over stdio 是
/// 单线程同步模型，`RefCell` 的运行时借用检查足够安全。
///
/// ## 执行流程（execute_sql）
///
/// 1. `parse_sql(sql)` → `Vec<Statement>`
/// 2. 对每条 Statement：
///    - `Statement::Comment` → 直接调用 `catalog.set_column_comment` / `set_table_comment`
///    - 其他 → `Planner::new(&catalog).plan_statement(stmt)` → `LogicalPlan`
///    - 根据 `LogicalPlan` variant 分派执行
///
/// ## 借用策略
///
/// - DDL：顺序 `borrow_mut()`，不重叠
/// - DML：先 `borrow_mut()` 移除目标表，再在内部作用域中 `borrow()` catalog
///   和剩余 tables 构造 Executor，执行后 `borrow_mut()` 放回目标表
/// - 读路径：`borrow()` catalog 和 tables 构造 Executor
///
/// # 用途
///
/// 让 LLM 通过 MCP 协议执行真实 SQL（CREATE/INSERT/SELECT/UPDATE/DELETE），
/// 覆盖"查数据"这一第二高频场景。修复差距 1（MCP 全 Mock）的 Query 子集。
pub struct ExecutorBackend {
    /// Schema 存储 — 实现 `szrsql_sql::plan::Catalog` trait
    catalog: RefCell<szrsql_sql::plan::InMemoryCatalog>,
    /// 数据存储 — 表名（小写）→ InMemoryTable
    tables: RefCell<HashMap<String, szrsql_sql::executor::InMemoryTable>>,
    /// 运行时统计 — 收集 execute_sql 调用记录（P3-Runtime）
    stats: RefCell<RuntimeStats>,
    /// 数据血缘存储 — 记录表/字段级血缘关系（P3-Lineage）
    lineage: RefCell<LineageStore>,
    /// 表维护状态 — 记录每张表的 vacuum/analyze 历史（P3-Maintenance）
    maintenance: RefCell<HashMap<String, TableMaintenanceState>>,
    /// 活动查询映射 — query_id → 查询信息（用于 cancel_query）
    active_queries: RefCell<HashMap<u64, ActiveQuery>>,
    /// 下一个事务 ID（自增）— 仅用于 MCP 层模拟事务 ID（兼容老测试）
    next_txn_id: Cell<u32>,
    /// 下一个查询 ID（自增）
    next_query_id: Cell<u64>,
    /// 下一个会话 ID（自增）
    next_session_id: Cell<u32>,
    /// 当前会话 ID（模拟单会话）
    current_session_id: Cell<u32>,
    /// MVCC 事务管理器（P3-Tx-Enhancement）— 提供真实状态机、快照、隔离级别
    ///
    /// `BEGIN` 时调用 `begin_with_isolation` 分配真实 txn_id + 快照；
    /// `COMMIT`/`ROLLBACK` 调用对应方法走状态机转换。
    /// MCP 层的 `next_txn_id` 仅作为未注入 MVCC 时的回退。
    mvcc: szrsql_tx::mvcc::MvccManager,
    /// 当前活动会话 ID（P3-MultiSession）
    ///
    /// 多会话模型：`current_session` 标识当前活跃的会话；
    /// `sessions` 存储所有会话的事务状态。
    /// 无会话时为 None，此时 BEGIN 使用 "default" 会话（向后兼容）。
    current_session: RefCell<Option<String>>,
    /// 所有会话的事务状态（P3-MultiSession）
    ///
    /// key = session_id, value = 该会话的活动 txn_id
    /// BEGIN 时插入，COMMIT/ROLLBACK 时移除
    sessions: RefCell<std::collections::HashMap<String, u32>>,
    /// 锁管理器（P3-Deadlock-Detection）— 提供真实锁表 + 等待图环检测
    ///
    /// `record_lock` 时调用 `try_lock` 真实加锁；冲突时记录等待边，
    /// 并调用 `detect_all_deadlocks` 检测环，发现死锁则写入 `deadlock_history`。
    /// `COMMIT`/`ROLLBACK`/`kill_transaction` 时调用 `unlock_all` 释放锁。
    lock_mgr: szrsql_tx::lock::LockManager,
    /// 统计信息存储（P5.1）— ANALYZE 收集的列级统计，供 CostModel 估算成本
    ///
    /// `analyze_table` 调用 `StatisticsCollector::collect` 扫描全表，
    /// 将 `TableStatistics` 写入此 store；`explain_query` 通过 `CostModel`
    /// 读取此 store 估算计划成本与行数。
    stats_store: RefCell<szrsql_optimizer::statistics::InMemoryStatisticsStore>,
}

// 会话事务状态（P3-MultiSession）— 已内联到 sessions HashMap，无需独立结构

/// 表维护状态 — 记录 vacuum/analyze 历史与死元组统计
#[derive(Debug, Clone, Default)]
struct TableMaintenanceState {
    /// 死元组数量（DELETE/UPDATE 累计）
    dead_tuples: u64,
    /// 活元组数量
    live_tuples: u64,
    /// 上次 VACUUM 时间戳（Unix 毫秒，0 表示未执行）
    last_vacuum_ms: u64,
    /// 上次 ANALYZE 时间戳（Unix 毫秒，0 表示未执行）
    last_analyze_ms: u64,
    /// VACUUM 执行次数
    vacuum_count: u32,
    /// ANALYZE 执行次数
    analyze_count: u32,
}

/// 活动查询 — 用于 cancel_query
#[derive(Debug, Clone)]
struct ActiveQuery {
    /// 查询 ID
    query_id: u64,
    /// SQL 文本
    sql: String,
    /// 开始时间戳（Unix 毫秒）
    started_at: u64,
    /// 是否已取消
    cancelled: bool,
}

/// 运行时统计 — 记录查询历史与当前活动会话（P3-Runtime）
///
/// 设计为内部可变性结构，由 `ExecutorBackend` 持有。每次 `execute_sql`
/// 会追加一条记录到 `query_history`，并按 SQL 文本归并到 `query_aggr`。
#[derive(Debug, Default)]
struct RuntimeStats {
    /// 完整查询历史（按时间顺序）
    query_history: Vec<QueryRecord>,
    /// 按 SQL 文本归并的统计（SQL → (count, total_ms, max_ms)）
    query_aggr: HashMap<String, QueryAggr>,
    /// 当前活动事务列表（按 txn_id 升序）
    active_transactions: Vec<TransactionInfo>,
    /// 当前活动锁列表（按授予时间排序）
    active_locks: Vec<LockInfo>,
    /// 等待事件聚合（按事件名归并）
    wait_events: HashMap<String, WaitEventAggr>,
    /// 死锁历史（按时间顺序）
    deadlock_history: Vec<DeadlockRecord>,
    /// 当前活动会话列表
    active_sessions: Vec<SessionInfo>,
    /// 告警列表（按时间顺序，最新追加）
    alerts: Vec<AlertInfo>,
    /// 统计是否已重置（用于 reset_stats 返回值）
    stats_reset: bool,
    /// 慢查询阈值（毫秒）— 超过此值触发慢查询告警
    slow_query_threshold_ms: u64,
    /// 慢查询计数（用于告警）
    slow_query_count: u64,
    /// 错误查询计数（用于告警）
    error_query_count: u64,
    /// VACUUM 总次数（用于 autovacuum_status）
    total_vacuum_count: u32,
    /// ANALYZE 总次数（用于 autovacuum_status）
    total_analyze_count: u32,
    /// 上次 autovacuum 运行时间戳
    last_autovacuum_ms: u64,
}

/// 等待事件聚合
#[derive(Debug, Clone, Default)]
struct WaitEventAggr {
    /// 总等待次数
    total_waits: u64,
    /// 总等待时长（毫秒）
    total_wait_ms: u64,
}

/// 单次查询记录
#[derive(Debug, Clone)]
struct QueryRecord {
    /// SQL 文本
    sql: String,
    /// 执行耗时（毫秒）
    elapsed_ms: u64,
    /// 受影响行数
    affected_rows: u64,
    /// 时间戳（Unix 毫秒）
    timestamp: u64,
}

/// 按 SQL 文本归并的查询统计
#[derive(Debug, Clone, Default)]
struct QueryAggr {
    /// 执行次数
    count: u64,
    /// 总耗时（毫秒）
    total_ms: u64,
    /// 最大耗时（毫秒）
    max_ms: u64,
}

/// 数据血缘存储 — 记录表/字段级血缘关系（P3-Lineage）
///
/// 血缘边为有向边：source → target（target 依赖 source）。
/// 支持 CTAS（CREATE TABLE AS SELECT）、VIEW、INSERT INTO SELECT、手动标注。
#[derive(Debug, Default)]
struct LineageStore {
    /// 血缘边列表（source → target）
    edges: Vec<LineageEdgeDto>,
}

impl LineageStore {
    /// 添加一条血缘边
    fn add_edge(&mut self, edge: LineageEdgeDto) {
        // 去重（相同 source + target + transform 不重复添加）
        if !self.edges.contains(&edge) {
            self.edges.push(edge);
        }
    }

    /// 查询某表的上游血缘（target = table）
    fn upstream_of(&self, table: &str) -> Vec<LineageEdgeDto> {
        self.edges
            .iter()
            .filter(|e| e.target.table.eq_ignore_ascii_case(table))
            .cloned()
            .collect()
    }

    /// 查询某表的下游血缘（source = table）
    fn downstream_of(&self, table: &str) -> Vec<LineageEdgeDto> {
        self.edges
            .iter()
            .filter(|e| e.source.table.eq_ignore_ascii_case(table))
            .cloned()
            .collect()
    }

    /// 所有涉及的表（去重排序）
    fn all_tables(&self) -> Vec<String> {
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for e in &self.edges {
            set.insert(e.source.table.to_lowercase());
            set.insert(e.target.table.to_lowercase());
        }
        set.into_iter().collect()
    }

    /// 总边数
    fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

impl ExecutorBackend {
    /// 创建空 ExecutorBackend
    pub fn new() -> Self {
        Self {
            catalog: RefCell::new(szrsql_sql::plan::InMemoryCatalog::new()),
            tables: RefCell::new(HashMap::new()),
            stats: RefCell::new(RuntimeStats {
                slow_query_threshold_ms: 1000, // 默认 1 秒
                ..Default::default()
            }),
            lineage: RefCell::new(LineageStore::default()),
            maintenance: RefCell::new(HashMap::new()),
            active_queries: RefCell::new(HashMap::new()),
            next_txn_id: Cell::new(1),
            next_query_id: Cell::new(1),
            next_session_id: Cell::new(1),
            current_session_id: Cell::new(0),
            mvcc: szrsql_tx::mvcc::MvccManager::new(),
            current_session: RefCell::new(None),
            sessions: RefCell::new(std::collections::HashMap::new()),
            lock_mgr: szrsql_tx::lock::LockManager::new(),
            stats_store: RefCell::new(szrsql_optimizer::statistics::InMemoryStatisticsStore::new()),
        }
    }

    /// 创建带初始 catalog 和 tables 的 ExecutorBackend
    ///
    /// 调用方需保证 catalog 和 tables 中的表名一致。
    pub fn with_data(
        catalog: szrsql_sql::plan::InMemoryCatalog,
        tables: HashMap<String, szrsql_sql::executor::InMemoryTable>,
    ) -> Self {
        Self {
            catalog: RefCell::new(catalog),
            tables: RefCell::new(tables),
            stats: RefCell::new(RuntimeStats {
                slow_query_threshold_ms: 1000,
                ..Default::default()
            }),
            lineage: RefCell::new(LineageStore::default()),
            maintenance: RefCell::new(HashMap::new()),
            active_queries: RefCell::new(HashMap::new()),
            next_txn_id: Cell::new(1),
            next_query_id: Cell::new(1),
            next_session_id: Cell::new(1),
            current_session_id: Cell::new(0),
            mvcc: szrsql_tx::mvcc::MvccManager::new(),
            current_session: RefCell::new(None),
            sessions: RefCell::new(std::collections::HashMap::new()),
            lock_mgr: szrsql_tx::lock::LockManager::new(),
            stats_store: RefCell::new(szrsql_optimizer::statistics::InMemoryStatisticsStore::new()),
        }
    }

    /// 获取当前 Unix 时间戳（毫秒）
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// P3-MultiSession：创建新会话并设为当前会话
    ///
    /// 返回新分配的 session_id。若已有同名会话则返回错误。
    pub fn begin_session(&self, session_id: &str) -> Result<(), McpError> {
        let sessions = self.sessions.borrow();
        if sessions.contains_key(session_id) {
            return Err(McpError::BackendError(format!(
                "session already exists: {session_id}"
            )));
        }
        drop(sessions);
        // 插入空会话（无活动事务，txn_id=0 表示无事务）
        self.sessions.borrow_mut().insert(session_id.to_string(), 0);
        *self.current_session.borrow_mut() = Some(session_id.to_string());
        Ok(())
    }

    /// P3-MultiSession：结束会话
    ///
    /// 若该会话有活动事务，自动回滚。
    pub fn end_session(&self, session_id: &str) -> Result<(), McpError> {
        let txn_id = self.sessions.borrow_mut().remove(session_id);
        if let Some(tid) = txn_id {
            if tid > 0 {
                // 回滚未提交的事务
                let _ = self.mvcc.abort(tid);
                self.lock_mgr.unlock_all(tid);
            }
        }
        // 若结束的是当前会话，清空 current_session
        if self.current_session.borrow().as_deref() == Some(session_id) {
            *self.current_session.borrow_mut() = None;
        }
        Ok(())
    }

    /// P3-MultiSession：切换当前会话
    pub fn set_current_session(&self, session_id: &str) -> Result<(), McpError> {
        let sessions = self.sessions.borrow();
        if !sessions.contains_key(session_id) {
            return Err(McpError::BackendError(format!(
                "session not found: {session_id}"
            )));
        }
        *self.current_session.borrow_mut() = Some(session_id.to_string());
        Ok(())
    }

    /// P3-MultiSession：获取当前会话的活动 txn_id（None 表示无活动事务）
    fn current_txn_id(&self) -> Option<u32> {
        let session_id = self.current_session.borrow().clone()?;
        let sessions = self.sessions.borrow();
        let txn_id = sessions.get(&session_id).copied()?;
        if txn_id > 0 {
            Some(txn_id)
        } else {
            None
        }
    }

    /// P3-MultiSession：设置当前会话的活动 txn_id
    fn set_current_txn_id(&self, txn_id: Option<u32>) {
        let session_id = self
            .current_session
            .borrow()
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let mut sessions = self.sessions.borrow_mut();
        // 若 "default" 会话不存在且 current_session 为 None，自动创建
        if !sessions.contains_key(&session_id) {
            sessions.insert(session_id.clone(), 0);
            *self.current_session.borrow_mut() = Some(session_id.clone());
        }
        sessions.insert(session_id, txn_id.unwrap_or(0));
    }

    /// P3-MultiSession：列出所有会话
    pub fn list_sessions(&self) -> Vec<String> {
        self.sessions.borrow().keys().cloned().collect()
    }

    /// P3-MultiSession：当前活动会话数
    pub fn active_session_count(&self) -> usize {
        self.sessions.borrow().len()
    }

    /// 分配下一个事务 ID
    fn alloc_txn_id(&self) -> u32 {
        let id = self.next_txn_id.get();
        self.next_txn_id.set(id.wrapping_add(1));
        id
    }

    /// 分配下一个查询 ID
    fn alloc_query_id(&self) -> u64 {
        let id = self.next_query_id.get();
        self.next_query_id.set(id.wrapping_add(1));
        id
    }

    /// 分配下一个会话 ID
    fn alloc_session_id(&self) -> u32 {
        let id = self.next_session_id.get();
        self.next_session_id.set(id.wrapping_add(1));
        id
    }

    /// 表名 → 小写 key（与 InMemoryCatalog::key 一致）
    fn table_key(name: &str) -> String {
        name.to_lowercase()
    }

    /// 从 `catalog.list_tables()` 中查找简单表名匹配的 `TableName`
    fn find_table_name(&self, table: &str) -> Option<szrsql_sql::ast::TableName> {
        use szrsql_sql::plan::Catalog;
        self.catalog
            .borrow()
            .list_tables()
            .into_iter()
            .find(|n| n.name == table)
    }

    /// 执行单条 Statement，返回 (columns, rows, affected_rows)
    fn execute_statement_inner(&self, stmt: szrsql_sql::ast::Statement) -> ExecResult {
        use szrsql_sql::ast::Statement;
        use szrsql_sql::plan::{LogicalPlan, Planner};

        // COMMENT ON 语句不走 Planner，直接操作 catalog
        if let Statement::Comment {
            object_type,
            object_name,
            column_name,
            comment,
        } = stmt
        {
            return self.execute_comment(object_type, object_name, column_name, comment);
        }

        // 其他语句走 Planner
        let plan = {
            let catalog = self.catalog.borrow();
            Planner::new(&*catalog)
                .plan_statement(stmt)
                .map_err(|e| McpError::BackendError(format!("plan error: {e}")))?
        };

        match &plan {
            // --- DDL ---
            LogicalPlan::CreateTable {
                name,
                columns,
                if_not_exists,
                ..
            } => {
                {
                    use szrsql_sql::plan::Catalog;
                    if *if_not_exists && self.catalog.borrow().table_exists(name) {
                        return Ok((vec![], vec![], 0));
                    }
                }
                // 注册 schema + 外键
                self.catalog
                    .borrow_mut()
                    .register_from_create_plan(&plan)
                    .map_err(|e| McpError::BackendError(format!("register table error: {e:?}")))?;
                // 创建数据存储
                let schema = szrsql_sql::plan::TableSchema {
                    name: name.clone(),
                    columns: columns.clone(),
                };
                let key = Self::table_key(&name.name);
                self.tables
                    .borrow_mut()
                    .insert(key, szrsql_sql::executor::InMemoryTable::new(schema));
                Ok((vec![], vec![], 0))
            }

            LogicalPlan::DropTable {
                names, if_exists, ..
            } => {
                for name in names {
                    {
                        use szrsql_sql::plan::Catalog;
                        if !self.catalog.borrow().table_exists(name) {
                            if *if_exists {
                                continue;
                            }
                            return Err(McpError::BackendError(format!(
                                "table not found: {}",
                                name.qualified_name()
                            )));
                        }
                    }
                    self.catalog.borrow_mut().remove_table(name);
                    self.tables
                        .borrow_mut()
                        .remove(&Self::table_key(&name.name));
                }
                Ok((vec![], vec![], 0))
            }

            LogicalPlan::CreateIndex {
                name,
                table,
                columns,
                unique,
                if_not_exists,
            } => {
                use szrsql_sql::plan::Catalog;
                let idx_name = name.clone().unwrap_or_else(|| {
                    format!(
                        "idx_{}_{}",
                        table.name.to_lowercase(),
                        columns.first().map(|c| c.column.as_str()).unwrap_or("col")
                    )
                });
                if *if_not_exists {
                    let existing = self.catalog.borrow().list_indexes(table);
                    if existing
                        .iter()
                        .any(|i| i.name.eq_ignore_ascii_case(&idx_name))
                    {
                        return Ok((vec![], vec![], 0));
                    }
                }
                let idx = szrsql_sql::plan::IndexDefinition {
                    name: idx_name,
                    table: table.clone(),
                    columns: columns.clone(),
                    unique: *unique,
                };
                self.catalog.borrow_mut().add_index(idx);
                Ok((vec![], vec![], 0))
            }

            LogicalPlan::DropIndex {
                names, if_exists, ..
            } => {
                for name in names {
                    let removed = self.catalog.borrow_mut().remove_index(name);
                    if removed.is_none() && !*if_exists {
                        return Err(McpError::BackendError(format!("index not found: {name}")));
                    }
                }
                Ok((vec![], vec![], 0))
            }

            LogicalPlan::Truncate { names, .. } => {
                use szrsql_sql::executor::{MutableTable, TableStorage};
                let mut affected = 0u64;
                let mut tables = self.tables.borrow_mut();
                for name in names {
                    let key = Self::table_key(&name.name);
                    if let Some(table) = tables.get_mut(&key) {
                        affected += table.row_count() as u64;
                        table.clear();
                    }
                }
                Ok((vec![], vec![], affected))
            }

            // --- DML：INSERT / UPDATE / DELETE ---
            LogicalPlan::Insert { table, .. }
            | LogicalPlan::Update { table, .. }
            | LogicalPlan::Delete { table, .. } => self.execute_dml(&plan, table),

            // --- 读路径：SELECT 等 ---
            _ => self.execute_read(&plan),
        }
    }

    /// 执行 COMMENT ON 语句
    fn execute_comment(
        &self,
        object_type: szrsql_sql::ast::CommentObjectType,
        object_name: szrsql_sql::ast::TableName,
        column_name: Option<String>,
        comment: Option<String>,
    ) -> ExecResult {
        use szrsql_sql::ast::CommentObjectType;
        match object_type {
            CommentObjectType::Table => {
                self.catalog
                    .borrow_mut()
                    .set_table_comment(&object_name, comment)
                    .map_err(|e| {
                        McpError::BackendError(format!("set table comment error: {e:?}"))
                    })?;
            }
            CommentObjectType::Column => {
                let col = column_name.ok_or_else(|| {
                    McpError::BackendError("COMMENT ON COLUMN requires column name".into())
                })?;
                self.catalog
                    .borrow_mut()
                    .set_column_comment(&object_name, &col, comment)
                    .map_err(|e| {
                        McpError::BackendError(format!("set column comment error: {e:?}"))
                    })?;
            }
        }
        Ok((vec![], vec![], 0))
    }

    /// 执行 DML（INSERT / UPDATE / DELETE）
    ///
    /// 策略：从 tables 中 temporarily remove 目标表，构造 Executor，
    /// 执行后再 insert 回去。避免可变借用与 Executor 不可变借用冲突。
    fn execute_dml(
        &self,
        plan: &szrsql_sql::plan::LogicalPlan,
        table_name: &szrsql_sql::ast::TableName,
    ) -> ExecResult {
        use szrsql_sql::executor::Executor;
        use szrsql_sql::plan::LogicalPlan;

        let key = Self::table_key(&table_name.name);
        // 取出目标表（temporarily remove 避免借用冲突）
        let mut target_table = self.tables.borrow_mut().remove(&key).ok_or_else(|| {
            McpError::BackendError(format!("table not found: {}", table_name.name))
        })?;

        // 构造 Executor，注册其他表（用于 INSERT...SELECT 等跨表场景）
        let result = {
            let catalog = self.catalog.borrow();
            let tables = self.tables.borrow();
            let mut exec = Executor::new()
                .with_catalog(&*catalog)
                .with_sql_functions_from_catalog(&catalog);
            for other_table in (*tables).values() {
                exec.register_table(other_table);
            }
            match plan {
                LogicalPlan::Insert { .. } => exec.execute_insert(plan, &mut target_table),
                LogicalPlan::Update { .. } => exec.execute_update(plan, &mut target_table),
                LogicalPlan::Delete { .. } => exec.execute_delete(plan, &mut target_table),
                _ => unreachable!("execute_dml called with non-DML plan"),
            }
        };

        // 将目标表放回
        self.tables.borrow_mut().insert(key, target_table);

        let dml_result =
            result.map_err(|e| McpError::BackendError(format!("execute error: {e:?}")))?;

        // 提取 RETURNING 行的列名（如果有）
        let columns = match plan {
            LogicalPlan::Insert { returning, .. }
            | LogicalPlan::Update { returning, .. }
            | LogicalPlan::Delete { returning, .. } => {
                if let Some(returning_items) = returning {
                    returning_items
                        .iter()
                        .map(|item| match item {
                            szrsql_sql::ast::SelectItem::UnnamedExpr(_) => String::new(),
                            szrsql_sql::ast::SelectItem::ExprWithAlias { alias, .. } => {
                                alias.clone()
                            }
                            _ => String::new(),
                        })
                        .collect()
                } else {
                    vec![]
                }
            }
            _ => vec![],
        };

        // 将 returning_rows 转换为 serde_json::Value
        let json_rows: Vec<Vec<Value>> = dml_result
            .returning_rows
            .into_iter()
            .map(|row| row.into_iter().map(value_to_json).collect())
            .collect();

        Ok((columns, json_rows, dml_result.affected_rows as u64))
    }

    /// 执行读路径（SELECT 等）
    fn execute_read(&self, plan: &szrsql_sql::plan::LogicalPlan) -> ExecResult {
        use szrsql_sql::executor::Executor;

        let rows = {
            let catalog = self.catalog.borrow();
            let tables = self.tables.borrow();
            let mut exec = Executor::new()
                .with_catalog(&*catalog)
                .with_sql_functions_from_catalog(&catalog);
            for table in (*tables).values() {
                exec.register_table(table);
            }
            exec.execute(plan)
                .map_err(|e| McpError::BackendError(format!("execute error: {e:?}")))?
        };

        // 提取列名（从 plan 的输出 schema 推断，简化处理）
        let columns = self.extract_column_names(plan);

        // 转换为 serde_json::Value
        let json_rows: Vec<Vec<Value>> = rows
            .into_iter()
            .map(|row| row.into_iter().map(value_to_json).collect())
            .collect();

        Ok((columns, json_rows, 0))
    }

    /// 从 LogicalPlan 提取输出列名（简化版）
    fn extract_column_names(&self, plan: &szrsql_sql::plan::LogicalPlan) -> Vec<String> {
        use szrsql_sql::plan::LogicalPlan;
        match plan {
            LogicalPlan::Scan { schema, .. } => {
                schema.columns.iter().map(|c| c.name.clone()).collect()
            }
            LogicalPlan::Projection { output_names, .. } => output_names.clone(),
            _ => vec![],
        }
    }
}

impl Default for ExecutorBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// ExecutorBackend 内部辅助方法 — 采集运行时事件（事务/锁/会话/血缘/维护）
///
/// 这些方法不是 `McpBackendV2` trait 方法，仅供 `execute_sql` 内部调用。
impl ExecutorBackend {
    /// 查询结束时清理活动查询列表
    fn finalize_query(&self, query_id: u64) {
        self.active_queries.borrow_mut().remove(&query_id);
    }

    /// 预扫描语句列表，采集事务/锁/会话/血缘/维护事件
    ///
    /// 在实际执行前调用，用于：
    /// - BEGIN/COMMIT/ROLLBACK → 走 MVCC 状态机 + 维护活动事务列表
    /// - INSERT/UPDATE/DELETE → 记录锁持有 + 更新死元组统计
    /// - CREATE VIEW → 记录血缘（View 类型）
    /// - INSERT INTO SELECT → 记录血缘（Ctas 类型）
    fn collect_runtime_events(&self, stmts: &[szrsql_sql::ast::Statement], now_ms: u64) {
        use szrsql_sql::ast::{InsertSource, Statement, TransactionIsolation};
        use szrsql_tx::mvcc::IsolationLevel;

        for stmt in stmts {
            match stmt {
                Statement::Begin { isolation, .. } => {
                    // P3-Tx-Enhancement：通过 MVCC 管理器分配真实 txn_id + 快照
                    let iso = isolation
                        .map(|i| match i {
                            TransactionIsolation::ReadUncommitted => {
                                IsolationLevel::ReadUncommitted
                            }
                            TransactionIsolation::ReadCommitted => IsolationLevel::ReadCommitted,
                            TransactionIsolation::RepeatableRead => IsolationLevel::RepeatableRead,
                            TransactionIsolation::Serializable => IsolationLevel::Serializable,
                        })
                        .unwrap_or(IsolationLevel::RepeatableRead);
                    let txn = self.mvcc.begin_with_isolation(iso);
                    let txn_id = txn.txn_id;
                    self.set_current_txn_id(Some(txn_id));
                    let session_id = self.ensure_session(now_ms);

                    let mut stats = self.stats.borrow_mut();
                    // 移除同 session 的旧事务（避免重复），添加新事务
                    stats.active_transactions.retain(|t| t.txn_id != txn_id);
                    stats.active_transactions.push(TransactionInfo {
                        txn_id,
                        state: "active".to_string(),
                        started_at: now_ms,
                        sql: "BEGIN".to_string(),
                        wait_event: None,
                        isolation: Some(format!("{:?}", iso)),
                        snapshot_active_count: Some(txn.snapshot.active_txns.len() as u32),
                        snapshot_xmax: Some(txn.snapshot.xmax),
                    });
                    // 更新会话状态
                    if let Some(session) = stats
                        .active_sessions
                        .iter_mut()
                        .find(|s| s.session_id == session_id)
                    {
                        session.state = "active".to_string();
                        session.sql = "BEGIN".to_string();
                    }
                }
                Statement::Commit => {
                    // P3-Tx-Enhancement：走 MVCC 状态机提交
                    // P3-Deadlock-Detection：通过 LockManager 释放所有锁
                    // P3-MultiSession：只提交当前会话的事务
                    let committed_txn = self.current_txn_id();
                    self.set_current_txn_id(None);
                    if let Some(txn_id) = committed_txn {
                        // MVCC commit 需要 commit_lsn，MCP 场景用 0（无 WAL）
                        // OPT-16 修复：原 `let _ =` 吞掉 commit 错误，导致 SSI 写偏斜/写写冲突
                        // 检测失败时客户端误以为事务已提交，但 MVCC 内部已标记为 Aborted，
                        // 造成状态不一致。commit_inner 失败时已自动从 active_txns 移除并加入
                        // aborted_txns，此处只需记录告警供监控可见，并继续释放锁和清理 stats。
                        if let Err(e) = self.mvcc.commit(txn_id, 0) {
                            let mut stats = self.stats.borrow_mut();
                            stats.alerts.push(AlertInfo {
                                level: "critical".to_string(),
                                rule_id: "mvcc_commit_failed".to_string(),
                                message: format!("MVCC commit failed for txn {txn_id}: {e:?}"),
                                timestamp: now_ms,
                                value: 1.0,
                                threshold: 0.0,
                            });
                        }
                        // 释放该事务在 LockManager 中持有的所有锁
                        self.lock_mgr.unlock_all(txn_id);
                        // 仅移除该事务，保留其他会话的活动事务
                        let mut stats = self.stats.borrow_mut();
                        stats.active_transactions.retain(|t| t.txn_id != txn_id);
                        // 仅释放该事务的锁
                        stats.active_locks.retain(|l| l.txn_id != txn_id);
                    } else {
                        // 无活动事务（兼容：清空所有）
                        let mut stats = self.stats.borrow_mut();
                        stats.active_transactions.clear();
                        stats.active_locks.clear();
                    }
                }
                Statement::Rollback { .. } => {
                    // P3-Tx-Enhancement：走 MVCC 状态机回滚
                    // P3-Deadlock-Detection：通过 LockManager 释放所有锁
                    // P3-MultiSession：只回滚当前会话的事务
                    let rolled_txn = self.current_txn_id();
                    self.set_current_txn_id(None);
                    if let Some(txn_id) = rolled_txn {
                        let _ = self.mvcc.abort(txn_id);
                        // 释放该事务在 LockManager 中持有的所有锁
                        self.lock_mgr.unlock_all(txn_id);
                        // 仅移除该事务
                        let mut stats = self.stats.borrow_mut();
                        stats.active_transactions.retain(|t| t.txn_id != txn_id);
                        stats.active_locks.retain(|l| l.txn_id != txn_id);
                    } else {
                        let mut stats = self.stats.borrow_mut();
                        stats.active_transactions.clear();
                        stats.active_locks.clear();
                    }
                }
                Statement::Insert { table, source, .. }
                | Statement::Replace { table, source, .. } => {
                    let table_name = &table.name;
                    // 记录锁（INSERT 持有写锁）
                    self.record_lock(table_name, "RowExclusiveLock", true, now_ms);
                    // INSERT INTO SELECT → 记录血缘
                    if let InsertSource::Select(select) = source {
                        self.record_lineage_from_select(
                            &table.name,
                            select,
                            LineageEdgeSource::Ctas,
                        );
                    }
                    // 更新维护状态（死元组不变，活元组增加）
                    let mut maint = self.maintenance.borrow_mut();
                    let state = maint.entry(table_name.to_lowercase()).or_default();
                    state.live_tuples = state.live_tuples.saturating_add(1);
                }
                Statement::Update { table, .. } => {
                    let table_name = &table.name;
                    // UPDATE 持有写锁，产生死元组
                    self.record_lock(table_name, "RowExclusiveLock", true, now_ms);
                    let mut maint = self.maintenance.borrow_mut();
                    let state = maint.entry(table_name.to_lowercase()).or_default();
                    state.dead_tuples = state.dead_tuples.saturating_add(1);
                }
                Statement::Delete { table, .. } => {
                    let table_name = &table.name;
                    // DELETE 持有写锁，产生死元组
                    self.record_lock(table_name, "RowExclusiveLock", true, now_ms);
                    let mut maint = self.maintenance.borrow_mut();
                    let state = maint.entry(table_name.to_lowercase()).or_default();
                    state.dead_tuples = state.dead_tuples.saturating_add(1);
                    if state.live_tuples > 0 {
                        state.live_tuples -= 1;
                    }
                }
                Statement::CreateView { name, query, .. } => {
                    // CREATE VIEW → 记录血缘（View 类型）
                    self.record_lineage_from_select(&name.name, query, LineageEdgeSource::View);
                }
                _ => {}
            }
        }
    }

    /// 确保当前会话存在，返回会话 ID
    fn ensure_session(&self, _now_ms: u64) -> u32 {
        let session_id = self.current_session_id.get();
        if session_id == 0 {
            // 首次调用，创建会话
            let new_id = self.alloc_session_id();
            self.current_session_id.set(new_id);
            let mut stats = self.stats.borrow_mut();
            stats.active_sessions.push(SessionInfo {
                session_id: new_id,
                state: "idle".to_string(),
                sql: String::new(),
                wait_event: None,
                user: "mcp".to_string(),
            });
            new_id
        } else {
            // 更新会话最后活动时间（通过更新 state）
            let mut stats = self.stats.borrow_mut();
            if !stats
                .active_sessions
                .iter()
                .any(|s| s.session_id == session_id)
            {
                stats.active_sessions.push(SessionInfo {
                    session_id,
                    state: "idle".to_string(),
                    sql: String::new(),
                    wait_event: None,
                    user: "mcp".to_string(),
                });
            }
            session_id
        }
    }

    /// 记录锁持有（P3-Deadlock-Detection：通过真实 LockManager 加锁 + 检测环）
    ///
    /// 改造点：
    /// - 通过表名 hash 生成 resource_id (u64)，调用 `lock_mgr.try_lock` 真实加锁
    /// - 成功 → granted=true；冲突 → granted=false（等待边），并调用
    ///   `detect_all_deadlocks` 检测等待图环，发现死锁则写入 `deadlock_history`
    /// - txn_id=0（无活动事务）时仅记录到 active_locks（保持向后兼容）
    fn record_lock(&self, table: &str, mode: &str, granted: bool, now_ms: u64) {
        let txn_id = {
            let stats = self.stats.borrow();
            stats
                .active_transactions
                .last()
                .map(|t| t.txn_id)
                .unwrap_or(0)
        };

        // 映射 MCP 锁模式字符串到 LockMode（写锁 → Exclusive，读锁 → Share）
        let lock_mode = match mode {
            "RowExclusiveLock" | "ExclusiveLock" | "AccessExclusiveLock" => {
                szrsql_tx::lock::LockMode::Exclusive
            }
            // ShareLock / AccessShareLock / RowShareLock / 其他
            _ => szrsql_tx::lock::LockMode::Share,
        };

        // 通过表名 hash 生成稳定的 resource_id (u64)
        let resource_id = table_resource_id(table);

        // 真实加锁（仅当有活动事务时；txn_id=0 不走 LockManager）
        let real_granted = if txn_id == 0 {
            // 无活动事务，保持原 granted 参数（向后兼容）
            granted
        } else {
            match self.lock_mgr.try_lock(txn_id, resource_id, lock_mode) {
                Ok(()) => true,
                Err(szrsql_tx::lock::LockError::Conflict { .. }) => {
                    // 冲突 → 等待边，检测死锁
                    let cycles = self.lock_mgr.detect_all_deadlocks();
                    if !cycles.is_empty() {
                        self.record_deadlocks(&cycles, table, now_ms);
                    }
                    false
                }
                // 升级或其他错误视为未授予
                Err(_) => false,
            }
        };

        let mut stats = self.stats.borrow_mut();
        // 去重：同 txn + table + mode 不重复添加
        let exists = stats
            .active_locks
            .iter()
            .any(|l| l.txn_id == txn_id && l.table.eq_ignore_ascii_case(table) && l.mode == mode);
        if !exists {
            stats.active_locks.push(LockInfo {
                txn_id,
                table: table.to_string(),
                mode: mode.to_string(),
                granted: real_granted,
                wait_start: if real_granted {
                    None
                } else {
                    Some(now_ms)
                },
            });
        }
    }

    /// 将检测到的死锁环写入 deadlock_history（P3-Deadlock-Detection）
    ///
    /// 去重策略：同一组 txn_ids + 同一 resource 只记录一次，
    /// 避免重复检测导致历史膨胀。
    fn record_deadlocks(&self, cycles: &[Vec<u32>], table: &str, now_ms: u64) {
        let mut stats = self.stats.borrow_mut();
        for cycle in cycles {
            // 生成稳定的环签名（排序后拼接，便于去重）
            let mut sorted = cycle.clone();
            sorted.sort_unstable();
            let signature: Vec<u32> = sorted;
            let exists = stats.deadlock_history.iter().any(|d| {
                let mut d_sorted = d.txn_ids.clone();
                d_sorted.sort_unstable();
                d_sorted == signature && d.resource.eq_ignore_ascii_case(table)
            });
            if !exists {
                stats.deadlock_history.push(DeadlockRecord {
                    timestamp: now_ms,
                    txn_ids: cycle.clone(),
                    resource: table.to_string(),
                });
            }
        }
    }

    /// 从 SELECT 语句中提取源表，记录到 lineage（target ← source）
    fn record_lineage_from_select(
        &self,
        target_table: &str,
        select: &szrsql_sql::ast::Select,
        source_type: LineageEdgeSource,
    ) {
        let source_tables = extract_source_tables(select);
        let mut lineage = self.lineage.borrow_mut();
        for src in source_tables {
            // 字段级血缘：简化为直接映射（transform = "direct"）
            lineage.add_edge(LineageEdgeDto {
                source: ColumnRefDto {
                    table: src.clone(),
                    column: "*".to_string(),
                },
                target: ColumnRefDto {
                    table: target_table.to_string(),
                    column: "*".to_string(),
                },
                transform: "direct".to_string(),
                source_type,
            });
        }
    }
}

impl McpBackendV2 for ExecutorBackend {
    // --- 类别 1: Schema（真实，复用 InMemoryCatalog） ---

    fn list_tables(&self) -> Result<Vec<crate::mcp::TableInfo>, McpError> {
        use szrsql_sql::executor::TableStorage;
        use szrsql_sql::plan::Catalog;
        let tables_map = self.tables.borrow();
        let tables = self
            .catalog
            .borrow()
            .list_tables()
            .into_iter()
            .map(|name| {
                let row_count = tables_map
                    .get(&Self::table_key(&name.name))
                    .map(|t| t.row_count() as u64)
                    .unwrap_or(0);
                crate::mcp::TableInfo {
                    name: name.name.clone(),
                    row_count,
                    size_bytes: row_count * 64, // 粗略估算
                }
            })
            .collect();
        Ok(tables)
    }

    fn describe_table(&self, table: &str) -> Result<crate::mcp::TableSchema, McpError> {
        use szrsql_sql::plan::Catalog;
        let name = self
            .find_table_name(table)
            .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))?;
        let schema = self
            .catalog
            .borrow()
            .get_table(&name)
            .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))?;

        let catalog = self.catalog.borrow();
        let columns = schema
            .columns
            .iter()
            .map(|col| {
                let comment = catalog
                    .get_column_comment(&name, &col.name)
                    .or_else(|| col.comment.clone());
                crate::mcp::ColumnDef {
                    name: col.name.clone(),
                    data_type: column_type_to_string(&col.data_type),
                    nullable: !(col.not_null || col.primary_key),
                    primary_key: col.primary_key,
                    comment,
                }
            })
            .collect();

        Ok(crate::mcp::TableSchema {
            table: name.name.clone(),
            columns,
        })
    }

    fn list_indexes(&self, table: &str) -> Result<Vec<IndexInfo>, McpError> {
        use szrsql_sql::plan::Catalog;
        let name = self
            .find_table_name(table)
            .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))?;
        let indexes = self.catalog.borrow().list_indexes(&name);
        let result = indexes
            .into_iter()
            .map(|idx| {
                let columns: Vec<String> =
                    idx.column_names().into_iter().map(String::from).collect();
                let is_primary = idx.name.ends_with("_pkey");
                IndexInfo {
                    is_primary,
                    name: idx.name,
                    table: idx.table.name,
                    columns,
                    unique: idx.unique,
                }
            })
            .collect();
        Ok(result)
    }

    fn list_views(&self) -> Result<Vec<ViewInfo>, McpError> {
        Ok(vec![])
    }

    // --- 类别 2: Query（真实执行） ---

    fn execute_sql(&self, sql: &str) -> Result<crate::mcp::QueryResult, McpError> {
        let start = std::time::Instant::now();
        let now_ms = Self::now_ms();
        let query_id = self.alloc_query_id();

        // 注册活动查询（用于 cancel_query）
        {
            self.active_queries.borrow_mut().insert(
                query_id,
                ActiveQuery {
                    query_id,
                    sql: sql.to_string(),
                    started_at: now_ms,
                    cancelled: false,
                },
            );
        }

        // 解析 SQL（解析失败也记录到统计）
        let stmts = match szrsql_sql::parser::parse_sql(sql) {
            Ok(s) => s,
            Err(e) => {
                self.finalize_query(query_id);
                return Err(McpError::BackendError(format!("parse error: {e:?}")));
            }
        };

        // 预扫描语句类型，采集事务/血缘事件
        self.collect_runtime_events(&stmts, now_ms);

        let mut all_columns = vec![];
        let mut all_rows = vec![];
        let mut total_affected = 0u64;

        for stmt in stmts {
            match self.execute_statement_inner(stmt) {
                Ok((columns, rows, affected)) => {
                    all_columns = columns;
                    all_rows = rows;
                    total_affected += affected;
                }
                Err(e) => {
                    // 记录错误到统计（即使提前返回也要更新 error_query_count）
                    {
                        let mut stats = self.stats.borrow_mut();
                        stats.error_query_count += 1;
                        let error_count = stats.error_query_count;
                        if error_count > 0 && error_count.is_multiple_of(10) {
                            stats.alerts.push(AlertInfo {
                                level: "critical".to_string(),
                                rule_id: "high_error_rate".to_string(),
                                message: format!("High error rate: {} errors total", error_count),
                                timestamp: now_ms,
                                value: error_count as f64,
                                threshold: 10.0,
                            });
                        }
                    }
                    self.finalize_query(query_id);
                    return Err(e);
                }
            }
        }

        let elapsed_ms = start.elapsed().as_millis() as u64;
        self.finalize_query(query_id);

        // 记录到运行时统计（P3-Runtime）
        {
            let mut stats = self.stats.borrow_mut();
            stats.query_history.push(QueryRecord {
                sql: sql.to_string(),
                elapsed_ms,
                affected_rows: total_affected,
                timestamp: now_ms,
            });
            let aggr = stats.query_aggr.entry(sql.to_string()).or_default();
            aggr.count += 1;
            aggr.total_ms += elapsed_ms;
            if elapsed_ms > aggr.max_ms {
                aggr.max_ms = elapsed_ms;
            }

            // 慢查询告警检测
            let slow_threshold = stats.slow_query_threshold_ms;
            if elapsed_ms > slow_threshold {
                stats.slow_query_count += 1;
                stats.alerts.push(AlertInfo {
                    level: "warning".to_string(),
                    rule_id: "slow_query".to_string(),
                    message: format!(
                        "Query took {}ms (threshold {}ms): {}",
                        elapsed_ms,
                        slow_threshold,
                        truncate_sql(sql, 100)
                    ),
                    timestamp: now_ms,
                    value: elapsed_ms as f64,
                    threshold: slow_threshold as f64,
                });
            }
        }

        Ok(crate::mcp::QueryResult {
            columns: all_columns,
            rows: all_rows,
            affected_rows: total_affected,
            elapsed_ms,
        })
    }

    fn explain_query(&self, sql: &str) -> Result<ExplainPlan, McpError> {
        let stmts = szrsql_sql::parser::parse_sql(sql)
            .map_err(|e| McpError::BackendError(format!("parse error: {e:?}")))?;
        let stmt = stmts
            .into_iter()
            .next()
            .ok_or_else(|| McpError::BackendError("empty SQL".into()))?;
        let plan = {
            let catalog = self.catalog.borrow();
            szrsql_sql::plan::Planner::new(&*catalog)
                .plan_statement(stmt)
                .map_err(|e| McpError::BackendError(format!("plan error: {e}")))?
        };

        // P5.2: 使用 CostModel 基于 ANALYZE 收集的统计信息估算真实成本与行数
        // — 无统计信息时回退到默认值（DEFAULT_ROW_COUNT=1000 等）
        let (cost, rows) = {
            let stats_clone = self.stats_store.borrow().clone();
            let cost_model =
                szrsql_optimizer::cost::CostModel::new(std::sync::Arc::new(stats_clone));
            let estimated = cost_model.estimate(&plan);
            (estimated.total(), estimated.cardinality as u64)
        };

        let operators = format_plan_operators(&plan);
        Ok(ExplainPlan {
            sql: sql.to_string(),
            cost,
            rows,
            operators,
        })
    }

    fn prepare_statement(&self, name: &str, sql: &str) -> Result<PrepareResult, McpError> {
        let stmts = szrsql_sql::parser::parse_sql(sql)
            .map_err(|e| McpError::BackendError(format!("parse error: {e:?}")))?;
        if stmts.is_empty() {
            return Err(McpError::BackendError("empty SQL".into()));
        }
        // P3-Prepare：遍历 AST 收集所有 Expr::Parameter(idx)，返回最大索引（1-based）
        // 支持 PG 风格 $1/$2/... 和 ? 占位符（解析器将 ? 转为 Parameter(1)）
        let parameter_count = count_parameters(&stmts);
        Ok(PrepareResult {
            name: name.to_string(),
            parameter_count,
        })
    }

    fn cancel_query(&self, query_id: u64) -> Result<CancelResult, McpError> {
        // P3-Runtime：从活动查询中标记取消
        let mut active = self.active_queries.borrow_mut();
        if let Some(query) = active.get_mut(&query_id) {
            query.cancelled = true;
            // 异步场景下应通过 cancellation channel 通知执行器；
            // MCP 后端为同步执行，查询已结束则无法取消，仅标记状态
            Ok(CancelResult {
                query_id,
                cancelled: true,
            })
        } else {
            // 查询不存在或已结束
            Ok(CancelResult {
                query_id,
                cancelled: false,
            })
        }
    }

    // --- 类别 3-8: 复用 MVP 模式（返回空/Err） ---

    fn slow_queries(&self, limit: usize) -> Result<Vec<SlowQueryRecord>, McpError> {
        // P3-Runtime：从 query_history 中按耗时倒序取前 limit 条
        let stats = self.stats.borrow();
        let mut records: Vec<SlowQueryRecord> = stats
            .query_history
            .iter()
            .map(|r| SlowQueryRecord {
                sql: r.sql.clone(),
                elapsed_ms: r.elapsed_ms,
                timestamp: r.timestamp,
                rows_scanned: r.affected_rows,
                plan_operator: String::new(),
            })
            .collect();
        records.sort_by_key(|b| std::cmp::Reverse(b.elapsed_ms));
        records.truncate(limit);
        Ok(records)
    }
    fn top_queries(&self, limit: usize) -> Result<Vec<TopQueryRecord>, McpError> {
        // P3-Runtime：从 query_aggr 中按调用次数倒序取前 limit 条
        let stats = self.stats.borrow();
        let mut records: Vec<TopQueryRecord> = stats
            .query_aggr
            .iter()
            .map(|(sql, aggr)| TopQueryRecord {
                sql: sql.clone(),
                calls: aggr.count,
                total_time_ms: aggr.total_ms as f64,
                mean_time_ms: if aggr.count > 0 {
                    aggr.total_ms as f64 / aggr.count as f64
                } else {
                    0.0
                },
                rows: 0,
            })
            .collect();
        records.sort_by_key(|b| std::cmp::Reverse(b.calls));
        records.truncate(limit);
        Ok(records)
    }
    fn query_stats(&self) -> Result<QueryStatsSummary, McpError> {
        // P3-Runtime：从 stats 聚合真实统计
        let stats = self.stats.borrow();
        let total_queries: u64 = stats.query_aggr.values().map(|a| a.count).sum();
        let total_time_ms: f64 = stats.query_aggr.values().map(|a| a.total_ms).sum::<u64>() as f64;
        let unique_queries = stats.query_aggr.len();
        let avg_time_ms = if total_queries > 0 {
            total_time_ms / total_queries as f64
        } else {
            0.0
        };
        Ok(QueryStatsSummary {
            total_queries,
            total_time_ms,
            unique_queries,
            avg_time_ms,
        })
    }
    fn reset_stats(&self) -> Result<ResetResult, McpError> {
        // P3-Runtime：清空所有统计（保留 slow_query_threshold_ms 配置）
        let mut stats = self.stats.borrow_mut();
        let threshold = stats.slow_query_threshold_ms;
        *stats = RuntimeStats {
            slow_query_threshold_ms: threshold,
            ..Default::default()
        };
        stats.stats_reset = true;
        Ok(ResetResult { reset: true })
    }
    fn list_transactions(&self) -> Result<Vec<TransactionInfo>, McpError> {
        // P3-Runtime：返回 stats 中维护的活动事务（从 collect_runtime_events 采集）
        Ok(self.stats.borrow().active_transactions.clone())
    }
    fn list_locks(&self) -> Result<Vec<LockInfo>, McpError> {
        // P3-Runtime：返回 stats 中维护的活动锁（从 DML 操作采集）
        Ok(self.stats.borrow().active_locks.clone())
    }
    fn kill_transaction(&self, txn_id: u32) -> Result<KillResult, McpError> {
        // P3-Runtime：中止活动事务并释放其持有的锁
        // P3-Deadlock-Detection：通过 LockManager 释放该事务的所有锁
        let mut stats = self.stats.borrow_mut();
        let before = stats.active_transactions.len();
        stats.active_transactions.retain(|t| t.txn_id != txn_id);
        let killed = stats.active_transactions.len() < before;
        // 同时释放该事务持有的锁（stats + LockManager）
        stats.active_locks.retain(|l| l.txn_id != txn_id);
        drop(stats);
        // 释放 LockManager 中该事务持有的所有锁
        self.lock_mgr.unlock_all(txn_id);
        Ok(KillResult { txn_id, killed })
    }
    fn deadlock_history(&self) -> Result<Vec<DeadlockRecord>, McpError> {
        // P3-Runtime：返回死锁历史
        // 当前实现：死锁检测通过锁等待图，当检测到环时记录到 deadlock_history
        Ok(self.stats.borrow().deadlock_history.clone())
    }
    fn wait_events(&self) -> Result<Vec<WaitEventSummary>, McpError> {
        // P3-Runtime：从 wait_events 聚合返回真实等待事件
        let stats = self.stats.borrow();
        let result: Vec<WaitEventSummary> = stats
            .wait_events
            .iter()
            .map(|(event, aggr)| WaitEventSummary {
                event: event.clone(),
                total_waits: aggr.total_waits,
                total_wait_ms: aggr.total_wait_ms,
                avg_wait_ms: if aggr.total_waits > 0 {
                    aggr.total_wait_ms as f64 / aggr.total_waits as f64
                } else {
                    0.0
                },
            })
            .collect();
        Ok(result)
    }
    fn ash_report(&self, duration_secs: u64) -> Result<AshReport, McpError> {
        // P3-Runtime：基于 stats 生成 ASH 报告
        let stats = self.stats.borrow();
        let sample_count: usize = stats.query_history.len();
        // top_sql：按总耗时倒序取前 5，格式化为 "sql (total_ms=N)"
        let mut top_sql: Vec<(String, u64)> = stats
            .query_aggr
            .iter()
            .map(|(sql, a)| (sql.clone(), a.total_ms))
            .collect();
        top_sql.sort_by_key(|b| std::cmp::Reverse(b.1));
        top_sql.truncate(5);
        // top_wait_events：从 wait_events 按总等待时长倒序取前 5
        let mut top_wait: Vec<(String, u64)> = stats
            .wait_events
            .iter()
            .map(|(e, a)| (e.clone(), a.total_wait_ms))
            .collect();
        top_wait.sort_by_key(|b| std::cmp::Reverse(b.1));
        top_wait.truncate(5);
        Ok(AshReport {
            duration_secs,
            sample_count,
            top_sql: top_sql
                .into_iter()
                .map(|(sql, ms)| format!("{sql} (total_ms={ms})"))
                .collect(),
            top_wait_events: top_wait
                .into_iter()
                .map(|(e, ms)| format!("{e} (wait_ms={ms})"))
                .collect(),
        })
    }
    fn active_sessions(&self) -> Result<Vec<SessionInfo>, McpError> {
        // P3-Runtime：返回活动会话（从 ensure_session 采集）
        Ok(self.stats.borrow().active_sessions.clone())
    }
    fn pprof_dump(&self, duration_secs: u64) -> Result<PprofResult, McpError> {
        // P3-Runtime：基于 stats 生成 pprof 摘要
        let stats = self.stats.borrow();
        let sample_count: usize = stats.query_history.len();
        // top_functions：把 SQL 文本当作"函数"按调用次数倒序取前 5
        // 格式化为 "sql (calls=N, total_ms=M)"
        let mut top_functions: Vec<(String, u64, u64)> = stats
            .query_aggr
            .iter()
            .map(|(sql, a)| (sql.clone(), a.count, a.total_ms))
            .collect();
        top_functions.sort_by_key(|b| std::cmp::Reverse(b.1));
        top_functions.truncate(5);
        Ok(PprofResult {
            sample_count,
            duration_secs,
            top_functions: top_functions
                .into_iter()
                .map(|(name, calls, total_ms)| {
                    format!("{name} (calls={calls}, total_ms={total_ms})")
                })
                .collect(),
        })
    }
    fn vacuum_table(&self, table: &str) -> Result<VacuumResult, McpError> {
        // P3-Maintenance：清理死元组，重置 dead_tuples 计数
        let start = std::time::Instant::now();
        let key = Self::table_key(table);
        let (dead_reclaimed, last_vacuum_ms) = {
            let mut maint = self.maintenance.borrow_mut();
            let state = maint
                .get_mut(&key)
                .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))?;
            let dead = state.dead_tuples;
            state.dead_tuples = 0;
            state.last_vacuum_ms = Self::now_ms();
            state.vacuum_count += 1;
            (dead, state.last_vacuum_ms)
        };
        let elapsed_ms = start.elapsed().as_millis() as u64;
        // 更新全局 vacuum 统计
        {
            let mut stats = self.stats.borrow_mut();
            stats.total_vacuum_count += 1;
            stats.last_autovacuum_ms = last_vacuum_ms;
        }
        Ok(VacuumResult {
            table: table.to_string(),
            dead_tuples_reclaimed: dead_reclaimed,
            elapsed_ms,
        })
    }
    fn analyze_table(&self, table: &str) -> Result<AnalyzeResult, McpError> {
        // P3-Maintenance + P5.1：扫描表生成统计信息，更新 last_analyze_ms
        use szrsql_optimizer::statistics::{StatisticsCollector, StatisticsStore};
        use szrsql_sql::executor::TableStorage;
        use szrsql_sql::plan::Catalog;
        let start = std::time::Instant::now();
        // 获取表的列数和行数，并收集列级统计信息
        let (rows_analyzed, columns_analyzed, table_stats) = {
            let catalog = self.catalog.borrow();
            let schema = catalog
                .get_table(&self.find_table_name(table).unwrap_or_else(|| {
                    szrsql_sql::ast::TableName {
                        schema: None,
                        name: table.to_string(),
                    }
                }))
                .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))?;
            let cols = schema.columns.len();
            let tables = self.tables.borrow();
            let table_ref = tables
                .get(&Self::table_key(table))
                .ok_or_else(|| McpError::BackendError(format!("table data not found: {table}")))?;
            let rows = table_ref.row_count() as u64;
            // P5.1: 使用 StatisticsCollector 扫描全表，收集列级统计
            //（null_count / distinct_count / min/max / 等深直方图）
            let stats = StatisticsCollector::collect(table_ref as &dyn TableStorage);
            (rows, cols, stats)
        };
        // 将统计信息写入 stats_store，供 CostModel 在 explain_query 中使用
        {
            let mut store = self.stats_store.borrow_mut();
            let table_name = table_stats.table_name.clone();
            store.update_table_stats(&table_name, table_stats);
        }
        let _ = start.elapsed();
        let mut maint = self.maintenance.borrow_mut();
        let state = maint.entry(Self::table_key(table)).or_default();
        state.last_analyze_ms = Self::now_ms();
        state.analyze_count += 1;
        state.live_tuples = rows_analyzed;
        drop(maint);
        {
            let mut stats = self.stats.borrow_mut();
            stats.total_analyze_count += 1;
        }
        Ok(AnalyzeResult {
            table: table.to_string(),
            rows_analyzed,
            columns_analyzed,
        })
    }
    fn autovacuum_status(&self) -> Result<AutovacuumStatus, McpError> {
        // P3-Maintenance：返回 autovacuum 状态
        let stats = self.stats.borrow();
        let tables_vacuumed = stats.total_vacuum_count as usize;
        let tables_analyzed = stats.total_analyze_count as usize;
        Ok(AutovacuumStatus {
            enabled: true,
            last_run: stats.last_autovacuum_ms,
            tables_vacuumed,
            tables_analyzed,
        })
    }
    fn list_alerts(&self) -> Result<Vec<AlertInfo>, McpError> {
        // P3-Alerting：返回 stats 中采集的告警（慢查询/高错误率）
        Ok(self.stats.borrow().alerts.clone())
    }
    fn db_stats(&self) -> Result<crate::mcp::DbStats, McpError> {
        use szrsql_sql::executor::TableStorage;
        use szrsql_sql::plan::Catalog;
        let table_count = self.catalog.borrow().list_tables().len();
        let total_rows: u64 = self
            .tables
            .borrow()
            .values()
            .map(|t| t.row_count() as u64)
            .sum();
        let active_connections = self.stats.borrow().active_sessions.len() as u32;
        Ok(crate::mcp::DbStats {
            table_count,
            total_rows,
            total_size_bytes: total_rows * 64,
            cache_hit_rate: 0.0,
            active_connections,
        })
    }
    fn capacity_predict(&self, days: u32) -> Result<CapacityForecast, McpError> {
        // P3-Capacity-Advanced：按表独立增长率 + 考虑 UPDATE 行数
        //
        // 改进点（相对于 P3-Capacity-Enhanced）：
        // 1. 从 SQL 文本解析表名，按表独立计算 INSERT/DELETE/UPDATE 行数
        // 2. 每张表独立计算净增长率（不再全局均分）
        // 3. UPDATE 行数计入存储增长（UPDATE 不改变行数但产生 dead_tuples）
        // 4. 全局增长率 = 各表增长率之和
        use szrsql_sql::executor::TableStorage;

        let stats = self.stats.borrow();

        // 1. 过滤 DML 记录（INSERT/DELETE/UPDATE）并解析表名
        //
        // 改进：span_days 只基于 DML 记录，避免 CREATE/SELECT 记录时间戳污染。
        /// 从 SQL 文本中提取表名（INSERT INTO t / DELETE FROM t / UPDATE t SET）
        fn parse_table_name(sql: &str) -> Option<String> {
            let upper = sql.to_uppercase();
            if upper.starts_with("INSERT") {
                // INSERT INTO table_name ... / INSERT INTO "table_name" ...
                let after_insert = &sql["INSERT".len()..];
                let after_into = after_insert.to_uppercase().find("INTO")?;
                let rest = after_insert[after_into + 4..].trim_start();
                extract_identifier(rest)
            } else if upper.starts_with("DELETE") {
                // DELETE FROM table_name ...
                let after_delete = &sql["DELETE".len()..];
                let after_from = after_delete.to_uppercase().find("FROM")?;
                let rest = after_delete[after_from + 4..].trim_start();
                extract_identifier(rest)
            } else if upper.starts_with("UPDATE") {
                // UPDATE table_name SET ...
                let rest = sql["UPDATE".len()..].trim_start();
                extract_identifier(rest)
            } else {
                None
            }
        }

        /// 从字符串开头提取标识符（支持双引号和普通标识符）
        fn extract_identifier(s: &str) -> Option<String> {
            let s = s.trim_start();
            if let Some(rest) = s.strip_prefix('"') {
                // 带引号的标识符
                let end = rest.find('"')?;
                Some(rest[..end].to_lowercase())
            } else {
                // 普通标识符：字母/数字/下划线
                let end = s
                    .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
                    .unwrap_or(s.len());
                if end > 0 {
                    Some(s[..end].to_lowercase())
                } else {
                    None
                }
            }
        }

        // 收集 DML 记录：(操作类型, 表名, affected_rows, timestamp)
        #[derive(Clone, Copy)]
        enum DmlKind {
            Insert,
            Delete,
            Update,
        }

        let mut dml_records: Vec<(DmlKind, String, u64, u64)> = Vec::new();
        for r in &stats.query_history {
            let sql_upper = r.sql.to_uppercase();
            let kind = if sql_upper.starts_with("INSERT") {
                DmlKind::Insert
            } else if sql_upper.starts_with("DELETE") {
                DmlKind::Delete
            } else if sql_upper.starts_with("UPDATE") {
                DmlKind::Update
            } else {
                continue;
            };
            // 解析表名，解析失败时用空字符串（仍计入全局统计）
            let table = parse_table_name(&r.sql).unwrap_or_default();
            dml_records.push((kind, table, r.affected_rows, r.timestamp));
        }

        if dml_records.is_empty() || days == 0 {
            return Ok(CapacityForecast {
                metric: "total_rows".to_string(),
                current_value: 0.0,
                predicted_value: 0.0,
                days_ahead: days,
                confidence: 0.0,
                storage_bytes_current: None,
                storage_bytes_predicted: None,
                net_growth_rate_per_day: None,
                table_breakdown: None,
            });
        }

        // 2. 计算时间跨度
        let first_ts = dml_records.first().map(|r| r.3).unwrap_or(0);
        let last_ts = dml_records.last().map(|r| r.3).unwrap_or(0);
        let span_ms = last_ts.saturating_sub(first_ts);
        let span_days = (span_ms as f64) / 86_400_000.0;

        // 3. 按表聚合 DML 行数
        // table_stats: table_name → (inserts, deletes, updates)
        let mut table_dml: std::collections::HashMap<String, (u64, u64, u64)> =
            std::collections::HashMap::new();
        let mut total_inserts: u64 = 0;
        let mut total_deletes: u64 = 0;
        let mut total_updates: u64 = 0;

        for (kind, table, affected, _) in &dml_records {
            let entry = table_dml.entry(table.clone()).or_insert((0, 0, 0));
            match kind {
                DmlKind::Insert => {
                    entry.0 = entry.0.saturating_add(*affected);
                    total_inserts = total_inserts.saturating_add(*affected);
                }
                DmlKind::Delete => {
                    entry.1 = entry.1.saturating_add(*affected);
                    total_deletes = total_deletes.saturating_add(*affected);
                }
                DmlKind::Update => {
                    entry.2 = entry.2.saturating_add(*affected);
                    total_updates = total_updates.saturating_add(*affected);
                }
            }
        }

        // 4. 全局净增长率（行数/天）= (INSERT - DELETE) / span_days
        //
        // span_days = 0 时用 DML 记录数作为分母避免除零。
        let net_growth_rate = if span_days > 0.0 {
            (total_inserts as f64 - total_deletes as f64) / span_days
        } else if !dml_records.is_empty() {
            (total_inserts as f64 - total_deletes as f64) / dml_records.len() as f64
        } else {
            0.0
        };

        // 5. 按表分解预测（每张表独立计算增长率）
        const AVG_ROW_BYTES: f64 = 100.0;
        // UPDATE 产生的 dead_tuples 存储开销系数（每次 UPDATE 产生约 0.5 行的 dead tuple）
        const UPDATE_DEAD_TUPLE_RATIO: f64 = 0.5;

        let tables = self.tables.borrow();
        let maintenance = self.maintenance.borrow();
        let mut table_breakdown: Vec<TableForecast> = Vec::new();
        let mut total_current_rows: f64 = 0.0;
        let mut total_current_bytes: f64 = 0.0;
        let mut total_predicted_bytes: f64 = 0.0;

        // 计算每张表的增长率
        let compute_table_growth = |inserts: u64, deletes: u64| -> f64 {
            let net = inserts as f64 - deletes as f64;
            if span_days > 0.0 {
                net / span_days
            } else if !dml_records.is_empty() {
                net / dml_records.len() as f64
            } else {
                0.0
            }
        };

        for (table_name, table) in tables.iter() {
            let current_rows = table.row_count() as f64;
            let dead_tuples = maintenance
                .get(table_name)
                .map(|m| m.dead_tuples as f64)
                .unwrap_or(0.0);
            let current_bytes = (current_rows + dead_tuples) * AVG_ROW_BYTES;

            // 按表独立增长率（P3-Capacity-Advanced 核心改进）
            let (ins, del, upd) = table_dml.get(table_name).copied().unwrap_or((0, 0, 0));
            let table_growth = compute_table_growth(ins, del);
            let predicted_rows = (current_rows + table_growth * days as f64).max(0.0);

            // UPDATE 产生的 dead_tuples 存储开销
            let update_dead_tuples = (upd as f64) * UPDATE_DEAD_TUPLE_RATIO;
            let update_bytes = update_dead_tuples * AVG_ROW_BYTES;
            let predicted_bytes = (predicted_rows + dead_tuples) * AVG_ROW_BYTES + update_bytes;

            table_breakdown.push(TableForecast {
                table: table_name.clone(),
                current_rows,
                predicted_rows,
                current_bytes,
                predicted_bytes,
                growth_rate_per_day: table_growth,
            });

            total_current_rows += current_rows;
            total_current_bytes += current_bytes;
            total_predicted_bytes += predicted_bytes;
        }

        // 6. 全局预测
        let predicted_rows = (total_current_rows + net_growth_rate * days as f64).max(0.0);
        // 全局存储预测：用 total_predicted_bytes（已含 UPDATE 开销）
        let predicted_bytes = total_predicted_bytes;

        // 7. 置信度：样本数 0.7 + 时间跨度 0.3
        let dml_sample_count = dml_records.len() as f64;
        let sample_score = (dml_sample_count / 50.0).min(0.7);
        let span_score = (span_days / 7.0).min(0.3);
        let confidence = (sample_score + span_score).min(1.0);

        Ok(CapacityForecast {
            metric: "total_rows".to_string(),
            current_value: total_current_rows,
            predicted_value: predicted_rows,
            days_ahead: days,
            confidence,
            storage_bytes_current: Some(total_current_bytes),
            storage_bytes_predicted: Some(predicted_bytes),
            net_growth_rate_per_day: Some(net_growth_rate),
            table_breakdown: Some(table_breakdown),
        })
    }
    fn summarize_table(&self, table: &str) -> Result<TableSummary, McpError> {
        use std::collections::HashMap;
        use szrsql_sql::executor::Row;
        use szrsql_sql::executor::TableStorage;
        use szrsql_types::value::Value as DbValue;

        let key = Self::table_key(table);
        let tables = self.tables.borrow();
        let target = tables
            .get(&key)
            .ok_or_else(|| McpError::BackendError(format!("table not found: {table}")))?;

        let schema = target.schema();
        let row_count = target.row_count() as u64;

        // 收集每列的统计信息
        let mut columns_summary: Vec<ColumnSummary> = Vec::with_capacity(schema.columns.len());

        // 按列索引收集所有值（用于 distinct / min / max / top_values）
        let rows: Vec<Row> = target.scan_iter().collect();

        for col in &schema.columns {
            // 找到列索引
            let col_idx = schema
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(&col.name))
                .unwrap_or(0);

            let mut null_count: u64 = 0;
            let mut distinct_set: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut value_counts: HashMap<String, u64> = HashMap::new();
            let mut min_str: Option<String> = None;
            let mut max_str: Option<String> = None;

            for row in &rows {
                let v = row.get(col_idx).cloned().unwrap_or(DbValue::Null);
                match v {
                    DbValue::Null => {
                        null_count += 1;
                    }
                    DbValue::Int64(n) => {
                        let s = n.to_string();
                        distinct_set.insert(s.clone());
                        *value_counts.entry(s.clone()).or_insert(0) += 1;
                        min_str = Some(match min_str.clone() {
                            Some(cur) => cur.min(s.clone()),
                            None => s.clone(),
                        });
                        max_str = Some(match max_str.clone() {
                            Some(cur) => cur.max(s.clone()),
                            None => s,
                        });
                    }
                    DbValue::Float64(f) => {
                        let s = format!("{f}");
                        distinct_set.insert(s.clone());
                        *value_counts.entry(s.clone()).or_insert(0) += 1;
                        min_str = Some(match min_str.clone() {
                            Some(cur) if cur.parse::<f64>().ok() <= Some(f) => cur,
                            _ => format!("{f}"),
                        });
                        max_str = Some(match max_str.clone() {
                            Some(cur) if cur.parse::<f64>().ok() >= Some(f) => cur,
                            _ => format!("{f}"),
                        });
                    }
                    DbValue::Decimal(unscaled, scale) => {
                        let f = (unscaled as f64) / 10f64.powi(scale as i32);
                        let s = format!("{f}");
                        distinct_set.insert(s.clone());
                        *value_counts.entry(s.clone()).or_insert(0) += 1;
                        min_str = Some(match min_str.clone() {
                            Some(cur) if cur.parse::<f64>().ok() <= Some(f) => cur,
                            _ => format!("{f}"),
                        });
                        max_str = Some(match max_str.clone() {
                            Some(cur) if cur.parse::<f64>().ok() >= Some(f) => cur,
                            _ => format!("{f}"),
                        });
                    }
                    DbValue::Text(s) => {
                        distinct_set.insert(s.clone());
                        *value_counts.entry(s.clone()).or_insert(0) += 1;
                        min_str = Some(match min_str.clone() {
                            Some(cur) => cur.min(s.clone()),
                            None => s.clone(),
                        });
                        max_str = Some(match max_str.clone() {
                            Some(cur) => cur.max(s.clone()),
                            None => s,
                        });
                    }
                    DbValue::Bool(b) => {
                        let s = b.to_string();
                        distinct_set.insert(s.clone());
                        *value_counts.entry(s.clone()).or_insert(0) += 1;
                    }
                    DbValue::Date(days) => {
                        let s = days.to_string();
                        distinct_set.insert(s.clone());
                        *value_counts.entry(s.clone()).or_insert(0) += 1;
                        min_str = Some(match min_str.clone() {
                            Some(cur) => cur.min(s.clone()),
                            None => s.clone(),
                        });
                        max_str = Some(match max_str.clone() {
                            Some(cur) => cur.max(s.clone()),
                            None => s,
                        });
                    }
                    DbValue::Timestamp(us) => {
                        let s = us.to_string();
                        distinct_set.insert(s.clone());
                        *value_counts.entry(s.clone()).or_insert(0) += 1;
                        min_str = Some(match min_str.clone() {
                            Some(cur) => cur.min(s.clone()),
                            None => s.clone(),
                        });
                        max_str = Some(match max_str.clone() {
                            Some(cur) => cur.max(s.clone()),
                            None => s,
                        });
                    }
                    _ => {
                        // 其他类型（Blob/Array/Enum/Json/Range/TsVector/TsQuery）仅计入 distinct
                        let s = format!("{v:?}");
                        distinct_set.insert(s.clone());
                        *value_counts.entry(s.clone()).or_insert(0) += 1;
                    }
                }
            }

            // top_values: 取出现次数最多的前 5 个值
            let mut top_values: Vec<(String, u64)> = value_counts.into_iter().collect();
            top_values.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            top_values.truncate(5);

            // 类型字符串映射
            let data_type = column_type_to_string(&col.data_type);

            columns_summary.push(ColumnSummary {
                name: col.name.clone(),
                data_type,
                null_count,
                distinct_count: distinct_set.len() as u64,
                min_value: min_str,
                max_value: max_str,
                top_values,
            });
        }

        Ok(TableSummary {
            table: table.to_string(),
            row_count,
            columns: columns_summary,
        })
    }
    fn ask_data(&self, question: &str) -> Result<AskAnswer, McpError> {
        use crate::nl2sql::Nl2SqlEngine;
        use szrsql_catalog::semantic_tag::parse_comment;
        use szrsql_sql::plan::Catalog;

        // P3-LLM-Enhanced：查询预处理 — 同义词替换 + 聚合意图增强
        //
        // 1. 从 catalog 的 COMMENT ON 中解析 SemanticTag.synonyms
        // 2. 将用户问题中的同义词替换为标准列名/表名
        // 3. 增强聚合意图识别（支持更多中文表达）

        /// 从 catalog 加载同义词映射：synonym → standard_name
        fn load_synonyms(catalog: &szrsql_sql::plan::InMemoryCatalog) -> Vec<(String, String)> {
            let mut synonyms: Vec<(String, String)> = Vec::new();
            for table_name in catalog.list_tables() {
                if let Some(schema) = catalog.get_table(&table_name) {
                    // 加载列注释中的同义词
                    for col in &schema.columns {
                        if let Some(comment) = catalog.get_column_comment(&table_name, &col.name) {
                            if let Some(tag) = parse_comment(Some(&comment)) {
                                for syn in tag.synonyms {
                                    synonyms.push((syn, col.name.clone()));
                                }
                            }
                        }
                    }
                }
            }
            synonyms
        }

        /// 将用户问题中的同义词替换为标准列名
        fn apply_synonyms(question: &str, synonyms: &[(String, String)]) -> String {
            let mut result = question.to_string();
            for (synonym, standard) in synonyms {
                // 大小写不敏感替换
                let lower_result = result.to_lowercase();
                let lower_syn = synonym.to_lowercase();
                if lower_result.contains(&lower_syn) {
                    result = lower_result.replace(&lower_syn, standard);
                }
            }
            result
        }

        /// P3-LLM-Enhanced：增强聚合意图识别
        ///
        /// 在 Nl2SqlEngine 之前预处理，将口语化聚合表达规范化
        fn enhance_aggregation_intent(question: &str) -> String {
            let mut result = question.to_string();
            // "一共有多少" → "多少"（避免 Nl2SqlEngine 误匹配"一共"）
            // "算一下平均" → "平均"
            // "总和是多少" → "总和"
            // "最大值" → "最大"
            // "最小值" → "最小"
            // "平均值" → "平均"
            let replacements: &[(&str, &str)] = &[
                ("一共有多少", "多少"),
                ("总共有多少", "多少"),
                ("算一下平均", "平均"),
                ("算下平均", "平均"),
                ("计算平均", "平均"),
                ("平均值", "平均"),
                ("最大值", "最大"),
                ("最小值", "最小"),
                ("总和是多少", "总和"),
                ("总计多少", "总和"),
            ];
            for (from, to) in replacements {
                result = result.replace(from, to);
            }
            result
        }

        // 1. 构造 Nl2SqlEngine 并注册所有表
        let mut engine = Nl2SqlEngine::new();
        {
            let catalog = self.catalog.borrow();
            for table_name in catalog.list_tables() {
                if let Some(schema) = catalog.get_table(&table_name) {
                    let cols: Vec<crate::nl2sql::ColumnDef> = schema
                        .columns
                        .iter()
                        .map(|c| crate::nl2sql::ColumnDef {
                            name: c.name.clone(),
                            data_type: column_type_to_coltype(&c.data_type),
                        })
                        .collect();
                    engine.register_table(&table_name.name, cols);
                }
            }

            // P3-LLM-Enhanced：加载同义词并预处理问题
            let synonyms = load_synonyms(&catalog);
            drop(catalog);

            // 2. 自然语言 → SQL（带预处理）
            let enhanced_question = enhance_aggregation_intent(question);
            let final_question = if synonyms.is_empty() {
                enhanced_question
            } else {
                apply_synonyms(&enhanced_question, &synonyms)
            };

            let sql = engine
                .translate(&final_question)
                .map_err(|e| McpError::BackendError(format!("nl2sql error: {e}")))?;

            // 3. 执行 SQL
            let result = self.execute_sql(&sql)?;

            // 4. 生成自然语言回答
            let answer = if result.rows.is_empty() {
                format!("查询无结果。SQL: {sql}")
            } else if result.rows.len() == 1 && result.rows[0].len() == 1 {
                // 标量结果（如 COUNT(*)）
                format!("{} = {}", result.columns[0], result.rows[0][0])
            } else {
                // 多行结果，显示前 10 行
                let mut lines = Vec::new();
                lines.push(format!("找到 {} 行", result.rows.len()));
                lines.push(format!("列: {}", result.columns.join(", ")));
                let show = result.rows.len().min(10);
                for (i, row) in result.rows.iter().take(show).enumerate() {
                    let cells: Vec<String> = row.iter().map(|v| v.to_string()).collect();
                    lines.push(format!("  行 {}: {}", i + 1, cells.join(" | ")));
                }
                if result.rows.len() > 10 {
                    lines.push(format!("  ... (省略 {} 行)", result.rows.len() - 10));
                }
                lines.join("\n")
            };

            // 5. 生成引用（取前 3 行作为数据来源追溯）
            let citations: Vec<AskCitation> = result
                .rows
                .iter()
                .take(3)
                .enumerate()
                .map(|(i, row)| {
                    let cells: Vec<String> = row.iter().map(|v| v.to_string()).collect();
                    AskCitation {
                        table: sql.clone(),
                        row_id: i as u64,
                        snippet: cells.join(" | "),
                        score: 1.0,
                    }
                })
                .collect();

            Ok(AskAnswer {
                answer,
                sql: Some(sql),
                citations,
            })
        }
    }
    fn explain_root_cause(&self, alert_id: &str) -> Result<RootCauseReport, McpError> {
        // P3-Insight：基于 alerts + slow_queries + wait_events + deadlock_history 综合分析根因
        // alert_id 按 rule_id 匹配（与 MockBackend 保持一致）
        let stats = self.stats.borrow();
        let alert = stats
            .alerts
            .iter()
            .find(|a| a.rule_id == alert_id)
            .ok_or_else(|| McpError::BackendError(format!("alert not found: {alert_id}")))?
            .clone();

        // 根因推理规则：基于 rule_id + 关联 slow_queries + wait_events + deadlock_history
        let (mut causes, evidence) = match alert.rule_id.as_str() {
            "slow_query" => {
                // 慢查询告警：可能是缺失索引、锁竞争或统计信息过期
                let mut causes: Vec<CauseEntry> = Vec::new();
                let mut evidence: Vec<Evidence> = Vec::new();

                // 证据 1：告警本身
                evidence.push(Evidence {
                    source: "alert".to_string(),
                    detail: format!(
                        "elapsed_ms={:.0}, threshold={:.0}, message={}",
                        alert.value, alert.threshold, alert.message
                    ),
                });

                // 证据 2：关联慢查询历史（取最慢的一条）
                let mut slow_sorted: Vec<&QueryRecord> = stats.query_history.iter().collect();
                slow_sorted.sort_by_key(|r| std::cmp::Reverse(r.elapsed_ms));
                if let Some(sq) = slow_sorted.first() {
                    // 若该查询受影响行数大，推断为全表扫描/缺失索引
                    if sq.affected_rows > 1000 || sq.elapsed_ms as f64 > alert.threshold * 2.0 {
                        causes.push(CauseEntry {
                            cause_type: CauseType::MissingIndex,
                            description: format!(
                                "最慢查询耗时 {}ms，受影响行数 {}，疑似全表扫描，建议为 WHERE 条件列添加索引",
                                sq.elapsed_ms, sq.affected_rows
                            ),
                            confidence: 0.8,
                        });
                    }
                    evidence.push(Evidence {
                        source: "slow_query".to_string(),
                        detail: format!(
                            "sql={}, elapsed_ms={}, affected_rows={}",
                            truncate_sql(&sq.sql, 80),
                            sq.elapsed_ms,
                            sq.affected_rows
                        ),
                    });
                }

                // 证据 3：关联等待事件，若存在锁等待则推断锁竞争
                let has_lock_wait = stats
                    .wait_events
                    .keys()
                    .any(|e| e.contains("lock") || e.contains("Lock"));
                if has_lock_wait {
                    causes.push(CauseEntry {
                        cause_type: CauseType::LockContention,
                        description: "等待事件中存在锁等待，可能存在锁竞争导致慢查询".to_string(),
                        confidence: 0.6,
                    });
                    let lock_events: Vec<String> = stats
                        .wait_events
                        .iter()
                        .filter(|(k, _)| k.contains("lock") || k.contains("Lock"))
                        .map(|(k, v)| {
                            format!("{k}(waits={},ms={})", v.total_waits, v.total_wait_ms)
                        })
                        .collect();
                    evidence.push(Evidence {
                        source: "wait_events".to_string(),
                        detail: lock_events.join(", "),
                    });
                }

                // P3-RootCause-Enhanced：证据链增强 — 关联活动事务和活动锁
                // 证据 4：关联有 wait_event 的活动事务
                let waiting_txns: Vec<String> = stats
                    .active_transactions
                    .iter()
                    .filter_map(|t| {
                        t.wait_event
                            .as_ref()
                            .map(|w| format!("txn={} state={} wait={}", t.txn_id, t.state, w))
                    })
                    .collect();
                if !waiting_txns.is_empty() {
                    evidence.push(Evidence {
                        source: "active_transactions".to_string(),
                        detail: waiting_txns.join("; "),
                    });
                }

                // 证据 5：关联未授予的活动锁（等待中的锁）
                let pending_locks: Vec<String> = stats
                    .active_locks
                    .iter()
                    .filter(|l| !l.granted)
                    .map(|l| {
                        format!(
                            "txn={} table={} mode={} wait_start={:?}",
                            l.txn_id, l.table, l.mode, l.wait_start
                        )
                    })
                    .collect();
                if !pending_locks.is_empty() {
                    evidence.push(Evidence {
                        source: "active_locks_pending".to_string(),
                        detail: pending_locks.join("; "),
                    });
                }

                // 若无明确根因，回退到统计信息过期
                if causes.is_empty() {
                    causes.push(CauseEntry {
                        cause_type: CauseType::StatsStale,
                        description: "未发现明显性能瓶颈，统计信息可能过期，建议执行 ANALYZE"
                            .to_string(),
                        confidence: 0.4,
                    });
                }
                (causes, evidence)
            }
            "high_error_rate" => {
                // 高错误率告警：可能是统计信息过期或 SQL 语法问题
                let mut causes: Vec<CauseEntry> = Vec::new();
                let mut evidence: Vec<Evidence> = Vec::new();

                evidence.push(Evidence {
                    source: "alert".to_string(),
                    detail: format!(
                        "error_count={:.0}, threshold={:.0}, message={}",
                        alert.value, alert.threshold, alert.message
                    ),
                });

                // 错误率高的常见根因：统计信息过期导致优化器选错计划
                causes.push(CauseEntry {
                    cause_type: CauseType::StatsStale,
                    description: format!(
                        "累计错误查询 {} 次，建议执行 ANALYZE 更新统计信息并检查 SQL 语法",
                        stats.error_query_count
                    ),
                    confidence: 0.5,
                });

                // 关联最近的失败查询作为证据
                let failed_count = stats.error_query_count;
                evidence.push(Evidence {
                    source: "query_stats".to_string(),
                    detail: format!(
                        "total_errors={}, slow_queries={}",
                        failed_count, stats.slow_query_count
                    ),
                });

                (causes, evidence)
            }
            "deadlock" => {
                // 死锁告警：检查死锁历史
                let mut causes: Vec<CauseEntry> = Vec::new();
                let mut evidence: Vec<Evidence> = Vec::new();

                causes.push(CauseEntry {
                    cause_type: CauseType::Deadlock,
                    description: "检测到死锁，建议检查事务锁获取顺序以避免环路".to_string(),
                    confidence: 0.9,
                });

                // 关联死锁历史作为证据
                for dl in &stats.deadlock_history {
                    evidence.push(Evidence {
                        source: "deadlock_history".to_string(),
                        detail: format!(
                            "txn_ids={:?} resource={} timestamp={}",
                            dl.txn_ids, dl.resource, dl.timestamp
                        ),
                    });
                }

                // P3-RootCause-Enhanced：证据链增强 — 关联等待事件和活动事务
                // 证据：锁等待事件统计
                let lock_events: Vec<String> = stats
                    .wait_events
                    .iter()
                    .filter(|(k, _)| k.contains("lock") || k.contains("Lock"))
                    .map(|(k, v)| format!("{k}(waits={},ms={})", v.total_waits, v.total_wait_ms))
                    .collect();
                if !lock_events.is_empty() {
                    evidence.push(Evidence {
                        source: "wait_events".to_string(),
                        detail: lock_events.join(", "),
                    });
                }

                // 证据：参与死锁的事务当前状态
                let dl_txn_ids: std::collections::HashSet<u32> = stats
                    .deadlock_history
                    .iter()
                    .flat_map(|dl| dl.txn_ids.iter().copied())
                    .collect();
                let dl_txns: Vec<String> = stats
                    .active_transactions
                    .iter()
                    .filter(|t| dl_txn_ids.contains(&t.txn_id))
                    .map(|t| {
                        format!(
                            "txn={} state={} sql={}",
                            t.txn_id,
                            t.state,
                            truncate_sql(&t.sql, 60)
                        )
                    })
                    .collect();
                if !dl_txns.is_empty() {
                    evidence.push(Evidence {
                        source: "active_transactions".to_string(),
                        detail: dl_txns.join("; "),
                    });
                }

                if evidence.is_empty() {
                    evidence.push(Evidence {
                        source: "alert".to_string(),
                        detail: format!("rule_id={}, message={}", alert.rule_id, alert.message),
                    });
                }

                (causes, evidence)
            }
            "high_qps" => {
                // 高 QPS 告警：可能是突发流量或缺失索引放大
                let mut causes: Vec<CauseEntry> = Vec::new();
                let mut evidence: Vec<Evidence> = Vec::new();

                causes.push(CauseEntry {
                    cause_type: CauseType::HighQps,
                    description: "QPS 超过阈值，可能由突发流量或缺失索引导致全表扫描放大"
                        .to_string(),
                    confidence: 0.7,
                });
                evidence.push(Evidence {
                    source: "alert".to_string(),
                    detail: format!("value={:.0}, threshold={:.0}", alert.value, alert.threshold),
                });

                // 关联慢查询作为证据
                if let Some(sq) = stats.query_history.iter().max_by_key(|r| r.elapsed_ms) {
                    if sq.elapsed_ms > stats.slow_query_threshold_ms {
                        causes.push(CauseEntry {
                            cause_type: CauseType::MissingIndex,
                            description: format!(
                                "慢查询耗时 {}ms，建议添加索引降低单次查询开销",
                                sq.elapsed_ms
                            ),
                            confidence: 0.75,
                        });
                    }
                    evidence.push(Evidence {
                        source: "slow_query".to_string(),
                        detail: format!(
                            "sql={}, elapsed_ms={}",
                            truncate_sql(&sq.sql, 80),
                            sq.elapsed_ms
                        ),
                    });
                }

                // P3-RootCause-Enhanced：证据链增强 — 关联查询聚合 Top N
                // 证据：QPS 最高的 Top 3 SQL
                let mut aggr_sorted: Vec<(&String, &QueryAggr)> = stats.query_aggr.iter().collect();
                aggr_sorted.sort_by_key(|(_, a)| std::cmp::Reverse(a.count));
                let top_qps_sqls: Vec<String> = aggr_sorted
                    .iter()
                    .take(3)
                    .map(|(sql, a)| {
                        format!(
                            "sql={} count={} total_ms={} max_ms={}",
                            truncate_sql(sql, 60),
                            a.count,
                            a.total_ms,
                            a.max_ms
                        )
                    })
                    .collect();
                if !top_qps_sqls.is_empty() {
                    evidence.push(Evidence {
                        source: "query_aggr_top_qps".to_string(),
                        detail: top_qps_sqls.join("; "),
                    });
                }

                (causes, evidence)
            }
            "lock_wait" => {
                // P3-RootCause-Enhanced：锁等待告警 — 专门分析锁竞争根因
                //
                // 触发场景：等待事件中锁等待次数/时长超过阈值
                // 根因推断：
                // 1. LockContention — 锁竞争（主要根因，高置信度）
                // 2. Deadlock — 若 deadlock_history 非空，可能已演化为死锁
                // 3. MissingIndex — 长事务持锁可能因全表扫描慢导致
                let mut causes: Vec<CauseEntry> = Vec::new();
                let mut evidence: Vec<Evidence> = Vec::new();

                // 证据 1：告警本身
                evidence.push(Evidence {
                    source: "alert".to_string(),
                    detail: format!(
                        "value={:.0}, threshold={:.0}, message={}",
                        alert.value, alert.threshold, alert.message
                    ),
                });

                // 证据 2：锁等待事件详细统计
                let lock_events: Vec<String> = stats
                    .wait_events
                    .iter()
                    .filter(|(k, _)| k.contains("lock") || k.contains("Lock"))
                    .map(|(k, v)| {
                        let avg_ms = if v.total_waits > 0 {
                            v.total_wait_ms as f64 / v.total_waits as f64
                        } else {
                            0.0
                        };
                        format!(
                            "{k}(waits={},total_ms={},avg_ms={:.1})",
                            v.total_waits, v.total_wait_ms, avg_ms
                        )
                    })
                    .collect();
                if !lock_events.is_empty() {
                    evidence.push(Evidence {
                        source: "wait_events_lock".to_string(),
                        detail: lock_events.join(", "),
                    });
                }

                // 根因 1：锁竞争（主根因）
                causes.push(CauseEntry {
                    cause_type: CauseType::LockContention,
                    description: format!(
                        "锁等待事件超过阈值，存在严重锁竞争；建议检查长事务和锁获取顺序，考虑使用低隔离级别或乐观并发控制（证据：{} 条锁等待事件）",
                        stats
                            .wait_events
                            .iter()
                            .filter(|(k, _)| k.contains("lock") || k.contains("Lock"))
                            .count()
                    ),
                    confidence: 0.85,
                });

                // 根因 2：若存在死锁历史，可能已演化为死锁
                if !stats.deadlock_history.is_empty() {
                    causes.push(CauseEntry {
                        cause_type: CauseType::Deadlock,
                        description: format!(
                            "死锁历史中存在 {} 条记录，锁竞争可能已演化为死锁；建议重构事务按固定顺序获取锁",
                            stats.deadlock_history.len()
                        ),
                        confidence: 0.8,
                    });
                    // 证据：死锁历史
                    let dl_summary: Vec<String> = stats
                        .deadlock_history
                        .iter()
                        .map(|dl| format!("txn_ids={:?} resource={}", dl.txn_ids, dl.resource))
                        .collect();
                    evidence.push(Evidence {
                        source: "deadlock_history".to_string(),
                        detail: dl_summary.join("; "),
                    });
                }

                // 证据 3：未授予的活动锁（正在等待的锁）
                let pending_locks: Vec<String> = stats
                    .active_locks
                    .iter()
                    .filter(|l| !l.granted)
                    .map(|l| {
                        format!(
                            "txn={} table={} mode={} wait_start={:?}",
                            l.txn_id, l.table, l.mode, l.wait_start
                        )
                    })
                    .collect();
                if !pending_locks.is_empty() {
                    evidence.push(Evidence {
                        source: "active_locks_pending".to_string(),
                        detail: pending_locks.join("; "),
                    });
                }

                // 证据 4：有 wait_event 的活动事务
                let waiting_txns: Vec<String> = stats
                    .active_transactions
                    .iter()
                    .filter_map(|t| {
                        t.wait_event.as_ref().map(|w| {
                            format!(
                                "txn={} state={} wait={} sql={}",
                                t.txn_id,
                                t.state,
                                w,
                                truncate_sql(&t.sql, 60)
                            )
                        })
                    })
                    .collect();
                if !waiting_txns.is_empty() {
                    evidence.push(Evidence {
                        source: "active_transactions_waiting".to_string(),
                        detail: waiting_txns.join("; "),
                    });
                }

                // 根因 3：长事务持锁（若存在长耗时查询且活动事务有锁等待）
                if let Some(sq) = stats.query_history.iter().max_by_key(|r| r.elapsed_ms) {
                    if sq.elapsed_ms > stats.slow_query_threshold_ms {
                        causes.push(CauseEntry {
                            cause_type: CauseType::MissingIndex,
                            description: format!(
                                "长耗时查询 ({}ms) 可能因全表扫描导致长时间持锁，建议添加索引加速查询以缩短锁持有时间",
                                sq.elapsed_ms
                            ),
                            confidence: 0.65,
                        });
                        evidence.push(Evidence {
                            source: "slow_query".to_string(),
                            detail: format!(
                                "sql={}, elapsed_ms={}, affected_rows={}",
                                truncate_sql(&sq.sql, 80),
                                sq.elapsed_ms,
                                sq.affected_rows
                            ),
                        });
                    }
                }

                (causes, evidence)
            }
            "full_table_scan" | "timeout" => {
                // 全表扫描/超时告警：推断缺失索引或锁竞争
                let mut causes: Vec<CauseEntry> = Vec::new();
                let mut evidence: Vec<Evidence> = Vec::new();

                evidence.push(Evidence {
                    source: "alert".to_string(),
                    detail: format!("rule_id={}, message={}", alert.rule_id, alert.message),
                });

                // 检查是否有锁等待
                let has_lock_wait = stats
                    .wait_events
                    .keys()
                    .any(|e| e.contains("lock") || e.contains("Lock"));
                if has_lock_wait {
                    causes.push(CauseEntry {
                        cause_type: CauseType::LockContention,
                        description: "等待事件中锁等待占比高，存在锁竞争".to_string(),
                        confidence: 0.6,
                    });
                    // P3-RootCause-Enhanced：证据链增强 — 锁等待事件详情
                    let lock_events: Vec<String> = stats
                        .wait_events
                        .iter()
                        .filter(|(k, _)| k.contains("lock") || k.contains("Lock"))
                        .map(|(k, v)| {
                            format!("{k}(waits={},ms={})", v.total_waits, v.total_wait_ms)
                        })
                        .collect();
                    evidence.push(Evidence {
                        source: "wait_events".to_string(),
                        detail: lock_events.join(", "),
                    });
                }

                // 检查是否有慢查询
                if let Some(sq) = stats.query_history.iter().max_by_key(|r| r.elapsed_ms) {
                    if sq.elapsed_ms > stats.slow_query_threshold_ms {
                        causes.push(CauseEntry {
                            cause_type: CauseType::MissingIndex,
                            description: format!(
                                "慢查询耗时 {}ms，扫描/影响 {} 行，建议添加索引",
                                sq.elapsed_ms, sq.affected_rows
                            ),
                            confidence: 0.8,
                        });
                        evidence.push(Evidence {
                            source: "slow_query".to_string(),
                            detail: format!(
                                "sql={}, elapsed_ms={}, affected_rows={}",
                                truncate_sql(&sq.sql, 80),
                                sq.elapsed_ms,
                                sq.affected_rows
                            ),
                        });
                    }
                }

                // P3-RootCause-Enhanced：证据链增强 — 关联未授予的活动锁
                let pending_locks: Vec<String> = stats
                    .active_locks
                    .iter()
                    .filter(|l| !l.granted)
                    .map(|l| format!("txn={} table={} mode={}", l.txn_id, l.table, l.mode))
                    .collect();
                if !pending_locks.is_empty() {
                    evidence.push(Evidence {
                        source: "active_locks_pending".to_string(),
                        detail: pending_locks.join("; "),
                    });
                }

                if causes.is_empty() {
                    causes.push(CauseEntry {
                        cause_type: CauseType::StatsStale,
                        description: "统计信息可能过期，建议执行 ANALYZE".to_string(),
                        confidence: 0.5,
                    });
                }

                (causes, evidence)
            }
            _ => {
                // 未知告警类型：回退到统计信息过期
                let causes = vec![CauseEntry {
                    cause_type: CauseType::StatsStale,
                    description: "未知告警类型，建议执行 ANALYZE 更新统计信息并检查日志"
                        .to_string(),
                    confidence: 0.3,
                }];
                let evidence = vec![Evidence {
                    source: "alert".to_string(),
                    detail: format!("rule_id={}, message={}", alert.rule_id, alert.message),
                }];
                (causes, evidence)
            }
        };

        // P3-RootCause-Advanced：综合加权评分后处理（类贝叶斯推理）
        //
        // 设计思路：
        // 1. 上述 match 块按 rule_id 分派产生"先验"根因（已有逻辑，保持不变）
        // 2. 此处根据全局 RuntimeStats 指标计算每种根因类型的"综合得分"（似然）
        // 3. 合并先验 + 似然：补充新根因、向上调整已有根因置信度
        // 4. 按置信度降序排序，确保最可能的根因排在前面
        //
        // 评分模型：6 种根因类型 × 7 种指标，每种组合有预设权重
        // 指标来源：slow_query_count, error_query_count, deadlock_history.len(),
        //          wait_events 锁等待数, active_locks 未授予数, active_transactions 数, query_aggr QPS

        /// 综合评分：计算某种根因类型的置信度增量
        ///
        /// 返回 (cause_type, score, description) 三元组，score ∈ [0, 1]
        fn compute_cause_scores(stats: &RuntimeStats) -> Vec<(CauseType, f64, String)> {
            let mut results: Vec<(CauseType, f64, String)> = Vec::new();

            // 指标采集
            let slow_count = stats.slow_query_count as f64;
            let error_count = stats.error_query_count as f64;
            let deadlock_count = stats.deadlock_history.len() as f64;
            let lock_wait_events = stats
                .wait_events
                .iter()
                .filter(|(k, _)| k.contains("lock") || k.contains("Lock"))
                .map(|(_, v)| v.total_waits)
                .sum::<u64>() as f64;
            let pending_locks = stats.active_locks.iter().filter(|l| !l.granted).count() as f64;
            let active_txns = stats.active_transactions.len() as f64;
            let total_qps: u64 = stats.query_aggr.values().map(|a| a.count).sum();
            let total_qps_f = total_qps as f64;

            // 1. MissingIndex：慢查询多 + 影响行数大 → 缺失索引
            let missing_index_score = (slow_count / 10.0).min(0.4)
                + if stats.query_history.iter().any(|r| r.affected_rows > 1000) {
                    0.3
                } else {
                    0.0
                };
            if missing_index_score > 0.2 {
                results.push((
                    CauseType::MissingIndex,
                    missing_index_score,
                    format!(
                        "综合评分：{slow_count} 条慢查询，部分查询影响行数 >1000，疑似缺失索引（得分 {missing_index_score:.2}）"
                    ),
                ));
            }

            // 2. LockContention：锁等待事件多 + 未授予锁多 → 锁竞争
            let lock_contention_score =
                (lock_wait_events / 20.0).min(0.4) + (pending_locks / 5.0).min(0.3);
            if lock_contention_score > 0.2 {
                results.push((
                    CauseType::LockContention,
                    lock_contention_score,
                    format!(
                        "综合评分：{lock_wait_events} 次锁等待，{pending_locks} 个未授予锁，存在锁竞争（得分 {lock_contention_score:.2}）"
                    ),
                ));
            }

            // 3. Deadlock：死锁历史多 → 死锁
            let deadlock_score = (deadlock_count / 3.0).min(0.5);
            if deadlock_score > 0.2 {
                results.push((
                    CauseType::Deadlock,
                    deadlock_score,
                    format!(
                        "综合评分：{deadlock_count} 条死锁记录，死锁风险高（得分 {deadlock_score:.2}）"
                    ),
                ));
            }

            // 4. HighQps：QPS 高 → 突发流量
            let high_qps_score = (total_qps_f / 1000.0).min(0.5);
            if high_qps_score > 0.2 {
                results.push((
                    CauseType::HighQps,
                    high_qps_score,
                    format!(
                        "综合评分：累计 QPS {total_qps}，流量压力高（得分 {high_qps_score:.2}）"
                    ),
                ));
            }

            // 5. StatsStale：错误查询多 → 统计信息过期
            let stats_stale_score = (error_count / 10.0).min(0.4);
            if stats_stale_score > 0.2 {
                results.push((
                    CauseType::StatsStale,
                    stats_stale_score,
                    format!(
                        "综合评分：{error_count} 次错误查询，统计信息可能过期（得分 {stats_stale_score:.2}）"
                    ),
                ));
            }

            // 6. ResourceContention：活动事务多 + 活跃锁多 → 资源竞争
            let resource_contention_score =
                (active_txns / 10.0).min(0.3) + (stats.active_locks.len() as f64 / 10.0).min(0.2);
            if resource_contention_score > 0.2 {
                results.push((
                    CauseType::ResourceContention,
                    resource_contention_score,
                    format!(
                        "综合评分：{active_txns} 个活动事务，{} 个活动锁，资源竞争压力大（得分 {resource_contention_score:.2}）",
                        stats.active_locks.len()
                    ),
                ));
            }

            results
        }

        // 应用综合评分后处理
        let cause_scores = compute_cause_scores(&stats);
        for (cause_type, score, description) in &cause_scores {
            // 查找已有根因
            let existing = causes.iter_mut().find(|c| c.cause_type == *cause_type);
            match existing {
                Some(c) => {
                    // 向上调整置信度（取先验和综合得分的加权平均，但不降低）
                    let adjusted = (c.confidence * 0.6 + score * 0.4).max(c.confidence);
                    c.confidence = adjusted.min(1.0);
                }
                None => {
                    // 补充新根因（仅当综合得分超过阈值）
                    if *score > 0.3 {
                        causes.push(CauseEntry {
                            cause_type: cause_type.clone(),
                            description: description.clone(),
                            confidence: *score * 0.7, // 新增根因置信度打折（缺乏先验支持）
                        });
                    }
                }
            }
        }

        // 按置信度降序排序
        causes.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(RootCauseReport {
            alert,
            likely_causes: causes,
            evidence,
        })
    }
    fn get_lineage(&self, table: Option<&str>) -> Result<LineageInfo, McpError> {
        // P3-Lineage：从 LineageStore 返回真实血缘数据
        // 血缘边由 execute_sql 在 CTAS/VIEW/INSERT INTO SELECT 时通过
        // record_lineage_from_select 自动记录
        let lineage = self.lineage.borrow();
        let total_edges = lineage.edge_count();
        let tables = lineage.all_tables();

        match table {
            None => Ok(LineageInfo {
                table: None,
                upstream: lineage.edges.clone(),
                downstream: vec![],
                tables,
                total_edges,
            }),
            Some(t) => Ok(LineageInfo {
                table: Some(t.to_string()),
                upstream: lineage.upstream_of(t),
                downstream: lineage.downstream_of(t),
                tables,
                total_edges,
            }),
        }
    }
}

/// 将 `szrsql_types::value::Value` 转换为 `serde_json::Value`
///
/// 用于把执行器返回的行数据（`Vec<szrsql_types::value::Value>`）转换为
/// MCP `QueryResult.rows` 所需的 `Vec<Vec<serde_json::Value>>`。
fn value_to_json(v: szrsql_types::value::Value) -> Value {
    use szrsql_types::value::Value as V;
    match v {
        V::Null => Value::Null,
        V::Int64(n) => json!(n),
        V::Float64(f) => json!(f),
        V::Text(s) => json!(s),
        V::Bool(b) => json!(b),
        V::Decimal(unscaled, scale) => {
            let f = (unscaled as f64) / 10f64.powi(scale as i32);
            json!(f)
        }
        V::Date(days) => json!(days),
        V::Timestamp(us) => json!(us),
        V::Blob(b) => json!(b),
        V::Array(arr) => json!(arr.into_iter().map(value_to_json).collect::<Vec<_>>()),
        V::Enum(s) => json!(s),
        V::Json(v) => v,
        _ => Value::Null,
    }
}

/// 将 LogicalPlan 格式化为操作符列表（EXPLAIN 输出）
fn format_plan_operators(plan: &szrsql_sql::plan::LogicalPlan) -> Vec<String> {
    let mut ops = vec![];
    collect_operators(plan, &mut ops, 0);
    ops
}

/// 递归收集操作符
fn collect_operators(plan: &szrsql_sql::plan::LogicalPlan, ops: &mut Vec<String>, depth: usize) {
    use szrsql_sql::plan::LogicalPlan;
    let indent = "  ".repeat(depth);
    match plan {
        LogicalPlan::Scan { table, .. } => {
            ops.push(format!("{indent}SeqScan({})", table.name));
        }
        LogicalPlan::IndexScan {
            table, index_name, ..
        } => {
            ops.push(format!(
                "{indent}IndexScan({}, idx={})",
                table.name, index_name
            ));
        }
        LogicalPlan::Projection { input, .. } => {
            ops.push(format!("{indent}Projection"));
            collect_operators(input, ops, depth + 1);
        }
        LogicalPlan::Filter { predicate, input } => {
            ops.push(format!("{indent}Filter({predicate:?})"));
            collect_operators(input, ops, depth + 1);
        }
        LogicalPlan::Join {
            join_type,
            left,
            right,
            ..
        } => {
            ops.push(format!("{indent}Join({join_type:?})"));
            collect_operators(left, ops, depth + 1);
            collect_operators(right, ops, depth + 1);
        }
        LogicalPlan::Aggregate { input, .. } => {
            ops.push(format!("{indent}Aggregate"));
            collect_operators(input, ops, depth + 1);
        }
        LogicalPlan::Sort { input, .. } => {
            ops.push(format!("{indent}Sort"));
            collect_operators(input, ops, depth + 1);
        }
        LogicalPlan::Limit { input, .. } => {
            ops.push(format!("{indent}Limit"));
            collect_operators(input, ops, depth + 1);
        }
        LogicalPlan::Distinct { input } => {
            ops.push(format!("{indent}Distinct"));
            collect_operators(input, ops, depth + 1);
        }
        LogicalPlan::Insert { table, .. } => {
            ops.push(format!("{indent}Insert({})", table.name));
        }
        LogicalPlan::Update { table, .. } => {
            ops.push(format!("{indent}Update({})", table.name));
        }
        LogicalPlan::Delete { table, .. } => {
            ops.push(format!("{indent}Delete({})", table.name));
        }
        LogicalPlan::CreateTable { name, .. } => {
            ops.push(format!("{indent}CreateTable({})", name.name));
        }
        LogicalPlan::DropTable { names, .. } => {
            let names_str = names
                .iter()
                .map(|n| n.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            ops.push(format!("{indent}DropTable({names_str})"));
        }
        _ => {
            ops.push(format!("{indent}{plan:?}"));
        }
    }
}

// =====================================================================
//  McpServerV2 — 30 工具 MCP 服务器
// =====================================================================

/// MCP Server V2 — JSON-RPC 2.0 over stdio，30 个 LLM 工具
///
/// 在 Phase 7b.6 `McpServer`（4 工具）基础上扩展为 30 工具，
/// 覆盖 8 个类别：Schema / Query / SlowQuery / TxLock / Perf / Maintenance / Alerting / Insight
pub struct McpServerV2 {
    backend: Box<dyn McpBackendV2>,
    initialized: bool,
}

impl Default for McpServerV2 {
    fn default() -> Self {
        Self::new(Box::new(MockBackendV2::default()))
    }
}

impl McpServerV2 {
    /// 创建 MCP Server V2
    pub fn new(backend: Box<dyn McpBackendV2>) -> Self {
        Self {
            backend,
            initialized: false,
        }
    }

    /// 创建连接真实 catalog 的 MCP Server V2 — Phase TDengine-P3-MVP
    ///
    /// 使用 `CatalogBackend` 作为后端，4 个 Schema 类工具
    /// （`list_tables` / `describe_table` / `list_indexes` / `list_views`）
    /// 返回真实元数据，其余 26 个方法返回空/Err。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let mut catalog = szrsql_catalog::ManagedCatalog::new();
    /// // ... 建表 + COMMENT ON COLUMN ...
    /// let mut server = McpServerV2::new_with_catalog(Box::new(catalog));
    /// ```
    pub fn new_with_catalog(catalog: Box<dyn szrsql_catalog::MutableCatalog>) -> Self {
        Self::new(Box::new(CatalogBackend::new(catalog)))
    }

    /// 创建连接真实执行器的 MCP Server V2 — Phase TDengine-P3-Full
    ///
    /// 使用 `ExecutorBackend` 作为后端，4 个 Schema 类工具 + 3 个 Query 类工具
    /// （`execute_sql` / `explain_query` / `prepare_statement`）返回真实结果，
    /// 其余 23 个方法返回空/Err。
    ///
    /// # 示例
    ///
    /// ```ignore
    /// let mut backend = ExecutorBackend::new();
    /// backend.execute_sql("CREATE TABLE t (id INT, name TEXT)").unwrap();
    /// let mut server = McpServerV2::new_with_executor(backend);
    /// ```
    pub fn new_with_executor(backend: ExecutorBackend) -> Self {
        Self::new(Box::new(backend))
    }

    /// 工具总数
    pub const TOOL_COUNT: usize = 35;

    /// 所有工具定义（35 个，按类别分组）
    pub fn tool_definitions(&self) -> Vec<ToolDefinitionV2> {
        vec![
            // === 类别 1: Schema ===
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "list_tables".to_string(),
                    description: "列出数据库中所有表".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::Schema,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "describe_table".to_string(),
                    description: "描述指定表的结构（列名、类型、约束）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"table": {"type": "string", "description": "表名"}},
                        "required": ["table"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Schema,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "list_indexes".to_string(),
                    description: "列出指定表的所有索引".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"table": {"type": "string", "description": "表名"}},
                        "required": ["table"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Schema,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "list_views".to_string(),
                    description: "列出数据库中所有视图".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::Schema,
            },
            // === 类别 2: Query ===
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "execute_sql".to_string(),
                    description: "执行 SQL 语句（SELECT/INSERT/UPDATE/DELETE）并返回结果"
                        .to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"sql": {"type": "string", "description": "SQL 语句"}},
                        "required": ["sql"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Query,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "explain_query".to_string(),
                    description: "获取 SQL 的执行计划（EXPLAIN）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"sql": {"type": "string", "description": "SQL 语句"}},
                        "required": ["sql"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Query,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "prepare_statement".to_string(),
                    description: "预处理 SQL 语句（PREPARE）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "name": {"type": "string", "description": "预处理语句名称"},
                            "sql": {"type": "string", "description": "含 ? 占位符的 SQL"}
                        },
                        "required": ["name", "sql"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Query,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "cancel_query".to_string(),
                    description: "取消正在执行的查询".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"query_id": {"type": "integer", "description": "查询 ID"}},
                        "required": ["query_id"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Query,
            },
            // === 类别 3: SlowQuery ===
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "slow_queries".to_string(),
                    description: "获取慢查询列表（按耗时降序）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"limit": {"type": "integer", "description": "返回条数上限", "default": 10}},
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::SlowQuery,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "top_queries".to_string(),
                    description: "获取高频查询 Top N（按调用次数降序）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"limit": {"type": "integer", "description": "返回条数上限", "default": 10}},
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::SlowQuery,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "query_stats".to_string(),
                    description: "获取查询统计摘要（pg_stat_statements 聚合）".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::SlowQuery,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "reset_stats".to_string(),
                    description: "重置查询统计".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::SlowQuery,
            },
            // === 类别 4: TxLock ===
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "list_transactions".to_string(),
                    description: "列出所有活跃事务".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::TxLock,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "list_locks".to_string(),
                    description: "列出所有锁信息".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::TxLock,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "kill_transaction".to_string(),
                    description: "终止指定事务".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"txn_id": {"type": "integer", "description": "事务 ID"}},
                        "required": ["txn_id"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::TxLock,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "deadlock_history".to_string(),
                    description: "获取死锁历史记录".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::TxLock,
            },
            // === 类别 5: Perf ===
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "wait_events".to_string(),
                    description: "获取等待事件统计（pg_stat_wait）".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::Perf,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "ash_report".to_string(),
                    description: "生成 ASH 报告（Active Session History）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"duration_secs": {"type": "integer", "description": "采样时长（秒）", "default": 60}},
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Perf,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "active_sessions".to_string(),
                    description: "获取活跃会话列表".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::Perf,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "pprof_dump".to_string(),
                    description: "采集 CPU 性能剖析（pprof 格式）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"duration_secs": {"type": "integer", "description": "采样时长（秒）", "default": 30}},
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Perf,
            },
            // === 类别 6: Maintenance ===
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "vacuum_table".to_string(),
                    description: "对指定表执行 VACUUM（回收死元组）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"table": {"type": "string", "description": "表名"}},
                        "required": ["table"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Maintenance,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "analyze_table".to_string(),
                    description: "对指定表执行 ANALYZE（更新统计信息）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"table": {"type": "string", "description": "表名"}},
                        "required": ["table"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Maintenance,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "autovacuum_status".to_string(),
                    description: "获取 Autovacuum 运行状态".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::Maintenance,
            },
            // === 类别 7: Alerting ===
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "list_alerts".to_string(),
                    description: "获取当前告警列表".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::Alerting,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "db_stats".to_string(),
                    description: "获取数据库统计信息（表数、总行数、缓存命中率等）".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::Alerting,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "capacity_predict".to_string(),
                    description: "容量预测（基于历史趋势预测未来增长）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"days": {"type": "integer", "description": "预测天数", "default": 30}},
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Alerting,
            },
            // === 类别 8: Insight（TDengine 启发） ===
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "summarize_table".to_string(),
                    description: "表数据摘要（自动统计各列基数/分布/top 值，LLM 无需写 SQL 即可理解数据）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"table": {"type": "string", "description": "表名"}},
                        "required": ["table"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Insight,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "ask_data".to_string(),
                    description: "自然语言问答（Agent Interface 统一入口，返回答案 + SQL + 数据引用）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"question": {"type": "string", "description": "自然语言问题"}},
                        "required": ["question"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Insight,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "explain_root_cause".to_string(),
                    description: "根因分析（关联 alerts + slow_queries + wait_events 三源数据，返回可能原因 + 证据）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"alert_id": {"type": "string", "description": "告警 rule_id"}},
                        "required": ["alert_id"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Insight,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "get_lineage".to_string(),
                    description: "数据血缘查询（字段级，返回上下游来源 + 转换描述；不传 table 返回全量血缘）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"table": {"type": "string", "description": "表名（可选，不传则返回全量血缘）"}},
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Insight,
            },
            // === 类别 9: Replication ===
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "create_replication_task".to_string(),
                    description: "创建 CDC 数据复制任务（源端→目标端，支持 PG/MySQL/Kafka）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {
                            "task_id": {"type": "string", "description": "任务 ID（唯一）"},
                            "description": {"type": "string", "description": "任务描述"},
                            "target_type": {"type": "string", "description": "目标端类型：postgres/mysql/kafka/memory"},
                            "target_connection": {"type": "string", "description": "目标端连接串"},
                            "table_filter": {"type": "array", "items": {"type": "string"}, "description": "表过滤白名单（不传则复制所有表）"},
                            "snapshot_first": {"type": "boolean", "description": "是否全量同步完成后才开启增量（默认 true）"}
                        },
                        "required": ["task_id", "target_type", "target_connection"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Replication,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "list_replication_tasks".to_string(),
                    description: "列出所有 CDC 复制任务及其状态".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::Replication,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "monitor_replication_task".to_string(),
                    description: "监控指定 CDC 复制任务（详细统计：事件数/字节数/LSN/滞后量）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"task_id": {"type": "string", "description": "任务 ID"}},
                        "required": ["task_id"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Replication,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "stop_replication_task".to_string(),
                    description: "停止指定的 CDC 复制任务（停止后 slot 保留，可重新启动）".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"task_id": {"type": "string", "description": "任务 ID"}},
                        "required": ["task_id"],
                        "additionalProperties": false
                    }),
                },
                category: ToolCategory::Replication,
            },
            ToolDefinitionV2 {
                base: ToolDefinition {
                    name: "replication_manager_stats".to_string(),
                    description: "获取 CDC 复制管理器统计（任务总数/运行数/累计创建/停止/失败数）".to_string(),
                    input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
                },
                category: ToolCategory::Replication,
            },
        ]
    }

    /// 按类别过滤工具
    pub fn tools_by_category(&self, category: ToolCategory) -> Vec<ToolDefinitionV2> {
        self.tool_definitions()
            .into_iter()
            .filter(|t| t.category == category)
            .collect()
    }

    /// 统计每个类别的工具数
    pub fn category_counts(&self) -> HashMap<ToolCategory, usize> {
        let mut counts = HashMap::new();
        for tool in self.tool_definitions() {
            *counts.entry(tool.category).or_insert(0) += 1;
        }
        counts
    }

    /// 处理 JSON-RPC 请求，返回 JSON-RPC 响应
    pub fn handle_request(&mut self, req: &JsonRpcRequest) -> JsonRpcResponse {
        let id = req.id.clone();

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
            "initialized" => Ok(json!({})),
            "tools/list" => self.handle_tools_list(req.params.as_ref()),
            "tools/call" => self.handle_tools_call(req.params.as_ref()),
            "shutdown" => {
                self.initialized = false;
                Ok(json!({}))
            }
            _ => Err(McpError::MethodNotFound(req.method.clone())),
        };

        match result {
            Ok(value) => JsonRpcResponse::success(id, value),
            Err(err) => {
                // 内联 McpError → JSON-RPC 错误码映射（mcp.rs 中 code() 为私有方法）
                let code = match &err {
                    McpError::ParseError(_) => -32700,
                    McpError::InvalidRequest(_) => -32600,
                    McpError::MethodNotFound(_) => -32601,
                    McpError::InvalidToolParams(_) => -32602,
                    McpError::ToolNotFound(_) | McpError::ToolExecutionError(_) => -32603,
                    McpError::BackendError(_) => -32000,
                };
                JsonRpcResponse::error(id, code, err.to_string(), None)
            }
        }
    }

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

    fn handle_tools_list(&self, _params: Option<&Value>) -> Result<Value, McpError> {
        let tools = self.tool_definitions();
        let tools_json: Vec<Value> = tools
            .iter()
            .map(|t| {
                let base = serde_json::to_value(&t.base).unwrap_or(json!({}));
                let mut map = base.as_object().unwrap_or(&serde_json::Map::new()).clone();
                map.insert("category".to_string(), json!(t.category.as_str()));
                Value::Object(map)
            })
            .collect();
        Ok(json!({ "tools": tools_json }))
    }

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
            // 类别 1: Schema
            "list_tables" => self.tool_list_tables(&args)?,
            "describe_table" => self.tool_describe_table(&args)?,
            "list_indexes" => self.tool_list_indexes(&args)?,
            "list_views" => self.tool_list_views(&args)?,
            // 类别 2: Query
            "execute_sql" => self.tool_execute_sql(&args)?,
            "explain_query" => self.tool_explain_query(&args)?,
            "prepare_statement" => self.tool_prepare_statement(&args)?,
            "cancel_query" => self.tool_cancel_query(&args)?,
            // 类别 3: SlowQuery
            "slow_queries" => self.tool_slow_queries(&args)?,
            "top_queries" => self.tool_top_queries(&args)?,
            "query_stats" => self.tool_query_stats(&args)?,
            "reset_stats" => self.tool_reset_stats(&args)?,
            // 类别 4: TxLock
            "list_transactions" => self.tool_list_transactions(&args)?,
            "list_locks" => self.tool_list_locks(&args)?,
            "kill_transaction" => self.tool_kill_transaction(&args)?,
            "deadlock_history" => self.tool_deadlock_history(&args)?,
            // 类别 5: Perf
            "wait_events" => self.tool_wait_events(&args)?,
            "ash_report" => self.tool_ash_report(&args)?,
            "active_sessions" => self.tool_active_sessions(&args)?,
            "pprof_dump" => self.tool_pprof_dump(&args)?,
            // 类别 6: Maintenance
            "vacuum_table" => self.tool_vacuum_table(&args)?,
            "analyze_table" => self.tool_analyze_table(&args)?,
            "autovacuum_status" => self.tool_autovacuum_status(&args)?,
            // 类别 7: Alerting
            "list_alerts" => self.tool_list_alerts(&args)?,
            "db_stats" => self.tool_db_stats(&args)?,
            "capacity_predict" => self.tool_capacity_predict(&args)?,
            // 类别 8: Insight（TDengine 启发）
            "summarize_table" => self.tool_summarize_table(&args)?,
            "ask_data" => self.tool_ask_data(&args)?,
            "explain_root_cause" => self.tool_explain_root_cause(&args)?,
            "get_lineage" => self.tool_get_lineage(&args)?,
            // 类别 9: Replication（NineData 启发）
            "create_replication_task" => self.tool_create_replication_task(&args)?,
            "list_replication_tasks" => self.tool_list_replication_tasks(&args)?,
            "monitor_replication_task" => self.tool_monitor_replication_task(&args)?,
            "stop_replication_task" => self.tool_stop_replication_task(&args)?,
            "replication_manager_stats" => self.tool_replication_manager_stats(&args)?,
            _ => return Err(McpError::ToolNotFound(tool_name.to_string())),
        };

        serde_json::to_value(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize result failed: {e}")))
    }

    // -----------------------------------------------------------------
    //  类别 1: Schema — 4 个工具实现
    // -----------------------------------------------------------------

    fn tool_list_tables(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let tables = self.backend.list_tables()?;
        let text = serde_json::to_string_pretty(&tables)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

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

    fn tool_list_indexes(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let table = args
            .get("table")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'table' argument".to_string()))?;
        let indexes = self.backend.list_indexes(table)?;
        let text = serde_json::to_string_pretty(&indexes)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_list_views(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let views = self.backend.list_views()?;
        let text = serde_json::to_string_pretty(&views)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    // -----------------------------------------------------------------
    //  类别 2: Query — 4 个工具实现
    // -----------------------------------------------------------------

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

    fn tool_explain_query(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let sql = args
            .get("sql")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'sql' argument".to_string()))?;
        let plan = self.backend.explain_query(sql)?;
        let text = serde_json::to_string_pretty(&plan)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_prepare_statement(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'name' argument".to_string()))?;
        let sql = args
            .get("sql")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'sql' argument".to_string()))?;
        let result = self.backend.prepare_statement(name, sql)?;
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_cancel_query(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let query_id = args
            .get("query_id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| {
                McpError::InvalidToolParams("missing 'query_id' argument".to_string())
            })?;
        let result = self.backend.cancel_query(query_id)?;
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    // -----------------------------------------------------------------
    //  类别 3: SlowQuery — 4 个工具实现
    // -----------------------------------------------------------------

    fn tool_slow_queries(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let queries = self.backend.slow_queries(limit)?;
        let text = serde_json::to_string_pretty(&queries)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_top_queries(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let queries = self.backend.top_queries(limit)?;
        let text = serde_json::to_string_pretty(&queries)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_query_stats(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let stats = self.backend.query_stats()?;
        let text = serde_json::to_string_pretty(&stats)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_reset_stats(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let result = self.backend.reset_stats()?;
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    // -----------------------------------------------------------------
    //  类别 4: TxLock — 4 个工具实现
    // -----------------------------------------------------------------

    fn tool_list_transactions(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let txns = self.backend.list_transactions()?;
        let text = serde_json::to_string_pretty(&txns)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_list_locks(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let locks = self.backend.list_locks()?;
        let text = serde_json::to_string_pretty(&locks)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_kill_transaction(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let txn_id =
            args.get("txn_id").and_then(|v| v.as_u64()).ok_or_else(|| {
                McpError::InvalidToolParams("missing 'txn_id' argument".to_string())
            })? as u32;
        let result = self.backend.kill_transaction(txn_id)?;
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_deadlock_history(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let deadlocks = self.backend.deadlock_history()?;
        let text = serde_json::to_string_pretty(&deadlocks)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    // -----------------------------------------------------------------
    //  类别 5: Perf — 4 个工具实现
    // -----------------------------------------------------------------

    fn tool_wait_events(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let events = self.backend.wait_events()?;
        let text = serde_json::to_string_pretty(&events)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_ash_report(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let duration = args
            .get("duration_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);
        let report = self.backend.ash_report(duration)?;
        let text = serde_json::to_string_pretty(&report)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_active_sessions(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let sessions = self.backend.active_sessions()?;
        let text = serde_json::to_string_pretty(&sessions)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_pprof_dump(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let duration = args
            .get("duration_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);
        let result = self.backend.pprof_dump(duration)?;
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    // -----------------------------------------------------------------
    //  类别 6: Maintenance — 3 个工具实现
    // -----------------------------------------------------------------

    fn tool_vacuum_table(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let table = args
            .get("table")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'table' argument".to_string()))?;
        let result = self.backend.vacuum_table(table)?;
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_analyze_table(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let table = args
            .get("table")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'table' argument".to_string()))?;
        let result = self.backend.analyze_table(table)?;
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_autovacuum_status(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let status = self.backend.autovacuum_status()?;
        let text = serde_json::to_string_pretty(&status)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    // -----------------------------------------------------------------
    //  类别 7: Alerting — 3 个工具实现
    // -----------------------------------------------------------------

    fn tool_list_alerts(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let alerts = self.backend.list_alerts()?;
        let text = serde_json::to_string_pretty(&alerts)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_db_stats(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let stats = self.backend.db_stats()?;
        let text = serde_json::to_string_pretty(&stats)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_capacity_predict(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let days = args.get("days").and_then(|v| v.as_u64()).unwrap_or(30) as u32;
        let forecast = self.backend.capacity_predict(days)?;
        let text = serde_json::to_string_pretty(&forecast)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    // -----------------------------------------------------------------
    //  类别 8: Insight — 2 个工具实现（TDengine 启发）
    // -----------------------------------------------------------------

    fn tool_summarize_table(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let table = args
            .get("table")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'table' argument".to_string()))?;
        let summary = self.backend.summarize_table(table)?;
        let text = serde_json::to_string_pretty(&summary)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_ask_data(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let question = args
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::InvalidToolParams("missing 'question' argument".to_string())
            })?;
        let answer = self.backend.ask_data(question)?;
        let text = serde_json::to_string_pretty(&answer)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_explain_root_cause(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let alert_id = args
            .get("alert_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::InvalidToolParams("missing 'alert_id' argument".to_string())
            })?;
        let report = self.backend.explain_root_cause(alert_id)?;
        let text = serde_json::to_string_pretty(&report)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    fn tool_get_lineage(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        // table 参数可选：未传或 null → 全量血缘；传字符串 → 该表上下游
        let table = args
            .get("table")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let info = self.backend.get_lineage(table)?;
        let text = serde_json::to_string_pretty(&info)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    // -----------------------------------------------------------------
    //  类别 9: Replication — 5 个工具实现（NineData 启发）
    // -----------------------------------------------------------------

    /// create_replication_task — 创建 CDC 数据复制任务
    ///
    /// 必填参数：task_id / target_type / target_connection
    /// 可选参数：description / table_filter (array) / snapshot_first (bool)
    fn tool_create_replication_task(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'task_id' argument".to_string()))?;
        let target_type = args
            .get("target_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::InvalidToolParams("missing 'target_type' argument".to_string())
            })?;
        let target_connection = args
            .get("target_connection")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::InvalidToolParams("missing 'target_connection' argument".to_string())
            })?;
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let table_filter = args
            .get("table_filter")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<String>>()
            });
        let snapshot_first = args
            .get("snapshot_first")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let params = CreateReplicationTaskParams {
            task_id: task_id.to_string(),
            description,
            target_type: target_type.to_string(),
            target_connection: target_connection.to_string(),
            table_filter,
            snapshot_first,
        };
        let result = self.backend.create_replication_task(params)?;
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    /// list_replication_tasks — 列出所有复制任务
    fn tool_list_replication_tasks(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let tasks = self.backend.list_replication_tasks()?;
        let text = serde_json::to_string_pretty(&tasks)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    /// monitor_replication_task — 监控指定复制任务
    fn tool_monitor_replication_task(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'task_id' argument".to_string()))?;
        let info = self.backend.monitor_replication_task(task_id)?;
        let text = serde_json::to_string_pretty(&info)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    /// stop_replication_task — 停止复制任务
    fn tool_stop_replication_task(&self, args: &Value) -> Result<ToolCallResult, McpError> {
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::InvalidToolParams("missing 'task_id' argument".to_string()))?;
        let result = self.backend.stop_replication_task(task_id)?;
        let text = serde_json::to_string_pretty(&result)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    /// replication_manager_stats — 复制管理器统计
    fn tool_replication_manager_stats(&self, _args: &Value) -> Result<ToolCallResult, McpError> {
        let stats = self.backend.replication_manager_stats()?;
        let text = serde_json::to_string_pretty(&stats)
            .map_err(|e| McpError::ToolExecutionError(format!("serialize failed: {e}")))?;
        Ok(ToolCallResult::text_success(text))
    }

    // -----------------------------------------------------------------
    //  stdio 主循环（复用 Phase 7b.6 的实现模式）
    // -----------------------------------------------------------------

    /// 运行 stdio 主循环（每行一条 JSON-RPC 消息）
    pub fn run_stdio(&mut self) -> Result<(), McpError> {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut stdout = stdout.lock();
        use std::io::{BufRead, Write};

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

/// 解析并处理单条 JSON-RPC 请求字符串（V2）
pub fn handle_request_json_v2(server: &mut McpServerV2, request_json: &str) -> String {
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
    // 1. 工具总数与类别覆盖测试（验证标准：30 个工具 + 8 个类别全覆盖）
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_tool_count_is_30() {
        let server = McpServerV2::default();
        let tools = server.tool_definitions();
        assert_eq!(tools.len(), 35, "MCP Server V2 must have exactly 35 tools");
        assert_eq!(McpServerV2::TOOL_COUNT, 35);
    }

    #[test]
    fn test_7d22_all_8_categories_covered() {
        let server = McpServerV2::default();
        let counts = server.category_counts();
        assert_eq!(counts.len(), 9, "must have 9 categories");
        for category in ToolCategory::all() {
            assert!(
                counts.contains_key(category),
                "category {:?} not covered",
                category
            );
        }
    }

    #[test]
    fn test_7d22_category_tool_counts() {
        let server = McpServerV2::default();
        let counts = server.category_counts();
        assert_eq!(counts.get(&ToolCategory::Schema), Some(&4));
        assert_eq!(counts.get(&ToolCategory::Query), Some(&4));
        assert_eq!(counts.get(&ToolCategory::SlowQuery), Some(&4));
        assert_eq!(counts.get(&ToolCategory::TxLock), Some(&4));
        assert_eq!(counts.get(&ToolCategory::Perf), Some(&4));
        assert_eq!(counts.get(&ToolCategory::Maintenance), Some(&3));
        assert_eq!(counts.get(&ToolCategory::Alerting), Some(&3));
        assert_eq!(counts.get(&ToolCategory::Insight), Some(&4));
        assert_eq!(counts.get(&ToolCategory::Replication), Some(&5));
        // 4+4+4+4+4+3+3+4+5 = 35
        let total: usize = counts.values().sum();
        assert_eq!(total, 35);
    }

    #[test]
    fn test_7d22_all_tool_names_unique() {
        let server = McpServerV2::default();
        let tools = server.tool_definitions();
        let names: Vec<&str> = tools.iter().map(|t| t.base.name.as_str()).collect();
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(names.len(), unique.len(), "tool names must be unique");
    }

    #[test]
    fn test_7d22_all_tools_have_schema_and_description() {
        let server = McpServerV2::default();
        for tool in server.tool_definitions() {
            assert!(!tool.base.name.is_empty(), "tool name empty");
            assert!(
                !tool.base.description.is_empty(),
                "tool description empty for {}",
                tool.base.name
            );
            assert!(
                tool.base.input_schema.is_object(),
                "tool input_schema not object for {}",
                tool.base.name
            );
        }
    }

    #[test]
    fn test_7d22_expected_tool_names_present() {
        let server = McpServerV2::default();
        let names: Vec<String> = server
            .tool_definitions()
            .iter()
            .map(|t| t.base.name.clone())
            .collect();
        let expected = [
            "list_tables",
            "describe_table",
            "list_indexes",
            "list_views",
            "execute_sql",
            "explain_query",
            "prepare_statement",
            "cancel_query",
            "slow_queries",
            "top_queries",
            "query_stats",
            "reset_stats",
            "list_transactions",
            "list_locks",
            "kill_transaction",
            "deadlock_history",
            "wait_events",
            "ash_report",
            "active_sessions",
            "pprof_dump",
            "vacuum_table",
            "analyze_table",
            "autovacuum_status",
            "list_alerts",
            "db_stats",
            "capacity_predict",
            "summarize_table",
            "ask_data",
            "explain_root_cause",
            "get_lineage",
            "create_replication_task",
            "list_replication_tasks",
            "monitor_replication_task",
            "stop_replication_task",
            "replication_manager_stats",
        ];
        for name in &expected {
            assert!(names.contains(&name.to_string()), "missing tool: {name}");
        }
    }

    // -----------------------------------------------------------------
    //  2. ToolCategory 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_tool_category_as_str() {
        assert_eq!(ToolCategory::Schema.as_str(), "schema");
        assert_eq!(ToolCategory::Query.as_str(), "query");
        assert_eq!(ToolCategory::SlowQuery.as_str(), "slow_query");
        assert_eq!(ToolCategory::TxLock.as_str(), "tx_lock");
        assert_eq!(ToolCategory::Perf.as_str(), "perf");
        assert_eq!(ToolCategory::Maintenance.as_str(), "maintenance");
        assert_eq!(ToolCategory::Alerting.as_str(), "alerting");
        assert_eq!(ToolCategory::Insight.as_str(), "insight");
        assert_eq!(ToolCategory::Replication.as_str(), "replication");
    }

    #[test]
    fn test_7d22_tool_category_all() {
        let all = ToolCategory::all();
        assert_eq!(all.len(), 9);
    }

    #[test]
    fn test_7d22_tools_by_category() {
        let server = McpServerV2::default();
        let schema_tools = server.tools_by_category(ToolCategory::Schema);
        assert_eq!(schema_tools.len(), 4);
        let maintenance_tools = server.tools_by_category(ToolCategory::Maintenance);
        assert_eq!(maintenance_tools.len(), 3);
    }

    // -----------------------------------------------------------------
    //  3. initialize / protocol 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_initialize() {
        let mut server = McpServerV2::default();
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
        assert!(server.initialized);
    }

    #[test]
    fn test_7d22_invalid_jsonrpc_version() {
        let mut server = McpServerV2::default();
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
    // 4. tools/list 测试（验证标准：list_tools 返回 30 个工具）
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_tools_list_returns_30() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/list".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 35, "tools/list must return 35 tools");
    }

    #[test]
    fn test_7d22_tools_list_has_category_field() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/list".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        for tool in &tools {
            assert!(tool["category"].is_string(), "tool missing category field");
            assert!(!tool["category"].as_str().unwrap().is_empty());
        }
    }

    // -----------------------------------------------------------------
    //  5. 类别 1: Schema 工具调用测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_call_list_tables() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "list_tables", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("products"));
        assert!(text.contains("orders"));
    }

    #[test]
    fn test_7d22_call_describe_table() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "describe_table", "arguments": {"table": "products"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("id"));
        assert!(text.contains("name"));
        assert!(text.contains("price"));
    }

    #[test]
    fn test_7d22_call_describe_table_not_found() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "describe_table", "arguments": {"table": "nonexistent"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32000);
    }

    #[test]
    fn test_7d22_call_describe_table_missing_arg() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "describe_table", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[test]
    fn test_7d22_call_list_indexes() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(5)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "list_indexes", "arguments": {"table": "products"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("idx_products_id"));
        assert!(text.contains("unique"));
    }

    #[test]
    fn test_7d22_call_list_indexes_not_found() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(6)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "list_indexes", "arguments": {"table": "nonexistent"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
    }

    #[test]
    fn test_7d22_call_list_views() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(7)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "list_views", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("v_product_summary"));
    }

    // -----------------------------------------------------------------
    //  6. 类别 2: Query 工具调用测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_call_execute_sql() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(
                json!({"name": "execute_sql", "arguments": {"sql": "SELECT id, name, price FROM products"}}),
            ),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("columns"));
        assert!(text.contains("苹果汁"));
    }

    #[test]
    fn test_7d22_call_execute_sql_empty() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "execute_sql", "arguments": {"sql": "  "}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[test]
    fn test_7d22_call_execute_sql_missing_arg() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "execute_sql", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[test]
    fn test_7d22_call_explain_query() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "tools/call".to_string(),
            params: Some(
                json!({"name": "explain_query", "arguments": {"sql": "SELECT * FROM products WHERE id = 1"}}),
            ),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("cost"));
        assert!(text.contains("Index Scan"));
    }

    #[test]
    fn test_7d22_call_explain_query_seq_scan() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(5)),
            method: "tools/call".to_string(),
            params: Some(
                json!({"name": "explain_query", "arguments": {"sql": "SELECT * FROM products"}}),
            ),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Seq Scan"));
    }

    #[test]
    fn test_7d22_call_prepare_statement() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(6)),
            method: "tools/call".to_string(),
            params: Some(
                json!({"name": "prepare_statement", "arguments": {"name": "stmt1", "sql": "SELECT * FROM products WHERE id = ? AND name = ?"}}),
            ),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("stmt1"));
        assert!(text.contains("parameter_count"));
    }

    #[test]
    fn test_7d22_call_prepare_statement_missing_args() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(7)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "prepare_statement", "arguments": {"name": "stmt1"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[test]
    fn test_7d22_call_cancel_query() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(8)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "cancel_query", "arguments": {"query_id": 42}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("42"));
        assert!(text.contains("cancelled"));
    }

    // -----------------------------------------------------------------
    //  7. 类别 3: SlowQuery 工具调用测试（验证标准：slow_queries 返回慢查询）
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_call_slow_queries() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "slow_queries", "arguments": {"limit": 10}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("SELECT * FROM orders"));
        assert!(text.contains("elapsed_ms"));
        assert!(text.contains("350"));
    }

    #[test]
    fn test_7d22_call_slow_queries_default_limit() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "slow_queries", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_7d22_call_top_queries() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "top_queries", "arguments": {"limit": 5}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("calls"));
        assert!(text.contains("total_time_ms"));
    }

    #[test]
    fn test_7d22_call_query_stats() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "query_stats", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("total_queries"));
        assert!(text.contains("avg_time_ms"));
    }

    #[test]
    fn test_7d22_call_reset_stats() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(5)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "reset_stats", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("reset"));
        assert!(text.contains("true"));
    }

    // -----------------------------------------------------------------
    //  8. 类别 4: TxLock 工具调用测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_call_list_transactions() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "list_transactions", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("1001"));
        assert!(text.contains("active"));
    }

    #[test]
    fn test_7d22_call_list_locks() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "list_locks", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Exclusive"));
        assert!(text.contains("products"));
    }

    #[test]
    fn test_7d22_call_kill_transaction() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "kill_transaction", "arguments": {"txn_id": 1001}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("1001"));
        assert!(text.contains("killed"));
    }

    #[test]
    fn test_7d22_call_kill_transaction_missing_arg() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "kill_transaction", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[test]
    fn test_7d22_call_deadlock_history() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(5)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "deadlock_history", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("1001"));
        assert!(text.contains("1002"));
    }

    // -----------------------------------------------------------------
    //  9. 类别 5: Perf 工具调用测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_call_wait_events() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "wait_events", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("db file sequential read"));
        assert!(text.contains("total_waits"));
    }

    #[test]
    fn test_7d22_call_ash_report() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "ash_report", "arguments": {"duration_secs": 30}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("sample_count"));
        assert!(text.contains("300"));
    }

    #[test]
    fn test_7d22_call_ash_report_default() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "ash_report", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("600"));
    }

    #[test]
    fn test_7d22_call_active_sessions() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "active_sessions", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("ACTIVE"));
        assert!(text.contains("admin"));
    }

    #[test]
    fn test_7d22_call_pprof_dump() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(5)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "pprof_dump", "arguments": {"duration_secs": 10}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("sample_count"));
        assert!(text.contains("1000"));
        assert!(text.contains("btree"));
    }

    // -----------------------------------------------------------------
    //  10. 类别 6: Maintenance 工具调用测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_call_vacuum_table() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "vacuum_table", "arguments": {"table": "products"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("dead_tuples_reclaimed"));
        assert!(text.contains("products"));
    }

    #[test]
    fn test_7d22_call_vacuum_table_not_found() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "vacuum_table", "arguments": {"table": "nonexistent"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
    }

    #[test]
    fn test_7d22_call_analyze_table() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "analyze_table", "arguments": {"table": "orders"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("rows_analyzed"));
        assert!(text.contains("5000"));
    }

    #[test]
    fn test_7d22_call_autovacuum_status() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "autovacuum_status", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("enabled"));
        assert!(text.contains("true"));
        assert!(text.contains("tables_vacuumed"));
    }

    // -----------------------------------------------------------------
    //  11. 类别 7: Alerting 工具调用测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_call_list_alerts() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "list_alerts", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("warning"));
        assert!(text.contains("high_qps"));
    }

    #[test]
    fn test_7d22_call_db_stats() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "db_stats", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("table_count"));
        assert!(text.contains("total_rows"));
        assert!(text.contains("cache_hit_rate"));
    }

    #[test]
    fn test_7d22_call_capacity_predict() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "capacity_predict", "arguments": {"days": 30}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("disk_usage_gb"));
        assert!(text.contains("predicted_value"));
        assert!(text.contains("65"));
    }

    #[test]
    fn test_7d22_call_capacity_predict_default() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "capacity_predict", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("65"));
    }

    // -----------------------------------------------------------------
    //  12. 错误处理测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_call_unknown_tool() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "nonexistent_tool", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32603);
    }

    #[test]
    fn test_7d22_call_missing_name() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({"arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[test]
    fn test_7d22_call_missing_params() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[test]
    fn test_7d22_method_not_found() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "nonexistent/method".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    // -----------------------------------------------------------------
    //  13. shutdown 测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_shutdown() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "shutdown".to_string(),
            params: None,
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        assert!(!server.initialized);
    }

    // -----------------------------------------------------------------
    //  14. handle_request_json_v2 便捷函数测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_handle_request_json_initialize() {
        let mut server = McpServerV2::default();
        let response = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );
        assert!(response.contains("protocolVersion"));
    }

    #[test]
    fn test_7d22_handle_request_json_invalid_json() {
        let mut server = McpServerV2::default();
        let response = handle_request_json_v2(&mut server, "not valid json");
        assert!(response.contains("-32700"));
    }

    // -----------------------------------------------------------------
    //  15. MockBackendV2 直接测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_mock_backend_list_tables() {
        let backend = MockBackendV2::default();
        let tables = backend.list_tables().unwrap();
        assert!(tables.len() >= 2);
    }

    #[test]
    fn test_7d22_mock_backend_list_indexes() {
        let backend = MockBackendV2::default();
        let indexes = backend.list_indexes("products").unwrap();
        assert!(!indexes.is_empty());
        assert!(indexes[0].is_primary);
    }

    #[test]
    fn test_7d22_mock_backend_list_views() {
        let backend = MockBackendV2::default();
        let views = backend.list_views().unwrap();
        assert!(!views.is_empty());
    }

    #[test]
    fn test_7d22_mock_backend_explain_query() {
        let backend = MockBackendV2::default();
        let plan = backend
            .explain_query("SELECT * FROM products WHERE id = 1")
            .unwrap();
        assert!(plan.cost > 0.0);
        assert!(!plan.operators.is_empty());
    }

    #[test]
    fn test_7d22_mock_backend_slow_queries() {
        let backend = MockBackendV2::default();
        let queries = backend.slow_queries(10).unwrap();
        assert!(!queries.is_empty());
        assert!(queries[0].elapsed_ms >= 200);
    }

    #[test]
    fn test_7d22_mock_backend_top_queries() {
        let backend = MockBackendV2::default();
        let queries = backend.top_queries(10).unwrap();
        assert!(!queries.is_empty());
    }

    #[test]
    fn test_7d22_mock_backend_query_stats() {
        let backend = MockBackendV2::default();
        let stats = backend.query_stats().unwrap();
        assert!(stats.total_queries > 0);
    }

    #[test]
    fn test_7d22_mock_backend_wait_events() {
        let backend = MockBackendV2::default();
        let events = backend.wait_events().unwrap();
        assert!(!events.is_empty());
    }

    #[test]
    fn test_7d22_mock_backend_vacuum_table() {
        let backend = MockBackendV2::default();
        let result = backend.vacuum_table("products").unwrap();
        assert!(result.dead_tuples_reclaimed > 0);
    }

    #[test]
    fn test_7d22_mock_backend_capacity_predict() {
        let backend = MockBackendV2::default();
        let forecast = backend.capacity_predict(30).unwrap();
        assert!(forecast.predicted_value > forecast.current_value);
        assert!(forecast.confidence > 0.0);
    }

    // -----------------------------------------------------------------
    //  16. 完整 LLM 工作流模拟（端到端）
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_full_llm_workflow() {
        let mut server = McpServerV2::default();

        // Step 1: initialize
        let r1 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );
        assert!(r1.contains("protocolVersion"));

        // Step 2: tools/list → 30 tools
        let r2 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        );
        assert!(r2.contains("list_tables"));
        assert!(r2.contains("execute_sql"));
        assert!(r2.contains("slow_queries"));
        assert!(r2.contains("autovacuum_status"));
        assert!(r2.contains("capacity_predict"));
        assert!(r2.contains("summarize_table"));
        assert!(r2.contains("ask_data"));
        assert!(r2.contains("explain_root_cause"));
        assert!(r2.contains("get_lineage"));

        // Step 3: list_tables
        let r3 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_tables","arguments":{}}}"#,
        );
        assert!(r3.contains("products"));

        // Step 4: execute_sql
        let r4 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"execute_sql","arguments":{"sql":"SELECT * FROM products"}}}"#,
        );
        assert!(r4.contains("苹果汁"));

        // Step 5: slow_queries
        let r5 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"slow_queries","arguments":{"limit":5}}}"#,
        );
        assert!(r5.contains("SELECT * FROM orders"));

        // Step 6: ask_data（TDengine 启发 — Agent Interface 统一入口）
        let r6 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"ask_data","arguments":{"question":"有多少商品？"}}}"#,
        );
        assert!(r6.contains("products"));

        // Step 7: explain_root_cause（TDengine 启发 — 根因分析）
        let r7 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"explain_root_cause","arguments":{"alert_id":"high_qps"}}}"#,
        );
        assert!(r7.contains("likely_causes"));

        // Step 7.5: get_lineage（TDengine 启发 — 数据血缘追踪）
        let r7_5 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_lineage","arguments":{"table":"orders"}}}"#,
        );
        assert!(r7_5.contains("upstream"));

        // Step 8: shutdown
        let r8 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":8,"method":"shutdown"}"#,
        );
        assert!(r8.contains("result"));
    }

    #[test]
    fn test_7d22_all_30_tools_callable() {
        let mut server = McpServerV2::default();
        server.initialized = true;

        let tool_calls = [
            ("list_tables", json!({})),
            ("describe_table", json!({"table": "products"})),
            ("list_indexes", json!({"table": "products"})),
            ("list_views", json!({})),
            ("execute_sql", json!({"sql": "SELECT 1"})),
            ("explain_query", json!({"sql": "SELECT 1"})),
            (
                "prepare_statement",
                json!({"name": "s1", "sql": "SELECT ?"}),
            ),
            ("cancel_query", json!({"query_id": 1})),
            ("slow_queries", json!({})),
            ("top_queries", json!({})),
            ("query_stats", json!({})),
            ("reset_stats", json!({})),
            ("list_transactions", json!({})),
            ("list_locks", json!({})),
            ("kill_transaction", json!({"txn_id": 1001})),
            ("deadlock_history", json!({})),
            ("wait_events", json!({})),
            ("ash_report", json!({})),
            ("active_sessions", json!({})),
            ("pprof_dump", json!({})),
            ("vacuum_table", json!({"table": "products"})),
            ("analyze_table", json!({"table": "products"})),
            ("autovacuum_status", json!({})),
            ("list_alerts", json!({})),
            ("db_stats", json!({})),
            ("capacity_predict", json!({})),
            ("summarize_table", json!({"table": "products"})),
            ("ask_data", json!({"question": "有多少商品？"})),
            ("explain_root_cause", json!({"alert_id": "high_qps"})),
            ("get_lineage", json!({"table": "orders"})),
        ];

        for (name, args) in &tool_calls {
            let req = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(json!(1)),
                method: "tools/call".to_string(),
                params: Some(json!({"name": name, "arguments": args})),
            };
            let resp = server.handle_request(&req);
            assert!(
                resp.error.is_none(),
                "tool '{}' returned error: {:?}",
                name,
                resp.error
            );
        }
    }

    // -----------------------------------------------------------------
    //  17. 自定义后端测试
    // -----------------------------------------------------------------

    struct EmptyBackend;

    impl McpBackendV2 for EmptyBackend {
        fn list_tables(&self) -> Result<Vec<crate::mcp::TableInfo>, McpError> {
            Ok(vec![])
        }
        fn describe_table(&self, _table: &str) -> Result<crate::mcp::TableSchema, McpError> {
            Err(McpError::BackendError("empty backend".to_string()))
        }
        fn list_indexes(&self, _table: &str) -> Result<Vec<IndexInfo>, McpError> {
            Ok(vec![])
        }
        fn list_views(&self) -> Result<Vec<ViewInfo>, McpError> {
            Ok(vec![])
        }
        fn execute_sql(&self, _sql: &str) -> Result<crate::mcp::QueryResult, McpError> {
            Err(McpError::BackendError("empty backend".to_string()))
        }
        fn explain_query(&self, _sql: &str) -> Result<ExplainPlan, McpError> {
            Err(McpError::BackendError("empty backend".to_string()))
        }
        fn prepare_statement(&self, _name: &str, _sql: &str) -> Result<PrepareResult, McpError> {
            Err(McpError::BackendError("empty backend".to_string()))
        }
        fn cancel_query(&self, query_id: u64) -> Result<CancelResult, McpError> {
            Ok(CancelResult {
                query_id,
                cancelled: false,
            })
        }
        fn slow_queries(&self, _limit: usize) -> Result<Vec<SlowQueryRecord>, McpError> {
            Ok(vec![])
        }
        fn top_queries(&self, _limit: usize) -> Result<Vec<TopQueryRecord>, McpError> {
            Ok(vec![])
        }
        fn query_stats(&self) -> Result<QueryStatsSummary, McpError> {
            Ok(QueryStatsSummary {
                total_queries: 0,
                total_time_ms: 0.0,
                unique_queries: 0,
                avg_time_ms: 0.0,
            })
        }
        fn reset_stats(&self) -> Result<ResetResult, McpError> {
            Ok(ResetResult { reset: false })
        }
        fn list_transactions(&self) -> Result<Vec<TransactionInfo>, McpError> {
            Ok(vec![])
        }
        fn list_locks(&self) -> Result<Vec<LockInfo>, McpError> {
            Ok(vec![])
        }
        fn kill_transaction(&self, txn_id: u32) -> Result<KillResult, McpError> {
            Ok(KillResult {
                txn_id,
                killed: false,
            })
        }
        fn deadlock_history(&self) -> Result<Vec<DeadlockRecord>, McpError> {
            Ok(vec![])
        }
        fn wait_events(&self) -> Result<Vec<WaitEventSummary>, McpError> {
            Ok(vec![])
        }
        fn ash_report(&self, duration_secs: u64) -> Result<AshReport, McpError> {
            Ok(AshReport {
                duration_secs,
                sample_count: 0,
                top_sql: vec![],
                top_wait_events: vec![],
            })
        }
        fn active_sessions(&self) -> Result<Vec<SessionInfo>, McpError> {
            Ok(vec![])
        }
        fn pprof_dump(&self, duration_secs: u64) -> Result<PprofResult, McpError> {
            Ok(PprofResult {
                sample_count: 0,
                duration_secs,
                top_functions: vec![],
            })
        }
        fn vacuum_table(&self, _table: &str) -> Result<VacuumResult, McpError> {
            Err(McpError::BackendError("empty backend".to_string()))
        }
        fn analyze_table(&self, _table: &str) -> Result<AnalyzeResult, McpError> {
            Err(McpError::BackendError("empty backend".to_string()))
        }
        fn autovacuum_status(&self) -> Result<AutovacuumStatus, McpError> {
            Ok(AutovacuumStatus {
                enabled: false,
                last_run: 0,
                tables_vacuumed: 0,
                tables_analyzed: 0,
            })
        }
        fn list_alerts(&self) -> Result<Vec<AlertInfo>, McpError> {
            Ok(vec![])
        }
        fn db_stats(&self) -> Result<crate::mcp::DbStats, McpError> {
            Ok(crate::mcp::DbStats {
                table_count: 0,
                total_rows: 0,
                total_size_bytes: 0,
                cache_hit_rate: 0.0,
                active_connections: 0,
            })
        }
        fn capacity_predict(&self, days: u32) -> Result<CapacityForecast, McpError> {
            Ok(CapacityForecast {
                metric: "none".to_string(),
                current_value: 0.0,
                predicted_value: 0.0,
                days_ahead: days,
                confidence: 0.0,
                storage_bytes_current: None,
                storage_bytes_predicted: None,
                net_growth_rate_per_day: None,
                table_breakdown: None,
            })
        }
        fn summarize_table(&self, _table: &str) -> Result<TableSummary, McpError> {
            Err(McpError::BackendError("empty backend".to_string()))
        }
        fn ask_data(&self, _question: &str) -> Result<AskAnswer, McpError> {
            Ok(AskAnswer {
                answer: "empty backend".to_string(),
                sql: None,
                citations: vec![],
            })
        }
        fn explain_root_cause(&self, _alert_id: &str) -> Result<RootCauseReport, McpError> {
            Err(McpError::BackendError("empty backend".to_string()))
        }
        fn get_lineage(&self, _table: Option<&str>) -> Result<LineageInfo, McpError> {
            Ok(LineageInfo {
                table: None,
                upstream: vec![],
                downstream: vec![],
                tables: vec![],
                total_edges: 0,
            })
        }
    }

    #[test]
    fn test_7d22_custom_backend() {
        let mut server = McpServerV2::new(Box::new(EmptyBackend));
        server.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "list_tables", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(text, "[]");
    }

    #[test]
    fn test_7d22_custom_backend_empty_results() {
        let mut server = McpServerV2::new(Box::new(EmptyBackend));
        server.initialized = true;

        // describe_table on empty backend should error
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "describe_table", "arguments": {"table": "x"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());

        // autovacuum_status on empty backend should return disabled
        let req2 = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "autovacuum_status", "arguments": {}})),
        };
        let resp2 = server.handle_request(&req2);
        assert!(resp2.error.is_none());
        let result2 = resp2.result.unwrap();
        let text = result2["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("false"));
    }

    // -----------------------------------------------------------------
    //  17b. Insight 工具测试（TDengine 启发 — P1 新增）
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_summarize_table_products() {
        let backend = MockBackendV2::default();
        let summary = backend.summarize_table("products").unwrap();
        assert_eq!(summary.table, "products");
        assert_eq!(summary.row_count, 1000);
        assert_eq!(summary.columns.len(), 3);

        // id 列：主键，无 NULL，distinct_count = row_count
        let id_col = &summary.columns[0];
        assert_eq!(id_col.name, "id");
        assert_eq!(id_col.null_count, 0);
        assert_eq!(id_col.distinct_count, 1000);

        // price 列：nullable，有 NULL
        let price_col = &summary.columns[2];
        assert_eq!(price_col.name, "price");
        assert!(price_col.null_count > 0);
        assert!(price_col.min_value.is_some());
        assert!(price_col.max_value.is_some());
    }

    #[test]
    fn test_7d22_summarize_table_not_found() {
        let backend = MockBackendV2::default();
        let result = backend.summarize_table("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_7d22_summarize_table_via_mcp() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "summarize_table", "arguments": {"table": "orders"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("orders"));
        assert!(text.contains("5000"));
    }

    #[test]
    fn test_7d22_summarize_table_missing_arg() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "summarize_table", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
    }

    #[test]
    fn test_7d22_ask_data_products() {
        let backend = MockBackendV2::default();
        let answer = backend.ask_data("有多少商品？").unwrap();
        assert!(answer.answer.contains("products"));
        assert!(answer.sql.is_some());
        assert!(!answer.citations.is_empty());
    }

    #[test]
    fn test_7d22_ask_data_orders() {
        let backend = MockBackendV2::default();
        let answer = backend.ask_data("订单总数").unwrap();
        assert!(answer.answer.contains("orders"));
        assert!(answer.sql.is_some());
    }

    #[test]
    fn test_7d22_ask_data_slow_query() {
        let backend = MockBackendV2::default();
        let answer = backend.ask_data("慢查询有哪些").unwrap();
        assert!(answer.answer.contains("慢查询"));
        assert!(answer.sql.is_none());
    }

    #[test]
    fn test_7d22_ask_data_no_match() {
        let backend = MockBackendV2::default();
        let answer = backend.ask_data("天气怎么样").unwrap();
        assert!(answer.answer.contains("暂无法"));
        assert!(answer.sql.is_none());
    }

    #[test]
    fn test_7d22_ask_data_via_mcp() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "ask_data", "arguments": {"question": "有多少商品？"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("products"));
        assert!(text.contains("citations"));
    }

    #[test]
    fn test_7d22_ask_data_missing_arg() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "ask_data", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
    }

    #[test]
    fn test_7d22_insight_category_tools() {
        let server = McpServerV2::default();
        let insight_tools = server.tools_by_category(ToolCategory::Insight);
        assert_eq!(insight_tools.len(), 4);
        let names: Vec<&str> = insight_tools.iter().map(|t| t.base.name.as_str()).collect();
        assert!(names.contains(&"summarize_table"));
        assert!(names.contains(&"ask_data"));
        assert!(names.contains(&"explain_root_cause"));
        assert!(names.contains(&"get_lineage"));
    }

    #[test]
    fn test_7d22_insight_dto_serialization() {
        let summary = TableSummary {
            table: "test".to_string(),
            row_count: 100,
            columns: vec![ColumnSummary {
                name: "id".to_string(),
                data_type: "BIGINT".to_string(),
                null_count: 0,
                distinct_count: 100,
                min_value: Some("1".to_string()),
                max_value: Some("100".to_string()),
                top_values: vec![],
            }],
        };
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("test"));
        assert!(json.contains("BIGINT"));

        let answer = AskAnswer {
            answer: "test answer".to_string(),
            sql: Some("SELECT 1".to_string()),
            citations: vec![AskCitation {
                table: "t".to_string(),
                row_id: 1,
                snippet: "test".to_string(),
                score: 0.9,
            }],
        };
        let json = serde_json::to_string(&answer).unwrap();
        assert!(json.contains("test answer"));
        assert!(json.contains("citations"));
    }

    // -----------------------------------------------------------------
    //  17c. RootCause explain_root_cause 工具测试（TDengine 启发 — P4）
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_root_cause_high_qps() {
        let backend = MockBackendV2::default();
        let report = backend.explain_root_cause("high_qps").unwrap();
        assert_eq!(report.alert.rule_id, "high_qps");
        // high_qps 应至少有 HighQps 原因
        assert!(report
            .likely_causes
            .iter()
            .any(|c| c.cause_type == CauseType::HighQps));
        // 应有关联慢查询证据
        assert!(report.evidence.iter().any(|e| e.source == "alert"));
    }

    #[test]
    fn test_7d22_root_cause_alert_not_found() {
        let backend = MockBackendV2::default();
        let result = backend.explain_root_cause("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_7d22_root_cause_via_mcp() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(
                json!({"name": "explain_root_cause", "arguments": {"alert_id": "high_qps"}}),
            ),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("likely_causes"));
        assert!(text.contains("high_qps"));
    }

    #[test]
    fn test_7d22_root_cause_missing_arg() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "explain_root_cause", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some());
    }

    #[test]
    fn test_7d22_root_cause_report_serialization() {
        let report = RootCauseReport {
            alert: AlertInfo {
                level: "warning".to_string(),
                rule_id: "test".to_string(),
                message: "test alert".to_string(),
                timestamp: 1700000000,
                value: 100.0,
                threshold: 50.0,
            },
            likely_causes: vec![CauseEntry {
                cause_type: CauseType::MissingIndex,
                description: "test cause".to_string(),
                confidence: 0.8,
            }],
            evidence: vec![Evidence {
                source: "test".to_string(),
                detail: "test detail".to_string(),
            }],
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("likely_causes"));
        assert!(json.contains("MissingIndex"));
        assert!(json.contains("test cause"));
    }

    #[test]
    fn test_7d22_cause_type_serialization() {
        let types = vec![
            CauseType::MissingIndex,
            CauseType::LockContention,
            CauseType::HighQps,
            CauseType::StatsStale,
            CauseType::Deadlock,
        ];
        let json = serde_json::to_string(&types).unwrap();
        assert!(json.contains("MissingIndex"));
        assert!(json.contains("LockContention"));
        assert!(json.contains("Deadlock"));
        // 反序列化验证
        let deserialized: Vec<CauseType> = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.len(), 5);
        assert_eq!(deserialized[0], CauseType::MissingIndex);
    }

    // -----------------------------------------------------------------
    //  17d. Lineage get_lineage 工具测试（TDengine 启发 — P5 新增）
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_lineage_get_all() {
        // 不传 table → 返回全量血缘
        let backend = MockBackendV2::default();
        let info = backend.get_lineage(None).unwrap();
        assert!(info.table.is_none());
        // Mock 共 3 条边
        assert_eq!(info.upstream.len(), 3);
        assert!(info.downstream.is_empty());
        assert_eq!(info.total_edges, 3);
        // 涉及 3 张表
        assert_eq!(info.tables.len(), 3);
        assert!(info.tables.contains(&"products".to_string()));
        assert!(info.tables.contains(&"orders".to_string()));
        assert!(info.tables.contains(&"order_items".to_string()));
    }

    #[test]
    fn test_7d22_lineage_get_for_orders() {
        // orders 表：上游来自 products（2 条），下游为空
        let backend = MockBackendV2::default();
        let info = backend.get_lineage(Some("orders")).unwrap();
        assert_eq!(info.table.as_deref(), Some("orders"));
        assert_eq!(
            info.upstream.len(),
            2,
            "orders has 2 upstream edges from products"
        );
        assert!(
            info.downstream.is_empty(),
            "orders has no downstream in mock"
        );
        // 上游边都来自 products
        for e in &info.upstream {
            assert_eq!(e.source.table, "products");
            assert_eq!(e.target.table, "orders");
        }
    }

    #[test]
    fn test_7d22_lineage_get_for_products() {
        // products 表：上游为空，下游有 3 条边（2 条到 orders，1 条到 order_items）
        let backend = MockBackendV2::default();
        let info = backend.get_lineage(Some("products")).unwrap();
        assert_eq!(info.table.as_deref(), Some("products"));
        assert!(info.upstream.is_empty(), "products has no upstream");
        assert_eq!(info.downstream.len(), 3, "products has 3 downstream edges");
        let target_tables: Vec<&str> = info
            .downstream
            .iter()
            .map(|e| e.target.table.as_str())
            .collect();
        assert!(target_tables.contains(&"orders"));
        assert!(target_tables.contains(&"order_items"));
        // 2 条到 orders
        let orders_count = target_tables.iter().filter(|t| **t == "orders").count();
        assert_eq!(orders_count, 2);
        // 1 条到 order_items
        let order_items_count = target_tables
            .iter()
            .filter(|t| **t == "order_items")
            .count();
        assert_eq!(order_items_count, 1);
    }

    #[test]
    fn test_7d22_lineage_get_unknown_table() {
        // 未知表 — 上游下游都为空，但 total_edges 仍报告全量
        let backend = MockBackendV2::default();
        let info = backend.get_lineage(Some("nonexistent")).unwrap();
        assert_eq!(info.table.as_deref(), Some("nonexistent"));
        assert!(info.upstream.is_empty());
        assert!(info.downstream.is_empty());
        assert_eq!(info.total_edges, 3, "total_edges is global count");
    }

    #[test]
    fn test_7d22_lineage_via_mcp() {
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "get_lineage", "arguments": {"table": "orders"}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("upstream"));
        assert!(text.contains("products"));
        assert!(text.contains("orders"));
        assert!(text.contains("SUM(price)"));
    }

    #[test]
    fn test_7d22_lineage_via_mcp_no_args() {
        // 不传 arguments 中的 table → 返回全量
        let mut server = McpServerV2::default();
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "get_lineage", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("total_edges"));
    }

    #[test]
    fn test_7d22_lineage_empty_backend() {
        // EmptyBackend.get_lineage 返回空结构
        let backend = EmptyBackend;
        let info = backend.get_lineage(None).unwrap();
        assert!(info.upstream.is_empty());
        assert!(info.downstream.is_empty());
        assert!(info.tables.is_empty());
        assert_eq!(info.total_edges, 0);
    }

    #[test]
    fn test_7d22_lineage_dto_serialization() {
        let edge = LineageEdgeDto {
            source: ColumnRefDto {
                table: "a".to_string(),
                column: "x".to_string(),
            },
            target: ColumnRefDto {
                table: "b".to_string(),
                column: "y".to_string(),
            },
            transform: "SUM(x)".to_string(),
            source_type: LineageEdgeSource::Ctas,
        };
        let json = serde_json::to_string(&edge).unwrap();
        assert!(json.contains("SUM(x)"));
        assert!(json.contains("\"ctas\""));
        // 反序列化验证
        let de: LineageEdgeDto = serde_json::from_str(&json).unwrap();
        assert_eq!(de, edge);

        // LineageInfo 序列化
        let info = LineageInfo {
            table: Some("orders".to_string()),
            upstream: vec![edge],
            downstream: vec![],
            tables: vec!["a".to_string(), "b".to_string()],
            total_edges: 1,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("upstream"));
        assert!(json.contains("total_edges"));
    }

    #[test]
    fn test_7d22_lineage_edge_source_as_str() {
        assert_eq!(LineageEdgeSource::Ctas.as_str(), "ctas");
        assert_eq!(LineageEdgeSource::View.as_str(), "view");
        assert_eq!(LineageEdgeSource::Cdc.as_str(), "cdc");
        assert_eq!(LineageEdgeSource::Manual.as_str(), "manual");
    }

    // -----------------------------------------------------------------
    //  18. DTO 序列化测试
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_dto_serialization() {
        let index = IndexInfo {
            name: "idx_test".to_string(),
            table: "t".to_string(),
            columns: vec!["c1".to_string()],
            unique: true,
            is_primary: false,
        };
        let json = serde_json::to_string(&index).unwrap();
        assert!(json.contains("idx_test"));

        let view = ViewInfo {
            name: "v".to_string(),
            definition: "SELECT 1".to_string(),
            owner: "admin".to_string(),
        };
        let json = serde_json::to_string(&view).unwrap();
        assert!(json.contains("SELECT 1"));

        let plan = ExplainPlan {
            sql: "SELECT 1".to_string(),
            cost: 1.5,
            rows: 1,
            operators: vec!["Seq Scan".to_string()],
        };
        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("Seq Scan"));

        let forecast = CapacityForecast {
            metric: "disk".to_string(),
            current_value: 10.0,
            predicted_value: 20.0,
            days_ahead: 30,
            confidence: 0.95,
            storage_bytes_current: None,
            storage_bytes_predicted: None,
            net_growth_rate_per_day: None,
            table_breakdown: None,
        };
        let json = serde_json::to_string(&forecast).unwrap();
        assert!(json.contains("0.95"));
    }

    #[test]
    fn test_7d22_tool_definition_v2_serialization() {
        let def = ToolDefinitionV2 {
            base: ToolDefinition {
                name: "test_tool".to_string(),
                description: "test".to_string(),
                input_schema: json!({"type": "object"}),
            },
            category: ToolCategory::Schema,
        };
        let json = serde_json::to_string(&def).unwrap();
        assert!(json.contains("test_tool"));
        assert!(json.contains("schema"));
    }

    // -----------------------------------------------------------------
    //  CatalogBackend 测试 — Phase TDengine-P3-MVP
    //
    // 验证 CatalogBackend 通过真实 catalog 提供 4 个 Schema 类工具的真实元数据：
    // - list_tables / describe_table / list_indexes / list_views
    // - 其余 26 个方法返回空/Err（MVP 限制）
    // -----------------------------------------------------------------

    /// 构建测试用 catalog（含 2 张表 + 1 个索引 + 列注释）
    ///
    /// 表结构：
    /// - `users`（id BIGINT PK, name TEXT NOT NULL, email TEXT）
    ///   - 索引：`users_email_key`（UNIQUE on email）
    ///   - 列注释：`users.name` = '用户名'
    /// - `orders`（order_id BIGINT PK, user_id BIGINT, total DECIMAL(10,2)）
    fn build_test_catalog() -> szrsql_catalog::ManagedCatalog {
        use szrsql_catalog::MutableCatalog;
        use szrsql_sql::ast::{ColumnDefinition, IndexColumn, TableName};
        use szrsql_sql::plan::TableSchema;
        use szrsql_types::value::ColumnType;

        let mut catalog = szrsql_catalog::ManagedCatalog::new();

        // 表 1: users
        let users_schema = TableSchema {
            name: TableName::new("users"),
            columns: vec![
                {
                    let mut col = ColumnDefinition::new("id", ColumnType::Int64);
                    col.primary_key = true;
                    col.not_null = true;
                    col
                },
                {
                    let mut col = ColumnDefinition::new("name", ColumnType::Text);
                    col.not_null = true;
                    col
                },
                ColumnDefinition::new("email", ColumnType::Text),
            ],
        };
        catalog
            .create_table(users_schema, false)
            .expect("create users table");

        // users 表的唯一索引
        let users_idx = szrsql_catalog::IndexInfo::new_unique(
            "users_email_key",
            TableName::new("users"),
            vec![IndexColumn::new("email")],
        );
        catalog
            .create_index(users_idx, false)
            .expect("create users_email_key index");

        // 列注释：users.name = '用户名'
        catalog
            .set_column_comment(&TableName::new("users"), "name", Some("用户名".to_string()))
            .expect("set column comment");

        // 表 2: orders
        let orders_schema = TableSchema {
            name: TableName::new("orders"),
            columns: vec![
                {
                    let mut col = ColumnDefinition::new("order_id", ColumnType::Int64);
                    col.primary_key = true;
                    col.not_null = true;
                    col
                },
                ColumnDefinition::new("user_id", ColumnType::Int64),
                ColumnDefinition::new(
                    "total",
                    ColumnType::Decimal {
                        precision: 10,
                        scale: 2,
                    },
                ),
            ],
        };
        catalog
            .create_table(orders_schema, false)
            .expect("create orders table");

        catalog
    }

    #[test]
    fn test_catalog_backend_list_tables_real() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let tables = backend.list_tables().expect("list_tables must succeed");
        // 应返回 2 张表：users 和 orders
        assert_eq!(tables.len(), 2, "must list 2 tables");
        let names: Vec<String> = tables.iter().map(|t| t.name.clone()).collect();
        assert!(names.contains(&"users".to_string()), "must contain users");
        assert!(names.contains(&"orders".to_string()), "must contain orders");
        // MVP 未连接 storage，row_count 和 size_bytes 应为 0
        for t in &tables {
            assert_eq!(t.row_count, 0, "row_count should be 0 in MVP");
            assert_eq!(t.size_bytes, 0, "size_bytes should be 0 in MVP");
        }
    }

    #[test]
    fn test_catalog_backend_describe_table_real_with_comment() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let schema = backend
            .describe_table("users")
            .expect("describe_table users must succeed");

        assert_eq!(schema.table, "users");
        assert_eq!(schema.columns.len(), 3, "users table has 3 columns");

        // 验证 id 列
        let id_col = &schema.columns[0];
        assert_eq!(id_col.name, "id");
        assert_eq!(id_col.data_type, "BIGINT");
        assert!(!id_col.nullable, "id is PK, should be non-nullable");
        assert!(id_col.primary_key, "id should be primary key");
        assert!(id_col.comment.is_none(), "id has no comment");

        // 验证 name 列（含 COMMENT ON COLUMN 设置的注释）
        let name_col = &schema.columns[1];
        assert_eq!(name_col.name, "name");
        assert_eq!(name_col.data_type, "TEXT");
        assert!(!name_col.nullable, "name is NOT NULL");
        assert!(!name_col.primary_key);
        assert_eq!(
            name_col.comment.as_deref(),
            Some("用户名"),
            "name column should have comment from COMMENT ON COLUMN"
        );

        // 验证 email 列
        let email_col = &schema.columns[2];
        assert_eq!(email_col.name, "email");
        assert_eq!(email_col.data_type, "TEXT");
        assert!(email_col.nullable, "email is nullable");
        assert!(!email_col.primary_key);
        assert!(email_col.comment.is_none());
    }

    #[test]
    fn test_catalog_backend_describe_table_decimal_type() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let schema = backend
            .describe_table("orders")
            .expect("describe_table orders must succeed");

        assert_eq!(schema.table, "orders");
        // 验证 DECIMAL 类型转换为字符串
        let total_col = schema
            .columns
            .iter()
            .find(|c| c.name == "total")
            .expect("orders.total column must exist");
        assert_eq!(
            total_col.data_type, "DECIMAL(10,2)",
            "Decimal type should be formatted as DECIMAL(p,s)"
        );
    }

    #[test]
    fn test_catalog_backend_describe_table_not_found() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let result = backend.describe_table("nonexistent");
        assert!(result.is_err(), "describe_table on nonexistent must error");
        match result {
            Err(McpError::BackendError(msg)) => {
                assert!(
                    msg.contains("table not found"),
                    "error message should mention table not found"
                );
                assert!(msg.contains("nonexistent"));
            }
            _ => panic!("expected BackendError, got: {:?}", result),
        }
    }

    #[test]
    fn test_catalog_backend_list_indexes_real() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let indexes = backend
            .list_indexes("users")
            .expect("list_indexes users must succeed");
        // users 表有 1 个唯一索引：users_email_key
        assert_eq!(indexes.len(), 1, "users table has 1 index");
        let idx = &indexes[0];
        assert_eq!(idx.name, "users_email_key");
        assert_eq!(idx.table, "users");
        assert!(idx.unique, "users_email_key is UNIQUE");
        assert!(!idx.is_primary, "users_email_key is not primary key");
        assert_eq!(idx.columns, vec!["email".to_string()]);
    }

    #[test]
    fn test_catalog_backend_list_indexes_table_without_index() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        // orders 表没有显式创建索引
        let indexes = backend
            .list_indexes("orders")
            .expect("list_indexes orders must succeed");
        assert!(indexes.is_empty(), "orders table has no index");
    }

    #[test]
    fn test_catalog_backend_list_indexes_not_found() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let result = backend.list_indexes("nonexistent");
        assert!(result.is_err(), "list_indexes on nonexistent must error");
    }

    #[test]
    fn test_catalog_backend_list_views_empty() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let views = backend.list_views().expect("list_views must succeed");
        // SzRSQL 不支持 VIEW，list_views 返回空 Vec（语义正确）
        assert!(
            views.is_empty(),
            "SzRSQL does not support VIEW, list_views should be empty"
        );
    }

    #[test]
    fn test_catalog_backend_execute_sql_without_executor() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let result = backend.execute_sql("SELECT * FROM users");
        assert!(result.is_err(), "execute_sql should err without executor");
        match result {
            Err(McpError::BackendError(msg)) => {
                assert!(
                    msg.contains("execute_sql"),
                    "error should mention execute_sql"
                );
                assert!(
                    msg.contains("no executor attached"),
                    "error should mention no executor attached"
                );
            }
            _ => panic!("expected BackendError"),
        }
    }

    #[test]
    fn test_catalog_backend_explain_query_without_executor() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let result = backend.explain_query("SELECT * FROM users");
        assert!(result.is_err(), "explain_query should err without executor");
    }

    #[test]
    fn test_catalog_backend_db_stats_table_count_real() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let stats = backend.db_stats().expect("db_stats must succeed");
        // table_count 应真实化（build_test_catalog 创建了 2 张表）
        assert_eq!(
            stats.table_count, 2,
            "table_count should be real (2 tables)"
        );
        // 其余字段未注入 executor，应为 0
        assert_eq!(stats.total_rows, 0);
        assert_eq!(stats.total_size_bytes, 0);
        assert_eq!(stats.cache_hit_rate, 0.0);
        assert_eq!(stats.active_connections, 0);
    }

    #[test]
    fn test_catalog_backend_insight_tools_without_executor() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        // summarize_table / ask_data / explain_root_cause 未注入 executor，返回 Err
        assert!(backend.summarize_table("users").is_err());
        assert!(backend.ask_data("how many users?").is_err());
        assert!(backend.explain_root_cause("alert_1").is_err());
    }

    #[test]
    fn test_catalog_backend_get_lineage_empty() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        // get_lineage 未连接 LineageStore，返回空 LineageInfo
        let lineage = backend.get_lineage(None).expect("get_lineage must succeed");
        assert!(lineage.upstream.is_empty());
        assert!(lineage.downstream.is_empty());
        assert!(lineage.tables.is_empty());
        assert_eq!(lineage.total_edges, 0);
    }

    #[test]
    fn test_catalog_backend_slow_queries_empty() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        // 未连接运行时状态，slow_queries 返回空
        let slow = backend.slow_queries(10).expect("slow_queries must succeed");
        assert!(slow.is_empty(), "slow_queries should be empty in MVP");
    }

    #[test]
    fn test_catalog_backend_list_transactions_empty() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        let txns = backend
            .list_transactions()
            .expect("list_transactions must succeed");
        assert!(txns.is_empty(), "list_transactions should be empty in MVP");
    }

    #[test]
    fn test_catalog_backend_maintenance_without_executor() {
        let catalog = build_test_catalog();
        let backend = CatalogBackend::new(Box::new(catalog));
        // vacuum_table / analyze_table 未注入 executor，返回 Err
        assert!(backend.vacuum_table("users").is_err());
        assert!(backend.analyze_table("users").is_err());
        // autovacuum_status 返回禁用状态
        let status = backend
            .autovacuum_status()
            .expect("autovacuum_status must succeed");
        assert!(
            !status.enabled,
            "autovacuum should be disabled without executor"
        );
    }

    // -----------------------------------------------------------------
    //  P3-CatalogBackend-Full: with_executor 委托测试
    // -----------------------------------------------------------------

    /// 构建带执行器的 CatalogBackend（catalog + executor 一致）
    fn build_catalog_backend_with_executor() -> CatalogBackend {
        // 先构建 executor 并创建表 + 插入数据
        let executor = ExecutorBackend::new();
        executor
            .execute_sql("CREATE TABLE users (id BIGINT PRIMARY KEY, name TEXT NOT NULL)")
            .expect("CREATE TABLE users");
        executor
            .execute_sql("INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob')")
            .expect("INSERT users");

        // 构建一致的 catalog（同名表 + 同 schema）
        use szrsql_catalog::MutableCatalog;
        use szrsql_sql::ast::{ColumnDefinition, TableName};
        use szrsql_sql::plan::TableSchema;
        use szrsql_types::value::ColumnType;
        let mut catalog = szrsql_catalog::ManagedCatalog::new();
        let users_schema = TableSchema {
            name: TableName::new("users"),
            columns: vec![
                {
                    let mut col = ColumnDefinition::new("id", ColumnType::Int64);
                    col.primary_key = true;
                    col.not_null = true;
                    col
                },
                {
                    let mut col = ColumnDefinition::new("name", ColumnType::Text);
                    col.not_null = true;
                    col
                },
            ],
        };
        catalog
            .create_table(users_schema, false)
            .expect("create users table in catalog");

        CatalogBackend::with_executor(Box::new(catalog), executor)
    }

    #[test]
    fn test_catalog_backend_with_executor_execute_sql() {
        let backend = build_catalog_backend_with_executor();
        // execute_sql 应委托到 executor，返回真实查询结果
        let result = backend
            .execute_sql("SELECT * FROM users")
            .expect("execute_sql via executor must succeed");
        assert_eq!(result.columns.len(), 2, "should return 2 columns");
        assert_eq!(result.rows.len(), 2, "should return 2 rows");
        assert_eq!(result.affected_rows, 0, "SELECT affects 0 rows");
    }

    #[test]
    fn test_catalog_backend_with_executor_runtime_stats() {
        let backend = build_catalog_backend_with_executor();
        // 执行几条 SQL 触发统计采集
        backend.execute_sql("SELECT * FROM users").expect("SELECT");
        backend
            .execute_sql("SELECT * FROM users")
            .expect("SELECT again");

        // query_stats 应委托到 executor，返回真实统计
        let stats = backend.query_stats().expect("query_stats must succeed");
        assert!(
            stats.total_queries >= 2,
            "should have at least 2 queries recorded"
        );

        // slow_queries 应委托到 executor
        let slow = backend.slow_queries(10).expect("slow_queries must succeed");
        // 慢查询阈值默认很大，可能为空，但调用应成功
        let _ = slow;
    }

    #[test]
    fn test_catalog_backend_with_executor_list_transactions() {
        let backend = build_catalog_backend_with_executor();
        // 执行 BEGIN 触发事务采集
        backend.execute_sql("BEGIN").expect("BEGIN");

        // list_transactions 应委托到 executor，返回活动事务
        let txns = backend
            .list_transactions()
            .expect("list_transactions must succeed");
        assert!(
            !txns.is_empty(),
            "should have at least 1 active transaction after BEGIN"
        );

        // COMMIT 后事务应清空
        backend.execute_sql("COMMIT").expect("COMMIT");
        let txns_after = backend
            .list_transactions()
            .expect("list_transactions after COMMIT");
        assert!(
            txns_after.is_empty(),
            "transactions should be empty after COMMIT"
        );
    }

    #[test]
    fn test_catalog_backend_with_executor_maintenance() {
        let backend = build_catalog_backend_with_executor();
        // 执行 DELETE 产生死元组
        backend
            .execute_sql("DELETE FROM users WHERE id = 1")
            .expect("DELETE");

        // vacuum_table 应委托到 executor，清理死元组
        let vacuum_result = backend
            .vacuum_table("users")
            .expect("vacuum_table must succeed");
        assert_eq!(vacuum_result.table, "users");
        assert!(
            vacuum_result.dead_tuples_reclaimed >= 1,
            "should reclaim at least 1 dead tuple"
        );

        // analyze_table 应委托到 executor
        let analyze_result = backend
            .analyze_table("users")
            .expect("analyze_table must succeed");
        assert_eq!(analyze_result.table, "users");

        // autovacuum_status 应反映真实 vacuum/analyze 次数
        let status = backend
            .autovacuum_status()
            .expect("autovacuum_status must succeed");
        assert!(
            status.tables_vacuumed >= 1,
            "should have vacuumed at least 1 table"
        );
    }

    #[test]
    fn test_catalog_backend_with_executor_lineage() {
        let backend = build_catalog_backend_with_executor();
        // 创建目标表，然后 INSERT INTO ... SELECT 记录血缘
        backend
            .execute_sql("CREATE TABLE users_copy (id BIGINT, name TEXT)")
            .expect("CREATE users_copy");
        backend
            .execute_sql("INSERT INTO users_copy SELECT * FROM users")
            .expect("INSERT INTO ... SELECT");

        // get_lineage 应委托到 executor，返回真实血缘
        let lineage = backend
            .get_lineage(Some("users_copy"))
            .expect("get_lineage must succeed");
        assert!(
            !lineage.upstream.is_empty(),
            "users_copy should have upstream lineage from users"
        );
    }

    #[test]
    fn test_catalog_backend_with_executor_db_stats() {
        let backend = build_catalog_backend_with_executor();
        let stats = backend.db_stats().expect("db_stats must succeed");
        // table_count 从 catalog 获取（1 张表）
        assert_eq!(stats.table_count, 1, "table_count from catalog (1 table)");
        // total_rows 从 executor 获取（2 行）
        assert!(
            stats.total_rows >= 2,
            "total_rows from executor (>= 2 rows)"
        );
    }

    #[test]
    fn test_catalog_backend_schema_methods_use_catalog_not_executor() {
        // Schema 方法应使用 catalog，而非 executor
        use szrsql_catalog::MutableCatalog;
        let executor = ExecutorBackend::new();
        executor
            .execute_sql("CREATE TABLE exec_only (id BIGINT)")
            .expect("CREATE exec_only");

        let mut catalog = szrsql_catalog::ManagedCatalog::new();
        use szrsql_sql::ast::{ColumnDefinition, TableName};
        use szrsql_sql::plan::TableSchema;
        use szrsql_types::value::ColumnType;
        catalog
            .create_table(
                TableSchema {
                    name: TableName::new("catalog_only"),
                    columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
                },
                false,
            )
            .expect("create catalog_only");

        let backend = CatalogBackend::with_executor(Box::new(catalog), executor);

        // list_tables 应返回 catalog 中的表（catalog_only），不返回 executor 中的表（exec_only）
        let tables = backend.list_tables().expect("list_tables");
        assert_eq!(tables.len(), 1, "should list 1 table from catalog");
        assert_eq!(
            tables[0].name, "catalog_only",
            "should be catalog_only table"
        );
    }

    #[test]
    fn test_p5_catalog_backend_integrates_catalog_tree() {
        // P5：CatalogBackend 集成层次化数据目录树
        let catalog = build_test_catalog();
        let mut backend = CatalogBackend::new(Box::new(catalog));

        // 1. 初始为空树（仅根节点）
        assert!(backend.catalog_tree().is_empty());

        // 2. 创建业务域目录
        backend
            .catalog_tree_mut()
            .create_dir("/sales")
            .expect("create /sales");
        backend
            .catalog_tree_mut()
            .create_dir("/hr")
            .expect("create /hr");

        // 3. 挂载表到目录
        backend
            .catalog_tree_mut()
            .mount_table("/sales/orders", "users")
            .expect("mount users at /sales/orders");
        backend
            .catalog_tree_mut()
            .mount_table("/hr/employees", "orders")
            .expect("mount orders at /hr/employees");

        // 4. 验证路径查找
        assert_eq!(
            backend
                .catalog_tree()
                .find_path_by_table_name("users")
                .unwrap(),
            "/sales/orders"
        );
        assert_eq!(
            backend
                .catalog_tree()
                .find_path_by_table_name("orders")
                .unwrap(),
            "/hr/employees"
        );

        // 5. 验证 list_children
        let root_children = backend.catalog_tree().list_children("/").expect("list /");
        assert_eq!(root_children.len(), 2);

        // 6. 验证 tree_view BFS
        let view = backend.catalog_tree().tree_view();
        assert_eq!(view.len(), 5); // 根 + 2 目录 + 2 表
        assert_eq!(view[0].path, "/");

        // 7. 移动节点
        backend
            .catalog_tree_mut()
            .move_node("/sales/orders", "/hr")
            .expect("move /sales/orders to /hr");
        assert_eq!(
            backend
                .catalog_tree()
                .find_path_by_table_name("users")
                .unwrap(),
            "/hr/orders"
        );

        // 8. 卸载节点
        backend
            .catalog_tree_mut()
            .unmount("/hr/orders")
            .expect("unmount /hr/orders");
        assert!(backend
            .catalog_tree()
            .find_path_by_table_name("users")
            .is_none());

        // 9. 验证现有 30 个 MCP 工具不受影响
        let tables = backend.list_tables().expect("list_tables");
        assert_eq!(
            tables.len(),
            2,
            "MCP list_tables still works after tree ops"
        );
    }

    #[test]
    fn test_new_with_catalog_constructor() {
        let catalog = build_test_catalog();
        // 使用 new_with_catalog 便捷构造函数
        let server = McpServerV2::new_with_catalog(Box::new(catalog));
        // 验证 server 工具总数仍为 35（与后端无关）
        let tools = server.tool_definitions();
        assert_eq!(tools.len(), 35, "tool count must still be 35");
        // 验证后端为 CatalogBackend（通过行为验证：list_tables 返回真实表清单）
        let backend_tables = server
            .backend
            .list_tables()
            .expect("list_tables via server backend must succeed");
        assert_eq!(backend_tables.len(), 2, "must list 2 real tables");
    }

    #[test]
    fn test_new_with_catalog_handles_list_tables_request() {
        let catalog = build_test_catalog();
        let mut server = McpServerV2::new_with_catalog(Box::new(catalog));
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({"name": "list_tables", "arguments": {}})),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "list_tables request should succeed");
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("users"),
            "response should contain users table"
        );
        assert!(
            text.contains("orders"),
            "response should contain orders table"
        );
    }

    #[test]
    fn test_new_with_catalog_handles_describe_table_request() {
        let catalog = build_test_catalog();
        let mut server = McpServerV2::new_with_catalog(Box::new(catalog));
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "describe_table",
                "arguments": {"table": "users"}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_none(),
            "describe_table request should succeed"
        );
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(text.contains("id"), "should contain id column");
        assert!(text.contains("name"), "should contain name column");
        assert!(
            text.contains("用户名"),
            "should contain column comment '用户名'"
        );
    }

    #[test]
    fn test_new_with_catalog_handles_list_indexes_request() {
        let catalog = build_test_catalog();
        let mut server = McpServerV2::new_with_catalog(Box::new(catalog));
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(3)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "list_indexes",
                "arguments": {"table": "users"}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "list_indexes request should succeed");
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("users_email_key"),
            "should contain index name"
        );
    }

    #[test]
    fn test_new_with_catalog_execute_sql_returns_error() {
        let catalog = build_test_catalog();
        let mut server = McpServerV2::new_with_catalog(Box::new(catalog));
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(4)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "execute_sql",
                "arguments": {"sql": "SELECT * FROM users"}
            })),
        };
        let resp = server.handle_request(&req);
        // CatalogBackend 不支持 execute_sql，应返回后端错误
        assert!(
            resp.error.is_some(),
            "execute_sql should return error in MVP"
        );
        assert_eq!(
            resp.error.unwrap().code,
            -32000,
            "should be BackendError code"
        );
    }

    // -----------------------------------------------------------------
    // ExecutorBackend 测试（Phase TDengine-P3-Full）
    //
    // 覆盖：CREATE/INSERT/SELECT/UPDATE/DELETE/CREATE INDEX/EXPLAIN/
    //       DROP TABLE/TRUNCATE/PREPARE/便捷构造函数/错误场景
    // -----------------------------------------------------------------

    /// 辅助：构造空 ExecutorBackend
    fn build_executor_backend() -> ExecutorBackend {
        ExecutorBackend::new()
    }

    #[test]
    fn test_executor_backend_create_table_and_list() {
        let backend = build_executor_backend();
        // CREATE TABLE
        let result = backend
            .execute_sql("CREATE TABLE users (id BIGINT PRIMARY KEY, name TEXT NOT NULL)")
            .expect("CREATE TABLE must succeed");
        assert_eq!(result.affected_rows, 0, "DDL should affect 0 rows");
        assert!(result.rows.is_empty(), "DDL should return no rows");

        // list_tables 应返回 1 张表
        let tables = backend.list_tables().expect("list_tables must succeed");
        assert_eq!(tables.len(), 1, "must list 1 table");
        assert_eq!(tables[0].name, "users", "table name must be users");
        assert_eq!(tables[0].row_count, 0, "row_count should be 0 after CREATE");

        // describe_table 应返回 2 列
        let schema = backend
            .describe_table("users")
            .expect("describe_table must succeed");
        assert_eq!(schema.table, "users");
        assert_eq!(schema.columns.len(), 2, "users table must have 2 columns");
        assert_eq!(schema.columns[0].name, "id");
        assert!(schema.columns[0].primary_key, "id should be PK");
        assert!(!schema.columns[0].nullable, "PK should not be nullable");
        assert_eq!(schema.columns[1].name, "name");
        assert!(!schema.columns[1].nullable, "name has NOT NULL");
    }

    #[test]
    fn test_executor_backend_insert_and_select() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE users (id BIGINT PRIMARY KEY, name TEXT NOT NULL)")
            .expect("CREATE TABLE");

        // INSERT 单行
        let r1 = backend
            .execute_sql("INSERT INTO users (id, name) VALUES (1, 'Alice')")
            .expect("INSERT 1");
        assert_eq!(r1.affected_rows, 1, "INSERT should affect 1 row");

        // INSERT 多行
        let r2 = backend
            .execute_sql("INSERT INTO users (id, name) VALUES (2, 'Bob'), (3, 'Charlie')")
            .expect("INSERT 2");
        assert_eq!(
            r2.affected_rows, 2,
            "multi-VALUES INSERT should affect 2 rows"
        );

        // list_tables 的 row_count 应反映真实行数
        let tables = backend.list_tables().expect("list_tables");
        assert_eq!(
            tables[0].row_count, 3,
            "row_count should be 3 after inserts"
        );

        // SELECT * 返回所有行
        let sel = backend
            .execute_sql("SELECT * FROM users")
            .expect("SELECT *");
        assert_eq!(sel.rows.len(), 3, "SELECT should return 3 rows");
        // 列应为 id, name
        assert_eq!(sel.columns.len(), 2, "SELECT * should have 2 columns");
        assert_eq!(sel.columns[0], "id");
        assert_eq!(sel.columns[1], "name");

        // 验证第一行数据（id=1, name=Alice）
        assert_eq!(sel.rows[0][0], serde_json::json!(1));
        assert_eq!(sel.rows[0][1], serde_json::json!("Alice"));
    }

    #[test]
    fn test_executor_backend_select_with_filter() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .expect("CREATE");
        backend
            .execute_sql("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'a')")
            .expect("INSERT");

        // WHERE 过滤
        let r = backend
            .execute_sql("SELECT * FROM t WHERE id = 2")
            .expect("SELECT WHERE");
        assert_eq!(r.rows.len(), 1, "WHERE should filter to 1 row");
        assert_eq!(r.rows[0][0], serde_json::json!(2));

        // WHERE 字符串过滤
        let r2 = backend
            .execute_sql("SELECT * FROM t WHERE name = 'a'")
            .expect("SELECT WHERE name");
        assert_eq!(r2.rows.len(), 2, "name='a' should match 2 rows");
    }

    #[test]
    fn test_executor_backend_update() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .expect("CREATE");
        backend
            .execute_sql("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')")
            .expect("INSERT");

        // UPDATE 全表
        let r = backend
            .execute_sql("UPDATE t SET name = 'updated'")
            .expect("UPDATE all");
        assert_eq!(r.affected_rows, 3, "UPDATE should affect 3 rows");

        // 验证更新生效
        let sel = backend.execute_sql("SELECT * FROM t").expect("SELECT");
        for row in &sel.rows {
            assert_eq!(row[1], serde_json::json!("updated"));
        }

        // UPDATE WHERE
        let r2 = backend
            .execute_sql("UPDATE t SET name = 'x' WHERE id = 1")
            .expect("UPDATE WHERE");
        assert_eq!(r2.affected_rows, 1, "UPDATE WHERE should affect 1 row");
    }

    #[test]
    fn test_executor_backend_delete() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .expect("CREATE");
        backend
            .execute_sql("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')")
            .expect("INSERT");

        // DELETE WHERE
        let r = backend
            .execute_sql("DELETE FROM t WHERE id = 2")
            .expect("DELETE WHERE");
        assert_eq!(r.affected_rows, 1, "DELETE should affect 1 row");

        // 验证剩余 2 行
        let sel = backend.execute_sql("SELECT * FROM t").expect("SELECT");
        assert_eq!(sel.rows.len(), 2, "should have 2 rows after DELETE");

        // DELETE 全表
        let r2 = backend.execute_sql("DELETE FROM t").expect("DELETE all");
        assert_eq!(
            r2.affected_rows, 2,
            "DELETE all should affect remaining 2 rows"
        );

        let sel2 = backend.execute_sql("SELECT * FROM t").expect("SELECT");
        assert!(
            sel2.rows.is_empty(),
            "table should be empty after DELETE all"
        );
    }

    #[test]
    fn test_executor_backend_create_and_list_indexes() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT, email TEXT, name TEXT)")
            .expect("CREATE");

        // CREATE INDEX
        let r = backend
            .execute_sql("CREATE INDEX idx_email ON t (email)")
            .expect("CREATE INDEX");
        assert_eq!(r.affected_rows, 0, "DDL should affect 0 rows");

        // list_indexes 应返回 1 个索引
        let indexes = backend.list_indexes("t").expect("list_indexes");
        assert_eq!(indexes.len(), 1, "should list 1 index");
        assert_eq!(indexes[0].name, "idx_email");
        assert!(!indexes[0].unique, "idx_email should not be unique");
        assert_eq!(indexes[0].columns, vec!["email".to_string()]);

        // CREATE UNIQUE INDEX
        backend
            .execute_sql("CREATE UNIQUE INDEX idx_name ON t (name)")
            .expect("CREATE UNIQUE INDEX");

        let indexes2 = backend.list_indexes("t").expect("list_indexes 2");
        assert_eq!(
            indexes2.len(),
            2,
            "should list 2 indexes after second CREATE"
        );
        let unique_idx = indexes2
            .iter()
            .find(|i| i.name == "idx_name")
            .expect("idx_name should exist");
        assert!(unique_idx.unique, "idx_name should be unique");
    }

    #[test]
    fn test_executor_backend_drop_index() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT, email TEXT)")
            .expect("CREATE");
        backend
            .execute_sql("CREATE INDEX idx_email ON t (email)")
            .expect("CREATE INDEX");

        // DROP INDEX
        backend
            .execute_sql("DROP INDEX idx_email")
            .expect("DROP INDEX");

        let indexes = backend.list_indexes("t").expect("list_indexes");
        assert!(indexes.is_empty(), "no indexes should remain after DROP");

        // DROP 不存在的索引（无 IF EXISTS）应报错
        let err = backend.execute_sql("DROP INDEX idx_email").unwrap_err();
        match err {
            McpError::BackendError(msg) => {
                assert!(msg.contains("index not found"), "unexpected error: {msg}");
            }
            other => panic!("expected BackendError, got {other:?}"),
        }
    }

    #[test]
    fn test_executor_backend_drop_table() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t1 (id BIGINT)")
            .expect("CREATE t1");
        backend
            .execute_sql("CREATE TABLE t2 (id BIGINT)")
            .expect("CREATE t2");

        // DROP TABLE
        backend.execute_sql("DROP TABLE t1").expect("DROP TABLE t1");

        let tables = backend.list_tables().expect("list_tables");
        assert_eq!(tables.len(), 1, "should have 1 table after DROP");
        assert_eq!(tables[0].name, "t2", "remaining table should be t2");

        // DROP 不存在的表（无 IF EXISTS）应报错
        let err = backend.execute_sql("DROP TABLE t1").unwrap_err();
        match err {
            McpError::BackendError(msg) => {
                assert!(msg.contains("table not found"), "unexpected error: {msg}");
            }
            other => panic!("expected BackendError, got {other:?}"),
        }

        // DROP TABLE IF EXISTS 不存在时静默跳过
        backend
            .execute_sql("DROP TABLE IF EXISTS nonexistent")
            .expect("DROP TABLE IF EXISTS should not error");
    }

    #[test]
    fn test_executor_backend_truncate() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");
        backend
            .execute_sql("INSERT INTO t (id) VALUES (1), (2), (3), (4), (5)")
            .expect("INSERT");

        // TRUNCATE
        let r = backend.execute_sql("TRUNCATE TABLE t").expect("TRUNCATE");
        assert_eq!(r.affected_rows, 5, "TRUNCATE should report 5 affected rows");

        // 表应为空
        let sel = backend.execute_sql("SELECT * FROM t").expect("SELECT");
        assert!(sel.rows.is_empty(), "table should be empty after TRUNCATE");

        // 但表结构仍存在
        let tables = backend.list_tables().expect("list_tables");
        assert_eq!(tables.len(), 1, "table should still exist after TRUNCATE");
        assert_eq!(tables[0].row_count, 0, "row_count should be 0");
    }

    #[test]
    fn test_executor_backend_explain_query() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .expect("CREATE");
        backend
            .execute_sql("INSERT INTO t (id, name) VALUES (1, 'a')")
            .expect("INSERT");

        // EXPLAIN SELECT
        let plan = backend
            .explain_query("SELECT * FROM t")
            .expect("EXPLAIN SELECT");
        assert_eq!(plan.sql, "SELECT * FROM t");
        assert!(
            !plan.operators.is_empty(),
            "EXPLAIN should produce operators"
        );
        // 应该包含 SeqScan(t)
        let has_scan = plan
            .operators
            .iter()
            .any(|op| op.contains("SeqScan") && op.contains("t"));
        assert!(
            has_scan,
            "EXPLAIN should contain SeqScan(t), got: {:?}",
            plan.operators
        );
    }

    #[test]
    fn test_executor_backend_explain_with_filter() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .expect("CREATE");

        let plan = backend
            .explain_query("SELECT * FROM t WHERE id = 1")
            .expect("EXPLAIN");
        // 应包含 Filter 节点
        let has_filter = plan.operators.iter().any(|op| op.contains("Filter"));
        assert!(
            has_filter,
            "EXPLAIN should contain Filter, got: {:?}",
            plan.operators
        );
    }

    #[test]
    fn test_executor_backend_prepare_statement() {
        let backend = build_executor_backend();

        // PREPARE 可解析的 SQL（无参数）
        let r = backend
            .prepare_statement("stmt1", "SELECT 1")
            .expect("PREPARE");
        assert_eq!(r.name, "stmt1");
        assert_eq!(r.parameter_count, 0, "SELECT 1 has 0 params");

        // PREPARE 空 SQL 应报错
        let err = backend.prepare_statement("stmt2", "").unwrap_err();
        match err {
            McpError::BackendError(msg) => {
                assert!(
                    msg.contains("parse error") || msg.contains("empty"),
                    "unexpected: {msg}"
                );
            }
            other => panic!("expected BackendError, got {other:?}"),
        }

        // PREPARE 语法错误的 SQL 应报错
        let err2 = backend
            .prepare_statement("stmt3", "SELECT FROM WHERE")
            .unwrap_err();
        match err2 {
            McpError::BackendError(msg) => {
                assert!(msg.contains("parse error"), "unexpected: {msg}");
            }
            other => panic!("expected BackendError, got {other:?}"),
        }
    }

    #[test]
    fn test_prepare_statement_parameter_count() {
        // P3-Prepare：验证参数占位符计数
        let backend = build_executor_backend();

        // 1. 无参数 SQL
        let r = backend
            .prepare_statement("s1", "SELECT 1 + 2")
            .expect("no params");
        assert_eq!(r.parameter_count, 0, "SELECT 1 + 2 should have 0 params");

        // 2. 单个 $1 参数 — WHERE 条件
        let r = backend
            .prepare_statement("s2", "SELECT * FROM t WHERE id = $1")
            .expect("one param");
        assert_eq!(r.parameter_count, 1, "WHERE id = $1 should have 1 param");

        // 3. 多个参数 $1, $2 — WHERE + VALUES
        let r = backend
            .prepare_statement("s3", "INSERT INTO t (a, b) VALUES ($1, $2)")
            .expect("two params");
        assert_eq!(r.parameter_count, 2, "VALUES ($1, $2) should have 2 params");

        // 4. 参数在多个位置出现 — 取最大索引
        let r = backend
            .prepare_statement("s4", "SELECT * FROM t WHERE a = $1 OR b = $3")
            .expect("max index");
        assert_eq!(
            r.parameter_count, 3,
            "WHERE a = $1 OR b = $3 should have 3 params (max index)"
        );

        // 5. ? 占位符（解析器仅在 PREPARE/EXECUTE 上下文中支持 ?，
        //    普通 SELECT 不支持，此处跳过 — 见 convert_placeholder）
        // 注：SzRSQL 解析器对 ? 的支持仅限于 PREPARE 语句，普通查询使用 $1/$2 风格

        // 6. UPDATE SET 中的参数
        let r = backend
            .prepare_statement("s6", "UPDATE t SET name = $1 WHERE id = $2")
            .expect("update params");
        assert_eq!(
            r.parameter_count, 2,
            "UPDATE SET $1 WHERE $2 should have 2 params"
        );

        // 7. DELETE 中的参数
        let r = backend
            .prepare_statement("s7", "DELETE FROM t WHERE id = $1")
            .expect("delete param");
        assert_eq!(r.parameter_count, 1, "DELETE WHERE $1 should have 1 param");

        // 8. 参数在复杂表达式中（函数调用 + CASE）
        let r = backend
            .prepare_statement(
                "s8",
                "SELECT CASE WHEN id = $1 THEN 'a' WHEN id = $2 THEN 'b' ELSE 'c' END FROM t",
            )
            .expect("case params");
        assert_eq!(
            r.parameter_count, 2,
            "CASE WHEN $1 ... $2 should have 2 params"
        );

        // 9. 参数在子查询中
        let r = backend
            .prepare_statement(
                "s9",
                "SELECT * FROM t WHERE id IN (SELECT id FROM t2 WHERE x = $1)",
            )
            .expect("subquery param");
        assert_eq!(r.parameter_count, 1, "Subquery with $1 should have 1 param");

        // 10. 参数在 JOIN ON 条件中
        let r = backend
            .prepare_statement(
                "s10",
                "SELECT * FROM t1 JOIN t2 ON t1.id = t2.id WHERE t1.x = $1",
            )
            .expect("join param");
        assert_eq!(
            r.parameter_count, 1,
            "JOIN with $1 in WHERE should have 1 param"
        );

        // 11. 参数在 LIMIT/OFFSET 中
        let r = backend
            .prepare_statement("s11", "SELECT * FROM t LIMIT $1 OFFSET $2")
            .expect("limit params");
        assert_eq!(
            r.parameter_count, 2,
            "LIMIT $1 OFFSET $2 should have 2 params"
        );

        // 12. 参数在 GROUP BY / HAVING 中
        let r = backend
            .prepare_statement(
                "s12",
                "SELECT COUNT(*) FROM t GROUP BY x HAVING COUNT(*) > $1",
            )
            .expect("having param");
        assert_eq!(r.parameter_count, 1, "HAVING $1 should have 1 param");
    }

    #[test]
    fn test_count_parameters_helper() {
        // 直接测试 count_parameters 辅助函数
        use szrsql_sql::parser::parse_sql;

        // 无参数
        let stmts = parse_sql("SELECT 1").expect("parse");
        assert_eq!(count_parameters(&stmts), 0);

        // 单参数
        let stmts = parse_sql("SELECT * FROM t WHERE id = $1").expect("parse");
        assert_eq!(count_parameters(&stmts), 1);

        // 多参数
        let stmts = parse_sql("SELECT $1, $2, $3 FROM t").expect("parse");
        assert_eq!(count_parameters(&stmts), 3);

        // 多语句取最大
        let stmts = parse_sql("SELECT 1; SELECT * FROM t WHERE x = $5").expect("parse");
        assert_eq!(count_parameters(&stmts), 5);
    }

    #[test]
    fn test_executor_backend_comment_on_column() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .expect("CREATE");

        // COMMENT ON COLUMN
        backend
            .execute_sql("COMMENT ON COLUMN t.name IS '用户名'")
            .expect("COMMENT ON COLUMN");

        // describe_table 应反映注释
        let schema = backend.describe_table("t").expect("describe_table");
        let name_col = schema
            .columns
            .iter()
            .find(|c| c.name == "name")
            .expect("name column should exist");
        assert_eq!(
            name_col.comment.as_deref(),
            Some("用户名"),
            "comment should be '用户名'"
        );
    }

    #[test]
    fn test_executor_backend_comment_on_table() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");

        // COMMENT ON TABLE
        backend
            .execute_sql("COMMENT ON TABLE t IS '用户表'")
            .expect("COMMENT ON TABLE");

        // 再次 COMMENT 修改
        backend
            .execute_sql("COMMENT ON TABLE t IS '订单表'")
            .expect("COMMENT ON TABLE update");
    }

    #[test]
    fn test_executor_backend_multiple_statements() {
        let backend = build_executor_backend();

        // 多语句一次性执行
        let r = backend
            .execute_sql(
                "CREATE TABLE t (id BIGINT, name TEXT); INSERT INTO t (id, name) VALUES (1, 'a');",
            )
            .expect("multi-statement");
        // 最后一条 INSERT 影响 1 行
        assert_eq!(r.affected_rows, 1, "last statement should affect 1 row");

        // 验证表存在且有数据
        let tables = backend.list_tables().expect("list_tables");
        assert_eq!(tables.len(), 1, "1 table should exist");
        assert_eq!(tables[0].row_count, 1, "1 row should be inserted");
    }

    #[test]
    fn test_executor_backend_parse_error() {
        let backend = build_executor_backend();

        // 语法错误的 SQL
        let err = backend.execute_sql("SELECT FROM WHERE").unwrap_err();
        match err {
            McpError::BackendError(msg) => {
                assert!(msg.contains("parse error"), "unexpected: {msg}");
            }
            other => panic!("expected BackendError, got {other:?}"),
        }
    }

    #[test]
    fn test_executor_backend_describe_table_not_found() {
        let backend = build_executor_backend();

        let err = backend.describe_table("nonexistent").unwrap_err();
        match err {
            McpError::BackendError(msg) => {
                assert!(msg.contains("table not found"), "unexpected: {msg}");
            }
            other => panic!("expected BackendError, got {other:?}"),
        }
    }

    #[test]
    fn test_executor_backend_list_indexes_table_not_found() {
        let backend = build_executor_backend();

        let err = backend.list_indexes("nonexistent").unwrap_err();
        match err {
            McpError::BackendError(msg) => {
                assert!(msg.contains("table not found"), "unexpected: {msg}");
            }
            other => panic!("expected BackendError, got {other:?}"),
        }
    }

    #[test]
    fn test_executor_backend_create_table_if_not_exists() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");

        // 第二次 CREATE 不带 IF NOT EXISTS 应报错
        let err = backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .unwrap_err();
        match err {
            McpError::BackendError(msg) => {
                assert!(msg.contains("already exists"), "unexpected: {msg}");
            }
            other => panic!("expected BackendError, got {other:?}"),
        }

        // CREATE TABLE IF NOT EXISTS 应静默跳过
        backend
            .execute_sql("CREATE TABLE IF NOT EXISTS t (id BIGINT)")
            .expect("CREATE IF NOT EXISTS should skip");
    }

    #[test]
    fn test_executor_backend_db_stats() {
        let backend = build_executor_backend();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");
        backend
            .execute_sql("INSERT INTO t (id) VALUES (1), (2)")
            .expect("INSERT");

        let stats = backend.db_stats().expect("db_stats");
        assert_eq!(stats.table_count, 1, "should have 1 table");
        assert_eq!(stats.total_rows, 2, "should have 2 total rows");
    }

    #[test]
    fn test_executor_backend_default_impl() {
        // Default trait 应等价于 new()
        let backend = ExecutorBackend::default();
        let tables = backend.list_tables().expect("list_tables");
        assert!(tables.is_empty(), "default backend should have no tables");
    }

    #[test]
    fn test_executor_backend_with_data_constructor() {
        use szrsql_sql::ast::{ColumnDefinition, TableName};
        use szrsql_sql::executor::InMemoryTable;
        use szrsql_sql::plan::{InMemoryCatalog, TableSchema};
        use szrsql_types::value::ColumnType;

        let mut catalog = InMemoryCatalog::new();
        let schema = TableSchema {
            name: TableName::new("t"),
            columns: vec![ColumnDefinition::new("id", ColumnType::Int64)],
        };
        catalog.add_table(schema.clone());

        let mut tables = std::collections::HashMap::new();
        tables.insert("t".to_string(), InMemoryTable::new(schema));

        let backend = ExecutorBackend::with_data(catalog, tables);
        let list = backend.list_tables().expect("list_tables");
        assert_eq!(list.len(), 1, "with_data should have 1 table");
        assert_eq!(list[0].name, "t");
    }

    // =================================================================
    // P3-Deadlock-Detection 单元测试
    // =================================================================

    #[test]
    fn test_p3_deadlock_table_resource_id_stable() {
        // 同一表名多次调用应产生相同 resource_id
        let id1 = table_resource_id("users");
        let id2 = table_resource_id("users");
        assert_eq!(id1, id2, "same table name must produce same resource_id");
    }

    #[test]
    fn test_p3_deadlock_table_resource_id_case_insensitive() {
        // 大小写不敏感（因为内部 to_lowercase）
        let id1 = table_resource_id("Users");
        let id2 = table_resource_id("USERS");
        let id3 = table_resource_id("users");
        assert_eq!(id1, id2, "case-insensitive: Users == USERS");
        assert_eq!(id1, id3, "case-insensitive: Users == users");
    }

    #[test]
    fn test_p3_deadlock_table_resource_id_different_tables() {
        // 不同表名应产生不同 resource_id
        let id1 = table_resource_id("users");
        let id2 = table_resource_id("orders");
        assert_ne!(
            id1, id2,
            "different table names must produce different resource_id"
        );
    }

    #[test]
    fn test_p3_deadlock_record_lock_granted() {
        // 无冲突时 record_lock 应正确加锁并记录到 active_locks（granted=true）
        let backend = ExecutorBackend::new();
        // 手动注入一个活动事务（txn_id=100）
        backend
            .stats
            .borrow_mut()
            .active_transactions
            .push(TransactionInfo {
                txn_id: 100,
                state: "active".to_string(),
                started_at: 1000,
                sql: "BEGIN".to_string(),
                wait_event: None,
                isolation: None,
                snapshot_active_count: None,
                snapshot_xmax: None,
            });
        // 调用 record_lock
        backend.record_lock("t1", "RowExclusiveLock", true, 2000);
        // 验证 active_locks
        let locks = backend.stats.borrow().active_locks.clone();
        assert_eq!(locks.len(), 1, "should have 1 lock");
        assert_eq!(locks[0].txn_id, 100);
        assert_eq!(locks[0].table, "t1");
        assert_eq!(locks[0].mode, "RowExclusiveLock");
        assert!(locks[0].granted, "lock should be granted (no conflict)");
        assert!(
            locks[0].wait_start.is_none(),
            "granted lock has no wait_start"
        );
        // 验证 LockManager 内部也持有该锁
        assert!(backend.lock_mgr.holds_lock(100, table_resource_id("t1")));
    }

    #[test]
    fn test_p3_deadlock_record_lock_conflict_granted_false() {
        // 冲突时 record_lock 应记录 granted=false
        let backend = ExecutorBackend::new();
        // 先让 txn 1 持有 t1 的 X 锁
        backend
            .lock_mgr
            .try_lock(
                1,
                table_resource_id("t1"),
                szrsql_tx::lock::LockMode::Exclusive,
            )
            .expect("txn1 lock");
        // 手动注入 txn 2 为活动事务
        backend
            .stats
            .borrow_mut()
            .active_transactions
            .push(TransactionInfo {
                txn_id: 2,
                state: "active".to_string(),
                started_at: 1000,
                sql: "BEGIN".to_string(),
                wait_event: None,
                isolation: None,
                snapshot_active_count: None,
                snapshot_xmax: None,
            });
        // txn 2 尝试加 t1 的 X 锁（冲突）
        backend.record_lock("t1", "RowExclusiveLock", true, 2000);
        // 验证 active_locks
        let locks = backend.stats.borrow().active_locks.clone();
        assert_eq!(locks.len(), 1, "should have 1 lock (waiting)");
        assert_eq!(locks[0].txn_id, 2);
        assert!(!locks[0].granted, "lock should NOT be granted (conflict)");
        assert!(locks[0].wait_start.is_some(), "waiting lock has wait_start");
    }

    #[test]
    fn test_p3_deadlock_unlock_on_commit() {
        // COMMIT 应通过 LockManager.unlock_all 释放所有锁
        let backend = ExecutorBackend::new();
        // 模拟 BEGIN：设置 current_txn 并添加活动事务
        backend.set_current_txn_id(Some(100));
        backend
            .stats
            .borrow_mut()
            .active_transactions
            .push(TransactionInfo {
                txn_id: 100,
                state: "active".to_string(),
                started_at: 1000,
                sql: "BEGIN".to_string(),
                wait_event: None,
                isolation: None,
                snapshot_active_count: None,
                snapshot_xmax: None,
            });
        // txn 100 持有 t1 的 X 锁
        backend
            .lock_mgr
            .try_lock(
                100,
                table_resource_id("t1"),
                szrsql_tx::lock::LockMode::Exclusive,
            )
            .expect("lock");
        assert!(
            backend.lock_mgr.holds_lock(100, table_resource_id("t1")),
            "lock held before COMMIT"
        );
        // 执行 COMMIT
        backend.execute_sql("COMMIT").expect("COMMIT");
        // 验证 LockManager 中的锁已释放
        assert!(
            !backend.lock_mgr.holds_lock(100, table_resource_id("t1")),
            "lock released after COMMIT"
        );
        // 验证 stats.active_locks 也已清空
        assert!(
            backend.stats.borrow().active_locks.is_empty(),
            "active_locks cleared after COMMIT"
        );
    }

    #[test]
    fn test_p3_deadlock_unlock_on_rollback() {
        // ROLLBACK 应通过 LockManager.unlock_all 释放所有锁
        let backend = ExecutorBackend::new();
        backend.set_current_txn_id(Some(200));
        backend
            .stats
            .borrow_mut()
            .active_transactions
            .push(TransactionInfo {
                txn_id: 200,
                state: "active".to_string(),
                started_at: 1000,
                sql: "BEGIN".to_string(),
                wait_event: None,
                isolation: None,
                snapshot_active_count: None,
                snapshot_xmax: None,
            });
        backend
            .lock_mgr
            .try_lock(
                200,
                table_resource_id("t2"),
                szrsql_tx::lock::LockMode::Exclusive,
            )
            .expect("lock");
        assert!(
            backend.lock_mgr.holds_lock(200, table_resource_id("t2")),
            "lock held before ROLLBACK"
        );
        // 执行 ROLLBACK
        backend.execute_sql("ROLLBACK").expect("ROLLBACK");
        // 验证锁已释放
        assert!(
            !backend.lock_mgr.holds_lock(200, table_resource_id("t2")),
            "lock released after ROLLBACK"
        );
        assert!(
            backend.stats.borrow().active_locks.is_empty(),
            "active_locks cleared after ROLLBACK"
        );
    }

    #[test]
    fn test_p3_deadlock_unlock_on_kill() {
        // kill_transaction 应通过 LockManager.unlock_all 释放该事务的锁
        let backend = ExecutorBackend::new();
        backend
            .stats
            .borrow_mut()
            .active_transactions
            .push(TransactionInfo {
                txn_id: 300,
                state: "active".to_string(),
                started_at: 1000,
                sql: "BEGIN".to_string(),
                wait_event: None,
                isolation: None,
                snapshot_active_count: None,
                snapshot_xmax: None,
            });
        backend
            .lock_mgr
            .try_lock(
                300,
                table_resource_id("t3"),
                szrsql_tx::lock::LockMode::Exclusive,
            )
            .expect("lock");
        // 记录到 stats.active_locks
        backend.stats.borrow_mut().active_locks.push(LockInfo {
            txn_id: 300,
            table: "t3".to_string(),
            mode: "RowExclusiveLock".to_string(),
            granted: true,
            wait_start: None,
        });
        assert!(
            backend.lock_mgr.holds_lock(300, table_resource_id("t3")),
            "lock held before kill"
        );
        // kill txn 300
        let result = backend.kill_transaction(300).expect("kill");
        assert!(result.killed, "transaction should be killed");
        // 验证锁已释放
        assert!(
            !backend.lock_mgr.holds_lock(300, table_resource_id("t3")),
            "lock released after kill"
        );
        assert!(
            backend.stats.borrow().active_locks.is_empty(),
            "active_locks cleared after kill"
        );
    }

    #[test]
    fn test_p3_deadlock_history_initially_empty() {
        // 初始状态 deadlock_history 应为空
        let backend = ExecutorBackend::new();
        let history = backend.deadlock_history().expect("deadlock_history");
        assert!(
            history.is_empty(),
            "deadlock_history should be empty initially"
        );
    }

    #[test]
    fn test_p3_deadlock_record_deadlocks_writes_history() {
        // record_deadlocks 应正确写入 deadlock_history（含去重）
        let backend = ExecutorBackend::new();
        let cycles = vec![vec![1, 2]];
        // 写入第一条死锁记录
        backend.record_deadlocks(&cycles, "t1", 1000);
        let history = backend.deadlock_history().expect("deadlock_history");
        assert_eq!(history.len(), 1, "should have 1 deadlock record");
        assert_eq!(history[0].txn_ids, vec![1, 2]);
        assert_eq!(history[0].resource, "t1");
        assert_eq!(history[0].timestamp, 1000);
        // 重复写入（同 txn_ids + 同 resource）应去重
        backend.record_deadlocks(&cycles, "t1", 2000);
        let history2 = backend.deadlock_history().expect("deadlock_history");
        assert_eq!(
            history2.len(),
            1,
            "duplicate deadlock should be deduplicated"
        );
        // 不同 resource 应记录新条目
        backend.record_deadlocks(&cycles, "t2", 3000);
        let history3 = backend.deadlock_history().expect("deadlock_history");
        assert_eq!(
            history3.len(),
            2,
            "different resource should add new record"
        );
        // 不同 txn_ids 应记录新条目
        let cycles2 = vec![vec![3, 4]];
        backend.record_deadlocks(&cycles2, "t1", 4000);
        let history4 = backend.deadlock_history().expect("deadlock_history");
        assert_eq!(history4.len(), 3, "different txn_ids should add new record");
    }

    #[test]
    fn test_p3_deadlock_lock_manager_detects_real_cycle() {
        // 端到端验证：LockManager 的 lock() 方法在进入等待队列后立即检测死锁
        // 通过多线程建立 2 个事务互相等待的环：txn1→txn2→txn1
        use std::sync::Arc;
        use std::thread;
        use std::time::Duration;

        let mgr = Arc::new(szrsql_tx::lock::LockManager::new());
        let resource_a: u64 = 1001;
        let resource_b: u64 = 2002;

        // txn 1 持有 resource_a 的 X 锁
        mgr.try_lock(1, resource_a, szrsql_tx::lock::LockMode::Exclusive)
            .expect("txn1 lock A");
        // txn 2 持有 resource_b 的 X 锁
        mgr.try_lock(2, resource_b, szrsql_tx::lock::LockMode::Exclusive)
            .expect("txn2 lock B");

        // 初始无环
        let cycles0 = mgr.detect_all_deadlocks();
        assert!(cycles0.is_empty(), "no cycle initially");

        // 线程 1：txn 1 请求 resource_b（阻塞，进入 waiters，但不会检测到死锁因为 txn 2 还没等待）
        let mgr_clone1 = Arc::clone(&mgr);
        let handle1 = thread::spawn(move || {
            // 超时 500ms，足够建立等待边
            mgr_clone1.lock(
                1,
                resource_b,
                szrsql_tx::lock::LockMode::Exclusive,
                Duration::from_millis(500),
            )
        });

        // 等待线程 1 进入等待队列
        thread::sleep(Duration::from_millis(30));

        // 主线程：txn 2 请求 resource_a（进入 waiters 后立即检测到死锁：txn2→txn1→txn2）
        let result2 = mgr.lock(
            2,
            resource_a,
            szrsql_tx::lock::LockMode::Exclusive,
            Duration::from_millis(500),
        );

        // 验证 txn 2 的 lock() 返回 Deadlock 错误（lock() 内部检测到环后中止自身）
        assert!(
            matches!(result2, Err(szrsql_tx::lock::LockError::Deadlock(2))),
            "txn 2 should detect deadlock, got: {:?}",
            result2
        );

        // 等待线程 1 完成（txn 1 的 lock 可能超时或被唤醒后获取锁）
        let result1 = handle1.join().expect("thread1 join");
        // txn 1 的 lock 结果可能是 Ok（txn 2 中止后释放了 resource_b 的等待，但 txn 2 仍持有 resource_b）
        // 或超时（txn 2 仍持有 resource_b）
        // 这里不严格断言，主要验证 txn 2 检测到了死锁
        let _ = result1;
    }

    #[test]
    fn test_p3_deadlock_record_lock_dedup() {
        // 同 txn + table + mode 不重复添加到 active_locks
        let backend = ExecutorBackend::new();
        backend
            .stats
            .borrow_mut()
            .active_transactions
            .push(TransactionInfo {
                txn_id: 500,
                state: "active".to_string(),
                started_at: 1000,
                sql: "BEGIN".to_string(),
                wait_event: None,
                isolation: None,
                snapshot_active_count: None,
                snapshot_xmax: None,
            });
        // 同一事务对同一表多次加锁
        backend.record_lock("t1", "RowExclusiveLock", true, 1000);
        backend.record_lock("t1", "RowExclusiveLock", true, 2000);
        backend.record_lock("t1", "RowExclusiveLock", true, 3000);
        let locks = backend.stats.borrow().active_locks.clone();
        assert_eq!(locks.len(), 1, "duplicate locks should be deduplicated");
    }

    #[test]
    fn test_p3_deadlock_record_lock_no_txn_records_only_stats() {
        // txn_id=0（无活动事务）时仅记录到 stats，不调用 LockManager
        let backend = ExecutorBackend::new();
        backend.record_lock("t1", "RowExclusiveLock", true, 1000);
        let locks = backend.stats.borrow().active_locks.clone();
        assert_eq!(locks.len(), 1, "should have 1 lock in stats");
        assert_eq!(
            locks[0].txn_id, 0,
            "txn_id should be 0 (no active transaction)"
        );
        // LockManager 中不应有锁（因为 txn_id=0 不走 LockManager）
        assert!(
            !backend.lock_mgr.holds_lock(0, table_resource_id("t1")),
            "LockManager should not have lock for txn_id=0"
        );
    }

    // =================================================================
    // P3-Capacity-Enhanced 单元测试
    // =================================================================

    #[test]
    fn test_p3_capacity_empty_history_returns_none_fields() {
        // 空查询历史时，新增字段应为 None
        let backend = ExecutorBackend::new();
        let forecast = backend.capacity_predict(30).expect("capacity_predict");
        assert_eq!(forecast.metric, "total_rows");
        assert_eq!(forecast.current_value, 0.0);
        assert_eq!(forecast.predicted_value, 0.0);
        assert_eq!(forecast.confidence, 0.0);
        assert!(
            forecast.storage_bytes_current.is_none(),
            "storage_bytes_current should be None for empty history"
        );
        assert!(
            forecast.storage_bytes_predicted.is_none(),
            "storage_bytes_predicted should be None for empty history"
        );
        assert!(
            forecast.net_growth_rate_per_day.is_none(),
            "net_growth_rate_per_day should be None for empty history"
        );
        assert!(
            forecast.table_breakdown.is_none(),
            "table_breakdown should be None for empty history"
        );
    }

    #[test]
    fn test_p3_capacity_days_zero_returns_none_fields() {
        // days=0 时，新增字段应为 None
        let backend = ExecutorBackend::new();
        // 手动注入一条查询历史（但 days=0 仍应返回 None）
        backend.stats.borrow_mut().query_history.push(QueryRecord {
            sql: "INSERT INTO t VALUES (1)".to_string(),
            elapsed_ms: 10,
            affected_rows: 1,
            timestamp: 1000,
        });
        let forecast = backend.capacity_predict(0).expect("capacity_predict");
        assert!(
            forecast.storage_bytes_current.is_none(),
            "storage_bytes_current should be None for days=0"
        );
        assert!(
            forecast.table_breakdown.is_none(),
            "table_breakdown should be None for days=0"
        );
    }

    #[test]
    fn test_p3_capacity_predict_with_inserts_returns_real_fields() {
        // 有 INSERT 操作时，新增字段应有真实值
        let backend = ExecutorBackend::new();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");
        backend
            .execute_sql("INSERT INTO t (id) VALUES (1)")
            .expect("INSERT");
        backend
            .execute_sql("INSERT INTO t (id) VALUES (2)")
            .expect("INSERT");

        let forecast = backend.capacity_predict(30).expect("capacity_predict");
        assert!(
            forecast.current_value > 0.0,
            "current_value should be > 0 (2 rows)"
        );
        assert!(
            forecast.predicted_value >= forecast.current_value,
            "predicted should be >= current"
        );
        assert!(
            forecast.storage_bytes_current.is_some(),
            "storage_bytes_current should be Some"
        );
        assert!(
            forecast.storage_bytes_predicted.is_some(),
            "storage_bytes_predicted should be Some"
        );
        assert!(
            forecast.net_growth_rate_per_day.is_some(),
            "net_growth_rate_per_day should be Some"
        );
        assert!(
            forecast.table_breakdown.is_some(),
            "table_breakdown should be Some"
        );
        // storage_bytes_current > 0
        assert!(
            forecast.storage_bytes_current.unwrap() > 0.0,
            "storage_bytes_current should be > 0"
        );
    }

    #[test]
    fn test_p3_capacity_net_growth_rate_insert_delete() {
        // 净增长率 = (INSERT - DELETE) / span_days
        let backend = ExecutorBackend::new();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");
        // 3 条 INSERT（时间戳 1000）
        backend.stats.borrow_mut().query_history.push(QueryRecord {
            sql: "INSERT INTO t VALUES (1)".to_string(),
            elapsed_ms: 10,
            affected_rows: 3,
            timestamp: 1000,
        });
        // 1 条 DELETE（时间戳 86400000 = 1 天后）
        backend.stats.borrow_mut().query_history.push(QueryRecord {
            sql: "DELETE FROM t WHERE id = 1".to_string(),
            elapsed_ms: 10,
            affected_rows: 1,
            timestamp: 86_400_000,
        });
        let forecast = backend.capacity_predict(30).expect("capacity_predict");
        let net_rate = forecast.net_growth_rate_per_day.expect("net_growth_rate");
        // 净增长 = 3 - 1 = 2，span_days = 1.0，所以 net_rate = 2.0
        assert!(
            (net_rate - 2.0).abs() < 0.01,
            "net_growth_rate should be 2.0 (3 inserts - 1 delete / 1 day), got {}",
            net_rate
        );
    }

    #[test]
    fn test_p3_capacity_table_breakdown_contains_tables() {
        // table_breakdown 应包含所有表
        let backend = ExecutorBackend::new();
        backend
            .execute_sql("CREATE TABLE t1 (id BIGINT)")
            .expect("CREATE t1");
        backend
            .execute_sql("CREATE TABLE t2 (id BIGINT)")
            .expect("CREATE t2");
        backend
            .execute_sql("INSERT INTO t1 (id) VALUES (1)")
            .expect("INSERT t1");
        backend
            .execute_sql("INSERT INTO t2 (id) VALUES (1)")
            .expect("INSERT t2");

        let forecast = backend.capacity_predict(30).expect("capacity_predict");
        let breakdown = forecast.table_breakdown.expect("table_breakdown");
        assert_eq!(breakdown.len(), 2, "should have 2 tables in breakdown");
        // 每张表应有 current_rows > 0
        for tf in &breakdown {
            assert!(
                tf.current_rows > 0.0,
                "table {} should have current_rows > 0",
                tf.table
            );
            assert!(
                tf.current_bytes > 0.0,
                "table {} should have current_bytes > 0",
                tf.table
            );
            assert!(
                tf.predicted_rows >= tf.current_rows,
                "table {} predicted should be >= current",
                tf.table
            );
        }
    }

    #[test]
    fn test_p3_capacity_confidence_bounded_0_1() {
        // 置信度应在 [0, 1] 范围内
        let backend = ExecutorBackend::new();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");
        // 注入大量查询历史
        for i in 0..200 {
            backend.stats.borrow_mut().query_history.push(QueryRecord {
                sql: format!("INSERT INTO t VALUES ({})", i),
                elapsed_ms: 10,
                affected_rows: 1,
                timestamp: 1000 + i * 1000,
            });
        }
        let forecast = backend.capacity_predict(30).expect("capacity_predict");
        assert!(
            forecast.confidence >= 0.0 && forecast.confidence <= 1.0,
            "confidence should be in [0, 1], got {}",
            forecast.confidence
        );
        // 200 个样本 + 足够时间跨度，置信度应较高
        assert!(
            forecast.confidence > 0.5,
            "confidence should be > 0.5 with 200 samples, got {}",
            forecast.confidence
        );
    }

    #[test]
    fn test_p3_capacity_storage_bytes_predicted_ge_current() {
        // 净增长时，预测存储大小应 >= 当前存储大小
        let backend = ExecutorBackend::new();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");
        backend
            .execute_sql("INSERT INTO t (id) VALUES (1)")
            .expect("INSERT");

        let forecast = backend.capacity_predict(30).expect("capacity_predict");
        let current = forecast.storage_bytes_current.expect("current");
        let predicted = forecast.storage_bytes_predicted.expect("predicted");
        assert!(
            predicted >= current,
            "predicted bytes ({}) should be >= current bytes ({})",
            predicted,
            current
        );
    }

    #[test]
    fn test_p3_capacity_delete_reduces_net_growth() {
        // DELETE 操作应降低净增长率
        let backend = ExecutorBackend::new();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");
        // 只有 INSERT
        backend.stats.borrow_mut().query_history.push(QueryRecord {
            sql: "INSERT INTO t VALUES (1)".to_string(),
            elapsed_ms: 10,
            affected_rows: 5,
            timestamp: 1000,
        });
        backend.stats.borrow_mut().query_history.push(QueryRecord {
            sql: "INSERT INTO t VALUES (2)".to_string(),
            elapsed_ms: 10,
            affected_rows: 5,
            timestamp: 86_400_000,
        });
        let forecast_insert_only = backend.capacity_predict(30).expect("capacity_predict");
        let rate_insert_only = forecast_insert_only.net_growth_rate_per_day.expect("rate");

        // 添加 DELETE
        backend.stats.borrow_mut().query_history.push(QueryRecord {
            sql: "DELETE FROM t".to_string(),
            elapsed_ms: 10,
            affected_rows: 3,
            timestamp: 172_800_000, // 2 天后
        });
        let forecast_with_delete = backend.capacity_predict(30).expect("capacity_predict");
        let rate_with_delete = forecast_with_delete.net_growth_rate_per_day.expect("rate");

        assert!(
            rate_with_delete < rate_insert_only,
            "net growth rate with delete ({}) should be < insert-only ({})",
            rate_with_delete,
            rate_insert_only
        );
    }

    // =================================================================
    // P3-RootCause-Enhanced 单元测试
    // =================================================================

    #[test]
    fn test_p3_root_cause_lock_wait_rule_returns_lock_contention() {
        // lock_wait 规则应返回 LockContention 根因
        let backend = ExecutorBackend::new();
        // 注入锁等待事件
        backend.stats.borrow_mut().wait_events.insert(
            "Lock:txn".to_string(),
            WaitEventAggr {
                total_waits: 10,
                total_wait_ms: 5000,
            },
        );
        // 注入告警
        backend.stats.borrow_mut().alerts.push(AlertInfo {
            level: "warning".to_string(),
            rule_id: "lock_wait".to_string(),
            message: "Lock wait exceeds threshold".to_string(),
            timestamp: 1000,
            value: 10.0,
            threshold: 5.0,
        });

        let report = backend
            .explain_root_cause("lock_wait")
            .expect("explain_root_cause");
        assert!(
            report
                .likely_causes
                .iter()
                .any(|c| c.cause_type == CauseType::LockContention),
            "lock_wait rule should return LockContention cause"
        );
        // 主根因置信度应较高（>= 0.8）
        let lock_cause = report
            .likely_causes
            .iter()
            .find(|c| c.cause_type == CauseType::LockContention)
            .expect("should have LockContention cause");
        assert!(
            lock_cause.confidence >= 0.8,
            "LockContention confidence should be >= 0.8, got {}",
            lock_cause.confidence
        );
        // 应有 wait_events_lock 证据
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.source == "wait_events_lock"),
            "should have wait_events_lock evidence"
        );
    }

    #[test]
    fn test_p3_root_cause_lock_wait_with_deadlock_history() {
        // lock_wait 规则 + 死锁历史 → 应同时返回 LockContention 和 Deadlock 根因
        let backend = ExecutorBackend::new();
        backend.stats.borrow_mut().wait_events.insert(
            "Lock:txn".to_string(),
            WaitEventAggr {
                total_waits: 10,
                total_wait_ms: 5000,
            },
        );
        backend
            .stats
            .borrow_mut()
            .deadlock_history
            .push(DeadlockRecord {
                timestamp: 1000,
                txn_ids: vec![1, 2],
                resource: "table:t1".to_string(),
            });
        backend.stats.borrow_mut().alerts.push(AlertInfo {
            level: "critical".to_string(),
            rule_id: "lock_wait".to_string(),
            message: "Lock wait exceeds threshold".to_string(),
            timestamp: 2000,
            value: 10.0,
            threshold: 5.0,
        });

        let report = backend
            .explain_root_cause("lock_wait")
            .expect("explain_root_cause");
        assert!(
            report
                .likely_causes
                .iter()
                .any(|c| c.cause_type == CauseType::LockContention),
            "should have LockContention cause"
        );
        assert!(
            report
                .likely_causes
                .iter()
                .any(|c| c.cause_type == CauseType::Deadlock),
            "should have Deadlock cause when deadlock_history is non-empty"
        );
        // 应有 deadlock_history 证据
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.source == "deadlock_history"),
            "should have deadlock_history evidence"
        );
    }

    #[test]
    fn test_p3_root_cause_lock_wait_with_slow_query() {
        // lock_wait 规则 + 慢查询 → 应同时返回 LockContention 和 MissingIndex 根因
        let backend = ExecutorBackend::new();
        backend.stats.borrow_mut().wait_events.insert(
            "Lock:txn".to_string(),
            WaitEventAggr {
                total_waits: 10,
                total_wait_ms: 5000,
            },
        );
        backend.stats.borrow_mut().query_history.push(QueryRecord {
            sql: "SELECT * FROM big_table WHERE col = 1".to_string(),
            elapsed_ms: 5000,
            affected_rows: 10000,
            timestamp: 1000,
        });
        backend.stats.borrow_mut().slow_query_threshold_ms = 1000;
        backend.stats.borrow_mut().alerts.push(AlertInfo {
            level: "warning".to_string(),
            rule_id: "lock_wait".to_string(),
            message: "Lock wait exceeds threshold".to_string(),
            timestamp: 2000,
            value: 10.0,
            threshold: 5.0,
        });

        let report = backend
            .explain_root_cause("lock_wait")
            .expect("explain_root_cause");
        assert!(
            report
                .likely_causes
                .iter()
                .any(|c| c.cause_type == CauseType::MissingIndex),
            "should have MissingIndex cause when slow query exists"
        );
        assert!(
            report.evidence.iter().any(|e| e.source == "slow_query"),
            "should have slow_query evidence"
        );
    }

    #[test]
    fn test_p3_root_cause_lock_wait_with_pending_locks() {
        // lock_wait 规则 + 未授予的活动锁 → 应有 active_locks_pending 证据
        let backend = ExecutorBackend::new();
        backend.stats.borrow_mut().wait_events.insert(
            "Lock:txn".to_string(),
            WaitEventAggr {
                total_waits: 10,
                total_wait_ms: 5000,
            },
        );
        backend.stats.borrow_mut().active_locks.push(LockInfo {
            txn_id: 1,
            table: "t1".to_string(),
            mode: "RowExclusiveLock".to_string(),
            granted: false,
            wait_start: Some(1000),
        });
        backend.stats.borrow_mut().alerts.push(AlertInfo {
            level: "warning".to_string(),
            rule_id: "lock_wait".to_string(),
            message: "Lock wait exceeds threshold".to_string(),
            timestamp: 2000,
            value: 10.0,
            threshold: 5.0,
        });

        let report = backend
            .explain_root_cause("lock_wait")
            .expect("explain_root_cause");
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.source == "active_locks_pending"),
            "should have active_locks_pending evidence"
        );
    }

    #[test]
    fn test_p3_root_cause_slow_query_evidence_chain_enhanced() {
        // slow_query 规则的证据链应包含活动事务和活动锁证据
        let backend = ExecutorBackend::new();
        backend.stats.borrow_mut().query_history.push(QueryRecord {
            sql: "SELECT * FROM big_table".to_string(),
            elapsed_ms: 5000,
            affected_rows: 10000,
            timestamp: 1000,
        });
        backend.stats.borrow_mut().slow_query_threshold_ms = 1000;
        // 注入有 wait_event 的活动事务
        backend
            .stats
            .borrow_mut()
            .active_transactions
            .push(TransactionInfo {
                txn_id: 1,
                state: "active".to_string(),
                started_at: 1000,
                sql: "UPDATE t SET x = 1".to_string(),
                wait_event: Some("Lock:txn".to_string()),
                isolation: None,
                snapshot_active_count: None,
                snapshot_xmax: None,
            });
        // 注入未授予的活动锁
        backend.stats.borrow_mut().active_locks.push(LockInfo {
            txn_id: 1,
            table: "t1".to_string(),
            mode: "RowExclusiveLock".to_string(),
            granted: false,
            wait_start: Some(1000),
        });
        backend.stats.borrow_mut().alerts.push(AlertInfo {
            level: "warning".to_string(),
            rule_id: "slow_query".to_string(),
            message: "Slow query detected".to_string(),
            timestamp: 2000,
            value: 5000.0,
            threshold: 1000.0,
        });

        let report = backend
            .explain_root_cause("slow_query")
            .expect("explain_root_cause");
        // 应有活动事务证据
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.source == "active_transactions"),
            "slow_query should have active_transactions evidence"
        );
        // 应有活动锁证据
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.source == "active_locks_pending"),
            "slow_query should have active_locks_pending evidence"
        );
    }

    #[test]
    fn test_p3_root_cause_deadlock_evidence_chain_enhanced() {
        // deadlock 规则的证据链应包含等待事件和活动事务证据
        let backend = ExecutorBackend::new();
        backend
            .stats
            .borrow_mut()
            .deadlock_history
            .push(DeadlockRecord {
                timestamp: 1000,
                txn_ids: vec![1, 2],
                resource: "table:t1".to_string(),
            });
        backend.stats.borrow_mut().wait_events.insert(
            "Lock:txn".to_string(),
            WaitEventAggr {
                total_waits: 5,
                total_wait_ms: 3000,
            },
        );
        // 注入参与死锁的活动事务
        backend
            .stats
            .borrow_mut()
            .active_transactions
            .push(TransactionInfo {
                txn_id: 1,
                state: "active".to_string(),
                started_at: 1000,
                sql: "UPDATE t1 SET x = 1 WHERE id = 1".to_string(),
                wait_event: Some("Lock:txn".to_string()),
                isolation: None,
                snapshot_active_count: None,
                snapshot_xmax: None,
            });
        backend.stats.borrow_mut().alerts.push(AlertInfo {
            level: "critical".to_string(),
            rule_id: "deadlock".to_string(),
            message: "Deadlock detected".to_string(),
            timestamp: 2000,
            value: 1.0,
            threshold: 0.0,
        });

        let report = backend
            .explain_root_cause("deadlock")
            .expect("explain_root_cause");
        // 应有 wait_events 证据
        assert!(
            report.evidence.iter().any(|e| e.source == "wait_events"),
            "deadlock should have wait_events evidence"
        );
        // 应有活动事务证据（包含参与死锁的事务）
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.source == "active_transactions"),
            "deadlock should have active_transactions evidence"
        );
    }

    #[test]
    fn test_p3_root_cause_high_qps_evidence_chain_enhanced() {
        // high_qps 规则的证据链应包含 query_aggr_top_qps 证据
        let backend = ExecutorBackend::new();
        backend.stats.borrow_mut().query_aggr.insert(
            "SELECT * FROM t1 WHERE id = ?".to_string(),
            QueryAggr {
                count: 1000,
                total_ms: 50000,
                max_ms: 100,
            },
        );
        backend.stats.borrow_mut().alerts.push(AlertInfo {
            level: "warning".to_string(),
            rule_id: "high_qps".to_string(),
            message: "High QPS detected".to_string(),
            timestamp: 2000,
            value: 1000.0,
            threshold: 100.0,
        });

        let report = backend
            .explain_root_cause("high_qps")
            .expect("explain_root_cause");
        // 应有 query_aggr_top_qps 证据
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.source == "query_aggr_top_qps"),
            "high_qps should have query_aggr_top_qps evidence"
        );
    }

    #[test]
    fn test_p3_root_cause_full_table_scan_evidence_chain_enhanced() {
        // full_table_scan 规则的证据链应包含 wait_events 和 active_locks_pending 证据
        let backend = ExecutorBackend::new();
        backend.stats.borrow_mut().wait_events.insert(
            "Lock:txn".to_string(),
            WaitEventAggr {
                total_waits: 5,
                total_wait_ms: 2000,
            },
        );
        backend.stats.borrow_mut().active_locks.push(LockInfo {
            txn_id: 1,
            table: "t1".to_string(),
            mode: "RowExclusiveLock".to_string(),
            granted: false,
            wait_start: Some(1000),
        });
        backend.stats.borrow_mut().alerts.push(AlertInfo {
            level: "warning".to_string(),
            rule_id: "full_table_scan".to_string(),
            message: "Full table scan detected".to_string(),
            timestamp: 2000,
            value: 1.0,
            threshold: 0.0,
        });

        let report = backend
            .explain_root_cause("full_table_scan")
            .expect("explain_root_cause");
        // 应有 wait_events 证据
        assert!(
            report.evidence.iter().any(|e| e.source == "wait_events"),
            "full_table_scan should have wait_events evidence"
        );
        // 应有 active_locks_pending 证据
        assert!(
            report
                .evidence
                .iter()
                .any(|e| e.source == "active_locks_pending"),
            "full_table_scan should have active_locks_pending evidence"
        );
    }

    #[test]
    fn test_p3_root_cause_resource_contention_variant_exists() {
        // CauseType::ResourceContention 变体应存在且可序列化
        let cause = CauseEntry {
            cause_type: CauseType::ResourceContention,
            description: "test".to_string(),
            confidence: 0.5,
        };
        let json = serde_json::to_string(&cause).expect("serialize");
        assert!(
            json.contains("ResourceContention"),
            "serialized JSON should contain ResourceContention: {}",
            json
        );
        // 反序列化
        let deserialized: CauseEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.cause_type, CauseType::ResourceContention);
    }

    #[test]
    fn test_p3_root_cause_lock_wait_alert_not_found_errors() {
        // 不存在的 alert_id 应返回错误
        let backend = ExecutorBackend::new();
        let result = backend.explain_root_cause("nonexistent_alert");
        assert!(
            result.is_err(),
            "explain_root_cause should error for non-existent alert"
        );
    }

    #[test]
    fn test_new_with_executor_constructor() {
        let backend = ExecutorBackend::new();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");

        // 使用 new_with_executor 便捷构造函数
        let server = McpServerV2::new_with_executor(backend);
        // 验证 server 工具总数仍为 35
        let tools = server.tool_definitions();
        assert_eq!(tools.len(), 35, "tool count must still be 35");
        // 验证后端为 ExecutorBackend（通过行为验证：list_tables 返回真实表清单）
        let backend_tables = server
            .backend
            .list_tables()
            .expect("list_tables via server backend must succeed");
        assert_eq!(backend_tables.len(), 1, "must list 1 real table");
        assert_eq!(backend_tables[0].name, "t");
    }

    #[test]
    fn test_new_with_executor_handles_execute_sql_request() {
        let backend = ExecutorBackend::new();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT, name TEXT)")
            .expect("CREATE");
        backend
            .execute_sql("INSERT INTO t (id, name) VALUES (1, 'alice')")
            .expect("INSERT");

        let mut server = McpServerV2::new_with_executor(backend);
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(1)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "execute_sql",
                "arguments": {"sql": "SELECT * FROM t"}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_none(),
            "execute_sql should succeed via server"
        );
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("alice"),
            "response should contain data, got: {text}"
        );
    }

    #[test]
    fn test_new_with_executor_handles_explain_request() {
        let backend = ExecutorBackend::new();
        backend
            .execute_sql("CREATE TABLE t (id BIGINT)")
            .expect("CREATE");

        let mut server = McpServerV2::new_with_executor(backend);
        server.initialized = true;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(2)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": "explain_query",
                "arguments": {"sql": "SELECT * FROM t"}
            })),
        };
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_none(),
            "explain_query should succeed via server"
        );
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("SeqScan"),
            "response should contain SeqScan, got: {text}"
        );
    }

    // =================================================================
    // 19. MCP Replication 工具端到端测试（P3-3 — NineData 启发）
    //
    // 验证通过 MCP JSON-RPC 协议调用 5 个 Replication 类工具：
    //   - create_replication_task
    //   - list_replication_tasks
    //   - monitor_replication_task
    //   - stop_replication_task
    //   - replication_manager_stats
    // =================================================================

    /// 构造带 ReplicationTaskManager 的 CatalogBackend 测试辅助函数
    ///
    /// 返回 `(CatalogBackend, Arc<CdcEngine>, Arc<SchemaRegistry>)`：
    /// - `CatalogBackend` 已通过 `with_replication` 注入任务管理器
    /// - `CdcEngine` 用于在测试中触发 WalRecord 事件
    /// - `SchemaRegistry` 用于注册测试表
    fn build_catalog_backend_with_replication() -> (
        CatalogBackend,
        std::sync::Arc<szrsql_cdc::CdcEngine>,
        std::sync::Arc<szrsql_cdc::schema::SchemaRegistry>,
    ) {
        use std::sync::Arc;
        use szrsql_cdc::schema::{ColumnDef, DataType, SchemaRegistry};
        use szrsql_cdc::slot::SlotManager;
        use szrsql_cdc::task::ReplicationTaskManager;
        use szrsql_cdc::{CdcEngine, CdcObserverManager};

        // 1. 构造 CDC 组件
        let observer_mgr = Arc::new(CdcObserverManager::new());
        let cdc_engine = Arc::new(CdcEngine::with_timestamp_fn(observer_mgr, Box::new(|| 0)));
        let slot_mgr = Arc::new(SlotManager::in_memory());
        let registry = Arc::new(SchemaRegistry::new());
        let decoder = Arc::new(szrsql_cdc::decoder::RowDecoder::new(registry.clone()));

        // 2. 注册测试表（table_id=200，名为 users）
        registry
            .create_table(
                200,
                "users",
                vec![
                    ColumnDef::not_null("id", DataType::Int64),
                    ColumnDef::nullable("name", DataType::Text),
                ],
            )
            .expect("create_table users");

        // 3. 构造 ReplicationTaskManager
        let task_mgr = Arc::new(ReplicationTaskManager::new(
            slot_mgr,
            decoder,
            registry.clone(),
            cdc_engine.clone(),
        ));

        // 4. 构造 CatalogBackend 并注入 replication
        let catalog = szrsql_catalog::ManagedCatalog::new();
        let backend = CatalogBackend::new(Box::new(catalog)).with_replication(task_mgr);

        (backend, cdc_engine, registry)
    }

    /// 构造 JSON-RPC tools/call 请求
    fn make_tools_call_request(
        id: i64,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!(id)),
            method: "tools/call".to_string(),
            params: Some(json!({
                "name": tool_name,
                "arguments": arguments,
            })),
        }
    }

    /// 测试 1：未注入 replication 管理器时，所有 replication 工具应返回错误
    #[test]
    fn test_mcp_replication_tools_without_manager_return_error() {
        let mut server = McpServerV2::default(); // 默认 ExecutorBackend，无 replication
        server.initialized = true;

        // create_replication_task 应返回错误
        let req = make_tools_call_request(
            1,
            "create_replication_task",
            json!({
                "task_id": "rep_test",
                "target_type": "memory",
                "target_connection": "memory://test",
            }),
        );
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_some(),
            "create_replication_task without manager should error"
        );

        // list_replication_tasks 应返回错误
        let req = make_tools_call_request(2, "list_replication_tasks", json!({}));
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_some(),
            "list_replication_tasks without manager should error"
        );

        // replication_manager_stats 应返回错误
        let req = make_tools_call_request(3, "replication_manager_stats", json!({}));
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_some(),
            "replication_manager_stats without manager should error"
        );
    }

    /// 测试 2：完整生命周期 — create → list → monitor → stop → stats
    #[test]
    fn test_mcp_replication_lifecycle_create_list_monitor_stop() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        // 1. create_replication_task
        let req = make_tools_call_request(
            1,
            "create_replication_task",
            json!({
                "task_id": "rep_e2e_1",
                "description": "E2E test replication task",
                "target_type": "memory",
                "target_connection": "memory://e2e",
                "snapshot_first": false,
            }),
        );
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_none(),
            "create_replication_task should succeed: {:?}",
            resp.error
        );
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("rep_e2e_1"),
            "result should contain task_id, got: {text}"
        );
        assert!(
            text.contains("\"created\": true"),
            "result should have created=true, got: {text}"
        );
        assert!(
            text.contains("\"state\": \"running\""),
            "task should be running, got: {text}"
        );

        // 2. list_replication_tasks
        let req = make_tools_call_request(2, "list_replication_tasks", json!({}));
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_none(),
            "list_replication_tasks should succeed: {:?}",
            resp.error
        );
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("rep_e2e_1"),
            "list should contain task_id, got: {text}"
        );

        // 3. monitor_replication_task
        let req = make_tools_call_request(
            3,
            "monitor_replication_task",
            json!({"task_id": "rep_e2e_1"}),
        );
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_none(),
            "monitor_replication_task should succeed: {:?}",
            resp.error
        );
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("rep_e2e_1"),
            "monitor should contain task_id, got: {text}"
        );
        assert!(
            text.contains("\"state\": \"running\""),
            "monitor should show running state, got: {text}"
        );

        // 4. stop_replication_task
        let req =
            make_tools_call_request(4, "stop_replication_task", json!({"task_id": "rep_e2e_1"}));
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_none(),
            "stop_replication_task should succeed: {:?}",
            resp.error
        );
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("\"stopped\": true"),
            "stop result should have stopped=true, got: {text}"
        );
        assert!(
            text.contains("\"state\": \"stopped\""),
            "task state should be stopped, got: {text}"
        );

        // 5. replication_manager_stats
        let req = make_tools_call_request(5, "replication_manager_stats", json!({}));
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_none(),
            "replication_manager_stats should succeed: {:?}",
            resp.error
        );
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("\"total_tasks\": 1"),
            "stats should show 1 total task, got: {text}"
        );
        assert!(
            text.contains("\"total_created\": 1"),
            "stats should show 1 created, got: {text}"
        );
    }

    /// 测试 3：create_replication_task 参数验证 — 缺少必填参数应失败
    #[test]
    fn test_mcp_replication_create_missing_required_params() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        // 缺少 task_id
        let req = make_tools_call_request(
            1,
            "create_replication_task",
            json!({
                "target_type": "memory",
                "target_connection": "memory://test",
            }),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some(), "missing task_id should error");
        assert_eq!(resp.error.unwrap().code, -32602); // InvalidParams

        // 缺少 target_type
        let req = make_tools_call_request(
            2,
            "create_replication_task",
            json!({
                "task_id": "rep_test",
                "target_connection": "memory://test",
            }),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some(), "missing target_type should error");
    }

    /// 测试 4：monitor_replication_task 查询不存在的任务应失败
    #[test]
    fn test_mcp_replication_monitor_nonexistent_task() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        let req = make_tools_call_request(
            1,
            "monitor_replication_task",
            json!({"task_id": "nonexistent_task"}),
        );
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_some(),
            "monitor nonexistent task should error"
        );
    }

    /// 测试 5：stop_replication_task 查询不存在的任务应失败
    #[test]
    fn test_mcp_replication_stop_nonexistent_task() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        let req = make_tools_call_request(
            1,
            "stop_replication_task",
            json!({"task_id": "nonexistent_task"}),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some(), "stop nonexistent task should error");
    }

    /// 测试 6：创建重复任务应失败
    #[test]
    fn test_mcp_replication_create_duplicate_task() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        // 第一次创建成功
        let req = make_tools_call_request(
            1,
            "create_replication_task",
            json!({
                "task_id": "rep_dup",
                "target_type": "memory",
                "target_connection": "memory://test",
            }),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "first create should succeed");

        // 第二次创建相同 task_id 应失败
        let req = make_tools_call_request(
            2,
            "create_replication_task",
            json!({
                "task_id": "rep_dup",
                "target_type": "memory",
                "target_connection": "memory://test",
            }),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some(), "duplicate create should error");
    }

    /// 测试 7：table_filter 参数传递 — 创建带表过滤的任务
    #[test]
    fn test_mcp_replication_create_with_table_filter() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        let req = make_tools_call_request(
            1,
            "create_replication_task",
            json!({
                "task_id": "rep_filter",
                "target_type": "memory",
                "target_connection": "memory://test",
                "table_filter": ["users", "orders"],
                "snapshot_first": false,
            }),
        );
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_none(),
            "create with table_filter should succeed: {:?}",
            resp.error
        );

        // 验证 monitor 返回的 table_filter 字段
        let req = make_tools_call_request(
            2,
            "monitor_replication_task",
            json!({"task_id": "rep_filter"}),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "monitor should succeed");
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("users"),
            "table_filter should contain users, got: {text}"
        );
        assert!(
            text.contains("orders"),
            "table_filter should contain orders, got: {text}"
        );
    }

    /// 测试 8：Kafka 目标端类型创建（使用 MockKafkaProducer）
    #[test]
    fn test_mcp_replication_create_kafka_target() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        let req = make_tools_call_request(
            1,
            "create_replication_task",
            json!({
                "task_id": "rep_kafka",
                "target_type": "kafka",
                "target_connection": "localhost:9092|cdc-events",
                "snapshot_first": false,
            }),
        );
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_none(),
            "create kafka target should succeed: {:?}",
            resp.error
        );
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("\"created\": true"),
            "kafka task should be created, got: {text}"
        );
    }

    /// 测试 9：不支持的目标类型应失败
    #[test]
    fn test_mcp_replication_unsupported_target_type() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        let req = make_tools_call_request(
            1,
            "create_replication_task",
            json!({
                "task_id": "rep_bad",
                "target_type": "unsupported_db",
                "target_connection": "bad://test",
            }),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some(), "unsupported target_type should error");
    }

    /// 测试 10：list_replication_tasks 返回空列表（无任务时）
    #[test]
    fn test_mcp_replication_list_empty() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        let req = make_tools_call_request(1, "list_replication_tasks", json!({}));
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "list should succeed even when empty");
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(text.contains("[]"), "empty list should be [], got: {text}");
    }

    /// 测试 11：多任务场景 — 创建多个任务，list 应返回全部
    #[test]
    fn test_mcp_replication_multiple_tasks() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        // 创建 3 个任务
        for i in 1..=3 {
            let req = make_tools_call_request(
                i,
                "create_replication_task",
                json!({
                    "task_id": format!("rep_multi_{i}"),
                    "target_type": "memory",
                    "target_connection": format!("memory://{i}"),
                }),
            );
            let resp = server.handle_request(&req);
            assert!(resp.error.is_none(), "create task {i} should succeed");
        }

        // list 应返回 3 个任务
        let req = make_tools_call_request(10, "list_replication_tasks", json!({}));
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "list should succeed");
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("rep_multi_1"),
            "list should contain rep_multi_1"
        );
        assert!(
            text.contains("rep_multi_2"),
            "list should contain rep_multi_2"
        );
        assert!(
            text.contains("rep_multi_3"),
            "list should contain rep_multi_3"
        );

        // stats 应显示 total_tasks=3
        let req = make_tools_call_request(11, "replication_manager_stats", json!({}));
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "stats should succeed");
        let result = resp.result.expect("result should be present");
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert!(
            text.contains("\"total_tasks\": 3"),
            "stats should show 3 tasks, got: {text}"
        );
        assert!(
            text.contains("\"running_tasks\": 3"),
            "stats should show 3 running, got: {text}"
        );
    }

    /// 测试 12：monitor_replication_task 缺少 task_id 参数应失败
    #[test]
    fn test_mcp_replication_monitor_missing_task_id() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        let req = make_tools_call_request(1, "monitor_replication_task", json!({}));
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some(), "monitor without task_id should error");
        assert_eq!(resp.error.unwrap().code, -32602); // InvalidParams
    }

    /// 测试 13：stop_replication_task 缺少 task_id 参数应失败
    #[test]
    fn test_mcp_replication_stop_missing_task_id() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        let req = make_tools_call_request(1, "stop_replication_task", json!({}));
        let resp = server.handle_request(&req);
        assert!(resp.error.is_some(), "stop without task_id should error");
        assert_eq!(resp.error.unwrap().code, -32602); // InvalidParams
    }

    /// 测试 14：端到端 CDC 事件流 — 创建任务后触发 WAL 事件，验证 monitor 统计更新
    #[test]
    fn test_mcp_replication_e2e_cdc_event_flow() {
        use szrsql_tx::wal::{WalObserver, WalOpType, WalRecord};

        let (backend, cdc_engine, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        // 1. 创建并启动任务
        let req = make_tools_call_request(
            1,
            "create_replication_task",
            json!({
                "task_id": "rep_e2e_flow",
                "target_type": "memory",
                "target_connection": "memory://flow",
                "snapshot_first": false,
            }),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "create should succeed");

        // 2. 触发 WalRecord 事件（Insert + Commit）
        // 行编码格式：null_flag(1B) + len(4B BE) + data
        let id: i64 = 1;
        let name = b"Alice";
        let mut insert_row = Vec::new();
        insert_row.push(0u8); // id 非 NULL
        insert_row.extend_from_slice(&8u32.to_be_bytes());
        insert_row.extend_from_slice(&id.to_be_bytes());
        insert_row.push(0u8); // name 非 NULL
        insert_row.extend_from_slice(&(name.len() as u32).to_be_bytes());
        insert_row.extend_from_slice(name);

        let records = vec![
            WalRecord::new(1000, 50, WalOpType::Insert, 200, insert_row),
            WalRecord::new(1001, 50, WalOpType::Commit, 0, vec![]),
        ];
        cdc_engine.on_commit(50, records);

        // 3. monitor 应显示已接收事件
        // P7-3：消费者线程异步处理事件，需轮询等待 events_written >= 1 且 transactions_processed >= 1
        // （与 szrsql-cdc/src/task.rs 中 e2e 测试的 wait_for_stats 模式一致）
        let mut monitor_req = || {
            let req = make_tools_call_request(
                2,
                "monitor_replication_task",
                json!({"task_id": "rep_e2e_flow"}),
            );
            let resp = server.handle_request(&req);
            if resp.error.is_some() {
                return None;
            }
            let result = resp.result?;
            let text = result["content"][0]["text"].as_str()?.to_string();
            Some(text)
        };

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
        let text = loop {
            if let Some(t) = monitor_req() {
                if t.contains("\"events_written\": 1")
                    && t.contains("\"transactions_processed\": 1")
                {
                    break t;
                }
            }
            if std::time::Instant::now() > deadline {
                let last = monitor_req().unwrap_or_default();
                panic!("timeout waiting for CDC event processing, got: {last}");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };

        // events_received 应 >= 1（至少接收到 Insert 事件）
        assert!(
            text.contains("\"events_received\": 1") || text.contains("\"events_received\": 2"),
            "events_received should be >= 1, got: {text}"
        );
        // events_written 应为 1（只有 Insert 写入目标端）
        assert!(
            text.contains("\"events_written\": 1"),
            "events_written should be 1, got: {text}"
        );
        // transactions_processed 应为 1（Commit 事件）
        assert!(
            text.contains("\"transactions_processed\": 1"),
            "transactions_processed should be 1, got: {text}"
        );

        // 4. 停止任务
        let req = make_tools_call_request(
            3,
            "stop_replication_task",
            json!({"task_id": "rep_e2e_flow"}),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "stop should succeed");
    }

    /// 测试 15：停止后的任务不能再停止（状态机保护）
    #[test]
    fn test_mcp_replication_stop_already_stopped() {
        let (backend, _cdc, _reg) = build_catalog_backend_with_replication();
        let mut server = McpServerV2::new(Box::new(backend));
        server.initialized = true;

        // 创建任务
        let req = make_tools_call_request(
            1,
            "create_replication_task",
            json!({
                "task_id": "rep_stop_twice",
                "target_type": "memory",
                "target_connection": "memory://test",
            }),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "create should succeed");

        // 第一次停止成功
        let req = make_tools_call_request(
            2,
            "stop_replication_task",
            json!({"task_id": "rep_stop_twice"}),
        );
        let resp = server.handle_request(&req);
        assert!(resp.error.is_none(), "first stop should succeed");

        // 第二次停止应失败（已 Stopped）
        let req = make_tools_call_request(
            3,
            "stop_replication_task",
            json!({"task_id": "rep_stop_twice"}),
        );
        let resp = server.handle_request(&req);
        assert!(
            resp.error.is_some(),
            "second stop should error (already stopped)"
        );
    }
}
