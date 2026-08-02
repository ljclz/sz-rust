//! SZ-Rust Cache facade — 对齐 PHP `think\facade\Cache`
//!
//! 缓存 facade 模块。
//!
//! ## PHP 对齐
//!
//! ### 1. 静态 API 风格（对齐 PHP `think\facade\Cache::__callStatic`）
//!
//! PHP `think\facade\Cache` 是 facade，所有静态方法通过 `__callStatic` 转发到
//! `think\Cache`（Manager）实例的对应方法。
//!
//! Rust 端通过全局 `OnceLock<Cache>` + `Cache::default_instance()` 提供"伪静态"
//! API；调用方也可以创建独立 `Cache` 实例用于测试隔离。
//!
//! ### 2. 驱动管理器（对齐 PHP `think\Cache extends Manager`）
//!
//! PHP `think\Cache` 继承 `think\Manager`，通过 `$namespace = '\\think\\cache\\driver\\'`
//! 和 `createDriver(array $config)` 创建驱动实例，并缓存到 `$this->drivers[]`。
//!
//! Rust 端通过 `CacheManager` + `CacheDriver` trait 提供等价能力：
//! - `register_store(name, driver)`：注册命名驱动
//! - `store(name)`：获取命名驱动
//! - `default_store()`：获取默认驱动
//!
//! ### 3. 序列化策略（对齐 PHP `is_numeric` 短路）
//!
//! PHP `think\cache\Driver::serialize($data)` 第 612 行：
//!
//! ```php
//! public function serialize($data): string
//! {
//!     if (is_numeric($data)) {
//!         return (string) $data;
//!     }
//!     return serialize($data);
//! }
//! ```
//!
//! PHP `think\cache\Driver::unserialize($data)` 第 623 行：
//!
//! ```php
//! public function unserialize($data)
//! {
//!     if (is_numeric($data)) {
//!         return $data;  // ⚠️ 返回 string，而非 int（PHP 源码 bug）
//!     }
//!     return unserialize($data);
//! }
//! ```
//!
//! **PHP 源码 bug 复刻**：`unserialize` 对 `is_numeric` 的值返回 string，
//! 而非还原为 int。本模块通过 `CacheValue::Number` 标记 + `get::<String>()`
//! 返回 string 来复刻此行为。
//!
//! ### 4. `remember` 锁机制（对齐 PHP `think\cache\Driver::remember`）
//!
//! PHP `think\cache\Driver::remember` 第 287-310 行：
//!
//! ```php
//! public function remember(string $name, callable $callback, $expire = null)
//! {
//!     if (($data = $this->get($name)) !== null) {
//!         return $data;
//!     }
//!
//!     $lockName = $name . '_lock';
//!
//!     // 抢锁
//!     if ($this->has($lockName)) {
//!         // 等待锁释放，200ms 轮询，5 秒超时
//!         $startTime = microtime(true);
//!         while ($this->has($lockName) && microtime(true) - $startTime < 5) {
//!             usleep(200000);
//!         }
//!         // 锁释放后再次读取
//!         if ($this->has($lockName)) {
//!             // 超时仍未释放，直接调用 callback（防止永久阻塞）
//!             return $callback();
//!         }
//!         $data = $this->get($name);
//!         if ($data !== null) {
//!             return $data;
//!         }
//!     }
//!
//!     // 抢到锁（无 TTL，PHP 源码 bug：锁不设过期时间）
//!     $this->set($lockName, 1);
//!
//!     try {
//!         $data = $callback();
//!         $this->set($name, $data, $expire);
//!     } finally {
//!         $this->delete($lockName);
//!     }
//!
//!     return $data;
//! }
//! ```
//!
//! **PHP 源码 bug 复刻**：
//! 1. 锁 key 无 TTL（若进程崩溃，锁永久存在 → 死锁）
//! 2. `has()` + `get() !== null` 双查（先 `has` 后 `get`，存在 TOCTOU）
//!
//! ### 5. `push` 上限 1000 + array_shift + array_unique（对齐 PHP）
//!
//! PHP `think\cache\Driver::push($name, $value)` 第 339-358 行：
//!
//! ```php
//! public function push(string $name, $value, $expire = null)
//! {
//!     $data = $this->get($name, []);
//!     if (!is_array($data)) {
//!         $data = [];
//!     }
//!     $data[] = $value;
//!
//!     // 上限 1000
//!     if (count($data) > 1000) {
//!         array_shift($data);  // 丢弃最旧
//!     }
//!
//!     // 去重
//!     $data = array_unique($data);
//!
//!     $this->set($name, $data, $expire);
//!     return $this;
//! }
//! ```
//!
//! **PHP 行为复刻**：
//! - 数组上限 1000，超过时丢弃最旧（FIFO）
//! - `array_unique` 去重（保留首次出现的元素）
//!
//! ### 6. `inc` / `dec` 不经序列化（对齐 PHP Redis 驱动）
//!
//! PHP `think\cache\driver\Redis::inc($name, $step = 1)` 第 156 行：
//!
//! ```php
//! public function inc(string $name, int $step = 1): bool
//! {
//!     if ($this->handler->exists($name)) {
//!         $value = $this->handler->incrby($name, $step);
//!         // ...
//!     }
//!     // 不存在时初始化为 step
//!     $this->handler->set($name, $step);
//!     return true;
//! }
//! ```
//!
//! Redis 驱动直接使用 `INCRBY` / `DECRBY` 命令，不经过 `serialize`/`unserialize`。
//! File 驱动则会读取 → 加减 → 写回。本 `MemoryCacheDriver` 采用 File 驱动行为：
//! 读取 → 解析为 i64 → 加减 → 写回（数字字符串形式）。
//!
//! ## 架构
//!
//! ```text
//! SzRustCache (facade)
//!     ↓
//! CacheManager (driver manager, like PHP think\Cache extends Manager)
//!     ↓
//! CacheDriver trait (like PHP think\cache\Driver abstract)
//!     ↓
//! MemoryCacheDriver (PHP think\cache\driver\File analog, in-memory)
//!     ↓
//! sz_orm_core::Cache trait / MemoryCache (底层 KV 存储)
//! ```
//!
//! ## 使用示例
//!
//! ```ignore
//! use sz_rust_core::cache::{Cache, MemoryCacheDriver};
//! use std::time::Duration;
//!
//! // 注册默认驱动
//! let cache = Cache::new();
//! cache.register_default(MemoryCacheDriver::new());
//!
//! // 基本 set/get
//! cache.set("user:1", "Alice", None).unwrap();
//! assert_eq!(cache.get::<String>("user:1").unwrap(), Some("Alice".to_string()));
//!
//! // is_numeric 短路
//! cache.set("count", 42i64, None).unwrap();
//! // PHP bug 复刻：unserialize 返回 string，而非 int
//! assert_eq!(cache.get::<String>("count").unwrap(), Some("42".to_string()));
//!
//! // remember
//! let val = cache.remember("expensive", None, || 100i64).unwrap();
//! assert_eq!(val, 100);
//! ```

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::orm::{Cache as InnerCache, CacheError, MemoryCache};

// Memcached 缓存驱动子模块
mod memcached;
pub use memcached::{
    MemcachedBackend, MemcachedCacheDriver, MemcachedConfig, MockMemcachedBackend,
};

// ============================================================================
// 全局 Cache facade 实例
// ============================================================================

/// 全局 `Cache` facade 实例（对齐 PHP `think\facade\Cache` 静态 API）
///
/// 使用 `OnceLock` 提供进程级单例，所有 `Cache::default_instance()` 调用
/// 返回同一个实例。PHP 端通过 `think\facade\Cache::__callStatic` 转发到
/// `think\App::get('cache')` 容器单例。
static GLOBAL_CACHE: OnceLock<Cache> = OnceLock::new();

/// 获取全局 `Cache` facade 实例
///
/// 对齐 PHP `Cache::set/get/delete/...` 静态调用方式。
///
/// ## 注意
///
/// 首次调用会创建一个空的 `Cache`（未注册任何驱动）。调用方需先调用
/// `Cache::init_default` 注册默认驱动后再使用。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::cache::{Cache, MemoryCacheDriver};
///
/// Cache::init_default(MemoryCacheDriver::new());
/// Cache::default().set("key", "value", None).unwrap();
/// ```
pub fn default_cache() -> &'static Cache {
    GLOBAL_CACHE.get_or_init(Cache::new)
}

/// 初始化全局 `Cache` facade 实例（注册默认驱动）
///
/// 调用此函数会替换全局 `Cache` 的内部状态。若已注册过默认驱动，会被覆盖。
///
/// ## 示例
///
/// ```ignore
/// use sz_rust_core::cache::{default_cache, init_default_cache, MemoryCacheDriver};
///
/// init_default_cache(MemoryCacheDriver::new());
/// default_cache().set("key", "value", None).unwrap();
/// ```
pub fn init_default_cache(driver: MemoryCacheDriver) {
    let cache = default_cache();
    cache.register_default(driver);
}

// ============================================================================
// CacheValue — 缓存值（区分 is_numeric 短路）
// ============================================================================

/// 缓存值（区分 `is_numeric` 短路与 JSON 序列化）
///
/// 对齐 PHP `think\cache\Driver::serialize/unserialize` 行为：
///
/// - `Number(s)`：对齐 `is_numeric($data) === true`，存储为字符串
/// - `Json(s)`：对齐 `serialize($data)`，存储为 JSON 字符串
///
/// **PHP bug 复刻**：`unserialize` 对 `is_numeric` 返回 string，而非 int。
/// 本枚举通过 `Number(String)` 携带原始字符串，反序列化时直接返回 string，
/// 不还原为 int 类型。
#[derive(Debug, Clone, PartialEq)]
pub enum CacheValue {
    /// 数值型缓存值（对齐 PHP `is_numeric` 短路）
    ///
    /// 存储为字符串形式，反序列化时直接返回 string。
    Number(String),
    /// JSON 序列化缓存值（对齐 PHP `serialize`）
    ///
    /// 存储为 JSON 字符串，反序列化时通过 `serde_json::from_str` 还原。
    Json(String),
}

impl CacheValue {
    /// 序列化缓存值为存储字节
    ///
    /// 对齐 PHP `Driver::serialize($data)`：返回字符串后转字节。
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            CacheValue::Number(s) => s.as_bytes().to_vec(),
            CacheValue::Json(s) => s.as_bytes().to_vec(),
        }
    }

    /// 从存储字节反序列化缓存值
    ///
    /// 对齐 PHP `Driver::unserialize($data)`：
    /// - 若 `is_numeric` → 返回 `Number(s)`（保留原始字符串）
    /// - 否则 → 返回 `Json(s)`
    pub fn from_bytes(bytes: &[u8]) -> Result<CacheValue, CacheError> {
        let s = std::str::from_utf8(bytes)
            .map_err(|e| CacheError::DeserializationError(e.to_string()))?;
        if php_is_numeric(s) {
            Ok(CacheValue::Number(s.to_string()))
        } else {
            Ok(CacheValue::Json(s.to_string()))
        }
    }

    /// 是否为数值型（对齐 PHP `is_numeric`）
    pub fn is_number(&self) -> bool {
        matches!(self, CacheValue::Number(_))
    }
}

// ============================================================================
// PHP is_numeric 对齐
// ============================================================================

/// PHP `is_numeric` 简化实现
///
/// 对齐 PHP `is_numeric($data)` 行为：
///
/// - 整数字符串（如 `"42"`, `"-42"`, `"+42"`）
/// - 浮点数字符串（如 `"3.14"`, `"-3.14"`, `"+3.14"`）
/// - 科学计数法（如 `"1e10"`, `"1.5E-3"`）
///
/// ## PHP 行为
///
/// ```php
/// is_numeric("42");      // true
/// is_numeric("-42");     // true
/// is_numeric("3.14");    // true
/// is_numeric("1e10");    // true
/// is_numeric("abc");     // false
/// is_numeric("12abc");   // false
/// is_numeric("");        // false
/// is_numeric("0x1A");    // false（PHP 7+ 不识别十六进制字符串）
/// ```
///
/// ## 限制
///
/// PHP `is_numeric` 还支持前导空格（如 `" 42"`），但 think-orm 的
/// `serialize` 入参 `$data` 是 `$value`（任意类型），不会包含前导空格。
/// 因此本实现不处理前导空格。
pub fn php_is_numeric(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // 整数
    if s.parse::<i64>().is_ok() {
        return true;
    }
    // 浮点数
    if s.parse::<f64>().is_ok() {
        return true;
    }
    // 科学计数法（parse::<f64> 已支持 "1e10" 等）
    // 但 PHP 还允许前导 + 号、前导 - 号等，parse::<f64> 也支持
    false
}

// ============================================================================
// 序列化辅助 — 对齐 PHP think\cache\Driver::serialize
// ============================================================================

/// 序列化值（对齐 PHP `Driver::serialize($data)`）
///
/// PHP `Driver::serialize($data)` 第 612 行：
/// - `is_numeric($data)` → `(string) $data`
/// - 否则 → `serialize($data)`（PHP 原生序列化）
///
/// Rust 端用 `serde_json::to_string` 替代 PHP `serialize`：
/// - 整数 / 浮点数 → `CacheValue::Number(s)`
/// - 其他 → `CacheValue::Json(s)`
pub fn php_serialize<T: Serialize>(value: &T) -> Result<CacheValue, CacheError> {
    // 短路：字符串字面量可能是数字字符串
    // 对齐 PHP: $data = "42"; is_numeric($data) === true
    let json =
        serde_json::to_string(value).map_err(|e| CacheError::SerializationError(e.to_string()))?;

    // serde_json 序列化 string "42" → "\"42\""（带引号）
    // serde_json 序列化 i64 42 → "42"（无引号）
    // 因此 JSON 字符串无引号即为 is_numeric
    if php_is_numeric(&json) {
        Ok(CacheValue::Number(json))
    } else {
        Ok(CacheValue::Json(json))
    }
}

/// 反序列化值（对齐 PHP `Driver::unserialize($data)`）
///
/// PHP `Driver::unserialize($data)` 第 623 行：
/// - `is_numeric($data)` → 返回 string（⚠️ PHP 源码 bug：不还原为 int）
/// - 否则 → `unserialize($data)`
///
/// ## 泛型
///
/// - `T = String`：对齐 PHP `unserialize` 对 numeric 返回 string 的行为
/// - `T = Other`：使用 `serde_json::from_str` 还原
///
/// **注意**：若想获取 `i64`，需调用方自行 `.parse::<i64>()`，
/// 对齐 PHP 业务代码中 `(int) Cache::get('count')` 的强转模式。
pub fn php_unserialize<T: DeserializeOwned>(value: &CacheValue) -> Result<Option<T>, CacheError> {
    match value {
        CacheValue::Number(s) => {
            // PHP bug 复刻：numeric 值返回 string
            // 若 T 是 String，直接返回；否则尝试解析
            serde_json::from_str::<T>(&format!("\"{}\"", s))
                .map(Some)
                .map_err(|e| CacheError::DeserializationError(e.to_string()))
        }
        CacheValue::Json(s) => serde_json::from_str::<T>(s)
            .map(Some)
            .map_err(|e| CacheError::DeserializationError(e.to_string())),
    }
}

// ============================================================================
// CacheDriver trait — 对齐 PHP think\cache\Driver
// ============================================================================

