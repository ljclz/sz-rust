//! 关联缓存 — 对齐 PHP `withCache()` / `Cache::clear($tag)` 行为
//!
//! 本模块 re-export sz-orm-core `l2_cache` 模块的类型，
//! 并提供 PHP 命名约定辅助函数对齐 PHP `withCache()` / `Cache::clear($tag)` 行为。
//!
//! ## PHP 端关联缓存机制
//!
//! PHP think-orm 2.0.x 通过 `withCache()` 方法为关联预载入启用缓存：
//!
//! ```php
//! // 全部关联缓存
//! User::with(['orders', 'profile'])->withCache(true)->select();
//!
//! // 指定关联缓存
//! User::with(['orders', 'profile'])
//!     ->withCache('orders', true, 3600, null)
//!     ->select();
//! ```
//!
//! ### PHP `withCache()` 源码（ModelRelationQuery.php 第 310-340 行）
//!
//! ```php
//! public function withCache($relation = true, $key = true, $expire = null, string $tag = null)
//! {
//!     if (false === $relation || false === $key || !$this->getConnection()->getCache()) {
//!         return $this;
//!     }
//!
//!     if ($key instanceof \DateTimeInterface || $key instanceof \DateInterval || (is_int($key) && is_null($expire))) {
//!         $expire = $key;
//!         $key    = true;
//!     }
//!
//!     if (true === $relation || is_numeric($relation)) {
//!         $this->options['with_cache'] = $relation;  // 全部关联缓存
//!         return $this;
//!     }
//!
//!     $relations = (array) $relation;
//!     foreach ($relations as $name => $relation) {
//!         if (!is_numeric($name)) {
//!             $this->options['with_cache'][$name] = is_array($relation) ? $relation : [$key, $relation, $tag];
//!         } else {
//!             $this->options['with_cache'][$relation] = [$key, $expire, $tag];
//!         }
//!     }
//!
//!     return $this;
//! }
//! ```
//!
//! ### PHP 关联缓存使用流程
//!
//! 1. `withCache()` 设置 `options['with_cache']`
//! 2. `resultSetToModelCollection()` 第 487 行传递 `$with_cache` 到 `eagerlyResultSet()`
//! 3. `HasMany::eagerlyOneToMany()` 第 215 行调用 `$this->query->cache($cache[0], $cache[1], $cache[2])`
//! 4. `cache()` 方法（BaseQuery.php 第 775 行）设置 `options['cache'] = [$key, $expire, $tag ?: $this->getTable()]`
//! 5. 查询执行时通过 `getCacheKey()` 生成缓存键，命中则直接返回，未命中则查询后写入
//!
//! ### PHP 缓存键生成（Connection.php 第 290-299 行）
//!
//! ```php
//! protected function getCacheKey(BaseQuery $query, string $method = ''): string
//! {
//!     if (!empty($query->getOptions('key')) && empty($method)) {
//!         $key = 'think_' . $this->getConfig('database') . '.' . $query->getTable() . '|' . $query->getOptions('key');
//!     } else {
//!         $key = $query->getQueryGuid();  // SQL + bind 的 hash
//!     }
//!     return $key;
//! }
//! ```
//!
//! ### PHP 缓存写入（Connection.php 第 274-281 行）
//!
//! ```php
//! protected function cacheData(CacheItem $cacheItem)
//! {
//!     if ($cacheItem->getTag() && method_exists($this->cache, 'tag')) {
//!         $this->cache->tag($cacheItem->getTag())->set($cacheItem->getKey(), $cacheItem->get(), $cacheItem->getExpire());
//!     } else {
//!         $this->cache->set($cacheItem->getKey(), $cacheItem->get(), $cacheItem->getExpire());
//!     }
//! }
//! ```
//!
//! ### PHP 缓存失效机制
//!
//! - `Cache::delete($key)`：单键失效
//! - `Cache::clear($tag)` 或 `Cache::tag($tag)->clear()`：tag 维度失效（默认 tag 为表名）
//!
//! ## sz-orm-core 缓存能力
//!
//! sz-orm-core 提供两级缓存：
//!
//! | 模块 | 类型 | 值类型 | 公开方式 |
//! |------|------|-------|---------|
//! | `cache`（私有） | `Cache` trait / `MemoryCache` / `MultiLevelCache` | `Vec<u8>` | `pub use cache::*;` |
//! | `l2_cache`（公开） | `L2Cache` / `CacheKey` / `CacheKeyKind` / `L2CacheStats` | `Value` | `pub mod l2_cache;` |
//!
//! `L2Cache` 提供：
//!
//! - `put(&key, value, ttl)`：写入缓存
//! - `get(&key)`：读取缓存
//! - `invalidate(&key)`：单键失效（对齐 PHP `Cache::delete($key)`）
//! - `invalidate_table(&table)`：表级失效（对齐 PHP `Cache::clear($tag)`，因为 PHP tag 默认为表名）
//! - `stats()`：命中率统计
//!
//! ## PHP tag 与 sz-orm-core CacheKey.table 的映射
//!
//! PHP 中 `tag` 是独立于 `key` 的概念（通过 `CacheItem` 对象传递），默认值为表名：
//!
//! ```php
//! $this->options['cache'] = [$key, $expire, $tag ?: $this->getTable()];
//! ```
//!
//! sz-orm-core `CacheKey` 通过 `table` 字段实现表级索引，等价于 PHP tag。
//! 因此本模块假设 **tag 默认等于表名**（PHP 默认行为）。如果使用自定义 tag，
//! 调用方需通过 `CacheKey { table: <tag_value>, ... }` 自行管理映射关系。
//!
//! ## 本模块提供的函数
//!
//! ### 1. re-export sz-orm-core l2_cache 类型
//!
//! - [`L2Cache`]：跨 Session 共享的二级缓存
//! - [`CacheKey`]：统一缓存键（table + kind + identifier）
//! - [`CacheKeyKind`]：缓存键类型（ByPk / ByQuery / ByRelation）
//! - [`L2CacheStats`]：命中率统计
//!
//! ### 2. PHP 命名约定辅助类型
//!
//! - [`WithCacheConfig`]：对齐 PHP `[$key, $expire, $tag]` 三元组
//! - [`WithCacheOption`]：对齐 PHP `options['with_cache']` 的三种形态
//!
//! ### 3. PHP 命名约定辅助函数
//!
//! - [`php_with_cache_config`]：构造关联缓存配置（对齐 PHP `withCache()` 入口）
//! - [`php_relation_cache_key`]：生成 PHP 关联缓存键（对齐 PHP `getCacheKey()`）
//! - [`php_relation_cache_tag`]：生成 PHP 关联缓存 tag（对齐 `$tag ?: $this->getTable()`）
//! - [`php_relation_cache_remember`]：缓存关联查询结果（对齐 PHP `cacheData()`）
//! - [`php_relation_cache_fetch`]：读取关联缓存（对齐 `$this->cache->get()`）
//! - [`php_relation_cache_invalidate`]：失效整表关联缓存（对齐 `Cache::clear($tag)`）
//! - [`php_relation_cache_delete`]：失效单个关联缓存（对齐 `Cache::delete($key)`）
//!
//! ## 架构说明
//!
//! 沿用既有的 sz-orm-core::model 模块私有约束统一处理模式：
//!
//! - **re-export sz-orm-core l2_cache 类型**：`L2Cache` / `CacheKey` / `CacheKeyKind` / `L2CacheStats`
//! - **PHP 命名约定辅助类型**：`WithCacheConfig` / `WithCacheOption`
//! - **PHP 命名约定辅助函数**：`php_with_cache_config` / `php_relation_cache_key` /
//!   `php_relation_cache_tag` / `php_relation_cache_remember` / `php_relation_cache_fetch` /
//!   `php_relation_cache_invalidate` / `php_relation_cache_delete`
//!
//! 端到端关联缓存由 sz-orm-core `L2Cache` 内部实现，sz-rust 端通过辅助函数验证
//! 缓存行为对齐 PHP。

