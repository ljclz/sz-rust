# sz-rust-cache-facade

SZ-Rust 缓存抽象层。提供统一的 `CacheDriver` trait 和多驱动实现。

## 功能

- **CacheDriver trait**：统一的缓存操作接口（get/set/delete/exists）
- **MemoryCache**：进程内缓存（`parking_lot` 锁）
- **RedisCache**：Redis 后端（`redis` crate + connection-manager）
- **MemcachedCache**：Memcached 后端
- **MultiLevelCache**：多级缓存（L1 Memory + L2 Redis）
- **缓存预热**：与 `cache_warmer` 模块集成

## 用法

```rust
use sz_rust_cache_facade::{CacheDriver, MemoryCache, MultiLevelCache};

let cache = MemoryCache::new();
cache.set("key", "value", 3600).await?;
```

## 依赖

- `sz-rust-orm-facade`（`Cache` / `CacheError` 类型）
- `parking_lot`
- `redis`（可选，Redis 驱动）
- `serde` / `serde_json`

## 版本策略

与 `sz-rust-core` 保持同步。
