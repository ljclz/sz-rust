// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Schema 缓存模块 — 数据表字段元数据缓存（对齐 PHP `think\db\Fetch::getFields`）
//!
//! 本模块提供数据表字段元数据（schema）的缓存能力，对齐 PHP ThinkPHP
//! `think\db\Fetch` 的字段缓存机制。在 PHP 端，每次查询表字段信息都需要
//! 执行 `SHOW COLUMNS FROM <table>`（MySQL）或等价 SQL，为避免重复查询，
//! ThinkPHP 将字段元数据缓存到 Cache 中（默认永不过期）。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Fetch::getFields($table)` | [`SchemaCache::get_schema`] | 从缓存读取字段元数据 |
//! | `Fetch::setFieldCache($table, $data)` | [`SchemaCache::set_schema`] | 写入字段缓存 |
//! | `Fetch::getFieldCacheKey($table)` | [`SchemaCache::cache_key`] | 构造缓存 key |
//! | `Cache::delete($key)` | [`SchemaCache::forget_schema`] | 清除单表字段缓存 |
//! | `Cache::clear()` | [`SchemaCache::clear`] | 清除所有字段缓存 |
//! | `Cache::has($key)` | [`SchemaCache::has_schema`] | 判断字段缓存是否存在 |
//! | `Fetch::getFields` 内部回源 | [`SchemaCache::remember_schema`] | 缓存未命中时回源加载 |
//!
//! ### PHP `getFields` 缓存行为
//!
//! PHP `think\db\Fetch::getFields` 核心逻辑：
//!
//! ```php
//! protected function getFields(string $tableName): array
//! {
//!     // 1. 从缓存读取
//!     if ($this->config['fields_cache']) {
//!         $guid = $tableName . $this->connection->getConfig('fields_cache_flag');
//!         $content = $this->connection->getCacheHandler()->get($guid);
//!         if ($content) {
//!             return $content;  // 缓存命中
//!         }
//!     }
//!
//!     // 2. 缓存未命中，查询数据库
//!     $fields = $this->connection->getFields($tableName);
//!
//!     // 3. 写入缓存（永不过期）
//!     if ($this->config['fields_cache']) {
//!         $guid = $tableName . $this->connection->getConfig('fields_cache_flag');
//!         $this->connection->getCacheHandler()->set($guid, $fields);
//!     }
//!
//!     return $fields;
//! }
//! ```
//!
//! **关键行为对齐**：
//! - 默认 TTL = None（永不过期），对齐 PHP `$expire = null`
//! - 缓存 key 格式：`schema_cache:<table_name>`（PHP 端为 `db_<flag>_<table_name>`）
//! - 缓存未命中时通过 loader 回源加载（对齐 PHP `getFields` 内部查询）
//!
//! ### PHP 字段元数据结构
//!
//! PHP `getFields` 返回的字段元数据结构：
//!
//! ```php
//! [
//!     'id' => [
//!         'name'      => 'id',
//!         'type'      => 'int(11) unsigned',
//!         'notnull'   => true,
//!         'default'   => null,
//!         'primary'   => true,
//!         'autoinc'   => true,
//!     ],
//!     'name' => [
//!         'name'      => 'name',
//!         'type'      => 'varchar(255)',
//!         'notnull'   => false,
//!         'default'   => null,
//!         'primary'   => false,
//!         'autoinc'   => false,
//!     ],
//! ]
//! ```
//!
//! Rust 端通过 [`ColumnDefinition`] 提供等价结构，并扩展 `unsigned`、`comment` 字段。
//!
//! ## 架构说明
//!
//! - **基于 `Cache` facade**：复用 [`crate::cache::Cache`] 的驱动管理、序列化、标签系统，
//!   不重新实现底层存储
//! - **标签批量清除**：所有字段缓存 key 注册到标签（tag），[`SchemaCache::clear`]
//!   通过标签一次性清除所有表字段缓存，对齐 PHP `Cache::clear()` 的批量语义
//! - **无锁设计**：`SchemaCache` 自身状态在构造后不可变（`key_prefix`、`tag_name`），
//!   所有并发安全由底层 `Cache` 保证（`RwLock` + `parking_lot`）
//! - **可配置前缀**：通过 [`SchemaCache::with_prefix`] 可自定义缓存 key 前缀，
//!   支持多实例隔离（如不同数据库连接使用不同前缀）
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_core::cache::{Cache, MemoryCacheDriver};
//! use sz_rust_core::schema_cache::{SchemaCache, TableSchema, ColumnDefinition};
//! use std::sync::Arc;
//!
//! // 创建 Cache facade
//! let cache = Arc::new(Cache::new());
//! cache.register_default(MemoryCacheDriver::new());
//!
//! // 创建 SchemaCache
//! let schema_cache = SchemaCache::new(cache.clone());
//!
//! // 构造表字段元数据
//! let schema = TableSchema::new("users", vec![
//!     ColumnDefinition::new("id", "int(11) unsigned")
//!         .nullable(false)
//!         .primary_key(true)
//!         .auto_increment(true),
//!     ColumnDefinition::new("name", "varchar(255)")
//!         .nullable(false),
//! ]);
//!
//! // 写入缓存（永不过期）
//! schema_cache.set_schema("users", &schema, None).unwrap();
//!
//! // 从缓存读取
//! let cached = schema_cache.get_schema("users").unwrap().unwrap();
//! assert_eq!(cached.columns.len(), 2);
//!
//! // remember_schema：缓存未命中时自动加载
//! let schema = schema_cache.remember_schema("orders", |_table| {
//!     Ok(TableSchema::new("orders", vec![
//!         ColumnDefinition::new("id", "bigint(20)")
//!             .primary_key(true)
//!             .auto_increment(true),
//!     ]))
//! }).unwrap();
//! ```

