//! SzRSQL CDC Schema 变更追踪 — 对应 `SzRSQL实施进度.md` Phase 2.5.10。
//!
//! 当表结构变更（CREATE TABLE / ALTER TABLE ADD COLUMN / DROP TABLE）时，
//! CDC 事件需要携带正确的 schema 版本，以便消费者正确解码行数据。
//!
//! # 核心概念
//!
//! - **`DataType`**：列数据类型枚举（Int32 / Int64 / Text / Blob / Real / Bool / Date / Timestamp / Json / Uuid）
//! - **`ColumnDef`**：列定义（name + data_type + nullable）
//! - **`TableSchema`**：表 schema（table_id + table_name + columns + version）
//! - **`SchemaRegistry`**：schema 注册表，管理所有表的 schema 和版本号
//! - **`SchemaChangeType`**：schema 变更类型（CreateTable / AlterTableAddColumn / AlterTableDropColumn / DropTable）
//! - **`SchemaChangeEvent`**：schema 变更事件（DDL 事件，独立于 DML 的 ChangeEvent）
//! - **`SchemaChangeObserver`**：schema 变更观察者 trait
//!
//! # 设计要点
//!
//! 1. **版本号单调递增**：
//!    - 每次 CREATE TABLE / ALTER TABLE / DROP TABLE 都会增加全局版本计数器
//!    - 表的 schema 版本 = 创建时的全局版本；ALTER 后更新为新的全局版本
//!    - 消费者通过比较 schema_version 判断是否需要重新解码
//!
//! 2. **DDL 事件 vs DML 事件**：
//!    - DDL 事件（CreateTable/AlterTable/DropTable）使用独立的 `SchemaChangeEvent`
//!    - DML 事件（Insert/Update/Delete）使用 `ChangeEvent`，携带 `schema_version: Option<u64>`
//!    - 两类事件通过不同的 observer trait 分发（`SchemaChangeObserver` vs `CdcObserver`）
//!
//! 3. **线程安全**：
//!    - `SchemaRegistry` 内部用 `RwLock<HashMap<u32, TableSchema>>` 支持并发读、互斥写
//!    - 版本计数器用 `AtomicU64` 无锁递增
//!    - 所有 API 都是 `&self`（内部可变性），支持多线程共享
//!
//! 4. **不变量**：
//!    - `version >= 1`（创建时为 1，每次 ALTER 递增）
//!    - `columns` 不能为空（至少 1 列）
//!    - `column.name` 在同一 table 内唯一
//!    - DROP TABLE 后 `get_schema` 返回 None，但历史版本号不再分配给新表

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

// =====================================================================
// DataType — 列数据类型
// =====================================================================

/// 列数据类型 — 支持的 SQL 数据类型
///
/// **设计**：覆盖常见 SQL 类型，便于消费者按类型解码行数据
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataType {
    /// 32 位有符号整数
    Int32,
    /// 64 位有符号整数
    Int64,
    /// 变长文本（UTF-8）
    Text,
    /// 二进制大对象
    Blob,
    /// 64 位浮点数
    Real,
    /// 布尔值
    Bool,
    /// 日期（自 Unix 纪元的天数）
    Date,
    /// 时间戳（自 Unix 纪元的毫秒数）
    Timestamp,
    /// JSON 文档
    Json,
    /// UUID
    Uuid,
}

impl DataType {
    /// 转为字符串（用于日志和显示）
    pub fn as_str(self) -> &'static str {
        match self {
            DataType::Int32 => "int32",
            DataType::Int64 => "int64",
            DataType::Text => "text",
            DataType::Blob => "blob",
            DataType::Real => "real",
            DataType::Bool => "bool",
            DataType::Date => "date",
            DataType::Timestamp => "timestamp",
            DataType::Json => "json",
            DataType::Uuid => "uuid",
        }
    }
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =====================================================================
// ColumnDef — 列定义
// =====================================================================

/// 列定义 — 描述一列的名称、类型和可空性
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ColumnDef {
    /// 列名（在表内唯一）
    pub name: String,
    /// 列数据类型
    pub data_type: DataType,
    /// 是否允许 NULL
    pub nullable: bool,
}

impl ColumnDef {
    /// 创建新的列定义
    pub fn new(name: impl Into<String>, data_type: DataType, nullable: bool) -> Self {
        Self {
            name: name.into(),
            data_type,
            nullable,
        }
    }

