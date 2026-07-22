# ADR-009：缓存策略（Cache facade + 全局实例 + 多驱动 + PHP 源码 bug 复刻）

> **状态**：已接受
> **日期**：2026-07-22
> **决策者**：SZ-Rust Team
> **关联 ADR**：ADR-010（配置加载）
> **相关代码**：`packages/sz-rust-core/src/cache.rs`

## 背景

PHP `think\facade\Cache` 是 facade，所有静态方法通过 `__callStatic` 转发到 `think\Cache`（Manager）实例。PHP 端缓存特性：

1. **静态 API 风格**：`Cache::set('key', $value, 3600)` / `Cache::get('key')`
2. **驱动管理器**：`think\Cache extends Manager`，通过 `$namespace = '\\think\\cache\\driver\\'` 创建驱动
3. **序列化策略**：`is_numeric` 短路（数字直接转字符串，非数字用 `serialize`）
4. **`remember` 锁机制**：200ms 轮询 + 5 秒超时，防止缓存击穿

PHP 源码 bug：
```php
public function unserialize($data) {
    if (is_numeric($data)) {
        return $data;  // ⚠️ 返回 string，而非 int（PHP 源码 bug）
    }
    return unserialize($data);
}
```

sz-rust 需要决定如何对齐 PHP 缓存行为，包括复刻 PHP 源码 bug。

## 决策

采用 **Cache facade + 全局实例 + 多驱动 + PHP 源码 bug 复刻** 策略：

### 1. 全局实例（伪静态 API）

```rust
// 通过全局 OnceLock<Cache> + Cache::default_instance() 提供"伪静态"API
// 调用方也可以创建独立 Cache 实例用于测试隔离
```

对齐 PHP `think\facade\Cache::__callStatic`：PHP facade 通过 `__callStatic` 转发到 Manager 实例，Rust 通过 `OnceLock<Cache>` 提供等价能力。

### 2. 驱动管理器（CacheManager + CacheDriver trait）

```rust
// CacheManager 对齐 PHP think\Cache extends Manager
// - register_store(name, driver)：注册命名驱动
// - store(name)：获取命名驱动
// - default_store()：获取默认驱动
```

对齐 PHP `$namespace = '\\think\\cache\\driver\\'` + `createDriver(array $config)` 的驱动创建机制。

### 3. 序列化策略（is_numeric 短路）

```rust
// PHP: is_numeric($data) → (string) $data
// Rust: CacheValue::Number 标记 + get::<String>() 返回 string
```

对齐 PHP `think\cache\Driver::serialize($data)` 的 `is_numeric` 短路逻辑。

### 4. PHP 源码 bug 复刻

**PHP 源码 bug**：`unserialize` 对 `is_numeric` 的值返回 string，而非还原为 int。

Rust 端通过 `CacheValue::Number` 标记 + `get::<String>()` 返回 string 来**复刻此行为**。这是有意的 bug 复刻，必须用注释说明。

### 5. `remember` 锁机制

对齐 PHP `think\cache\Driver::remember`：
- 200ms 轮询检查锁
- 5 秒超时
- 超时后直接调用 callback（防止永久阻塞）
- 锁释放后再次读取缓存

## 后果

### 正面后果

- **PHP 完全对齐**：静态 API、驱动管理器、序列化策略、remember 锁机制全部对齐
- **PHP 源码 bug 复刻**：`unserialize` 的 bug 行为被有意复刻，确保 PHP 迁移零差异
- **测试隔离**：`Cache::default_instance()` 用于生产，独立 `Cache` 实例用于测试
- **多驱动支持**：Memory（测试）/ Redis（生产）等驱动可插拔

### 负面后果

- **PHP 源码 bug 成为技术债务**：复刻的 bug 可能在未来被"修复"，导致行为不一致
- **全局状态**：`OnceLock<Cache>` 是全局状态，测试需要显式重置
- **`is_numeric` 判断差异**：PHP 的 `is_numeric` 与 Rust 的数字判断逻辑不完全一致（如 `"1e5"` 在 PHP 中是 numeric，在 Rust 中需要特殊处理）
- **序列化格式不兼容**：PHP 用 `serialize()`，Rust 用 `serde_json`，跨语言缓存不兼容

## 注意事项

- **PHP 源码 bug 必须有注释**：所有复刻 PHP bug 的代码必须有 `// PHP 源码 bug 复刻` 注释，防止后续开发者"修复"
- **`CacheValue::Number` 标记**：Rust 端用枚举标记数字类型，`get::<String>()` 返回 string（复刻 bug）
- **`remember` 锁的超时**：5 秒超时是 PHP 硬编码，Rust 端保持一致，不可配置
- **全局实例初始化**：`Cache::default_instance()` 首次调用时初始化，需要确保线程安全
- **Redis 驱动**：生产环境使用 Redis 驱动，需要配置连接池

## Bug 定位提示

如果生产 Bug 表现为"缓存值类型错误"或"remember 锁死锁"：

1. **L1 决策层**：查阅本 ADR，确认是否使用 `Cache::default_instance()`，驱动是否正确注册
2. **L2 运行时层**：检查 tracing span `cache.get` / `cache.set` 中的 `key` 和 `hit` 字段
3. **L3 指标层**：检查 `cache.hit.rate` 和 `cache.duration` 指标
4. **L4 代码层**：
   - 值类型错误 Bug → 检查 `CacheValue::Number` 标记是否正确，`get::<String>()` 是否复刻了 PHP bug
   - remember 锁死锁 Bug → 检查锁的 200ms 轮询和 5 秒超时是否正确实现
   - 全局状态 Bug → 检查测试是否重置了 `OnceLock<Cache>`
   - 驱动未找到 Bug → 检查 `CacheManager::register_store()` 是否在启动时注册了所有驱动