use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::cache::Cache;

// ============================================================================
// 常量
// ============================================================================

/// 默认缓存 key 前缀（对齐 PHP `fields_cache_flag` 默认值）
const DEFAULT_KEY_PREFIX: &str = "schema_cache";

// ============================================================================
// 错误类型
// ============================================================================

/// Schema 缓存错误
///
/// 对齐 PHP `think\db\Fetch` 字段缓存操作中可能产生的错误。
#[derive(Debug, Error)]
pub enum SchemaCacheError {
    /// 底层缓存操作失败（读/写/删除/清除等）
    #[error("缓存操作失败: {0}")]
    CacheError(#[from] sz_orm_core::CacheError),
    /// Schema 加载器执行失败（`remember_schema` 中 loader 返回的错误）
    #[error("Schema 加载失败: {0}")]
    LoaderError(String),
    /// 序列化/反序列化失败（TableSchema 与缓存字节之间的转换错误）
    #[error("Schema 序列化失败: {0}")]
    Serialize(String),
}

// ============================================================================
// ColumnDefinition — 单个字段定义
// ============================================================================

/// 单个字段定义（对齐 PHP `think\db\Fetch::getFields` 返回的单字段元数据）
///
/// 存储数据表单个字段的完整元数据，包括字段名、类型、约束、默认值等。
///
/// # PHP 对齐
///
/// PHP `getFields` 返回字段数组，每个元素包含：
/// - `name`：字段名
/// - `type`：字段类型（如 `int(11) unsigned`）
/// - `notnull`：是否 NOT NULL（注意 PHP 是"是否非空"，Rust 用 `nullable` 取反）
/// - `default`：默认值
/// - `primary`：是否主键
/// - `autoinc`：是否自增
///
/// Rust 端额外扩展 `unsigned`（无符号）和 `comment`（字段注释）字段。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ColumnDefinition {
    /// 字段名
    pub name: String,
    /// 字段数据类型（如 `int(11)`, `varchar(255)`, `text`, `decimal(10,2)`）
    pub data_type: String,
    /// 是否允许 NULL（对齐 PHP `notnull` 取反：`nullable = !notnull`）
    pub nullable: bool,
    /// 是否为主键（对齐 PHP `primary`）
    pub primary_key: bool,
    /// 是否自增（对齐 PHP `autoinc`）
    pub auto_increment: bool,
    /// 是否无符号（数值类型扩展字段，PHP 端从 `type` 字符串解析）
    pub unsigned: bool,
    /// 默认值（`None` 表示无 DEFAULT 子句，`Some(value)` 表示有默认值）
    pub default: Option<String>,
    /// 字段注释（从 `COMMENT` 子句获取，PHP 端通常不缓存）
    pub comment: Option<String>,
}