    /// 创建非空列（nullable = false）
    pub fn not_null(name: impl Into<String>, data_type: DataType) -> Self {
        Self::new(name, data_type, false)
    }

    /// 创建可空列（nullable = true）
    pub fn nullable(name: impl Into<String>, data_type: DataType) -> Self {
        Self::new(name, data_type, true)
    }
}

// =====================================================================
// TableSchema — 表 schema
// =====================================================================

/// 表 schema — 描述一张表的结构和版本
///
/// **字段**：
/// - `table_id`：表 ID（与 WalRecord.page_id 对应）
/// - `table_name`：表名（人类可读）
/// - `columns`：列定义列表（顺序敏感，与行数据的列顺序一致）
/// - `version`：schema 版本（从 1 开始，每次 ALTER 递增）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TableSchema {
    /// 表 ID
    pub table_id: u32,
    /// 表名
    pub table_name: String,
    /// 列定义列表
    pub columns: Vec<ColumnDef>,
    /// schema 版本（>= 1）
    pub version: u64,
}

impl TableSchema {
    /// 获取列数
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// 按名称查找列定义
    pub fn find_column(&self, name: &str) -> Option<&ColumnDef> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// 获取列名列表
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.name.as_str()).collect()
    }
}

// =====================================================================
// SchemaError — schema 错误
// =====================================================================

/// schema 操作错误
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    /// 表已存在
    #[error("table already exists: table_id={table_id} name={table_name}")]
    TableAlreadyExists { table_id: u32, table_name: String },

    /// 表不存在
    #[error("table not found: table_id={0}")]
    TableNotFound(u32),

    /// 列已存在
    #[error("column already exists: table_id={table_id} column={column_name}")]
    ColumnAlreadyExists { table_id: u32, column_name: String },

    /// 列不存在
    #[error("column not found: table_id={table_id} column={column_name}")]
    ColumnNotFound { table_id: u32, column_name: String },

    /// 列定义为空（至少需要 1 列）
    #[error("columns must not be empty")]
    EmptyColumns,

    /// 列名重复
    #[error("duplicate column name: {0}")]
    DuplicateColumnName(String),
}

// =====================================================================
// SchemaChangeType — schema 变更类型
// =====================================================================

/// schema 变更类型 — DDL 操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaChangeType {
    /// 创建表
    CreateTable,
    /// 添加列
    AlterTableAddColumn,
    /// 删除列
    AlterTableDropColumn,
    /// 删除表
    DropTable,
}

impl SchemaChangeType {
    /// 转为字符串
    pub fn as_str(self) -> &'static str {
        match self {
            SchemaChangeType::CreateTable => "create_table",
            SchemaChangeType::AlterTableAddColumn => "alter_table_add_column",
            SchemaChangeType::AlterTableDropColumn => "alter_table_drop_column",
            SchemaChangeType::DropTable => "drop_table",
        }
    }
}

// =====================================================================
// SchemaChangeEvent — schema 变更事件
// =====================================================================

/// schema 变更事件 — DDL 操作产生的事件
///
/// **设计**：
/// - 独立于 `ChangeEvent`（DML 事件），因为 schema 变更携带的信息不同
/// - `old_schema`：变更前的 schema（CreateTable 时为 None）
/// - `new_schema`：变更后的 schema（DropTable 时为 None）
/// - `changed_column`：变更涉及的列（AddColumn/DropColumn 时为 Some，CreateTable/DropTable 时为 None）
///
/// **字段**：
/// - `tx_id`：所属事务 ID
/// - `lsn`：WAL 日志序列号
/// - `change_type`：变更类型
/// - `table_id`：目标表 ID
/// - `old_schema`：变更前的 schema（CreateTable 时为 None）
/// - `new_schema`：变更后的 schema（DropTable 时为 None）
/// - `changed_column`：变更涉及的列名（AddColumn/DropColumn 时为 Some）
/// - `schema_version`：变更后的 schema 版本（全局递增）
/// - `timestamp`：事件生成时间戳
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SchemaChangeEvent {
    /// 所属事务 ID
    pub tx_id: u32,
    /// WAL 日志序列号
    pub lsn: u64,
    /// 变更类型
    pub change_type: SchemaChangeType,
    /// 目标表 ID
    pub table_id: u32,
    /// 变更前的 schema（CreateTable 时为 None）
    pub old_schema: Option<TableSchema>,
    /// 变更后的 schema（DropTable 时为 None）
    pub new_schema: Option<TableSchema>,
    /// 变更涉及的列名（AddColumn/DropColumn 时为 Some）
    pub changed_column: Option<String>,
    /// 变更后的 schema 版本（全局递增）
    pub schema_version: u64,
    /// 事件生成时间戳
    pub timestamp: u64,
}

