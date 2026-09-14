// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Memcached 缓存驱动 — 对齐 PHP `think\cache\driver\Memcached`
//!
//! ## PHP 对齐
//!
//! PHP `think\cache\driver\Memcached` 通过 `Memcached` 扩展连接 Memcached 服务器，
//! 提供 KV 缓存操作。本模块通过 `MemcachedBackend` trait 抽象 Memcached 协议命令，
//! 允许应用层注入真实 backend（如 `memcache` crate 包装）。
//!
//! ## 与 Redis 驱动的差异
//!
//! | 特性 | Redis | Memcached |
//! |------|-------|-----------|
//! | 数据结构 | KV + Set + Hash 等 | 仅 KV |
//! | TTL | 可选 | 必填（最大 30 天） |
//! | 持久化 | 支持 RDB/AOF | 不支持 |
//! | key 前缀 | 服务端无 | 客户端实现 |
//! | 标签 | 通过 SADD/SMEMBERS | 通过模拟 Set |
//! | 批量删除 | DEL key1 key2 | 逐个 delete |
//!
//! ## 标签模拟
//!
//! Memcached 不支持 Set 数据结构，本驱动通过 JSON 序列化模拟标签集合：
//! - `tag_append`：读取标签 Set JSON → 追加 → 写回
//! - `tag_items`：读取标签 Set JSON
//! - `tag_clear`：逐个删除标签内记录的 key

use std::collections::HashMap;
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;

use parking_lot::RwLock;

use crate::compute_md5;
use crate::CacheDriver;
use crate::CacheError;

/// Memcached value 最大大小（1MB，对齐 Memcached 协议）
const MEMCACHED_MAX_VALUE_SIZE: usize = 1024 * 1024;

/// Memcached key 最大长度（250 字节，对齐 Memcached 协议）
const MEMCACHED_MAX_KEY_LEN: usize = 250;

/// Memcached 最大 TTL（30 天，2592000 秒）
const MEMCACHED_MAX_TTL_SECS: u64 = 30 * 24 * 60 * 60;

/// Memcached 配置（对齐 PHP `think\cache\driver\Memcached` 的 `$options`）
///
/// PHP 配置示例：
/// ```php
/// 'memcached' => [
///     'host' => '127.0.0.1',
///     'port' => 11211,
///     'expire' => 3600,
///     'prefix' => '',
///     'timeout' => 1000,
///     'weight' => 0,
/// ],
/// ```
#[derive(Debug, Clone)]
pub struct MemcachedConfig {
    /// Memcached 主机地址（对齐 PHP `host`）
    pub host: String,
    /// Memcached 端口（对齐 PHP `port`，默认 11211）
    pub port: u16,
    /// 默认过期时间（对齐 PHP `expire`，`None` 表示使用最大 TTL 30 天）
    pub expire: Option<Duration>,
    /// key 前缀（对齐 PHP `prefix`，客户端实现）
    pub prefix: String,
    /// tag 前缀（对齐 PHP `tag_prefix`）
    pub tag_prefix: String,
    /// 连接超时（对齐 PHP `timeout`，毫秒；`Duration::ZERO` 表示无超时）
    pub timeout: Duration,
    /// 服务器权重（对齐 PHP `weight`，多服务器场景使用）
    pub weight: u32,
}

impl Default for MemcachedConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 11211,
            expire: None,
            prefix: String::new(),
            tag_prefix: "tag:".to_string(),
            timeout: Duration::ZERO,
            weight: 0,
        }
    }
}

impl MemcachedConfig {
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

/// Memcached 后端 trait（抽象 Memcached 协议命令）
///
/// 由于 Rust 端不强制依赖 `memcache`/`memcached` crate，本 trait 抽象 Memcached 命令，
/// 允许应用层注入真实 backend。
///
/// ## 命令映射
///
/// | Trait 方法 | Memcached 命令 | PHP Memcached 调用 |
/// |------------|---------------|-------------------|
/// | `get` | GET | `$mc->get($key)` |
/// | `set` | SET（带 TTL） | `$mc->set($key, $value, $ttl)` |
/// | `delete` | DELETE | `$mc->delete($key)` |
/// | `increment` | INCR | `$mc->increment($key, $offset)` |
/// | `decrement` | DECR | `$mc->decrement($key, $offset)` |
/// | `flush` | FLUSH_ALL | `$mc->flush()` |
/// | `touch` | TOUCH（设置 TTL） | `$mc->touch($key, $ttl)` |
pub trait MemcachedBackend: Send + Sync {
    /// GET 命令 — 读取缓存
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError>;