// re-export sz-orm-core l2_cache 类型
pub use sz_orm_core::l2_cache::{CacheKey, CacheKeyKind, L2Cache, L2CacheStats};

use std::collections::HashMap;
use std::time::Duration;
use sz_orm_core::Value;

// ============================================================================
// WithCacheConfig — PHP [$key, $expire, $tag] 三元组
// ============================================================================

/// PHP 关联缓存配置（对齐 `[$key, $expire, $tag]` 三元组）
///
/// 对齐 PHP `withCache($relation, $key, $expire, $tag)` 中每个关联的配置三元组：
///
/// ```php
/// $this->options['with_cache'][$relation] = [$key, $expire, $tag];
/// ```
///
/// ## 字段说明
///
/// - `key`：缓存键（`None` 对齐 PHP `$key = true` 自动生成；`Some(s)` 对齐自定义 key）
/// - `expire`：过期时间（`None` 对齐 PHP `$expire = null` 永不过期）
/// - `tag`：缓存标签（`None` 对齐 PHP `$tag = null`，使用默认表名）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::cache::{WithCacheConfig, php_with_cache_config};
/// use std::time::Duration;
///
/// // 等价 PHP: withCache('orders', true, 3600, null)
/// let config = php_with_cache_config(None, Some(Duration::from_secs(3600)), None);
/// assert_eq!(config.key, None);
/// assert_eq!(config.expire, Some(Duration::from_secs(3600)));
/// assert_eq!(config.tag, None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WithCacheConfig {
    /// 缓存键（None = 自动生成，对齐 PHP `$key = true`）
    pub key: Option<String>,
    /// 过期时间（None = 永不过期，对齐 PHP `$expire = null`）
    pub expire: Option<Duration>,
    /// 缓存标签（None = 使用默认表名，对齐 PHP `$tag = null`）
    pub tag: Option<String>,
}

impl WithCacheConfig {
    /// 创建新的缓存配置
    ///
    /// ## 参数
    ///
    /// - `key`：缓存键（`None` = 自动生成）
    /// - `expire`：过期时间（`None` = 永不过期）
    /// - `tag`：缓存标签（`None` = 使用默认表名）
    pub fn new(key: Option<String>, expire: Option<Duration>, tag: Option<String>) -> Self {
        Self { key, expire, tag }
    }

    /// 是否使用自动生成的缓存键（对齐 PHP `$key = true`）
    pub fn is_auto_key(&self) -> bool {
        self.key.is_none()
    }

    /// 是否永不过期（对齐 PHP `$expire = null`）
    pub fn is_permanent(&self) -> bool {
        self.expire.is_none()
    }

    /// 是否使用默认 tag 即表名（对齐 PHP `$tag = null`）
    pub fn is_default_tag(&self) -> bool {
        self.tag.is_none()
    }
}

// ============================================================================
// WithCacheOption — PHP options['with_cache'] 三种形态
// ============================================================================

