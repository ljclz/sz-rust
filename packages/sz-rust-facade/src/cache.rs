// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Cache 静态门面
//!
//! 对齐 PHP `think\facade\Cache`，委托全局 `sz_rust_cache_facade::Cache` 单例。

use std::sync::OnceLock;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;

use sz_rust_cache_facade::Cache as CacheInner;

use crate::FacadeError;

static CACHE_INSTANCE: OnceLock<CacheInner> = OnceLock::new();

/// Cache 静态门面（对齐 PHP `think\facade\Cache`）
///
/// 委托全局 `OnceCell<CacheInner>` 单例，首次调用自动初始化（MemoryCacheDriver 默认驱动）。
pub struct Cache;

impl Cache {
    /// 获取或初始化全局缓存实例
    fn instance() -> &'static CacheInner {
        CACHE_INSTANCE.get_or_init(|| {
            let cache = CacheInner::new();
            cache.register_default(sz_rust_cache_facade::MemoryCacheDriver::new());
            cache
        })
    }

    /// 手动设置全局缓存实例（高级用法，通常不需要）
    pub fn init(cache: CacheInner) {
        let _ = CACHE_INSTANCE.set(cache);
    }

    /// 写入缓存（对齐 PHP `Cache::set($name, $value, $ttl = null)`）
    pub fn set<T: Serialize>(
        key: &str,
        value: T,
        ttl: Option<Duration>,
    ) -> Result<(), FacadeError> {
        Self::instance()
            .set(key, value, ttl)
            .map_err(|e| FacadeError::Cache(e.to_string()))
    }

    /// 读取缓存（对齐 PHP `Cache::get($name, $default = null)`）
    pub fn get<T: DeserializeOwned>(key: &str) -> Result<Option<T>, FacadeError> {
        Self::instance()
            .get(key)
            .map_err(|e| FacadeError::Cache(e.to_string()))
    }

    /// 删除缓存（对齐 PHP `Cache::delete($name)`）
    pub fn delete(key: &str) -> Result<(), FacadeError> {
        Self::instance()
            .delete(key)
            .map_err(|e| FacadeError::Cache(e.to_string()))
    }

    /// 判断缓存是否存在（对齐 PHP `Cache::has($name)`）
    pub fn has(key: &str) -> Result<bool, FacadeError> {
        Self::instance()
            .has(key)
            .map_err(|e| FacadeError::Cache(e.to_string()))
    }

    /// 自增（对齐 PHP `Cache::inc($name, $step = 1)`）
    pub fn inc(key: &str, step: i64) -> Result<i64, FacadeError> {
        Self::instance()
            .inc(key, step)
            .map_err(|e| FacadeError::Cache(e.to_string()))
    }

    /// 自减（对齐 PHP `Cache::dec($name, $step = 1)`）
    pub fn dec(key: &str, step: i64) -> Result<i64, FacadeError> {
        Self::instance()
            .dec(key, step)
            .map_err(|e| FacadeError::Cache(e.to_string()))
    }

    /// 清空缓存（对齐 PHP `Cache::clear()`）
    pub fn clear() -> Result<(), FacadeError> {
        Self::instance()
            .clear()
            .map_err(|e| FacadeError::Cache(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serde_json::Value;

    #[test]
    fn test_cache_set_and_get() {
        Cache::set("facade_test_key", json!("hello"), None).unwrap();
        let val: Option<Value> = Cache::get("facade_test_key").unwrap();
        assert_eq!(val, Some(json!("hello")));
    }

    #[test]
    fn test_cache_has_and_delete() {
        Cache::set("facade_has_test", json!(42), None).unwrap();
        assert!(Cache::has("facade_has_test").unwrap());
        Cache::delete("facade_has_test").unwrap();
        assert!(!Cache::has("facade_has_test").unwrap());
    }

    #[test]
    fn test_cache_inc_and_dec() {
        Cache::set("facade_counter", json!(0i64), None).unwrap();
        let after_inc = Cache::inc("facade_counter", 5).unwrap();
        assert_eq!(after_inc, 5);
        let after_dec = Cache::dec("facade_counter", 3).unwrap();
        assert_eq!(after_dec, 2);
    }

    #[test]
    fn test_cache_singleton() {
        let a = Cache::instance as fn() -> &'static CacheInner;
        let b = Cache::instance as fn() -> &'static CacheInner;
        assert_eq!(a as usize, b as usize);
    }
}
