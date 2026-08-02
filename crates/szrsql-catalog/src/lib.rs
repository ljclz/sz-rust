//! SzRSQL Catalog — 表/索引元数据管理 + 系统表（Phase 3.8）。
//!
//! # 设计
//!
//! - **`Catalog` trait**（复用 `szrsql_sql::plan::Catalog`）— 只读接口：`table_exists` / `get_table` / `list_tables`
//! - **`MutableCatalog` trait** — 扩展 `Catalog` 添加 DDL 操作：`create_table` / `drop_table` / `create_index` / `drop_index` / `list_indexes` / `list_indexes_for_table` / `get_index`
//! - **`ManagedCatalog`** — 内存实现，存储 `HashMap<String, TableSchema>` + `HashMap<String, IndexInfo>`，支持完整 DDL 语义（IF EXISTS / IF NOT EXISTS / CASCADE）
//! - **`system_tables` 模块** — PG 兼容的只读系统表视图（`pg_tables` / `pg_indexes` 子集）
//!
//! # 关键决策
//!
//! - **Catalog 与 Storage 解耦**：本 crate 只管理"元数据"（Schema + 索引定义），不持有表数据。
//!   实际表数据由 `szrsql-storage` + `szrsql-sql::executor::InMemoryTable` 管理。
//!   执行器通过 `Executor::register_table` 注册表数据，Catalog 仅提供 Schema 查询。
//! - **CASCADE 暂为占位**：`drop_table(cascade=true)` 仅删除表 Schema + 关联索引，
//!   不递归删除外键引用表（外键约束执行留待 Phase 5）。
//! - **系统表为函数式视图**：`pg_tables(catalog)` / `pg_indexes(catalog)` 返回 `Vec<SysRow>`，
//!   每次调用实时计算，不持久化。Phase 4 pgwire 集成时可直接作为 `SELECT * FROM pg_tables` 的数据源。
//!
//! 对应 `SzRSQL实施进度.md` Phase 3.8。

pub mod catalog_tree;
pub mod information_schema;
pub mod lineage;
pub mod multitenant;
pub mod navicat;
pub mod quota;
pub mod rbac;
pub mod rls;
pub mod semantic_tag;
pub mod system_tables;

use std::collections::HashMap;
use szrsql_sql::ast::{ColumnDefinition, IndexColumn, TableConstraint, TableName};
use szrsql_sql::plan::{Catalog, SequenceDefinition, TableSchema};
use thiserror::Error;

// =====================================================================
//  错误类型
// =====================================================================

/// Catalog 操作错误
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CatalogError {
    /// 表已存在（CREATE TABLE 未指定 IF NOT EXISTS 时）
    #[error("table already exists: {0}")]
    TableAlreadyExists(String),
    /// 表不存在（DROP TABLE 未指定 IF EXISTS 时）
    #[error("table not found: {0}")]
    TableNotFound(String),
    /// 索引已存在
    #[error("index already exists: {0}")]
    IndexAlreadyExists(String),
    /// 索引不存在
    #[error("index not found: {0}")]
    IndexNotFound(String),
    /// 无效参数
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// 依赖对象存在（L7：DROP TABLE cascade=false 时若有索引/视图依赖该表）
    #[error("cannot drop table {table}: other objects depend on it (use CASCADE): {dependent}")]
    DependencyExists { table: String, dependent: String },
}

// =====================================================================
//  索引元数据
// =====================================================================

/// 索引元数据 — 描述一个已创建的索引
#[derive(Debug, Clone, PartialEq)]
pub struct IndexInfo {
    /// 索引名（全局唯一）
    pub name: String,
    /// 所属表名
    pub table: TableName,
    /// 索引列（按声明顺序）
    pub columns: Vec<IndexColumn>,
    /// 是否为 UNIQUE 索引
    pub unique: bool,
}

