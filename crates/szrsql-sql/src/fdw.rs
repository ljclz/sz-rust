//! FDW 外部表（Foreign Data Wrapper）— Phase 6.23
//!
//! 提供 PG 风格的 FDW（Foreign Data Wrapper）功能：
//!
//! - **Foreign Server**：外部数据源连接（`CREATE SERVER`）
//! - **User Mapping**：本地用户到远程凭证的映射（`CREATE USER MAPPING`）
//! - **Foreign Table**：映射到远程表（`CREATE FOREIGN TABLE`）
//! - **FDW 接口**：扫描（SELECT）/ 插入 / 更新 / 删除的抽象接口
//! - **谓词下推**：简单的等值/范围条件可下推到 FDW
//!
//! # 设计
//!
//! - **ForeignDataWrapper trait**：定义外部数据访问的抽象接口（scan/insert/update/delete）
//! - **InMemoryFdw**：内存 mock 实现（用于测试和演示），使用 `RefCell<HashMap>` 存储数据
//! - **FdwManager**：注册中心，管理 server / user mapping / foreign table，并分派操作到对应 FDW
//! - **ScanFilter**：简单的谓词下推模型（None / Eq / Range），FDW 可选择性地利用
//!
//! # 与 PG 的关系
//!
//! - PG 8.4+ 支持 FDW（SQL/MED 标准）
//! - PG 的 FDW 通过 C 函数回调实现（`BeginForeignScan` / `IterateForeignScan` / `EndForeignScan` 等）
//! - PG 的 `postgres_fdw` 是最常用的 FDW（连接远程 PG）
//! - PG 支持谓词下推（WHERE 条件推到远程执行）、连接下推、聚合下推
//! - 本实现使用 Rust trait 替代 C 回调，语义更清晰
//!
//! # 限制
//!
//! - **无 DDL/SQL 集成**：未集成到 SQL 解析路径，仅提供程序化 API
//! - **谓词下推有限**：仅支持 Eq / Range 简单条件（PG 支持任意表达式下推）
//! - **无连接下推**：不支持 JOIN 下推到远程
//! - **无聚合下推**：不支持 GROUP BY 聚合下推
//! - **无事务下推**：不支持远程事务（2PC）
//! - **无列下推**：不支持只读取需要的列（PG 9.2+ 支持）
//! - **单线程**：InMemoryFdw 使用 RefCell，非线程安全

use crate::executor::{ExecutionError, Row};
use crate::plan::TableSchema;
use std::cell::RefCell;
use std::collections::HashMap;
use szrsql_types::value::Value;

// =====================================================================
//  错误类型
// =====================================================================

/// FDW 操作错误
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FdwError {
    /// 外部服务器不存在
    #[error("foreign server '{0}' does not exist")]
    ServerNotFound(String),
    /// 外部服务器已存在
    #[error("foreign server '{0}' already exists")]
    ServerAlreadyExists(String),
    /// 外部表不存在
    #[error("foreign table '{0}' does not exist")]
    TableNotFound(String),
    /// 外部表已存在
    #[error("foreign table '{0}' already exists")]
    TableAlreadyExists(String),
    /// 用户映射不存在
    #[error("user mapping for user '{0}' on server '{1}' does not exist")]
    UserMappingNotFound(String, String),
    /// 用户映射已存在
    #[error("user mapping for user '{0}' on server '{1}' already exists")]
    UserMappingAlreadyExists(String, String),
    /// FDW handler 未注册
    #[error("no FDW handler registered for server type '{0}'")]
    HandlerNotRegistered(String),
    /// 远程操作失败
    #[error("remote operation failed: {0}")]
    RemoteError(String),
    /// 列不匹配
    #[error("column mismatch: {0}")]
    ColumnMismatch(String),
    /// 不支持的操作
    #[error("unsupported operation: {0}")]
    Unsupported(String),
}

impl From<FdwError> for ExecutionError {
    fn from(e: FdwError) -> Self {
        ExecutionError::EvalError(format!("FDW error: {e}"))
    }
}

// =====================================================================
//  外部服务器
// =====================================================================

/// 外部服务器（Foreign Server）
///
/// 对应 PG 的 `CREATE SERVER`，表示一个外部数据源连接。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignServer {
    /// 服务器名称
    pub name: String,
    /// 服务器类型（如 `postgres`、`mysql`、`file`）— 决定使用哪个 FDW handler
    pub server_type: String,
    /// 服务器版本（可选）
    pub version: Option<String>,
    /// 服务器选项（如 host、port、dbname）
    pub options: HashMap<String, String>,
    /// 所有者
    pub owner: String,
}

impl ForeignServer {
    /// 创建外部服务器
    pub fn new(name: impl Into<String>, server_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            server_type: server_type.into(),
            version: None,
            options: HashMap::new(),
            owner: String::new(),
        }
    }

    /// 设置服务器版本
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// 设置所有者
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = owner.into();
        self
    }

    /// 添加选项
    pub fn with_option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.insert(key.into(), value.into());
        self
    }
}

// =====================================================================
//  用户映射
// =====================================================================

/// 用户映射（User Mapping）
///
/// 对应 PG 的 `CREATE USER MAPPING`，将本地用户映射到远程凭证。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMapping {
    /// 本地用户名
    pub local_user: String,
    /// 外部服务器名称
    pub server_name: String,
    /// 远程认证选项（如 user、password）
    pub options: HashMap<String, String>,
}

impl UserMapping {
    /// 创建用户映射
    pub fn new(local_user: impl Into<String>, server_name: impl Into<String>) -> Self {
        Self {
            local_user: local_user.into(),
            server_name: server_name.into(),
            options: HashMap::new(),
        }
    }

    /// 添加选项
    pub fn with_option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.insert(key.into(), value.into());
        self
    }
}

// =====================================================================
//  外部表
// =====================================================================

/// 外部表（Foreign Table）
///
/// 对应 PG 的 `CREATE FOREIGN TABLE`，映射到远程表。
#[derive(Debug, Clone, PartialEq)]
pub struct ForeignTable {
    /// 外部表名
    pub name: String,
    /// 关联的外部服务器名
    pub server_name: String,
    /// 表结构（列定义）
    pub schema: TableSchema,
    /// 外部表选项（如 `schema_name`、`table_name`）
    pub options: HashMap<String, String>,
}

impl ForeignTable {
    /// 创建外部表
    pub fn new(
        name: impl Into<String>,
        server_name: impl Into<String>,
        schema: TableSchema,
    ) -> Self {
        Self {
            name: name.into(),
            server_name: server_name.into(),
            schema,
            options: HashMap::new(),
        }
    }

