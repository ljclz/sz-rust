//! 物化视图 — Phase 6.10 + Phase 6.11 + Phase 6.12 + Phase 6.13 + Phase 6.14 + Phase 6.15
//!
//! 提供物化视图定义、存储与刷新能力。设计目标：
//!
//! - **`ViewDefinition`**：视图元数据（名称、查询、列别名、是否物化）
//! - **`MaterializedViewStore`**：物化视图数据存储（基于 `InMemoryTable`）— Phase 6.11
//! - **`CdcFeed`**：CDC 事件缓冲，捕获源表 INSERT/UPDATE/DELETE 事件 — Phase 6.11 + Phase 6.12 + Phase 6.13 + Phase 6.14
//! - **`REFRESH MATERIALIZED VIEW`**：重新执行查询，将结果写入存储表
//! - **查询重写**：`SELECT * FROM mv` 自动路由到物化视图存储表（而非展开查询）— Phase 6.15
//!
//! # 与 PG 的关系
//!
//! - `CREATE MATERIALIZED VIEW mv AS SELECT ...` 在 catalog 中注册为视图
//! - `SELECT * FROM mv` 路由到物化视图存储表（而非展开查询）— Phase 6.15 实现
//! - `REFRESH MATERIALIZED VIEW mv` 重新执行查询，全量替换存储表数据
//! - Phase 6.10 仅支持全量刷新；Phase 6.11 支持 INSERT_ONLY 增量刷新；Phase 6.12 支持 SIMPLE 增量刷新；Phase 6.13 支持 AGGREGATE 增量刷新；Phase 6.14 支持 GROUP_AGGREGATE 分组聚合增量刷新；Phase 6.15 支持查询重写
//!
//! # 限制
//!
//! - **Phase 6.15 查询重写**：基于视图名路由（非查询等价匹配）；自引用物化视图会导致无限递归（用户错误）；普通视图展开为子查询
//! - **Phase 6.14 GROUP_AGGREGATE 模式**：CDC 捕获 INSERT/DELETE 事件，按分组列值独立维护每组的聚合状态（SUM/COUNT/AVG/MIN/MAX）；新分组自动创建存储行；DELETE 需提供完整旧行以支持 SUM/COUNT/AVG 递减；MIN/MAX 的 DELETE 无法简单递减
//! - **Phase 6.13 AGGREGATE 模式**：CDC 捕获 INSERT/DELETE 事件，按聚合函数（SUM/COUNT/AVG/MIN/MAX）增量更新预聚合值；DELETE 需要提供完整旧行（非仅 pk）以支持 SUM/COUNT/AVG 递减；MIN/MAX 的 DELETE 无法简单递减（需全量重算）
//! - **Phase 6.12 SIMPLE 模式**：CDC 捕获 INSERT/UPDATE/DELETE 事件，按主键合并；不处理聚合（留待 Phase 6.13）
//! - **无并发刷新**：刷新期间读操作会看到部分新数据（无事务隔离）

use crate::ast::{Select, TableName};
use crate::executor::{InMemoryTable, TableStorage};
use szrsql_types::value::Value;

// =====================================================================
//  视图定义
// =====================================================================

/// 视图定义（普通视图与物化视图共用）
///
/// # 字段
/// - `name`：视图名（含可选 schema 前缀）
/// - `columns`：显式列别名（空 Vec 表示未指定，使用查询输出列名）
/// - `query`：视图查询体（`SELECT ...`）
/// - `materialized`：是否为物化视图
///   - `true`：`CREATE MATERIALIZED VIEW`，在 catalog 中注册为可扫描的表
///   - `false`：`CREATE VIEW`，仅存储查询定义，查询时展开为子查询
#[derive(Debug, Clone, PartialEq)]
pub struct ViewDefinition {
    /// 视图名
    pub name: TableName,
    /// 显式列别名
    pub columns: Vec<String>,
    /// 视图查询体
    pub query: Box<Select>,
    /// 是否为物化视图
    pub materialized: bool,
}

impl ViewDefinition {
    /// 创建物化视图定义
    pub fn new_materialized(name: TableName, query: Box<Select>) -> Self {
        Self {
            name,
            columns: Vec::new(),
            query,
            materialized: true,
        }
    }

    /// 创建普通视图定义
    pub fn new_view(name: TableName, query: Box<Select>) -> Self {
        Self {
            name,
            columns: Vec::new(),
            query,
            materialized: false,
        }
    }

    /// 设置列别名
    pub fn with_columns(mut self, columns: Vec<String>) -> Self {
        self.columns = columns;
        self
    }
}

// =====================================================================
//  刷新模式
// =====================================================================

/// 物化视图刷新模式
///
/// Phase 6.10 仅支持 `Full`；增量刷新留待 Phase 6.11+。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefreshMode {
    /// 全量刷新：重新执行查询，全量替换存储表数据
    #[default]
    Full,
    /// INSERT_ONLY 增量刷新：仅追加源表新增行（Phase 6.11）
    InsertOnly,
    /// SIMPLE 增量刷新：按主键合并 INSERT/UPDATE/DELETE（Phase 6.12）
    Simple,
    /// AGGREGATE 增量刷新：聚合值递增/递减（Phase 6.13）
    Aggregate,
    /// GROUP_AGGREGATE 增量刷新：分组聚合独立更新（Phase 6.14）
    GroupAggregate,
}

impl std::fmt::Display for RefreshMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefreshMode::Full => write!(f, "FULL"),
            RefreshMode::InsertOnly => write!(f, "INSERT_ONLY"),
            RefreshMode::Simple => write!(f, "SIMPLE"),
            RefreshMode::Aggregate => write!(f, "AGGREGATE"),
            RefreshMode::GroupAggregate => write!(f, "GROUP_AGGREGATE"),
        }
    }
}

// =====================================================================
//  刷新状态
// =====================================================================

/// 物化视图刷新状态
///
/// 跟踪物化视图的最后刷新时间与刷新行数，供监控与增量刷新使用。
#[derive(Debug, Clone, Default)]
pub struct RefreshState {
    /// 是否已初始化（已执行过至少一次 REFRESH）
    pub initialized: bool,
    /// 最后刷新行数
    pub last_row_count: usize,
    /// 最后刷新时间戳（Unix 微秒）
    pub last_refresh_timestamp: i64,
    /// 刷新模式
    pub mode: RefreshMode,
}

impl RefreshState {
    /// 创建未初始化的刷新状态
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建已初始化的刷新状态
    pub fn initialized(row_count: usize, timestamp: i64, mode: RefreshMode) -> Self {
        Self {
            initialized: true,
            last_row_count: row_count,
            last_refresh_timestamp: timestamp,
            mode,
        }
    }

    /// 更新刷新状态
    pub fn update(&mut self, row_count: usize, timestamp: i64) {
        self.initialized = true;
        self.last_row_count = row_count;
        self.last_refresh_timestamp = timestamp;
    }
}

// =====================================================================
//  CDC 事件 — Phase 6.11 + Phase 6.12
// =====================================================================

/// CDC（Change Data Capture）事件 — Phase 6.11 + Phase 6.12 + Phase 6.13
///
/// 跟踪源表的数据变更，供物化视图增量刷新消费。
///
/// # 变体
/// - `Insert`：源表新增一行（Phase 6.11）
/// - `Update`：源表更新一行（按主键定位）— Phase 6.12
/// - `Delete`：源表删除一行（按主键定位）— Phase 6.12 + Phase 6.13
///
/// # 设计
///
/// CDC 事件由调用方在执行 DML 时显式推送到 `CdcFeed`。
/// 物化视图刷新引擎从 feed 中 drain 事件并应用到物化视图存储。
///
/// `Insert.row` / `Update.row` 为已通过视图查询投影的行（与物化视图存储表 Schema 一致）。
/// `Update.pk` / `Delete.pk` 为源表主键值（投影后存储表中的主键列值，用于按 key 合并）。
/// `Delete.row` 为可选的完整旧行（Phase 6.13 AGGREGATE 模式需要用于 SUM/COUNT 递减）。
#[derive(Debug, Clone, PartialEq)]
pub enum CdcEvent {
    /// 源表 INSERT 事件
    ///
    /// `source_table`：源表名；`row`：已通过视图查询投影的行（即物化视图存储格式）
    Insert {
        /// 源表名
        source_table: TableName,
        /// 投影后的行（与物化视图存储表 Schema 一致）
        row: Vec<Value>,
    },
    /// 源表 UPDATE 事件 — Phase 6.12
    ///
    /// `source_table`：源表名；`pk`：主键值（用于按 key 合并）；`row`：投影后的新行
    Update {
        /// 源表名
        source_table: TableName,
        /// 主键值（与物化视图存储表主键列对应）
        pk: Vec<Value>,
        /// 投影后的新行（与物化视图存储表 Schema 一致）
        row: Vec<Value>,
    },
    /// 源表 DELETE 事件 — Phase 6.12 + Phase 6.13
    ///
    /// `source_table`：源表名；`pk`：主键值（用于按 key 合并）；
    /// `row`：可选的完整旧行（Phase 6.13 AGGREGATE 模式用于 SUM/COUNT 递减）
    Delete {
        /// 源表名
        source_table: TableName,
        /// 主键值（与物化视图存储表主键列对应）
        pk: Vec<Value>,
        /// 可选的完整旧行（Phase 6.13 AGGREGATE 模式需要）
        row: Option<Vec<Value>>,
    },
}