impl IndexInfo {
    /// 创建普通索引（非 UNIQUE）
    pub fn new(name: impl Into<String>, table: TableName, columns: Vec<IndexColumn>) -> Self {
        Self {
            name: name.into(),
            table,
            columns,
            unique: false,
        }
    }

    /// 创建 UNIQUE 索引
    pub fn new_unique(
        name: impl Into<String>,
        table: TableName,
        columns: Vec<IndexColumn>,
    ) -> Self {
        Self {
            name: name.into(),
            table,
            columns,
            unique: true,
        }
    }

    /// 索引列名列表
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|c| c.column.as_str()).collect()
    }
}

// =====================================================================
//  MutableCatalog trait
// =====================================================================

/// 可变 Catalog — 扩展 `Catalog` 添加 DDL 操作
///
/// 实现方需保证：
/// - `create_table` 后 `table_exists` 返回 true，`get_table` 返回 Some
/// - `drop_table` 后 `table_exists` 返回 false，关联索引也被删除
/// - `create_index` 后 `get_index` 返回 Some，`list_indexes_for_table` 包含该索引
/// - 索引名全局唯一（跨表也不允许重名，与 PG 一致）
pub trait MutableCatalog: Catalog + Send {
    /// 创建表
    ///
    /// - `if_not_exists=true` 时若表已存在，静默返回 Ok(())（与 `CREATE TABLE IF NOT EXISTS` 语义一致）
    /// - `if_not_exists=false` 时若表已存在，返回 `CatalogError::TableAlreadyExists`
    fn create_table(
        &mut self,
        schema: TableSchema,
        if_not_exists: bool,
    ) -> Result<(), CatalogError>;

    /// 删除表
    ///
    /// - `if_exists=true` 时若表不存在，静默返回 Ok(())（与 `DROP TABLE IF EXISTS` 语义一致）
    /// - `if_exists=false` 时若表不存在，返回 `CatalogError::TableNotFound`
    /// - `cascade=true` 时同时删除该表的所有关联索引（当前实现总是如此，cascade 参数为未来外键级联保留）
    fn drop_table(
        &mut self,
        name: &TableName,
        if_exists: bool,
        cascade: bool,
    ) -> Result<(), CatalogError>;

    /// 创建索引
    ///
    /// - `if_not_exists=true` 时若索引已存在，静默返回 Ok(())
    /// - `if_not_exists=false` 时若索引已存在，返回 `CatalogError::IndexAlreadyExists`
    /// - 若所属表不存在，返回 `CatalogError::TableNotFound`
    fn create_index(&mut self, index: IndexInfo, if_not_exists: bool) -> Result<(), CatalogError>;

    /// 删除索引
    ///
    /// - `if_exists=true` 时若索引不存在，静默返回 Ok(())
    /// - `if_exists=false` 时若索引不存在，返回 `CatalogError::IndexNotFound`
    fn drop_index(&mut self, name: &str, if_exists: bool) -> Result<(), CatalogError>;

    /// 列出所有索引
    fn list_indexes(&self) -> Vec<IndexInfo>;

    /// 列出指定表的所有索引
    fn list_indexes_for_table(&self, table: &TableName) -> Vec<IndexInfo>;

    /// 按索引名查询
    fn get_index(&self, name: &str) -> Option<IndexInfo>;

    /// 替换表 Schema — Phase F-10
    ///
    /// 用于 `ALTER TABLE` 系列操作：执行器先 `get_table` 取得现有 Schema，
    /// 在克隆上修改（增删列、改类型、改约束、改默认值、改 NOT NULL 等），
    /// 再调用此方法整体替换。
    ///
    /// - 若表不存在，返回 `CatalogError::TableNotFound`
    /// - 表名必须与现有表名一致（不可用于 RENAME，RENAME 走 `rename_table`）
    /// - 不会影响数据行（数据迁移由执行器在 storage 层完成）
    /// - 不会影响关联索引（索引元数据保持不变；若 DROP COLUMN 删除了被索引引用的列，
    ///   执行器应先调用 `drop_index` 再调用此方法）
    fn replace_table_schema(&mut self, schema: TableSchema) -> Result<(), CatalogError>;

