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

pub mod information_schema;
pub mod multitenant;
pub mod navicat;
pub mod quota;
pub mod rbac;
pub mod rls;
pub mod system_tables;

use std::collections::HashMap;
use szrsql_sql::ast::{ColumnDefinition, IndexColumn, TableConstraint, TableName};
use szrsql_sql::plan::{Catalog, TableSchema};
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
pub trait MutableCatalog: Catalog {
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

    /// 表名 → lowercase qualified key（大小写不敏感）
    fn table_key(&self, name: &TableName) -> String {
        name.qualified_name().to_lowercase()
    }

    /// 索引名 → lowercase key（大小写不敏感，与 PG 一致）
    fn index_key(&self, name: &str) -> String {
        name.to_lowercase()
    }

    /// 删除指定表的所有关联索引（内部辅助）
    fn drop_indexes_for_table(&mut self, table: &TableName) -> usize {
        let table_key = self.table_key(table);
        let to_remove: Vec<String> = self
            .indexes
            .iter()
            .filter(|(_, idx)| self.table_key(&idx.table) == table_key)
            .map(|(k, _)| k.clone())
            .collect();
        let removed = to_remove.len();
        for k in to_remove {
            self.indexes.remove(&k);
        }
        removed
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
        self.tables.remove(&key);
        // 当前实现总是删除关联索引；cascade 参数为未来外键级联保留
        if cascade {
            self.drop_indexes_for_table(name);
        } else {
            // 即使 cascade=false，也删除关联索引（索引不能独立于表存在）
            self.drop_indexes_for_table(name);
        }
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

    /// 添加表级约束（占位：当前 ManagedCatalog 不持久化约束，仅记录 Schema）
    ///
    /// 未来扩展时可在此处添加 `constraints: HashMap<String, Vec<TableConstraint>>` 字段。
    #[allow(unused_variables)]
    pub fn add_table_constraint(
        &mut self,
        table: &TableName,
        constraint: TableConstraint,
    ) -> Result<(), CatalogError> {
        // 当前实现：约束校验留待执行器层处理，Catalog 仅记录 Schema
        // 此方法为 API 预留，供未来扩展
        if !self.table_exists(table) {
            return Err(CatalogError::TableNotFound(table.qualified_name()));
        }
        Ok(())
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