impl CdcEvent {
    /// 创建 INSERT 事件
    pub fn insert(source_table: impl Into<String>, row: Vec<Value>) -> Self {
        Self::Insert {
            source_table: TableName::new(source_table),
            row,
        }
    }

    /// 创建 UPDATE 事件 — Phase 6.12
    pub fn update(source_table: impl Into<String>, pk: Vec<Value>, row: Vec<Value>) -> Self {
        Self::Update {
            source_table: TableName::new(source_table),
            pk,
            row,
        }
    }

    /// 创建 DELETE 事件（仅 pk，SIMPLE 模式）— Phase 6.12
    pub fn delete(source_table: impl Into<String>, pk: Vec<Value>) -> Self {
        Self::Delete {
            source_table: TableName::new(source_table),
            pk,
            row: None,
        }
    }

    /// 创建 DELETE 事件（带完整旧行，AGGREGATE 模式）— Phase 6.13
    pub fn delete_with_row(
        source_table: impl Into<String>,
        pk: Vec<Value>,
        row: Vec<Value>,
    ) -> Self {
        Self::Delete {
            source_table: TableName::new(source_table),
            pk,
            row: Some(row),
        }
    }

    /// 获取事件类型字符串（用于日志/调试）
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Insert { .. } => "INSERT",
            Self::Update { .. } => "UPDATE",
            Self::Delete { .. } => "DELETE",
        }
    }

    /// 获取源表名引用
    pub fn source_table(&self) -> &TableName {
        match self {
            Self::Insert { source_table, .. }
            | Self::Update { source_table, .. }
            | Self::Delete { source_table, .. } => source_table,
        }
    }
}

/// CDC 事件缓冲 — Phase 6.11 + Phase 6.12
///
/// FIFO 缓冲区，存储待消费的 CDC 事件。
///
/// # 用法
///
/// ```ignore
/// let mut feed = CdcFeed::new();
/// feed.push_insert("users", vec![Value::Int64(1), Value::Text("Alice".into())]);
/// feed.push_update("users", vec![Value::Int64(1)], vec![Value::Int64(1), Value::Text("Bob".into())]);
/// feed.push_delete("users", vec![Value::Int64(1)]);
/// let events = feed.drain();
/// assert_eq!(events.len(), 3);
/// ```
#[derive(Debug, Clone, Default)]
pub struct CdcFeed {
    events: Vec<CdcEvent>,
}

impl CdcFeed {
    /// 创建空 feed
    pub fn new() -> Self {
        Self::default()
    }

    /// 推送 INSERT 事件
    pub fn push_insert(&mut self, source_table: impl Into<String>, row: Vec<Value>) {
        self.events.push(CdcEvent::insert(source_table, row));
    }

    /// 推送 UPDATE 事件 — Phase 6.12
    pub fn push_update(
        &mut self,
        source_table: impl Into<String>,
        pk: Vec<Value>,
        row: Vec<Value>,
    ) {
        self.events.push(CdcEvent::update(source_table, pk, row));
    }

    /// 推送 DELETE 事件 — Phase 6.12
    pub fn push_delete(&mut self, source_table: impl Into<String>, pk: Vec<Value>) {
        self.events.push(CdcEvent::delete(source_table, pk));
    }

    /// 推送带完整旧行的 DELETE 事件 — Phase 6.13 AGGREGATE 模式
    ///
    /// `row` 为被删除行的完整投影值，用于 SUM/COUNT/AVG 聚合状态递减。
    pub fn push_delete_with_row(
        &mut self,
        source_table: impl Into<String>,
        pk: Vec<Value>,
        row: Vec<Value>,
    ) {
        self.events
            .push(CdcEvent::delete_with_row(source_table, pk, row));
    }

    /// 推送预构造的事件
    pub fn push(&mut self, event: CdcEvent) {
        self.events.push(event);
    }

    /// 批量推送 INSERT 事件
    pub fn push_inserts(
        &mut self,
        source_table: impl Into<String>,
        rows: impl IntoIterator<Item = Vec<Value>>,
    ) {
        let source_table: String = source_table.into();
        for row in rows {
            self.events
                .push(CdcEvent::insert(source_table.clone(), row));
        }
    }

    /// 消费所有事件（清空缓冲）
    pub fn drain(&mut self) -> Vec<CdcEvent> {
        std::mem::take(&mut self.events)
    }

    /// 当前缓冲事件数
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 预览事件（不消费）
    pub fn peek(&self) -> &[CdcEvent] {
        &self.events
    }
}

// =====================================================================
//  聚合函数与状态 — Phase 6.13
// =====================================================================

/// 聚合函数类型 — Phase 6.13
///
/// 用于 AGGREGATE 增量刷新模式，物化视图预聚合值随 CDC 事件递增/递减。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateFunction {
    /// SUM：累加与递减
    Sum,
    /// COUNT：计数与递减
    Count,
    /// AVG：通过 SUM/COUNT 派生
    Avg,
    /// MIN：取最小值（INSERT 可增量；DELETE 需全量重算）
    Min,
    /// MAX：取最大值（INSERT 可增量；DELETE 需全量重算）
    Max,
}

impl AggregateFunction {
    /// 是否支持 DELETE 递减（即不需要全量重算）
    pub fn supports_decrement(&self) -> bool {
        matches!(self, Self::Sum | Self::Count | Self::Avg)
    }
}

impl std::fmt::Display for AggregateFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sum => write!(f, "SUM"),
            Self::Count => write!(f, "COUNT"),
            Self::Avg => write!(f, "AVG"),
            Self::Min => write!(f, "MIN"),
            Self::Max => write!(f, "MAX"),
        }
    }
}

/// 聚合规格 — Phase 6.13
///
/// 描述物化视图的一个聚合列：源表某列按指定聚合函数计算，结果写入物化视图存储表某列。
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateSpec {
    /// 聚合函数
    pub function: AggregateFunction,
    /// 源表列索引（在 CDC 事件 row 中的列位置）
    pub source_column: usize,
    /// 物化视图存储表列索引（聚合值写入位置）
    pub output_column: usize,
}

impl AggregateSpec {
    /// 创建聚合规格
    pub fn new(function: AggregateFunction, source_column: usize, output_column: usize) -> Self {
        Self {
            function,
            source_column,
            output_column,
        }
    }
}

/// 聚合运行状态 — Phase 6.13
///
/// 跟踪一个聚合列的运行值。不同聚合函数使用不同字段：
/// - `Sum` / `Avg`：使用 `sum` + `count`
/// - `Count`：使用 `count`（忽略源列值，对行计数）
/// - `Min`：使用 `min`
/// - `Max`：使用 `max`
#[derive(Debug, Clone, Default)]
pub struct AggregateState {
    /// SUM/AVG 的累加值（f64 以兼容整数与浮点）
    pub sum: f64,
    /// COUNT/AVG 的计数
    pub count: i64,
    /// MIN 的当前最小值
    pub min: Option<Value>,
    /// MAX 的当前最大值
    pub max: Option<Value>,
}