    /// 重命名表 — Phase F-10
    ///
    /// 用于 `ALTER TABLE ... RENAME TO new_name`。
    /// - 若旧表不存在，返回 `CatalogError::TableNotFound`
    /// - 若新表名已存在，返回 `CatalogError::TableAlreadyExists`
    /// - 同时更新关联索引的 `table` 字段
    fn rename_table(
        &mut self,
        old_name: &TableName,
        new_name: &TableName,
    ) -> Result<(), CatalogError>;

    /// 设置表注释 — Phase TDengine-P2
    ///
    /// `comment=None` 时删除已有注释。
    fn set_table_comment(
        &mut self,
        name: &TableName,
        comment: Option<String>,
    ) -> Result<(), CatalogError>;

    /// 设置列注释 — Phase TDengine-P2
    ///
    /// `comment=None` 时删除已有注释。
    fn set_column_comment(
        &mut self,
        table: &TableName,
        column: &str,
        comment: Option<String>,
    ) -> Result<(), CatalogError>;

    /// 获取表注释 — Phase TDengine-P2
    fn get_table_comment(&self, name: &TableName) -> Option<String>;

    /// 获取列注释 — Phase TDengine-P2
    fn get_column_comment(&self, table: &TableName, column: &str) -> Option<String>;
}

// =====================================================================
//  ManagedCatalog — 内存实现
// =====================================================================

/// 内存管理 Catalog — 存储 表 Schema + 索引元数据
///
/// 用于单元测试、示例、以及作为 szrsql-bin 的默认 Catalog。
/// 生产环境可替换为基于 szrsql-storage 的持久化实现。
#[derive(Debug, Default, Clone)]
pub struct ManagedCatalog {
    /// 表名（lowercase qualified）→ TableSchema
    tables: HashMap<String, TableSchema>,
    /// 索引名（lowercase）→ IndexInfo
    indexes: HashMap<String, IndexInfo>,
    /// 注释存储（key = "table_name" 或 "table_name.column_name"）— Phase TDengine-P2
    comments: HashMap<String, String>,
    /// 序列存储（lowercase qualified key → SequenceDefinition）— P0-PG-7 修复
    sequences: HashMap<String, SequenceDefinition>,
    /// 视图存储（lowercase qualified key → ViewDefinition）— 用于 pg_views 系统表
    views: HashMap<String, szrsql_sql::materialized_view::ViewDefinition>,
    /// 表约束存储（L8 修复：原 add_table_constraint 是占位，现真实持久化）
    /// key = table_key, value = 该表的所有约束列表
    constraints: HashMap<String, Vec<TableConstraint>>,
}

impl ManagedCatalog {
    /// 创建空 catalog
    pub fn new() -> Self {
        Self::default()
    }

    /// 表数量
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// 索引数量
    pub fn index_count(&self) -> usize {
        self.indexes.len()
    }

    /// 序列数量 — P0-PG-7 修复
    pub fn sequence_count(&self) -> usize {
        self.sequences.len()
    }

    /// 注册序列 — P0-PG-7 修复
    ///
    /// 用于测试和运行时序列元数据管理。同名覆盖。
    pub fn create_sequence(&mut self, def: SequenceDefinition) {
        let key = self.table_key(&def.name);
        self.sequences.insert(key, def);
    }

    /// 表名 → lowercase qualified key（大小写不敏感）
    ///
    /// 当 schema 为 None 时默认使用 "public"，确保：
    /// - `CREATE TABLE t (...)` 创建的表（schema=None）
    /// - `SELECT * FROM t` 查询的表（schema=None）
    /// - `SELECT * FROM "public"."t"` 查询的表（schema=Some("public")）
    ///
    /// 三者使用相同的 key，能互相匹配。
    fn table_key(&self, name: &TableName) -> String {
        match &name.schema {
            Some(s) => format!("{}.{}", s.to_lowercase(), name.name.to_lowercase()),
            None => format!("public.{}", name.name.to_lowercase()),
        }
    }