/// PHP `options['with_cache']` 的三种形态
///
/// 对齐 PHP `withCache($relation, $key, $expire, $tag)` 行为：
///
/// ```php
/// // 1. 不缓存（false 或未设置）
/// $this->options['with_cache'] = false;
///
/// // 2. 全部关联缓存（true）
/// if (true === $relation || is_numeric($relation)) {
///     $this->options['with_cache'] = $relation;
/// }
///
/// // 3. 指定关联缓存
/// $this->options['with_cache'][$relation] = [$key, $expire, $tag];
/// ```
///
/// ## 变体
///
/// - [`WithCacheOption::None`]：对齐 PHP `with_cache = false` 或未设置（不缓存）
/// - [`WithCacheOption::All`]：对齐 PHP `with_cache = true`（全部关联都缓存）
/// - [`WithCacheOption::Specific`]：对齐 PHP `with_cache = [$name => [$key, $expire, $tag]]`
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::cache::{WithCacheOption, WithCacheConfig};
///
/// // 全部关联缓存（对齐 PHP withCache(true)）
/// let opt = WithCacheOption::All;
/// assert!(opt.is_enabled());
///
/// // 指定关联缓存（对齐 PHP withCache('orders', true, 3600, null)）
/// let mut specific = std::collections::HashMap::new();
/// specific.insert("orders".to_string(), WithCacheConfig::default());
/// let opt = WithCacheOption::Specific(specific);
/// assert!(opt.is_enabled());
///
/// // 不缓存（对齐 PHP withCache(false)）
/// let opt = WithCacheOption::None;
/// assert!(!opt.is_enabled());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WithCacheOption {
    /// 不缓存（对齐 PHP `with_cache = false` 或未设置）
    #[default]
    None,
    /// 全部关联都缓存（对齐 PHP `with_cache = true`）
    All,
    /// 指定关联缓存（对齐 PHP `with_cache = [$name => [$key, $expire, $tag]]`）
    Specific(HashMap<String, WithCacheConfig>),
}

impl WithCacheOption {
    /// 是否启用缓存
    pub fn is_enabled(&self) -> bool {
        !matches!(self, WithCacheOption::None)
    }

    /// 是否为全部关联缓存
    pub fn is_all(&self) -> bool {
        matches!(self, WithCacheOption::All)
    }

    /// 是否为指定关联缓存
    pub fn is_specific(&self) -> bool {
        matches!(self, WithCacheOption::Specific(_))
    }

    /// 获取指定关联的缓存配置
    ///
    /// 返回 `Some(&WithCacheConfig)` 如果：
    /// - 当前为 `All`（返回一个默认配置，对齐 PHP 全部关联使用相同默认配置）
    /// - 当前为 `Specific` 且包含指定关联名
    ///
    /// 返回 `None` 如果：
    /// - 当前为 `None`
    /// - 当前为 `Specific` 但不包含指定关联名
    pub fn get_config(&self, relation_name: &str) -> Option<&WithCacheConfig> {
        match self {
            WithCacheOption::All => Some(&DEFAULT_ALL_CONFIG),
            WithCacheOption::Specific(map) => map.get(relation_name),
            WithCacheOption::None => None,
        }
    }
}

/// `WithCacheOption::All` 的默认配置
///
/// 对齐 PHP `withCache(true)` 时所有关联使用默认 `[$key=true, $expire=null, $tag=null]` 配置。
const DEFAULT_ALL_CONFIG: WithCacheConfig = WithCacheConfig {
    key: None,
    expire: None,
    tag: None,
};

// ============================================================================
// php_with_cache_config — 构造关联缓存配置
// ============================================================================

/// 构造 PHP 关联缓存配置
///
/// 对齐 PHP `withCache($relation, $key, $expire, $tag)` 的入口：
///
/// ```php
/// public function withCache($relation = true, $key = true, $expire = null, string $tag = null)
/// ```
///
/// ## 参数
///
/// - `key`：缓存键（`None` 对齐 PHP `$key = true` 自动生成）
/// - `expire`：过期时间（`None` 对齐 PHP `$expire = null` 永不过期）
/// - `tag`：缓存标签（`None` 对齐 PHP `$tag = null` 使用默认表名）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::cache::php_with_cache_config;
/// use std::time::Duration;
///
/// // 等价 PHP: withCache('orders', true, 3600, null)
/// let config = php_with_cache_config(None, Some(Duration::from_secs(3600)), None);
/// assert!(config.is_auto_key());
/// assert!(!config.is_permanent());
/// assert!(config.is_default_tag());
/// ```
pub fn php_with_cache_config(
    key: Option<&str>,
    expire: Option<Duration>,
    tag: Option<&str>,
) -> WithCacheConfig {
    WithCacheConfig {
        key: key.map(|s| s.to_string()),
        expire,
        tag: tag.map(|s| s.to_string()),
    }
}

// ============================================================================
// php_relation_cache_key — 生成 PHP 关联缓存键
// ============================================================================

/// 生成 PHP 关联缓存键
///
/// 对齐 PHP `Connection::getCacheKey()` 第 293 行：
///
/// ```php
/// $key = 'think_' . $this->getConfig('database') . '.' . $query->getTable() . '|' . $query->getOptions('key');
/// ```
///
/// ## 参数
///
/// - `database`：数据库名（如 `"shop"`）
/// - `table`：表名（如 `"users"`）
/// - `key`：缓存键标识（如 `"1"` 或 `"orders:1"`）
///
/// ## 生成规则
///
/// ```text
/// think_{database}.{table}|{key}
/// ```
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::cache::php_relation_cache_key;
///
/// // 对齐 PHP: 'think_shop.users|1'
/// let key = php_relation_cache_key("shop", "users", "1");
/// assert_eq!(key, "think_shop.users|1");
/// ```
pub fn php_relation_cache_key(database: &str, table: &str, key: &str) -> String {
    format!("think_{}.{}|{}", database, table, key)
}