// =====================================================================
// SchemaChangeObserver — schema 变更观察者 trait
// =====================================================================

/// schema 变更观察者 trait — 接收 SchemaChangeEvent
///
/// **与 CdcObserver 的区别**：
/// - `CdcObserver` 接收 DML 事件（Insert/Update/Delete/Commit/Abort）
/// - `SchemaChangeObserver` 接收 DDL 事件（CreateTable/AlterTable/DropTable）
///
/// **线程安全**：实现者必须是 `Send + Sync`
pub trait SchemaChangeObserver: Send + Sync {
    /// 接收一个 SchemaChangeEvent
    fn on_schema_change(&self, event: SchemaChangeEvent);
}

// =====================================================================
// SchemaChangeObserverManager — schema 变更观察者管理器
// =====================================================================

/// schema 变更观察者管理器 — 管理多个 SchemaChangeObserver
///
/// **设计**（与 CdcObserverManager 同风格）：
/// 1. 多观察者：支持注册多个 observer
/// 2. 线程安全：`RwLock<Vec<Arc<dyn SchemaChangeObserver>>>`
/// 3. 同步触发：notify 同步调用所有 observer
/// 4. panic 隔离：catch_unwind 捕获单个 observer panic
pub struct SchemaChangeObserverManager {
    observers: RwLock<Vec<Arc<dyn SchemaChangeObserver>>>,
    total_dispatched: AtomicU64,
}

impl Default for SchemaChangeObserverManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaChangeObserverManager {
    /// 创建空的观察者管理器
    pub fn new() -> Self {
        Self {
            observers: RwLock::new(Vec::new()),
            total_dispatched: AtomicU64::new(0),
        }
    }

    /// 注册观察者（返回 true 表示注册成功，false 表示已注册相同指针的 observer）
    pub fn register(&self, observer: Arc<dyn SchemaChangeObserver>) -> bool {
        let mut observers = self.observers.write().unwrap();
        let target_addr = Arc::as_ptr(&observer) as *const () as usize;
        if observers
            .iter()
            .any(|o| Arc::as_ptr(o) as *const () as usize == target_addr)
        {
            return false;
        }
        observers.push(observer);
        true
    }

    /// 注销观察者（返回 true 表示注销成功）
    pub fn unregister<O: SchemaChangeObserver + 'static>(&self, observer: &Arc<O>) -> bool {
        let mut observers = self.observers.write().unwrap();
        let target_addr = Arc::as_ptr(observer) as *const () as usize;
        let original_len = observers.len();
        observers.retain(|o| Arc::as_ptr(o) as *const () as usize != target_addr);
        observers.len() < original_len
    }

    /// 通知所有观察者：分发一个 SchemaChangeEvent
    pub fn notify(&self, event: SchemaChangeEvent) {
        let observers = self.observers.read().unwrap();
        let count = observers.len();
        for observer in observers.iter() {
            let event_clone = event.clone();
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                observer.on_schema_change(event_clone);
            }));
        }
        self.total_dispatched
            .fetch_add(count as u64, Ordering::SeqCst);
    }

    /// 获取已注册的观察者数量
    pub fn observer_count(&self) -> usize {
        self.observers.read().unwrap().len()
    }

    /// 获取已分发的总次数
    pub fn total_dispatched(&self) -> u64 {
        self.total_dispatched.load(Ordering::SeqCst)
    }
}

// =====================================================================
// SchemaRegistry — schema 注册表
// =====================================================================