    /// 索引名 → lowercase key（大小写不敏感，与 PG 一致）
    fn index_key(&self, name: &str) -> String {
        name.to_lowercase()
    }
}

impl Catalog for ManagedCatalog {
    fn table_exists(&self, name: &TableName) -> bool {
        self.tables.contains_key(&self.table_key(name))
    }

    fn get_table(&self, name: &TableName) -> Option<TableSchema> {
        self.tables.get(&self.table_key(name)).cloned()
    }

    fn list_tables(&self) -> Vec<TableName> {
        self.tables.values().map(|t| t.name.clone()).collect()
    }

    /// 检查序列是否存在 — P0-PG-7 修复
    fn sequence_exists(&self, name: &TableName) -> bool {
        self.sequences.contains_key(&self.table_key(name))
    }

    /// 获取序列定义 — P0-PG-7 修复
    fn get_sequence(&self, name: &TableName) -> Option<SequenceDefinition> {
        self.sequences.get(&self.table_key(name)).cloned()
    }

    /// 列出所有序列 — P0-PG-7 修复
    fn list_sequences(&self) -> Vec<TableName> {
        self.sequences.values().map(|s| s.name.clone()).collect()
    }

    /// 列出所有视图名 — 用于 pg_views 系统表
    fn list_views(&self) -> Vec<TableName> {
        self.views.values().map(|v| v.name.clone()).collect()
    }

    /// 获取视图定义 — 用于 pg_views 系统表
    fn get_view(&self, name: &TableName) -> Option<szrsql_sql::materialized_view::ViewDefinition> {
        self.views.get(&self.table_key(name)).cloned()
    }
}

impl MutableCatalog for ManagedCatalog {
    fn create_table(
        &mut self,
        schema: TableSchema,
        if_not_exists: bool,
    ) -> Result<(), CatalogError> {
        let key = self.table_key(&schema.name);
        if self.tables.contains_key(&key) {
            if if_not_exists {
                return Ok(());
            }
            return Err(CatalogError::TableAlreadyExists(
                schema.name.qualified_name(),
            ));
        }
        self.tables.insert(key, schema);
        Ok(())
    }

    fn drop_table(
        &mut self,
        name: &TableName,
        if_exists: bool,
        cascade: bool,
    ) -> Result<(), CatalogError> {
        let key = self.table_key(name);
        if !self.tables.contains_key(&key) {
            if if_exists {
                return Ok(());
            }
            return Err(CatalogError::TableNotFound(name.qualified_name()));
        }

        // L7 修复：原实现 cascade 参数被完全忽略，无论取值都删除关联索引。
        // 正确语义（与 PostgreSQL 一致）：
        // - cascade=false：若有依赖索引则拒绝删除，返回 DependencyExists 错误
        // - cascade=true：级联删除所有依赖对象（索引 + 视图）
        //
        // 注：视图级联需要从 ViewDefinition.query 提取表引用，复杂度高，
        // 当前仅检测索引依赖；视图级联留作后续 P8 阶段扩展。
        let dependent_indexes: Vec<String> = self
            .indexes
            .values()
            .filter(|idx| self.table_key(&idx.table) == key)
            .map(|idx| idx.name.clone())
            .collect();

        if !cascade && !dependent_indexes.is_empty() {
            let dep = dependent_indexes.first().cloned().unwrap_or_default();
            return Err(CatalogError::DependencyExists {
                table: name.qualified_name(),
                dependent: dep,
            });
        }

        // cascade=true 或无依赖：执行删除
        self.tables.remove(&key);
        // 删除关联索引（索引不能独立于表存在）
        for idx_name in &dependent_indexes {
            self.indexes.remove(&self.index_key(idx_name));
        }
        // L8：同时删除该表的所有约束
        self.constraints.remove(&key);
        Ok(())
    }