    /// SET 命令 — 写入缓存（带 TTL）
    ///
    /// Memcached 的 SET 必须指定 TTL，若 TTL 为 0 或超过 30 天，
    /// Memcached 会将其视为 Unix 时间戳。
    fn set(&self, key: &str, value: Vec<u8>, ttl: Duration) -> Result<(), CacheError>;

    /// DELETE 命令 — 删除缓存
    ///
    /// 返回是否删除成功（key 存在且删除成功返回 true）
    fn delete(&self, key: &str) -> Result<bool, CacheError>;

    /// INCR 命令 — 自增
    ///
    /// key 不存在时 Memcached 返回错误（与 Redis 不同）。
    /// 本 trait 约定：key 不存在时初始化为 0 再 INCR。
    fn increment(&self, key: &str, step: u64) -> Result<i64, CacheError>;

    /// DECR 命令 — 自减
    ///
    /// Memcached 的 DECR 不会变为负数（最小为 0）。
    fn decrement(&self, key: &str, step: u64) -> Result<i64, CacheError>;

    /// FLUSH_ALL 命令 — 清空所有缓存
    fn flush(&self) -> Result<(), CacheError>;

    /// TOUCH 命令 — 更新 key 的 TTL（Memcached 1.4.8+）
    fn touch(&self, key: &str, ttl: Duration) -> Result<bool, CacheError>;
}

/// Mock Memcached 后端（用 HashMap 模拟 Memcached 行为）
///
/// 用于测试和开发环境，不需要真实 Memcached 服务器。
///
/// ## 模拟行为
///
/// - KV 存储：`HashMap<String, (Vec<u8>, Option<Instant>)>` — value + expires_at
/// - TTL 过期：`get`/`increment`/`decrement`/`touch` 时检查过期
/// - INCR：key 不存在时初始化为 0，非数字时返回错误
/// - DECR：最小为 0（对齐 Memcached 行为）
type MemcachedKvEntry = (Vec<u8>, Option<Instant>);

/// KV 存储映射：key → (value, expires_at)
type MemcachedKvMap = HashMap<String, MemcachedKvEntry>;

/// 内存 Mock Memcached 后端
///
/// 用于测试和开发环境，不需要真实 Memcached 服务器。
pub struct MockMemcachedBackend {
    /// KV 存储：key → (value, expires_at)
    kv: RwLock<MemcachedKvMap>,
}

impl Default for MockMemcachedBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockMemcachedBackend {
    /// 创建空的 Mock Memcached 后端
    pub fn new() -> Self {
        Self {
            kv: RwLock::new(HashMap::new()),
        }
    }

    /// 检查 key 是否已过期
    fn is_expired(expires_at: Option<Instant>) -> bool {
        match expires_at {
            Some(exp) => Instant::now() >= exp,
            None => false,
        }
    }

    /// 清理已过期的 key（惰性清理）
    fn cleanup_expired(kv: &mut HashMap<String, (Vec<u8>, Option<Instant>)>, key: &str) {
        if let Some((_, Some(exp))) = kv.get(key) {
            if Self::is_expired(Some(*exp)) {
                kv.remove(key);
            }
        }
    }
}

impl MemcachedBackend for MockMemcachedBackend {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        let mut kv = self.kv.write();
        Self::cleanup_expired(&mut kv, key);
        Ok(kv.get(key).map(|(v, _)| v.clone()))
    }