// ============================================================================
// php_relation_cache_tag — 生成 PHP 关联缓存 tag
// ============================================================================

/// 生成 PHP 关联缓存 tag
///
/// 对齐 PHP `BaseQuery::cache()` 第 786 行：
///
/// ```php
/// $this->options['cache'] = [$key, $expire, $tag ?: $this->getTable()];
/// ```
///
/// 默认 tag 为表名（`$this->getTable()`），如果传入自定义 tag 则使用自定义值。
///
/// ## 参数
///
/// - `table`：表名（作为默认 tag）
/// - `custom_tag`：自定义 tag（`None` 或空字符串使用表名）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::cache::php_relation_cache_tag;
///
/// // 无自定义 tag → 使用表名
/// let tag = php_relation_cache_tag("users", None);
/// assert_eq!(tag, "users");
///
/// // 有自定义 tag → 使用自定义值
/// let tag = php_relation_cache_tag("users", Some("user_cache"));
/// assert_eq!(tag, "user_cache");
/// ```
pub fn php_relation_cache_tag(table: &str, custom_tag: Option<&str>) -> String {
    match custom_tag {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => table.to_string(),
    }
}

// ============================================================================
// php_relation_cache_remember — 缓存关联查询结果
// ============================================================================

/// 缓存关联查询结果
///
/// 对齐 PHP `Connection::cacheData()` 第 274-281 行：
///
/// ```php
/// protected function cacheData(CacheItem $cacheItem)
/// {
///     if ($cacheItem->getTag() && method_exists($this->cache, 'tag')) {
///         $this->cache->tag($cacheItem->getTag())->set($cacheItem->getKey(), $cacheItem->get(), $cacheItem->getExpire());
///     } else {
///         $this->cache->set($cacheItem->getKey(), $cacheItem->get(), $cacheItem->getExpire());
///     }
/// }
/// ```
///
/// ## PHP tag 与 sz-orm-core CacheKey.table 的映射
///
/// PHP 中 `tag` 通过 `CacheItem` 独立传递，sz-orm-core `L2Cache` 通过 `CacheKey.table`
/// 字段实现表级索引（等价于 PHP tag）。因此本函数假设 `key.table` 已设置为正确的
/// tag 值（默认为表名，对齐 PHP `$tag ?: $this->getTable()`）。
///
/// ## 参数
///
/// - `cache`：L2Cache 实例
/// - `key`：缓存键（`key.table` 字段作为 tag，对齐 PHP `Cache::tag($tag)->set()`）
/// - `value`：缓存值
/// - `ttl`：过期时间（`None` 永不过期，对齐 PHP `$expire = null`）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::cache::*;
/// use sz_orm_core::Value;
///
/// let cache = L2Cache::new();
/// let key = CacheKey::by_relation("users", "orders:1");
/// php_relation_cache_remember(&cache, &key, Value::I64(42), None);
/// assert_eq!(cache.get(&key), Some(Value::I64(42)));
/// ```
pub fn php_relation_cache_remember(
    cache: &L2Cache,
    key: &CacheKey,
    value: Value,
    ttl: Option<Duration>,
) {
    cache.put(key, value, ttl);
}

// ============================================================================
// php_relation_cache_fetch — 读取关联缓存
// ============================================================================

/// 读取关联缓存
///
/// 对齐 PHP `$this->cache->get($key)` 行为。
///
/// ## 参数
///
/// - `cache`：L2Cache 实例
/// - `key`：缓存键
///
/// ## 返回值
///
/// - `Some(value)`：缓存命中
/// - `None`：缓存未命中或已过期
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::cache::*;
/// use sz_orm_core::Value;
///
/// let cache = L2Cache::new();
/// let key = CacheKey::by_relation("users", "orders:1");
/// cache.put(&key, Value::I64(42), None);
/// assert_eq!(php_relation_cache_fetch(&cache, &key), Some(Value::I64(42)));
/// ```
pub fn php_relation_cache_fetch(cache: &L2Cache, key: &CacheKey) -> Option<Value> {
    cache.get(key)
}

// ============================================================================
// php_relation_cache_invalidate — 失效整表关联缓存
// ============================================================================

/// 失效整表关联缓存
///
/// 对齐 PHP `Cache::clear($tag)` 或 `Cache::tag($tag)->clear()` 行为。
///
/// PHP 端 `cache()` 方法第 786 行默认 tag 为表名：
///
/// ```php
/// $this->options['cache'] = [$key, $expire, $tag ?: $this->getTable()];
/// ```
///
/// 因此 `Cache::clear($tag)` 实际是按表名失效所有缓存项，等价于
/// sz-orm-core `L2Cache::invalidate_table(table)`。
///
/// ## 参数
///
/// - `cache`：L2Cache 实例
/// - `table`：表名（对齐 PHP tag，默认为表名）
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::cache::*;
/// use sz_orm_core::Value;
///
/// let cache = L2Cache::new();
/// let key = CacheKey::by_relation("users", "orders:1");
/// cache.put(&key, Value::I64(42), None);
///
/// php_relation_cache_invalidate(&cache, "users");
/// assert_eq!(cache.get(&key), None);
/// ```
pub fn php_relation_cache_invalidate(cache: &L2Cache, table: &str) {
    cache.invalidate_table(table);
}