    fn create_index(&mut self, index: IndexInfo, if_not_exists: bool) -> Result<(), CatalogError> {
        // 检查所属表存在
        if !self.table_exists(&index.table) {
            return Err(CatalogError::TableNotFound(index.table.qualified_name()));
        }
        let key = self.index_key(&index.name);
        if self.indexes.contains_key(&key) {
            if if_not_exists {
                return Ok(());
            }
            return Err(CatalogError::IndexAlreadyExists(index.name));
        }
        self.indexes.insert(key, index);
        Ok(())
    }

    fn drop_index(&mut self, name: &str, if_exists: bool) -> Result<(), CatalogError> {
        let key = self.index_key(name);
        if !self.indexes.contains_key(&key) {
            if if_exists {
                return Ok(());
            }
            return Err(CatalogError::IndexNotFound(name.to_string()));
        }
        self.indexes.remove(&key);
        Ok(())
    }

    fn list_indexes(&self) -> Vec<IndexInfo> {
        self.indexes.values().cloned().collect()
    }

    fn list_indexes_for_table(&self, table: &TableName) -> Vec<IndexInfo> {
        let table_key = self.table_key(table);
        self.indexes
            .values()
            .filter(|idx| self.table_key(&idx.table) == table_key)
            .cloned()
            .collect()
    }

    fn get_index(&self, name: &str) -> Option<IndexInfo> {
        self.indexes.get(&self.index_key(name)).cloned()
    }

    /// 替换表 Schema — Phase F-10
    ///
    /// 行为：
    /// - 表不存在 → `CatalogError::TableNotFound`
    /// - 表存在 → 用新 Schema 整体替换（保留索引元数据）
    fn replace_table_schema(&mut self, schema: TableSchema) -> Result<(), CatalogError> {
        let key = self.table_key(&schema.name);
        if !self.tables.contains_key(&key) {
            return Err(CatalogError::TableNotFound(schema.name.qualified_name()));
        }
        self.tables.insert(key, schema);
        Ok(())
    }

    /// 重命名表 — Phase F-10
    ///
    /// 行为：
    /// - 旧表不存在 → `CatalogError::TableNotFound`
    /// - 新表名已存在 → `CatalogError::TableAlreadyExists`
    /// - 同时更新关联索引的 `table` 字段，保持索引可用
    fn rename_table(
        &mut self,
        old_name: &TableName,
        new_name: &TableName,
    ) -> Result<(), CatalogError> {
        let old_key = self.table_key(old_name);
        let new_key = self.table_key(new_name);

        if !self.tables.contains_key(&old_key) {
            return Err(CatalogError::TableNotFound(old_name.qualified_name()));
        }
        if self.tables.contains_key(&new_key) {
            return Err(CatalogError::TableAlreadyExists(new_name.qualified_name()));
        }

        // 移除旧 schema，修改表名后插入新 key
        let mut schema = self.tables.remove(&old_key).expect("checked above");
        schema.name = new_name.clone();
        self.tables.insert(new_key, schema);

        // 更新关联索引的 table 字段
        // 注意：old_table_key 已在循环外计算，避免与 self.indexes.values_mut() 借用冲突
        let old_table_key = old_key.clone();
        for idx in self.indexes.values_mut() {
            // 直接用 lowercase qualified name 比较（与 table_key 逻辑一致）
            if idx.table.qualified_name().to_lowercase() == old_table_key {
                idx.table = new_name.clone();
            }
        }
        Ok(())
    }

    // Phase TDengine-P2: COMMENT ON 存储实现

    fn set_table_comment(
        &mut self,
        name: &TableName,
        comment: Option<String>,
    ) -> Result<(), CatalogError> {
        let key = self.table_key(name);
        match comment {
            Some(c) => {
                self.comments.insert(key, c);
            }
            None => {
                self.comments.remove(&key);
            }
        }
        Ok(())
    }