impl ColumnDefinition {
    /// 创建新的字段定义
    ///
    /// 默认值：可空、非主键、非自增、非无符号、无默认值、无注释。
    ///
    /// # 参数
    ///
    /// - `name`: 字段名
    /// - `data_type`: 字段数据类型
    pub fn new(name: impl Into<String>, data_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable: true,
            primary_key: false,
            auto_increment: false,
            unsigned: false,
            default: None,
            comment: None,
        }
    }

    /// 设置是否允许 NULL（Builder 模式）
    pub fn nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }

    /// 设置是否为主键（Builder 模式）
    pub fn primary_key(mut self, primary_key: bool) -> Self {
        self.primary_key = primary_key;
        self
    }

    /// 设置是否自增（Builder 模式）
    pub fn auto_increment(mut self, auto_increment: bool) -> Self {
        self.auto_increment = auto_increment;
        self
    }

    /// 设置是否无符号（Builder 模式）
    pub fn unsigned(mut self, unsigned: bool) -> Self {
        self.unsigned = unsigned;
        self
    }

    /// 设置默认值（Builder 模式）
    pub fn default_value(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }

    /// 设置字段注释（Builder 模式）
    pub fn comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }
}

// ============================================================================
// TableSchema — 数据表字段元数据
// ============================================================================

/// 数据表字段元数据（对齐 PHP `think\db\Fetch::getFields` 返回的完整字段列表）
///
/// 存储一张表所有字段的元数据，以及主键信息和缓存时间戳。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TableSchema {
    /// 表名
    pub table_name: String,
    /// 字段列表（按表定义顺序）
    pub columns: Vec<ColumnDefinition>,
    /// 主键字段名列表（从 `columns` 中 `primary_key == true` 的字段自动提取）
    pub primary_keys: Vec<String>,
    /// 缓存写入时间戳（Unix 秒，用于判断缓存新鲜度）
    pub cached_at: i64,
}

impl TableSchema {
    /// 创建新的表字段元数据
    ///
    /// 自动从 `columns` 中提取主键字段名列表。
    ///
    /// # 参数
    ///
    /// - `table_name`: 表名
    /// - `columns`: 字段定义列表
    pub fn new(table_name: impl Into<String>, columns: Vec<ColumnDefinition>) -> Self {
        let table_name = table_name.into();
        let primary_keys: Vec<String> = columns
            .iter()
            .filter(|col| col.primary_key)
            .map(|col| col.name.clone())
            .collect();
        Self {
            table_name,
            columns,
            primary_keys,
            cached_at: chrono::Utc::now().timestamp(),
        }
    }

    /// 按字段名查找字段定义
    ///
    /// # 参数
    ///
    /// - `name`: 字段名
    ///
    /// # 返回
    ///
    /// 找到返回 `Some(&ColumnDefinition)`，未找到返回 `None`
    pub fn column(&self, name: &str) -> Option<&ColumnDefinition> {
        self.columns.iter().find(|col| col.name == name)
    }

    /// 判断是否存在指定字段
    ///
    /// # 参数
    ///
    /// - `name`: 字段名
    pub fn has_column(&self, name: &str) -> bool {
        self.column(name).is_some()
    }

    /// 获取所有字段名列表
    pub fn column_names(&self) -> Vec<&str> {
        self.columns.iter().map(|col| col.name.as_str()).collect()
    }
}

// ============================================================================
// SchemaCache — 数据表字段缓存
// ============================================================================

/// 数据表字段缓存（对齐 PHP `think\db\Fetch` 字段缓存机制）
///
/// 基于 [`Cache`] facade 实现表字段元数据的缓存，支持：
/// - 按 key 读写单表字段缓存
/// - `remember_schema` 缓存未命中时自动回源加载
/// - 按表名清除单表缓存
/// - 按标签批量清除所有表字段缓存
///
/// # 线程安全
///
/// `SchemaCache` 自身状态在构造后不可变（`key_prefix`、`tag_name`），
/// 所有并发安全由底层 [`Cache`] 保证。`SchemaCache` 是 `Send + Sync`。
///
/// # 缓存 key 格式
///
/// 默认格式：`schema_cache:<table_name>`
///
/// 可通过 [`SchemaCache::with_prefix`] 自定义前缀。
///
/// # 标签批量清除
///
/// 所有通过 [`SchemaCache::set_schema`] 写入的缓存 key 自动注册到标签
/// （标签名 = key 前缀），[`SchemaCache::clear`] 通过标签一次性清除所有
/// 表字段缓存，不影响 Cache 中的其他缓存。
pub struct SchemaCache {
    /// 底层 Cache facade 实例
    cache: Arc<Cache>,
    /// 缓存 key 前缀（默认 `schema_cache`）
    key_prefix: String,
    /// 标签名（用于批量清除，默认等于 `key_prefix`）
    tag_name: String,
}