// ============================================================================
// php_relation_cache_delete — 失效单个关联缓存
// ============================================================================

/// 失效单个关联缓存
///
/// 对齐 PHP `Cache::delete($key)` 行为。
///
/// ## 参数
///
/// - `cache`：L2Cache 实例
/// - `key`：缓存键
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::relation::cache::*;
/// use sz_orm_core::Value;
///
/// let cache = L2Cache::new();
/// let key = CacheKey::by_relation("users", "orders:1");
/// cache.put(&key, Value::I64(42), None);
///
/// php_relation_cache_delete(&cache, &key);
/// assert_eq!(cache.get(&key), None);
/// ```
pub fn php_relation_cache_delete(cache: &L2Cache, key: &CacheKey) {
    cache.invalidate(key);
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use sz_orm_core::Value;

    // ====================================================================
    // 组 1：WithCacheConfig 结构体（5 个测试）
    // ====================================================================

    #[test]
    fn test_with_cache_config_default() {
        // 对齐 PHP 默认值：$key = true, $expire = null, $tag = null
        let config = WithCacheConfig::default();
        assert_eq!(config.key, None);
        assert_eq!(config.expire, None);
        assert_eq!(config.tag, None);
    }

    #[test]
    fn test_with_cache_config_new() {
        let config = WithCacheConfig::new(
            Some("custom_key".to_string()),
            Some(Duration::from_secs(3600)),
            Some("custom_tag".to_string()),
        );
        assert_eq!(config.key, Some("custom_key".to_string()));
        assert_eq!(config.expire, Some(Duration::from_secs(3600)));
        assert_eq!(config.tag, Some("custom_tag".to_string()));
    }

    #[test]
    fn test_with_cache_config_is_auto_key() {
        // None 对齐 PHP $key = true（自动生成）
        let config = WithCacheConfig::default();
        assert!(config.is_auto_key());

        let config = WithCacheConfig::new(Some("custom".to_string()), None, None);
        assert!(!config.is_auto_key());
    }

    #[test]
    fn test_with_cache_config_is_permanent() {
        // None 对齐 PHP $expire = null（永不过期）
        let config = WithCacheConfig::default();
        assert!(config.is_permanent());

        let config = WithCacheConfig::new(None, Some(Duration::from_secs(60)), None);
        assert!(!config.is_permanent());
    }

    #[test]
    fn test_with_cache_config_is_default_tag() {
        // None 对齐 PHP $tag = null（使用默认表名）
        let config = WithCacheConfig::default();
        assert!(config.is_default_tag());

        let config = WithCacheConfig::new(None, None, Some("custom_tag".to_string()));
        assert!(!config.is_default_tag());
    }

    // ====================================================================
    // 组 2：WithCacheOption 枚举（7 个测试）
    // ====================================================================

    #[test]
    fn test_with_cache_option_default_is_none() {
        // 对齐 PHP 默认值：未设置 with_cache
        let opt = WithCacheOption::default();
        assert!(matches!(opt, WithCacheOption::None));
    }

    #[test]
    fn test_with_cache_option_all_is_enabled() {
        // 对齐 PHP withCache(true)
        let opt = WithCacheOption::All;
        assert!(opt.is_enabled());
        assert!(opt.is_all());
        assert!(!opt.is_specific());
    }

    #[test]
    fn test_with_cache_option_specific_is_enabled() {
        // 对齐 PHP withCache('orders', true, null, null)
        let mut map = HashMap::new();
        map.insert("orders".to_string(), WithCacheConfig::default());
        let opt = WithCacheOption::Specific(map);
        assert!(opt.is_enabled());
        assert!(!opt.is_all());
        assert!(opt.is_specific());
    }

    #[test]
    fn test_with_cache_option_none_is_not_enabled() {
        // 对齐 PHP withCache(false)
        let opt = WithCacheOption::None;
        assert!(!opt.is_enabled());
        assert!(!opt.is_all());
        assert!(!opt.is_specific());
    }

    #[test]
    fn test_with_cache_option_get_config_all() {
        // All 变体：返回默认配置（对齐 PHP withCache(true) 全部关联使用默认配置）
        let opt = WithCacheOption::All;
        let config = opt.get_config("any_relation").unwrap();
        assert_eq!(config.key, None);
        assert_eq!(config.expire, None);
        assert_eq!(config.tag, None);
    }

    #[test]
    fn test_with_cache_option_get_config_specific_hit() {
        // Specific 变体：命中指定关联
        let mut map = HashMap::new();
        map.insert(
            "orders".to_string(),
            WithCacheConfig::new(None, Some(Duration::from_secs(3600)), None),
        );
        let opt = WithCacheOption::Specific(map);

        let config = opt.get_config("orders").unwrap();
        assert_eq!(config.expire, Some(Duration::from_secs(3600)));
    }

    #[test]
    fn test_with_cache_option_get_config_specific_miss_and_none() {
        // Specific 变体：未命中指定关联
        let mut map = HashMap::new();
        map.insert("orders".to_string(), WithCacheConfig::default());
        let opt = WithCacheOption::Specific(map);
        assert!(opt.get_config("nonexistent").is_none());

        // None 变体：始终返回 None
        let opt = WithCacheOption::None;
        assert!(opt.get_config("any").is_none());
    }

    // ====================================================================
    // 组 3：php_with_cache_config()（5 个测试）
    // ====================================================================

    #[test]
    fn test_php_with_cache_config_default() {
        // 对齐 PHP withCache($relation, true, null, null) 的配置三元组
        let config = php_with_cache_config(None, None, None);
        assert_eq!(config.key, None);
        assert_eq!(config.expire, None);
        assert_eq!(config.tag, None);
    }

    #[test]
    fn test_php_with_cache_config_with_expire() {
        // 对齐 PHP withCache('orders', true, 3600, null)
        let config = php_with_cache_config(None, Some(Duration::from_secs(3600)), None);
        assert_eq!(config.key, None);
        assert_eq!(config.expire, Some(Duration::from_secs(3600)));
        assert_eq!(config.tag, None);
    }

    #[test]
    fn test_php_with_cache_config_with_custom_key() {
        // 对齐 PHP withCache('orders', 'custom_key', null, null)
        let config = php_with_cache_config(Some("custom_key"), None, None);
        assert_eq!(config.key, Some("custom_key".to_string()));
    }

    #[test]
    fn test_php_with_cache_config_with_custom_tag() {
        // 对齐 PHP withCache('orders', true, null, 'user_cache')
        let config = php_with_cache_config(None, None, Some("user_cache"));
        assert_eq!(config.tag, Some("user_cache".to_string()));
    }

    #[test]
    fn test_php_with_cache_config_full() {
        // 对齐 PHP withCache('orders', 'key1', 3600, 'tag1')
        let config =
            php_with_cache_config(Some("key1"), Some(Duration::from_secs(3600)), Some("tag1"));
        assert_eq!(config.key, Some("key1".to_string()));
        assert_eq!(config.expire, Some(Duration::from_secs(3600)));
        assert_eq!(config.tag, Some("tag1".to_string()));
    }

    // ====================================================================
    // 组 4：php_relation_cache_key()（5 个测试）
    // ====================================================================

    #[test]
    fn test_php_relation_cache_key_basic() {
        // 对齐 PHP: 'think_shop.users|1'
        let key = php_relation_cache_key("shop", "users", "1");
        assert_eq!(key, "think_shop.users|1");
    }

    #[test]
    fn test_php_relation_cache_key_different_databases() {
        // 不同数据库生成不同 key
        let key1 = php_relation_cache_key("shop", "users", "1");
        let key2 = php_relation_cache_key("admin", "users", "1");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_php_relation_cache_key_different_tables() {
        // 不同表生成不同 key
        let key1 = php_relation_cache_key("shop", "users", "1");
        let key2 = php_relation_cache_key("shop", "orders", "1");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_php_relation_cache_key_different_pk() {
        // 不同主键生成不同 key
        let key1 = php_relation_cache_key("shop", "users", "1");
        let key2 = php_relation_cache_key("shop", "users", "2");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_php_relation_cache_key_format() {
        // 验证完整格式：think_{database}.{table}|{key}
        // 对齐 PHP Connection::getCacheKey() 第 293 行
        let key = php_relation_cache_key("my_db", "my_table", "my_key");
        assert_eq!(key, "think_my_db.my_table|my_key");
        // 验证 PHP 格式中的分隔符
        assert!(key.starts_with("think_"));
        assert!(key.contains("."));
        assert!(key.contains("|"));
    }

    // ====================================================================
    // 组 5：php_relation_cache_tag()（4 个测试）
    // ====================================================================

    #[test]
    fn test_php_relation_cache_tag_default() {
        // 对齐 PHP $tag ?: $this->getTable() — 无自定义 tag 时使用表名
        let tag = php_relation_cache_tag("users", None);
        assert_eq!(tag, "users");
    }

    #[test]
    fn test_php_relation_cache_tag_custom() {
        // 有自定义 tag 时使用自定义值
        let tag = php_relation_cache_tag("users", Some("user_cache"));
        assert_eq!(tag, "user_cache");
    }

    #[test]
    fn test_php_relation_cache_tag_empty_string_uses_table() {
        // 空字符串视为无 tag，使用表名（对齐 PHP $tag ?: $table 中 ?: 的 falsy 语义）
        let tag = php_relation_cache_tag("users", Some(""));
        assert_eq!(tag, "users");
    }

    #[test]
    fn test_php_relation_cache_tag_different_tables() {
        // 不同表生成不同 tag
        let tag1 = php_relation_cache_tag("users", None);
        let tag2 = php_relation_cache_tag("orders", None);
        assert_ne!(tag1, tag2);
    }

    // ====================================================================
    // 组 6：L2Cache 集成测试（10 个测试）
    // ====================================================================

    #[test]
    fn test_php_relation_cache_remember_and_fetch_hit() {
        // 对齐 PHP cacheData() + $this->cache->get() — 缓存命中
        let cache = L2Cache::new();
        let key = CacheKey::by_relation("users", "orders:1");
        php_relation_cache_remember(&cache, &key, Value::I64(42), None);

        let val = php_relation_cache_fetch(&cache, &key);
        assert_eq!(val, Some(Value::I64(42)));
    }

    #[test]
    fn test_php_relation_cache_fetch_miss() {
        // 缓存未命中
        let cache = L2Cache::new();
        let key = CacheKey::by_relation("users", "orders:1");
        let val = php_relation_cache_fetch(&cache, &key);
        assert_eq!(val, None);
    }

    #[test]
    fn test_php_relation_cache_invalidate_table() {
        // 对齐 PHP Cache::clear($tag) — 表级失效
        let cache = L2Cache::new();

        let key1 = CacheKey::by_relation("users", "orders:1");
        let key2 = CacheKey::by_relation("users", "orders:2");
        let key3 = CacheKey::by_relation("orders", "items:1"); // 不同表

        php_relation_cache_remember(&cache, &key1, Value::I64(1), None);
        php_relation_cache_remember(&cache, &key2, Value::I64(2), None);
        php_relation_cache_remember(&cache, &key3, Value::I64(3), None);

        // 失效 users 表
        php_relation_cache_invalidate(&cache, "users");

        // users 表的缓存项应被失效
        assert_eq!(php_relation_cache_fetch(&cache, &key1), None);
        assert_eq!(php_relation_cache_fetch(&cache, &key2), None);
        // orders 表的缓存项应保留
        assert_eq!(php_relation_cache_fetch(&cache, &key3), Some(Value::I64(3)));
    }

    #[test]
    fn test_php_relation_cache_delete_single() {
        // 对齐 PHP Cache::delete($key) — 单键失效
        let cache = L2Cache::new();
        let key1 = CacheKey::by_relation("users", "orders:1");
        let key2 = CacheKey::by_relation("users", "orders:2");

        php_relation_cache_remember(&cache, &key1, Value::I64(1), None);
        php_relation_cache_remember(&cache, &key2, Value::I64(2), None);

        // 仅删除 key1
        php_relation_cache_delete(&cache, &key1);

        assert_eq!(php_relation_cache_fetch(&cache, &key1), None);
        assert_eq!(php_relation_cache_fetch(&cache, &key2), Some(Value::I64(2)));
    }

    #[test]
    fn test_php_relation_cache_ttl_expiration() {
        // 对齐 PHP $expire 参数 — TTL 过期
        let cache = L2Cache::new();
        let key = CacheKey::by_relation("users", "orders:1");

        php_relation_cache_remember(
            &cache,
            &key,
            Value::I64(42),
            Some(Duration::from_millis(50)),
        );

        // 立即读取应命中
        assert_eq!(php_relation_cache_fetch(&cache, &key), Some(Value::I64(42)));

        // 等待过期
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(php_relation_cache_fetch(&cache, &key), None);
    }

    #[test]
    fn test_php_relation_cache_multiple_relations() {
        // 多关联缓存共存
        let cache = L2Cache::new();

        let orders_key = CacheKey::by_relation("users", "orders:1");
        let profile_key = CacheKey::by_relation("users", "profile:1");

        php_relation_cache_remember(&cache, &orders_key, Value::I64(10), None);
        php_relation_cache_remember(
            &cache,
            &profile_key,
            Value::String("Alice".to_string()),
            None,
        );

        assert_eq!(
            php_relation_cache_fetch(&cache, &orders_key),
            Some(Value::I64(10))
        );
        assert_eq!(
            php_relation_cache_fetch(&cache, &profile_key),
            Some(Value::String("Alice".to_string()))
        );
    }

    #[test]
    fn test_php_relation_cache_table_isolation() {
        // 不同表缓存隔离
        let cache = L2Cache::new();

        let users_key = CacheKey::by_relation("users", "pk:1");
        let orders_key = CacheKey::by_relation("orders", "pk:1");

        php_relation_cache_remember(&cache, &users_key, Value::I64(1), None);
        php_relation_cache_remember(&cache, &orders_key, Value::I64(2), None);

        // 失效 users 表不影响 orders 表
        php_relation_cache_invalidate(&cache, "users");
        assert_eq!(php_relation_cache_fetch(&cache, &users_key), None);
        assert_eq!(
            php_relation_cache_fetch(&cache, &orders_key),
            Some(Value::I64(2))
        );
    }

    #[test]
    fn test_with_cache_option_all_integration() {
        // 集成测试：WithCacheOption::All 全部关联缓存
        let opt = WithCacheOption::All;
        let cache = L2Cache::new();

        // 对所有关联应用缓存配置（对齐 PHP withCache(true)）
        for relation_name in &["orders", "profile", "comments"] {
            let config = opt.get_config(relation_name).unwrap();
            let key = CacheKey::by_relation("users", format!("{}:1", relation_name));
            let ttl = config.expire;
            php_relation_cache_remember(&cache, &key, Value::I64(1), ttl);
        }

        // 所有关联都应命中
        for relation_name in &["orders", "profile", "comments"] {
            let key = CacheKey::by_relation("users", format!("{}:1", relation_name));
            assert!(php_relation_cache_fetch(&cache, &key).is_some());
        }
    }

    #[test]
    fn test_with_cache_option_specific_integration() {
        // 集成测试：WithCacheOption::Specific 仅指定关联缓存
        let mut map = HashMap::new();
        map.insert(
            "orders".to_string(),
            WithCacheConfig::new(None, Some(Duration::from_secs(3600)), None),
        );
        // profile 未在 map 中，不应缓存
        let opt = WithCacheOption::Specific(map);

        assert!(opt.get_config("orders").is_some());
        assert!(opt.get_config("profile").is_none());

        // 仅 orders 关联应用缓存
        let cache = L2Cache::new();
        if let Some(config) = opt.get_config("orders") {
            let key = CacheKey::by_relation("users", "orders:1");
            php_relation_cache_remember(&cache, &key, Value::I64(1), config.expire);
        }

        let orders_key = CacheKey::by_relation("users", "orders:1");
        assert!(php_relation_cache_fetch(&cache, &orders_key).is_some());
    }

    #[test]
    fn test_php_relation_cache_overwrite() {
        // 对齐 PHP 同一 key 重复写入 — 覆盖旧值
        let cache = L2Cache::new();
        let key = CacheKey::by_relation("users", "orders:1");

        php_relation_cache_remember(&cache, &key, Value::I64(1), None);
        php_relation_cache_remember(&cache, &key, Value::I64(2), None);

        assert_eq!(php_relation_cache_fetch(&cache, &key), Some(Value::I64(2)));
    }

    // ====================================================================
    // 组 7：R5 PHP 行为对齐验证（7 个测试）
    // ====================================================================

    #[test]
    fn test_r5_php_with_cache_true_to_all() {
        // R5: PHP withCache(true) → WithCacheOption::All
        // PHP 源码 ModelRelationQuery.php 第 325-328 行：
        //   if (true === $relation || is_numeric($relation)) {
        //       $this->options['with_cache'] = $relation;
        //       return $this;
        //   }
        let opt = WithCacheOption::All;
        assert!(opt.is_enabled());
        assert!(opt.is_all());
        // 全部关联都应能获取到默认配置
        assert!(opt.get_config("orders").is_some());
        assert!(opt.get_config("profile").is_some());
    }

    #[test]
    fn test_r5_php_with_cache_named_to_specific() {
        // R5: PHP withCache('orders', true, 3600, null) → WithCacheOption::Specific
        // PHP 源码 ModelRelationQuery.php 第 330-337 行：
        //   $relations = (array) $relation;
        //   foreach ($relations as $name => $relation) {
        //       $this->options['with_cache'][$relation] = [$key, $expire, $tag];
        //   }
        let mut map = HashMap::new();
        map.insert(
            "orders".to_string(),
            php_with_cache_config(None, Some(Duration::from_secs(3600)), None),
        );
        let opt = WithCacheOption::Specific(map);

        assert!(opt.is_enabled());
        assert!(opt.is_specific());

        // 指定关联应能获取到配置
        let config = opt.get_config("orders").unwrap();
        assert_eq!(config.expire, Some(Duration::from_secs(3600)));

        // 未指定关联应获取不到配置
        assert!(opt.get_config("profile").is_none());
    }

    #[test]
    fn test_r5_php_with_cache_false_to_none() {
        // R5: PHP withCache(false) → WithCacheOption::None
        // PHP 源码 ModelRelationQuery.php 第 316-318 行：
        //   if (false === $relation || false === $key || !$this->getConnection()->getCache()) {
        //       return $this;
        //   }
        let opt = WithCacheOption::None;
        assert!(!opt.is_enabled());
        assert!(opt.get_config("any").is_none());
    }

    #[test]
    fn test_r5_php_tag_default_to_table() {
        // R5: PHP $tag ?: $this->getTable() → php_relation_cache_tag() 默认表名
        // PHP 源码 BaseQuery.php 第 786 行：
        //   $this->options['cache'] = [$key, $expire, $tag ?: $this->getTable()];
        let tag = php_relation_cache_tag("users", None);
        assert_eq!(tag, "users"); // 默认 tag = 表名
    }

    #[test]
    fn test_r5_php_cache_clear_to_invalidate_table() {
        // R5: PHP Cache::clear($tag) → php_relation_cache_invalidate() 表级失效
        // PHP 源码：tag 默认为表名，clear($tag) 失效该 tag 下的所有缓存
        let cache = L2Cache::new();

        let key1 = CacheKey::by_relation("users", "orders:1");
        let key2 = CacheKey::by_relation("users", "profile:1");
        let key3 = CacheKey::by_relation("orders", "items:1");

        php_relation_cache_remember(&cache, &key1, Value::I64(1), None);
        php_relation_cache_remember(&cache, &key2, Value::I64(2), None);
        php_relation_cache_remember(&cache, &key3, Value::I64(3), None);

        // Cache::clear('users') — 失效 users 表所有缓存
        php_relation_cache_invalidate(&cache, "users");

        // users 表缓存全部失效
        assert_eq!(php_relation_cache_fetch(&cache, &key1), None);
        assert_eq!(php_relation_cache_fetch(&cache, &key2), None);
        // orders 表缓存保留
        assert_eq!(php_relation_cache_fetch(&cache, &key3), Some(Value::I64(3)));
    }

    #[test]
    fn test_r5_php_cache_delete_to_invalidate_key() {
        // R5: PHP Cache::delete($key) → php_relation_cache_delete() 单键失效
        let cache = L2Cache::new();
        let key = CacheKey::by_relation("users", "orders:1");

        php_relation_cache_remember(&cache, &key, Value::I64(42), None);
        assert!(php_relation_cache_fetch(&cache, &key).is_some());

        php_relation_cache_delete(&cache, &key);
        assert!(php_relation_cache_fetch(&cache, &key).is_none());
    }

    #[test]
    fn test_r5_php_get_cache_key_format() {
        // R5: PHP getCacheKey() 格式 → php_relation_cache_key() 生成
        // PHP 源码 Connection.php 第 293 行：
        //   $key = 'think_' . $this->getConfig('database') . '.' . $query->getTable() . '|' . $query->getOptions('key');
        let key = php_relation_cache_key("shop", "users", "1");
        assert_eq!(key, "think_shop.users|1");

        // 验证 PHP 格式中的分隔符
        assert!(key.starts_with("think_"));
        assert!(key.contains("."));
        assert!(key.contains("|"));
    }
}