    fn set_column_comment(
        &mut self,
        table: &TableName,
        column: &str,
        comment: Option<String>,
    ) -> Result<(), CatalogError> {
        let key = format!("{}.{}", self.table_key(table), column.to_lowercase());
        match comment {
            Some(c) => {
                self.comments.insert(key, c);
            }
            None => {
                self.comments.remove(&key);
            }
        }
        Ok(())
    }

    fn get_table_comment(&self, name: &TableName) -> Option<String> {
        self.comments.get(&self.table_key(name)).cloned()
    }

    fn get_column_comment(&self, table: &TableName, column: &str) -> Option<String> {
        let key = format!("{}.{}", self.table_key(table), column.to_lowercase());
        self.comments.get(&key).cloned()
    }
}

// =====================================================================
//  便捷构造方法
// =====================================================================

impl ManagedCatalog {
    /// 添加表（简化方式：表名 + 列定义），用于测试
    pub fn add_simple_table(
        &mut self,
        name: &str,
        columns: Vec<(&str, szrsql_types::value::ColumnType)>,
    ) {
        let table_name = TableName::new(name);
        let cols: Vec<ColumnDefinition> = columns
            .into_iter()
            .map(|(n, t)| ColumnDefinition::new(n, t))
            .collect();
        let schema = TableSchema {
            name: table_name,
            columns: cols,
        };
        // 测试便捷方法，忽略 "已存在" 错误
        let _ = self.create_table(schema, true);
    }

    /// 添加视图定义 — 用于测试 pg_views 系统表
    ///
    /// 若同名视图已存在，直接替换（简化测试场景）。
    pub fn add_view(&mut self, view: szrsql_sql::materialized_view::ViewDefinition) {
        let key = self.table_key(&view.name);
        self.views.insert(key, view);
    }

    /// 添加表级约束（L8 修复：原方法是占位 — 仅校验表存在不持久化约束）
    ///
    /// 现在真实持久化到 `constraints: HashMap<String, Vec<TableConstraint>>`，
    /// 可通过 `list_table_constraints` 查询。
    ///
    /// 重复添加同名约束返回 `ConstraintAlreadyExists` 错误（与 PG 行为一致）。
    pub fn add_table_constraint(
        &mut self,
        table: &TableName,
        constraint: TableConstraint,
    ) -> Result<(), CatalogError> {
        if !self.table_exists(table) {
            return Err(CatalogError::TableNotFound(table.qualified_name()));
        }
        let key = self.table_key(table);
        let constraints = self.constraints.entry(key).or_default();
        // 检查重名约束（Primary/Unique/Foreign/Check 各类约束名唯一）
        if let Some(name) = constraint.name() {
            if constraints.iter().any(|c| c.name() == Some(name)) {
                return Err(CatalogError::InvalidArgument(format!(
                    "constraint \"{}\" already exists for table \"{}\"",
                    name,
                    table.qualified_name()
                )));
            }
        }
        constraints.push(constraint);
        Ok(())
    }

    /// 列出表的所有约束（L8 新增：配合 add_table_constraint 持久化）
    pub fn list_table_constraints(&self, table: &TableName) -> Vec<TableConstraint> {
        let key = self.table_key(table);
        self.constraints.get(&key).cloned().unwrap_or_default()
    }

    /// 删除表的所有约束（L8 新增：DROP TABLE cascade 时调用）
    pub fn drop_constraints_for_table(&mut self, table: &TableName) {
        let key = self.table_key(table);
        self.constraints.remove(&key);
    }
}

#[cfg(test)]
mod catalog_tests;

#[cfg(test)]
mod information_schema_tests;

#[cfg(test)]
mod multitenant_tests;

#[cfg(test)]
mod navicat_tests;

#[cfg(test)]
mod quota_tests;

#[cfg(test)]
mod rbac_tests;

#[cfg(test)]
mod rls_tests;

#[cfg(test)]
mod system_tables_tests;