impl AggregateState {
    /// 创建空状态
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用一次 INSERT（递增聚合值）
    pub fn apply_insert(&mut self, function: AggregateFunction, value: &Value) {
        match function {
            AggregateFunction::Sum | AggregateFunction::Avg => {
                if let Some(n) = value_to_f64(value) {
                    self.sum += n;
                    self.count += 1;
                }
            }
            AggregateFunction::Count => {
                self.count += 1;
            }
            AggregateFunction::Min => {
                if self.min.is_none() || value_lt(value, self.min.as_ref().unwrap()) {
                    self.min = Some(value.clone());
                }
            }
            AggregateFunction::Max => {
                if self.max.is_none() || value_gt(value, self.max.as_ref().unwrap()) {
                    self.max = Some(value.clone());
                }
            }
        }
    }

    /// 应用一次 DELETE（递减聚合值）
    ///
    /// 返回 `true` 表示递减成功；`false` 表示该聚合函数不支持递减（MIN/MAX）。
    pub fn apply_delete(&mut self, function: AggregateFunction, value: &Value) -> bool {
        match function {
            AggregateFunction::Sum | AggregateFunction::Avg => {
                if let Some(n) = value_to_f64(value) {
                    self.sum -= n;
                    self.count -= 1;
                    true
                } else {
                    // 非数值不参与聚合，但视为递减成功（no-op）
                    true
                }
            }
            AggregateFunction::Count => {
                self.count -= 1;
                true
            }
            AggregateFunction::Min | AggregateFunction::Max => {
                // MIN/MAX 无法简单递减
                false
            }
        }
    }

    /// 计算当前聚合值
    pub fn current_value(&self, function: AggregateFunction) -> Value {
        match function {
            AggregateFunction::Sum => Value::Float64(self.sum),
            AggregateFunction::Count => Value::Int64(self.count),
            AggregateFunction::Avg => {
                if self.count == 0 {
                    Value::Null
                } else {
                    Value::Float64(self.sum / self.count as f64)
                }
            }
            AggregateFunction::Min => self.min.clone().unwrap_or(Value::Null),
            AggregateFunction::Max => self.max.clone().unwrap_or(Value::Null),
        }
    }
}

/// 将 `Value` 转换为 `f64`（仅数值类型可转换）
fn value_to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Int64(i) => Some(*i as f64),
        Value::Float64(f) => Some(*f),
        Value::Decimal(unscaled, scale) => {
            let scale_factor = 10f64.powi(*scale as i32);
            Some(*unscaled as f64 / scale_factor)
        }
        _ => None,
    }
}

/// 比较 `a < b`（基于 `Value` 的 `PartialOrd` 近似；NULL 视为最大）
fn value_lt(a: &Value, b: &Value) -> bool {
    matches!(compare_values(a, b), Some(std::cmp::Ordering::Less))
}

/// 比较 `a > b`
fn value_gt(a: &Value, b: &Value) -> bool {
    matches!(compare_values(a, b), Some(std::cmp::Ordering::Greater))
}