impl SchemaCache {
    /// 创建 SchemaCache（使用默认前缀 `schema_cache`）
    ///
    /// # 参数
    ///
    /// - `cache`: Cache facade 实例（`Arc<Cache>`）
    pub fn new(cache: Arc<Cache>) -> Self {
        Self {
            cache,
            key_prefix: DEFAULT_KEY_PREFIX.to_string(),
            tag_name: DEFAULT_KEY_PREFIX.to_string(),
        }
    }

    /// 创建 SchemaCache（使用自定义缓存 key 前缀）
    ///
    /// 用于多实例隔离场景，如不同数据库连接使用不同前缀：
    /// `schema_cache_db1:users`、`schema_cache_db2:users`。
    ///
    /// # 参数
    ///
    /// - `cache`: Cache facade 实例
    /// - `prefix`: 缓存 key 前缀（同时用作标签名）
    pub fn with_prefix(cache: Arc<Cache>, prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        Self {
            cache,
            tag_name: prefix.clone(),
            key_prefix: prefix,
        }
    }

    /// 构造完整缓存 key（`{prefix}:{table_name}`）
    ///
    /// 对齐 PHP `Fetch::getFieldCacheKey($tableName)`。
    ///
    /// # 参数
    ///
    /// - `table_name`: 表名
    pub fn cache_key(&self, table_name: &str) -> String {
        format!("{}:{}", self.key_prefix, table_name)
    }

    /// 从缓存获取表字段元数据（对齐 PHP `Fetch::getFields` 缓存读取）
    ///
    /// # 参数
    ///
    /// - `table_name`: 表名
    ///
    /// # 返回
    ///
    /// - `Ok(Some(schema))`: 缓存命中
    /// - `Ok(None)`: 缓存未命中或已过期
    pub fn get_schema(&self, table_name: &str) -> Result<Option<TableSchema>, SchemaCacheError> {
        let key = self.cache_key(table_name);
        let result = self.cache.get::<TableSchema>(&key)?;
        Ok(result)
    }

    /// 缓存未命中时调用 loader 加载并缓存（对齐 PHP `Fetch::getFields` 回源逻辑）
    ///
    /// 1. 先从缓存读取，命中则直接返回
    /// 2. 未命中时调用 `loader(table_name)` 加载字段元数据
    /// 3. 将 loader 返回的 schema 写入缓存（TTL = None，永不过期）
    /// 4. 返回 schema
    ///
    /// # 参数
    ///
    /// - `table_name`: 表名
    /// - `loader`: 回源加载闭包，接收表名，返回 `Result<TableSchema, SchemaCacheError>`
    pub fn remember_schema<F>(
        &self,
        table_name: &str,
        loader: F,
    ) -> Result<TableSchema, SchemaCacheError>
    where
        F: FnOnce(&str) -> Result<TableSchema, SchemaCacheError>,
    {
        // 1. 先从缓存读取（对齐 PHP: $content = $this->connection->getCacheHandler()->get($guid)）
        if let Some(schema) = self.get_schema(table_name)? {
            return Ok(schema);
        }
        // 2. 缓存未命中，调用 loader 加载（对齐 PHP: $fields = $this->connection->getFields($tableName)）
        let schema = loader(table_name)?;
        // 3. 写入缓存（默认永不过期，对齐 PHP: $this->connection->getCacheHandler()->set($guid, $fields)）
        self.set_schema(table_name, &schema, None)?;
        // 4. 返回 schema
        Ok(schema)
    }