    /// 添加选项
    pub fn with_option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.insert(key.into(), value.into());
        self
    }

    /// 获取远程表名（从 options 读取 `table_name`，默认使用本地表名）
    pub fn remote_table_name(&self) -> &str {
        self.options
            .get("table_name")
            .map(|s| s.as_str())
            .unwrap_or(&self.name)
    }

    /// 获取远程 schema 名（从 options 读取 `schema_name`，默认 `public`）
    pub fn remote_schema_name(&self) -> &str {
        self.options
            .get("schema_name")
            .map(|s| s.as_str())
            .unwrap_or("public")
    }

    /// 列数
    pub fn num_columns(&self) -> usize {
        self.schema.columns.len()
    }
}

// =====================================================================
//  扫描谓词（谓词下推）
// =====================================================================

/// 类型感知的 Value 比较 — 返回 Ordering
///
/// `Value` 未实现 `PartialOrd`，故在此提供本地比较函数。
/// 仅支持常用类型（Null/Int64/Float64/Text/Bool/Date/Timestamp），
/// 跨类型比较按 Int64↔Float64 隐式转换，其余按 Debug 字符串排序。
fn value_compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Int64(x), Value::Int64(y)) => x.cmp(y),
        (Value::Float64(x), Value::Float64(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Int64(x), Value::Float64(y)) => {
            (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (Value::Float64(x), Value::Int64(y)) => {
            x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
        }
        (Value::Text(x), Value::Text(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::Timestamp(x), Value::Timestamp(y)) => x.cmp(y),
        _ => format!("{a:?}").cmp(&format!("{b:?}")),
    }
}

/// 扫描谓词 — 可下推到 FDW 的简单过滤条件
///
/// FDW 实现可选择性地利用此谓词在远程端过滤，减少数据传输。
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ScanFilter {
    /// 无过滤（全表扫描）
    #[default]
    None,
    /// 等值条件（列索引, 值）— `column[idx] = value`
    Eq(usize, Value),
    /// 范围条件（列索引, 下界, 上界）— `lower <= column[idx] <= upper`
    Range(usize, Option<Value>, Option<Value>),
}

impl ScanFilter {
    /// 检查行是否匹配谓词
    ///
    /// FDW 实现可使用此方法在本地过滤（当远程不支持谓词下推时）。
    pub fn matches(&self, row: &Row) -> bool {
        match self {
            Self::None => true,
            Self::Eq(idx, value) => row.get(*idx).is_some_and(|v| v == value),
            Self::Range(idx, lower, upper) => {
                let Some(v) = row.get(*idx) else {
                    return false;
                };
                if let Some(lo) = lower {
                    if value_compare(v, lo) == std::cmp::Ordering::Less {
                        return false;
                    }
                }
                if let Some(hi) = upper {
                    if value_compare(v, hi) == std::cmp::Ordering::Greater {
                        return false;
                    }
                }
                true
            }
        }
    }

    /// 是否为空（无过滤）
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

// =====================================================================
//  FDW 接口 trait
// =====================================================================

/// 外部数据包装器接口（Foreign Data Wrapper Interface）
///
/// 定义外部数据访问的抽象接口。每种外部数据源（postgres、mysql、file 等）
/// 实现此 trait 以提供数据访问能力。
///
/// 对应 PG 的 FDW handler 回调函数集：
/// - `BeginForeignScan` / `IterateForeignScan` / `EndForeignScan` → `scan`
/// - `ExecForeignInsert` → `insert`
/// - `ExecForeignUpdate` → `update`
/// - `ExecForeignDelete` → `delete`
/// - `ExplainForeignScan` → `explain`
pub trait ForeignDataWrapper {
    /// 扫描外部表（SELECT）
    ///
    /// - `table` — 外部表定义
    /// - `filter` — 可选谓词下推
    ///
    /// 返回匹配的行。FDW 实现应尽可能在远程端应用 `filter`。
    fn scan(&self, table: &ForeignTable, filter: &ScanFilter) -> Result<Vec<Row>, FdwError>;

    /// 插入行到外部表（INSERT）
    ///
    /// 返回插入的行数。
    fn insert(&self, table: &ForeignTable, rows: &[Row]) -> Result<usize, FdwError>;

    /// 更新外部表行（UPDATE）
    ///
    /// - `filter` — 哪些行需要更新
    /// - `new_values` — 需要更新的列（列索引, 新值）
    ///
    /// 返回更新的行数。
    fn update(
        &self,
        table: &ForeignTable,
        filter: &ScanFilter,
        new_values: &[(usize, Value)],
    ) -> Result<usize, FdwError>;

    /// 删除外部表行（DELETE）
    ///
    /// 返回删除的行数。
    fn delete(&self, table: &ForeignTable, filter: &ScanFilter) -> Result<usize, FdwError>;

    /// EXPLAIN 输出
    ///
    /// 默认返回 "Foreign Scan on <table>"。FDW 实现可覆盖以提供更多信息。
    fn explain(&self, table: &ForeignTable) -> Vec<String> {
        vec![format!("Foreign Scan on {}", table.name)]
    }
}

// =====================================================================
//  InMemoryFdw — 内存 mock 实现
// =====================================================================

/// 内存 FDW 实现（用于测试和演示）
///
/// 数据存储在内存 `RefCell<HashMap<String, Vec<Row>>>` 中，
/// 键为远程表名（`remote_table_name()`）。
///
/// 支持谓词下推（在 scan 中应用 ScanFilter）。
pub struct InMemoryFdw {
    /// 远程数据：<remote_table_name, rows>
    data: RefCell<HashMap<String, Vec<Row>>>,
}

impl InMemoryFdw {
    /// 创建空的内存 FDW
    pub fn new() -> Self {
        Self {
            data: RefCell::new(HashMap::new()),
        }
    }

    /// 预加载远程表数据（用于测试初始化）
    pub fn with_data(table_name: impl Into<String>, rows: Vec<Row>) -> Self {
        let mut data = HashMap::new();
        data.insert(table_name.into(), rows);
        Self {
            data: RefCell::new(data),
        }
    }

    /// 预加载多张远程表数据
    pub fn with_data_multi(tables: Vec<(String, Vec<Row>)>) -> Self {
        let data: HashMap<String, Vec<Row>> = tables.into_iter().collect();
        Self {
            data: RefCell::new(data),
        }
    }

    /// 获取远程表的行数（用于测试验证）
    pub fn row_count(&self, table_name: &str) -> usize {
        self.data.borrow().get(table_name).map_or(0, Vec::len)
    }

    /// 获取远程表的全部行（用于测试验证）
    pub fn get_rows(&self, table_name: &str) -> Vec<Row> {
        self.data
            .borrow()
            .get(table_name)
            .cloned()
            .unwrap_or_default()
    }
}

impl Default for InMemoryFdw {
    fn default() -> Self {
        Self::new()
    }
}

impl ForeignDataWrapper for InMemoryFdw {
    fn scan(&self, table: &ForeignTable, filter: &ScanFilter) -> Result<Vec<Row>, FdwError> {
        let data = self.data.borrow();
        let remote_name = table.remote_table_name().to_string();
        let rows = data.get(&remote_name).cloned().unwrap_or_default();

        // 应用谓词下推
        let result = if filter.is_none() {
            rows
        } else {
            rows.into_iter().filter(|r| filter.matches(r)).collect()
        };

        Ok(result)
    }

    fn insert(&self, table: &ForeignTable, rows: &[Row]) -> Result<usize, FdwError> {
        // 列数校验
        let expected = table.num_columns();
        for (i, row) in rows.iter().enumerate() {
            if row.len() != expected {
                return Err(FdwError::ColumnMismatch(format!(
                    "row {i}: expected {expected} columns, got {}",
                    row.len()
                )));
            }
        }

        let mut data = self.data.borrow_mut();
        let remote_name = table.remote_table_name().to_string();
        let table_rows = data.entry(remote_name).or_default();
        let count = rows.len();
        table_rows.extend_from_slice(rows);
        Ok(count)
    }

    fn update(
        &self,
        table: &ForeignTable,
        filter: &ScanFilter,
        new_values: &[(usize, Value)],
    ) -> Result<usize, FdwError> {
        // 列索引校验
        let num_cols = table.num_columns();
        for (idx, _) in new_values {
            if *idx >= num_cols {
                return Err(FdwError::ColumnMismatch(format!(
                    "update column index {idx} out of range (table has {num_cols} columns)"
                )));
            }
        }

        let mut data = self.data.borrow_mut();
        let remote_name = table.remote_table_name().to_string();
        let table_rows = data.entry(remote_name).or_default();

        let mut updated = 0usize;
        for row in table_rows.iter_mut() {
            if filter.matches(row) {
                for (idx, val) in new_values {
                    if *idx < row.len() {
                        row[*idx] = val.clone();
                    }
                }
                updated += 1;
            }
        }
        Ok(updated)
    }

    fn delete(&self, table: &ForeignTable, filter: &ScanFilter) -> Result<usize, FdwError> {
        let mut data = self.data.borrow_mut();
        let remote_name = table.remote_table_name().to_string();

        let table_rows = match data.get_mut(&remote_name) {
            Some(rows) => rows,
            None => return Ok(0),
        };

        let before = table_rows.len();
        table_rows.retain(|r| !filter.matches(r));
        let deleted = before - table_rows.len();
        Ok(deleted)
    }

    fn explain(&self, table: &ForeignTable) -> Vec<String> {
        let mut lines = vec![format!("Foreign Scan on {}", table.name)];
        lines.push(format!("  Remote server: {}", table.server_name));
        lines.push(format!(
            "  Remote table: {}.{}",
            table.remote_schema_name(),
            table.remote_table_name()
        ));
        lines
    }
}

// =====================================================================
//  FdwManager — FDW 管理器
// =====================================================================

/// FDW 管理器
///
/// 管理外部服务器、用户映射、外部表，以及 FDW handler 注册。
/// 分派操作到对应 server_type 的 FDW handler。
///
/// # 用法
///
/// ```ignore
/// use szrsql_sql::fdw::*;
///
/// let mut manager = FdwManager::new();
///
/// // 1. 注册 FDW handler（server_type = "memory"）
/// manager.register_handler("memory", Box::new(InMemoryFdw::new()));
///
/// // 2. 创建外部服务器
/// manager.create_server(ForeignServer::new("my_server", "memory")).unwrap();
///
/// // 3. 创建外部表
/// let schema = TableSchema::new(TableName::new("ft"));
/// manager.create_foreign_table(ForeignTable::new("ft", "my_server", schema)).unwrap();
///
/// // 4. 扫描
/// let rows = manager.scan("ft", &ScanFilter::None).unwrap();
/// ```
pub struct FdwManager {
    /// 已注册的 FDW handler：<server_type, handler>
    handlers: HashMap<String, Box<dyn ForeignDataWrapper>>,
    /// 外部服务器：<server_name, server>
    servers: HashMap<String, ForeignServer>,
    /// 用户映射：<(local_user, server_name), mapping>
    user_mappings: HashMap<(String, String), UserMapping>,
    /// 外部表：<table_name, table>
    foreign_tables: HashMap<String, ForeignTable>,
}

impl FdwManager {
    /// 创建空的 FDW 管理器
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            servers: HashMap::new(),
            user_mappings: HashMap::new(),
            foreign_tables: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------
    //  FDW handler 注册
    // -----------------------------------------------------------------

    /// 注册 FDW handler
    ///
    /// `server_type` 对应 `ForeignServer.server_type`，决定使用哪个 handler。
    /// 重复注册会覆盖原有 handler。
    pub fn register_handler(
        &mut self,
        server_type: impl Into<String>,
        handler: Box<dyn ForeignDataWrapper>,
    ) {
        self.handlers.insert(server_type.into(), handler);
    }

    /// 检查 server_type 是否已注册 handler
    pub fn has_handler(&self, server_type: &str) -> bool {
        self.handlers.contains_key(server_type)
    }

    /// 获取外部表对应的 FDW handler
    fn get_handler(&self, table_name: &str) -> Result<&dyn ForeignDataWrapper, FdwError> {
        let table = self
            .foreign_tables
            .get(table_name)
            .ok_or_else(|| FdwError::TableNotFound(table_name.to_string()))?;
        let server = self
            .servers
            .get(&table.server_name)
            .ok_or_else(|| FdwError::ServerNotFound(table.server_name.clone()))?;
        self.handlers
            .get(&server.server_type)
            .map(|b| b.as_ref())
            .ok_or_else(|| FdwError::HandlerNotRegistered(server.server_type.clone()))
    }

    // -----------------------------------------------------------------
    //  外部服务器管理
    // -----------------------------------------------------------------

    /// 创建外部服务器
    pub fn create_server(&mut self, server: ForeignServer) -> Result<(), FdwError> {
        if self.servers.contains_key(&server.name) {
            return Err(FdwError::ServerAlreadyExists(server.name));
        }
        self.servers.insert(server.name.clone(), server);
        Ok(())
    }

    /// 删除外部服务器
    ///
    /// 如果有关联的外部表或用户映射，返回错误（需先删除关联对象）。
    pub fn drop_server(&mut self, name: &str, if_exists: bool) -> Result<(), FdwError> {
        if !self.servers.contains_key(name) {
            if if_exists {
                return Ok(());
            }
            return Err(FdwError::ServerNotFound(name.to_string()));
        }

        // 检查关联外部表
        if self.foreign_tables.values().any(|t| t.server_name == name) {
            return Err(FdwError::Unsupported(format!(
                "cannot drop server '{name}' because foreign tables depend on it"
            )));
        }

        // 检查关联用户映射
        let has_mapping = self.user_mappings.keys().any(|(_, sn)| sn == name);
        if has_mapping {
            return Err(FdwError::Unsupported(format!(
                "cannot drop server '{name}' because user mappings depend on it"
            )));
        }

        self.servers.remove(name);
        Ok(())
    }

    /// 获取外部服务器
    pub fn get_server(&self, name: &str) -> Option<&ForeignServer> {
        self.servers.get(name)
    }

    /// 列出所有外部服务器
    pub fn list_servers(&self) -> Vec<&ForeignServer> {
        self.servers.values().collect()
    }

    // -----------------------------------------------------------------
    //  用户映射管理
    // -----------------------------------------------------------------

    /// 创建用户映射
    pub fn create_user_mapping(&mut self, mapping: UserMapping) -> Result<(), FdwError> {
        // 检查服务器存在
        if !self.servers.contains_key(&mapping.server_name) {
            return Err(FdwError::ServerNotFound(mapping.server_name.clone()));
        }

        let key = (mapping.local_user.clone(), mapping.server_name.clone());
        if self.user_mappings.contains_key(&key) {
            return Err(FdwError::UserMappingAlreadyExists(
                mapping.local_user,
                mapping.server_name,
            ));
        }

        self.user_mappings.insert(key, mapping);
        Ok(())
    }

    /// 删除用户映射
    pub fn drop_user_mapping(
        &mut self,
        local_user: &str,
        server_name: &str,
        if_exists: bool,
    ) -> Result<(), FdwError> {
        let key = (local_user.to_string(), server_name.to_string());
        if self.user_mappings.remove(&key).is_none() && !if_exists {
            return Err(FdwError::UserMappingNotFound(
                local_user.to_string(),
                server_name.to_string(),
            ));
        }
        Ok(())
    }

    /// 获取用户映射
    pub fn get_user_mapping(&self, local_user: &str, server_name: &str) -> Option<&UserMapping> {
        self.user_mappings
            .get(&(local_user.to_string(), server_name.to_string()))
    }

    /// 列出所有用户映射
    pub fn list_user_mappings(&self) -> Vec<&UserMapping> {
        self.user_mappings.values().collect()
    }

    // -----------------------------------------------------------------
    //  外部表管理
    // -----------------------------------------------------------------

    /// 创建外部表
    pub fn create_foreign_table(&mut self, table: ForeignTable) -> Result<(), FdwError> {
        // 检查服务器存在
        if !self.servers.contains_key(&table.server_name) {
            return Err(FdwError::ServerNotFound(table.server_name.clone()));
        }

        if self.foreign_tables.contains_key(&table.name) {
            return Err(FdwError::TableAlreadyExists(table.name));
        }

        self.foreign_tables.insert(table.name.clone(), table);
        Ok(())
    }

    /// 删除外部表
    pub fn drop_foreign_table(&mut self, name: &str, if_exists: bool) -> Result<(), FdwError> {
        if self.foreign_tables.remove(name).is_none() && !if_exists {
            return Err(FdwError::TableNotFound(name.to_string()));
        }
        Ok(())
    }

    /// 获取外部表
    pub fn get_foreign_table(&self, name: &str) -> Option<&ForeignTable> {
        self.foreign_tables.get(name)
    }

    /// 列出所有外部表
    pub fn list_foreign_tables(&self) -> Vec<&ForeignTable> {
        self.foreign_tables.values().collect()
    }

    // -----------------------------------------------------------------
    //  数据操作（分派到 FDW handler）
    // -----------------------------------------------------------------

    /// 扫描外部表（SELECT）
    pub fn scan(&self, table_name: &str, filter: &ScanFilter) -> Result<Vec<Row>, FdwError> {
        let handler = self.get_handler(table_name)?;
        let table = self
            .foreign_tables
            .get(table_name)
            .ok_or_else(|| FdwError::TableNotFound(table_name.to_string()))?;
        handler.scan(table, filter)
    }

    /// 插入行到外部表（INSERT）
    pub fn insert(&self, table_name: &str, rows: &[Row]) -> Result<usize, FdwError> {
        let handler = self.get_handler(table_name)?;
        let table = self
            .foreign_tables
            .get(table_name)
            .ok_or_else(|| FdwError::TableNotFound(table_name.to_string()))?;
        handler.insert(table, rows)
    }

    /// 更新外部表行（UPDATE）
    pub fn update(
        &self,
        table_name: &str,
        filter: &ScanFilter,
        new_values: &[(usize, Value)],
    ) -> Result<usize, FdwError> {
        let handler = self.get_handler(table_name)?;
        let table = self
            .foreign_tables
            .get(table_name)
            .ok_or_else(|| FdwError::TableNotFound(table_name.to_string()))?;
        handler.update(table, filter, new_values)
    }

    /// 删除外部表行（DELETE）
    pub fn delete(&self, table_name: &str, filter: &ScanFilter) -> Result<usize, FdwError> {
        let handler = self.get_handler(table_name)?;
        let table = self
            .foreign_tables
            .get(table_name)
            .ok_or_else(|| FdwError::TableNotFound(table_name.to_string()))?;
        handler.delete(table, filter)
    }

    /// EXPLAIN 外部表扫描
    pub fn explain(&self, table_name: &str) -> Result<Vec<String>, FdwError> {
        let handler = self.get_handler(table_name)?;
        let table = self
            .foreign_tables
            .get(table_name)
            .ok_or_else(|| FdwError::TableNotFound(table_name.to_string()))?;
        Ok(handler.explain(table))
    }
}

impl Default for FdwManager {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
//  测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{ColumnDefinition, TableName};
    use szrsql_types::value::ColumnType;

    // -----------------------------------------------------------------
    //  测试辅助
    // -----------------------------------------------------------------

    /// 创建测试用表结构：id INT, name TEXT
    fn make_schema(name: &str) -> TableSchema {
        TableSchema {
            name: TableName::new(name),
            columns: vec![
                ColumnDefinition::new("id", ColumnType::Int64),
                ColumnDefinition::new("name", ColumnType::Text),
            ],
        }
    }

    /// 创建测试行：(id, name)
    fn make_row(id: i64, name: &str) -> Row {
        vec![Value::Int64(id), Value::Text(name.to_string())]
    }

    /// 创建预加载的 InMemoryFdw
    fn make_fdw_with_data() -> InMemoryFdw {
        InMemoryFdw::with_data(
            "remote_t",
            vec![
                make_row(1, "alice"),
                make_row(2, "bob"),
                make_row(3, "carol"),
                make_row(4, "dave"),
                make_row(5, "eve"),
            ],
        )
    }

    // =================================================================
    //  ForeignServer 测试
    // =================================================================

    #[test]
    fn test_foreign_server_new() {
        let server = ForeignServer::new("pg_server", "postgres");
        assert_eq!(server.name, "pg_server");
        assert_eq!(server.server_type, "postgres");
        assert_eq!(server.version, None);
        assert!(server.options.is_empty());
    }

    #[test]
    fn test_foreign_server_builder() {
        let server = ForeignServer::new("pg_server", "postgres")
            .with_version("15.3")
            .with_owner("admin")
            .with_option("host", "localhost")
            .with_option("port", "5432");
        assert_eq!(server.version, Some("15.3".to_string()));
        assert_eq!(server.owner, "admin");
        assert_eq!(server.options.get("host"), Some(&"localhost".to_string()));
        assert_eq!(server.options.get("port"), Some(&"5432".to_string()));
    }

    // =================================================================
    //  UserMapping 测试
    // =================================================================

    #[test]
    fn test_user_mapping_new() {
        let mapping = UserMapping::new("local_user", "pg_server");
        assert_eq!(mapping.local_user, "local_user");
        assert_eq!(mapping.server_name, "pg_server");
        assert!(mapping.options.is_empty());
    }

    #[test]
    fn test_user_mapping_with_options() {
        let mapping = UserMapping::new("local_user", "pg_server")
            .with_option("user", "remote_user")
            .with_option("password", "secret");
        assert_eq!(
            mapping.options.get("user"),
            Some(&"remote_user".to_string())
        );
        assert_eq!(mapping.options.get("password"), Some(&"secret".to_string()));
    }

    // =================================================================
    //  ForeignTable 测试
    // =================================================================

    #[test]
    fn test_foreign_table_new() {
        let schema = make_schema("ft");
        let table = ForeignTable::new("ft", "pg_server", schema);
        assert_eq!(table.name, "ft");
        assert_eq!(table.server_name, "pg_server");
        assert_eq!(table.num_columns(), 2);
        assert!(table.options.is_empty());
    }

    #[test]
    fn test_foreign_table_with_option() {
        let schema = make_schema("ft");
        let table = ForeignTable::new("ft", "pg_server", schema)
            .with_option("schema_name", "public")
            .with_option("table_name", "remote_t");
        assert_eq!(
            table.options.get("schema_name"),
            Some(&"public".to_string())
        );
        assert_eq!(
            table.options.get("table_name"),
            Some(&"remote_t".to_string())
        );
    }

    #[test]
    fn test_foreign_table_remote_names() {
        let schema = make_schema("ft");

        // 默认：remote_table_name = 本地名，remote_schema_name = "public"
        let table1 = ForeignTable::new("ft", "pg_server", schema.clone());
        assert_eq!(table1.remote_table_name(), "ft");
        assert_eq!(table1.remote_schema_name(), "public");

        // 带选项：使用 options 中的值
        let table2 = ForeignTable::new("ft", "pg_server", schema)
            .with_option("schema_name", "my_schema")
            .with_option("table_name", "remote_t");
        assert_eq!(table2.remote_table_name(), "remote_t");
        assert_eq!(table2.remote_schema_name(), "my_schema");
    }

    // =================================================================
    //  ScanFilter 测试
    // =================================================================

    #[test]
    fn test_scan_filter_none_matches_all() {
        let row = make_row(1, "alice");
        assert!(ScanFilter::None.matches(&row));
        assert!(ScanFilter::None.is_none());
    }

    #[test]
    fn test_scan_filter_eq() {
        let row = make_row(3, "carol");
        assert!(ScanFilter::Eq(0, Value::Int64(3)).matches(&row));
        assert!(!ScanFilter::Eq(0, Value::Int64(5)).matches(&row));
        assert!(ScanFilter::Eq(1, Value::Text("carol".to_string())).matches(&row));
        assert!(!ScanFilter::Eq(1, Value::Text("bob".to_string())).matches(&row));
    }

    #[test]
    fn test_scan_filter_range() {
        let row = make_row(3, "carol");

        // 3 在 [1, 5] 范围内
        assert!(ScanFilter::Range(0, Some(Value::Int64(1)), Some(Value::Int64(5))).matches(&row));
        // 3 不在 [5, 10] 范围内
        assert!(!ScanFilter::Range(0, Some(Value::Int64(5)), Some(Value::Int64(10))).matches(&row));
        // 只有下界：3 >= 2
        assert!(ScanFilter::Range(0, Some(Value::Int64(2)), None).matches(&row));
        // 只有上界：3 <= 5
        assert!(ScanFilter::Range(0, None, Some(Value::Int64(5))).matches(&row));
        // 无界（等同 None）
        assert!(ScanFilter::Range(0, None, None).matches(&row));
    }

    #[test]
    fn test_scan_filter_default() {
        assert!(ScanFilter::default().is_none());
    }

    // =================================================================
    //  InMemoryFdw — scan 测试
    // =================================================================

    #[test]
    fn test_inmemory_fdw_scan_no_filter() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");
        let rows = fdw.scan(&table, &ScanFilter::None).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0][0], Value::Int64(1));
        assert_eq!(rows[4][0], Value::Int64(5));
    }

    #[test]
    fn test_inmemory_fdw_scan_with_eq_filter() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        // id = 3
        let rows = fdw
            .scan(&table, &ScanFilter::Eq(0, Value::Int64(3)))
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], Value::Text("carol".to_string()));
    }

    #[test]
    fn test_inmemory_fdw_scan_with_range_filter() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        // 2 <= id <= 4
        let rows = fdw
            .scan(
                &table,
                &ScanFilter::Range(0, Some(Value::Int64(2)), Some(Value::Int64(4))),
            )
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][0], Value::Int64(2));
        assert_eq!(rows[2][0], Value::Int64(4));
    }

    #[test]
    fn test_inmemory_fdw_scan_empty_table() {
        let fdw = InMemoryFdw::new();
        let table = ForeignTable::new("ft", "srv", make_schema("ft"))
            .with_option("table_name", "nonexistent");
        let rows = fdw.scan(&table, &ScanFilter::None).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn test_inmemory_fdw_scan_no_match() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        // id = 999 — 无匹配
        let rows = fdw
            .scan(&table, &ScanFilter::Eq(0, Value::Int64(999)))
            .unwrap();
        assert!(rows.is_empty());
    }

    // =================================================================
    //  InMemoryFdw — insert 测试
    // =================================================================

    #[test]
    fn test_inmemory_fdw_insert() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        let count = fdw
            .insert(&table, &[make_row(6, "frank"), make_row(7, "grace")])
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(fdw.row_count("remote_t"), 7);
        assert_eq!(
            fdw.get_rows("remote_t")[5][1],
            Value::Text("frank".to_string())
        );
        assert_eq!(
            fdw.get_rows("remote_t")[6][1],
            Value::Text("grace".to_string())
        );
    }

    #[test]
    fn test_inmemory_fdw_insert_into_empty_table() {
        let fdw = InMemoryFdw::new();
        let table = ForeignTable::new("ft", "srv", make_schema("ft"))
            .with_option("table_name", "new_table");

        let count = fdw.insert(&table, &[make_row(1, "first")]).unwrap();
        assert_eq!(count, 1);
        assert_eq!(fdw.row_count("new_table"), 1);
    }

    #[test]
    fn test_inmemory_fdw_insert_column_mismatch() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        // 列数不匹配（3 列 vs 表 2 列）
        let bad_row = vec![
            Value::Int64(1),
            Value::Text("x".to_string()),
            Value::Int64(99),
        ];
        let err = fdw.insert(&table, &[bad_row]).unwrap_err();
        assert!(matches!(err, FdwError::ColumnMismatch(_)));
    }

    // =================================================================
    //  InMemoryFdw — update 测试
    // =================================================================

    #[test]
    fn test_inmemory_fdw_update_eq() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        // UPDATE ... WHERE id = 3 SET name = 'charlie'
        let count = fdw
            .update(
                &table,
                &ScanFilter::Eq(0, Value::Int64(3)),
                &[(1, Value::Text("charlie".to_string()))],
            )
            .unwrap();
        assert_eq!(count, 1);

        let rows = fdw.get_rows("remote_t");
        let carol_row = rows.iter().find(|r| r[0] == Value::Int64(3)).unwrap();
        assert_eq!(carol_row[1], Value::Text("charlie".to_string()));
    }

    #[test]
    fn test_inmemory_fdw_update_range() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        // UPDATE ... WHERE id BETWEEN 2 AND 4 SET name = 'updated'
        let count = fdw
            .update(
                &table,
                &ScanFilter::Range(0, Some(Value::Int64(2)), Some(Value::Int64(4))),
                &[(1, Value::Text("updated".to_string()))],
            )
            .unwrap();
        assert_eq!(count, 3);

        let rows = fdw.get_rows("remote_t");
        for r in &rows {
            let id = &r[0];
            let in_range = value_compare(id, &Value::Int64(2)) != std::cmp::Ordering::Less
                && value_compare(id, &Value::Int64(4)) != std::cmp::Ordering::Greater;
            if in_range {
                assert_eq!(r[1], Value::Text("updated".to_string()));
            }
        }
    }

    #[test]
    fn test_inmemory_fdw_update_no_match() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        let count = fdw
            .update(
                &table,
                &ScanFilter::Eq(0, Value::Int64(999)),
                &[(1, Value::Text("x".to_string()))],
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_inmemory_fdw_update_column_out_of_range() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        let err = fdw
            .update(&table, &ScanFilter::None, &[(5, Value::Int64(99))])
            .unwrap_err();
        assert!(matches!(err, FdwError::ColumnMismatch(_)));
    }

    // =================================================================
    //  InMemoryFdw — delete 测试
    // =================================================================

    #[test]
    fn test_inmemory_fdw_delete_eq() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        let count = fdw
            .delete(&table, &ScanFilter::Eq(0, Value::Int64(3)))
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(fdw.row_count("remote_t"), 4);

        // carol (id=3) 不再存在
        let rows = fdw.get_rows("remote_t");
        assert!(!rows.iter().any(|r| r[0] == Value::Int64(3)));
    }

    #[test]
    fn test_inmemory_fdw_delete_range() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        let count = fdw
            .delete(
                &table,
                &ScanFilter::Range(0, Some(Value::Int64(2)), Some(Value::Int64(4))),
            )
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(fdw.row_count("remote_t"), 2);
    }

    #[test]
    fn test_inmemory_fdw_delete_all() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        let count = fdw.delete(&table, &ScanFilter::None).unwrap();
        assert_eq!(count, 5);
        assert_eq!(fdw.row_count("remote_t"), 0);
    }

    #[test]
    fn test_inmemory_fdw_delete_no_match() {
        let fdw = make_fdw_with_data();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");

        let count = fdw
            .delete(&table, &ScanFilter::Eq(0, Value::Int64(999)))
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(fdw.row_count("remote_t"), 5);
    }

    #[test]
    fn test_inmemory_fdw_delete_from_empty_table() {
        let fdw = InMemoryFdw::new();
        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "empty");

        let count = fdw.delete(&table, &ScanFilter::None).unwrap();
        assert_eq!(count, 0);
    }

    // =================================================================
    //  InMemoryFdw — explain 测试
    // =================================================================

    #[test]
    fn test_inmemory_fdw_explain() {
        let fdw = make_fdw_with_data();
        let table = ForeignTable::new("ft", "pg_server", make_schema("ft"))
            .with_option("schema_name", "public")
            .with_option("table_name", "remote_t");

        let lines = fdw.explain(&table);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Foreign Scan on ft");
        assert!(lines[1].contains("pg_server"));
        assert!(lines[2].contains("public.remote_t"));
    }

    // =================================================================
    //  FdwManager — handler 注册测试
    // =================================================================

    #[test]
    fn test_manager_register_handler() {
        let mut manager = FdwManager::new();
        assert!(!manager.has_handler("memory"));

        manager.register_handler("memory", Box::new(InMemoryFdw::new()));
        assert!(manager.has_handler("memory"));
    }

    // =================================================================
    //  FdwManager — server 管理测试
    // =================================================================

    #[test]
    fn test_manager_create_server() {
        let mut manager = FdwManager::new();
        let server = ForeignServer::new("pg_server", "memory").with_option("host", "localhost");
        manager.create_server(server).unwrap();

        assert!(manager.get_server("pg_server").is_some());
        assert_eq!(manager.list_servers().len(), 1);
    }

    #[test]
    fn test_manager_create_server_duplicate() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("pg_server", "memory"))
            .unwrap();

        let err = manager
            .create_server(ForeignServer::new("pg_server", "memory"))
            .unwrap_err();
        assert!(matches!(err, FdwError::ServerAlreadyExists(_)));
    }

    #[test]
    fn test_manager_drop_server() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("pg_server", "memory"))
            .unwrap();

        manager.drop_server("pg_server", false).unwrap();
        assert!(manager.get_server("pg_server").is_none());
    }

    #[test]
    fn test_manager_drop_server_not_found() {
        let mut manager = FdwManager::new();

        let err = manager.drop_server("nonexistent", false).unwrap_err();
        assert!(matches!(err, FdwError::ServerNotFound(_)));
    }

    #[test]
    fn test_manager_drop_server_if_exists() {
        let mut manager = FdwManager::new();
        // 删除不存在的服务器，if_exists = true 不报错
        manager.drop_server("nonexistent", true).unwrap();
    }

    #[test]
    fn test_manager_drop_server_with_foreign_table_fails() {
        let mut manager = FdwManager::new();
        manager.register_handler("memory", Box::new(InMemoryFdw::new()));
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();
        manager
            .create_foreign_table(ForeignTable::new("ft", "srv", make_schema("ft")))
            .unwrap();

        // 有关联外部表，不能删除
        let err = manager.drop_server("srv", false).unwrap_err();
        assert!(matches!(err, FdwError::Unsupported(_)));
    }

    #[test]
    fn test_manager_drop_server_with_user_mapping_fails() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();
        manager
            .create_user_mapping(UserMapping::new("user", "srv"))
            .unwrap();

        // 有关联用户映射，不能删除
        let err = manager.drop_server("srv", false).unwrap_err();
        assert!(matches!(err, FdwError::Unsupported(_)));
    }

    // =================================================================
    //  FdwManager — user mapping 管理测试
    // =================================================================

    #[test]
    fn test_manager_create_user_mapping() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();

        let mapping = UserMapping::new("local_user", "srv").with_option("user", "remote_user");
        manager.create_user_mapping(mapping).unwrap();

        assert!(manager.get_user_mapping("local_user", "srv").is_some());
        assert_eq!(manager.list_user_mappings().len(), 1);
    }

    #[test]
    fn test_manager_create_user_mapping_server_not_found() {
        let mut manager = FdwManager::new();

        let err = manager
            .create_user_mapping(UserMapping::new("user", "nonexistent"))
            .unwrap_err();
        assert!(matches!(err, FdwError::ServerNotFound(_)));
    }

    #[test]
    fn test_manager_create_user_mapping_duplicate() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();
        manager
            .create_user_mapping(UserMapping::new("user", "srv"))
            .unwrap();

        let err = manager
            .create_user_mapping(UserMapping::new("user", "srv"))
            .unwrap_err();
        assert!(matches!(err, FdwError::UserMappingAlreadyExists(_, _)));
    }

    #[test]
    fn test_manager_drop_user_mapping() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();
        manager
            .create_user_mapping(UserMapping::new("user", "srv"))
            .unwrap();

        manager.drop_user_mapping("user", "srv", false).unwrap();
        assert!(manager.get_user_mapping("user", "srv").is_none());
    }

    #[test]
    fn test_manager_drop_user_mapping_not_found() {
        let mut manager = FdwManager::new();
        let err = manager.drop_user_mapping("user", "srv", false).unwrap_err();
        assert!(matches!(err, FdwError::UserMappingNotFound(_, _)));
    }

    // =================================================================
    //  FdwManager — foreign table 管理测试
    // =================================================================

    #[test]
    fn test_manager_create_foreign_table() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();

        let table =
            ForeignTable::new("ft", "srv", make_schema("ft")).with_option("table_name", "remote_t");
        manager.create_foreign_table(table).unwrap();

        assert!(manager.get_foreign_table("ft").is_some());
        assert_eq!(manager.list_foreign_tables().len(), 1);
    }

    #[test]
    fn test_manager_create_foreign_table_server_not_found() {
        let mut manager = FdwManager::new();

        let err = manager
            .create_foreign_table(ForeignTable::new("ft", "nonexistent", make_schema("ft")))
            .unwrap_err();
        assert!(matches!(err, FdwError::ServerNotFound(_)));
    }

    #[test]
    fn test_manager_create_foreign_table_duplicate() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();
        manager
            .create_foreign_table(ForeignTable::new("ft", "srv", make_schema("ft")))
            .unwrap();

        let err = manager
            .create_foreign_table(ForeignTable::new("ft", "srv", make_schema("ft")))
            .unwrap_err();
        assert!(matches!(err, FdwError::TableAlreadyExists(_)));
    }

    #[test]
    fn test_manager_drop_foreign_table() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();
        manager
            .create_foreign_table(ForeignTable::new("ft", "srv", make_schema("ft")))
            .unwrap();

        manager.drop_foreign_table("ft", false).unwrap();
        assert!(manager.get_foreign_table("ft").is_none());
    }

    #[test]
    fn test_manager_drop_foreign_table_not_found() {
        let mut manager = FdwManager::new();
        let err = manager
            .drop_foreign_table("nonexistent", false)
            .unwrap_err();
        assert!(matches!(err, FdwError::TableNotFound(_)));
    }

    #[test]
    fn test_manager_drop_foreign_table_if_exists() {
        let mut manager = FdwManager::new();
        manager.drop_foreign_table("nonexistent", true).unwrap();
    }

    // =================================================================
    //  FdwManager — 数据操作测试
    // =================================================================

    fn setup_manager_with_data() -> (FdwManager, InMemoryFdw) {
        let fdw = make_fdw_with_data();
        let mut manager = FdwManager::new();

        // 克隆 fdw 的数据用于验证（但 handler 需要 Box）
        // 这里我们用 with_data 创建独立的 fdw 实例
        let fdw_for_manager = make_fdw_with_data();
        manager.register_handler("memory", Box::new(fdw_for_manager));
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();
        manager
            .create_foreign_table(
                ForeignTable::new("ft", "srv", make_schema("ft"))
                    .with_option("table_name", "remote_t"),
            )
            .unwrap();

        (manager, fdw)
    }

    #[test]
    fn test_manager_scan() {
        let (manager, _fdw) = setup_manager_with_data();

        let rows = manager.scan("ft", &ScanFilter::None).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0][0], Value::Int64(1));
    }

    #[test]
    fn test_manager_scan_with_filter() {
        let (manager, _fdw) = setup_manager_with_data();

        let rows = manager
            .scan("ft", &ScanFilter::Eq(0, Value::Int64(2)))
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], Value::Text("bob".to_string()));
    }

    #[test]
    fn test_manager_insert() {
        let (manager, _fdw) = setup_manager_with_data();

        let count = manager.insert("ft", &[make_row(6, "frank")]).unwrap();
        assert_eq!(count, 1);

        // 验证插入后行数
        let rows = manager.scan("ft", &ScanFilter::None).unwrap();
        assert_eq!(rows.len(), 6);
    }

    #[test]
    fn test_manager_update() {
        let (manager, _fdw) = setup_manager_with_data();

        let count = manager
            .update(
                "ft",
                &ScanFilter::Eq(0, Value::Int64(1)),
                &[(1, Value::Text("ALICE".to_string()))],
            )
            .unwrap();
        assert_eq!(count, 1);

        let rows = manager
            .scan("ft", &ScanFilter::Eq(0, Value::Int64(1)))
            .unwrap();
        assert_eq!(rows[0][1], Value::Text("ALICE".to_string()));
    }

    #[test]
    fn test_manager_delete() {
        let (manager, _fdw) = setup_manager_with_data();

        let count = manager
            .delete("ft", &ScanFilter::Eq(0, Value::Int64(5)))
            .unwrap();
        assert_eq!(count, 1);

        let rows = manager.scan("ft", &ScanFilter::None).unwrap();
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn test_manager_explain() {
        let (manager, _fdw) = setup_manager_with_data();

        let lines = manager.explain("ft").unwrap();
        assert!(!lines.is_empty());
        assert!(lines[0].contains("Foreign Scan"));
    }

    // =================================================================
    //  FdwManager — 错误场景测试
    // =================================================================

    #[test]
    fn test_manager_scan_table_not_found() {
        let manager = FdwManager::new();
        let err = manager.scan("nonexistent", &ScanFilter::None).unwrap_err();
        assert!(matches!(err, FdwError::TableNotFound(_)));
    }

    #[test]
    fn test_manager_scan_handler_not_registered() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("srv", "unregistered_type"))
            .unwrap();
        manager
            .create_foreign_table(ForeignTable::new("ft", "srv", make_schema("ft")))
            .unwrap();

        let err = manager.scan("ft", &ScanFilter::None).unwrap_err();
        assert!(matches!(err, FdwError::HandlerNotRegistered(_)));
    }

    #[test]
    fn test_manager_scan_server_dropped() {
        let mut manager = FdwManager::new();
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();
        manager
            .create_foreign_table(ForeignTable::new("ft", "srv", make_schema("ft")))
            .unwrap();

        // 直接从内部删除 server（模拟不一致状态）
        manager.servers.remove("srv");

        let err = manager.scan("ft", &ScanFilter::None).unwrap_err();
        assert!(matches!(err, FdwError::ServerNotFound(_)));
    }

    // =================================================================
    //  端到端 CRUD 流程测试
    // =================================================================

    #[test]
    fn test_e2e_crud_flow() {
        let fdw = InMemoryFdw::new();
        let mut manager = FdwManager::new();
        manager.register_handler("memory", Box::new(fdw));
        manager
            .create_server(
                ForeignServer::new("pg_srv", "memory")
                    .with_option("host", "remote.host")
                    .with_option("port", "5432"),
            )
            .unwrap();
        manager
            .create_user_mapping(
                UserMapping::new("admin", "pg_srv")
                    .with_option("user", "remote_admin")
                    .with_option("password", "secret"),
            )
            .unwrap();
        manager
            .create_foreign_table(
                ForeignTable::new("ft", "pg_srv", make_schema("ft"))
                    .with_option("schema_name", "public")
                    .with_option("table_name", "users"),
            )
            .unwrap();

        // INSERT
        let inserted = manager
            .insert(
                "ft",
                &[
                    make_row(1, "alice"),
                    make_row(2, "bob"),
                    make_row(3, "carol"),
                ],
            )
            .unwrap();
        assert_eq!(inserted, 3);

        // SELECT all
        let all = manager.scan("ft", &ScanFilter::None).unwrap();
        assert_eq!(all.len(), 3);

        // SELECT with filter
        let filtered = manager
            .scan("ft", &ScanFilter::Eq(0, Value::Int64(2)))
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0][1], Value::Text("bob".to_string()));

        // UPDATE
        let updated = manager
            .update(
                "ft",
                &ScanFilter::Eq(0, Value::Int64(2)),
                &[(1, Value::Text("BOB".to_string()))],
            )
            .unwrap();
        assert_eq!(updated, 1);

        // 验证更新
        let bob = manager
            .scan("ft", &ScanFilter::Eq(0, Value::Int64(2)))
            .unwrap();
        assert_eq!(bob[0][1], Value::Text("BOB".to_string()));

        // DELETE
        let deleted = manager
            .delete("ft", &ScanFilter::Eq(0, Value::Int64(1)))
            .unwrap();
        assert_eq!(deleted, 1);

        // 验证删除后的状态
        let remaining = manager.scan("ft", &ScanFilter::None).unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(!remaining.iter().any(|r| r[0] == Value::Int64(1)));
    }

    #[test]
    fn test_e2e_multiple_foreign_tables_same_server() {
        let fdw = InMemoryFdw::with_data_multi(vec![
            (
                "users".to_string(),
                vec![make_row(1, "alice"), make_row(2, "bob")],
            ),
            (
                "orders".to_string(),
                vec![make_row(101, "first"), make_row(102, "second")],
            ),
        ]);
        let mut manager = FdwManager::new();
        manager.register_handler("memory", Box::new(fdw));
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();

        // 两张外部表共享同一 server
        manager
            .create_foreign_table(
                ForeignTable::new("ft_users", "srv", make_schema("ft_users"))
                    .with_option("table_name", "users"),
            )
            .unwrap();
        manager
            .create_foreign_table(
                ForeignTable::new("ft_orders", "srv", make_schema("ft_orders"))
                    .with_option("table_name", "orders"),
            )
            .unwrap();

        // 分别扫描
        let users = manager.scan("ft_users", &ScanFilter::None).unwrap();
        assert_eq!(users.len(), 2);

        let orders = manager.scan("ft_orders", &ScanFilter::None).unwrap();
        assert_eq!(orders.len(), 2);

        assert_eq!(manager.list_foreign_tables().len(), 2);
    }

    // =================================================================
    //  FdwError 转换测试
    // =================================================================

    #[test]
    fn test_fdw_error_to_execution_error() {
        let err = FdwError::TableNotFound("ft".to_string());
        let exec_err: ExecutionError = err.into();
        match exec_err {
            ExecutionError::EvalError(msg) => {
                assert!(msg.contains("FDW error"));
                assert!(msg.contains("foreign table 'ft' does not exist"));
            }
            _ => panic!("expected EvalError"),
        }
    }

    // =================================================================
    //  Drop 依赖关系链测试
    // =================================================================

    #[test]
    fn test_drop_cascade_chain() {
        let mut manager = FdwManager::new();
        manager.register_handler("memory", Box::new(InMemoryFdw::new()));
        manager
            .create_server(ForeignServer::new("srv", "memory"))
            .unwrap();
        manager
            .create_user_mapping(UserMapping::new("user", "srv"))
            .unwrap();
        manager
            .create_foreign_table(ForeignTable::new("ft", "srv", make_schema("ft")))
            .unwrap();

        // 1. 删除外部表 — OK
        manager.drop_foreign_table("ft", false).unwrap();
        assert_eq!(manager.list_foreign_tables().len(), 0);

        // 2. 删除用户映射 — OK（外部表已删）
        manager.drop_user_mapping("user", "srv", false).unwrap();
        assert_eq!(manager.list_user_mappings().len(), 0);

        // 3. 删除服务器 — OK（无依赖）
        manager.drop_server("srv", false).unwrap();
        assert_eq!(manager.list_servers().len(), 0);
    }

    // =================================================================
    //  多 handler 测试
    // =================================================================

    #[test]
    fn test_multiple_handlers_dispatch() {
        let fdw1 = InMemoryFdw::with_data("table_a", vec![make_row(1, "a1")]);
        let fdw2 = InMemoryFdw::with_data("table_b", vec![make_row(2, "b1")]);

        let mut manager = FdwManager::new();
        manager.register_handler("type_a", Box::new(fdw1));
        manager.register_handler("type_b", Box::new(fdw2));

        manager
            .create_server(ForeignServer::new("srv_a", "type_a"))
            .unwrap();
        manager
            .create_server(ForeignServer::new("srv_b", "type_b"))
            .unwrap();

        manager
            .create_foreign_table(
                ForeignTable::new("ft_a", "srv_a", make_schema("ft_a"))
                    .with_option("table_name", "table_a"),
            )
            .unwrap();
        manager
            .create_foreign_table(
                ForeignTable::new("ft_b", "srv_b", make_schema("ft_b"))
                    .with_option("table_name", "table_b"),
            )
            .unwrap();

        // 验证分派到正确的 handler
        let rows_a = manager.scan("ft_a", &ScanFilter::None).unwrap();
        assert_eq!(rows_a[0][1], Value::Text("a1".to_string()));

        let rows_b = manager.scan("ft_b", &ScanFilter::None).unwrap();
        assert_eq!(rows_b[0][1], Value::Text("b1".to_string()));
    }
}