/// 比较 `Value`（NULL 视为最小；不同类型按类型名排序）
fn compare_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Some(Ordering::Equal),
        (Value::Null, _) => Some(Ordering::Less),
        (_, Value::Null) => Some(Ordering::Greater),
        (Value::Int64(x), Value::Int64(y)) => Some(x.cmp(y)),
        (Value::Float64(x), Value::Float64(y)) => x.partial_cmp(y),
        (Value::Int64(x), Value::Float64(y)) => (*x as f64).partial_cmp(y),
        (Value::Float64(x), Value::Int64(y)) => x.partial_cmp(&(*y as f64)),
        (Value::Text(x), Value::Text(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::Date(x), Value::Date(y)) => Some(x.cmp(y)),
        (Value::Timestamp(x), Value::Timestamp(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

// =====================================================================
//  分组聚合条目 — Phase 6.14
// =====================================================================

/// 分组聚合条目 — Phase 6.14 GROUP_AGGREGATE 模式
///
/// 跟踪一个分组（由分组列值标识）的聚合运行状态。
/// 每个分组在存储表中对应一行（`row_id`），并维护与 `aggregate_specs`
/// 一一对应的 `AggregateState` 列表。
#[derive(Debug, Clone)]
struct GroupAggregateEntry {
    /// 该分组在 storage 中的 row_id
    row_id: usize,
    /// 每个聚合规格的运行状态（与 `aggregate_specs` 一一对应）
    states: Vec<AggregateState>,
}

// =====================================================================
//  物化视图存储 — Phase 6.11 + Phase 6.12 + Phase 6.13 + Phase 6.14
// =====================================================================

/// 物化视图存储 — Phase 6.11 + Phase 6.12 + Phase 6.13 + Phase 6.14
///
/// 包装 `InMemoryTable` 存储物化视图数据，并跟踪刷新状态。
///
/// # 设计
///
/// - `storage`：物化视图数据表（列 Schema 与视图查询输出一致）
/// - `refresh_state`：跟踪最后刷新行数、时间戳、模式
/// - `high_water_marks`：源表高水位（源表名小写 → 已见行数）
///   - 用于 INSERT_ONLY 模式：仅处理 `source.rows()[hwm..]` 的新增行
///   - Phase 6.11 主要使用 CDC feed，高水位作为辅助追踪
/// - `pk_indices`：主键列在存储表中的列索引（Phase 6.12 SIMPLE 模式用于按 key 合并）
/// - `pk_index_map`：主键序列化字符串 → row_id 索引（Phase 6.12 SIMPLE 模式快速定位行）
///   - key 为 `serde_json::to_string(&pk_values)`，避免 `Value` 未实现 `Hash` 的限制
/// - `aggregate_specs`：聚合规格列表（Phase 6.13 AGGREGATE + Phase 6.14 GROUP_AGGREGATE 模式共用）
/// - `aggregate_states`：聚合运行状态列表（与 `aggregate_specs` 一一对应，Phase 6.13 单行全局聚合）
/// - `aggregate_row_id`：聚合行在 storage 中的 row_id（Phase 6.13，单行聚合结果）
/// - `group_column_indices`：分组列在源行中的列索引（Phase 6.14 GROUP_AGGREGATE 模式）
/// - `group_output_indices`：分组列在存储表中的输出列索引（Phase 6.14 GROUP_AGGREGATE 模式）
/// - `group_states`：分组聚合状态映射（group key 字符串 → `GroupAggregateEntry`，Phase 6.14）
///
/// # 用法
///
/// ```ignore
/// let store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
/// let mut store = store;
/// store.append_row(vec![Value::Int64(1)]);
/// assert_eq!(store.row_count(), 1);
/// ```
#[derive(Debug)]
pub struct MaterializedViewStore {
    /// 物化视图数据存储表
    pub storage: InMemoryTable,
    /// 刷新状态
    pub refresh_state: RefreshState,
    /// 源表高水位（源表名小写 → 已见行数）
    high_water_marks: std::collections::HashMap<String, usize>,
    /// 主键列索引（Phase 6.12 SIMPLE 模式）
    pk_indices: Vec<usize>,
    /// 主键序列化字符串 → row_id 索引（Phase 6.12 SIMPLE 模式快速定位）
    pk_index_map: std::collections::HashMap<String, usize>,
    /// 聚合规格列表（Phase 6.13 AGGREGATE + Phase 6.14 GROUP_AGGREGATE 模式共用）
    aggregate_specs: Vec<AggregateSpec>,
    /// 聚合运行状态列表（与 aggregate_specs 一一对应，Phase 6.13 单行全局聚合）
    aggregate_states: Vec<AggregateState>,
    /// 聚合行在 storage 中的 row_id（Phase 6.13，单行聚合结果）
    aggregate_row_id: Option<usize>,
    /// 分组列在源行中的列索引（Phase 6.14 GROUP_AGGREGATE 模式）
    group_column_indices: Vec<usize>,
    /// 分组列在存储表中的输出列索引（Phase 6.14 GROUP_AGGREGATE 模式）
    group_output_indices: Vec<usize>,
    /// 分组聚合状态映射（group key 字符串 → `GroupAggregateEntry`，Phase 6.14）
    group_states: std::collections::HashMap<String, GroupAggregateEntry>,
}

impl MaterializedViewStore {
    /// 创建物化视图存储（使用列定义）
    pub fn new(name: &str, columns: Vec<(&str, szrsql_types::value::ColumnType)>) -> Self {
        Self {
            storage: InMemoryTable::with_columns(name, columns),
            refresh_state: RefreshState::new(),
            high_water_marks: std::collections::HashMap::new(),
            pk_indices: Vec::new(),
            pk_index_map: std::collections::HashMap::new(),
            aggregate_specs: Vec::new(),
            aggregate_states: Vec::new(),
            aggregate_row_id: None,
            group_column_indices: Vec::new(),
            group_output_indices: Vec::new(),
            group_states: std::collections::HashMap::new(),
        }
    }

    /// 创建带主键的物化视图存储 — Phase 6.12
    ///
    /// `pk_indices` 为主键列在 columns 中的索引位置。
    pub fn new_with_pk(
        name: &str,
        columns: Vec<(&str, szrsql_types::value::ColumnType)>,
        pk_indices: Vec<usize>,
    ) -> Self {
        Self {
            storage: InMemoryTable::with_columns(name, columns),
            refresh_state: RefreshState::new(),
            high_water_marks: std::collections::HashMap::new(),
            pk_indices,
            pk_index_map: std::collections::HashMap::new(),
            aggregate_specs: Vec::new(),
            aggregate_states: Vec::new(),
            aggregate_row_id: None,
            group_column_indices: Vec::new(),
            group_output_indices: Vec::new(),
            group_states: std::collections::HashMap::new(),
        }
    }

    /// 创建带聚合规格的物化视图存储 — Phase 6.13
    ///
    /// `specs` 为聚合规格列表（源列索引 → 聚合函数 → 输出列索引）。
    /// 创建时会自动初始化一行聚合结果（全 NULL），并跟踪其 row_id。
    pub fn new_with_aggregates(
        name: &str,
        columns: Vec<(&str, szrsql_types::value::ColumnType)>,
        specs: Vec<AggregateSpec>,
    ) -> Self {
        let mut store = Self {
            storage: InMemoryTable::with_columns(name, columns),
            refresh_state: RefreshState::new(),
            high_water_marks: std::collections::HashMap::new(),
            pk_indices: Vec::new(),
            pk_index_map: std::collections::HashMap::new(),
            aggregate_specs: specs,
            aggregate_states: Vec::new(),
            aggregate_row_id: None,
            group_column_indices: Vec::new(),
            group_output_indices: Vec::new(),
            group_states: std::collections::HashMap::new(),
        };
        // 初始化聚合状态（每个 spec 对应一个 state）
        store.aggregate_states = (0..store.aggregate_specs.len())
            .map(|_| AggregateState::new())
            .collect();
        // 初始化聚合结果行（全 NULL，后续 refresh 时更新）
        let null_row = vec![Value::Null; store.storage.schema().columns.len()];
        let row_id = store.storage.insert(null_row);
        store.aggregate_row_id = Some(row_id);
        store
    }

    /// 创建带分组聚合规格的物化视图存储 — Phase 6.14
    ///
    /// `group_column_indices` 为分组列在源 CDC 行中的列索引列表；
    /// `group_output_indices` 为分组列在存储表中的输出列索引列表（与 `group_column_indices` 一一对应）；
    /// `specs` 为聚合规格列表（每个分组独立维护一份聚合状态）。
    ///
    /// 创建时 `group_states` 为空，首次 INSERT 时按分组键自动创建存储行。
    pub fn new_with_group_aggregates(
        name: &str,
        columns: Vec<(&str, szrsql_types::value::ColumnType)>,
        group_column_indices: Vec<usize>,
        group_output_indices: Vec<usize>,
        specs: Vec<AggregateSpec>,
    ) -> Self {
        assert_eq!(
            group_column_indices.len(),
            group_output_indices.len(),
            "group_column_indices and group_output_indices must have same length"
        );
        Self {
            storage: InMemoryTable::with_columns(name, columns),
            refresh_state: RefreshState::new(),
            high_water_marks: std::collections::HashMap::new(),
            pk_indices: Vec::new(),
            pk_index_map: std::collections::HashMap::new(),
            aggregate_specs: specs,
            aggregate_states: Vec::new(),
            aggregate_row_id: None,
            group_column_indices,
            group_output_indices,
            group_states: std::collections::HashMap::new(),
        }
    }

    /// 从 `InMemoryTable` 创建物化视图存储
    pub fn from_table(table: InMemoryTable) -> Self {
        Self {
            storage: table,
            refresh_state: RefreshState::new(),
            high_water_marks: std::collections::HashMap::new(),
            pk_indices: Vec::new(),
            pk_index_map: std::collections::HashMap::new(),
            aggregate_specs: Vec::new(),
            aggregate_states: Vec::new(),
            aggregate_row_id: None,
            group_column_indices: Vec::new(),
            group_output_indices: Vec::new(),
            group_states: std::collections::HashMap::new(),
        }
    }

    /// 追加一行到物化视图存储（不经过 CDC）
    ///
    /// 如果设置了主键索引，同时更新主键索引。
    pub fn append_row(&mut self, row: Vec<Value>) {
        let row_id = self.storage.insert(row.clone());
        if !self.pk_indices.is_empty() {
            let pk_key = self.extract_pk_key(&row);
            self.pk_index_map.insert(pk_key, row_id);
        }
    }

    /// 批量追加多行
    pub fn append_rows(&mut self, rows: impl IntoIterator<Item = Vec<Value>>) {
        for row in rows {
            self.append_row(row);
        }
    }

    /// 按主键合并一行（UPSERT）— Phase 6.12
    ///
    /// 如果主键已存在，替换对应行；否则追加新行。
    ///
    /// 返回 `(was_insert, was_update)`：
    /// - `(true, false)`：新插入
    /// - `(false, true)`：更新已有行
    pub fn upsert_row(&mut self, row: Vec<Value>) -> (bool, bool) {
        if self.pk_indices.is_empty() {
            // 无主键索引，退化为追加
            self.append_row(row);
            return (true, false);
        }
        let pk_key = self.extract_pk_key(&row);
        if let Some(&row_id) = self.pk_index_map.get(&pk_key) {
            // 主键存在，替换行
            self.storage.update_row(row_id, row);
            (false, true)
        } else {
            // 主键不存在，追加
            let row_id = self.storage.insert(row.clone());
            self.pk_index_map.insert(pk_key, row_id);
            (true, false)
        }
    }

    /// 按主键删除一行 — Phase 6.12
    ///
    /// 返回 `true` 如果删除成功；`false` 如果主键不存在或已删除。
    pub fn delete_by_pk(&mut self, pk: &[Value]) -> bool {
        let pk_key = self.pk_values_to_key(pk);
        if let Some(&row_id) = self.pk_index_map.get(&pk_key) {
            let deleted = self.storage.delete_row(row_id);
            if deleted {
                self.pk_index_map.remove(&pk_key);
            }
            deleted
        } else {
            false
        }
    }

    /// 提取行的主键值
    fn extract_pk(&self, row: &[Value]) -> Vec<Value> {
        self.pk_indices
            .iter()
            .map(|&i| row.get(i).cloned().unwrap_or(Value::Null))
            .collect()
    }

    /// 提取行的主键序列化 key
    fn extract_pk_key(&self, row: &[Value]) -> String {
        let pk = self.extract_pk(row);
        self.pk_values_to_key(&pk)
    }

    /// 将主键值列表序列化为字符串 key
    ///
    /// 使用 `Debug` 格式化避免依赖 `serde_json`（`Value` 实现了 `Debug`）。
    /// 同一组 `Vec<Value>` 总是产生相同的字符串。
    fn pk_values_to_key(&self, pk: &[Value]) -> String {
        format!("{pk:?}")
    }

    /// 当前物化视图存储行数（含已删除的行）
    pub fn row_count(&self) -> usize {
        self.storage.rows().len()
    }

    /// 活跃行数（排除 tombstone）— Phase 6.12
    pub fn active_row_count(&self) -> usize {
        self.storage.row_count()
    }

    /// 获取存储表所有行的引用（含已删除的行）
    pub fn rows(&self) -> &[Vec<Value>] {
        self.storage.rows()
    }

    /// 获取主键列索引
    pub fn pk_indices(&self) -> &[usize] {
        &self.pk_indices
    }

    /// 是否已设置主键索引
    pub fn has_primary_key(&self) -> bool {
        !self.pk_indices.is_empty()
    }

    /// 获取源表高水位（默认 0）
    pub fn high_water_mark(&self, source_table: &TableName) -> usize {
        let key = source_table.name.to_lowercase();
        *self.high_water_marks.get(&key).unwrap_or(&0)
    }

    /// 设置源表高水位
    pub fn set_high_water_mark(&mut self, source_table: &TableName, hwm: usize) {
        let key = source_table.name.to_lowercase();
        self.high_water_marks.insert(key, hwm);
    }

    /// 清空物化视图存储（用于全量刷新前的清理）
    pub fn clear(&mut self) {
        // InMemoryTable 没有公开的 clear 方法，通过重建实现
        let schema = self.storage.schema().clone();
        self.storage = InMemoryTable::new(schema);
        self.refresh_state = RefreshState::new();
        self.high_water_marks.clear();
        self.pk_index_map.clear();
        // Phase 6.13: 重置全局聚合状态
        for state in &mut self.aggregate_states {
            *state = AggregateState::new();
        }
        // 重建全局聚合结果行
        if !self.aggregate_specs.is_empty() && self.group_column_indices.is_empty() {
            let null_row = vec![Value::Null; self.storage.schema().columns.len()];
            let row_id = self.storage.insert(null_row);
            self.aggregate_row_id = Some(row_id);
        } else {
            self.aggregate_row_id = None;
        }
        // Phase 6.14: 清空分组聚合状态
        self.group_states.clear();
    }

    // -----------------------------------------------------------------
    //  Phase 6.13: 聚合访问器与方法
    // -----------------------------------------------------------------

    /// 获取聚合规格列表 — Phase 6.13
    pub fn aggregate_specs(&self) -> &[AggregateSpec] {
        &self.aggregate_specs
    }

    /// 获取聚合状态列表（只读）— Phase 6.13
    pub fn aggregate_states(&self) -> &[AggregateState] {
        &self.aggregate_states
    }

    /// 是否已配置聚合规格 — Phase 6.13
    pub fn has_aggregates(&self) -> bool {
        !self.aggregate_specs.is_empty()
    }

    /// 获取聚合行 row_id — Phase 6.13
    pub fn aggregate_row_id(&self) -> Option<usize> {
        self.aggregate_row_id
    }

    /// 应用一次 INSERT 到聚合状态 — Phase 6.13
    ///
    /// 对每个聚合规格，提取源列值并递增对应聚合状态。
    pub fn apply_aggregate_insert(&mut self, row: &[Value]) {
        for (spec, state) in self
            .aggregate_specs
            .iter()
            .zip(self.aggregate_states.iter_mut())
        {
            let value = row.get(spec.source_column).cloned().unwrap_or(Value::Null);
            state.apply_insert(spec.function, &value);
        }
        self.flush_aggregate_row();
    }

    /// 应用一次 DELETE 到聚合状态 — Phase 6.13
    ///
    /// 对每个聚合规格，提取源列值并递减对应聚合状态。
    /// 返回 `true` 表示所有聚合都成功递减；`false` 表示至少一个聚合（MIN/MAX）无法递减。
    pub fn apply_aggregate_delete(&mut self, row: &[Value]) -> bool {
        let mut all_ok = true;
        for (spec, state) in self
            .aggregate_specs
            .iter()
            .zip(self.aggregate_states.iter_mut())
        {
            let value = row.get(spec.source_column).cloned().unwrap_or(Value::Null);
            if !state.apply_delete(spec.function, &value) {
                all_ok = false;
            }
        }
        self.flush_aggregate_row();
        all_ok
    }

    /// 将聚合状态写入存储表的聚合行 — Phase 6.13
    ///
    /// 对每个聚合规格，将当前聚合值写入聚合行的对应输出列。
    fn flush_aggregate_row(&mut self) {
        let row_id = match self.aggregate_row_id {
            Some(id) => id,
            None => return,
        };
        // 构造新的聚合结果行（保留原行的其他列不变）
        let mut new_row = match self.storage.get_row(row_id) {
            Some(r) => r.to_vec(),
            None => return,
        };
        for (spec, state) in self
            .aggregate_specs
            .iter()
            .zip(self.aggregate_states.iter())
        {
            if spec.output_column < new_row.len() {
                new_row[spec.output_column] = state.current_value(spec.function);
            }
        }
        // 使用 update_row 替换聚合行（注意：聚合行不应被 tombstone）
        // 若 update_row 失败（行已删除），则重新插入
        if !self.storage.update_row(row_id, new_row) {
            // 回退：重新插入聚合行
            let null_row = vec![Value::Null; self.storage.schema().columns.len()];
            let new_id = self.storage.insert(null_row);
            self.aggregate_row_id = Some(new_id);
        }
    }

    // -----------------------------------------------------------------
    //  Phase 6.14: 分组聚合访问器与方法
    // -----------------------------------------------------------------

    /// 是否已配置分组聚合 — Phase 6.14
    ///
    /// 返回 `true` 当且仅当同时配置了分组列索引和聚合规格。
    pub fn has_group_aggregates(&self) -> bool {
        !self.group_column_indices.is_empty() && !self.aggregate_specs.is_empty()
    }

    /// 获取分组列在源行中的索引列表 — Phase 6.14
    pub fn group_column_indices(&self) -> &[usize] {
        &self.group_column_indices
    }

    /// 获取分组列在存储表中的输出列索引列表 — Phase 6.14
    pub fn group_output_indices(&self) -> &[usize] {
        &self.group_output_indices
    }

    /// 获取当前分组数量 — Phase 6.14
    pub fn group_count(&self) -> usize {
        self.group_states.len()
    }

    /// 从源行提取分组键字符串 — Phase 6.14
    ///
    /// 按 `group_column_indices` 提取各分组列的 `Value`，序列化为 `Debug` 字符串。
    fn extract_group_key(&self, row: &[Value]) -> String {
        let pk: Vec<Value> = self
            .group_column_indices
            .iter()
            .map(|&i| row.get(i).cloned().unwrap_or(Value::Null))
            .collect();
        format!("{pk:?}")
    }

    /// 应用一次 INSERT 到分组聚合状态 — Phase 6.14
    ///
    /// 提取分组键，查找或创建该分组的聚合状态条目，
    /// 然后对每个聚合规格递增该分组的聚合值。
    ///
    /// 返回 `true` 表示创建了新分组（首次见到该分组键）；`false` 表示分组已存在。
    pub fn apply_group_aggregate_insert(&mut self, row: &[Value]) -> bool {
        let group_key = self.extract_group_key(row);
        let is_new = !self.group_states.contains_key(&group_key);
        if is_new {
            // 创建新分组：初始化聚合状态 + 插入存储行
            let states: Vec<AggregateState> = (0..self.aggregate_specs.len())
                .map(|_| AggregateState::new())
                .collect();
            // 构造初始存储行：分组列写入分组键值，聚合列暂为 NULL
            let mut new_row = vec![Value::Null; self.storage.schema().columns.len()];
            for (src_idx, &out_idx) in self
                .group_column_indices
                .iter()
                .zip(self.group_output_indices.iter())
            {
                if out_idx < new_row.len() {
                    new_row[out_idx] = row.get(*src_idx).cloned().unwrap_or(Value::Null);
                }
            }
            let row_id = self.storage.insert(new_row);
            self.group_states
                .insert(group_key.clone(), GroupAggregateEntry { row_id, states });
        }
        // 递增该分组的聚合状态
        let entry = self.group_states.get_mut(&group_key).expect("just ensured");
        for (spec, state) in self.aggregate_specs.iter().zip(entry.states.iter_mut()) {
            let value = row.get(spec.source_column).cloned().unwrap_or(Value::Null);
            state.apply_insert(spec.function, &value);
        }
        self.flush_group_aggregate_row(&group_key);
        is_new
    }

    /// 应用一次 DELETE 到分组聚合状态 — Phase 6.14
    ///
    /// 提取分组键，查找该分组的聚合状态条目，对每个聚合规格递减聚合值。
    /// 返回 `true` 表示所有聚合都成功递减；`false` 表示至少一个聚合（MIN/MAX）无法递减
    /// 或分组不存在（视为 no-op 但返回 `false`）。
    pub fn apply_group_aggregate_delete(&mut self, row: &[Value]) -> bool {
        let group_key = self.extract_group_key(row);
        let Some(entry) = self.group_states.get_mut(&group_key) else {
            // 分组不存在：no-op，返回 false
            return false;
        };
        let mut all_ok = true;
        for (spec, state) in self.aggregate_specs.iter().zip(entry.states.iter_mut()) {
            let value = row.get(spec.source_column).cloned().unwrap_or(Value::Null);
            if !state.apply_delete(spec.function, &value) {
                all_ok = false;
            }
        }
        self.flush_group_aggregate_row(&group_key);
        all_ok
    }

    /// 将指定分组的聚合状态写入存储表对应行 — Phase 6.14
    ///
    /// 对每个聚合规格，将当前聚合值写入该分组存储行的对应输出列。
    /// 分组列值保持不变（在 INSERT 时已写入）。
    fn flush_group_aggregate_row(&mut self, group_key: &str) {
        let row_id = match self.group_states.get(group_key) {
            Some(entry) => entry.row_id,
            None => return,
        };
        // 获取当前存储行（保留分组列值）
        let mut new_row = match self.storage.get_row(row_id) {
            Some(r) => r.to_vec(),
            None => return,
        };
        // 需要重新借 group_states 获取 states（避免双重借用 storage）
        let states_snapshot: Vec<AggregateState> = self
            .group_states
            .get(group_key)
            .map(|e| e.states.clone())
            .unwrap_or_default();
        for (spec, state) in self.aggregate_specs.iter().zip(states_snapshot.iter()) {
            if spec.output_column < new_row.len() {
                new_row[spec.output_column] = state.current_value(spec.function);
            }
        }
        // 使用 update_row 替换分组行
        if !self.storage.update_row(row_id, new_row) {
            // 回退：重新插入分组行
            let null_row = vec![Value::Null; self.storage.schema().columns.len()];
            let new_id = self.storage.insert(null_row);
            if let Some(entry) = self.group_states.get_mut(group_key) {
                entry.row_id = new_id;
            }
        }
    }

    /// 更新刷新状态
    pub fn update_refresh_state(&mut self, timestamp: i64) {
        self.refresh_state.update(self.row_count(), timestamp);
    }
}

// =====================================================================
//  刷新结果 — Phase 6.11 + Phase 6.12
// =====================================================================

/// 增量刷新结果 — Phase 6.11 + Phase 6.12
///
/// 描述一次增量刷新操作的统计信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshOutcome {
    /// 追加的行数（INSERT_ONLY + SIMPLE 模式：新插入行数）
    pub rows_appended: usize,
    /// 移除的行数（Phase 6.11 始终为 0；Phase 6.12 SIMPLE 模式为 DELETE 行数）
    pub rows_removed: usize,
    /// 更新的行数（Phase 6.12 SIMPLE 模式：UPDATE 行数；其他模式为 0）
    pub rows_updated: usize,
    /// 刷新模式
    pub mode: RefreshMode,
    /// 刷新后物化视图总行数
    pub total_rows: usize,
}

impl RefreshOutcome {
    /// 创建 INSERT_ONLY 刷新结果
    pub fn insert_only(rows_appended: usize, total_rows: usize) -> Self {
        Self {
            rows_appended,
            rows_removed: 0,
            rows_updated: 0,
            mode: RefreshMode::InsertOnly,
            total_rows,
        }
    }

    /// 创建 SIMPLE 刷新结果 — Phase 6.12
    pub fn simple(
        rows_inserted: usize,
        rows_updated: usize,
        rows_deleted: usize,
        total_rows: usize,
    ) -> Self {
        Self {
            rows_appended: rows_inserted,
            rows_removed: rows_deleted,
            rows_updated,
            mode: RefreshMode::Simple,
            total_rows,
        }
    }

    /// 创建 AGGREGATE 刷新结果 — Phase 6.13
    ///
    /// `rows_inserted` / `rows_deleted` 为 CDC 事件数；
    /// `decrements_failed` 为无法递减的聚合数（MIN/MAX DELETE）。
    pub fn aggregate(
        rows_inserted: usize,
        rows_deleted: usize,
        decrements_failed: usize,
        total_rows: usize,
    ) -> Self {
        Self {
            rows_appended: rows_inserted,
            rows_removed: rows_deleted,
            rows_updated: decrements_failed,
            mode: RefreshMode::Aggregate,
            total_rows,
        }
    }

    /// 创建 GROUP_AGGREGATE 刷新结果 — Phase 6.14
    ///
    /// `rows_inserted` / `rows_deleted` 为 CDC 事件数；
    /// `decrements_failed` 为无法递减的聚合数（MIN/MAX DELETE 或分组不存在）；
    /// `total_rows` 为刷新后的分组总数（即存储表活跃行数）。
    pub fn group_aggregate(
        rows_inserted: usize,
        rows_deleted: usize,
        decrements_failed: usize,
        total_rows: usize,
    ) -> Self {
        Self {
            rows_appended: rows_inserted,
            rows_removed: rows_deleted,
            rows_updated: decrements_failed,
            mode: RefreshMode::GroupAggregate,
            total_rows,
        }
    }
}

// =====================================================================
//  单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Select;
    use szrsql_types::value::ColumnType;

    fn make_test_select() -> Select {
        // 构造一个最小的 SELECT（具体字段不重要，仅用于测试 ViewDefinition）
        Select {
            with: None,
            distinct: false,
            projection: Vec::new(),
            from: Vec::new(),
            where_clause: None,
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            set_op: None,
        }
    }

    // --- ViewDefinition 测试 ---

    #[test]
    fn test_view_definition_new_materialized() {
        let name = TableName::new("mv_test");
        let query = Box::new(make_test_select());
        let view = ViewDefinition::new_materialized(name.clone(), query);
        assert!(view.materialized);
        assert_eq!(view.name, name);
        assert!(view.columns.is_empty());
    }

    #[test]
    fn test_view_definition_new_view() {
        let name = TableName::new("v_test");
        let query = Box::new(make_test_select());
        let view = ViewDefinition::new_view(name.clone(), query);
        assert!(!view.materialized);
        assert_eq!(view.name, name);
    }

    #[test]
    fn test_view_definition_with_columns() {
        let name = TableName::new("mv_test");
        let query = Box::new(make_test_select());
        let view = ViewDefinition::new_materialized(name, query)
            .with_columns(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(view.columns, vec!["a", "b"]);
    }

    #[test]
    fn test_view_definition_clone() {
        let name = TableName::new("mv_test");
        let query = Box::new(make_test_select());
        let view = ViewDefinition::new_materialized(name, query);
        let cloned = view.clone();
        assert_eq!(view, cloned);
    }

    // --- RefreshMode 测试 ---

    #[test]
    fn test_refresh_mode_default() {
        assert_eq!(RefreshMode::default(), RefreshMode::Full);
    }

    #[test]
    fn test_refresh_mode_display() {
        assert_eq!(RefreshMode::Full.to_string(), "FULL");
        assert_eq!(RefreshMode::InsertOnly.to_string(), "INSERT_ONLY");
        assert_eq!(RefreshMode::Simple.to_string(), "SIMPLE");
        assert_eq!(RefreshMode::Aggregate.to_string(), "AGGREGATE");
        assert_eq!(RefreshMode::GroupAggregate.to_string(), "GROUP_AGGREGATE");
    }

    // --- RefreshState 测试 ---

    #[test]
    fn test_refresh_state_new() {
        let state = RefreshState::new();
        assert!(!state.initialized);
        assert_eq!(state.last_row_count, 0);
        assert_eq!(state.last_refresh_timestamp, 0);
        assert_eq!(state.mode, RefreshMode::Full);
    }

    #[test]
    fn test_refresh_state_initialized() {
        let state = RefreshState::initialized(100, 1234567890, RefreshMode::InsertOnly);
        assert!(state.initialized);
        assert_eq!(state.last_row_count, 100);
        assert_eq!(state.last_refresh_timestamp, 1234567890);
        assert_eq!(state.mode, RefreshMode::InsertOnly);
    }

    #[test]
    fn test_refresh_state_update() {
        let mut state = RefreshState::new();
        state.update(50, 1111111);
        assert!(state.initialized);
        assert_eq!(state.last_row_count, 50);
        assert_eq!(state.last_refresh_timestamp, 1111111);
    }

    #[test]
    fn test_refresh_state_default() {
        let state = RefreshState::default();
        assert!(!state.initialized);
    }

    #[test]
    fn test_refresh_state_clone() {
        let state = RefreshState::initialized(100, 1234567890, RefreshMode::Simple);
        let cloned = state.clone();
        assert_eq!(state.initialized, cloned.initialized);
        assert_eq!(state.last_row_count, cloned.last_row_count);
        assert_eq!(state.last_refresh_timestamp, cloned.last_refresh_timestamp);
        assert_eq!(state.mode, cloned.mode);
    }

    // --- CdcEvent 测试 — Phase 6.11 ---

    #[test]
    fn test_cdc_event_insert_construction() {
        let event = CdcEvent::insert("users", vec![Value::Int64(1), Value::Text("Alice".into())]);
        match &event {
            CdcEvent::Insert { source_table, row } => {
                assert_eq!(source_table.name, "users");
                assert_eq!(row.len(), 2);
                assert_eq!(row[0], Value::Int64(1));
            }
            CdcEvent::Update { .. } | CdcEvent::Delete { .. } => {
                panic!("expected Insert, got {event:?}")
            }
        }
    }

    #[test]
    fn test_cdc_event_kind_str() {
        let event = CdcEvent::insert("t", vec![]);
        assert_eq!(event.kind_str(), "INSERT");
    }

    #[test]
    fn test_cdc_event_clone_eq() {
        let event1 = CdcEvent::insert("users", vec![Value::Int64(1)]);
        let event2 = event1.clone();
        assert_eq!(event1, event2);
    }

    // --- CdcFeed 测试 — Phase 6.11 ---

    #[test]
    fn test_cdc_feed_new_is_empty() {
        let feed = CdcFeed::new();
        assert!(feed.is_empty());
        assert_eq!(feed.len(), 0);
    }

    #[test]
    fn test_cdc_feed_push_insert() {
        let mut feed = CdcFeed::new();
        feed.push_insert("users", vec![Value::Int64(1)]);
        assert!(!feed.is_empty());
        assert_eq!(feed.len(), 1);
    }

    #[test]
    fn test_cdc_feed_push_event() {
        let mut feed = CdcFeed::new();
        let event = CdcEvent::insert("users", vec![Value::Int64(1)]);
        feed.push(event);
        assert_eq!(feed.len(), 1);
    }

    #[test]
    fn test_cdc_feed_push_inserts_batch() {
        let mut feed = CdcFeed::new();
        let rows = vec![
            vec![Value::Int64(1)],
            vec![Value::Int64(2)],
            vec![Value::Int64(3)],
        ];
        feed.push_inserts("users", rows);
        assert_eq!(feed.len(), 3);
    }

    #[test]
    fn test_cdc_feed_drain() {
        let mut feed = CdcFeed::new();
        feed.push_insert("users", vec![Value::Int64(1)]);
        feed.push_insert("users", vec![Value::Int64(2)]);
        let events = feed.drain();
        assert_eq!(events.len(), 2);
        assert!(feed.is_empty());
    }

    #[test]
    fn test_cdc_feed_drain_empty() {
        let mut feed = CdcFeed::new();
        let events = feed.drain();
        assert!(events.is_empty());
    }

    #[test]
    fn test_cdc_feed_peek() {
        let mut feed = CdcFeed::new();
        feed.push_insert("users", vec![Value::Int64(42)]);
        let peeked = feed.peek();
        assert_eq!(peeked.len(), 1);
        // peek 不消费
        assert_eq!(feed.len(), 1);
    }

    #[test]
    fn test_cdc_feed_default_is_empty() {
        let feed = CdcFeed::default();
        assert!(feed.is_empty());
    }

    // --- MaterializedViewStore 测试 — Phase 6.11 ---

    fn make_test_store() -> MaterializedViewStore {
        MaterializedViewStore::new(
            "mv_test",
            vec![("id", szrsql_types::value::ColumnType::Int64)],
        )
    }

    #[test]
    fn test_mv_store_new_empty() {
        let store = make_test_store();
        assert_eq!(store.row_count(), 0);
        assert!(!store.refresh_state.initialized);
    }

    #[test]
    fn test_mv_store_append_row() {
        let mut store = make_test_store();
        store.append_row(vec![Value::Int64(1)]);
        assert_eq!(store.row_count(), 1);
        store.append_row(vec![Value::Int64(2)]);
        assert_eq!(store.row_count(), 2);
    }

    #[test]
    fn test_mv_store_append_rows_batch() {
        let mut store = make_test_store();
        let rows = vec![
            vec![Value::Int64(1)],
            vec![Value::Int64(2)],
            vec![Value::Int64(3)],
        ];
        store.append_rows(rows);
        assert_eq!(store.row_count(), 3);
    }

    #[test]
    fn test_mv_store_rows_access() {
        let mut store = make_test_store();
        store.append_row(vec![Value::Int64(10)]);
        store.append_row(vec![Value::Int64(20)]);
        let rows = store.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], Value::Int64(10));
        assert_eq!(rows[1][0], Value::Int64(20));
    }

    #[test]
    fn test_mv_store_high_water_mark_default_zero() {
        let store = make_test_store();
        let hwm = store.high_water_mark(&TableName::new("users"));
        assert_eq!(hwm, 0);
    }

    #[test]
    fn test_mv_store_set_high_water_mark() {
        let mut store = make_test_store();
        let table = TableName::new("users");
        store.set_high_water_mark(&table, 100);
        assert_eq!(store.high_water_mark(&table), 100);
    }

    #[test]
    fn test_mv_store_high_water_mark_case_insensitive() {
        let mut store = make_test_store();
        store.set_high_water_mark(&TableName::new("Users"), 50);
        // 小写化存储，所以 "users" 也能取到
        assert_eq!(store.high_water_mark(&TableName::new("users")), 50);
        assert_eq!(store.high_water_mark(&TableName::new("USERS")), 50);
    }

    #[test]
    fn test_mv_store_clear() {
        let mut store = make_test_store();
        store.append_row(vec![Value::Int64(1)]);
        store.append_row(vec![Value::Int64(2)]);
        store.set_high_water_mark(&TableName::new("users"), 50);
        store.clear();
        assert_eq!(store.row_count(), 0);
        assert_eq!(store.high_water_mark(&TableName::new("users")), 0);
        assert!(!store.refresh_state.initialized);
    }

    #[test]
    fn test_mv_store_update_refresh_state() {
        let mut store = make_test_store();
        store.append_row(vec![Value::Int64(1)]);
        store.update_refresh_state(1234567890);
        assert!(store.refresh_state.initialized);
        assert_eq!(store.refresh_state.last_row_count, 1);
        assert_eq!(store.refresh_state.last_refresh_timestamp, 1234567890);
    }

    // --- RefreshOutcome 测试 — Phase 6.11 + Phase 6.12 ---

    #[test]
    fn test_refresh_outcome_insert_only() {
        let outcome = RefreshOutcome::insert_only(100, 150);
        assert_eq!(outcome.rows_appended, 100);
        assert_eq!(outcome.rows_removed, 0);
        assert_eq!(outcome.rows_updated, 0);
        assert_eq!(outcome.mode, RefreshMode::InsertOnly);
        assert_eq!(outcome.total_rows, 150);
    }

    #[test]
    fn test_refresh_outcome_clone_eq() {
        let outcome = RefreshOutcome::insert_only(10, 20);
        let cloned = outcome.clone();
        assert_eq!(outcome, cloned);
    }

    // --- Phase 6.12: CdcEvent Update/Delete 测试 ---

    #[test]
    fn test_cdc_event_update_construction() {
        let event = CdcEvent::update(
            "users",
            vec![Value::Int64(1)],
            vec![Value::Int64(1), Value::Text("Alice".into())],
        );
        match event {
            CdcEvent::Update {
                source_table,
                pk,
                row,
            } => {
                assert_eq!(source_table.name, "users");
                assert_eq!(pk, vec![Value::Int64(1)]);
                assert_eq!(row.len(), 2);
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn test_cdc_event_delete_construction() {
        let event = CdcEvent::delete("users", vec![Value::Int64(1)]);
        match event {
            CdcEvent::Delete {
                source_table,
                pk,
                row: None,
            } => {
                assert_eq!(source_table.name, "users");
                assert_eq!(pk, vec![Value::Int64(1)]);
            }
            other => panic!("expected Delete with row=None, got {other:?}"),
        }
    }

    #[test]
    fn test_cdc_event_kind_str_all_variants() {
        assert_eq!(CdcEvent::insert("t", vec![]).kind_str(), "INSERT");
        assert_eq!(CdcEvent::update("t", vec![], vec![]).kind_str(), "UPDATE");
        assert_eq!(CdcEvent::delete("t", vec![]).kind_str(), "DELETE");
    }

    #[test]
    fn test_cdc_event_source_table_accessor() {
        let ins = CdcEvent::insert("users", vec![]);
        let upd = CdcEvent::update("orders", vec![], vec![]);
        let del = CdcEvent::delete("products", vec![]);
        assert_eq!(ins.source_table().name, "users");
        assert_eq!(upd.source_table().name, "orders");
        assert_eq!(del.source_table().name, "products");
    }

    // --- Phase 6.12: CdcFeed push_update/push_delete 测试 ---

    #[test]
    fn test_cdc_feed_push_update() {
        let mut feed = CdcFeed::new();
        feed.push_update(
            "users",
            vec![Value::Int64(1)],
            vec![Value::Int64(1), Value::Text("Bob".into())],
        );
        assert_eq!(feed.len(), 1);
        let events = feed.drain();
        assert!(matches!(events[0], CdcEvent::Update { .. }));
    }

    #[test]
    fn test_cdc_feed_push_delete() {
        let mut feed = CdcFeed::new();
        feed.push_delete("users", vec![Value::Int64(1)]);
        assert_eq!(feed.len(), 1);
        let events = feed.drain();
        assert!(matches!(events[0], CdcEvent::Delete { .. }));
    }

    #[test]
    fn test_cdc_feed_mixed_events() {
        let mut feed = CdcFeed::new();
        feed.push_insert("t", vec![Value::Int64(1)]);
        feed.push_update("t", vec![Value::Int64(1)], vec![Value::Int64(1)]);
        feed.push_delete("t", vec![Value::Int64(1)]);
        assert_eq!(feed.len(), 3);
        let events = feed.drain();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], CdcEvent::Insert { .. }));
        assert!(matches!(events[1], CdcEvent::Update { .. }));
        assert!(matches!(events[2], CdcEvent::Delete { .. }));
    }

    // --- Phase 6.12: MaterializedViewStore 主键索引测试 ---

    #[test]
    fn test_mv_store_new_with_pk() {
        let store = MaterializedViewStore::new_with_pk(
            "mv",
            vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
            vec![0],
        );
        assert!(store.has_primary_key());
        assert_eq!(store.pk_indices(), &[0]);
    }

    #[test]
    fn test_mv_store_append_row_updates_pk_index() {
        let mut store =
            MaterializedViewStore::new_with_pk("mv", vec![("id", ColumnType::Int64)], vec![0]);
        store.append_row(vec![Value::Int64(1)]);
        store.append_row(vec![Value::Int64(2)]);
        assert_eq!(store.row_count(), 2);
    }

    #[test]
    fn test_mv_store_upsert_insert_new() {
        let mut store = MaterializedViewStore::new_with_pk(
            "mv",
            vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
            vec![0],
        );
        let (was_insert, was_update) =
            store.upsert_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
        assert!(was_insert);
        assert!(!was_update);
        assert_eq!(store.active_row_count(), 1);
    }

    #[test]
    fn test_mv_store_upsert_update_existing() {
        let mut store = MaterializedViewStore::new_with_pk(
            "mv",
            vec![("id", ColumnType::Int64), ("name", ColumnType::Text)],
            vec![0],
        );
        store.append_row(vec![Value::Int64(1), Value::Text("Alice".into())]);
        let (was_insert, was_update) =
            store.upsert_row(vec![Value::Int64(1), Value::Text("Bob".into())]);
        assert!(!was_insert);
        assert!(was_update);
        assert_eq!(store.active_row_count(), 1);
        // 验证行已更新
        let rows = store.rows();
        assert_eq!(rows[0][1], Value::Text("Bob".into()));
    }

    #[test]
    fn test_mv_store_delete_by_pk_existing() {
        let mut store =
            MaterializedViewStore::new_with_pk("mv", vec![("id", ColumnType::Int64)], vec![0]);
        store.append_row(vec![Value::Int64(1)]);
        store.append_row(vec![Value::Int64(2)]);
        assert!(store.delete_by_pk(&[Value::Int64(1)]));
        assert_eq!(store.active_row_count(), 1);
    }

    #[test]
    fn test_mv_store_delete_by_pk_nonexistent() {
        let mut store =
            MaterializedViewStore::new_with_pk("mv", vec![("id", ColumnType::Int64)], vec![0]);
        store.append_row(vec![Value::Int64(1)]);
        assert!(!store.delete_by_pk(&[Value::Int64(99)]));
        assert_eq!(store.active_row_count(), 1);
    }

    #[test]
    fn test_mv_store_delete_by_pk_no_pk_index() {
        let mut store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
        store.append_row(vec![Value::Int64(1)]);
        // 无主键索引，delete_by_pk 应返回 false
        assert!(!store.delete_by_pk(&[Value::Int64(1)]));
    }

    #[test]
    fn test_mv_store_upsert_without_pk_degrades_to_append() {
        let mut store = MaterializedViewStore::new("mv", vec![("id", ColumnType::Int64)]);
        let (was_insert, _) = store.upsert_row(vec![Value::Int64(1)]);
        assert!(was_insert);
        assert_eq!(store.row_count(), 1);
    }

    #[test]
    fn test_mv_store_active_row_count_excludes_tombstone() {
        let mut store =
            MaterializedViewStore::new_with_pk("mv", vec![("id", ColumnType::Int64)], vec![0]);
        store.append_row(vec![Value::Int64(1)]);
        store.append_row(vec![Value::Int64(2)]);
        store.append_row(vec![Value::Int64(3)]);
        assert!(store.delete_by_pk(&[Value::Int64(2)]));
        // row_count 含 tombstone，active_row_count 排除
        assert_eq!(store.row_count(), 3);
        assert_eq!(store.active_row_count(), 2);
    }

    #[test]
    fn test_mv_store_clear_resets_pk_index() {
        let mut store =
            MaterializedViewStore::new_with_pk("mv", vec![("id", ColumnType::Int64)], vec![0]);
        store.append_row(vec![Value::Int64(1)]);
        store.clear();
        assert_eq!(store.row_count(), 0);
        assert!(store.has_primary_key()); // pk_indices 保留
                                          // 清空后可重新追加
        store.append_row(vec![Value::Int64(2)]);
        assert_eq!(store.row_count(), 1);
    }

    #[test]
    fn test_mv_store_composite_pk() {
        // 复合主键：(tenant_id, user_id)
        let mut store = MaterializedViewStore::new_with_pk(
            "mv",
            vec![
                ("tenant_id", ColumnType::Int64),
                ("user_id", ColumnType::Int64),
                ("name", ColumnType::Text),
            ],
            vec![0, 1],
        );
        store.append_row(vec![
            Value::Int64(1),
            Value::Int64(100),
            Value::Text("Alice".into()),
        ]);
        store.append_row(vec![
            Value::Int64(1),
            Value::Int64(101),
            Value::Text("Bob".into()),
        ]);
        assert_eq!(store.active_row_count(), 2);
        // 按复合主键删除
        assert!(store.delete_by_pk(&[Value::Int64(1), Value::Int64(100)]));
        assert_eq!(store.active_row_count(), 1);
        // UPSERT 复合主键
        let (was_insert, _) = store.upsert_row(vec![
            Value::Int64(1),
            Value::Int64(101),
            Value::Text("Charlie".into()),
        ]);
        assert!(!was_insert); // 已存在，应更新
        assert_eq!(store.active_row_count(), 1);
    }

    // --- Phase 6.12: RefreshOutcome SIMPLE 测试 ---

    #[test]
    fn test_refresh_outcome_simple() {
        let outcome = RefreshOutcome::simple(100, 50, 30, 120);
        assert_eq!(outcome.rows_appended, 100);
        assert_eq!(outcome.rows_updated, 50);
        assert_eq!(outcome.rows_removed, 30);
        assert_eq!(outcome.mode, RefreshMode::Simple);
        assert_eq!(outcome.total_rows, 120);
    }

    #[test]
    fn test_refresh_outcome_simple_clone_eq() {
        let outcome = RefreshOutcome::simple(10, 5, 3, 12);
        let cloned = outcome.clone();
        assert_eq!(outcome, cloned);
    }

    #[test]
    fn test_refresh_outcome_simple_zero_ops() {
        let outcome = RefreshOutcome::simple(0, 0, 0, 100);
        assert_eq!(outcome.rows_appended, 0);
        assert_eq!(outcome.rows_updated, 0);
        assert_eq!(outcome.rows_removed, 0);
        assert_eq!(outcome.total_rows, 100);
    }
}