    /// 写入表字段缓存（对齐 PHP `Fetch::setFieldCache`）
    ///
    /// 缓存 key 自动注册到标签，支持 [`SchemaCache::clear`] 批量清除。
    ///
    /// # 参数
    ///
    /// - `table_name`: 表名
    /// - `schema`: 表字段元数据
    /// - `ttl`: 过期时间（`None` 永不过期，对齐 PHP 默认行为）
    pub fn set_schema(
        &self,
        table_name: &str,
        schema: &TableSchema,
        ttl: Option<Duration>,
    ) -> Result<(), SchemaCacheError> {
        let key = self.cache_key(table_name);
        // 使用 tag().set() 写入缓存，同时将 key 注册到标签（对齐 PHP TagSet::set）
        self.cache.tag(&self.tag_name).set(&key, schema, ttl)?;
        Ok(())
    }

    /// 清除单表字段缓存（对齐 PHP `Cache::delete($key)`）
    ///
    /// # 参数
    ///
    /// - `table_name`: 表名
    pub fn forget_schema(&self, table_name: &str) -> Result<(), SchemaCacheError> {
        let key = self.cache_key(table_name);
        self.cache.delete(&key)?;
        Ok(())
    }

    /// 清除所有表字段缓存（对齐 PHP `Cache::clear()` 批量语义）
    ///
    /// 通过标签批量清除所有 `set_schema` 写入的缓存 key，不影响 Cache 中的
    /// 其他缓存（如业务缓存、Session 等）。
    pub fn clear(&self) -> Result<(), SchemaCacheError> {
        self.cache.tag(&self.tag_name).clear()?;
        Ok(())
    }

    /// 判断表字段缓存是否存在（对齐 PHP `Cache::has($key)`）
    ///
    /// # 参数
    ///
    /// - `table_name`: 表名
    pub fn has_schema(&self, table_name: &str) -> Result<bool, SchemaCacheError> {
        let key = self.cache_key(table_name);
        Ok(self.cache.has(&key)?)
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{Cache, MemoryCacheDriver};
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 创建带默认驱动的测试用 Cache（Arc 包装）
    fn make_cache() -> Arc<Cache> {
        let cache = Arc::new(Cache::new());
        cache.register_default(MemoryCacheDriver::new());
        cache
    }

    /// 创建测试用 ColumnDefinition 列表（模拟 users 表）
    fn make_columns() -> Vec<ColumnDefinition> {
        vec![
            ColumnDefinition::new("id", "int(11) unsigned")
                .nullable(false)
                .primary_key(true)
                .auto_increment(true)
                .unsigned(true),
            ColumnDefinition::new("name", "varchar(255)")
                .nullable(false)
                .default_value(""),
            ColumnDefinition::new("email", "varchar(255)")
                .nullable(true)
                .comment("用户邮箱"),
        ]
    }

    // ------------------------------------------------------------------------
    // ColumnDefinition / TableSchema 结构测试
    // ------------------------------------------------------------------------

    /// 测试 ColumnDefinition builder 模式
    #[test]
    fn test_column_definition_builder() {
        let col = ColumnDefinition::new("id", "int(11) unsigned")
            .nullable(false)
            .primary_key(true)
            .auto_increment(true)
            .unsigned(true)
            .default_value("0")
            .comment("主键");

        assert_eq!(col.name, "id");
        assert_eq!(col.data_type, "int(11) unsigned");
        assert!(!col.nullable);
        assert!(col.primary_key);
        assert!(col.auto_increment);
        assert!(col.unsigned);
        assert_eq!(col.default, Some("0".to_string()));
        assert_eq!(col.comment, Some("主键".to_string()));
    }

    /// 测试 ColumnDefinition 默认值
    #[test]
    fn test_column_definition_defaults() {
        let col = ColumnDefinition::new("name", "varchar(255)");
        assert_eq!(col.name, "name");
        assert!(col.nullable);
        assert!(!col.primary_key);
        assert!(!col.auto_increment);
        assert!(!col.unsigned);
        assert_eq!(col.default, None);
        assert_eq!(col.comment, None);
    }

    /// 测试 TableSchema 自动提取主键
    #[test]
    fn test_table_schema_primary_keys() {
        let schema = TableSchema::new("users", make_columns());
        assert_eq!(schema.table_name, "users");
        assert_eq!(schema.columns.len(), 3);
        assert_eq!(schema.primary_keys, vec!["id"]);
    }

    /// 测试 TableSchema 查找字段
    #[test]
    fn test_table_schema_column_lookup() {
        let schema = TableSchema::new("users", make_columns());

        assert!(schema.has_column("id"));
        assert!(schema.has_column("name"));
        assert!(schema.has_column("email"));
        assert!(!schema.has_column("nonexistent"));

        let col = schema.column("email").unwrap();
        assert_eq!(col.data_type, "varchar(255)");
        assert_eq!(col.comment, Some("用户邮箱".to_string()));
    }

    /// 测试 TableSchema::column_names
    #[test]
    fn test_table_schema_column_names() {
        let schema = TableSchema::new("users", make_columns());
        let names = schema.column_names();
        assert_eq!(names, vec!["id", "name", "email"]);
    }

    /// 测试 TableSchema 序列化/反序列化
    #[test]
    fn test_table_schema_serde() {
        let schema = TableSchema::new("users", make_columns());
        let json = serde_json::to_string(&schema).unwrap();
        let deserialized: TableSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(schema, deserialized);
    }

    // ------------------------------------------------------------------------
    // SchemaCache: set/get 基本流程
    // ------------------------------------------------------------------------

    /// 测试 set/get schema 基本流程
    #[test]
    fn test_set_get_schema() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache);

