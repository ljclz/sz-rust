//! MCP Server 详细实现 — Phase 7d.22
//!
//! 对应 `SzRSQL技术实现方案.md` Phase 7d.22 MCP Server 详细实现设计。
//!
//! # 设计
//!
//! 在 Phase 7b.6 基础 MCP Server（4 工具）之上，扩展为 26 个 LLM 工具，
//! 覆盖数据库运维全生命周期的 7 大类别：
//!
//! ## 7 个类别 × 26 个工具
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
//!
//! ## 协议
//!
//! 复用 Phase 7b.6 的 JSON-RPC 2.0 over stdio 协议层：
//! - `initialize` / `tools/list` / `tools/call` / `shutdown`
//! - 工具定义包含 `category` 自定义字段，便于 LLM 按类别检索
//!
//! ## 验证标准
//!
//! - MCP Server 启动 → list_tools 返回 26 个工具
//! - query 工具执行 SQL → 返回结果
//! - slow_queries 返回慢查询
//! - 7 个类别全覆盖

use crate::mcp::{
    JsonRpcRequest, JsonRpcResponse, McpError, ToolCallResult, ToolDefinition,
    MCP_PROTOCOL_VERSION, MCP_SERVER_NAME, MCP_SERVER_VERSION,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

// =====================================================================
//  ToolCategory — 工具类别（7 个类别）
// =====================================================================

/// MCP 工具类别 — 7 个类别覆盖数据库运维全生命周期
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
}

// =====================================================================
//  McpBackendV2 — 扩展后端接口（26 个工具方法）
// =====================================================================

/// MCP 扩展后端 — 提供 26 个工具的实际执行能力
///
/// 工具通过后端执行实际操作，便于测试时注入 Mock 后端。
/// 实现方可以桥接到真实的 szrsql-ops / szrsql-tx 模块。
pub trait McpBackendV2 {
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

/// 内存 Mock 后端 — 模拟 26 个工具的返回数据，用于测试和演示
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
                    },
                    crate::mcp::ColumnDef {
                        name: "name".to_string(),
                        data_type: "VARCHAR(255)".to_string(),
                        nullable: false,
                        primary_key: false,
                    },
                    crate::mcp::ColumnDef {
                        name: "price".to_string(),
                        data_type: "DECIMAL(10,2)".to_string(),
                        nullable: true,
                        primary_key: false,
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
                    },
                    crate::mcp::ColumnDef {
                        name: "customer_id".to_string(),
                        data_type: "BIGINT".to_string(),
                        nullable: false,
                        primary_key: false,
                    },
                    crate::mcp::ColumnDef {
                        name: "total".to_string(),
                        data_type: "DECIMAL(10,2)".to_string(),
                        nullable: false,
                        primary_key: false,
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
        })
    }
}

// =====================================================================
//  McpServerV2 — 26 工具 MCP 服务器
// =====================================================================

/// MCP Server V2 — JSON-RPC 2.0 over stdio，26 个 LLM 工具
///
/// 在 Phase 7b.6 `McpServer`（4 工具）基础上扩展为 26 工具，
/// 覆盖 7 个类别：Schema / Query / SlowQuery / TxLock / Perf / Maintenance / Alerting
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

    /// 工具总数
    pub const TOOL_COUNT: usize = 26;

    /// 所有工具定义（26 个，按类别分组）
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
    //  1. 工具总数与类别覆盖测试（验证标准：26 个工具 + 7 个类别全覆盖）
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_tool_count_is_26() {
        let server = McpServerV2::default();
        let tools = server.tool_definitions();
        assert_eq!(tools.len(), 26, "MCP Server V2 must have exactly 26 tools");
        assert_eq!(McpServerV2::TOOL_COUNT, 26);
    }

    #[test]
    fn test_7d22_all_7_categories_covered() {
        let server = McpServerV2::default();
        let counts = server.category_counts();
        assert_eq!(counts.len(), 7, "must have 7 categories");
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
        // 4+4+4+4+4+3+3 = 26
        let total: usize = counts.values().sum();
        assert_eq!(total, 26);
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
    }

    #[test]
    fn test_7d22_tool_category_all() {
        let all = ToolCategory::all();
        assert_eq!(all.len(), 7);
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
    //  4. tools/list 测试（验证标准：list_tools 返回 26 个工具）
    // -----------------------------------------------------------------

    #[test]
    fn test_7d22_tools_list_returns_26() {
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
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 26, "tools/list must return 26 tools");
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

        // Step 2: tools/list → 26 tools
        let r2 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        );
        assert!(r2.contains("list_tables"));
        assert!(r2.contains("execute_sql"));
        assert!(r2.contains("slow_queries"));
        assert!(r2.contains("autovacuum_status"));
        assert!(r2.contains("capacity_predict"));

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

        // Step 6: shutdown
        let r6 = handle_request_json_v2(
            &mut server,
            r#"{"jsonrpc":"2.0","id":6,"method":"shutdown"}"#,
        );
        assert!(r6.contains("result"));
    }

    #[test]
    fn test_7d22_all_26_tools_callable() {
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
}