/// schema 注册表 — 管理所有表的 schema 和版本号
///
/// **设计**：
/// 1. **内部可变性**：所有 API 都是 `&self`，支持多线程共享
/// 2. **并发读、互斥写**：`RwLock<HashMap<u32, TableSchema>>`
/// 3. **全局版本计数器**：`AtomicU64`，每次 DDL 操作递增
/// 4. **版本号语义**：
///    - 全局计数器从 0 开始，每次 DDL 操作先递增再使用
///    - CreateTable：version = ++global_counter（首次为 1）
///    - AlterTable：version = ++global_counter
///    - DropTable：version = ++global_counter（标记删除）
///
/// **线程安全**：`Send + Sync`，可直接用 `Arc<SchemaRegistry>` 共享
pub struct SchemaRegistry {
    /// 表 schema 映射（table_id -> TableSchema）
    schemas: RwLock<HashMap<u32, TableSchema>>,
    /// 全局版本计数器（每次 DDL 操作递增）
    global_version: AtomicU64,
    /// 已删除表的版本历史（table_id -> 最后一个 version，便于审计）
    dropped_versions: RwLock<HashMap<u32, u64>>,
}

impl Default for SchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaRegistry {
    /// 创建空的 schema 注册表
    pub fn new() -> Self {
        Self {
            schemas: RwLock::new(HashMap::new()),
            global_version: AtomicU64::new(0),
            dropped_versions: RwLock::new(HashMap::new()),
        }
    }

    /// 获取当前全局版本号（已分配的最大版本号）
    pub fn current_global_version(&self) -> u64 {
        self.global_version.load(Ordering::SeqCst)
    }

    /// 创建表 — 分配新版本号并注册 schema
    ///
    /// **错误**：
    /// - `TableAlreadyExists`：table_id 已存在
    /// - `EmptyColumns`：columns 为空
    /// - `DuplicateColumnName`：列名重复
    pub fn create_table(
        &self,
        table_id: u32,
        table_name: impl Into<String>,
        columns: Vec<ColumnDef>,
    ) -> Result<TableSchema, SchemaError> {
        if columns.is_empty() {
            return Err(SchemaError::EmptyColumns);
        }
        // 检查列名唯一性
        let mut seen = std::collections::HashSet::new();
        for col in &columns {
            if !seen.insert(col.name.clone()) {
                return Err(SchemaError::DuplicateColumnName(col.name.clone()));
            }
        }

        let table_name = table_name.into();
        let mut schemas = self.schemas.write().unwrap();
        if schemas.contains_key(&table_id) {
            return Err(SchemaError::TableAlreadyExists {
                table_id,
                table_name,
            });
        }

        let version = self.global_version.fetch_add(1, Ordering::SeqCst) + 1;
        let schema = TableSchema {
            table_id,
            table_name,
            columns,
            version,
        };
        schemas.insert(table_id, schema.clone());
        Ok(schema)
    }

    /// 添加列 — 递增版本号并更新 schema
    ///
    /// **错误**：
    /// - `TableNotFound`：table_id 不存在
    /// - `ColumnAlreadyExists`：列名已存在
    pub fn alter_table_add_column(
        &self,
        table_id: u32,
        column: ColumnDef,
    ) -> Result<TableSchema, SchemaError> {
        let mut schemas = self.schemas.write().unwrap();
        let schema = schemas
            .get_mut(&table_id)
            .ok_or(SchemaError::TableNotFound(table_id))?;

        if schema.columns.iter().any(|c| c.name == column.name) {
            return Err(SchemaError::ColumnAlreadyExists {
                table_id,
                column_name: column.name,
            });
        }

        let new_version = self.global_version.fetch_add(1, Ordering::SeqCst) + 1;
        schema.columns.push(column);
        schema.version = new_version;
        Ok(schema.clone())
    }

    /// 删除列 — 递增版本号并更新 schema
    ///
    /// **错误**：
    /// - `TableNotFound`：table_id 不存在
    /// - `ColumnNotFound`：列名不存在
    pub fn alter_table_drop_column(
        &self,
        table_id: u32,
        column_name: &str,
    ) -> Result<TableSchema, SchemaError> {
        let mut schemas = self.schemas.write().unwrap();
        let schema = schemas
            .get_mut(&table_id)
            .ok_or(SchemaError::TableNotFound(table_id))?;

        // 先查找列索引，避免在 retain 修改后再判断错误
        let original_len = schema.columns.len();
        let col_idx = schema.columns.iter().position(|c| c.name == column_name);
        let col_idx = match col_idx {
            Some(idx) => idx,
            None => {
                return Err(SchemaError::ColumnNotFound {
                    table_id,
                    column_name: column_name.to_string(),
                });
            }
        };
        // 不允许删除最后一列（至少保留 1 列）
        // 在修改前判断，保证错误时原始 schema 不变
        if original_len == 1 {
            return Err(SchemaError::EmptyColumns);
        }

        schema.columns.remove(col_idx);
        let new_version = self.global_version.fetch_add(1, Ordering::SeqCst) + 1;
        schema.version = new_version;
        Ok(schema.clone())
    }