        let schema = TableSchema::new("users", make_columns());
        schema_cache.set_schema("users", &schema, None).unwrap();

        let result = schema_cache.get_schema("users").unwrap();
        assert!(result.is_some());
        let cached = result.unwrap();
        assert_eq!(cached.table_name, "users");
        assert_eq!(cached.columns.len(), 3);
        assert_eq!(cached.primary_keys, vec!["id"]);
        assert_eq!(cached.columns[0].name, "id");
        assert!(cached.columns[0].primary_key);
        assert!(cached.columns[0].auto_increment);
        assert!(!cached.columns[0].nullable);
    }

    /// 测试 get_schema 缓存未命中
    #[test]
    fn test_get_schema_miss() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache);

        let result = schema_cache.get_schema("nonexistent").unwrap();
        assert!(result.is_none());
    }

    // ------------------------------------------------------------------------
    // SchemaCache: remember_schema
    // ------------------------------------------------------------------------

    /// 测试 remember_schema 缓存命中（不调用 loader）
    #[test]
    fn test_remember_schema_cache_hit() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache);

        // 预先写入缓存
        let schema = TableSchema::new("users", make_columns());
        schema_cache.set_schema("users", &schema, None).unwrap();

        // remember_schema 应命中缓存，不调用 loader
        let loader_called = Arc::new(AtomicBool::new(false));
        let loader_called_clone = loader_called.clone();

        let result = schema_cache
            .remember_schema("users", |_| {
                loader_called_clone.store(true, Ordering::SeqCst);
                Ok(TableSchema::new("users", vec![]))
            })
            .unwrap();

        assert!(
            !loader_called.load(Ordering::SeqCst),
            "loader should not be called on cache hit"
        );
        // 返回缓存中的 schema（3 个字段），不是 loader 的空 schema
        assert_eq!(result.columns.len(), 3);
    }

    /// 测试 remember_schema 缓存未命中（调用 loader 并缓存）
    #[test]
    fn test_remember_schema_cache_miss() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache.clone());

        // 缓存未命中，调用 loader
        let result = schema_cache
            .remember_schema("orders", |table| {
                assert_eq!(table, "orders");
                Ok(TableSchema::new(
                    "orders",
                    vec![ColumnDefinition::new("id", "bigint(20)")
                        .primary_key(true)
                        .auto_increment(true)],
                ))
            })
            .unwrap();

        assert_eq!(result.table_name, "orders");
        assert_eq!(result.columns.len(), 1);

        // 验证已写入缓存
        let cached = schema_cache.get_schema("orders").unwrap().unwrap();
        assert_eq!(cached.columns.len(), 1);
        assert_eq!(cached.columns[0].name, "id");
    }

    /// 测试 remember_schema loader 错误传播
    #[test]
    fn test_remember_schema_loader_error() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache);

        let result = schema_cache.remember_schema("broken", |_| {
            Err(SchemaCacheError::LoaderError("数据库连接失败".to_string()))
        });

        assert!(result.is_err());
        match result.unwrap_err() {
            SchemaCacheError::LoaderError(msg) => assert_eq!(msg, "数据库连接失败"),
            other => panic!("期望 LoaderError，实际: {:?}", other),
        }

        // loader 失败时不应写入缓存
        assert!(schema_cache.get_schema("broken").unwrap().is_none());
    }

    // ------------------------------------------------------------------------
    // SchemaCache: forget_schema
    // ------------------------------------------------------------------------

    /// 测试 forget_schema 清除单表
    #[test]
    fn test_forget_schema() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache);

        let schema = TableSchema::new("users", make_columns());
        schema_cache.set_schema("users", &schema, None).unwrap();
        assert!(schema_cache.has_schema("users").unwrap());

        schema_cache.forget_schema("users").unwrap();
        assert!(!schema_cache.has_schema("users").unwrap());
        assert!(schema_cache.get_schema("users").unwrap().is_none());
    }

    // ------------------------------------------------------------------------
    // SchemaCache: clear
    // ------------------------------------------------------------------------

    /// 测试 clear 清除所有
    #[test]
    fn test_clear() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache);

        // 写入多张表
        schema_cache
            .set_schema("users", &TableSchema::new("users", make_columns()), None)
            .unwrap();
        schema_cache
            .set_schema("orders", &TableSchema::new("orders", make_columns()), None)
            .unwrap();
        schema_cache
            .set_schema(
                "products",
                &TableSchema::new("products", make_columns()),
                None,
            )
            .unwrap();

        assert!(schema_cache.has_schema("users").unwrap());
        assert!(schema_cache.has_schema("orders").unwrap());
        assert!(schema_cache.has_schema("products").unwrap());

        // 清除所有
        schema_cache.clear().unwrap();

        assert!(!schema_cache.has_schema("users").unwrap());
        assert!(!schema_cache.has_schema("orders").unwrap());
        assert!(!schema_cache.has_schema("products").unwrap());
    }

    /// 测试 clear 不影响其他缓存（非 schema_cache 标签的缓存）
    #[test]
    fn test_clear_preserves_other_caches() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache.clone());

        // 写入 schema 缓存
        schema_cache
            .set_schema("users", &TableSchema::new("users", make_columns()), None)
            .unwrap();

        // 写入业务缓存（非 schema_cache 标签）
        cache.set("business:config", "value", None).unwrap();

        // 清除 schema 缓存
        schema_cache.clear().unwrap();

        // schema 缓存被清除
        assert!(!schema_cache.has_schema("users").unwrap());
        // 业务缓存不受影响
        assert!(cache.has("business:config").unwrap());
    }

    // ------------------------------------------------------------------------
    // SchemaCache: has_schema
    // ------------------------------------------------------------------------

    /// 测试 has_schema
    #[test]
    fn test_has_schema() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache);

        assert!(!schema_cache.has_schema("users").unwrap());

        let schema = TableSchema::new("users", make_columns());
        schema_cache.set_schema("users", &schema, None).unwrap();

        assert!(schema_cache.has_schema("users").unwrap());
        assert!(!schema_cache.has_schema("orders").unwrap());
    }

    // ------------------------------------------------------------------------
    // SchemaCache: 不同表名不冲突
    // ------------------------------------------------------------------------

    /// 测试不同表名不冲突
    #[test]
    fn test_different_tables_no_conflict() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache);

        let users_schema = TableSchema::new(
            "users",
            vec![
                ColumnDefinition::new("id", "int(11)").primary_key(true),
                ColumnDefinition::new("name", "varchar(255)"),
            ],
        );
        let orders_schema = TableSchema::new(
            "orders",
            vec![
                ColumnDefinition::new("order_id", "bigint(20)").primary_key(true),
                ColumnDefinition::new("amount", "decimal(10,2)"),
            ],
        );

        schema_cache
            .set_schema("users", &users_schema, None)
            .unwrap();
        schema_cache
            .set_schema("orders", &orders_schema, None)
            .unwrap();

        let users = schema_cache.get_schema("users").unwrap().unwrap();
        let orders = schema_cache.get_schema("orders").unwrap().unwrap();

        assert_eq!(users.table_name, "users");
        assert_eq!(users.columns[0].name, "id");
        assert_eq!(orders.table_name, "orders");
        assert_eq!(orders.columns[0].name, "order_id");

        // 清除单表不影响另一表
        schema_cache.forget_schema("users").unwrap();
        assert!(schema_cache.get_schema("users").unwrap().is_none());
        assert!(schema_cache.get_schema("orders").unwrap().is_some());
    }

    // ------------------------------------------------------------------------
    // SchemaCache: TTL 过期
    // ------------------------------------------------------------------------

    /// 测试 TTL 过期（使用较短 TTL）
    #[test]
    fn test_ttl_expiry() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache);

        let schema = TableSchema::new("temp_table", make_columns());
        // 设置 50ms TTL
        schema_cache
            .set_schema("temp_table", &schema, Some(Duration::from_millis(50)))
            .unwrap();

        // 立即读取应命中
        assert!(schema_cache.has_schema("temp_table").unwrap());
        assert!(schema_cache.get_schema("temp_table").unwrap().is_some());

        // 等待过期
        std::thread::sleep(Duration::from_millis(100));

        // 过期后应未命中
        assert!(!schema_cache.has_schema("temp_table").unwrap());
        assert!(schema_cache.get_schema("temp_table").unwrap().is_none());
    }

    /// 测试默认 TTL 为 None（永不过期）
    #[test]
    fn test_default_ttl_no_expiry() {
        let cache = make_cache();
        let schema_cache = SchemaCache::new(cache);

        let schema = TableSchema::new("permanent", make_columns());
        schema_cache.set_schema("permanent", &schema, None).unwrap();

        // 等待一小段时间
        std::thread::sleep(Duration::from_millis(50));

        // 仍然存在
        assert!(schema_cache.has_schema("permanent").unwrap());
        assert!(schema_cache.get_schema("permanent").unwrap().is_some());
    }

    // ------------------------------------------------------------------------
    // SchemaCache: 自定义 cache key 前缀
    // ------------------------------------------------------------------------

    /// 测试自定义 cache key 前缀
    #[test]
    fn test_custom_prefix() {
        let cache = make_cache();
        let schema_cache = SchemaCache::with_prefix(cache.clone(), "my_schema");

        let schema = TableSchema::new("users", make_columns());
        schema_cache.set_schema("users", &schema, None).unwrap();

        // 验证使用了自定义前缀
        let key = schema_cache.cache_key("users");
        assert_eq!(key, "my_schema:users");

        // 通过底层 cache 验证 key 存在
        assert!(cache.has("my_schema:users").unwrap());
        // 默认前缀的 key 不应存在
        assert!(!cache.has("schema_cache:users").unwrap());

        // clear 应清除自定义前缀的缓存
        schema_cache.clear().unwrap();
        assert!(!cache.has("my_schema:users").unwrap());
    }

    /// 测试不同前缀的 SchemaCache 实例互不干扰
    #[test]
    fn test_multiple_prefixes_no_conflict() {
        let cache = make_cache();
        let schema_cache_1 = SchemaCache::new(cache.clone());
        let schema_cache_2 = SchemaCache::with_prefix(cache.clone(), "db2_schema");

        let schema = TableSchema::new("users", make_columns());

        // 两个实例缓存同名的表
        schema_cache_1.set_schema("users", &schema, None).unwrap();
        schema_cache_2.set_schema("users", &schema, None).unwrap();

        // 两个实例都能读取
        assert!(schema_cache_1.has_schema("users").unwrap());
        assert!(schema_cache_2.has_schema("users").unwrap());

        // 底层 cache 中有两个不同的 key
        assert!(cache.has("schema_cache:users").unwrap());
        assert!(cache.has("db2_schema:users").unwrap());

        // 清除实例 1 不影响实例 2
        schema_cache_1.clear().unwrap();
        assert!(!schema_cache_1.has_schema("users").unwrap());
        assert!(schema_cache_2.has_schema("users").unwrap());
    }

    // ------------------------------------------------------------------------
    // SchemaCache: cache_key
    // ------------------------------------------------------------------------

    /// 测试 cache_key 格式
    #[test]
    fn test_cache_key_format() {
        let cache = make_cache();

        // 默认前缀
        let sc1 = SchemaCache::new(cache.clone());
        assert_eq!(sc1.cache_key("users"), "schema_cache:users");
        assert_eq!(sc1.cache_key("orders"), "schema_cache:orders");

        // 自定义前缀
        let sc2 = SchemaCache::with_prefix(cache, "custom_prefix");
        assert_eq!(sc2.cache_key("users"), "custom_prefix:users");
    }
}