    fn set(&self, key: &str, value: Vec<u8>, ttl: Duration) -> Result<(), CacheError> {
        if value.len() > MEMCACHED_MAX_VALUE_SIZE {
            return Err(CacheError::SerializationError(format!(
                "Memcached value size {} exceeds max {} bytes",
                value.len(),
                MEMCACHED_MAX_VALUE_SIZE
            )));
        }

        let expires_at = if ttl == Duration::ZERO {
            None
        } else {
            Some(Instant::now() + ttl)
        };

        let mut kv = self.kv.write();
        kv.insert(key.to_string(), (value, expires_at));
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<bool, CacheError> {
        let mut kv = self.kv.write();
        Ok(kv.remove(key).is_some())
    }

    fn increment(&self, key: &str, step: u64) -> Result<i64, CacheError> {
        let mut kv = self.kv.write();
        Self::cleanup_expired(&mut kv, key);

        let current = match kv.get(key) {
            Some((bytes, exp)) => {
                if Self::is_expired(*exp) {
                    kv.remove(key);
                    0
                } else {
                    let s = std::str::from_utf8(bytes).map_err(|e| {
                        CacheError::DeserializationError(format!(
                            "increment: value is not valid UTF-8: {}",
                            e
                        ))
                    })?;
                    s.parse::<i64>().map_err(|e| {
                        CacheError::DeserializationError(format!(
                            "increment: value '{}' is not numeric: {}",
                            s, e
                        ))
                    })?
                }
            }
            None => 0,
        };

        let new_value = current + step as i64;
        let expires_at = kv
            .get(key)
            .and_then(|(_, exp)| *exp)
            .or_else(|| Some(Instant::now() + Duration::from_secs(MEMCACHED_MAX_TTL_SECS)));

        kv.insert(
            key.to_string(),
            (new_value.to_string().into_bytes(), expires_at),
        );
        Ok(new_value)
    }

    fn decrement(&self, key: &str, step: u64) -> Result<i64, CacheError> {
        let mut kv = self.kv.write();
        Self::cleanup_expired(&mut kv, key);

        let current = match kv.get(key) {
            Some((bytes, exp)) => {
                if Self::is_expired(*exp) {
                    kv.remove(key);
                    0
                } else {
                    let s = std::str::from_utf8(bytes).map_err(|e| {
                        CacheError::DeserializationError(format!(
                            "decrement: value is not valid UTF-8: {}",
                            e
                        ))
                    })?;
                    s.parse::<i64>().map_err(|e| {
                        CacheError::DeserializationError(format!(
                            "decrement: value '{}' is not numeric: {}",
                            s, e
                        ))
                    })?
                }
            }
            None => 0,
        };

        // Memcached DECR 不会变为负数（最小为 0）
        let new_value = (current - step as i64).max(0);
        let expires_at = kv
            .get(key)
            .and_then(|(_, exp)| *exp)
            .or_else(|| Some(Instant::now() + Duration::from_secs(MEMCACHED_MAX_TTL_SECS)));

        kv.insert(
            key.to_string(),
            (new_value.to_string().into_bytes(), expires_at),
        );
        Ok(new_value)
    }

    fn flush(&self) -> Result<(), CacheError> {
        let mut kv = self.kv.write();
        kv.clear();
        Ok(())
    }

    fn touch(&self, key: &str, ttl: Duration) -> Result<bool, CacheError> {
        let mut kv = self.kv.write();
        Self::cleanup_expired(&mut kv, key);

        if let Some((_, ref mut exp)) = kv.get_mut(key) {
            *exp = if ttl == Duration::ZERO {
                None
            } else {
                Some(Instant::now() + ttl)
            };
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Memcached 缓存驱动（对齐 PHP `think\cache\driver\Memcached`）
///
/// 实现 `CacheDriver` trait，通过 `MemcachedBackend` 抽象层操作 Memcached。
///
/// ## 标签模拟
///
/// 由于 Memcached 不支持 Set 数据结构，标签功能通过 JSON 序列化模拟：
///
/// - `tag_append(name, value)`：读取 `tag:<md5(tag)>` 的 JSON 数组 → 追加 → 写回
/// - `tag_items(tag)`：读取 `tag:<md5(tag)>` 的 JSON 数组
/// - `tag_clear(keys)`：逐个删除 key
///
/// ## 用法
///
/// ```ignore
/// use sz_rust_cache_facade::{Cache, MemcachedCacheDriver, MemcachedConfig, MockMemcachedBackend};
///
/// let cache = Cache::new();
/// cache.register_default(
///     MemcachedCacheDriver::with_backend(
///         MemcachedConfig::default(),
///         Box::new(MockMemcachedBackend::new()),
///     )
/// );
///
/// cache.set("key", "value", None).unwrap();
/// assert_eq!(cache.get::<String>("key").unwrap(), Some("value".to_string()));
/// ```
pub struct MemcachedCacheDriver {
    backend: Box<dyn MemcachedBackend>,
    config: MemcachedConfig,
}

impl MemcachedCacheDriver {
    /// 创建 Memcached 缓存驱动（用 Mock backend）
    ///
    /// 对齐 PHP `new \think\cache\driver\Memcached($options)`。
    pub fn new(config: MemcachedConfig) -> Self {
        Self::with_backend(config, Box::new(MockMemcachedBackend::new()))
    }

    /// 创建 Memcached 缓存驱动（自定义 backend）
    ///
    /// 应用层可注入真实 Memcached backend（如 `memcache::Client` 包装）。
    pub fn with_backend(config: MemcachedConfig, backend: Box<dyn MemcachedBackend>) -> Self {
        Self { backend, config }
    }

    /// 获取配置引用
    pub fn config(&self) -> &MemcachedConfig {
        &self.config
    }

    /// 校验 key 合法性（对齐 Memcached 协议限制）
    ///
    /// Memcached key 规则：
    /// - 长度 ≤ 250 字节
    /// - 不包含空格和控制字符
    /// - 不为空
    fn validate_key(key: &str) -> Result<(), CacheError> {
        if key.is_empty() {
            return Err(CacheError::Internal(
                "Memcached key cannot be empty".to_string(),
            ));
        }
        if key.len() > MEMCACHED_MAX_KEY_LEN {
            return Err(CacheError::Internal(format!(
                "Memcached key length {} exceeds max {} bytes",
                key.len(),
                MEMCACHED_MAX_KEY_LEN
            )));
        }
        if key.chars().any(|c| c.is_control() || c == ' ') {
            return Err(CacheError::Internal(format!(
                "Memcached key contains invalid characters (space or control): {}",
                key
            )));
        }
        Ok(())
    }

    /// 将 TTL 转换为 Memcached 兼容的 TTL
    ///
    /// Memcached TTL 规则：
    /// - TTL = 0：永不过期
    /// - TTL ≤ 60*60*24*30（30天）：相对秒数
    /// - TTL > 60*60*24*30：Unix 时间戳
    ///
    /// 本实现始终使用相对秒数（≤ 30 天），超过 30 天的 TTL 截断为 30 天。
    fn normalize_ttl(ttl: Duration) -> Duration {
        let max_ttl = Duration::from_secs(MEMCACHED_MAX_TTL_SECS);
        if ttl > max_ttl {
            max_ttl
        } else {
            ttl
        }
    }

    /// 追加 TagSet 数据（模拟 PHP `Driver::append`）
    ///
    /// Memcached 不支持 Set，通过 JSON 序列化模拟：
    /// 1. 读取 `tag:<md5(tag)>` 的 JSON 数组（若不存在则为空数组）
    /// 2. 追加新 value（去重）
    /// 3. 写回
    pub fn append(&self, name: &str, value: &str) -> Result<(), CacheError> {
        let tag_key = self.get_tag_key(name);
        let cache_key = self.get_cache_key(&tag_key);

        let mut items: HashSet<String> = self.read_tag_set(&cache_key)?;
        items.insert(value.to_string());

        self.write_tag_set(&cache_key, &items)
    }

    /// 获取标签包含的缓存标识（模拟 PHP `getTagItems`）
    pub fn get_tag_items(&self, tag: &str) -> Result<Vec<String>, CacheError> {
        let name = self.get_tag_key(tag);
        let cache_key = self.get_cache_key(&name);
        let items = self.read_tag_set(&cache_key)?;
        Ok(items.into_iter().collect())
    }

    /// 删除缓存标签（模拟 PHP `clearTag`）
    ///
    /// Memcached 不支持批量 DEL，逐个删除。
    pub fn clear_tag(&self, keys: &[&str]) -> Result<(), CacheError> {
        for key in keys {
            self.backend.delete(key)?;
        }
        Ok(())
    }

    /// 读取标签集合（JSON 反序列化）
    fn read_tag_set(&self, cache_key: &str) -> Result<HashSet<String>, CacheError> {
        match self.backend.get(cache_key)? {
            Some(bytes) => {
                if bytes.is_empty() {
                    return Ok(HashSet::new());
                }
                let json_str = std::str::from_utf8(&bytes).map_err(|e| {
                    CacheError::DeserializationError(format!("tag set is not valid UTF-8: {}", e))
                })?;
                let items: Vec<String> = serde_json::from_str(json_str).map_err(|e| {
                    CacheError::DeserializationError(format!(
                        "tag set JSON deserialization failed: {}",
                        e
                    ))
                })?;
                Ok(items.into_iter().collect())
            }
            None => Ok(HashSet::new()),
        }
    }

    /// 写入标签集合（JSON 序列化）
    fn write_tag_set(&self, cache_key: &str, items: &HashSet<String>) -> Result<(), CacheError> {
        let mut vec: Vec<String> = items.iter().cloned().collect();
        vec.sort(); // 排序确保幂等
        let json = serde_json::to_string(&vec).map_err(|e| {
            CacheError::SerializationError(format!("tag set JSON serialization failed: {}", e))
        })?;
        let ttl = Duration::from_secs(MEMCACHED_MAX_TTL_SECS);
        self.backend.set(cache_key, json.into_bytes(), ttl)?;
        Ok(())
    }
}

impl CacheDriver for MemcachedCacheDriver {
    fn get_raw(&self, key: &str) -> Result<Option<Vec<u8>>, CacheError> {
        Self::validate_key(key)?;
        let cache_key = self.get_cache_key(key);
        self.backend.get(&cache_key)
    }

    fn set_raw(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<(), CacheError> {
        Self::validate_key(key)?;
        let cache_key = self.get_cache_key(key);

        // 对齐 PHP: $expire = is_null($ttl) ? $this->options['expire'] : $ttl
        let effective_ttl = ttl.or(self.config.expire);
        // 对齐 Memcached: 必须有 TTL，None → 最大 TTL（30天）
        let normalized_ttl = effective_ttl
            .map(Self::normalize_ttl)
            .unwrap_or_else(|| Duration::from_secs(MEMCACHED_MAX_TTL_SECS));

        self.backend.set(&cache_key, value, normalized_ttl)
    }

    fn delete(&self, key: &str) -> Result<(), CacheError> {
        Self::validate_key(key)?;
        let cache_key = self.get_cache_key(key);
        self.backend.delete(&cache_key)?;
        Ok(())
    }

    fn has(&self, key: &str) -> Result<bool, CacheError> {
        Self::validate_key(key)?;
        let cache_key = self.get_cache_key(key);
        Ok(self.backend.get(&cache_key)?.is_some())
    }

    fn inc(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        Self::validate_key(key)?;
        let cache_key = self.get_cache_key(key);

        if step >= 0 {
            self.backend.increment(&cache_key, step as u64)
        } else {
            // 负数 step 使用 decrement
            self.backend.decrement(&cache_key, (-step) as u64)
        }
    }

    fn dec(&self, key: &str, step: i64) -> Result<i64, CacheError> {
        self.inc(key, -step)
    }

    fn clear(&self) -> Result<(), CacheError> {
        self.backend.flush()
    }

    fn get_cache_key(&self, name: &str) -> String {
        if self.config.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}{}", self.config.prefix, name)
        }
    }

    fn get_tag_key(&self, tag: &str) -> String {
        format!("{}{}", self.config.tag_prefix, compute_md5(tag))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // 测试组 1: MemcachedConfig
    // ========================================================================

    #[test]
    fn test_memcached_config_default() {
        let config = MemcachedConfig::default();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 11211);
        assert!(config.expire.is_none());
        assert!(config.prefix.is_empty());
        assert_eq!(config.tag_prefix, "tag:");
    }

    #[test]
    fn test_memcached_config_with_prefix() {
        let config = MemcachedConfig::with_prefix("myapp:");
        assert_eq!(config.prefix, "myapp:");
        assert_eq!(config.port, 11211);
    }

    #[test]
    fn test_memcached_config_with_expire() {
        let config = MemcachedConfig::with_expire(Duration::from_secs(3600));
        assert_eq!(config.expire, Some(Duration::from_secs(3600)));
    }

    // ========================================================================
    // 测试组 2: MockMemcachedBackend 基本 KV 操作
    // ========================================================================

    #[test]
    fn test_mock_backend_set_get() {
        let backend = MockMemcachedBackend::new();
        backend
            .set("key1", b"value1".to_vec(), Duration::from_secs(60))
            .unwrap();
        let result = backend.get("key1").unwrap();
        assert_eq!(result, Some(b"value1".to_vec()));
    }

    #[test]
    fn test_mock_backend_get_nonexistent() {
        let backend = MockMemcachedBackend::new();
        let result = backend.get("nonexistent").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_mock_backend_delete() {
        let backend = MockMemcachedBackend::new();
        backend
            .set("key1", b"value1".to_vec(), Duration::from_secs(60))
            .unwrap();
        assert!(backend.delete("key1").unwrap());
        assert!(backend.get("key1").unwrap().is_none());
    }

    #[test]
    fn test_mock_backend_delete_nonexistent() {
        let backend = MockMemcachedBackend::new();
        assert!(!backend.delete("nonexistent").unwrap());
    }

    #[test]
    fn test_mock_backend_flush() {
        let backend = MockMemcachedBackend::new();
        backend
            .set("key1", b"value1".to_vec(), Duration::from_secs(60))
            .unwrap();
        backend
            .set("key2", b"value2".to_vec(), Duration::from_secs(60))
            .unwrap();
        backend.flush().unwrap();
        assert!(backend.get("key1").unwrap().is_none());
        assert!(backend.get("key2").unwrap().is_none());
    }

    // ========================================================================
    // 测试组 3: MockMemcachedBackend TTL 过期
    // ========================================================================

    #[test]
    fn test_mock_backend_ttl_expiration() {
        let backend = MockMemcachedBackend::new();
        backend
            .set("key1", b"value1".to_vec(), Duration::from_millis(10))
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(backend.get("key1").unwrap().is_none());
    }

    #[test]
    fn test_mock_backend_no_ttl_never_expires() {
        let backend = MockMemcachedBackend::new();
        backend
            .set("key1", b"value1".to_vec(), Duration::ZERO)
            .unwrap();
        assert!(backend.get("key1").unwrap().is_some());
    }

    // ========================================================================
    // 测试组 4: MockMemcachedBackend increment/decrement
    // ========================================================================

    #[test]
    fn test_mock_backend_increment_new_key() {
        let backend = MockMemcachedBackend::new();
        let result = backend.increment("counter", 5).unwrap();
        assert_eq!(result, 5);
    }

    #[test]
    fn test_mock_backend_increment_existing_key() {
        let backend = MockMemcachedBackend::new();
        backend.increment("counter", 5).unwrap();
        let result = backend.increment("counter", 3).unwrap();
        assert_eq!(result, 8);
    }

    #[test]
    fn test_mock_backend_decrement() {
        let backend = MockMemcachedBackend::new();
        backend.increment("counter", 10).unwrap();
        let result = backend.decrement("counter", 3).unwrap();
        assert_eq!(result, 7);
    }

    #[test]
    fn test_mock_backend_decrement_not_below_zero() {
        let backend = MockMemcachedBackend::new();
        backend.increment("counter", 5).unwrap();
        let result = backend.decrement("counter", 10).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_mock_backend_increment_non_numeric_value() {
        let backend = MockMemcachedBackend::new();
        backend
            .set("text", b"hello".to_vec(), Duration::from_secs(60))
            .unwrap();
        let result = backend.increment("text", 1);
        assert!(result.is_err());
    }

    // ========================================================================
    // 测试组 5: MockMemcachedBackend touch
    // ========================================================================

    #[test]
    fn test_mock_backend_touch_existing_key() {
        let backend = MockMemcachedBackend::new();
        backend
            .set("key1", b"value1".to_vec(), Duration::from_secs(60))
            .unwrap();
        assert!(backend.touch("key1", Duration::from_millis(10)).unwrap());
        std::thread::sleep(Duration::from_millis(20));
        assert!(backend.get("key1").unwrap().is_none());
    }

    #[test]
    fn test_mock_backend_touch_nonexistent_key() {
        let backend = MockMemcachedBackend::new();
        assert!(!backend
            .touch("nonexistent", Duration::from_secs(60))
            .unwrap());
    }

    // ========================================================================
    // 测试组 6: MockMemcachedBackend value 大小限制
    // ========================================================================

    #[test]
    fn test_mock_backend_set_oversized_value() {
        let backend = MockMemcachedBackend::new();
        let large_value = vec![0u8; MEMCACHED_MAX_VALUE_SIZE + 1];
        let result = backend.set("key1", large_value, Duration::from_secs(60));
        assert!(result.is_err());
    }

    // ========================================================================
    // 测试组 7: MemcachedCacheDriver 基本 KV 操作
    // ========================================================================

    #[test]
    fn test_driver_set_get_raw() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        driver
            .set_raw("key1", b"value1".to_vec(), Some(Duration::from_secs(60)))
            .unwrap();
        let result = driver.get_raw("key1").unwrap();
        assert_eq!(result, Some(b"value1".to_vec()));
    }

    #[test]
    fn test_driver_get_nonexistent() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        let result = driver.get_raw("nonexistent").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_driver_delete() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        driver
            .set_raw("key1", b"value1".to_vec(), Some(Duration::from_secs(60)))
            .unwrap();
        driver.delete("key1").unwrap();
        assert!(driver.get_raw("key1").unwrap().is_none());
    }

    #[test]
    fn test_driver_has() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        assert!(!driver.has("key1").unwrap());
        driver
            .set_raw("key1", b"value1".to_vec(), Some(Duration::from_secs(60)))
            .unwrap();
        assert!(driver.has("key1").unwrap());
    }

    #[test]
    fn test_driver_clear() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        driver
            .set_raw("key1", b"value1".to_vec(), Some(Duration::from_secs(60)))
            .unwrap();
        driver
            .set_raw("key2", b"value2".to_vec(), Some(Duration::from_secs(60)))
            .unwrap();
        driver.clear().unwrap();
        assert!(driver.get_raw("key1").unwrap().is_none());
        assert!(driver.get_raw("key2").unwrap().is_none());
    }

    // ========================================================================
    // 测试组 8: MemcachedCacheDriver inc/dec
    // ========================================================================

    #[test]
    fn test_driver_inc_new_key() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        let result = driver.inc("counter", 5).unwrap();
        assert_eq!(result, 5);
    }

