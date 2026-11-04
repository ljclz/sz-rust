# sz-rust-cache-facade

> **中文** | [English](README.en.md)

SZ-Rust cache abstraction layer. Provides unified `CacheDriver` trait and multi-driver implementations.

## Features

- **CacheDriver trait**: Unified cache operation interface (get/set/delete/exists)
- **MemoryCache**: In-process cache (`parking_lot` locks)
- **RedisCache**: Redis backend (`redis` crate + connection-manager)
- **MemcachedCache**: Memcached backend
- **MultiLevelCache**: Multi-level cache (L1 Memory + L2 Redis)
- **Cache warming**: Integrated with `cache_warmer` module

## Usage

```rust
use sz_rust_cache_facade::{CacheDriver, MemoryCache, MultiLevelCache};

let cache = MemoryCache::new();
cache.set("key", "value", 3600).await?;
```

## Dependencies

- `sz-rust-orm-facade` (`Cache` / `CacheError` types)
- `parking_lot`
- `redis` (optional, Redis driver)
- `serde` / `serde_json`

## Version Policy

Keeps in sync with `sz-rust-core`.