    /// 删除表 — 递增版本号并移除 schema
    ///
    /// **错误**：
    /// - `TableNotFound`：table_id 不存在
    pub fn drop_table(&self, table_id: u32) -> Result<TableSchema, SchemaError> {
        let mut schemas = self.schemas.write().unwrap();
        let schema = schemas
            .remove(&table_id)
            .ok_or(SchemaError::TableNotFound(table_id))?;

        let new_version = self.global_version.fetch_add(1, Ordering::SeqCst) + 1;
        // 记录已删除表的最后版本（便于审计）
        self.dropped_versions
            .write()
            .unwrap()
            .insert(table_id, new_version);
        // 返回的 schema.version 是删除前的版本，但 new_version 是本次操作的版本
        // 为了语义清晰，返回的 schema 携带新版本
        let mut dropped_schema = schema;
        dropped_schema.version = new_version;
        Ok(dropped_schema)
    }

    /// 获取表的 schema（如果存在）
    pub fn get_schema(&self, table_id: u32) -> Option<TableSchema> {
        self.schemas.read().unwrap().get(&table_id).cloned()
    }

    /// 获取表的当前 schema 版本（如果表存在）
    pub fn get_version(&self, table_id: u32) -> Option<u64> {
        self.schemas
            .read()
            .unwrap()
            .get(&table_id)
            .map(|s| s.version)
    }

    /// 获取已注册的表数量
    pub fn table_count(&self) -> usize {
        self.schemas.read().unwrap().len()
    }

    /// 获取已删除表的最后版本号（审计用）
    pub fn get_dropped_version(&self, table_id: u32) -> Option<u64> {
        self.dropped_versions
            .read()
            .unwrap()
            .get(&table_id)
            .copied()
    }

    /// 判断表是否存在
    pub fn contains_table(&self, table_id: u32) -> bool {
        self.schemas.read().unwrap().contains_key(&table_id)
    }

    /// 获取所有表的 schema 快照
    pub fn list_tables(&self) -> Vec<TableSchema> {
        self.schemas.read().unwrap().values().cloned().collect()
    }
}

// =====================================================================
// SchemaAwareCdcEngine — 感知 schema 的 CDC 引擎
// =====================================================================