    #[test]
    fn test_driver_inc_existing_key() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        driver.inc("counter", 5).unwrap();
        let result = driver.inc("counter", 3).unwrap();
        assert_eq!(result, 8);
    }

    #[test]
    fn test_driver_dec() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        driver.inc("counter", 10).unwrap();
        let result = driver.dec("counter", 3).unwrap();
        assert_eq!(result, 7);
    }

    #[test]
    fn test_driver_dec_not_below_zero() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        driver.inc("counter", 5).unwrap();
        let result = driver.dec("counter", 10).unwrap();
        assert_eq!(result, 0);
    }

    // ========================================================================
    // 测试组 9: MemcachedCacheDriver key 前缀
    // ========================================================================

    #[test]
    fn test_driver_get_cache_key_no_prefix() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        assert_eq!(driver.get_cache_key("mykey"), "mykey");
    }

    #[test]
    fn test_driver_get_cache_key_with_prefix() {
        let config = MemcachedConfig::with_prefix("myapp:");
        let driver = MemcachedCacheDriver::new(config);
        assert_eq!(driver.get_cache_key("mykey"), "myapp:mykey");
    }

    #[test]
    fn test_driver_set_get_with_prefix() {
        let config = MemcachedConfig::with_prefix("myapp:");
        let driver = MemcachedCacheDriver::new(config);
        driver
            .set_raw("key1", b"value1".to_vec(), Some(Duration::from_secs(60)))
            .unwrap();
        let result = driver.get_raw("key1").unwrap();
        assert_eq!(result, Some(b"value1".to_vec()));
    }

    // ========================================================================
    // 测试组 10: MemcachedCacheDriver key 校验
    // ========================================================================

    #[test]
    fn test_driver_validate_key_empty() {
        assert!(MemcachedCacheDriver::validate_key("").is_err());
    }

    #[test]
    fn test_driver_validate_key_with_space() {
        assert!(MemcachedCacheDriver::validate_key("key with space").is_err());
    }

    #[test]
    fn test_driver_validate_key_too_long() {
        let long_key = "a".repeat(MEMCACHED_MAX_KEY_LEN + 1);
        assert!(MemcachedCacheDriver::validate_key(&long_key).is_err());
    }

    #[test]
    fn test_driver_validate_key_valid() {
        assert!(MemcachedCacheDriver::validate_key("valid_key_123").is_ok());
        assert!(MemcachedCacheDriver::validate_key("user:123:session").is_ok());
    }

    // ========================================================================
    // 测试组 11: MemcachedCacheDriver TTL 规范化
    // ========================================================================

    #[test]
    fn test_driver_normalize_ttl_within_limit() {
        let ttl = Duration::from_secs(3600);
        assert_eq!(MemcachedCacheDriver::normalize_ttl(ttl), ttl);
    }

    #[test]
    fn test_driver_normalize_ttl_exceeds_limit() {
        let ttl = Duration::from_secs(MEMCACHED_MAX_TTL_SECS + 100);
        let normalized = MemcachedCacheDriver::normalize_ttl(ttl);
        assert_eq!(normalized, Duration::from_secs(MEMCACHED_MAX_TTL_SECS));
    }

    #[test]
    fn test_driver_set_with_default_expire() {
        let config = MemcachedConfig::with_expire(Duration::from_secs(60));
        let driver = MemcachedCacheDriver::new(config);
        driver.set_raw("key1", b"value1".to_vec(), None).unwrap();
        assert!(driver.get_raw("key1").unwrap().is_some());
    }

    // ========================================================================
    // 测试组 12: MemcachedCacheDriver 标签模拟
    // ========================================================================

    #[test]
    fn test_driver_tag_append_and_items() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());

        driver.append("mytag", "key1").unwrap();
        driver.append("mytag", "key2").unwrap();
        driver.append("mytag", "key3").unwrap();

        let items = driver.get_tag_items("mytag").unwrap();
        assert_eq!(items.len(), 3);
        assert!(items.contains(&"key1".to_string()));
        assert!(items.contains(&"key2".to_string()));
        assert!(items.contains(&"key3".to_string()));
    }

    #[test]
    fn test_driver_tag_append_deduplication() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());

        driver.append("mytag", "key1").unwrap();
        driver.append("mytag", "key1").unwrap();
        driver.append("mytag", "key1").unwrap();

        let items = driver.get_tag_items("mytag").unwrap();
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn test_driver_tag_items_empty() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        let items = driver.get_tag_items("nonexistent").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_driver_tag_clear() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());

        driver
            .set_raw("key1", b"value1".to_vec(), Some(Duration::from_secs(60)))
            .unwrap();
        driver
            .set_raw("key2", b"value2".to_vec(), Some(Duration::from_secs(60)))
            .unwrap();

        driver.append("mytag", "key1").unwrap();
        driver.append("mytag", "key2").unwrap();

        let items = driver.get_tag_items("mytag").unwrap();
        let item_refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
        driver.clear_tag(&item_refs).unwrap();

        assert!(driver.get_raw("key1").unwrap().is_none());
        assert!(driver.get_raw("key2").unwrap().is_none());
    }

    #[test]
    fn test_driver_tag_clear_empty() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        driver.clear_tag(&[]).unwrap();
    }

    // ========================================================================
    // 测试组 13: MemcachedCacheDriver tag key 生成
    // ========================================================================

    #[test]
    fn test_driver_get_tag_key_uses_md5() {
        let driver = MemcachedCacheDriver::new(MemcachedConfig::default());
        let tag_key = driver.get_tag_key("mytag");
        assert!(tag_key.starts_with("tag:"));
        let md5_part = &tag_key["tag:".len()..];
        assert_eq!(md5_part.len(), 32);
    }

    #[test]
    fn test_driver_get_tag_key_with_custom_prefix() {
        let config = MemcachedConfig {
            tag_prefix: "tagset:".to_string(),
            ..MemcachedConfig::default()
        };
        let driver = MemcachedCacheDriver::new(config);
        let tag_key = driver.get_tag_key("mytag");
        assert!(tag_key.starts_with("tagset:"));
    }
}