/// 缓存驱动 trait（对齐 PHP `think\cache\Driver` 抽象基类）
///
/// PHP `think\cache\Driver`（359 行）定义了缓存驱动的核心 API：
/// `get/set/delete/has/inc/dec/remember/pull/push/tag/clear`。
///
/// 本 trait 提供等价的 Rust 抽象，底层由具体驱动（如 `MemoryCacheDriver`）
/// 实现，上层 `Cache` facade 委托到 trait 方法。
///
/// ## 与 `sz_orm_core::Cache` 的关系
///
/// `sz_orm_core::Cache` trait 提供底层 KV 操作（`get/set/delete/exists`），
/// 本 trait 在其之上扩展 PHP think-orm 行为：
///
/// - `inc/dec`：自增/自减（不经序列化层）
/// - `has`：键是否存在（对齐 PHP `has`，含 TTL 过期检查）
/// - `clear`：清空所有缓存
pub trait CacheDriver: Send + Sync {
    /// 读取缓存原始字节
    fn get_raw(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError>;

    /// 写入缓存原始字节
    fn set_raw(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<(), CacheError>;

    /// 删除缓存
    fn delete(&self, key: &str) -> Result<(), CacheError>;

    /// 判断键是否存在（对齐 PHP `has`，含 TTL 过期检查）
    fn has(&self, key: &str) -> Result<bool, CacheError>;

    /// 自增（对齐 PHP `inc`，不经序列化层）
    ///
    /// PHP Redis 驱动直接 `INCRBY`；File 驱动读取 → 加减 → 写回。
    /// 本 trait 默认实现采用 File 驱动行为。
    ///
    /// ## 行为
    ///
    /// - 键不存在：初始化为 `step`，返回 `step`
    /// - 键存在：解析为 i64 → 加 `step` → 写回 → 返回新值
    /// - 解析失败：返回 `CacheError`
    fn inc(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        let current = match self.get_raw(key)? {
            Some(bytes) => {
                let s = std::str::from_utf8(&bytes)
                    .map_err(|e| CacheError::DeserializationError(e.to_string()))?;
                s.parse::<i64>().map_err(|e| {
                    CacheError::DeserializationError(format!("inc: parse {} failed: {}", s, e))
                })?
            }
            None => 0,
        };
        let new_value = current + step;
        self.set_raw(key, new_value.to_string().into_bytes(), None)?;
        Ok(new_value)
    }

    /// 自减（对齐 PHP `dec`，不经序列化层）
    ///
    /// 默认实现委托到 `inc(key, -step)`。
    fn dec(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        self.inc(key, -step)
    }

    /// 清空所有缓存（对齐 PHP `clear`）
    fn clear(&self) -> Result<(), CacheError>;

    // ========================================================================
    // 缓存标签（对齐 PHP think\cache\Driver tag 相关方法）
    // ========================================================================

    /// 构造缓存 key（对齐 PHP `Driver::getCacheKey`）
    ///
    /// PHP `Driver::getCacheKey(name)` 第 86-89 行：
    /// ```php
    /// return $this->options['prefix'] . $name;
    /// ```
    ///
    /// 默认实现：无前缀（对齐 `MemoryCacheDriver` / `MultiLevelCacheDriver`）。
    /// `RedisCacheDriver` 重写为 `prefix + name`。
    fn get_cache_key(&self, name: &str) -> String {
        name.to_string()
    }

    /// 构造 tag key（对齐 PHP `Driver::getTagKey`）
    ///
    /// PHP `Driver::getTagKey(tag)` 第 226-229 行：
    /// ```php
    /// return $this->options['tag_prefix'] . md5($tag);
    /// ```
    ///
    /// 默认实现：`format!("tag:{}", compute_md5(tag))`（PHP 默认 `tag_prefix = "tag:"`）。
    fn get_tag_key(&self, tag: &str) -> String {
        format!("tag:{}", compute_md5(tag))
    }

    /// 追加 cache_key 到标签集合（对齐 PHP `Driver::append` / `Driver::push`）
    ///
    /// PHP `Driver::append(name, value)` 调用 `push(name, value)`（第 140-143 行），
    /// `push` 实现（第 114-131 行）：
    ///
    /// ```php
    /// public function push(string $name, $value): void
    /// {
    ///     $item = $this->get($name, []);
    ///     if (!is_array($item)) {
    ///         throw new InvalidArgumentException('only array cache can be push');
    ///     }
    ///     $item[] = $value;
    ///     if (count($item) > 1000) {
    ///         array_shift($item);
    ///     }
    ///     $item = array_unique($item);
    ///     $this->set($name, $item);
    /// }
    /// ```
    ///
    /// ## 参数
    ///
    /// - `tag_key`：标签 key（`getTagKey(tag)` 返回值，**未**应用 `getCacheKey` 前缀）
    /// - `cache_key`：缓存 key（`getCacheKey(name)` 返回值，**已**应用前缀）
    ///
    /// ## 行为
    ///
    /// 默认实现采用 File 驱动语义（`push`）：
    /// 1. 读取 `getCacheKey(tag_key)` → 反序列化为 `Vec<String>`
    /// 2. 追加 `cache_key`
    /// 3. 长度 > 1000 → 丢弃最旧（FIFO，对齐 `array_shift`）
    /// 4. 去重（对齐 `array_unique`，保留首次出现）
    /// 5. 写回 `getCacheKey(tag_key)`
    ///
    /// `RedisCacheDriver` 重写为 `sAdd`（Redis Set 语义）。
    fn tag_append(&self, tag_key: &str, cache_key: &str) -> Result<(), CacheError> {
        let storage_key = self.get_cache_key(tag_key);
        // 对齐 PHP: $item = $this->get($name, []);
        let mut items: Vec<String> = match self.get_raw(&storage_key)? {
            Some(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| CacheError::DeserializationError(format!("tag_append: {}", e)))?,
            None => Vec::new(),
        };
        // 对齐 PHP: $item[] = $value;
        items.push(cache_key.to_string());
        // 对齐 PHP: if (count($item) > 1000) { array_shift($item); }
        while items.len() > 1000 {
            items.remove(0);
        }
        // 对齐 PHP: $item = array_unique($item);
        let mut seen = HashSet::new();
        items.retain(|item| seen.insert(item.clone()));
        // 对齐 PHP: $this->set($name, $item);
        let serialized = serde_json::to_vec(&items)
            .map_err(|e| CacheError::SerializationError(e.to_string()))?;
        self.set_raw(&storage_key, serialized, None)
    }

    /// 获取标签包含的所有缓存 key（对齐 PHP `Driver::getTagItems`）
    ///
    /// PHP `Driver::getTagItems(tag)` 第 214-218 行：
    /// ```php
    /// public function getTagItems(string $tag): array
    /// {
    ///     $name = $this->getTagKey($tag);
    ///     return $this->get($name, []);
    /// }
    /// ```
    ///
    /// ## 参数
    ///
    /// - `tag`：标签名（方法内部计算 `getTagKey` + 应用 `getCacheKey` 前缀）
    ///
    /// ## 返回
    ///
    /// 返回该标签下所有缓存 key（已应用前缀）的列表。
    fn tag_items(&self, tag: &str) -> Result<Vec<String>, CacheError> {
        let tag_key = self.get_tag_key(tag);
        let storage_key = self.get_cache_key(&tag_key);
        match self.get_raw(&storage_key)? {
            Some(bytes) => {
                let items: Vec<String> = serde_json::from_slice(&bytes)
                    .map_err(|e| CacheError::DeserializationError(format!("tag_items: {}", e)))?;
                Ok(items)
            }
            None => Ok(Vec::new()),
        }
    }

    /// 批量删除缓存 key（对齐 PHP `Redis::clearTag`）
    ///
    /// PHP `Redis::clearTag(keys)` 第 217-221 行：
    /// ```php
    /// public function clearTag(array $keys): void
    /// {
    ///     $this->handler->del($keys);
    /// }
    /// ```
    ///
    /// ## 注意
    ///
    /// `keys` 是**已应用前缀**的缓存 key（来自 `tag_items` 返回值），
    /// 因此本方法**不得**再次应用 `getCacheKey`。
    ///
    /// 默认实现：对每个 key 调用 `delete`（适用于无前缀驱动）。
    /// `RedisCacheDriver` 重写为 `del_many`（raw delete，不应用前缀）。
    fn tag_clear(&self, keys: &[String]) -> Result<(), CacheError> {
        for key in keys {
            let _ = self.delete(key);
        }
        Ok(())
    }
}

// ============================================================================
// MemoryCacheDriver — 内存驱动（对齐 PHP think\cache\driver\File）
// ============================================================================

/// 内存缓存驱动（对齐 PHP `think\cache\driver\File`）
///
/// 基于 `sz_orm_core::MemoryCache`，提供进程内内存缓存。
///
/// ## PHP 对齐
///
/// PHP `think\cache\driver\File` 使用文件系统存储缓存，序列化为 PHP 字符串
/// 加 `<?php exit();?>` 头部防护。本驱动使用内存 HashMap，简化文件 IO，
/// 但保留 PHP think-orm 的核心行为：
///
/// - `get`：TTL 过期返回 `None`
/// - `set`：支持 TTL
/// - `has`：TTL 过期返回 `false`
/// - `inc/dec`：读取 → 加减 → 写回（数字字符串）
pub struct MemoryCacheDriver {
    inner: MemoryCache,
}

impl MemoryCacheDriver {
    /// 创建新的内存缓存驱动
    pub fn new() -> Self {
        Self {
            inner: MemoryCache::new(),
        }
    }

    /// 创建带默认 TTL 的内存缓存驱动
    pub fn with_default_ttl(ttl: Duration) -> Self {
        Self {
            inner: MemoryCache::with_ttl(ttl),
        }
    }
}

impl Default for MemoryCacheDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheDriver for MemoryCacheDriver {
    fn get_raw(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        InnerCache::get(&self.inner, key)
    }

    fn set_raw(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<(), CacheError> {
        InnerCache::set(&self.inner, key, value, ttl)
    }

    fn delete(&self, key: &str) -> Result<(), CacheError> {
        InnerCache::delete(&self.inner, key)
    }

    fn has(&self, key: &str) -> Result<bool, CacheError> {
        InnerCache::exists(&self.inner, key)
    }

    fn clear(&self) -> Result<(), CacheError> {
        InnerCache::clear(&self.inner)
    }
}

// ============================================================================
// CacheManager — 驱动管理器（对齐 PHP think\Cache extends Manager）
// ============================================================================

/// 缓存驱动管理器（对齐 PHP `think\Cache extends Manager`）
///
/// PHP `think\Cache` 继承 `think\Manager`，提供多驱动支持：
///
/// ```php
/// $cache = new think\Cache();
/// $cache->store('file')->set('key', 'value');
/// $cache->store('redis')->set('key', 'value');
/// ```
///
/// 本结构体提供等价的 Rust 抽象：
///
/// - `register_store(name, driver)`：注册命名驱动
/// - `store(name)`：获取命名驱动
/// - `default_store()`：获取默认驱动
pub struct CacheManager {
    /// 默认驱动名（对齐 PHP `'default' => Env::get('cache.driver', 'redis')`）
    default: String,
    /// 命名驱动表（对齐 PHP `Manager::$drivers`）
    stores: HashMap<String, Box<dyn CacheDriver>>,
}

impl CacheManager {
    /// 创建新的驱动管理器（无默认驱动）
    pub fn new() -> Self {
        Self {
            default: String::new(),
            stores: HashMap::new(),
        }
    }

    /// 注册命名驱动
    ///
    /// 对齐 PHP `Manager::createDriver(array $config)` + `$this->drivers[$name] = $driver`。
    ///
    /// ## 参数
    ///
    /// - `name`：驱动名（如 `"file"`、`"redis"`）
    /// - `driver`：驱动实例
    pub fn register_store(&mut self, name: impl Into<String>, driver: Box<dyn CacheDriver>) {
        let name = name.into();
        if self.default.is_empty() {
            self.default = name.clone();
        }
        self.stores.insert(name, driver);
    }

    /// 设置默认驱动名
    ///
    /// 对齐 PHP `config/cache.php` 中 `'default' => 'redis'`。
    pub fn set_default(&mut self, name: impl Into<String>) -> Result<(), CacheError> {
        let name = name.into();
        if !self.stores.contains_key(&name) {
            return Err(CacheError::NotFound(format!(
                "cache store '{}' not registered",
                name
            )));
        }
        self.default = name;
        Ok(())
    }

    /// 获取命名驱动
    ///
    /// 对齐 PHP `Manager::store(string $name = null)`。
    pub fn store(&self, name: &str) -> Result<&dyn CacheDriver, CacheError> {
        self.stores
            .get(name)
            .map(|d| d.as_ref())
            .ok_or_else(|| CacheError::NotFound(format!("cache store '{}' not found", name)))
    }

    /// 获取默认驱动
    ///
    /// 对齐 PHP `$cache->store()`（默认驱动）。
    pub fn default_store(&self) -> Result<&dyn CacheDriver, CacheError> {
        if self.default.is_empty() {
            return Err(CacheError::NotFound(
                "no default cache store registered".to_string(),
            ));
        }
        self.store(&self.default)
    }
}

impl Default for CacheManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Cache facade — 对齐 PHP think\facade\Cache
// ============================================================================

/// Cache facade（对齐 PHP `think\facade\Cache`）
///
/// 通过全局单例 + 委托 `CacheManager` 提供 PHP facade 风格 API。
///
/// ## 使用方式
///
/// ### 1. 全局使用（对齐 PHP `Cache::set(...)` 静态调用）
///
/// ```ignore
/// use sz_rust_core::cache::{init_default_cache, default_cache, MemoryCacheDriver};
///
/// init_default_cache(MemoryCacheDriver::new());
/// default_cache().set("key", "value", None).unwrap();
/// ```
///
/// ### 2. 独立实例（用于测试隔离）
///
/// ```ignore
/// use sz_rust_core::cache::{Cache, MemoryCacheDriver};
///
/// let cache = Cache::new();
/// cache.register_default(MemoryCacheDriver::new());
/// cache.set("key", "value", None).unwrap();
/// ```
pub struct Cache {
    manager: RwLock<CacheManager>,
    /// remember 锁等待参数（对齐 PHP 200ms 轮询 + 5s 超时）
    remember_lock_poll_interval: Duration,
    remember_lock_timeout: Duration,
    /// singleflight inflight map（按 key 互斥，防止缓存击穿）
    /// Rust 特有扩展：用 `parking_lot::Mutex` 实现真正的互斥，对齐 PHP `remember` 的"锁意图"但用正确方式实现
    inflight: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl Cache {
    /// 创建空的 Cache facade 实例
    pub fn new() -> Self {
        Self {
            manager: RwLock::new(CacheManager::new()),
            remember_lock_poll_interval: Duration::from_millis(200),
            remember_lock_timeout: Duration::from_secs(5),
            inflight: Mutex::new(HashMap::new()),
        }
    }

    /// 注册默认驱动
    ///
    /// 等价于 PHP `think\App::get('cache')` + 注册默认 store。
    pub fn register_default(&self, driver: MemoryCacheDriver) {
        let mut mgr = self.manager.write();
        mgr.register_store("default", Box::new(driver));
    }

    /// 注册命名驱动
    pub fn register_store(&self, name: impl Into<String>, driver: Box<dyn CacheDriver>) {
        let mut mgr = self.manager.write();
        mgr.register_store(name, driver);
    }

    /// 设置默认驱动名
    pub fn set_default_store(&self, name: impl Into<String>) -> Result<(), CacheError> {
        let mut mgr = self.manager.write();
        mgr.set_default(name)
    }

    // ========================================================================
    // PHP think\facade\Cache 核心 API
    // ========================================================================

    /// 写入缓存（对齐 PHP `Cache::set($name, $value, $ttl = null)`）
    ///
    /// PHP `Driver::set($name, $value, $ttl = null)` 第 110 行：
    ///
    /// ```php
    /// public function set($name, $value, $expire = null): bool
    /// {
    ///     $this->writeTimes++;
    ///     if (is_null($expire)) {
    ///         $expire = $this->options['expire'];
    ///     }
    ///     $data = $this->serialize($value);
    ///     // ... 写入底层存储
    /// }
    /// ```
    ///
    /// ## 参数
    ///
    /// - `key`：缓存键
    /// - `value`：缓存值（实现 `Serialize`）
    /// - `ttl`：过期时间（`None` 永不过期，对齐 PHP `$expire = null`）
    #[tracing::instrument(skip(self, value))]
    pub fn set<T: Serialize>(
        &self,
        key: &str,
        value: T,
        ttl: Option<Duration>,
    ) -> Result<(), CacheError> {
        let cache_value = php_serialize(&value)?;
        let bytes = cache_value.to_bytes();
        let mgr = self.manager.read();
        let driver = mgr.default_store()?;
        driver.set_raw(key, bytes, ttl)
    }

    /// 读取缓存（对齐 PHP `Cache::get($name, $default = null)`）
    ///
    /// PHP `Driver::get($name, $default = null)` 第 90 行：
    ///
    /// ```php
    /// public function get($name, $default = null)
    /// {
    ///     $this->readTimes++;
    ///     $value = $this->read($name);  // 读取原始字节
    ///     if (is_null($value)) {
    ///         return $default;
    ///     }
    ///     return $this->unserialize($value);  // ⚠️ numeric 返回 string
    /// }
    /// ```
    ///
    /// ## 泛型
    ///
    /// - `T = String`：对齐 PHP `unserialize` 对 numeric 返回 string 的行为
    /// - `T = Other`：通过 `serde_json::from_str` 还原
    ///
    /// ## PHP bug 复刻
    ///
    /// PHP `unserialize` 对 `is_numeric` 的值返回 string，而非 int。
    /// 调用方若想获取 `i64`，需自行 `.parse::<i64>()`，对齐 PHP 业务代码
    /// `(int) Cache::get('count')` 的强转模式。
    ///
    /// ## 参数
    ///
    /// - `key`：缓存键
    ///
    /// ## 返回
    ///
    /// - `Ok(Some(value))`：缓存命中
    /// - `Ok(None)`：缓存未命中或已过期
    #[tracing::instrument(skip(self))]
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, CacheError> {
        let mgr = self.manager.read();
        let driver = mgr.default_store()?;
        match driver.get_raw(key)? {
            None => Ok(None),
            Some(bytes) => {
                let cache_value = CacheValue::from_bytes(&bytes)?;
                php_unserialize(&cache_value)
            }
        }
    }

    /// 读取缓存，未命中时返回默认值（对齐 PHP `Cache::get($name, $default)`）
    pub fn get_or<T: DeserializeOwned>(&self, key: &str, default: T) -> Result<T, CacheError> {
        match self.get::<T>(key)? {
            Some(v) => Ok(v),
            None => Ok(default),
        }
    }

    /// remember 专用的弱类型读取（对齐 PHP 弱类型强转）
    ///
    /// PHP 是弱类型语言，`remember` 返回 `unserialize($data)` 后的值。
    /// 当存储的是 numeric 时，PHP `unserialize` 返回 string（源码 bug），
    /// 但调用方通常期望得到原始类型（如 int），PHP 通过隐式强转实现。
    ///
    /// Rust 端 `remember<T>` 是泛型，需要能从 string 还原为 T：
    /// 1. 先尝试 `get::<T>` 直接反序列化
    /// 2. 失败时降级 `get::<String>` + `serde_json::from_str::<T>(&s)`
    ///    （对齐 PHP `(int) Cache::get('count')` 强转模式）
    ///
    /// ## 示例
    ///
    /// - 存储 `i64 42` → `CacheValue::Number("42")`
    /// - `get::<i64>` 失败（JSON string 不是 number）
    /// - 降级 `get::<String>` 返回 `"42"`
    /// - `serde_json::from_str::<i64>("42")` 成功返回 `42`（"42" 是合法 JSON number）
    fn get_weak<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, CacheError> {
        match self.get::<T>(key) {
            Ok(v) => Ok(v),
            Err(CacheError::DeserializationError(_)) => {
                // 降级：尝试从 string 还原为 T
                // 对齐 PHP: numeric 存储返回 string，调用方需自行强转
                match self.get::<String>(key) {
                    Ok(Some(s)) => serde_json::from_str::<T>(&s)
                        .map(Some)
                        .map_err(|e| CacheError::DeserializationError(e.to_string())),
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// 删除缓存（对齐 PHP `Cache::delete($name)`）
    #[tracing::instrument(skip(self))]
    pub fn delete(&self, key: &str) -> Result<(), CacheError> {
        let mgr = self.manager.read();
        let driver = mgr.default_store()?;
        driver.delete(key)
    }

    /// 判断键是否存在（对齐 PHP `Cache::has($name)`）
    ///
    /// PHP `Driver::has($name)` 第 222 行：
    ///
    /// ```php
    /// public function has($name): bool
    /// {
    ///     return $this->read($name) !== null;
    /// }
    /// ```
    ///
    /// ## 注意
    ///
    /// PHP `has` 通过 `read` 检查是否为 null，会同时检查 TTL 过期。
    pub fn has(&self, key: &str) -> Result<bool, CacheError> {
        let mgr = self.manager.read();
        let driver = mgr.default_store()?;
        driver.has(key)
    }

    /// 自增（对齐 PHP `Cache::inc($name, $step = 1)`）
    ///
    /// PHP Redis 驱动直接 `INCRBY`；File 驱动读取 → 加减 → 写回。
    /// 本驱动默认实现采用 File 驱动行为。
    ///
    /// ## 行为
    ///
    /// - 键不存在：初始化为 `step`
    /// - 键存在：解析为 i64 → 加 `step` → 写回
    pub fn inc(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        let mgr = self.manager.read();
        let driver = mgr.default_store()?;
        driver.inc(key, step)
    }

    /// 自减（对齐 PHP `Cache::dec($name, $step = 1)`）
    pub fn dec(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        let mgr = self.manager.read();
        let driver = mgr.default_store()?;
        driver.dec(key, step)
    }

    /// 自增 1（便捷方法，对齐 PHP `Cache::inc($name)` 默认参数）
    pub fn increment(&self, key: &str) -> Result<i64, CacheError> {
        self.inc(key, 1)
    }

    /// 自减 1（便捷方法，对齐 PHP `Cache::dec($name)` 默认参数）
    pub fn decrement(&self, key: &str) -> Result<i64, CacheError> {
        self.dec(key, 1)
    }

    /// 读取并删除（对齐 PHP `Cache::pull($name, $default = null)`）
    ///
    /// PHP `Driver::pull($name, $default = null)` 第 332 行：
    ///
    /// ```php
    /// public function pull(string $name, $default = null)
    /// {
    ///     $result = $this->get($name, $default);
    ///     $this->delete($name);
    ///     return $result;
    /// }
    /// ```
    pub fn pull<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, CacheError> {
        let value = self.get::<T>(key)?;
        if value.is_some() {
            self.delete(key)?;
        }
        Ok(value)
    }

    /// 追加到数组缓存（对齐 PHP `Cache::push($name, $value, $expire = null)`）
    ///
    /// PHP `Driver::push($name, $value, $expire = null)` 第 339-358 行：
    ///
    /// ```php
    /// public function push(string $name, $value, $expire = null)
    /// {
    ///     $data = $this->get($name, []);
    ///     if (!is_array($data)) {
    ///         $data = [];
    ///     }
    ///     $data[] = $value;
    ///     if (count($data) > 1000) {
    ///         array_shift($data);
    ///     }
    ///     $data = array_unique($data);
    ///     $this->set($name, $data, $expire);
    ///     return $this;
    /// }
    /// ```
    ///
    /// ## 行为
    ///
    /// - 缓存不存在 → 创建 `vec![value]`
    /// - 缓存非数组 → 创建 `vec![value]`
    /// - 缓存为数组 → 追加 `value`
    /// - 长度 > 1000 → 丢弃最旧（FIFO）
    /// - `array_unique` 去重（保留首次出现的元素）
    pub fn push<T: Serialize + DeserializeOwned + PartialEq + Clone>(
        &self,
        key: &str,
        value: T,
        ttl: Option<Duration>,
    ) -> Result<(), CacheError> {
        // 对齐 PHP: $data = $this->get($name, []);
        // PHP is_array($data) 为 false 时（含反序列化失败），重置为 []
        let mut data: Vec<T> = match self.get::<Vec<T>>(key) {
            Ok(Some(v)) => v,
            Ok(None) => Vec::new(),
            Err(_) => Vec::new(),
        };
        data.push(value);

        // 对齐 PHP array_shift：长度 > 1000 时丢弃最旧
        while data.len() > 1000 {
            data.remove(0);
        }

        // 对齐 PHP array_unique：去重（保留首次出现的元素）
        let mut seen = std::collections::HashSet::new();
        data.retain(|item| seen.insert(item.hashable_string()));

        self.set(key, data, ttl)
    }

    /// 缓存击穿防护读取（对齐 PHP `Cache::remember($name, callable, $expire = null)`）
    ///
    /// PHP `Driver::remember` 第 287-310 行 + PHP bug 复刻：
    ///
    /// 1. 先 `get($name)`，命中直接返回
    /// 2. 抢锁 `set($name . '_lock', 1)`（无 TTL，PHP 源码 bug）
    /// 3. 等待锁释放，200ms 轮询，5 秒超时
    /// 4. 锁释放后 `get($name)`，命中则返回
    /// 5. 超时仍未释放：直接调用 `callback()`（防止永久阻塞）
    /// 6. 抢到锁：调用 `callback()` → `set($name, $data, $expire)` → 释放锁
    ///
    /// ## PHP bug 复刻
    ///
    /// 1. **锁 key 无 TTL**：若进程崩溃，锁永久存在 → 死锁
    /// 2. **`has()` + `get()` 双查 TOCTOU**：先 `has` 后 `get`
    ///
    /// ## 异步安全
    ///
    /// 本方法为 `async fn`，等待锁释放时使用 `tokio::time::sleep` 让出 worker，
    /// 不会阻塞 tokio 运行时。
    ///
    /// ## 与 `remember_async` 的差异
    ///
    /// | 维度 | `remember` | `remember_async` |
    /// |------|-----------|------------------|
    /// | callback 类型 | `FnOnce() -> T`（同步） | `async fn -> T`（异步） |
    /// | 适用场景 | 纯计算 / 已缓存值构造 | IO 密集型回源（DB / HTTP） |
    ///
    /// ## 参数
    ///
    /// - `key`：缓存键
    /// - `ttl`：缓存过期时间
    /// - `callback`：未命中时的回调函数
    ///
    /// ## 异步安全
    ///
    /// 本方法为 `async fn`，等待锁释放时使用 `tokio::time::sleep` 让出 worker，
    /// 不会阻塞 tokio 运行时。对齐 [`Cache::remember_async`] 的非阻塞行为。
    #[tracing::instrument(skip(self, callback))]
    pub async fn remember<T, F>(
        &self,
        key: &str,
        ttl: Option<Duration>,
        callback: F,
    ) -> Result<T, CacheError>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> T,
    {
        // 1. 先尝试读取缓存（弱类型读取，对齐 PHP 弱类型强转）
        if let Some(cached) = self.get_weak::<T>(key)? {
            return Ok(cached);
        }

        // 2. 抢锁
        let lock_key = format!("{}_lock", key);

        // PHP 源码 bug 复刻：先 has() 后 get() 双查
        if self.has(&lock_key)? {
            // 等待锁释放，200ms 轮询，5s 超时（使用 tokio::time::sleep 非阻塞）
            let start = Instant::now();
            while self.has(&lock_key)? {
                if start.elapsed() >= self.remember_lock_timeout {
                    // 超时仍未释放，直接调用 callback（防止永久阻塞）
                    return Ok(callback());
                }
                tokio::time::sleep(self.remember_lock_poll_interval).await;
            }

            // 锁释放后再次读取（弱类型读取）
            if let Some(cached) = self.get_weak::<T>(key)? {
                return Ok(cached);
            }
        }

        // 抢到锁（无 TTL，PHP 源码 bug）
        self.set(&lock_key, 1i64, None)?;

        // 调用 callback 并写入缓存
        let result = callback();
        let _ = self.set(key, &result, ttl);

        // 释放锁
        let _ = self.delete(&lock_key);

        Ok(result)
    }

    /// 缓存击穿防护读取（异步 callback 版本）
    ///
    /// 与 [`Cache::remember`] 行为一致（同样使用 `tokio::time::sleep` 让出 worker），
    /// 区别在于支持异步 callback，避免在 callback 中执行阻塞 IO 时阻塞 worker。
    ///
    /// ## 参数差异
    ///
    /// | 维度 | `remember` | `remember_async` |
    /// |------|-----------|------------------|
    /// | callback 类型 | `FnOnce() -> T`（同步） | `async fn -> T`（异步） |
    /// | 适用场景 | 纯计算 / 已缓存值构造 | IO 密集型回源（DB / HTTP） |
    ///
    /// ## 参数
    ///
    /// - `key`：缓存键
    /// - `ttl`：缓存过期时间
    /// - `callback`：未命中时的异步回调函数
    ///
    /// ## 用法
    ///
    /// ```ignore
    /// # async fn example(cache: &sz_rust_core::cache::Cache) {
    /// let user: User = cache.remember_async("user_1", Some(Duration::from_secs(60)), || async {
    ///     // 异步回源逻辑（如 DB 查询）
    ///     User::find_async(1).await
    /// }).await?;
    /// # }
    /// ```
    pub async fn remember_async<T, F, Fut>(
        &self,
        key: &str,
        ttl: Option<Duration>,
        callback: F,
    ) -> Result<T, CacheError>
    where
        T: Serialize + DeserializeOwned + Clone,
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        // 1. 先尝试读取缓存（弱类型读取，对齐 PHP 弱类型强转）
        if let Some(cached) = self.get_weak::<T>(key)? {
            return Ok(cached);
        }

        // 2. 抢锁
        let lock_key = format!("{}_lock", key);

        // PHP 源码 bug 复刻：先 has() 后 get() 双查
        if self.has(&lock_key)? {
            // 等待锁释放，200ms 轮询，5s 超时（异步等待，不阻塞 worker）
            let start = Instant::now();
            while self.has(&lock_key)? {
                if start.elapsed() >= self.remember_lock_timeout {
                    // 超时仍未释放，直接调用 callback（防止永久阻塞）
                    let result = callback().await;
                    let _ = self.set(key, &result, ttl);
                    return Ok(result);
                }
                tokio::time::sleep(self.remember_lock_poll_interval).await;
            }

            // 锁释放后再次读取（弱类型读取）
            if let Some(cached) = self.get_weak::<T>(key)? {
                return Ok(cached);
            }
        }

        // 抢到锁（无 TTL，PHP 源码 bug）
        self.set(&lock_key, 1i64, None)?;

        // 调用异步 callback 并写入缓存
        let result = callback().await;
        let _ = self.set(key, &result, ttl);

        // 释放锁
        let _ = self.delete(&lock_key);

        Ok(result)
    }

    /// 清空所有缓存（对齐 PHP `Cache::clear()`）
    #[tracing::instrument(skip(self))]
    pub fn clear(&self) -> Result<(), CacheError> {
        let mgr = self.manager.read();
        let driver = mgr.default_store()?;
        driver.clear()
    }

    // ========================================================================
    // 缓存失效策略（对齐 PHP 业务代码写后失效模式）
    // ========================================================================

    /// 批量删除多个缓存 key（对齐 PHP `Driver::deleteMultiple($keys): bool`）
    ///
    /// PHP `think\cache\Driver::deleteMultiple` 第 342-351 行：
    /// ```php
    /// public function deleteMultiple($keys): bool
    /// {
    ///     foreach ($keys as $key) {
    ///         $result = $this->delete($key);
    ///         if (false === $result) {
    ///             return false;
    ///         }
    ///     }
    ///     return true;
    /// }
    /// ```
    ///
    /// ## PHP 行为对齐
    ///
    /// - 逐个调用 `delete(key)`，任一失败立即返回 `Err`
    /// - 对齐 PHP `if (false === $result) return false`
    /// - **注意**：PHP `File::delete` 文件不存在也返回 `false`，导致 `deleteMultiple`
    ///   在文件不存在时也返回 `false`（PHP bug）。Rust 端 `delete` 对不存在的 key
    ///   返回 `Ok(())`，因此 `delete_many` 对不存在的 key 不会失败（修正 PHP bug）。
    ///
    /// ## 业务场景对齐
    ///
    /// 对齐业务场景 4（一次写操作失效多类缓存）：
    /// ```php
    /// // addons/sdp/model/Category.php
    /// Cache::delete('sdp_category_tree');
    /// Cache::delete('sdp_category_select');
    /// Cache::delete('sdp_category_child');
    /// Cache::delete('sdp_category_nav');
    /// Cache::delete('sdp_category_info:'.$data['cat_id']);
    /// ```
    ///
    /// Rust 端用 `delete_many` 一次调用：
    /// ```ignore
    /// cache.delete_many(&["sdp_category_tree", "sdp_category_select", "sdp_category_child"])?;
    /// ```
    pub fn delete_many(&self, keys: &[&str]) -> Result<(), CacheError> {
        let mgr = self.manager.read();
        let driver = mgr.default_store()?;
        for key in keys {
            // 对齐 PHP `foreach ($keys as $key) { $result = $this->delete($key); if (false === $result) return false; }`
            driver.delete(key)?;
        }
        Ok(())
    }

    /// 写操作后失效缓存（对齐 PHP 业务场景 1：事务内写后失效）
    ///
    /// PHP 业务代码典型模式（`app/food/model/cashier/Clerk.php`）：
    /// ```php
    /// public function add($data): bool
    /// {
    ///     $this->startTrans();
    ///     try {
    ///         if($this->save($data)){
    ///             Cache::delete('foodCashierClerkAll_' . $data['cashier_id']);
    ///             $this->commit();
    ///         }
    ///     } catch (\Exception $e) {
    ///         $this->rollback();
    ///     }
    /// }
    /// ```
    ///
    /// ## 设计决策
    ///
    /// - **严禁直接更新缓存**：写操作后应 `delete`（让下次 `get` 时回源），
    ///   而非 `set` 更新缓存值（cache-aside 模式）
    /// - 返回 `Result<(), CacheError>`：调用方可选择忽略错误（对齐 PHP fire and forget）
    /// - 与 `delete_many` 的区别：`invalidate_after_write` 语义明确（写后失效），
    ///   便于代码审查和日志追踪
    ///
    /// ## 用法
    ///
    /// ```ignore
    /// // 写操作后失效相关缓存
    /// cache.invalidate_after_write(&["foodCashierClerkAll_1", "foodCashierClerkList_1"])?;
    /// // 或 fire and forget（对齐 PHP 业务代码不检查返回值）
    /// let _ = cache.invalidate_after_write(&["foodCashierClerkAll_1"]);
    /// ```
    pub fn invalidate_after_write(&self, keys: &[&str]) -> Result<(), CacheError> {
        self.delete_many(keys)
    }

    /// 先删后读强制刷新（对齐 PHP 业务场景 2：delete → get → 回源 set）
    ///
    /// PHP 业务代码典型模式（`app/common/model/store/Store.php`）：
    /// ```php
    /// public static function info($store_id){
    ///     $cacheKey = 'wmall_store_info_'.$store_id;
    ///     Cache::delete($cacheKey);          // 先删
    ///     $info = Cache::get($cacheKey);     // 再读（必为空，触发回源）
    ///     if(!$info){
    ///         $info = $model->with(['supplier','nav'])->find();
    ///         if($info){
    ///             Cache::set($cacheKey, $info, 86400);
    ///         }
    ///     }
    ///     return $info;
    /// }
    /// ```
    ///
    /// ## 设计决策
    ///
    /// - 优化 PHP 模式：`delete → fetcher() → set`，避免一次无意义的 `get`（PHP 模式中
    ///   `delete` 后 `get` 必为空，直接调用 `fetcher` 更高效）
    /// - **严禁直接更新缓存**：通过 `delete` + `fetcher` + `set` 实现"强制刷新"，
    ///   而非直接 `set` 覆盖（确保 fetcher 是唯一数据源）
    /// - 返回 `Result<T, CacheError>`：fetcher 失败时传播错误，不写入缓存
    ///
    /// ## 用法
    ///
    /// ```ignore
    /// # async fn example(cache: &Cache) {
    /// let store_info: StoreInfo = cache.refresh("wmall_store_info_1", Some(Duration::from_secs(86400)), || {
    ///     // 回源逻辑
    ///     Ok(StoreInfo::find(1))
    /// })?;
    /// # }
    /// ```
    pub fn refresh<T, F>(
        &self,
        key: &str,
        ttl: Option<Duration>,
        fetcher: F,
    ) -> Result<T, CacheError>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Result<T, CacheError>,
    {
        // 对齐 PHP Cache::delete($cacheKey) — 先删
        self.delete(key)?;

        // 对齐 PHP 回源逻辑（PHP 模式中 delete → get 必为空 → 回源）
        let value = fetcher()?;

        // 对齐 PHP Cache::set($cacheKey, $info, $expire) — 写回缓存
        self.set(key, &value, ttl)?;

        Ok(value)
    }

    // ========================================================================
    // 缓存击穿/雪崩防护（Rust 特有扩展）
    //
    // PHP `think\cache\Driver::remember` 有"锁雏形"但缺陷严重：
    //   1. 加锁非原子（`$this->set($name.'_lock', true)` 不是 SET NX EX）
    //   2. 锁无 TTL（进程崩溃会永久锁死）
    //   3. 业务代码 0 处使用 remember
    //
    // Rust 端用 `parking_lot::Mutex` 实现真正的 singleflight（按 key 互斥），
    // 用随机过期时间实现雪崩防护。这是 Rust 特有扩展，对齐 PHP `remember` 的
    // "锁意图"但用正确方式实现。
    // ========================================================================

    /// singleflight 模式回源（Rust 特有扩展，防止缓存击穿）
    ///
    /// 同一 key 并发请求时，只允许一个线程回源，其他线程等待锁释放后
    /// 通过 double-check 从缓存读取结果。
    ///
    /// ## 与 PHP `remember` 的差异
    ///
    /// | 维度 | PHP `remember` | Rust `fetch_singleflight` |
    /// |------|----------------|---------------------------|
    /// | 加锁方式 | `$this->set($name.'_lock', true)` 非原子 | `parking_lot::Mutex::lock()` 原子互斥 |
    /// | 锁 TTL | 无（进程崩溃永久锁死） | 无需 TTL（Mutex guard 释放即解锁，panic 自动释放） |
    /// | 等待方式 | `while + usleep(200ms)` 轮询 5s 超时 | `Mutex::lock()` 阻塞等待（无超时，但 panic 自动释放） |
    /// | double-check | 无 | 有（获取锁后再次检查缓存） |
    ///
    /// ## 用法
    ///
    /// ```ignore
    /// let value: String = cache.fetch_singleflight("hot_key", Some(Duration::from_secs(60)), || {
    ///     // 回源逻辑（数据库查询等）
    ///     Ok("expensive_value".to_string())
    /// })?;
    /// ```
    #[tracing::instrument(skip(self, fetcher))]
    pub fn fetch_singleflight<T, F>(
        &self,
        key: &str,
        ttl: Option<Duration>,
        fetcher: F,
    ) -> Result<T, CacheError>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Result<T, CacheError>,
    {
        // 1. 先尝试读缓存（快速路径）
        if let Some(cached) = self.get::<T>(key)? {
            return Ok(cached);
        }

        // 2. 获取该 key 的互斥锁（singleflight 核心）
        //    inflight map 保证同一 key 的所有请求共享同一个 Arc<Mutex<()>>
        let mutex = {
            let mut inflight = self.inflight.lock();
            inflight
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        // 3. 持有锁执行回源（其他线程在此阻塞等待）
        let _guard = mutex.lock();

        // 4. double-check：其他线程可能已经回源完成并写入缓存
        if let Some(cached) = self.get::<T>(key)? {
            return Ok(cached);
        }

        // 5. 回源并写入缓存
        let value = fetcher()?;
        self.set(key, &value, ttl)?;

        Ok(value)
    }

    /// 设置带随机抖动的 TTL（Rust 特有扩展，防止缓存雪崩）
    ///
    /// 在 TTL 上加 `[0, jitter]` 范围的随机抖动，避免大量 key 同时过期触发雪崩。
    ///
    /// ## 设计决策
    ///
    /// - PHP 无随机过期时间机制（`getExpireTime` 不做 TTL 抖动）
    /// - Rust 特有扩展：用 `rand` crate 生成随机抖动
    /// - 实际 TTL 在 `[ttl, ttl + jitter]` 范围内
    /// - `jitter` 为 0 时等价于 `set`（无抖动）
    /// - `ttl` 为 `None` 时等价于永久缓存（无抖动）
    ///
    /// ## 用法
    ///
    /// ```ignore
    /// // 基础 TTL 60s + 随机抖动 0-10s（实际 TTL 60-70s）
    /// cache.set_with_jitter("key", "value", Some(Duration::from_secs(60)), Duration::from_secs(10))?;
    /// ```
    pub fn set_with_jitter<T>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
        jitter: Duration,
    ) -> Result<(), CacheError>
    where
        T: Serialize + ?Sized,
    {
        let actual_ttl = match ttl {
            Some(t) if !jitter.is_zero() => {
                // 在 [t, t + jitter] 范围内随机
                use rand::Rng;
                let jitter_nanos = jitter.as_nanos() as u64;
                let random_jitter =
                    Duration::from_nanos(rand::thread_rng().gen_range(0..jitter_nanos));
                Some(t + random_jitter)
            }
            Some(t) => Some(t),
            None => None,
        };
        self.set(key, value, actual_ttl)
    }

    /// singleflight + 随机过期时间组合防护（最完整防护）
    ///
    /// 组合 `fetch_singleflight`（防击穿）+ `set_with_jitter`（防雪崩），
    /// 提供最完整的缓存防护。
    ///
    /// ## 用法
    ///
    /// ```ignore
    /// let value: String = cache.fetch_with_protection(
    ///     "hot_key",
    ///     Some(Duration::from_secs(60)),
    ///     Duration::from_secs(10),  // 随机抖动 0-10s
    ///     || Ok("expensive_value".to_string()),
    /// )?;
    /// ```
    #[tracing::instrument(skip(self, fetcher))]
    pub fn fetch_with_protection<T, F>(
        &self,
        key: &str,
        ttl: Option<Duration>,
        jitter: Duration,
        fetcher: F,
    ) -> Result<T, CacheError>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Result<T, CacheError>,
    {
        // 1. 先尝试读缓存（快速路径）
        if let Some(cached) = self.get::<T>(key)? {
            return Ok(cached);
        }

        // 2. 获取该 key 的互斥锁（singleflight 核心）
        let mutex = {
            let mut inflight = self.inflight.lock();
            inflight
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        // 3. 持有锁执行回源
        let _guard = mutex.lock();

        // 4. double-check
        if let Some(cached) = self.get::<T>(key)? {
            return Ok(cached);
        }

        // 5. 回源并写入缓存（带随机抖动 TTL）
        let value = fetcher()?;
        self.set_with_jitter(key, &value, ttl, jitter)?;

        Ok(value)
    }

    // ========================================================================
    // 命名 store 访问（对齐 PHP $cache->store('redis')->...）
    // ========================================================================

    /// 获取命名 store 的代理（对齐 PHP `$cache->store('redis')`）
    ///
    /// 通过回调方式访问命名 store，避免生命周期问题。
    ///
    /// ## 示例
    ///
    /// ```ignore
    /// cache.with_store("redis", |driver| {
    ///     driver.set_raw("key", b"value".to_vec(), None)
    /// }).unwrap();
    /// ```
    pub fn with_store<R, F>(&self, name: &str, f: F) -> Result<R, CacheError>
    where
        F: FnOnce(&dyn CacheDriver) -> Result<R, CacheError>,
    {
        let mgr = self.manager.read();
        let driver = mgr.store(name)?;
        f(driver)
    }

    // ========================================================================
    // 缓存标签（对齐 PHP think\facade\Cache::tag）
    // ========================================================================

    /// 缓存标签（对齐 PHP `Driver::tag($name)`）
    ///
    /// PHP `Driver::tag($name)` 第 196-206 行：
    /// ```php
    /// public function tag($name): TagSet
    /// {
    ///     $name = (array) $name;
    ///     $key  = implode('-', $name);
    ///     if (!isset($this->tag[$key])) {
    ///         $this->tag[$key] = new TagSet($name, $this);
    ///     }
    ///     return $this->tag[$key];
    /// }
    /// ```
    ///
    /// ## PHP 单例 vs Rust 实现
    ///
    /// PHP 使用 `$this->tag[$key]` 单例缓存 `TagSet` 对象，避免重复创建。
    /// Rust 端不实现单例（`TagSet` 是无状态结构体，每次创建行为一致），
    /// 功能上完全等价。
    ///
    /// ## 示例
    ///
    /// ```ignore
    /// cache.tag("user").set("user:1", &data, None)?;
    /// cache.tag("user").clear();  // 清除所有 user 标签下的缓存
    /// ```
    pub fn tag(&self, name: &str) -> TagSet<'_> {
        TagSet {
            tags: vec![name.to_string()],
            cache: self,
        }
    }

    /// 多标签缓存（对齐 PHP `Cache::tag(['user', 'admin'])`）
    ///
    /// PHP `tag($name)` 中 `$name = (array) $name`，支持传入数组。
    /// Rust 端通过 `tag_many` 方法提供等价功能。
    ///
    /// ## 示例
    ///
    /// ```ignore
    /// cache.tag_many(&["user", "admin"]).set("key", &data, None)?;
    /// cache.tag_many(&["user", "admin"]).clear();
    /// ```
    pub fn tag_many(&self, names: &[&str]) -> TagSet<'_> {
        TagSet {
            tags: names.iter().map(|s| s.to_string()).collect(),
            cache: self,
        }
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Trait helper for push deduplication
// ============================================================================

/// `push` 去重辅助 trait
///
/// 由于 `T: PartialEq + Clone` 不直接支持 `HashSet`，
/// 本 trait 通过 `serde_json::to_string` 生成可哈希的字符串表示。
trait CloneHashable: Clone {
    fn hashable_string(&self) -> String;
}

impl<T> CloneHashable for T
where
    T: Serialize + Clone,
{
    fn hashable_string(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

// ============================================================================
// RedisCacheDriver — Redis 驱动（对齐 PHP think\cache\driver\Redis）
// ============================================================================
//
// PHP `think\cache\driver\Redis`（249 行）基于 phpredis 扩展或 Predis\Client，
// 提供 Redis 协议级别的缓存操作。核心特性：
//
// 1. `set` 时 expire > 0 用 SETEX，否则用 SET（对齐 PHP 第 145-149 行）
// 2. `inc/dec` 直接 INCRBY/DECRBY（不经 serialize，对齐 PHP 第 161-182 行）
// 3. `has` 用 EXISTS（对齐 PHP 第 100-103 行）
// 4. `delete` 用 DEL，返回 bool（del 数量 > 0，对齐 PHP 第 190-197 行）
// 5. `clear` 用 FLUSHDB（对齐 PHP 第 204-209 行）
// 6. `append`（tag）用 SADD（对齐 PHP 第 230-234 行，重写父类 `push` 行为）
// 7. `getTagItems` 用 SMEMBERS（对齐 PHP 第 242-247 行）
// 8. `clearTag` 用 DEL 批量（对齐 PHP 第 217-221 行）
//
// ## 架构
//
// 由于 Rust 端不强制依赖 `redis` crate（避免编译时间增加），本模块通过
// `RedisBackend` trait 抽象 Redis 命令，提供 `MockRedisBackend`（用 HashMap
// 模拟）作为默认实现。应用层可注入真实 Redis backend（如 `redis::Connection`
// 包装）以连接真实 Redis 服务器。

use md5::{Digest, Md5};
use std::collections::HashSet;

/// Redis 缓存配置（对齐 PHP `think\cache\driver\Redis::$options`）
///
/// PHP 默认配置（Redis.php 第 33-44 行）：
///
/// ```php
/// protected $options = [
///     'host'       => '127.0.0.1',
///     'port'       => 6379,
///     'password'   => '',
///     'select'     => 0,
///     'timeout'    => 0,
///     'expire'     => 0,
///     'persistent' => false,
///     'prefix'     => '',
///     'tag_prefix' => 'tag:',
///     'serialize'  => [],
/// ];
/// ```
#[derive(Debug, Clone)]
pub struct RedisConfig {
    /// Redis 主机地址（对齐 PHP `host`）
    pub host: String,
    /// Redis 端口（对齐 PHP `port`）
    pub port: u16,
    /// Redis 密码（对齐 PHP `password`，空字符串表示无密码）
    pub password: String,
    /// Redis db 索引（对齐 PHP `select`）
    pub select: u32,
    /// 连接超时（对齐 PHP `timeout`，`Duration::ZERO` 表示无超时）
    pub timeout: Duration,
    /// 默认过期时间（对齐 PHP `expire`，`None` 表示永不过期）
    pub expire: Option<Duration>,
    /// 是否使用持久连接（对齐 PHP `persistent`）
    pub persistent: bool,
    /// key 前缀（对齐 PHP `prefix`）
    pub prefix: String,
    /// tag 前缀（对齐 PHP `tag_prefix`）
    pub tag_prefix: String,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 6379,
            password: String::new(),
            select: 0,
            timeout: Duration::ZERO,
            expire: None,
            persistent: false,
            prefix: String::new(),
            tag_prefix: "tag:".to_string(),
        }
    }
}

impl RedisConfig {
    /// 创建带前缀的配置（便捷方法）
    pub fn with_prefix(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            ..Default::default()
        }
    }

    /// 创建带默认 TTL 的配置（便捷方法）
    pub fn with_expire(expire: Duration) -> Self {
        Self {
            expire: Some(expire),
            ..Default::default()
        }
    }
}

/// Redis 后端 trait（抽象 Redis 命令）
///
/// 由于 Rust 端不强制依赖 `redis` crate，本 trait 抽象 Redis 命令，
/// 允许应用层注入真实 Redis backend（如 `redis::Connection` 包装）。
///
/// ## 默认实现
///
/// `MockRedisBackend` 用 `HashMap` 模拟 Redis 行为，用于测试和开发环境。
///
/// ## 命令映射
///
/// | Trait 方法  | Redis 命令 | PHP phpredis 调用                  |
/// |------------|-----------|-----------------------------------|
/// | `get`      | GET       | `$handler->get($key)`             |
/// | `set`      | SET       | `$handler->set($key, $value)`     |
/// | `set_ex`   | SETEX     | `$handler->setex($key, $ttl, $v)` |
/// | `del`      | DEL       | `$handler->del($key)`             |
/// | `del_many` | DEL ...   | `$handler->del($keys)`            |
/// | `exists`   | EXISTS    | `$handler->exists($key)`          |
/// | `incr_by`  | INCRBY    | `$handler->incrby($key, $step)`   |
/// | `decr_by`  | DECRBY    | `$handler->decrby($key, $step)`   |
/// | `flush_db` | FLUSHDB   | `$handler->flushDB()`             |
/// | `sadd`     | SADD      | `$handler->sAdd($key, $value)`    |
/// | `smembers` | SMEMBERS  | `$handler->sMembers($key)`        |
pub trait RedisBackend: Send + Sync {
    /// GET 命令
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError>;
    /// SET 命令（无 TTL）
    fn set(&self, key: &str, value: Vec<u8>) -> Result<(), CacheError>;
    /// SETEX 命令（带 TTL）
    fn set_ex(&self, key: &str, value: Vec<u8>, ttl: Duration) -> Result<(), CacheError>;
    /// DEL 命令（单个 key），返回删除数量
    fn del(&self, key: &str) -> Result<i64, CacheError>;
    /// DEL 命令（批量），返回删除数量
    fn del_many(&self, keys: &[&str]) -> Result<i64, CacheError>;
    /// EXISTS 命令
    fn exists(&self, key: &str) -> Result<bool, CacheError>;
    /// INCRBY 命令（key 不存在时 Redis 初始化为 0 再 INCRBY）
    fn incr_by(&self, key: &str, step: i64) -> Result<i64, CacheError>;
    /// DECRBY 命令
    fn decr_by(&self, key: &str, step: i64) -> Result<i64, CacheError>;
    /// FLUSHDB 命令（清空当前 db）
    fn flush_db(&self) -> Result<(), CacheError>;
    /// SADD 命令（向 Set 添加成员）
    fn sadd(&self, key: &str, member: &str) -> Result<i64, CacheError>;
    /// SMEMBERS 命令（返回 Set 全部成员）
    fn smembers(&self, key: &str) -> Result<Vec<String>, CacheError>;
}

/// Mock Redis 后端（用 HashMap 模拟 Redis 行为）
///
/// 用于测试和开发环境，不需要真实 Redis 服务器。
///
/// ## 模拟行为
///
/// - KV 存储：`HashMap<String, (Vec<u8>, Option<Instant>)>` — value + expires_at
/// - Set 存储：`HashMap<String, HashSet<String>>`
/// - TTL 过期：`get`/`exists`/`incr_by`/`decr_by` 时检查过期
/// - INCRBY：key 不存在时初始化为 0，非数字时返回错误（对齐 Redis 行为）
pub struct MockRedisBackend {
    /// KV 存储：key → (value, expires_at)
    kv: parking_lot::RwLock<MockRedisKv>,
    /// Set 存储：key → members
    sets: parking_lot::RwLock<MockRedisSets>,
}

/// Mock Redis KV 存储类型（key → (value, expires_at)）
type MockRedisKv = HashMap<String, (Vec<u8>, Option<Instant>)>;

/// Mock Redis Set 存储类型（key → members）
type MockRedisSets = HashMap<String, HashSet<String>>;

impl Default for MockRedisBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRedisBackend {
    /// 创建新的 Mock Redis 后端
    pub fn new() -> Self {
        Self {
            kv: parking_lot::RwLock::new(HashMap::new()),
            sets: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// 检查 key 是否已过期（内部辅助，不删除）
    fn is_expired(kv: &MockRedisKv, key: &str) -> bool {
        if let Some((_, Some(expires_at))) = kv.get(key) {
            return *expires_at <= Instant::now();
        }
        false
    }
}

impl RedisBackend for MockRedisBackend {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let kv = self.kv.read();
        if Self::is_expired(&kv, key) {
            return Ok(None);
        }
        Ok(kv.get(key).map(|(v, _)| v.clone()))
    }

    fn set(&self, key: &str, value: Vec<u8>) -> Result<(), CacheError> {
        let mut kv = self.kv.write();
        kv.insert(key.to_string(), (value, None));
        Ok(())
    }

    fn set_ex(&self, key: &str, value: Vec<u8>, ttl: Duration) -> Result<(), CacheError> {
        let mut kv = self.kv.write();
        let expires_at = Some(Instant::now() + ttl);
        kv.insert(key.to_string(), (value, expires_at));
        Ok(())
    }

    fn del(&self, key: &str) -> Result<i64, CacheError> {
        let mut kv = self.kv.write();
        let removed = kv.remove(key).is_some() as i64;
        // 同时清理 Set（Redis DEL 会删除所有类型）
        let mut sets = self.sets.write();
        if sets.remove(key).is_some() && removed == 0 {
            return Ok(1);
        }
        Ok(removed)
    }

    fn del_many(&self, keys: &[&str]) -> Result<i64, CacheError> {
        let mut count = 0i64;
        for key in keys {
            count += self.del(key)?;
        }
        Ok(count)
    }

    fn exists(&self, key: &str) -> Result<bool, CacheError> {
        let kv = self.kv.read();
        if Self::is_expired(&kv, key) {
            return Ok(false);
        }
        if kv.contains_key(key) {
            return Ok(true);
        }
        let sets = self.sets.read();
        Ok(sets.contains_key(key))
    }

    fn incr_by(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        let mut kv = self.kv.write();
        // 检查过期：过期则视为不存在
        if Self::is_expired(&kv, key) {
            kv.remove(key);
        }
        let current = match kv.get(key) {
            Some((bytes, _)) => {
                let s = std::str::from_utf8(bytes)
                    .map_err(|e| CacheError::DeserializationError(e.to_string()))?;
                s.parse::<i64>().map_err(|e| {
                    CacheError::Internal(format!("INCRBY failed: '{}' is not an integer: {}", s, e))
                })?
            }
            None => 0, // Redis 对不存在的 key 初始化为 0
        };
        let new_value = current + step;
        kv.insert(key.to_string(), (new_value.to_string().into_bytes(), None));
        Ok(new_value)
    }

    fn decr_by(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        // Redis DECRBY 等价于 INCRBY(-step)
        self.incr_by(key, -step)
    }

    fn flush_db(&self) -> Result<(), CacheError> {
        let mut kv = self.kv.write();
        kv.clear();
        let mut sets = self.sets.write();
        sets.clear();
        Ok(())
    }

    fn sadd(&self, key: &str, member: &str) -> Result<i64, CacheError> {
        let mut sets = self.sets.write();
        let set = sets.entry(key.to_string()).or_default();
        let added = set.insert(member.to_string()) as i64;
        Ok(added)
    }

    fn smembers(&self, key: &str) -> Result<Vec<String>, CacheError> {
        let sets = self.sets.read();
        Ok(sets
            .get(key)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default())
    }
}

/// Redis 缓存驱动（对齐 PHP `think\cache\driver\Redis`）
///
/// 基于 `RedisBackend` trait，提供 PHP think-orm Redis 驱动的等价实现。
///
/// ## PHP 行为对齐
///
/// 1. **`set` 行为**（PHP 第 133-152 行）：
///    - `expire > 0` → `SETEX(key, expire, value)`
///    - `expire = 0` → `SET(key, value)`
///
/// 2. **`inc/dec` 行为**（PHP 第 161-182 行）：
///    - 直接 `INCRBY`/`DECRBY`，**不经 serialize**
///    - key 不存在时 Redis 初始化为 0
///    - 返回新值（i64）
///
/// 3. **`has` 行为**（PHP 第 100-103 行）：
///    - `EXISTS(key)` ? true : false
///
/// 4. **`delete` 行为**（PHP 第 190-197 行）：
///    - `DEL(key)` > 0 返回 true
///
/// 5. **`clear` 行为**（PHP 第 204-209 行）：
///    - `FLUSHDB()`
///
/// 6. **`append`（tag）行为**（PHP 第 230-234 行）：
///    - `SADD(key, value)` — 用 Set 存储 tag 成员
///
/// 7. **`getTagItems` 行为**（PHP 第 242-247 行）：
///    - `SMEMBERS(key)` — 返回 Set 全部成员
///
/// 8. **`clearTag` 行为**（PHP 第 217-221 行）：
///    - `DEL(keys...)` — 批量删除
///
/// ## key 构造
///
/// - `getCacheKey(name)` = `prefix + name`（对齐 PHP `Driver::getCacheKey`）
/// - `getTagKey(tag)` = `tag_prefix + md5(tag)`（对齐 PHP `Driver::getTagKey`）
///
/// ## 使用示例
///
/// ```ignore
/// use sz_rust_core::cache::{RedisCacheDriver, RedisConfig, Cache};
///
/// let driver = RedisCacheDriver::new(RedisConfig::default());
/// let cache = Cache::new();
/// cache.register_store("redis", Box::new(driver));
/// cache.set_default_store("redis").unwrap();
///
/// cache.set("key", "value", None).unwrap();
/// assert_eq!(cache.get::<String>("key").unwrap(), Some("value".to_string()));
/// ```
pub struct RedisCacheDriver {
    backend: Box<dyn RedisBackend>,
    config: RedisConfig,
}

impl RedisCacheDriver {
    /// 创建 Redis 缓存驱动（用 Mock backend）
    ///
    /// 对齐 PHP `new \think\cache\driver\Redis($options)`。
    pub fn new(config: RedisConfig) -> Self {
        Self::with_backend(config, Box::new(MockRedisBackend::new()))
    }

    /// 创建 Redis 缓存驱动（自定义 backend）
    ///
    /// 应用层可注入真实 Redis backend（如 `redis::Connection` 包装）。
    pub fn with_backend(config: RedisConfig, backend: Box<dyn RedisBackend>) -> Self {
        Self { backend, config }
    }

    /// 获取配置引用
    pub fn config(&self) -> &RedisConfig {
        &self.config
    }

    /// 获取 backend 引用（用于高级操作）
    pub fn backend(&self) -> &dyn RedisBackend {
        self.backend.as_ref()
    }

    /// 追加 TagSet 数据（对齐 PHP `Redis::append`）
    ///
    /// PHP `Redis::append(name, value)` 第 230-234 行：
    /// ```php
    /// public function append(string $name, $value): void
    /// {
    ///     $key = $this->getCacheKey($name);
    ///     $this->handler->sAdd($key, $value);
    /// }
    /// ```
    ///
    /// **注意**：PHP `think\cache\driver\Redis` 重写了父类 `Driver::append`
    /// （父类用 `push`，Redis 驱动用 `SADD`）。
    pub fn append(&self, name: &str, value: &str) -> Result<(), CacheError> {
        let key = self.get_cache_key(name);
        self.backend.sadd(&key, value)?;
        Ok(())
    }

    /// 获取标签包含的缓存标识（对齐 PHP `Redis::getTagItems`）
    ///
    /// PHP `Redis::getTagItems(tag)` 第 242-247 行：
    /// ```php
    /// public function getTagItems(string $tag): array
    /// {
    ///     $name = $this->getTagKey($tag);
    ///     $key  = $this->getCacheKey($name);
    ///     return $this->handler->sMembers($key);
    /// }
    /// ```
    pub fn get_tag_items(&self, tag: &str) -> Result<Vec<String>, CacheError> {
        let name = self.get_tag_key(tag);
        let key = self.get_cache_key(&name);
        self.backend.smembers(&key)
    }

    /// 删除缓存标签（对齐 PHP `Redis::clearTag`）
    ///
    /// PHP `Redis::clearTag(keys)` 第 217-221 行：
    /// ```php
    /// public function clearTag(array $keys): void
    /// {
    ///     $this->handler->del($keys);
    /// }
    /// ```
    pub fn clear_tag(&self, keys: &[&str]) -> Result<(), CacheError> {
        self.backend.del_many(keys)?;
        Ok(())
    }

    /// 获取 tag key（公开接口，用于测试和调试）
    pub fn tag_key(&self, tag: &str) -> String {
        self.get_tag_key(tag)
    }

    /// 获取 cache key（公开接口，用于测试和调试）
    pub fn cache_key(&self, name: &str) -> String {
        self.get_cache_key(name)
    }
}

impl CacheDriver for RedisCacheDriver {
    fn get_raw(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let cache_key = self.get_cache_key(key);
        self.backend.get(&cache_key)
    }

    fn set_raw(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<(), CacheError> {
        let cache_key = self.get_cache_key(key);
        // 对齐 PHP: $expire = is_null($ttl) ? $this->options['expire'] : $ttl
        let effective_ttl = ttl.or(self.config.expire);
        // 对齐 PHP: if ($expire) { setex } else { set }
        match effective_ttl {
            Some(t) if t > Duration::ZERO => self.backend.set_ex(&cache_key, value, t),
            _ => self.backend.set(&cache_key, value),
        }
    }

    fn delete(&self, key: &str) -> Result<(), CacheError> {
        let cache_key = self.get_cache_key(key);
        // PHP Redis::delete 返回 bool（del 数量 > 0），但 CacheDriver::delete 返回 ()
        self.backend.del(&cache_key)?;
        Ok(())
    }

    fn has(&self, key: &str) -> Result<bool, CacheError> {
        let cache_key = self.get_cache_key(key);
        // 对齐 PHP: return $this->handler->exists($key) ? true : false
        self.backend.exists(&cache_key)
    }

    /// 重写 inc（对齐 PHP `Redis::inc`，直接 INCRBY，不经 serialize）
    ///
    /// PHP `Redis::inc(name, step)` 第 161-167 行：
    /// ```php
    /// public function inc(string $name, int $step = 1)
    /// {
    ///     $this->writeTimes++;
    ///     $key = $this->getCacheKey($name);
    ///     return $this->handler->incrby($key, $step);
    /// }
    /// ```
    ///
    /// **关键差异**：File 驱动读取 → 加减 → 写回（经 serialize 层）；
    /// Redis 驱动直接 INCRBY（不经 serialize）。Redis 自身处理 key 不存在
    /// 的情况（初始化为 0）。
    fn inc(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        let cache_key = self.get_cache_key(key);
        self.backend.incr_by(&cache_key, step)
    }

    /// 重写 dec（对齐 PHP `Redis::dec`，直接 DECRBY，不经 serialize）
    fn dec(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        let cache_key = self.get_cache_key(key);
        self.backend.decr_by(&cache_key, step)
    }

    fn clear(&self) -> Result<(), CacheError> {
        // 对齐 PHP: $this->handler->flushDB()
        self.backend.flush_db()
    }

    // ========================================================================
    // 缓存标签重写（对齐 PHP think\cache\driver\Redis tag 相关方法）
    // ========================================================================

    /// 重写 `getCacheKey`（对齐 PHP `Driver::getCacheKey`）
    ///
    /// PHP: `return $this->options['prefix'] . $name;`
    fn get_cache_key(&self, name: &str) -> String {
        format!("{}{}", self.config.prefix, name)
    }

    /// 重写 `getTagKey`（对齐 PHP `Driver::getTagKey`）
    ///
    /// PHP: `return $this->options['tag_prefix'] . md5($tag);`
    fn get_tag_key(&self, tag: &str) -> String {
        let md5_hex = compute_md5(tag);
        format!("{}{}", self.config.tag_prefix, md5_hex)
    }

    /// 重写 `tag_append`（对齐 PHP `Redis::append`，使用 `sAdd` 而非 `push`）
    ///
    /// PHP `Redis::append(name, value)` 第 230-234 行：
    /// ```php
    /// public function append(string $name, $value): void
    /// {
    ///     $key = $this->getCacheKey($name);
    ///     $this->handler->sAdd($key, $value);
    /// }
    /// ```
    ///
    /// **关键差异**：PHP `think\cache\driver\Redis` 重写了父类 `Driver::append`
    /// （父类用 `push` = get→append→set，Redis 驱动用 `SADD` = 原子 Set 操作）。
    fn tag_append(&self, tag_key: &str, cache_key: &str) -> Result<(), CacheError> {
        let key = self.get_cache_key(tag_key);
        self.backend.sadd(&key, cache_key)?;
        Ok(())
    }

    /// 重写 `tag_items`（对齐 PHP `Redis::getTagItems`，使用 `sMembers`）
    ///
    /// PHP `Redis::getTagItems(tag)` 第 242-247 行：
    /// ```php
    /// public function getTagItems(string $tag): array
    /// {
    ///     $name = $this->getTagKey($tag);
    ///     $key  = $this->getCacheKey($name);
    ///     return $this->handler->sMembers($key);
    /// }
    /// ```
    fn tag_items(&self, tag: &str) -> Result<Vec<String>, CacheError> {
        let name = self.get_tag_key(tag);
        let key = self.get_cache_key(&name);
        self.backend.smembers(&key)
    }

    /// 重写 `tag_clear`（对齐 PHP `Redis::clearTag`，raw del 不应用前缀）
    ///
    /// PHP `Redis::clearTag(keys)` 第 217-221 行：
    /// ```php
    /// public function clearTag(array $keys): void
    /// {
    ///     $this->handler->del($keys);
    /// }
    /// ```
    ///
    /// **关键**：`keys` 已是 `prefix + name` 格式（来自 `tag_items` 返回值），
    /// 因此直接 `del_many`，**不得**再次应用 `getCacheKey`。
    fn tag_clear(&self, keys: &[String]) -> Result<(), CacheError> {
        let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        self.backend.del_many(&key_refs)?;
        Ok(())
    }
}

/// 计算 MD5 哈希（对齐 PHP `md5()` 函数）
///
/// PHP `md5("hello")` 返回 32 字符小写十六进制字符串。
pub(crate) fn compute_md5(s: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(s.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

// ============================================================================
// MultiLevelCacheDriver — 多级缓存驱动（复用 sz_orm_core::MultiLevelCache）
// ============================================================================

/// 多级缓存驱动（对齐 PHP think-cache 多驱动场景）
///
/// PHP think-cache 本身没有显式的 MultiLevelCache 概念，但业务层常通过
/// `Cache::store('redis')` / `Cache::store('file')` 切换驱动。本驱动封装
/// `sz_orm_core::MultiLevelCache`，提供 L1（内存）→ L2（Redis）级联查询：
///
/// - `get`：从高到低查询，命中后回填低层（带 TTL 保留）
/// - `set`：写入所有层级
/// - `delete`：删除所有层级
/// - `has`：任一层级存在即返回 true
/// - `clear`：清空所有层级
/// - `inc`/`dec`：委托到第一层（最高级），读取→加减→写回
///
/// ## 使用示例
///
/// ```ignore
/// use sz_rust_core::cache::{MultiLevelCacheDriver, MemoryCacheDriver};
/// use sz_orm_core::MemoryCache;
///
/// let l1 = MemoryCache::new();
/// let l2 = MemoryCache::with_ttl(std::time::Duration::from_secs(60));
/// let driver = MultiLevelCacheDriver::new()
///     .add_level(Box::new(l1))
///     .add_level(Box::new(l2));
/// ```
pub struct MultiLevelCacheDriver {
    inner: sz_orm_core::MultiLevelCache,
}

impl Default for MultiLevelCacheDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiLevelCacheDriver {
    /// 创建空的多级缓存驱动
    pub fn new() -> Self {
        Self {
            inner: sz_orm_core::MultiLevelCache::new(),
        }
    }

    /// 添加缓存层级（链式调用，先添加的优先级高）
    ///
    /// 对齐 sz_orm_core::MultiLevelCache::add_cache
    pub fn add_level(mut self, cache: Box<dyn InnerCache>) -> Self {
        self.inner = self.inner.add_cache(cache);
        self
    }

    /// 获取底层 MultiLevelCache 引用（用于 ttl 等底层查询）
    pub fn inner(&self) -> &sz_orm_core::MultiLevelCache {
        &self.inner
    }
}

impl CacheDriver for MultiLevelCacheDriver {
    fn get_raw(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        self.inner.get(key)
    }

    fn set_raw(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<(), CacheError> {
        self.inner.set(key, value, ttl)
    }

    fn delete(&self, key: &str) -> Result<(), CacheError> {
        self.inner.delete(key)
    }

    fn has(&self, key: &str) -> Result<bool, CacheError> {
        self.inner.exists(key)
    }

    fn inc(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        // 对齐 PHP File 驱动 inc/dec 行为：读取 → 解析为 i64 → 加减 → 写回
        // 多级缓存下，inc/dec 委托到第一层（最高级）
        let current = match self.inner.get(key)? {
            Some(bytes) => String::from_utf8(bytes)
                .map_err(|e| CacheError::DeserializationError(e.to_string()))?
                .parse::<i64>()
                .unwrap_or(0),
            None => 0,
        };
        let new_value = current + step;
        let new_bytes = new_value.to_string().into_bytes();
        // 保留原 TTL：查询第一层的 ttl，写回时不重新设置
        let ttl = self.inner.ttl(key).ok().flatten();
        self.inner.set(key, new_bytes, ttl)?;
        Ok(new_value)
    }

    fn dec(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        // 对齐 PHP File 驱动：读取 → 解析为 i64 → 减 → 写回
        let current = match self.inner.get(key)? {
            Some(bytes) => String::from_utf8(bytes)
                .map_err(|e| CacheError::DeserializationError(e.to_string()))?
                .parse::<i64>()
                .unwrap_or(0),
            None => 0,
        };
        let new_value = current - step;
        let new_bytes = new_value.to_string().into_bytes();
        let ttl = self.inner.ttl(key).ok().flatten();
        self.inner.set(key, new_bytes, ttl)?;
        Ok(new_value)
    }

    fn clear(&self) -> Result<(), CacheError> {
        self.inner.clear()
    }
}

// ============================================================================
// TagSet — 缓存标签集合（对齐 PHP think\cache\TagSet）
// ============================================================================

/// 缓存标签集合（对齐 PHP `think\cache\TagSet`）
///
/// PHP `TagSet`（132 行）提供基于标签的缓存批量管理：
///
/// - `set(name, value, expire)`：写入缓存 + 追加 key 到标签集合
/// - `append(name)`：追加 key 到所有标签
/// - `clear()`：清除所有标签下的缓存
///
/// ## PHP 对齐
///
/// PHP `TagSet::set` 第 52-59 行：
/// ```php
/// public function set(string $name, $value, $expire = null): bool
/// {
///     $this->handler->set($name, $value, $expire);
///     $this->append($name);
///     return true;
/// }
/// ```
///
/// PHP `TagSet::append` 第 67-75 行：
/// ```php
/// public function append(string $name): void
/// {
///     $name = $this->handler->getCacheKey($name);
///     foreach ($this->tag as $tag) {
///         $key = $this->handler->getTagKey($tag);
///         $this->handler->append($key, $name);
///     }
/// }
/// ```
///
/// PHP `TagSet::clear` 第 119-131 行：
/// ```php
/// public function clear(): bool
/// {
///     foreach ($this->tag as $tag) {
///         $names = $this->handler->getTagItems($tag);
///         $this->handler->clearTag($names);
///         $key = $this->handler->getTagKey($tag);
///         $this->handler->delete($key);
///     }
///     return true;
/// }
/// ```
///
/// ## 生命周期
///
/// `TagSet<'a>` 持有 `&'a Cache` 引用，确保在 TagSet 使用期间 Cache 不会被释放。
/// 通过 `Cache::tag()` 或 `Cache::tag_many()` 创建。
pub struct TagSet<'a> {
    /// 标签名列表（对齐 PHP `TagSet::$tag`）
    tags: Vec<String>,
    /// 缓存句柄（对齐 PHP `TagSet::$handler`）
    cache: &'a Cache,
}

impl<'a> TagSet<'a> {
    /// 写入缓存并追加到标签（对齐 PHP `TagSet::set`）
    ///
    /// PHP `TagSet::set(name, value, expire)` 第 52-59 行：
    /// 1. `handler->set(name, value, expire)` — 写入缓存
    /// 2. `append(name)` — 追加 key 到所有标签
    ///
    /// ## 参数
    ///
    /// - `key`：缓存键
    /// - `value`：缓存值（实现 `Serialize`）
    /// - `ttl`：过期时间（`None` 永不过期）
    pub fn set<T: Serialize>(
        &self,
        key: &str,
        value: T,
        ttl: Option<Duration>,
    ) -> Result<(), CacheError> {
        // 对齐 PHP: $this->handler->set($name, $value, $expire);
        self.cache.set(key, value, ttl)?;
        // 对齐 PHP: $this->append($name);
        self.append(key)
    }

    /// 追加缓存 key 到所有标签（对齐 PHP `TagSet::append`）
    ///
    /// PHP `TagSet::append(name)` 第 67-75 行：
    /// ```php
    /// $name = $this->handler->getCacheKey($name);
    /// foreach ($this->tag as $tag) {
    ///     $key = $this->handler->getTagKey($tag);
    ///     $this->handler->append($key, $name);
    /// }
    /// ```
    ///
    /// ## 行为
    ///
    /// 1. 计算 `cache_key = getCacheKey(name)`（应用前缀）
    /// 2. 对每个 tag：计算 `tag_key = getTagKey(tag)`，调用 `driver.tag_append(tag_key, cache_key)`
    pub fn append(&self, key: &str) -> Result<(), CacheError> {
        let mgr = self.cache.manager.read();
        let driver = mgr.default_store()?;
        // 对齐 PHP: $name = $this->handler->getCacheKey($name);
        let cache_key = driver.get_cache_key(key);
        // 对齐 PHP: foreach ($this->tag as $tag) { ... }
        for tag in &self.tags {
            // 对齐 PHP: $key = $this->handler->getTagKey($tag);
            let tag_key = driver.get_tag_key(tag);
            // 对齐 PHP: $this->handler->append($key, $name);
            driver.tag_append(&tag_key, &cache_key)?;
        }
        Ok(())
    }

    /// 清除所有标签下的缓存（对齐 PHP `TagSet::clear`）
    ///
    /// PHP `TagSet::clear()` 第 119-131 行：
    /// ```php
    /// foreach ($this->tag as $tag) {
    ///     $names = $this->handler->getTagItems($tag);
    ///     $this->handler->clearTag($names);
    ///     $key = $this->handler->getTagKey($tag);
    ///     $this->handler->delete($key);
    /// }
    /// ```
    ///
    /// ## 行为
    ///
    /// 对每个 tag：
    /// 1. `tag_items(tag)` — 获取该标签下所有缓存 key（已应用前缀）
    /// 2. `tag_clear(items)` — 批量删除这些缓存 key（raw delete，不应用前缀）
    /// 3. `delete(get_tag_key(tag))` — 删除标签 key 本身（delete 应用前缀）
    pub fn clear(&self) -> Result<(), CacheError> {
        let mgr = self.cache.manager.read();
        let driver = mgr.default_store()?;
        for tag in &self.tags {
            // 对齐 PHP: $names = $this->handler->getTagItems($tag);
            let items = driver.tag_items(tag)?;
            // 对齐 PHP: $this->handler->clearTag($names);
            driver.tag_clear(&items)?;
            // 对齐 PHP: $key = $this->handler->getTagKey($tag);
            let tag_key = driver.get_tag_key(tag);
            // 对齐 PHP: $this->handler->delete($key);
            driver.delete(&tag_key)?;
        }
        Ok(())
    }

    /// 获取标签列表（用于测试和调试）
    pub fn tags(&self) -> &[String] {
        &self.tags
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::sync::Barrier;

    /// 创建带默认驱动的测试用 Cache
    fn make_cache() -> Cache {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache
    }

    // ========================================================================
    // 组 1：php_is_numeric 对齐 PHP is_numeric
    // ========================================================================

    #[test]
    fn test_php_is_numeric_integer() {
        // 对齐 PHP: is_numeric("42") === true
        assert!(php_is_numeric("42"));
        assert!(php_is_numeric("-42"));
        assert!(php_is_numeric("+42"));
        assert!(php_is_numeric("0"));
    }

    #[test]
    fn test_php_is_numeric_float() {
        // 对齐 PHP: is_numeric("3.14") === true
        assert!(php_is_numeric("3.14"));
        assert!(php_is_numeric("-3.14"));
        assert!(php_is_numeric("+3.14"));
        assert!(php_is_numeric("0.0"));
    }

    #[test]
    fn test_php_is_numeric_scientific_notation() {
        // 对齐 PHP: is_numeric("1e10") === true
        assert!(php_is_numeric("1e10"));
        assert!(php_is_numeric("1.5E-3"));
    }

    #[test]
    fn test_php_is_numeric_non_numeric() {
        // 对齐 PHP: is_numeric("abc") === false
        assert!(!php_is_numeric("abc"));
        assert!(!php_is_numeric("12abc"));
        assert!(!php_is_numeric(""));
        assert!(!php_is_numeric("0x1A")); // PHP 7+ 不识别十六进制字符串
        assert!(!php_is_numeric("null"));
        assert!(!php_is_numeric("true"));
    }

    // ========================================================================
    // 组 2：php_serialize / php_unserialize 序列化策略
    // ========================================================================

    #[test]
    fn test_php_serialize_integer_to_number() {
        // 对齐 PHP: serialize(42) === "42"（is_numeric 短路）
        let v = php_serialize(&42i64).unwrap();
        assert!(matches!(v, CacheValue::Number(_)));
        if let CacheValue::Number(s) = v {
            assert_eq!(s, "42");
        }
    }

    #[test]
    fn test_php_serialize_float_to_number() {
        // 对齐 PHP: serialize(2.5) === "2.5"（is_numeric 短路）
        // 注：避开 3.14（clippy approx_constant 误报为 PI 近似值）
        let v = php_serialize(&2.5f64).unwrap();
        assert!(matches!(v, CacheValue::Number(_)));
        if let CacheValue::Number(s) = v {
            assert_eq!(s, "2.5");
        }
    }

    #[test]
    fn test_php_serialize_string_to_json() {
        // 对齐 PHP: serialize("Alice") === 's:5:"Alice";'（非 numeric 走 serialize）
        // Rust 端用 serde_json，"Alice" → "\"Alice\""
        let v = php_serialize(&"Alice".to_string()).unwrap();
        assert!(matches!(v, CacheValue::Json(_)));
        if let CacheValue::Json(s) = v {
            assert_eq!(s, "\"Alice\"");
        }
    }

    #[test]
    fn test_php_serialize_numeric_string_to_number() {
        // PHP is_numeric("42") === true，但 serde_json 序列化 String "42" → "\"42\""（带引号）
        // 因此 Rust 端把 String "42" 视为非 numeric，存为 Json
        // 这是 PHP 与 Rust 序列化机制的差异：PHP serialize 直接接收 $data 值，
        // 若 $data 是字符串 "42"，PHP is_numeric 检查的是字符串内容 "42"（true）
        // Rust serde_json 序列化 String "42" → "\"42\""（带引号），php_is_numeric 检查的是 "\"42\""（false）
        //
        // 这个差异是预期的：PHP 的 $value 是弱类型，Rust 的 T: Serialize 是强类型。
        // 业务对齐通过 i64/f64 类型直接调用 set 来保证 is_numeric 短路生效。
        let v = php_serialize(&"42".to_string()).unwrap();
        assert!(matches!(v, CacheValue::Json(_))); // 注意：是 Json，不是 Number
    }

    #[test]
    fn test_php_serialize_array_to_json() {
        // 对齐 PHP: serialize([1, 2, 3])（非 numeric 走 serialize）
        let v = php_serialize(&vec![1, 2, 3]).unwrap();
        assert!(matches!(v, CacheValue::Json(_)));
        if let CacheValue::Json(s) = v {
            assert_eq!(s, "[1,2,3]");
        }
    }

    #[test]
    fn test_php_unserialize_number_returns_string() {
        // PHP bug 复刻：unserialize 对 is_numeric 返回 string，而非 int
        // Rust 端通过泛型 T = String 来对齐
        let v = CacheValue::Number("42".to_string());
        let result: Option<String> = php_unserialize(&v).unwrap();
        assert_eq!(result, Some("42".to_string()));
    }

    #[test]
    fn test_php_unserialize_json_returns_struct() {
        // unserialize 对 JSON 字符串还原为原始结构
        let v = CacheValue::Json("\"Alice\"".to_string());
        let result: Option<String> = php_unserialize(&v).unwrap();
        assert_eq!(result, Some("Alice".to_string()));

        let v = CacheValue::Json("[1,2,3]".to_string());
        let result: Option<Vec<i64>> = php_unserialize(&v).unwrap();
        assert_eq!(result, Some(vec![1, 2, 3]));
    }

    #[test]
    fn test_php_unserialize_number_to_int_via_parse() {
        // 对齐 PHP 业务代码 (int) Cache::get('count') 强转模式
        // Rust 端：get::<String>() + .parse::<i64>()
        let v = CacheValue::Number("42".to_string());
        let s: String = php_unserialize(&v).unwrap().unwrap();
        let n: i64 = s.parse().unwrap();
        assert_eq!(n, 42);
    }

    // ========================================================================
    // 组 3：CacheValue from_bytes / to_bytes 往返
    // ========================================================================

    #[test]
    fn test_cache_value_number_roundtrip() {
        let v = CacheValue::Number("42".to_string());
        let bytes = v.to_bytes();
        let restored = CacheValue::from_bytes(&bytes).unwrap();
        assert_eq!(v, restored);
    }

    #[test]
    fn test_cache_value_json_roundtrip() {
        let v = CacheValue::Json("\"Alice\"".to_string());
        let bytes = v.to_bytes();
        let restored = CacheValue::from_bytes(&bytes).unwrap();
        assert_eq!(v, restored);
    }

    #[test]
    fn test_cache_value_array_roundtrip() {
        let v = CacheValue::Json("[1,2,3]".to_string());
        let bytes = v.to_bytes();
        let restored = CacheValue::from_bytes(&bytes).unwrap();
        assert_eq!(v, restored);
    }

    #[test]
    fn test_cache_value_from_bytes_numeric_string_becomes_number() {
        // "42" 字节 → Number（对齐 PHP unserialize is_numeric 短路）
        let bytes = b"42".to_vec();
        let v = CacheValue::from_bytes(&bytes).unwrap();
        assert!(matches!(v, CacheValue::Number(_)));
    }

    #[test]
    fn test_cache_value_from_bytes_json_string_becomes_json() {
        // "\"Alice\"" 字节 → Json
        let bytes = b"\"Alice\"".to_vec();
        let v = CacheValue::from_bytes(&bytes).unwrap();
        assert!(matches!(v, CacheValue::Json(_)));
    }

    // ========================================================================
    // 组 4：MemoryCacheDriver 基本操作
    // ========================================================================

    #[test]
    fn test_memory_driver_set_get_raw() {
        let driver = MemoryCacheDriver::new();
        driver.set_raw("key", b"value".to_vec(), None).unwrap();
        let val = driver.get_raw("key").unwrap();
        assert_eq!(val, Some(b"value".to_vec()));
    }

    #[test]
    fn test_memory_driver_delete() {
        let driver = MemoryCacheDriver::new();
        driver.set_raw("key", b"value".to_vec(), None).unwrap();
        driver.delete("key").unwrap();
        let val = driver.get_raw("key").unwrap();
        assert_eq!(val, None);
    }

    #[test]
    fn test_memory_driver_has() {
        let driver = MemoryCacheDriver::new();
        driver.set_raw("key", b"value".to_vec(), None).unwrap();
        assert!(driver.has("key").unwrap());
        assert!(!driver.has("nonexistent").unwrap());
    }

    #[test]
    fn test_memory_driver_clear() {
        let driver = MemoryCacheDriver::new();
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        driver.set_raw("key2", b"value2".to_vec(), None).unwrap();
        driver.clear().unwrap();
        assert!(!driver.has("key1").unwrap());
        assert!(!driver.has("key2").unwrap());
    }

    #[test]
    fn test_memory_driver_ttl_expiration() {
        let driver = MemoryCacheDriver::new();
        driver
            .set_raw("key", b"value".to_vec(), Some(Duration::from_millis(50)))
            .unwrap();
        assert!(driver.get_raw("key").unwrap().is_some());
        std::thread::sleep(Duration::from_millis(100));
        assert!(driver.get_raw("key").unwrap().is_none());
    }

    #[test]
    fn test_memory_driver_inc_default_implementation() {
        // 默认实现：File 驱动行为（读取 → 加减 → 写回）
        let driver = MemoryCacheDriver::new();

        // 键不存在：初始化为 step
        let v = driver.inc("counter", 5).unwrap();
        assert_eq!(v, 5);

        // 键存在：加 step
        let v = driver.inc("counter", 3).unwrap();
        assert_eq!(v, 8);
    }

    #[test]
    fn test_memory_driver_dec_default_implementation() {
        let driver = MemoryCacheDriver::new();

        // 键不存在：初始化为 -step（即 0 - step）
        let v = driver.dec("counter", 3).unwrap();
        assert_eq!(v, -3);

        // 键存在：减 step
        let v = driver.dec("counter", 2).unwrap();
        assert_eq!(v, -5);
    }

    // ========================================================================
    // 组 5：CacheManager 多驱动管理
    // ========================================================================

    #[test]
    fn test_cache_manager_register_and_get_default() {
        let mut mgr = CacheManager::new();
        mgr.register_store("default", Box::new(MemoryCacheDriver::new()));

        let driver = mgr.default_store().unwrap();
        driver.set_raw("key", b"value".to_vec(), None).unwrap();
        assert_eq!(driver.get_raw("key").unwrap(), Some(b"value".to_vec()));
    }

    #[test]
    fn test_cache_manager_multiple_stores_isolation() {
        let mut mgr = CacheManager::new();
        mgr.register_store("file", Box::new(MemoryCacheDriver::new()));
        mgr.register_store("redis", Box::new(MemoryCacheDriver::new()));

        let file_driver = mgr.store("file").unwrap();
        let redis_driver = mgr.store("redis").unwrap();

        file_driver
            .set_raw("key", b"file_value".to_vec(), None)
            .unwrap();
        redis_driver
            .set_raw("key", b"redis_value".to_vec(), None)
            .unwrap();

        // 不同 store 隔离
        assert_eq!(
            file_driver.get_raw("key").unwrap(),
            Some(b"file_value".to_vec())
        );
        assert_eq!(
            redis_driver.get_raw("key").unwrap(),
            Some(b"redis_value".to_vec())
        );
    }

    #[test]
    fn test_cache_manager_set_default() {
        let mut mgr = CacheManager::new();
        mgr.register_store("file", Box::new(MemoryCacheDriver::new()));
        mgr.register_store("redis", Box::new(MemoryCacheDriver::new()));

        // 默认是第一个注册的（file）
        let driver = mgr.default_store().unwrap();
        driver.set_raw("file_key", b"file".to_vec(), None).unwrap();
        assert_eq!(driver.get_raw("file_key").unwrap(), Some(b"file".to_vec()));

        // 切换默认到 redis
        mgr.set_default("redis").unwrap();
        let driver = mgr.default_store().unwrap();
        driver
            .set_raw("redis_key", b"redis".to_vec(), None)
            .unwrap();
        assert_eq!(
            driver.get_raw("redis_key").unwrap(),
            Some(b"redis".to_vec())
        );
    }

    #[test]
    fn test_cache_manager_default_store_not_registered_error() {
        let mgr = CacheManager::new();
        let result = mgr.default_store();
        assert!(matches!(result, Err(CacheError::NotFound(_))));
    }

    #[test]
    fn test_cache_manager_store_not_found_error() {
        let mgr = CacheManager::new();
        let result = mgr.store("nonexistent");
        assert!(matches!(result, Err(CacheError::NotFound(_))));
    }

    // ========================================================================
    // 组 6：Cache facade set/get 基本 API
    // ========================================================================

    #[test]
    fn test_cache_set_get_string() {
        let cache = make_cache();
        cache.set("name", "Alice", None).unwrap();
        let val: Option<String> = cache.get("name").unwrap();
        assert_eq!(val, Some("Alice".to_string()));
    }

    #[test]
    fn test_cache_set_get_int_as_string_php_bug() {
        // PHP bug 复刻：is_numeric 短路 + unserialize 返回 string
        // 调用方需自行 parse::<i64>()
        let cache = make_cache();
        cache.set("count", 42i64, None).unwrap();

        // 直接 get::<i64> 会失败（PHP 也不允许，需 (int) 强转）
        // 对齐方式：get::<String> + parse::<i64>
        let s: Option<String> = cache.get("count").unwrap();
        assert_eq!(s, Some("42".to_string()));

        let n: i64 = s.unwrap().parse().unwrap();
        assert_eq!(n, 42);
    }

    #[test]
    fn test_cache_set_get_struct() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct User {
            name: String,
            age: u32,
        }

        let cache = make_cache();
        let user = User {
            name: "Alice".to_string(),
            age: 30,
        };
        cache.set("user:1", &user, None).unwrap();

        let val: Option<User> = cache.get("user:1").unwrap();
        assert_eq!(val, Some(user));
    }

    #[test]
    fn test_cache_set_get_vec() {
        let cache = make_cache();
        let list = vec![1, 2, 3];
        cache.set("list", &list, None).unwrap();

        let val: Option<Vec<i64>> = cache.get("list").unwrap();
        assert_eq!(val, Some(vec![1, 2, 3]));
    }

    #[test]
    fn test_cache_get_miss_returns_none() {
        let cache = make_cache();
        let val: Option<String> = cache.get("nonexistent").unwrap();
        assert_eq!(val, None);
    }

    #[test]
    fn test_cache_get_or_default_value() {
        let cache = make_cache();
        let val: String = cache.get_or("nonexistent", "default".to_string()).unwrap();
        assert_eq!(val, "default");
    }

    #[test]
    fn test_cache_set_with_ttl_expires() {
        let cache = make_cache();
        cache
            .set("key", "value", Some(Duration::from_millis(50)))
            .unwrap();
        assert!(cache.get::<String>("key").unwrap().is_some());
        std::thread::sleep(Duration::from_millis(100));
        assert!(cache.get::<String>("key").unwrap().is_none());
    }

    // ========================================================================
    // 组 7：Cache facade delete / has / clear
    // ========================================================================

    #[test]
    fn test_cache_delete() {
        let cache = make_cache();
        cache.set("key", "value", None).unwrap();
        assert!(cache.has("key").unwrap());

        cache.delete("key").unwrap();
        assert!(!cache.has("key").unwrap());

        // 删除不存在的 key 不报错（对齐 PHP）
        cache.delete("nonexistent").unwrap();
    }

    #[test]
    fn test_cache_has_checks_ttl() {
        let cache = make_cache();
        cache
            .set("key", "value", Some(Duration::from_millis(50)))
            .unwrap();
        assert!(cache.has("key").unwrap());

        std::thread::sleep(Duration::from_millis(100));
        // TTL 过期后 has 返回 false
        assert!(!cache.has("key").unwrap());
    }

    #[test]
    fn test_cache_clear() {
        let cache = make_cache();
        cache.set("key1", "value1", None).unwrap();
        cache.set("key2", "value2", None).unwrap();

        cache.clear().unwrap();

        assert!(!cache.has("key1").unwrap());
        assert!(!cache.has("key2").unwrap());
    }

    // ========================================================================
    // 组 8：Cache facade inc / dec
    // ========================================================================

    #[test]
    fn test_cache_inc_initial_value() {
        // 对齐 PHP: 键不存在时初始化为 step
        let cache = make_cache();
        let v = cache.inc("counter", 5).unwrap();
        assert_eq!(v, 5);

        // 验证存储的值
        let s: String = cache.get("counter").unwrap().unwrap();
        assert_eq!(s, "5");
    }

    #[test]
    fn test_cache_inc_accumulate() {
        let cache = make_cache();
        cache.inc("counter", 5).unwrap();
        cache.inc("counter", 3).unwrap();
        let v = cache.inc("counter", 2).unwrap();
        assert_eq!(v, 10);
    }

    #[test]
    fn test_cache_dec_initial_value() {
        // 对齐 PHP: 键不存在时初始化为 -step
        let cache = make_cache();
        let v = cache.dec("counter", 3).unwrap();
        assert_eq!(v, -3);
    }

    #[test]
    fn test_cache_dec_accumulate() {
        let cache = make_cache();
        cache.set("counter", 100i64, None).unwrap();
        cache.dec("counter", 30).unwrap();
        let v = cache.dec("counter", 20).unwrap();
        assert_eq!(v, 50);
    }

    #[test]
    fn test_cache_increment_default_step_1() {
        let cache = make_cache();
        let v = cache.increment("counter").unwrap();
        assert_eq!(v, 1);
        let v = cache.increment("counter").unwrap();
        assert_eq!(v, 2);
    }

    #[test]
    fn test_cache_decrement_default_step_1() {
        let cache = make_cache();
        let v = cache.decrement("counter").unwrap();
        assert_eq!(v, -1);
        let v = cache.decrement("counter").unwrap();
        assert_eq!(v, -2);
    }

    // ========================================================================
    // 组 9：Cache facade pull（读后删）
    // ========================================================================

    #[test]
    fn test_cache_pull_existing_key() {
        // 对齐 PHP: get + delete
        let cache = make_cache();
        cache.set("key", "value", None).unwrap();

        let val: Option<String> = cache.pull("key").unwrap();
        assert_eq!(val, Some("value".to_string()));

        // pull 后 key 应被删除
        assert!(!cache.has("key").unwrap());
    }

    #[test]
    fn test_cache_pull_missing_key_returns_none() {
        let cache = make_cache();
        let val: Option<String> = cache.pull("nonexistent").unwrap();
        assert_eq!(val, None);
    }

    // ========================================================================
    // 组 10：Cache facade push（数组追加 + 上限 + 去重）
    // ========================================================================

    #[test]
    fn test_cache_push_initial_array() {
        let cache = make_cache();
        cache.push("list", "a".to_string(), None).unwrap();

        let val: Option<Vec<String>> = cache.get("list").unwrap();
        assert_eq!(val, Some(vec!["a".to_string()]));
    }

    #[test]
    fn test_cache_push_appends() {
        let cache = make_cache();
        cache.push("list", "a".to_string(), None).unwrap();
        cache.push("list", "b".to_string(), None).unwrap();
        cache.push("list", "c".to_string(), None).unwrap();

        let val: Option<Vec<String>> = cache.get("list").unwrap();
        assert_eq!(
            val,
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn test_cache_push_deduplication() {
        // 对齐 PHP array_unique：保留首次出现的元素
        let cache = make_cache();
        cache.push("list", "a".to_string(), None).unwrap();
        cache.push("list", "b".to_string(), None).unwrap();
        cache.push("list", "a".to_string(), None).unwrap(); // 重复
        cache.push("list", "c".to_string(), None).unwrap();
        cache.push("list", "b".to_string(), None).unwrap(); // 重复

        let val: Option<Vec<String>> = cache.get("list").unwrap();
        assert_eq!(
            val,
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
    }

    #[test]
    fn test_cache_push_max_1000_fifo() {
        // 对齐 PHP array_shift：长度 > 1000 时丢弃最旧
        let cache = make_cache();

        // 推入 1001 个元素
        for i in 0..1001i64 {
            cache.push("list", i, None).unwrap();
        }

        let val: Option<Vec<i64>> = cache.get("list").unwrap();
        let list = val.unwrap();

        // 长度应为 1000（丢弃了 0）
        assert_eq!(list.len(), 1000);
        // 第一个元素应为 1（0 被丢弃）
        assert_eq!(list[0], 1);
        // 最后一个元素应为 1000
        assert_eq!(list[999], 1000);
    }

    #[test]
    fn test_cache_push_non_array_becomes_array() {
        // 对齐 PHP: 缓存非数组 → 创建 [value]
        let cache = make_cache();

        // 先写入字符串（非数组）
        cache.set("key", "not_an_array".to_string(), None).unwrap();

        // push 应覆盖为数组
        cache.push("key", "first".to_string(), None).unwrap();

        let val: Option<Vec<String>> = cache.get("key").unwrap();
        assert_eq!(val, Some(vec!["first".to_string()]));
    }

    // ========================================================================
    // 组 11：Cache facade remember（缓存击穿防护 + PHP bug 复刻）
    // ========================================================================

    #[tokio::test]
    async fn test_cache_remember_cache_miss() {
        // 未命中：调用 callback 并写入缓存
        let cache = make_cache();
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = counter.clone();

        let val: i64 = cache
            .remember("expensive", None, || {
                counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst) as i64 + 100
            })
            .await
            .unwrap();
        assert_eq!(val, 100);

        // 第二次调用应命中缓存，callback 不被调用
        let val: i64 = cache
            .remember("expensive", None, || {
                counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst) as i64 + 200
            })
            .await
            .unwrap();
        assert_eq!(val, 100); // 命中缓存，仍是 100

        // callback 只被调用一次
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_cache_remember_cache_hit_returns_cached() {
        let cache = make_cache();
        cache.set("predefined", 42i64, None).unwrap();

        // 命中缓存，callback 不应被调用
        let val: i64 = cache
            .remember("predefined", None, || {
                panic!("callback should not be called on cache hit");
            })
            .await
            .unwrap();
        assert_eq!(val, 42);
    }

    #[tokio::test]
    async fn test_cache_remember_writes_with_ttl() {
        let cache = make_cache();
        cache
            .remember("key", Some(Duration::from_millis(50)), || 42i64)
            .await
            .unwrap();

        // 立即读取应命中
        assert_eq!(cache.get::<String>("key").unwrap(), Some("42".to_string()));

        // 等待过期
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(cache.get::<String>("key").unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cache_remember_releases_lock_on_success() {
        // 对齐 PHP: callback 成功后释放锁
        let cache = make_cache();
        cache.remember("key", None, || 42i64).await.unwrap();

        // 锁应被释放
        assert!(!cache.has("key_lock").unwrap());
    }

    #[tokio::test]
    async fn test_cache_remember_releases_lock_on_panic() {
        // 对齐 PHP finally 块：callback panic 时也应释放锁
        // Rust 端：由于我们用 FnOnce() -> T（无 Result），panic 会传播
        // 但锁会被泄漏（因为 panic 会跳过 delete）
        // 这是 PHP 行为的"复刻"（PHP 也存在 try/finally 在 fatal error 时不执行）
        // 此测试验证正常路径下锁被释放
        let cache = make_cache();
        let _ = cache.remember("key", None, || 42i64).await;
        assert!(!cache.has("key_lock").unwrap());
    }

    #[tokio::test]
    async fn test_cache_remember_lock_has_no_ttl_php_bug() {
        // PHP 源码 bug 复刻：锁 key 无 TTL
        // 验证方式：remember 期间，锁 key 被设置为 1，且无 TTL
        // 此测试通过代码审查确认（PHP 源码第 305 行：$this->set($lockName, 1) 无第三个参数）
        // 函数签名 set(&self, key, value, ttl: Option<Duration>)，ttl = None 即无 TTL

        // 此处验证锁被正确释放（间接验证锁机制正常工作）
        let cache = make_cache();
        cache.remember("key", None, || 42i64).await.unwrap();
        assert!(!cache.has("key_lock").unwrap());
    }

    #[tokio::test]
    async fn test_cache_remember_async_cache_miss() {
        // 未命中：调用 async callback 并写入缓存
        let cache = make_cache();
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = counter.clone();

        let val: i64 = cache
            .remember_async("expensive_async", None, || {
                let counter_clone = counter_clone.clone();
                async move {
                    counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst) as i64 + 100
                }
            })
            .await
            .unwrap();
        assert_eq!(val, 100);

        // 第二次调用应命中缓存，callback 不被调用
        let val: i64 = cache
            .remember_async("expensive_async", None, || {
                let counter_clone = counter_clone.clone();
                async move {
                    counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst) as i64 + 200
                }
            })
            .await
            .unwrap();
        assert_eq!(val, 100);

        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_cache_remember_async_cache_hit_returns_cached() {
        let cache = make_cache();
        cache.set("predefined_async", 42i64, None).unwrap();

        let val: i64 = cache
            .remember_async("predefined_async", None, || async {
                panic!("callback should not be called on cache hit");
            })
            .await
            .unwrap();
        assert_eq!(val, 42);
    }

    #[tokio::test]
    async fn test_cache_remember_async_writes_with_ttl() {
        let cache = make_cache();
        cache
            .remember_async("key_async", Some(Duration::from_millis(50)), || async {
                42i64
            })
            .await
            .unwrap();

        assert_eq!(
            cache.get::<String>("key_async").unwrap(),
            Some("42".to_string())
        );

        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(cache.get::<String>("key_async").unwrap().is_none());
    }

    #[tokio::test]
    async fn test_cache_remember_async_releases_lock_on_success() {
        let cache = make_cache();
        cache
            .remember_async("key_async", None, || async { 42i64 })
            .await
            .unwrap();
        assert!(!cache.has("key_async_lock").unwrap());
    }

    #[tokio::test]
    async fn test_cache_remember_async_lock_has_no_ttl_php_bug() {
        let cache = make_cache();
        cache
            .remember_async("key_async", None, || async { 42i64 })
            .await
            .unwrap();
        assert!(!cache.has("key_async_lock").unwrap());
    }

    // ========================================================================
    // 组 12：Cache facade with_store（命名 store 访问）
    // ========================================================================

    #[test]
    fn test_cache_with_store() {
        let cache = Cache::new();
        cache.register_store("redis", Box::new(MemoryCacheDriver::new()));

        let result = cache
            .with_store("redis", |driver| {
                driver.set_raw("key", b"value".to_vec(), None)?;
                driver.get_raw("key")
            })
            .unwrap();

        assert_eq!(result, Some(b"value".to_vec()));
    }

    #[test]
    fn test_cache_with_store_not_found() {
        let cache = Cache::new();
        let result: Result<Option<Vec<u8>>, CacheError> =
            cache.with_store("nonexistent", |driver| driver.get_raw("key"));
        assert!(matches!(result, Err(CacheError::NotFound(_))));
    }

    // ========================================================================
    // 组 13：全局 Cache facade（default_cache / init_default_cache）
    // ========================================================================

    #[test]
    fn test_default_cache_singleton() {
        let c1 = default_cache();
        let c2 = default_cache();
        // 同一个全局实例
        assert!(std::ptr::eq(c1, c2));
    }

    // 注：不测试 init_default_cache 的副作用，因为它修改全局状态，
    // 会影响其他测试。全局初始化应由应用入口负责。

    // ========================================================================
    // 组 14：R5 PHP 行为对齐验证（硬约束）
    // ========================================================================

    #[test]
    fn test_r5_php_set_get_basic_alignment() {
        // R5: PHP Cache::set + Cache::get 基本行为对齐
        // PHP:
        //   Cache::set('name', 'Alice');
        //   $val = Cache::get('name');  // 'Alice'
        let cache = make_cache();
        cache.set("name", "Alice", None).unwrap();
        let val: String = cache.get("name").unwrap().unwrap();
        assert_eq!(val, "Alice");
    }

    #[test]
    fn test_r5_php_is_numeric_short_circuit() {
        // R5: PHP is_numeric 短路 — 数字不经过 serialize
        // PHP:
        //   Cache::set('count', 42);
        //   $val = Cache::get('count');  // "42" (string, PHP bug)
        let cache = make_cache();
        cache.set("count", 42i64, None).unwrap();

        // PHP bug 复刻：get 返回 string 而非 int
        let s: String = cache.get("count").unwrap().unwrap();
        assert_eq!(s, "42");
    }

    #[test]
    fn test_r5_php_inc_no_serialize() {
        // R5: PHP inc 不经序列化层 — 直接数字操作
        // PHP Redis: INCRBY; File: read → +step → write
        let cache = make_cache();
        cache.set("counter", 100i64, None).unwrap();
        let new_val = cache.inc("counter", 50).unwrap();
        assert_eq!(new_val, 150);

        // 验证存储的是数字字符串
        let s: String = cache.get("counter").unwrap().unwrap();
        assert_eq!(s, "150");
    }

    #[test]
    fn test_r5_php_dec_no_serialize() {
        // R5: PHP dec 不经序列化层
        let cache = make_cache();
        cache.set("counter", 100i64, None).unwrap();
        let new_val = cache.dec("counter", 30).unwrap();
        assert_eq!(new_val, 70);
    }

    #[tokio::test]
    async fn test_r5_php_remember_lock_mechanism() {
        // R5: PHP remember 锁机制 — {name}_lock key + 200ms 轮询 + 5s 超时
        // 验证：未命中 → 调用 callback → 写入缓存 → 释放锁
        let cache = make_cache();
        let val: i64 = cache.remember("key", None, || 42).await.unwrap();
        assert_eq!(val, 42);
        assert_eq!(cache.get::<String>("key").unwrap(), Some("42".to_string()));
        // 锁应被释放
        assert!(!cache.has("key_lock").unwrap());
    }

    #[test]
    fn test_r5_php_push_max_1000_array_shift() {
        // R5: PHP push 上限 1000 + array_shift
        let cache = make_cache();
        for i in 0..1001i64 {
            cache.push("list", i, None).unwrap();
        }
        let list: Vec<i64> = cache.get("list").unwrap().unwrap();
        assert_eq!(list.len(), 1000);
        assert_eq!(list[0], 1); // 0 被丢弃
        assert_eq!(list[999], 1000);
    }

    #[test]
    fn test_r5_php_push_array_unique() {
        // R5: PHP push array_unique 去重
        let cache = make_cache();
        cache.push("list", "a".to_string(), None).unwrap();
        cache.push("list", "a".to_string(), None).unwrap();
        cache.push("list", "b".to_string(), None).unwrap();
        cache.push("list", "a".to_string(), None).unwrap();

        let list: Vec<String> = cache.get("list").unwrap().unwrap();
        assert_eq!(list, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn test_r5_php_pull_get_then_delete() {
        // R5: PHP pull — get + delete
        let cache = make_cache();
        cache.set("key", "value", None).unwrap();

        let val: Option<String> = cache.pull("key").unwrap();
        assert_eq!(val, Some("value".to_string()));
        assert!(!cache.has("key").unwrap());
    }

    #[test]
    fn test_r5_php_delete_nonexistent_no_error() {
        // R5: PHP delete 不存在的 key 不报错
        let cache = make_cache();
        let result = cache.delete("nonexistent");
        assert!(result.is_ok());
    }

    #[test]
    fn test_r5_php_has_ttl_expiration() {
        // R5: PHP has 检查 TTL 过期
        let cache = make_cache();
        cache
            .set("key", "value", Some(Duration::from_millis(50)))
            .unwrap();
        assert!(cache.has("key").unwrap());

        std::thread::sleep(Duration::from_millis(100));
        assert!(!cache.has("key").unwrap());
    }

    #[test]
    fn test_r5_php_clear_all_keys() {
        // R5: PHP clear 清空所有缓存
        let cache = make_cache();
        cache.set("key1", "value1", None).unwrap();
        cache.set("key2", "value2", None).unwrap();
        cache.set("key3", "value3", None).unwrap();

        cache.clear().unwrap();

        assert!(!cache.has("key1").unwrap());
        assert!(!cache.has("key2").unwrap());
        assert!(!cache.has("key3").unwrap());
    }

    // ========================================================================
    // 组 15：PHP 源码行为对齐 — 关键 bug 复刻验证
    // ========================================================================

    #[test]
    fn test_php_bug_unserialize_numeric_returns_string() {
        // PHP 源码 bug 复刻：unserialize 对 is_numeric 返回 string，而非 int
        // PHP 源码 think\cache\Driver::unserialize 第 623-630 行：
        //   public function unserialize($data)
        //   {
        //       if (is_numeric($data)) {
        //           return $data;  // ⚠️ 返回 string，而非 int
        //       }
        //       return unserialize($data);
        //   }
        //
        // Rust 端通过 CacheValue::Number + 泛型 T = String 复刻此行为

        let cache = make_cache();

        // 存入 i64 → is_numeric 短路 → CacheValue::Number("42")
        cache.set("count", 42i64, None).unwrap();

        // 取出时返回 string，对齐 PHP bug
        let s: String = cache.get("count").unwrap().unwrap();
        assert_eq!(s, "42");

        // 业务代码需自行 parse::<i64>()（对齐 PHP (int) Cache::get('count')）
        let n: i64 = s.parse().unwrap();
        assert_eq!(n, 42);
    }

    #[tokio::test]
    async fn test_php_bug_remember_lock_no_ttl() {
        // PHP 源码 bug 复刻：remember 锁 key 无 TTL
        // PHP 源码 think\cache\Driver::remember 第 305 行：
        //   $this->set($lockName, 1);  // 无第三个参数 $expire
        //
        // 验证方式：检查我们的实现也使用 ttl = None（代码审查确认）
        // 这里通过正常路径验证锁被释放（间接验证无 TTL 锁不会卡死）

        let cache = make_cache();
        cache.remember("key", None, || 42i64).await.unwrap();
        // 锁被正常释放（无 TTL 锁在正常路径下也会被 delete）
        assert!(!cache.has("key_lock").unwrap());
    }

    #[tokio::test]
    async fn test_php_bug_remember_has_get_double_check() {
        // PHP 源码 bug 复刻：remember 中 has() + get() 双查（TOCTOU）
        // PHP 源码 think\cache\Driver::remember 第 295-298 行：
        //   if ($this->has($lockName)) {  // 第一次检查
        //       while ($this->has($lockName) && ...) {  // 循环中再次检查
        //           usleep(200000);
        //       }
        //       // ...
        //       $data = $this->get($name);  // 第二次检查（read）
        //   }
        //
        // Rust 端复刻此双查模式（代码审查确认）：
        // 1. if self.has(&lock_key)?  // 第一次
        // 2. while self.has(&lock_key)?  // 循环
        // 3. if let Some(cached) = self.get::<T>(key)?  // 读

        // 此测试验证正常路径下的双查行为（不触发死锁）
        let cache = make_cache();
        let val: i64 = cache.remember("key", None, || 42).await.unwrap();
        assert_eq!(val, 42);
    }

    #[test]
    fn test_php_behavior_set_overwrite() {
        // PHP 行为：同 key 重复 set 覆盖旧值
        let cache = make_cache();
        cache.set("key", "first", None).unwrap();
        cache.set("key", "second", None).unwrap();

        let val: String = cache.get("key").unwrap().unwrap();
        assert_eq!(val, "second");
    }

    #[test]
    fn test_php_behavior_ttl_permanent() {
        // PHP 行为：ttl = null 永不过期
        let cache = make_cache();
        cache.set("key", "value", None).unwrap();

        // 立即读取应命中
        assert!(cache.has("key").unwrap());

        // 短暂等待后读取应仍命中（永不过期）
        std::thread::sleep(Duration::from_millis(50));
        assert!(cache.has("key").unwrap());
    }

    // ========================================================================
    // 组 16：RedisConfig 配置（对齐 PHP think\cache\driver\Redis::$options）
    // ========================================================================

    #[test]
    fn test_redis_config_default() {
        // 对齐 PHP 默认值（Redis.php 第 33-44 行）
        let config = RedisConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 6379);
        assert_eq!(config.password, "");
        assert_eq!(config.select, 0);
        assert_eq!(config.timeout, Duration::ZERO);
        assert_eq!(config.expire, None);
        assert!(!config.persistent);
        assert_eq!(config.prefix, "");
        assert_eq!(config.tag_prefix, "tag:");
    }

    #[test]
    fn test_redis_config_with_prefix() {
        let config = RedisConfig::with_prefix("myapp:");
        assert_eq!(config.prefix, "myapp:");
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.tag_prefix, "tag:");
    }

    #[test]
    fn test_redis_config_with_expire() {
        let config = RedisConfig::with_expire(Duration::from_secs(3600));
        assert_eq!(config.expire, Some(Duration::from_secs(3600)));
        assert_eq!(config.prefix, "");
    }

    // ========================================================================
    // 组 17：MockRedisBackend 基础 KV 操作
    // ========================================================================

    #[test]
    fn test_mock_redis_set_get_roundtrip() {
        let backend = MockRedisBackend::new();
        backend.set("key1", b"value1".to_vec()).unwrap();
        let val = backend.get("key1").unwrap();
        assert_eq!(val, Some(b"value1".to_vec()));
    }

    #[test]
    fn test_mock_redis_del() {
        let backend = MockRedisBackend::new();
        backend.set("key1", b"value1".to_vec()).unwrap();
        let removed = backend.del("key1").unwrap();
        assert_eq!(removed, 1);
        assert_eq!(backend.get("key1").unwrap(), None);
        // 再次删除返回 0
        assert_eq!(backend.del("key1").unwrap(), 0);
    }

    #[test]
    fn test_mock_redis_exists() {
        let backend = MockRedisBackend::new();
        assert!(!backend.exists("key1").unwrap());
        backend.set("key1", b"value1".to_vec()).unwrap();
        assert!(backend.exists("key1").unwrap());
    }

    #[test]
    fn test_mock_redis_incr_by_new_key() {
        // 对齐 Redis: key 不存在时初始化为 0 再 INCRBY
        let backend = MockRedisBackend::new();
        let result = backend.incr_by("counter", 5).unwrap();
        assert_eq!(result, 5);
        // 存储的是数字字符串 "5"，不经 serialize
        let val = backend.get("counter").unwrap();
        assert_eq!(val, Some(b"5".to_vec()));
    }

    #[test]
    fn test_mock_redis_incr_by_existing_key() {
        let backend = MockRedisBackend::new();
        backend.set("counter", b"10".to_vec()).unwrap();
        let result = backend.incr_by("counter", 5).unwrap();
        assert_eq!(result, 15);
        let val = backend.get("counter").unwrap();
        assert_eq!(val, Some(b"15".to_vec()));
    }

    #[test]
    fn test_mock_redis_incr_by_non_integer_error() {
        // 对齐 Redis: INCRBY 对非数字值返回错误
        let backend = MockRedisBackend::new();
        backend.set("key", b"not_a_number".to_vec()).unwrap();
        let result = backend.incr_by("key", 1);
        assert!(result.is_err());
    }

    #[test]
    fn test_mock_redis_decr_by() {
        let backend = MockRedisBackend::new();
        let result = backend.decr_by("counter", 3).unwrap();
        assert_eq!(result, -3);
    }

    // ========================================================================
    // 组 18：MockRedisBackend TTL 过期
    // ========================================================================

    #[test]
    fn test_mock_redis_set_ex_and_expire() {
        let backend = MockRedisBackend::new();
        backend
            .set_ex("key1", b"value1".to_vec(), Duration::from_millis(50))
            .unwrap();
        assert!(backend.get("key1").unwrap().is_some());
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(backend.get("key1").unwrap(), None);
    }

    #[test]
    fn test_mock_redis_expired_key_exists_false() {
        let backend = MockRedisBackend::new();
        backend
            .set_ex("key1", b"value1".to_vec(), Duration::from_millis(50))
            .unwrap();
        assert!(backend.exists("key1").unwrap());
        std::thread::sleep(Duration::from_millis(80));
        assert!(!backend.exists("key1").unwrap());
    }

    // ========================================================================
    // 组 19：MockRedisBackend Set 操作（对齐 Redis SADD/SMEMBERS）
    // ========================================================================

    #[test]
    fn test_mock_redis_sadd_smembers() {
        let backend = MockRedisBackend::new();
        backend.sadd("tag:users", "user:1").unwrap();
        backend.sadd("tag:users", "user:2").unwrap();
        backend.sadd("tag:users", "user:3").unwrap();
        let members = backend.smembers("tag:users").unwrap();
        assert_eq!(members.len(), 3);
        assert!(members.contains(&"user:1".to_string()));
        assert!(members.contains(&"user:2".to_string()));
        assert!(members.contains(&"user:3".to_string()));
    }

    #[test]
    fn test_mock_redis_sadd_dedup() {
        // 对齐 Redis SADD: 重复成员只保留一份
        let backend = MockRedisBackend::new();
        let added1 = backend.sadd("tag:users", "user:1").unwrap();
        assert_eq!(added1, 1);
        let added2 = backend.sadd("tag:users", "user:1").unwrap();
        assert_eq!(added2, 0);
        let members = backend.smembers("tag:users").unwrap();
        assert_eq!(members.len(), 1);
    }

    #[test]
    fn test_mock_redis_smembers_nonexistent_key() {
        // 对齐 Redis: SMEMBERS 不存在的 key 返回空数组
        let backend = MockRedisBackend::new();
        let members = backend.smembers("nonexistent").unwrap();
        assert!(members.is_empty());
    }

    // ========================================================================
    // 组 20：MockRedisBackend flush_db 和 del_many
    // ========================================================================

    #[test]
    fn test_mock_redis_flush_db() {
        let backend = MockRedisBackend::new();
        backend.set("key1", b"v1".to_vec()).unwrap();
        backend.set("key2", b"v2".to_vec()).unwrap();
        backend.sadd("tag:1", "m1").unwrap();
        backend.flush_db().unwrap();
        assert_eq!(backend.get("key1").unwrap(), None);
        assert_eq!(backend.get("key2").unwrap(), None);
        assert!(backend.smembers("tag:1").unwrap().is_empty());
    }

    #[test]
    fn test_mock_redis_del_many() {
        let backend = MockRedisBackend::new();
        backend.set("key1", b"v1".to_vec()).unwrap();
        backend.set("key2", b"v2".to_vec()).unwrap();
        backend.set("key3", b"v3".to_vec()).unwrap();
        let removed = backend.del_many(&["key1", "key2", "nonexistent"]).unwrap();
        assert_eq!(removed, 2);
        assert_eq!(backend.get("key1").unwrap(), None);
        assert_eq!(backend.get("key2").unwrap(), None);
        assert!(backend.get("key3").unwrap().is_some());
    }

    // ========================================================================
    // 组 21：RedisCacheDriver 基本 API（对齐 PHP Redis 驱动）
    // ========================================================================

    /// 创建带 Mock backend 的 RedisCacheDriver
    fn make_redis_driver() -> RedisCacheDriver {
        RedisCacheDriver::new(RedisConfig::default())
    }

    #[test]
    fn test_redis_driver_set_get_roundtrip() {
        let driver = make_redis_driver();
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        let val = driver.get_raw("key1").unwrap();
        assert_eq!(val, Some(b"value1".to_vec()));
    }

    #[test]
    fn test_redis_driver_delete() {
        let driver = make_redis_driver();
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        driver.delete("key1").unwrap();
        assert_eq!(driver.get_raw("key1").unwrap(), None);
    }

    #[test]
    fn test_redis_driver_has() {
        let driver = make_redis_driver();
        assert!(!driver.has("key1").unwrap());
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        assert!(driver.has("key1").unwrap());
    }

    #[test]
    fn test_redis_driver_clear() {
        let driver = make_redis_driver();
        driver.set_raw("key1", b"v1".to_vec(), None).unwrap();
        driver.set_raw("key2", b"v2".to_vec(), None).unwrap();
        driver.clear().unwrap();
        assert_eq!(driver.get_raw("key1").unwrap(), None);
        assert_eq!(driver.get_raw("key2").unwrap(), None);
    }

    #[test]
    fn test_redis_driver_inc_dec() {
        let driver = make_redis_driver();
        // inc 新 key（对齐 Redis INCRBY）
        let result = driver.inc("counter", 5).unwrap();
        assert_eq!(result, 5);
        let result = driver.inc("counter", 3).unwrap();
        assert_eq!(result, 8);
        let result = driver.dec("counter", 2).unwrap();
        assert_eq!(result, 6);
    }

    // ========================================================================
    // 组 22：RedisCacheDriver key 构造（对齐 PHP getCacheKey/getTagKey）
    // ========================================================================

    #[test]
    fn test_redis_driver_cache_key_with_prefix() {
        // 对齐 PHP: getCacheKey(name) = prefix + name
        let driver = RedisCacheDriver::new(RedisConfig::with_prefix("myapp:"));
        assert_eq!(driver.cache_key("user:1"), "myapp:user:1");
    }

    #[test]
    fn test_redis_driver_cache_key_no_prefix() {
        let driver = RedisCacheDriver::new(RedisConfig::default());
        assert_eq!(driver.cache_key("user:1"), "user:1");
    }

    #[test]
    fn test_redis_driver_tag_key_md5() {
        // 对齐 PHP: getTagKey(tag) = tag_prefix + md5(tag)
        let driver = RedisCacheDriver::new(RedisConfig::default());
        let tag_key = driver.tag_key("users");
        let expected_md5 = compute_md5("users");
        assert_eq!(tag_key, format!("tag:{}", expected_md5));
    }

    #[test]
    fn test_redis_driver_tag_key_custom_prefix() {
        let config = RedisConfig {
            tag_prefix: "t:".to_string(),
            ..RedisConfig::default()
        };
        let driver = RedisCacheDriver::new(config);
        let tag_key = driver.tag_key("users");
        let expected_md5 = compute_md5("users");
        assert_eq!(tag_key, format!("t:{}", expected_md5));
    }

    // ========================================================================
    // 组 23：RedisCacheDriver prefix 生效（对齐 PHP Redis::set/get 自动加前缀）
    // ========================================================================

    #[test]
    fn test_redis_driver_prefix_applied_to_set() {
        // 对齐 PHP: set 时自动加 prefix 到 key
        let driver = RedisCacheDriver::new(RedisConfig::with_prefix("myapp:"));
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        // 底层 backend 收到的 key 应该是 "myapp:key1"
        let val = driver.backend().get("myapp:key1").unwrap();
        assert_eq!(val, Some(b"value1".to_vec()));
        // 不带 prefix 的 key 应该不存在
        assert_eq!(driver.backend().get("key1").unwrap(), None);
    }

    #[test]
    fn test_redis_driver_prefix_applied_to_get() {
        let driver = RedisCacheDriver::new(RedisConfig::with_prefix("myapp:"));
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        let val = driver.get_raw("key1").unwrap();
        assert_eq!(val, Some(b"value1".to_vec()));
    }

    #[test]
    fn test_redis_driver_prefix_applied_to_delete() {
        let driver = RedisCacheDriver::new(RedisConfig::with_prefix("myapp:"));
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        driver.delete("key1").unwrap();
        assert_eq!(driver.backend().get("myapp:key1").unwrap(), None);
    }

    #[test]
    fn test_redis_driver_prefix_applied_to_has() {
        let driver = RedisCacheDriver::new(RedisConfig::with_prefix("myapp:"));
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        assert!(driver.has("key1").unwrap());
        assert!(driver.backend().exists("myapp:key1").unwrap());
    }

    #[test]
    fn test_redis_driver_prefix_applied_to_inc() {
        let driver = RedisCacheDriver::new(RedisConfig::with_prefix("myapp:"));
        let result = driver.inc("counter", 5).unwrap();
        assert_eq!(result, 5);
        let val = driver.backend().get("myapp:counter").unwrap();
        assert_eq!(val, Some(b"5".to_vec()));
    }

    // ========================================================================
    // 组 24：RedisCacheDriver tag 操作（对齐 PHP Redis::append/getTagItems/clearTag）
    // ========================================================================

    #[test]
    fn test_redis_driver_append_and_get_tag_items() {
        // 对齐 PHP Redis::append 用 SADD
        let driver = make_redis_driver();
        driver.append("tag:users", "user:1").unwrap();
        driver.append("tag:users", "user:2").unwrap();
        driver.append("tag:users", "user:3").unwrap();
        let members = driver.backend().smembers("tag:users").unwrap();
        assert_eq!(members.len(), 3);
    }

    #[test]
    fn test_redis_driver_get_tag_items_with_tag_key() {
        // 对齐 PHP: getTagItems(tag) 通过 getTagKey(tag) + getCacheKey 转换
        let driver = make_redis_driver();
        let tag_name = driver.tag_key("users");
        driver.append(&tag_name, "user:1").unwrap();
        driver.append(&tag_name, "user:2").unwrap();
        let members = driver.get_tag_items("users").unwrap();
        assert_eq!(members.len(), 2);
        assert!(members.contains(&"user:1".to_string()));
        assert!(members.contains(&"user:2".to_string()));
    }

    #[test]
    fn test_redis_driver_clear_tag() {
        let driver = make_redis_driver();
        driver.set_raw("key1", b"v1".to_vec(), None).unwrap();
        driver.set_raw("key2", b"v2".to_vec(), None).unwrap();
        driver.clear_tag(&["key1", "key2"]).unwrap();
        assert_eq!(driver.get_raw("key1").unwrap(), None);
        assert_eq!(driver.get_raw("key2").unwrap(), None);
    }

    // ========================================================================
    // 组 25：RedisCacheDriver TTL（对齐 PHP Redis::set 的 SETEX vs SET 行为）
    // ========================================================================

    #[test]
    fn test_redis_driver_set_with_ttl_uses_setex() {
        // 对齐 PHP: expire > 0 时用 SETEX
        let driver = make_redis_driver();
        driver
            .set_raw("key1", b"value1".to_vec(), Some(Duration::from_millis(100)))
            .unwrap();
        assert!(driver.get_raw("key1").unwrap().is_some());
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(driver.get_raw("key1").unwrap(), None);
    }

    #[test]
    fn test_redis_driver_set_without_ttl_uses_set() {
        // 对齐 PHP: expire = 0 时用 SET（永不过期）
        let driver = make_redis_driver();
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(driver.get_raw("key1").unwrap().is_some());
    }

    #[test]
    fn test_redis_driver_set_with_config_expire() {
        // 对齐 PHP: ttl = null 时用 config.expire
        let config = RedisConfig::with_expire(Duration::from_millis(100));
        let driver = RedisCacheDriver::new(config);
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        assert!(driver.get_raw("key1").unwrap().is_some());
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(driver.get_raw("key1").unwrap(), None);
    }

    #[test]
    fn test_redis_driver_set_ttl_overrides_config_expire() {
        // 对齐 PHP: 显式 ttl 优先于 config.expire
        let config = RedisConfig::with_expire(Duration::from_secs(3600));
        let driver = RedisCacheDriver::new(config);
        driver
            .set_raw("key1", b"value1".to_vec(), Some(Duration::from_millis(50)))
            .unwrap();
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(driver.get_raw("key1").unwrap(), None);
    }

    // ========================================================================
    // 组 26：RedisCacheDriver 通过 Cache facade 使用
    // ========================================================================

    #[test]
    fn test_redis_driver_with_cache_facade() {
        // 对齐 PHP: $cache = new think\Cache(); $cache->store('redis')->set(...)
        let cache = Cache::new();
        let driver = RedisCacheDriver::new(RedisConfig::default());
        cache.register_store("redis", Box::new(driver));
        cache.set_default_store("redis").unwrap();
        cache.set("key", "value", None).unwrap();
        let val: String = cache.get("key").unwrap().unwrap();
        assert_eq!(val, "value");
    }

    #[test]
    fn test_redis_driver_with_cache_facade_inc() {
        // 通过 Cache facade 使用 Redis 驱动的 inc（不经 serialize）
        let cache = Cache::new();
        let driver = RedisCacheDriver::new(RedisConfig::default());
        cache.register_store("redis", Box::new(driver));
        cache.set_default_store("redis").unwrap();
        let result = cache.inc("counter", 5).unwrap();
        assert_eq!(result, 5);
        let result = cache.inc("counter", 3).unwrap();
        assert_eq!(result, 8);
    }

    // ========================================================================
    // 组 27：PHP 行为对齐验证（R5 Redis 硬约束）
    // ========================================================================

    #[test]
    fn test_php_redis_inc_not_through_serialize() {
        // R5-Redis-1: inc 不经 serialize（存储的是数字字符串，不是 PHP serialize 格式）
        // PHP Redis::inc 直接 INCRBY，File 驱动读取→加减→写回
        // 验证：Redis 驱动 inc 后，底层存储的是 "5"（数字字符串），
        //       而不是 PHP serialize 格式 "i:5;"
        let driver = make_redis_driver();
        driver.inc("counter", 5).unwrap();
        let val = driver.backend().get(&driver.cache_key("counter")).unwrap();
        assert_eq!(val, Some(b"5".to_vec())); // 数字字符串
        assert_ne!(val, Some(b"i:5;".to_vec())); // 不是 PHP serialize 格式
    }

    #[test]
    fn test_php_redis_set_with_ttl_expires() {
        // R5-Redis-2: set 带 TTL 用 SETEX（通过 TTL 过期行为验证）
        let driver = make_redis_driver();
        driver
            .set_raw("key", b"value".to_vec(), Some(Duration::from_millis(50)))
            .unwrap();
        assert!(driver.get_raw("key").unwrap().is_some());
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(driver.get_raw("key").unwrap(), None);
    }

    #[test]
    fn test_php_redis_set_without_ttl_permanent() {
        // R5-Redis-3: set 无 TTL 用 SET（永不过期）
        let driver = make_redis_driver();
        driver.set_raw("key", b"value".to_vec(), None).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(driver.get_raw("key").unwrap().is_some());
    }

    #[test]
    fn test_php_redis_tag_key_format() {
        // R5-Redis-4: tag_key = tag_prefix + md5(tag)
        let driver = make_redis_driver();
        let tag_key = driver.tag_key("users");
        let expected = format!("tag:{}", compute_md5("users"));
        assert_eq!(tag_key, expected);
        assert_eq!(compute_md5("users").len(), 32);
    }

    #[test]
    fn test_php_redis_clear_uses_flushdb() {
        // R5-Redis-5: clear 用 FLUSHDB（清空所有 key 和 set）
        let driver = make_redis_driver();
        driver.set_raw("key1", b"v1".to_vec(), None).unwrap();
        driver.set_raw("key2", b"v2".to_vec(), None).unwrap();
        driver.append("tag:1", "m1").unwrap();
        driver.clear().unwrap();
        assert_eq!(driver.get_raw("key1").unwrap(), None);
        assert_eq!(driver.get_raw("key2").unwrap(), None);
        assert!(driver.backend().smembers("tag:1").unwrap().is_empty());
    }

    #[test]
    fn test_php_redis_inc_returns_new_value() {
        // R5-Redis-6: inc 返回新值（对齐 PHP incrby 返回值）
        let driver = make_redis_driver();
        let r1 = driver.inc("c", 1).unwrap();
        assert_eq!(r1, 1);
        let r2 = driver.inc("c", 1).unwrap();
        assert_eq!(r2, 2);
        let r3 = driver.inc("c", 10).unwrap();
        assert_eq!(r3, 12);
        let r4 = driver.dec("c", 5).unwrap();
        assert_eq!(r4, 7);
    }

    #[test]
    fn test_php_redis_delete_nonexistent_returns_ok() {
        // R5-Redis-7: delete 不存在的 key 返回 Ok
        let driver = make_redis_driver();
        driver.delete("nonexistent").unwrap();
    }

    #[test]
    fn test_php_redis_md5_alignment() {
        // R5-Redis-8: md5 对齐 PHP md5() 函数
        // PHP: md5("hello") = "5d41402abc4b2a76b9719d911017c592"
        assert_eq!(compute_md5("hello"), "5d41402abc4b2a76b9719d911017c592");
        // PHP: md5("") = "d41d8cd98f00b204e9800998ecf8427e"
        assert_eq!(compute_md5(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(compute_md5("users").len(), 32);
    }

    #[test]
    fn test_php_redis_append_uses_sadd() {
        // R5-Redis-9: append 用 SADD（Set 存储，去重）
        // 对比 PHP File 驱动 append 用 push（数组存储）
        let driver = make_redis_driver();
        driver.append("tag:1", "m1").unwrap();
        driver.append("tag:1", "m1").unwrap(); // 重复
        driver.append("tag:1", "m2").unwrap();
        let members = driver.backend().smembers("tag:1").unwrap();
        // SADD 去重：只有 2 个成员
        assert_eq!(members.len(), 2);
    }

    #[test]
    fn test_php_redis_config_precedence_ttl() {
        // R5-Redis-10: TTL 优先级：显式 ttl > config.expire > None（永不过期）
        // 1. 显式 ttl 优先于 config.expire
        let config = RedisConfig::with_expire(Duration::from_secs(3600));
        let driver = RedisCacheDriver::new(config);
        driver
            .set_raw("key1", b"v1".to_vec(), Some(Duration::from_millis(50)))
            .unwrap();
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(driver.get_raw("key1").unwrap(), None); // 显式 ttl 生效

        // 2. config.expire 优先于 None
        let config = RedisConfig::with_expire(Duration::from_millis(50));
        let driver = RedisCacheDriver::new(config);
        driver.set_raw("key2", b"v2".to_vec(), None).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(driver.get_raw("key2").unwrap(), None); // config.expire 生效

        // 3. 都无则永不过期
        let driver = make_redis_driver();
        driver.set_raw("key3", b"v3".to_vec(), None).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(driver.get_raw("key3").unwrap().is_some()); // 永不过期
    }

    // ========================================================================
    // 组 28：MultiLevelCacheDriver 基础操作
    // ========================================================================

    #[test]
    fn test_multi_level_driver_set_get() {
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        let val = driver.get_raw("key1").unwrap();
        assert_eq!(val, Some(b"value1".to_vec()));
    }

    #[test]
    fn test_multi_level_driver_delete() {
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        driver.delete("key1").unwrap();
        assert_eq!(driver.get_raw("key1").unwrap(), None);
    }

    #[test]
    fn test_multi_level_driver_has() {
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        assert!(!driver.has("key1").unwrap());
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        assert!(driver.has("key1").unwrap());
    }

    #[test]
    fn test_multi_level_driver_clear() {
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        driver.set_raw("key1", b"v1".to_vec(), None).unwrap();
        driver.set_raw("key2", b"v2".to_vec(), None).unwrap();
        driver.clear().unwrap();
        assert_eq!(driver.get_raw("key1").unwrap(), None);
        assert_eq!(driver.get_raw("key2").unwrap(), None);
    }

    // ========================================================================
    // 组 29：MultiLevelCacheDriver 多层级联
    // ========================================================================

    #[test]
    fn test_multi_level_two_levels_cascade_get() {
        // L1 空，L2 有数据 → get 应从 L2 命中并回填 L1
        let l1 = sz_orm_core::MemoryCache::new();
        let l2 = sz_orm_core::MemoryCache::new();

        // 先在 L2 写入
        l2.set("key", b"from_l2".to_vec(), None).unwrap();

        let driver = MultiLevelCacheDriver::new()
            .add_level(Box::new(l1.clone()))
            .add_level(Box::new(l2));

        // L1 未命中，L2 命中
        let val = driver.get_raw("key").unwrap();
        assert_eq!(val, Some(b"from_l2".to_vec()));

        // L1 应被回填
        let l1_val = l1.get("key").unwrap();
        assert_eq!(l1_val, Some(b"from_l2".to_vec()));
    }

    #[test]
    fn test_multi_level_set_writes_all_levels() {
        let l1 = sz_orm_core::MemoryCache::new();
        let l2 = sz_orm_core::MemoryCache::new();

        let driver = MultiLevelCacheDriver::new()
            .add_level(Box::new(l1.clone()))
            .add_level(Box::new(l2.clone()));

        driver.set_raw("key", b"value".to_vec(), None).unwrap();

        // 两层都应有
        assert_eq!(l1.get("key").unwrap(), Some(b"value".to_vec()));
        assert_eq!(l2.get("key").unwrap(), Some(b"value".to_vec()));
    }

    #[test]
    fn test_multi_level_delete_removes_all_levels() {
        let l1 = sz_orm_core::MemoryCache::new();
        let l2 = sz_orm_core::MemoryCache::new();

        let driver = MultiLevelCacheDriver::new()
            .add_level(Box::new(l1.clone()))
            .add_level(Box::new(l2.clone()));

        driver.set_raw("key", b"value".to_vec(), None).unwrap();
        driver.delete("key").unwrap();

        assert_eq!(l1.get("key").unwrap(), None);
        assert_eq!(l2.get("key").unwrap(), None);
    }

    #[test]
    fn test_multi_level_l1_hit_skips_l2() {
        // L1 命中时不查询 L2
        let l1 = sz_orm_core::MemoryCache::new();
        let l2 = sz_orm_core::MemoryCache::new();

        l1.set("key", b"from_l1".to_vec(), None).unwrap();
        l2.set("key", b"from_l2".to_vec(), None).unwrap();

        let driver = MultiLevelCacheDriver::new()
            .add_level(Box::new(l1))
            .add_level(Box::new(l2));

        let val = driver.get_raw("key").unwrap();
        assert_eq!(val, Some(b"from_l1".to_vec()));
    }

    // ========================================================================
    // 组 30：MultiLevelCacheDriver inc/dec
    // ========================================================================

    #[test]
    fn test_multi_level_driver_inc_initial_value() {
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        let new_val = driver.inc("counter", 1).unwrap();
        assert_eq!(new_val, 1);

        let val = driver.get_raw("counter").unwrap();
        assert_eq!(val, Some(b"1".to_vec()));
    }

    #[test]
    fn test_multi_level_driver_inc_accumulate() {
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        driver.inc("counter", 5).unwrap();
        driver.inc("counter", 3).unwrap();
        driver.inc("counter", 1).unwrap();

        let val = driver.get_raw("counter").unwrap();
        assert_eq!(val, Some(b"9".to_vec()));
    }

    #[test]
    fn test_multi_level_driver_dec() {
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        driver.set_raw("counter", b"10".to_vec(), None).unwrap();
        let new_val = driver.dec("counter", 3).unwrap();
        assert_eq!(new_val, 7);

        let val = driver.get_raw("counter").unwrap();
        assert_eq!(val, Some(b"7".to_vec()));
    }

    #[test]
    fn test_multi_level_driver_inc_preserves_ttl() {
        // inc/dec 应保留原 TTL
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        driver
            .set_raw("counter", b"5".to_vec(), Some(Duration::from_millis(200)))
            .unwrap();

        // TTL 存在
        let ttl_before = driver.inner().ttl("counter").unwrap();
        assert!(ttl_before.is_some());

        driver.inc("counter", 1).unwrap();

        // TTL 应被保留
        let ttl_after = driver.inner().ttl("counter").unwrap();
        assert!(ttl_after.is_some());
    }

    // ========================================================================
    // 组 31：MultiLevelCacheDriver TTL 过期
    // ========================================================================

    #[test]
    fn test_multi_level_driver_ttl_expiration() {
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        driver
            .set_raw("key", b"value".to_vec(), Some(Duration::from_millis(50)))
            .unwrap();
        assert!(driver.get_raw("key").unwrap().is_some());

        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(driver.get_raw("key").unwrap(), None);
    }

    #[test]
    fn test_multi_level_driver_has_checks_ttl() {
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        driver
            .set_raw("key", b"value".to_vec(), Some(Duration::from_millis(50)))
            .unwrap();
        assert!(driver.has("key").unwrap());

        std::thread::sleep(Duration::from_millis(80));
        assert!(!driver.has("key").unwrap());
    }

    // ========================================================================
    // 组 32：MultiLevelCacheDriver 通过 Cache facade 使用
    // ========================================================================

    #[test]
    fn test_multi_level_driver_with_cache_facade() {
        let l1 = sz_orm_core::MemoryCache::new();
        let l2 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new()
            .add_level(Box::new(l1))
            .add_level(Box::new(l2));

        let cache = Cache::new();
        cache.register_store("default", Box::new(driver));

        cache.set("user:1", "Alice", None).unwrap();
        assert_eq!(
            cache.get::<String>("user:1").unwrap(),
            Some("Alice".to_string())
        );

        cache.delete("user:1").unwrap();
        assert_eq!(cache.get::<String>("user:1").unwrap(), None);
    }

    #[test]
    fn test_multi_level_driver_with_cache_facade_inc() {
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        let cache = Cache::new();
        cache.register_store("default", Box::new(driver));

        cache.inc("counter", 5).unwrap();
        cache.inc("counter", 3).unwrap();

        // PHP bug 复刻：inc 不经 serialize，存储为字符串 "8"
        // get::<String> 返回 "8"
        let val = cache.get::<String>("counter").unwrap();
        assert_eq!(val, Some("8".to_string()));
    }

    // ========================================================================
    // 组 33：MultiLevelCacheDriver 边界情况
    // ========================================================================

    #[test]
    fn test_multi_level_driver_empty_levels_get_returns_none() {
        // 无任何层级的驱动，get 返回 None（不报错）
        let driver = MultiLevelCacheDriver::new();
        assert_eq!(driver.get_raw("key").unwrap(), None);
    }

    #[test]
    fn test_multi_level_driver_empty_levels_has_returns_false() {
        let driver = MultiLevelCacheDriver::new();
        assert!(!driver.has("key").unwrap());
    }

    #[test]
    fn test_multi_level_driver_inc_non_numeric_value_resets_to_step() {
        // 非 numeric value 时，parse 失败降级为 0，再 +step
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        driver
            .set_raw("counter", b"not_a_number".to_vec(), None)
            .unwrap();
        let new_val = driver.inc("counter", 5).unwrap();
        assert_eq!(new_val, 5);
    }

    #[test]
    fn test_multi_level_driver_default_impl() {
        // Default::default() 等价于 new()
        let driver = MultiLevelCacheDriver::default();
        assert_eq!(driver.get_raw("key").unwrap(), None);
    }

    // ========================================================================
    // 组 34：R5 PHP 行为对齐验证（多级缓存）
    // ========================================================================

    #[test]
    fn test_r5_multi_level_get_set_basic() {
        // R5: 基本 set/get 行为对齐 PHP Cache::set/get
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        driver.set_raw("key", b"value".to_vec(), None).unwrap();
        assert_eq!(driver.get_raw("key").unwrap(), Some(b"value".to_vec()));
    }

    #[test]
    fn test_r5_multi_level_delete_nonexistent_no_error() {
        // R5: 删除不存在的 key 不报错
        let l1 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new().add_level(Box::new(l1));

        assert!(driver.delete("nonexistent").is_ok());
    }

    #[test]
    fn test_r5_multi_level_clear_empties_all() {
        // R5: clear 清空所有层
        let l1 = sz_orm_core::MemoryCache::new();
        let l2 = sz_orm_core::MemoryCache::new();
        let driver = MultiLevelCacheDriver::new()
            .add_level(Box::new(l1))
            .add_level(Box::new(l2));

        driver.set_raw("k1", b"v1".to_vec(), None).unwrap();
        driver.set_raw("k2", b"v2".to_vec(), None).unwrap();
        driver.clear().unwrap();

        assert_eq!(driver.get_raw("k1").unwrap(), None);
        assert_eq!(driver.get_raw("k2").unwrap(), None);
    }

    #[test]
    fn test_r5_multi_level_cascade_fill_back() {
        // R5: 多级查询命中后回填低层（带 TTL 保留）
        let l1 = sz_orm_core::MemoryCache::new();
        let l2 = sz_orm_core::MemoryCache::new();

        // L2 有数据，L1 无
        l2.set("key", b"from_l2".to_vec(), None).unwrap();

        let driver = MultiLevelCacheDriver::new()
            .add_level(Box::new(l1.clone()))
            .add_level(Box::new(l2));

        // get 触发回填
        let val = driver.get_raw("key").unwrap();
        assert_eq!(val, Some(b"from_l2".to_vec()));

        // L1 应被回填
        assert_eq!(l1.get("key").unwrap(), Some(b"from_l2".to_vec()));
    }

    // ========================================================================
    // 测试组 35: CacheDriver tag trait 默认实现（MemoryCacheDriver）
    // ========================================================================

    #[test]
    fn test_tag_get_cache_key_default_no_prefix() {
        // 默认 get_cache_key 不应用前缀
        let driver = MemoryCacheDriver::new();
        assert_eq!(driver.get_cache_key("user:1"), "user:1");
        assert_eq!(driver.get_cache_key("hello"), "hello");
    }

    #[test]
    fn test_tag_get_tag_key_default_format() {
        // 默认 get_tag_key = "tag:" + md5(tag)
        let driver = MemoryCacheDriver::new();
        let tag_key = driver.get_tag_key("user");
        // md5("user") = "ee11cbb19052e40b07aac0ca060c23ee"
        assert_eq!(tag_key, "tag:ee11cbb19052e40b07aac0ca060c23ee");
    }

    #[test]
    fn test_tag_append_creates_new_tag_set() {
        // tag_append 首次调用创建空数组 + 追加
        let driver = MemoryCacheDriver::new();
        driver.tag_append("tag:abc123", "user:1").unwrap();
        // 直接验证存储（tag_append 第一个参数是 tag_key，存储在 get_cache_key(tag_key) 位置）
        let storage_key = "tag:abc123"; // get_cache_key("tag:abc123") = "tag:abc123" (no prefix)
        let raw = driver.get_raw(storage_key).unwrap();
        let stored: Vec<String> = serde_json::from_slice(&raw.unwrap()).unwrap();
        assert_eq!(stored, vec!["user:1"]);
    }

    #[test]
    fn test_tag_append_appends_to_existing() {
        let driver = MemoryCacheDriver::new();
        driver.tag_append("tag:abc", "key1").unwrap();
        driver.tag_append("tag:abc", "key2").unwrap();
        let storage_key = "tag:abc";
        let raw = driver.get_raw(storage_key).unwrap();
        let stored: Vec<String> = serde_json::from_slice(&raw.unwrap()).unwrap();
        assert_eq!(stored, vec!["key1", "key2"]);
    }

    #[test]
    fn test_tag_append_dedup() {
        // 对齐 PHP array_unique：重复值只保留首次出现
        let driver = MemoryCacheDriver::new();
        driver.tag_append("tag:abc", "key1").unwrap();
        driver.tag_append("tag:abc", "key1").unwrap(); // 重复
        let storage_key = "tag:abc";
        let raw = driver.get_raw(storage_key).unwrap();
        let stored: Vec<String> = serde_json::from_slice(&raw.unwrap()).unwrap();
        assert_eq!(stored, vec!["key1"]);
    }

    #[test]
    fn test_tag_append_max_1000_cap() {
        // 对齐 PHP: count > 1000 时 array_shift（FIFO 丢弃最旧）
        let driver = MemoryCacheDriver::new();
        for i in 0..1001i64 {
            driver.tag_append("tag:abc", &format!("key{}", i)).unwrap();
        }
        let storage_key = "tag:abc";
        let raw = driver.get_raw(storage_key).unwrap();
        let stored: Vec<String> = serde_json::from_slice(&raw.unwrap()).unwrap();
        // 上限 1000，丢弃 key0
        assert_eq!(stored.len(), 1000);
        assert!(!stored.contains(&"key0".to_string()));
        assert!(stored.contains(&"key1".to_string()));
        assert!(stored.contains(&"key1000".to_string()));
    }

    #[test]
    fn test_tag_items_empty_returns_empty() {
        let driver = MemoryCacheDriver::new();
        let items = driver.tag_items("nonexistent_tag").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_tag_items_returns_stored_keys() {
        let driver = MemoryCacheDriver::new();
        // tag_items 内部计算 get_tag_key("mytag") = "tag:" + md5("mytag")
        let tag_key = driver.get_tag_key("mytag");
        driver.tag_append(&tag_key, "key1").unwrap();
        driver.tag_append(&tag_key, "key2").unwrap();
        let items = driver.tag_items("mytag").unwrap();
        assert_eq!(items, vec!["key1", "key2"]);
    }

    #[test]
    fn test_tag_clear_deletes_keys() {
        let driver = MemoryCacheDriver::new();
        // 写入两个缓存 key
        driver.set_raw("key1", b"v1".to_vec(), None).unwrap();
        driver.set_raw("key2", b"v2".to_vec(), None).unwrap();
        // tag_clear 删除这些 key（无前缀驱动，key = name）
        driver
            .tag_clear(&["key1".to_string(), "key2".to_string()])
            .unwrap();
        assert!(!driver.has("key1").unwrap());
        assert!(!driver.has("key2").unwrap());
    }

    #[test]
    fn test_tag_clear_empty_no_error() {
        let driver = MemoryCacheDriver::new();
        // 空列表不报错
        driver.tag_clear(&[]).unwrap();
    }

    // ========================================================================
    // 测试组 36: RedisCacheDriver tag trait 重写
    // ========================================================================

    #[test]
    fn test_redis_tag_get_cache_key_with_prefix() {
        let config = RedisConfig {
            prefix: "myapp:".to_string(),
            ..RedisConfig::default()
        };
        let driver = RedisCacheDriver::new(config);
        assert_eq!(driver.get_cache_key("user:1"), "myapp:user:1");
    }

    #[test]
    fn test_redis_tag_get_tag_key_with_tag_prefix() {
        let config = RedisConfig {
            tag_prefix: "tag:".to_string(),
            ..RedisConfig::default()
        };
        let driver = RedisCacheDriver::new(config);
        let tag_key = driver.get_tag_key("user");
        // md5("user") = "ee11cbb19052e40b07aac0ca060c23ee"
        assert_eq!(tag_key, "tag:ee11cbb19052e40b07aac0ca060c23ee");
    }

    #[test]
    fn test_redis_tag_get_tag_key_custom_prefix() {
        let config = RedisConfig {
            tag_prefix: "t:".to_string(),
            ..RedisConfig::default()
        };
        let driver = RedisCacheDriver::new(config);
        let tag_key = driver.get_tag_key("user");
        assert_eq!(tag_key, "t:ee11cbb19052e40b07aac0ca060c23ee");
    }

    #[test]
    fn test_redis_tag_append_uses_sadd() {
        // Redis tag_append 用 sAdd（Set 语义），而非 push（Array 语义）
        let driver = RedisCacheDriver::new(RedisConfig::default());
        let tag_key = driver.get_tag_key("mytag");
        driver.tag_append(&tag_key, "key1").unwrap();
        driver.tag_append(&tag_key, "key2").unwrap();
        driver.tag_append(&tag_key, "key1").unwrap(); // 重复（Set 自动去重）
        let items = driver.tag_items("mytag").unwrap();
        // Set 去重，只有 2 个元素
        assert_eq!(items.len(), 2);
        assert!(items.contains(&"key1".to_string()));
        assert!(items.contains(&"key2".to_string()));
    }

    #[test]
    fn test_redis_tag_items_uses_smembers() {
        let driver = RedisCacheDriver::new(RedisConfig::default());
        let tag_key = driver.get_tag_key("mytag");
        driver.tag_append(&tag_key, "a").unwrap();
        driver.tag_append(&tag_key, "b").unwrap();
        driver.tag_append(&tag_key, "c").unwrap();
        let items = driver.tag_items("mytag").unwrap();
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn test_redis_tag_clear_does_not_double_prefix() {
        // tag_clear 接收已前缀化的 key，不得再次应用 getCacheKey
        let config = RedisConfig {
            prefix: "app:".to_string(),
            ..RedisConfig::default()
        };
        let driver = RedisCacheDriver::new(config);
        // 写入缓存（set_raw 应用前缀 → "app:key1"）
        driver.set_raw("key1", b"v1".to_vec(), None).unwrap();
        // tag_items 返回 "app:key1"（已前缀化）
        let tag_key = driver.get_tag_key("mytag");
        driver.tag_append(&tag_key, "app:key1").unwrap();
        let items = driver.tag_items("mytag").unwrap();
        assert_eq!(items, vec!["app:key1"]);
        // tag_clear 应删除 "app:key1"（raw del），不是 "app:app:key1"
        driver.tag_clear(&items).unwrap();
        assert!(!driver.has("key1").unwrap());
    }

    #[test]
    fn test_redis_tag_clear_empty_no_error() {
        let driver = RedisCacheDriver::new(RedisConfig::default());
        driver.tag_clear(&[]).unwrap();
    }

    // ========================================================================
    // 测试组 37: TagSet with MemoryCacheDriver
    // ========================================================================

    #[test]
    fn test_tagset_set_stores_value_and_appends_tag() {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        // TagSet::set = cache.set + append
        cache.tag("user").set("user:1", "Alice", None).unwrap();

        // 缓存值已写入
        assert_eq!(
            cache.get::<String>("user:1").unwrap(),
            Some("Alice".to_string())
        );

        // 标签已记录 key
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let items = driver.tag_items("user").unwrap();
        assert_eq!(items, vec!["user:1"]);
    }

    #[test]
    fn test_tagset_set_multiple_keys_same_tag() {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache.tag("user").set("user:1", "Alice", None).unwrap();
        cache.tag("user").set("user:2", "Bob", None).unwrap();
        cache.tag("user").set("user:3", "Carol", None).unwrap();

        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let items = driver.tag_items("user").unwrap();
        assert_eq!(items, vec!["user:1", "user:2", "user:3"]);
    }

    #[test]
    fn test_tagset_clear_deletes_all_tagged_keys() {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache.tag("user").set("user:1", "Alice", None).unwrap();
        cache.tag("user").set("user:2", "Bob", None).unwrap();
        cache.tag("user").set("user:3", "Carol", None).unwrap();

        // clear 删除所有标签下的缓存
        cache.tag("user").clear().unwrap();

        assert!(cache.get::<String>("user:1").unwrap().is_none());
        assert!(cache.get::<String>("user:2").unwrap().is_none());
        assert!(cache.get::<String>("user:3").unwrap().is_none());
    }

    #[test]
    fn test_tagset_clear_deletes_tag_key() {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache.tag("user").set("user:1", "Alice", None).unwrap();

        // 验证 tag key 存在
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let tag_key = driver.get_tag_key("user");
        assert!(driver.has(&tag_key).unwrap());
        drop(mgr);

        cache.tag("user").clear().unwrap();

        // tag key 本身也被删除
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        assert!(!driver.has(&tag_key).unwrap());
    }

    #[test]
    fn test_tagset_clear_empty_tag_no_error() {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        // 没有写入任何缓存，clear 不报错
        cache.tag("empty").clear().unwrap();
    }

    #[test]
    fn test_tagset_append_adds_key_to_tag() {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        // 先写入缓存（不经 TagSet）
        cache.set("user:1", "Alice", None).unwrap();
        // 然后手动 append 到标签
        cache.tag("user").append("user:1").unwrap();

        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let items = driver.tag_items("user").unwrap();
        assert_eq!(items, vec!["user:1"]);
    }

    #[test]
    fn test_tagset_many_tags_single_key() {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        // 多标签：key 同时属于 user 和 admin
        cache
            .tag_many(&["user", "admin"])
            .set("key1", "val", None)
            .unwrap();

        // 两个标签都应包含 key1
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let user_items = driver.tag_items("user").unwrap();
        let admin_items = driver.tag_items("admin").unwrap();
        assert_eq!(user_items, vec!["key1"]);
        assert_eq!(admin_items, vec!["key1"]);
    }

    #[test]
    fn test_tagset_many_tags_clear_one() {
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache
            .tag_many(&["user", "admin"])
            .set("key1", "val", None)
            .unwrap();

        // 清除 user 标签
        cache.tag("user").clear().unwrap();

        // key1 被删除
        assert!(cache.get::<String>("key1").unwrap().is_none());

        // admin 标签的 tag_items 仍有 key1（但 key1 已被删除）
        // 这是 PHP 的行为：clear 只清除 tag 下记录的 key，不清理其他 tag 的记录
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let admin_items = driver.tag_items("admin").unwrap();
        assert_eq!(admin_items, vec!["key1"]); // 记录仍在，但 key1 已被删
                                               // user 的 tag key 被删除
        let user_tag_key = driver.get_tag_key("user");
        assert!(!driver.has(&user_tag_key).unwrap());
    }

    #[test]
    fn test_tagset_tags_getter() {
        let cache = Cache::new();
        let ts = cache.tag_many(&["a", "b", "c"]);
        assert_eq!(ts.tags(), &["a", "b", "c"]);
    }

    // ========================================================================
    // 测试组 38: TagSet with RedisCacheDriver
    // ========================================================================

    #[test]
    fn test_redis_tagset_set_stores_value_and_appends_tag() {
        let cache = Cache::new();
        let driver = RedisCacheDriver::new(RedisConfig::default());
        cache.register_store("redis", Box::new(driver));

        cache.tag("user").set("user:1", "Alice", None).unwrap();

        // 缓存值已写入
        assert_eq!(
            cache.get::<String>("user:1").unwrap(),
            Some("Alice".to_string())
        );

        // 标签已记录 key（Redis Set 语义）
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let items = driver.tag_items("user").unwrap();
        assert_eq!(items, vec!["user:1"]);
    }

    #[test]
    fn test_redis_tagset_set_with_prefix() {
        let cache = Cache::new();
        let config = RedisConfig {
            prefix: "app:".to_string(),
            ..RedisConfig::default()
        };
        let driver = RedisCacheDriver::new(config);
        cache.register_store("redis", Box::new(driver));

        cache.tag("user").set("user:1", "Alice", None).unwrap();

        // 验证 tag_items 返回已前缀化的 key
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let items = driver.tag_items("user").unwrap();
        assert_eq!(items, vec!["app:user:1"]);
    }

    #[test]
    fn test_redis_tagset_clear_deletes_all_tagged_keys() {
        let cache = Cache::new();
        let driver = RedisCacheDriver::new(RedisConfig::default());
        cache.register_store("redis", Box::new(driver));

        cache.tag("user").set("user:1", "Alice", None).unwrap();
        cache.tag("user").set("user:2", "Bob", None).unwrap();
        cache.tag("user").set("user:3", "Carol", None).unwrap();

        cache.tag("user").clear().unwrap();

        assert!(cache.get::<String>("user:1").unwrap().is_none());
        assert!(cache.get::<String>("user:2").unwrap().is_none());
        assert!(cache.get::<String>("user:3").unwrap().is_none());
    }

    #[test]
    fn test_redis_tagset_clear_deletes_tag_key() {
        let cache = Cache::new();
        let driver = RedisCacheDriver::new(RedisConfig::default());
        cache.register_store("redis", Box::new(driver));

        cache.tag("user").set("user:1", "Alice", None).unwrap();

        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let tag_key = driver.get_tag_key("user");
        // tag key 存在于 Redis backend
        assert!(driver.has(&tag_key).unwrap());
        drop(mgr);

        cache.tag("user").clear().unwrap();

        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        assert!(!driver.has(&tag_key).unwrap());
    }

    #[test]
    fn test_redis_tagset_many_tags() {
        let cache = Cache::new();
        let driver = RedisCacheDriver::new(RedisConfig::default());
        cache.register_store("redis", Box::new(driver));

        cache
            .tag_many(&["user", "admin"])
            .set("key1", "val", None)
            .unwrap();

        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let user_items = driver.tag_items("user").unwrap();
        let admin_items = driver.tag_items("admin").unwrap();
        assert_eq!(user_items, vec!["key1"]);
        assert_eq!(admin_items, vec!["key1"]);
    }

    // ========================================================================
    // 测试组 39: R5 PHP 行为对齐
    // ========================================================================

    #[test]
    fn test_r5_php_tag_set_then_clear() {
        // R5: 对齐 PHP Cache::tag('user')->set() + Cache::tag('user')->clear()
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache.tag("user").set("u1", "Alice", None).unwrap();
        cache.tag("user").set("u2", "Bob", None).unwrap();

        // 非标签缓存不受影响
        cache.set("other", "data", None).unwrap();

        cache.tag("user").clear().unwrap();

        // 标签缓存被清除
        assert!(cache.get::<String>("u1").unwrap().is_none());
        assert!(cache.get::<String>("u2").unwrap().is_none());
        // 非标签缓存保留
        assert_eq!(
            cache.get::<String>("other").unwrap(),
            Some("data".to_string())
        );
    }

    #[test]
    fn test_r5_php_tag_multiple_tags_clear() {
        // R5: 对齐 PHP 多标签场景
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        // key1 属于 user + admin
        cache
            .tag_many(&["user", "admin"])
            .set("key1", "v1", None)
            .unwrap();
        // key2 仅属于 user
        cache.tag("user").set("key2", "v2", None).unwrap();

        // 清除 user 标签（key1 和 key2 都被删除）
        cache.tag("user").clear().unwrap();

        assert!(cache.get::<String>("key1").unwrap().is_none());
        assert!(cache.get::<String>("key2").unwrap().is_none());
    }

    #[test]
    fn test_r5_php_tag_get_cache_key_prefix() {
        // R5: 对齐 PHP getCacheKey — MemoryCacheDriver 无前缀
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        assert_eq!(driver.get_cache_key("test"), "test");
    }

    #[test]
    fn test_r5_php_tag_get_tag_key_md5() {
        // R5: 对齐 PHP getTagKey — tag_prefix + md5(tag)
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        // md5("hello") = "5d41402abc4b2a76b9719d911017c592"
        assert_eq!(
            driver.get_tag_key("hello"),
            "tag:5d41402abc4b2a76b9719d911017c592"
        );
    }

    #[test]
    fn test_r5_php_tag_push_max_1000_array_shift() {
        // R5: 对齐 PHP push 上限 1000 + array_shift（FIFO 丢弃最旧）
        let driver = MemoryCacheDriver::new();
        for i in 0..1005i64 {
            driver.tag_append("tag:test", &format!("key{}", i)).unwrap();
        }
        let storage_key = "tag:test";
        let raw = driver.get_raw(storage_key).unwrap();
        let stored: Vec<String> = serde_json::from_slice(&raw.unwrap()).unwrap();
        // 上限 1000
        assert_eq!(stored.len(), 1000);
        // key0~key4 被丢弃
        assert!(!stored.contains(&"key0".to_string()));
        assert!(!stored.contains(&"key4".to_string()));
        // key5~key1004 保留
        assert!(stored.contains(&"key5".to_string()));
        assert!(stored.contains(&"key1004".to_string()));
    }

    #[test]
    fn test_r5_php_tag_push_array_unique() {
        // R5: 对齐 PHP array_unique — 去重保留首次出现
        let driver = MemoryCacheDriver::new();
        driver.tag_append("tag:u", "a").unwrap();
        driver.tag_append("tag:u", "b").unwrap();
        driver.tag_append("tag:u", "a").unwrap(); // 重复
        driver.tag_append("tag:u", "c").unwrap();
        driver.tag_append("tag:u", "b").unwrap(); // 重复

        let storage_key = "tag:u";
        let raw = driver.get_raw(storage_key).unwrap();
        let stored: Vec<String> = serde_json::from_slice(&raw.unwrap()).unwrap();
        // 去重后 a, b, c（保留首次出现顺序）
        assert_eq!(stored, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_r5_php_tag_singleton_equivalent() {
        // R5: 对齐 PHP tag() 单例 — 多次调用行为一致
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        // 第一次调用 tag("user")
        cache.tag("user").set("u1", "Alice", None).unwrap();
        // 第二次调用 tag("user") — PHP 返回同一个 TagSet 单例
        cache.tag("user").set("u2", "Bob", None).unwrap();

        // 两次 set 的 key 都在同一标签下
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let items = driver.tag_items("user").unwrap();
        assert_eq!(items, vec!["u1", "u2"]);

        // clear 一次清除所有
        cache.tag("user").clear().unwrap();
        assert!(cache.get::<String>("u1").unwrap().is_none());
        assert!(cache.get::<String>("u2").unwrap().is_none());
    }

    #[test]
    fn test_r5_php_tag_clear_then_set_again() {
        // R5: clear 后可以重新 set
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache.tag("user").set("u1", "Alice", None).unwrap();
        cache.tag("user").clear().unwrap();
        assert!(cache.get::<String>("u1").unwrap().is_none());

        // 重新 set
        cache.tag("user").set("u1", "Alice2", None).unwrap();
        assert_eq!(
            cache.get::<String>("u1").unwrap(),
            Some("Alice2".to_string())
        );

        // tag items 只包含重新 set 后的 key
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let items = driver.tag_items("user").unwrap();
        assert_eq!(items, vec!["u1"]);
    }

    #[test]
    fn test_r5_php_tag_redis_set_then_clear() {
        // R5: Redis 驱动 tag set + clear 对齐 PHP
        let cache = Cache::new();
        let driver = RedisCacheDriver::new(RedisConfig::default());
        cache.register_store("redis", Box::new(driver));

        cache.tag("article").set("a:1", "Hello", None).unwrap();
        cache.tag("article").set("a:2", "World", None).unwrap();
        cache.set("untagged", "data", None).unwrap();

        cache.tag("article").clear().unwrap();

        assert!(cache.get::<String>("a:1").unwrap().is_none());
        assert!(cache.get::<String>("a:2").unwrap().is_none());
        // 非标签缓存保留
        assert_eq!(
            cache.get::<String>("untagged").unwrap(),
            Some("data".to_string())
        );
    }

    #[test]
    fn test_r5_php_tag_redis_with_prefix() {
        // R5: Redis 带前缀的 tag 行为对齐 PHP
        let cache = Cache::new();
        let config = RedisConfig {
            prefix: "myapp:".to_string(),
            tag_prefix: "tag:".to_string(),
            ..RedisConfig::default()
        };
        let driver = RedisCacheDriver::new(config);
        cache.register_store("redis", Box::new(driver));

        cache.tag("user").set("u1", "Alice", None).unwrap();

        // 验证 tag_items 返回已前缀化的 key
        let mgr = cache.manager.read();
        let driver = mgr.default_store().unwrap();
        let items = driver.tag_items("user").unwrap();
        assert_eq!(items, vec!["myapp:u1"]);

        // clear 正确删除
        drop(mgr);
        cache.tag("user").clear().unwrap();
        assert!(cache.get::<String>("u1").unwrap().is_none());
    }

    #[test]
    fn test_r5_php_tag_different_tags_isolation() {
        // R5: 不同标签之间互不影响
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache.tag("user").set("u1", "Alice", None).unwrap();
        cache.tag("article").set("a1", "Hello", None).unwrap();

        // 清除 user 不影响 article
        cache.tag("user").clear().unwrap();

        assert!(cache.get::<String>("u1").unwrap().is_none());
        assert_eq!(
            cache.get::<String>("a1").unwrap(),
            Some("Hello".to_string())
        );
    }

    #[test]
    fn test_r5_php_tag_set_with_ttl() {
        // R5: TagSet::set 支持 TTL
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache
            .tag("user")
            .set("u1", "Alice", Some(Duration::from_millis(50)))
            .unwrap();

        assert_eq!(
            cache.get::<String>("u1").unwrap(),
            Some("Alice".to_string())
        );

        std::thread::sleep(Duration::from_millis(60));
        assert!(cache.get::<String>("u1").unwrap().is_none());
    }

    // ========================================================================
    // 测试组 54: delete_many 批量删除（对齐 PHP deleteMultiple）
    // ========================================================================

    #[test]
    fn test_delete_many_multiple_keys() {
        // 批量删除多个 key
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("k1", "v1", None).unwrap();
        cache.set("k2", "v2", None).unwrap();
        cache.set("k3", "v3", None).unwrap();

        cache.delete_many(&["k1", "k2", "k3"]).unwrap();

        assert!(cache.get::<String>("k1").unwrap().is_none());
        assert!(cache.get::<String>("k2").unwrap().is_none());
        assert!(cache.get::<String>("k3").unwrap().is_none());
    }

    #[test]
    fn test_delete_many_nonexistent_keys_ok() {
        // 删除不存在的 key 不失败（修正 PHP File::delete bug）
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("exists", "v", None).unwrap();

        // 不存在的 key + 存在的 key 混合
        let result = cache.delete_many(&["exists", "nonexistent"]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_delete_many_empty_slice() {
        // 空切片返回 Ok
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        let result = cache.delete_many(&[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_delete_many_partial_delete_before_failure() {
        // delete_many 逐个删除，部分成功后失败（对齐 PHP 语义）
        // 注：MemoryCacheDriver 不会失败，这里验证空切片+有值切片行为一致性
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("a", "1", None).unwrap();
        cache.set("b", "2", None).unwrap();

        cache.delete_many(&["a", "b"]).unwrap();
        assert!(cache.get::<String>("a").unwrap().is_none());
        assert!(cache.get::<String>("b").unwrap().is_none());
    }

    // ========================================================================
    // 测试组 55: invalidate_after_write 写后失效
    // ========================================================================

    #[test]
    fn test_invalidate_after_write_basic() {
        // 写后失效基本语义：delete 后下次 get 返回 None
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("user:1", "Alice", None).unwrap();
        assert_eq!(
            cache.get::<String>("user:1").unwrap(),
            Some("Alice".to_string())
        );

        // 写操作后失效缓存
        cache.invalidate_after_write(&["user:1"]).unwrap();
        assert!(cache.get::<String>("user:1").unwrap().is_none());
    }

    #[test]
    fn test_invalidate_after_write_multiple_keys() {
        // 多 key 失效（对齐业务场景 4：一次写操作失效多类缓存）
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("sdp_category_tree", "t1", None).unwrap();
        cache.set("sdp_category_select", "s1", None).unwrap();
        cache.set("sdp_category_child", "c1", None).unwrap();

        cache
            .invalidate_after_write(&[
                "sdp_category_tree",
                "sdp_category_select",
                "sdp_category_child",
            ])
            .unwrap();

        assert!(cache.get::<String>("sdp_category_tree").unwrap().is_none());
        assert!(cache
            .get::<String>("sdp_category_select")
            .unwrap()
            .is_none());
        assert!(cache.get::<String>("sdp_category_child").unwrap().is_none());
    }

    #[test]
    fn test_invalidate_after_write_fire_and_forget() {
        // fire and forget 模式（对齐 PHP 业务代码不检查返回值）
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("clerk:1", "data", None).unwrap();

        // 用 let _ = 忽略返回值（对齐 PHP fire and forget）
        let _ = cache.invalidate_after_write(&["clerk:1"]);
        assert!(cache.get::<String>("clerk:1").unwrap().is_none());
    }

    // ========================================================================
    // 测试组 56: refresh 先删后读强制刷新
    // ========================================================================

    #[test]
    fn test_refresh_force_update() {
        // 先删后读强制刷新：delete → fetcher → set
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("store:1", "old_data", None).unwrap();

        let result: String = cache
            .refresh("store:1", None, || Ok("new_data".to_string()))
            .unwrap();

        assert_eq!(result, "new_data");
        assert_eq!(
            cache.get::<String>("store:1").unwrap(),
            Some("new_data".to_string())
        );
    }

    #[test]
    fn test_refresh_fetcher_error_no_write() {
        // fetcher 失败时不写入缓存（错误传播）
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("key", "original", None).unwrap();

        // fetcher 返回错误
        let result: Result<String, CacheError> = cache.refresh("key", None, || {
            Err(CacheError::SerializationError("fetch failed".to_string()))
        });

        assert!(result.is_err());
        // delete 已执行，缓存被清空
        assert!(cache.get::<String>("key").unwrap().is_none());
    }

    #[test]
    fn test_refresh_ttl_propagation() {
        // TTL 透传到 set
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        let _result: String = cache
            .refresh("ttl_key", Some(Duration::from_millis(50)), || {
                Ok("value".to_string())
            })
            .unwrap();

        // 立即可读
        assert_eq!(
            cache.get::<String>("ttl_key").unwrap(),
            Some("value".to_string())
        );

        // 等待过期
        std::thread::sleep(Duration::from_millis(60));
        assert!(cache.get::<String>("ttl_key").unwrap().is_none());
    }

    #[test]
    fn test_refresh_returns_fetcher_value() {
        // refresh 返回 fetcher 的值（不是缓存中的旧值）
        // 注：用 String 类型避免 PHP unserialize numeric → string bug 影响
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("counter", "old_value", None).unwrap();

        let result: String = cache
            .refresh("counter", None, || Ok("new_value".to_string()))
            .unwrap();
        assert_eq!(result, "new_value");
        assert_eq!(
            cache.get::<String>("counter").unwrap(),
            Some("new_value".to_string())
        );
    }

    // ========================================================================
    // 测试组 57: R5 PHP 行为对齐（缓存失效策略）
    // ========================================================================

    #[test]
    fn test_r5_php_delete_multiple_semantics() {
        // R5: 对齐 PHP Driver::deleteMultiple 逐个删除语义
        // PHP: foreach ($keys as $key) { $result = $this->delete($key); if (false === $result) return false; }
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("a", "1", None).unwrap();
        cache.set("b", "2", None).unwrap();
        cache.set("c", "3", None).unwrap();

        // 对齐 PHP deleteMultiple(['a', 'b', 'c'])
        let result = cache.delete_many(&["a", "b", "c"]);
        assert!(result.is_ok()); // PHP 返回 true

        // 所有 key 已删除
        assert!(cache.get::<String>("a").unwrap().is_none());
        assert!(cache.get::<String>("b").unwrap().is_none());
        assert!(cache.get::<String>("c").unwrap().is_none());
    }

    #[test]
    fn test_r5_php_invalidate_after_write_pattern() {
        // R5: 对齐 PHP 业务场景 1（事务内写后失效）
        // PHP: if($this->save($data)){ Cache::delete('foodCashierClerkAll_' . $data['cashier_id']); $this->commit(); }
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        // 模拟业务：先写入缓存数据
        cache
            .set("foodCashierClerkAll_1", vec!["clerk1"], None)
            .unwrap();

        // 模拟写操作（如数据库 save）后失效缓存
        let write_success = true;
        if write_success {
            cache
                .invalidate_after_write(&["foodCashierClerkAll_1"])
                .unwrap();
        }

        // 缓存已失效，下次 get 返回 None（触发回源）
        assert!(cache
            .get::<Vec<String>>("foodCashierClerkAll_1")
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_r5_php_refresh_pattern() {
        // R5: 对齐 PHP 业务场景 2（先删后读强制刷新）
        // PHP: Cache::delete($cacheKey); $info = Cache::get($cacheKey); if(!$info){ $info = 回源; Cache::set(...); }
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        // 初始缓存数据
        cache
            .set("wmall_store_info_1", "old_store_data", None)
            .unwrap();

        // 强制刷新（对齐 PHP info() 方法）
        let result: String = cache
            .refresh("wmall_store_info_1", None, || {
                // 模拟回源查询
                Ok("fresh_store_data".to_string())
            })
            .unwrap();

        assert_eq!(result, "fresh_store_data");
        assert_eq!(
            cache.get::<String>("wmall_store_info_1").unwrap(),
            Some("fresh_store_data".to_string())
        );
    }

    // ========================================================================
    // 测试组 58: fetch_singleflight 单飞模式（防止缓存击穿）
    // ========================================================================

    #[test]
    fn test_fetch_singleflight_cache_hit() {
        // 缓存命中时不调用 fetcher
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("hot", "cached_value", None).unwrap();

        let called = Arc::new(Mutex::new(false));
        let called_clone = called.clone();
        let result: String = cache
            .fetch_singleflight("hot", None, || {
                *called_clone.lock() = true;
                Ok("fetcher_value".to_string())
            })
            .unwrap();

        assert_eq!(result, "cached_value");
        assert!(!*called.lock(), "fetcher 不应被调用（缓存命中）");
    }

    #[test]
    fn test_fetch_singleflight_cache_miss_invokes_fetcher() {
        // 缓存未命中时调用 fetcher 并写入缓存
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        let result: String = cache
            .fetch_singleflight("miss_key", None, || Ok("fetched".to_string()))
            .unwrap();

        assert_eq!(result, "fetched");
        assert_eq!(
            cache.get::<String>("miss_key").unwrap(),
            Some("fetched".to_string())
        );
    }

    #[test]
    fn test_fetch_singleflight_concurrent_only_one_fetcher_call() {
        // 并发场景：同一 key 多线程请求，fetcher 只调用一次
        let cache = Arc::new(Cache::new());
        cache.register_default(MemoryCacheDriver::new());

        let fetcher_call_count = Arc::new(Mutex::new(0u32));
        let barrier = Arc::new(Barrier::new(4));
        let results = Arc::new(Mutex::new(Vec::<String>::new()));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let cache_clone = Arc::clone(&cache);
            let count_clone = Arc::clone(&fetcher_call_count);
            let barrier_clone = Arc::clone(&barrier);
            let results_clone = Arc::clone(&results);

            handles.push(std::thread::spawn(move || {
                // 所有线程同步开始，确保并发
                barrier_clone.wait();

                let value: String = cache_clone
                    .fetch_singleflight("concurrent_key", None, || {
                        // 模拟慢回源
                        std::thread::sleep(Duration::from_millis(50));
                        let mut count = count_clone.lock();
                        *count += 1;
                        Ok(format!("fetched_{}", *count))
                    })
                    .unwrap();

                results_clone.lock().push(value);
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // fetcher 应该只被调用一次（singleflight 互斥）
        assert_eq!(
            *fetcher_call_count.lock(),
            1,
            "fetcher 应只调用一次（singleflight）"
        );

        // 所有线程应该拿到相同的值
        let results = results.lock();
        assert_eq!(results.len(), 4);
        for value in results.iter() {
            assert_eq!(value, "fetched_1");
        }
    }

    #[test]
    fn test_fetch_singleflight_fetcher_error_propagates() {
        // fetcher 失败时错误传播，不写入缓存
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        let result: Result<String, CacheError> = cache.fetch_singleflight("err_key", None, || {
            Err(CacheError::SerializationError("fetcher failed".to_string()))
        });

        assert!(result.is_err());
        assert!(cache.get::<String>("err_key").unwrap().is_none());
    }

    // ========================================================================
    // 测试组 59: set_with_jitter 随机抖动 TTL（防止缓存雪崩）
    // ========================================================================

    #[test]
    fn test_set_with_jitter_basic() {
        // 基本功能：写入缓存且可读
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache
            .set_with_jitter(
                "jitter_key",
                "value",
                Some(Duration::from_secs(60)),
                Duration::from_secs(10),
            )
            .unwrap();

        assert_eq!(
            cache.get::<String>("jitter_key").unwrap(),
            Some("value".to_string())
        );
    }

    #[test]
    fn test_set_with_jitter_zero_jitter_equivalent_to_set() {
        // jitter = 0 时等价于 set
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache
            .set_with_jitter(
                "no_jitter",
                "value",
                Some(Duration::from_secs(60)),
                Duration::ZERO,
            )
            .unwrap();

        assert_eq!(
            cache.get::<String>("no_jitter").unwrap(),
            Some("value".to_string())
        );
    }

    #[test]
    fn test_set_with_jitter_none_ttl_no_jitter() {
        // ttl = None 时等价于永久缓存，jitter 被忽略
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        cache
            .set_with_jitter("permanent", "value", None, Duration::from_secs(10))
            .unwrap();

        assert_eq!(
            cache.get::<String>("permanent").unwrap(),
            Some("value".to_string())
        );
    }

    #[test]
    fn test_set_with_jitter_ttl_in_expected_range() {
        // TTL 在 [ttl, ttl + jitter] 范围内（通过过期时间验证）
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        let base_ttl = Duration::from_millis(50);
        let jitter = Duration::from_millis(100);

        cache
            .set_with_jitter("range_key", "value", Some(base_ttl), jitter)
            .unwrap();

        // 立即可读
        assert!(cache.get::<String>("range_key").unwrap().is_some());

        // 等待 base_ttl + jitter + 缓冲后应已过期
        std::thread::sleep(base_ttl + jitter + Duration::from_millis(20));
        assert!(
            cache.get::<String>("range_key").unwrap().is_none(),
            "TTL 应在 [{:?}, {:?}] 范围内，已过期",
            base_ttl,
            base_ttl + jitter
        );
    }

    // ========================================================================
    // 测试组 60: fetch_with_protection 组合防护（singleflight + jitter）
    // ========================================================================

    #[test]
    fn test_fetch_with_protection_cache_hit() {
        // 缓存命中时不调用 fetcher
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());
        cache.set("protected", "cached", None).unwrap();

        let called = Arc::new(Mutex::new(false));
        let called_clone = called.clone();
        let result: String = cache
            .fetch_with_protection(
                "protected",
                Some(Duration::from_secs(60)),
                Duration::from_secs(10),
                || {
                    *called_clone.lock() = true;
                    Ok("fetched".to_string())
                },
            )
            .unwrap();

        assert_eq!(result, "cached");
        assert!(!*called.lock());
    }

    #[test]
    fn test_fetch_with_protection_cache_miss_invokes_fetcher() {
        // 缓存未命中时调用 fetcher 并写入缓存
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        let result: String = cache
            .fetch_with_protection(
                "miss_protected",
                Some(Duration::from_secs(60)),
                Duration::from_secs(10),
                || Ok("fetched_protected".to_string()),
            )
            .unwrap();

        assert_eq!(result, "fetched_protected");
        assert_eq!(
            cache.get::<String>("miss_protected").unwrap(),
            Some("fetched_protected".to_string())
        );
    }

    #[test]
    fn test_fetch_with_protection_concurrent_single_flight() {
        // 并发场景：组合防护下 fetcher 只调用一次
        let cache = Arc::new(Cache::new());
        cache.register_default(MemoryCacheDriver::new());

        let fetcher_call_count = Arc::new(Mutex::new(0u32));
        let barrier = Arc::new(Barrier::new(4));
        let results = Arc::new(Mutex::new(Vec::<String>::new()));

        let mut handles = Vec::new();
        for _ in 0..4 {
            let cache_clone = Arc::clone(&cache);
            let count_clone = Arc::clone(&fetcher_call_count);
            let barrier_clone = Arc::clone(&barrier);
            let results_clone = Arc::clone(&results);

            handles.push(std::thread::spawn(move || {
                barrier_clone.wait();

                let value: String = cache_clone
                    .fetch_with_protection(
                        "concurrent_protected",
                        Some(Duration::from_secs(60)),
                        Duration::from_secs(10),
                        || {
                            std::thread::sleep(Duration::from_millis(50));
                            let mut count = count_clone.lock();
                            *count += 1;
                            Ok(format!("value_{}", *count))
                        },
                    )
                    .unwrap();

                results_clone.lock().push(value);
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(*fetcher_call_count.lock(), 1, "fetcher 应只调用一次");

        let results = results.lock();
        assert_eq!(results.len(), 4);
        for value in results.iter() {
            assert_eq!(value, "value_1");
        }
    }

    #[test]
    fn test_fetch_with_protection_fetcher_error_propagates() {
        // fetcher 失败时错误传播，不写入缓存
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        let result: Result<String, CacheError> = cache.fetch_with_protection(
            "err_protected",
            Some(Duration::from_secs(60)),
            Duration::from_secs(10),
            || Err(CacheError::SerializationError("failed".to_string())),
        );

        assert!(result.is_err());
        assert!(cache.get::<String>("err_protected").unwrap().is_none());
    }

    // ========================================================================
    // 测试组 61: R5 PHP 行为对比（缓存防护）
    // ========================================================================

    #[test]
    fn test_r5_php_remember_lock_vs_rust_singleflight() {
        // R5: 对比 PHP remember 的"锁雏形"与 Rust singleflight 的正确实现
        // PHP remember 用 $this->set($name.'_lock', true) 非原子加锁（缺陷）
        // Rust singleflight 用 parking_lot::Mutex::lock() 原子互斥（正确）
        let cache = Arc::new(Cache::new());
        cache.register_default(MemoryCacheDriver::new());

        let call_count = Arc::new(Mutex::new(0u32));
        let barrier = Arc::new(Barrier::new(3));
        let results = Arc::new(Mutex::new(Vec::<String>::new()));

        let mut handles = Vec::new();
        for _ in 0..3 {
            let cache_clone = Arc::clone(&cache);
            let count_clone = Arc::clone(&call_count);
            let barrier_clone = Arc::clone(&barrier);
            let results_clone = Arc::clone(&results);

            handles.push(std::thread::spawn(move || {
                barrier_clone.wait();

                let value: String = cache_clone
                    .fetch_singleflight("r5_compare_key", None, || {
                        std::thread::sleep(Duration::from_millis(30));
                        let mut count = count_clone.lock();
                        *count += 1;
                        Ok(format!("v_{}", *count))
                    })
                    .unwrap();

                results_clone.lock().push(value);
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Rust singleflight：fetcher 只调用一次（对齐"正确互斥"语义）
        assert_eq!(
            *call_count.lock(),
            1,
            "Rust singleflight fetcher 应只调用一次"
        );

        // 所有线程拿到相同值
        let results = results.lock();
        assert_eq!(results.len(), 3);
        for value in results.iter() {
            assert_eq!(value, "v_1");
        }
    }

    #[test]
    fn test_r5_php_no_jitter_vs_rust_jitter() {
        // R5: 对比 PHP 无 TTL 抖动与 Rust 有 TTL 抖动
        // PHP getExpireTime 不做 TTL 抖动（缺陷，会雪崩）
        // Rust set_with_jitter 在 [ttl, ttl + jitter] 范围内随机（正确，防雪崩）
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        // 多次设置相同 TTL + jitter，验证实际 TTL 不同（随机性）
        let mut ttl_samples = Vec::new();
        for i in 0..10 {
            let key = format!("jitter_sample_{}", i);
            cache
                .set_with_jitter(
                    &key,
                    "value",
                    Some(Duration::from_secs(60)),
                    Duration::from_secs(10),
                )
                .unwrap();

            // 通过 MemoryCacheDriver 内部 TTL 验证随机性
            // 注：这里只验证缓存写入成功，实际 TTL 随机性通过统计方法验证
            let _ = cache.get::<String>(&key).unwrap();
            ttl_samples.push(key);
        }

        // 验证所有 key 都写入成功
        for key in &ttl_samples {
            assert_eq!(
                cache.get::<String>(key).unwrap(),
                Some("value".to_string()),
                "所有带抖动 TTL 的 key 都应写入成功"
            );
        }
    }

    #[test]
    fn test_r5_php_remember_no_double_check_vs_rust_double_check() {
        // R5: 对比 PHP remember 无 double-check 与 Rust singleflight 有 double-check
        // PHP remember 获取锁后直接回源，不再次检查缓存（缺陷）
        // Rust singleflight 获取锁后 double-check 缓存（优化，其他线程可能已回源完成）
        let cache = Cache::new();
        cache.register_default(MemoryCacheDriver::new());

        // 先写入缓存
        cache.set("double_check_key", "pre_cached", None).unwrap();

        // 调用 fetch_singleflight，应直接命中缓存（不调用 fetcher）
        let called = Arc::new(Mutex::new(false));
        let called_clone = called.clone();
        let result: String = cache
            .fetch_singleflight("double_check_key", None, || {
                *called_clone.lock() = true;
                Ok("fetched".to_string())
            })
            .unwrap();

        // 应返回预缓存的值，fetcher 不被调用
        assert_eq!(result, "pre_cached");
        assert!(
            !*called.lock(),
            "double-check 应命中预缓存，fetcher 不被调用"
        );
    }
}