/// 感知 schema 的 CDC 引擎 — 在 CdcEngine 基础上增加 schema 追踪
///
/// **设计**：
/// 1. 持有 `Arc<SchemaRegistry>` 和 `Arc<SchemaChangeObserverManager>`
/// 2. DDL 操作（create_table / alter_table / drop_table）：
///    - 更新 SchemaRegistry
///    - 构造 SchemaChangeEvent 并分发给 SchemaChangeObserver
///    - 返回新的 schema 版本
/// 3. DML 操作（insert / update / delete）：
///    - 从 SchemaRegistry 查询当前 schema 版本
///    - 构造带 schema_version 的 ChangeEvent
///    - 通过 CdcObserverManager 分发
///
/// **使用示例**：
/// ```ignore
/// use szrsql_cdc::schema::SchemaAwareCdcEngine;
/// use szrsql_cdc::{CdcObserverManager, CdcObserver, ChangeEvent};
/// use std::sync::Arc;
///
/// let cdc_mgr = Arc::new(CdcObserverManager::new());
/// let schema_mgr = Arc::new(SchemaChangeObserverManager::new());
/// let engine = SchemaAwareCdcEngine::new(cdc_mgr.clone(), schema_mgr.clone());
///
/// // CREATE TABLE → SchemaChangeEvent 分发
/// engine.create_table(1, "users", vec![ColumnDef::not_null("id", DataType::Int64)]).unwrap();
/// // INSERT → ChangeEvent 携带 schema_version
/// engine.insert(1, 100, 1, vec![1], 12345).unwrap();
/// ```
pub struct SchemaAwareCdcEngine {
    /// schema 注册表
    registry: Arc<SchemaRegistry>,
    /// schema 变更观察者管理器
    schema_observer_manager: Arc<SchemaChangeObserverManager>,
    /// DML 观察者管理器（复用 CdcObserverManager）
    dml_observer_manager: Arc<crate::CdcObserverManager>,
    /// 时间戳注入函数
    timestamp_fn: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl SchemaAwareCdcEngine {
    /// 创建感知 schema 的 CDC 引擎，使用 SystemTime 作为时间戳源
    pub fn new(
        dml_observer_manager: Arc<crate::CdcObserverManager>,
        schema_observer_manager: Arc<SchemaChangeObserverManager>,
    ) -> Self {
        Self::with_registry(
            Arc::new(SchemaRegistry::new()),
            dml_observer_manager,
            schema_observer_manager,
        )
    }

    /// 创建感知 schema 的 CDC 引擎，注入已有的 SchemaRegistry
    pub fn with_registry(
        registry: Arc<SchemaRegistry>,
        dml_observer_manager: Arc<crate::CdcObserverManager>,
        schema_observer_manager: Arc<SchemaChangeObserverManager>,
    ) -> Self {
        Self {
            registry,
            schema_observer_manager,
            dml_observer_manager,
            timestamp_fn: Box::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0)
            }),
        }
    }

    /// 创建感知 schema 的 CDC 引擎，注入自定义时间戳函数
    pub fn with_timestamp_fn(
        registry: Arc<SchemaRegistry>,
        dml_observer_manager: Arc<crate::CdcObserverManager>,
        schema_observer_manager: Arc<SchemaChangeObserverManager>,
        timestamp_fn: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            registry,
            schema_observer_manager,
            dml_observer_manager,
            timestamp_fn,
        }
    }

    /// 获取 schema 注册表的引用
    pub fn registry(&self) -> &Arc<SchemaRegistry> {
        &self.registry
    }

    // -----------------------------------------------------------------
    // DDL 操作 — 产生 SchemaChangeEvent
    // -----------------------------------------------------------------

    /// CREATE TABLE — 注册 schema 并分发 SchemaChangeEvent
    ///
    /// **返回**：新创建的 TableSchema（含版本号）
    pub fn create_table(
        &self,
        tx_id: u32,
        lsn: u64,
        table_id: u32,
        table_name: impl Into<String>,
        columns: Vec<ColumnDef>,
    ) -> Result<TableSchema, SchemaError> {
        let timestamp = (self.timestamp_fn)();
        let new_schema = self.registry.create_table(table_id, table_name, columns)?;

        let event = SchemaChangeEvent {
            tx_id,
            lsn,
            change_type: SchemaChangeType::CreateTable,
            table_id,
            old_schema: None,
            new_schema: Some(new_schema.clone()),
            changed_column: None,
            schema_version: new_schema.version,
            timestamp,
        };
        self.schema_observer_manager.notify(event);
        Ok(new_schema)
    }

    /// ALTER TABLE ADD COLUMN — 添加列并分发 SchemaChangeEvent
    pub fn alter_table_add_column(
        &self,
        tx_id: u32,
        lsn: u64,
        table_id: u32,
        column: ColumnDef,
    ) -> Result<TableSchema, SchemaError> {
        let timestamp = (self.timestamp_fn)();
        let old_schema = self.registry.get_schema(table_id);
        let new_schema = self
            .registry
            .alter_table_add_column(table_id, column.clone())?;

        let event = SchemaChangeEvent {
            tx_id,
            lsn,
            change_type: SchemaChangeType::AlterTableAddColumn,
            table_id,
            old_schema,
            new_schema: Some(new_schema.clone()),
            changed_column: Some(column.name),
            schema_version: new_schema.version,
            timestamp,
        };
        self.schema_observer_manager.notify(event);
        Ok(new_schema)
    }

    /// ALTER TABLE DROP COLUMN — 删除列并分发 SchemaChangeEvent
    pub fn alter_table_drop_column(
        &self,
        tx_id: u32,
        lsn: u64,
        table_id: u32,
        column_name: &str,
    ) -> Result<TableSchema, SchemaError> {
        let timestamp = (self.timestamp_fn)();
        let old_schema = self.registry.get_schema(table_id);
        let new_schema = self
            .registry
            .alter_table_drop_column(table_id, column_name)?;

        let event = SchemaChangeEvent {
            tx_id,
            lsn,
            change_type: SchemaChangeType::AlterTableDropColumn,
            table_id,
            old_schema,
            new_schema: Some(new_schema.clone()),
            changed_column: Some(column_name.to_string()),
            schema_version: new_schema.version,
            timestamp,
        };
        self.schema_observer_manager.notify(event);
        Ok(new_schema)
    }

    /// DROP TABLE — 删除表并分发 SchemaChangeEvent
    pub fn drop_table(
        &self,
        tx_id: u32,
        lsn: u64,
        table_id: u32,
    ) -> Result<TableSchema, SchemaError> {
        let timestamp = (self.timestamp_fn)();
        let old_schema = self.registry.get_schema(table_id);
        let dropped_schema = self.registry.drop_table(table_id)?;

        let event = SchemaChangeEvent {
            tx_id,
            lsn,
            change_type: SchemaChangeType::DropTable,
            table_id,
            old_schema,
            new_schema: None,
            changed_column: None,
            schema_version: dropped_schema.version,
            timestamp,
        };
        self.schema_observer_manager.notify(event);
        Ok(dropped_schema)
    }

    // -----------------------------------------------------------------
    // DML 操作 — 产生带 schema_version 的 ChangeEvent
    // -----------------------------------------------------------------

    /// INSERT — 构造带 schema_version 的 Insert 事件并分发
    ///
    /// **返回**：`Ok(())` 表示表存在且事件已分发；`Err` 表示表不存在
    pub fn insert(
        &self,
        tx_id: u32,
        lsn: u64,
        table_id: u32,
        new_row: Vec<u8>,
    ) -> Result<(), SchemaError> {
        let schema_version = self
            .registry
            .get_version(table_id)
            .ok_or(SchemaError::TableNotFound(table_id))?;
        let timestamp = (self.timestamp_fn)();
        let event = crate::ChangeEvent::insert(tx_id, lsn, table_id, new_row, timestamp)
            .with_schema_version(schema_version);
        self.dml_observer_manager.notify(event);
        Ok(())
    }

    /// UPDATE — 构造带 schema_version 的 Update 事件并分发
    pub fn update(
        &self,
        tx_id: u32,
        lsn: u64,
        table_id: u32,
        old_row: Vec<u8>,
        new_row: Vec<u8>,
    ) -> Result<(), SchemaError> {
        let schema_version = self
            .registry
            .get_version(table_id)
            .ok_or(SchemaError::TableNotFound(table_id))?;
        let timestamp = (self.timestamp_fn)();
        let event = crate::ChangeEvent::update(tx_id, lsn, table_id, old_row, new_row, timestamp)
            .with_schema_version(schema_version);
        self.dml_observer_manager.notify(event);
        Ok(())
    }

    /// DELETE — 构造带 schema_version 的 Delete 事件并分发
    pub fn delete(
        &self,
        tx_id: u32,
        lsn: u64,
        table_id: u32,
        old_row: Vec<u8>,
    ) -> Result<(), SchemaError> {
        let schema_version = self
            .registry
            .get_version(table_id)
            .ok_or(SchemaError::TableNotFound(table_id))?;
        let timestamp = (self.timestamp_fn)();
        let event = crate::ChangeEvent::delete(tx_id, lsn, table_id, old_row, timestamp)
            .with_schema_version(schema_version);
        self.dml_observer_manager.notify(event);
        Ok(())
    }
}

// =====================================================================
// CollectingSchemaObserver — 测试用 SchemaChangeObserver
// =====================================================================

/// 收集型 SchemaChangeObserver — 将接收到的所有 SchemaChangeEvent 存入 Mutex<Vec>
///
/// 主要用于测试：注册后可通过 `events()` 获取所有接收的事件
pub struct CollectingSchemaObserver {
    events: std::sync::Mutex<Vec<SchemaChangeEvent>>,
}

impl CollectingSchemaObserver {
    pub fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// 获取已接收的事件快照（clone）
    pub fn events(&self) -> Vec<SchemaChangeEvent> {
        self.events.lock().unwrap().clone()
    }

    /// 获取已接收的事件数量
    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.events.lock().unwrap().is_empty()
    }

    /// 清空已接收的事件
    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

impl Default for CollectingSchemaObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaChangeObserver for CollectingSchemaObserver {
    fn on_schema_change(&self, event: SchemaChangeEvent) {
        self.events.lock().unwrap().push(event);
    }
}

// =====================================================================
// 单元测试
// =====================================================================

#[cfg(test)]
#[path = "schema_tests.rs"]
mod schema_tests;
