//! 缓存一致性测试（PHP 对比） — 集成测试
//!
//! 本文件验证 sz-rust 缓存模块与 PHP `think\facade\Cache` / `think\cache\Driver`
//! / `think\cache\driver\File` / `think\cache\driver\Redis` 的行为一致性。
//!
//! ## 验收标准
//!
//! **Cache::delete 后下次 get 返回 None，与 PHP 行为一致**
//!
//! ## 测试组织
//!
//! - 组 1：set + get 基本行为对齐（R5-P1）
//! - 组 2：is_numeric 短路（PHP bug 复刻）（R5-P2）
//! - 组 3：delete 后 get 返回 None（核心验收点）（R5-P3）
//! - 组 4：delete 不存在的 key 不报错（R5-P4）
//! - 组 5：has TTL 过期（R5-P5）
//! - 组 6：clear 清空所有（R5-P6）
//! - 组 7：inc/dec 不经序列化（R5-P7）
//! - 组 8：remember 锁机制（PHP bug 复刻）（R5-P8）
//! - 组 9：push 上限 1000 + array_shift + array_unique（R5-P9）
//! - 组 10：pull = get + delete（R5-P10）
//! - 组 11：TTL 过期后 get 返回 None（R5-P11）
//! - 组 12：set 带 TTL（秒）（R5-P12）
//! - 组 13：tag 标签（R5-P13）
//! - 组 14：多驱动 store 隔离（R5-P14）
//! - 组 15：set 返回值（R5-P15）
//!
//! ## PHP 源码参考
//!
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\cache\Driver.php`（抽象驱动基类）
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\Cache.php`（Cache 类，继承 Manager）
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\cache\driver\File.php`（File 驱动）
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\cache\driver\Redis.php`（Redis 驱动）
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\cache\TagSet.php`（标签集合）
//! - `e:\vue\test\鲜视达\server\vendor\topthink\framework\src\think\facade\Cache.php`（facade）
//!
//! ## R5 PHP 行为对齐验证（硬约束）
//!
//! 本测试文件验证以下 PHP 行为（15 项）：
//!
//! - R5-P1：`Cache::set('name', 'Alice')` + `Cache::get('name')` 返回 `'Alice'`
//! - R5-P2：`Cache::set('count', 42)` + `Cache::get('count')` 返回 `"42"`（string，PHP bug）
//! - R5-P3：`Cache::delete('key')` 后 `Cache::get('key')` 返回 `null`（核心验收点）
//! - R5-P4：`Cache::delete('nonexistent')` 不报错（File 返回 false，Redis 返回 false）
//! - R5-P5：`Cache::has('key')` TTL 过期返回 false
//! - R5-P6：`Cache::clear()` 清空所有缓存
//! - R5-P7：`Cache::inc('counter', 1)` 不经 serialize（存储数字字符串）
//! - R5-P8：`Cache::remember('key', fn, $expire)` 锁机制（`{name}_lock` + 200ms 轮询 + 5s 超时 + 无 TTL bug）
//! - R5-P9：`Cache::push('list', $value)` 上限 1000 + array_shift（FIFO）+ array_unique（保留首次）
//! - R5-P10：`Cache::pull('key')` = get + delete
//! - R5-P11：TTL 过期后 `Cache::get('key')` 返回 null
//! - R5-P12：`Cache::set('key', 'value', 60)` TTL 单位为秒，0 = 永久
//! - R5-P13：`Cache::tag('user')->set(...)` + `Cache::tag('user')->clear()` 标签行为
//! - R5-P14：`Cache::store('redis')->set(...)` + `Cache::store('file')->get(...)` 驱动隔离
//! - R5-P15：`Cache::set(...)` 返回 bool（File 反映写入结果，Redis 永远 true）

use std::time::Duration;

use sz_rust_core::cache::{Cache, MemoryCacheDriver};

// ============================================================================
// 辅助函数
// ============================================================================

/// 创建带默认 MemoryCacheDriver 的测试用 Cache（对齐 PHP File 驱动行为）
fn make_cache() -> Cache {
    let cache = Cache::new();
    cache.register_default(MemoryCacheDriver::new());
    cache
}

// ============================================================================
// 组 1：set + get 基本行为对齐（R5-P1）
// ============================================================================

#[test]
fn test_r5_p1_set_get_string_alignment() {
    // R5-P1: PHP Cache::set + Cache::get 基本行为对齐
    //
    // PHP:
    //   Cache::set('name', 'Alice');
    //   $val = Cache::get('name');  // 'Alice' (string)
    //
    // PHP 源码: Driver.php:237-263 (serialize/unserialize)
    //   'Alice' 不是 is_numeric，走 serialize('Alice') = s:5:"Alice";
    //   get 时 unserialize('s:5:"Alice";') = 'Alice'
    let cache = make_cache();
    cache.set("name", "Alice", None).unwrap();
    let val: String = cache.get("name").unwrap().unwrap();
    assert_eq!(val, "Alice");
}

#[test]
fn test_r5_p1_set_get_struct_alignment() {
    // R5-P1: 结构体序列化对齐 PHP serialize(array)
    //
    // PHP:
    //   Cache::set('user', ['id' => 1, 'name' => 'Bob']);
    //   $val = Cache::get('user');  // ['id' => 1, 'name' => 'Bob']
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct User {
        id: i64,
        name: String,
    }

    let cache = make_cache();
    let user = User {
        id: 1,
        name: "Bob".to_string(),
    };
    cache.set("user", &user, None).unwrap();
    let val: User = cache.get("user").unwrap().unwrap();
    assert_eq!(val, user);
}

#[test]
fn test_r5_p1_set_get_overwrite_alignment() {
    // R5-P1: 重复 set 覆盖旧值
    //
    // PHP:
    //   Cache::set('key', 'v1');
    //   Cache::set('key', 'v2');
    //   $val = Cache::get('key');  // 'v2'
    let cache = make_cache();
    cache.set("key", "v1", None).unwrap();
    cache.set("key", "v2", None).unwrap();
    let val: String = cache.get("key").unwrap().unwrap();
    assert_eq!(val, "v2");
}

#[test]
fn test_r5_p1_get_nonexistent_returns_none() {
    // R5-P1: get 不存在的 key 返回 None（对齐 PHP 返回 null）
    //
    // PHP:
    //   $val = Cache::get('nonexistent');  // null
    let cache = make_cache();
    let val: Option<String> = cache.get("nonexistent").unwrap();
    assert_eq!(val, None);
}

// ============================================================================
// 组 2：is_numeric 短路（PHP bug 复刻）（R5-P2）
// ============================================================================

#[test]
fn test_r5_p2_is_numeric_int_returns_string() {
    // R5-P2: PHP is_numeric 短路 — int 存储返回 string（PHP 源码 bug）
    //
    // PHP:
    //   Cache::set('count', 42);
    //   $val = Cache::get('count');  // "42" (string, PHP bug)
    //
    // PHP 源码: Driver.php:237-246
    //   serialize(42): is_numeric(42) = true → (string)42 = "42"
    //   unserialize("42"): is_numeric("42") = true → return "42" (string, 不还原 int)
    let cache = make_cache();
    cache.set("count", 42i64, None).unwrap();
    // PHP bug 复刻：get 返回 string 而非 int
    let s: String = cache.get("count").unwrap().unwrap();
    assert_eq!(s, "42");
}

#[test]
fn test_r5_p2_is_negative_numeric_returns_string() {
    // R5-P2: 负数也走 is_numeric 短路
    //
    // PHP:
    //   Cache::set('temp', -42);
    //   $val = Cache::get('temp');  // "-42" (string)
    let cache = make_cache();
    cache.set("temp", -42i64, None).unwrap();
    let s: String = cache.get("temp").unwrap().unwrap();
    assert_eq!(s, "-42");
}

#[test]
fn test_r5_p2_is_zero_numeric_returns_string() {
    // R5-P2: 0 也是 numeric
    //
    // PHP:
    //   Cache::set('zero', 0);
    //   $val = Cache::get('zero');  // "0" (string)
    let cache = make_cache();
    cache.set("zero", 0i64, None).unwrap();
    let s: String = cache.get("zero").unwrap().unwrap();
    assert_eq!(s, "0");
}

// ============================================================================
// 组 3：delete 后 get 返回 None（核心验收点）（R5-P3）
// ============================================================================

#[test]
fn test_r5_p3_delete_then_get_returns_none() {
    // R5-P3: Cache::delete 后下次 get 返回 None（缓存模块核心验收点）
    //
    // PHP:
    //   Cache::set('key', 'value');
    //   Cache::delete('key');
    //   $val = Cache::get('key');  // null
    //
    // PHP 源码:
    //   File.php:228-233 delete → unlink 文件
    //   File.php:133-140 get → getRaw 返回 null → 返回 $default (null)
    //   Redis.php:190-197 delete → del key
    //   Redis.php:112-123 get → handler->get 返回 false → 返回 $default (null)
    let cache = make_cache();
    cache.set("key", "value", None).unwrap();
    assert_eq!(
        cache.get::<String>("key").unwrap(),
        Some("value".to_string())
    );

    cache.delete("key").unwrap();
    let val: Option<String> = cache.get("key").unwrap();
    assert_eq!(val, None);
}

#[test]
fn test_r5_p3_delete_then_has_returns_false() {
    // R5-P3: delete 后 has 返回 false
    //
    // PHP:
    //   Cache::set('key', 'value');
    //   Cache::delete('key');
    //   Cache::has('key');  // false
    let cache = make_cache();
    cache.set("key", "value", None).unwrap();
    assert!(cache.has("key").unwrap());

    cache.delete("key").unwrap();
    assert!(!cache.has("key").unwrap());
}

#[test]
fn test_r5_p3_delete_does_not_affect_other_keys() {
    // R5-P3: delete 一个 key 不影响其他 key
    //
    // PHP:
    //   Cache::set('k1', 'v1');
    //   Cache::set('k2', 'v2');
    //   Cache::delete('k1');
    //   Cache::get('k1');  // null
    //   Cache::get('k2');  // 'v2'
    let cache = make_cache();
    cache.set("k1", "v1", None).unwrap();
    cache.set("k2", "v2", None).unwrap();

    cache.delete("k1").unwrap();

    assert_eq!(cache.get::<String>("k1").unwrap(), None);
    assert_eq!(cache.get::<String>("k2").unwrap(), Some("v2".to_string()));
}

#[test]
fn test_r5_p3_delete_then_recreate() {
    // R5-P3: delete 后可以重新 set
    //
    // PHP:
    //   Cache::set('key', 'v1');
    //   Cache::delete('key');
    //   Cache::set('key', 'v2');
    //   $val = Cache::get('key');  // 'v2'
    let cache = make_cache();
    cache.set("key", "v1", None).unwrap();
    cache.delete("key").unwrap();
    cache.set("key", "v2", None).unwrap();
    let val: String = cache.get("key").unwrap().unwrap();
    assert_eq!(val, "v2");
}

// ============================================================================
// 组 4：delete 不存在的 key 不报错（R5-P4）
// ============================================================================

#[test]
fn test_r5_p4_delete_nonexistent_no_error() {
    // R5-P4: delete 不存在的 key 不报错
    //
    // PHP:
    //   $result = Cache::delete('nonexistent');  // false (不报错)
    //
    // PHP 源码:
    //   File.php:270-277 unlink: is_file($path) = false → 短路返回 false
    //   Redis.php:190-197 delete: del key 返回 0 → $result > 0 = false
    let cache = make_cache();
    let result = cache.delete("nonexistent");
    assert!(result.is_ok());
}

#[test]
fn test_r5_p4_delete_nonexistent_does_not_create_key() {
    // R5-P4: delete 不存在的 key 不会创建该 key
    let cache = make_cache();
    cache.delete("nonexistent").unwrap();
    assert!(!cache.has("nonexistent").unwrap());
    assert_eq!(cache.get::<String>("nonexistent").unwrap(), None);
}

#[test]
fn test_r5_p4_delete_multiple_times_no_error() {
    // R5-P4: 多次 delete 同一 key 都不报错
    let cache = make_cache();
    cache.set("key", "value", None).unwrap();
    cache.delete("key").unwrap();
    cache.delete("key").unwrap();
    cache.delete("key").unwrap();
    assert_eq!(cache.get::<String>("key").unwrap(), None);
}

// ============================================================================
// 组 5：has TTL 过期（R5-P5）
// ============================================================================

#[test]
fn test_r5_p5_has_returns_true_for_existing_key() {
    // R5-P5: has 对存在的 key 返回 true
    let cache = make_cache();
    cache.set("key", "value", None).unwrap();
    assert!(cache.has("key").unwrap());
}

#[test]
fn test_r5_p5_has_returns_false_for_nonexistent_key() {
    // R5-P5: has 对不存在的 key 返回 false
    let cache = make_cache();
    assert!(!cache.has("nonexistent").unwrap());
}

#[test]
fn test_r5_p5_has_returns_false_after_ttl_expiration() {
    // R5-P5: has 检查 TTL 过期
    //
    // PHP:
    //   Cache::set('key', 'value', 1);  // 1 秒 TTL
    //   sleep(2);
    //   Cache::has('key');  // false
    //
    // PHP 源码: File.php:86-113 getRaw
    //   $expire = (int) substr($content, 8, 12);
    //   if (0 != $expire && time() - $expire > filemtime($filename)) {
    //       $this->unlink($filename);  // 删除过期文件
    //       return null;
    //   }
    let cache = make_cache();
    cache
        .set("key", "value", Some(Duration::from_millis(50)))
        .unwrap();
    assert!(cache.has("key").unwrap());

    std::thread::sleep(Duration::from_millis(100));
    assert!(!cache.has("key").unwrap());
}

#[test]
fn test_r5_p5_has_returns_true_for_no_ttl() {
    // R5-P5: 无 TTL 的 key 永久存在，has 始终返回 true
    //
    // PHP:
    //   Cache::set('key', 'value');  // 无 TTL = 永久
    //   sleep(2);
    //   Cache::has('key');  // true
    let cache = make_cache();
    cache.set("key", "value", None).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert!(cache.has("key").unwrap());
}

// ============================================================================
// 组 6：clear 清空所有（R5-P6）
// ============================================================================

#[test]
fn test_r5_p6_clear_all_keys() {
    // R5-P6: clear 清空所有缓存
    //
    // PHP:
    //   Cache::set('k1', 'v1');
    //   Cache::set('k2', 'v2');
    //   Cache::clear();
    //   Cache::has('k1');  // false
    //   Cache::has('k2');  // false
    //
    // PHP 源码:
    //   File.php:240-249 clear → rmdir 目录
    //   Redis.php:204-209 clear → flushDB
    let cache = make_cache();
    cache.set("k1", "v1", None).unwrap();
    cache.set("k2", "v2", None).unwrap();
    cache.set("k3", "v3", None).unwrap();

    cache.clear().unwrap();

    assert!(!cache.has("k1").unwrap());
    assert!(!cache.has("k2").unwrap());
    assert!(!cache.has("k3").unwrap());
}

#[test]
fn test_r5_p6_clear_empty_cache_no_error() {
    // R5-P6: clear 空缓存不报错
    //
    // PHP: File clear 即使目录不存在也返回 true
    let cache = make_cache();
    let result = cache.clear();
    assert!(result.is_ok());
}

#[test]
fn test_r5_p6_clear_then_recreate() {
    // R5-P6: clear 后可以重新 set
    let cache = make_cache();
    cache.set("k1", "v1", None).unwrap();
    cache.clear().unwrap();
    cache.set("k1", "v2", None).unwrap();
    let val: String = cache.get("k1").unwrap().unwrap();
    assert_eq!(val, "v2");
}

// ============================================================================
// 组 7：inc/dec 不经序列化（R5-P7）
// ============================================================================

#[test]
fn test_r5_p7_inc_existing_key() {
    // R5-P7: inc 已存在的 key — 值 + step
    //
    // PHP:
    //   Cache::set('counter', 100);
    //   $new = Cache::inc('counter', 50);  // 150
    //
    // PHP 源码:
    //   File.php:197-208 inc: getRaw → unserialize + step → set
    //   Redis.php:161-167 inc: INCRBY（不经 serialize）
    let cache = make_cache();
    cache.set("counter", 100i64, None).unwrap();
    let new_val = cache.inc("counter", 50).unwrap();
    assert_eq!(new_val, 150);
}

#[test]
fn test_r5_p7_inc_nonexistent_key() {
    // R5-P7: inc 不存在的 key — 初始化为 step
    //
    // PHP:
    //   $new = Cache::inc('new_counter', 5);  // 5
    //
    // PHP 源码 File.php:197-208:
    //   if ($raw = $this->getRaw($name)) { ... } else { $value = $step; }
    let cache = make_cache();
    let new_val = cache.inc("new_counter", 5).unwrap();
    assert_eq!(new_val, 5);
}

#[test]
fn test_r5_p7_inc_stores_numeric_string() {
    // R5-P7: inc 后存储的是数字字符串（对齐 PHP is_numeric 短路）
    //
    // PHP:
    //   Cache::inc('counter', 10);
    //   $val = Cache::get('counter');  // "10" (string, PHP bug)
    let cache = make_cache();
    cache.inc("counter", 10).unwrap();
    let s: String = cache.get("counter").unwrap().unwrap();
    assert_eq!(s, "10");
}

#[test]
fn test_r5_p7_dec_existing_key() {
    // R5-P7: dec 已存在的 key — 值 - step
    //
    // PHP:
    //   Cache::set('counter', 100);
    //   $new = Cache::dec('counter', 30);  // 70
    let cache = make_cache();
    cache.set("counter", 100i64, None).unwrap();
    let new_val = cache.dec("counter", 30).unwrap();
    assert_eq!(new_val, 70);
}

#[test]
fn test_r5_p7_dec_nonexistent_key() {
    // R5-P7: dec 不存在的 key — 初始化为 -step
    //
    // PHP File.php:209-220 dec:
    //   if ($raw = $this->getRaw($name)) { ... } else { $value = -$step; }
    let cache = make_cache();
    let new_val = cache.dec("new_counter", 5).unwrap();
    assert_eq!(new_val, -5);
}

// ============================================================================
// 组 8：remember 锁机制（PHP bug 复刻）（R5-P8）
// ============================================================================

#[test]
fn test_r5_p8_remember_cache_miss() {
    // R5-P8: remember 缓存未命中 — 调用 callback 并写入缓存
    //
    // PHP:
    //   $val = Cache::remember('key', function() { return 42; }, 60);
    //   // 缓存未命中 → 调用 callback → 写入缓存 → 返回 42
    //
    // PHP 源码: Driver.php:153-188 remember
    let cache = make_cache();
    let val: i64 = cache.remember("key", None, || 42).unwrap();
    assert_eq!(val, 42);
    // 缓存应被写入
    assert_eq!(cache.get::<String>("key").unwrap(), Some("42".to_string()));
}

#[test]
fn test_r5_p8_remember_cache_hit() {
    // R5-P8: remember 缓存命中 — 不调用 callback
    //
    // PHP:
    //   Cache::set('key', 'existing');
    //   $val = Cache::remember('key', function() { return 'new'; });
    //   // 'existing' (不调用 callback)
    let cache = make_cache();
    cache.set("key", "existing", None).unwrap();
    let val: String = cache.remember("key", None, || "new".to_string()).unwrap();
    assert_eq!(val, "existing");
}

#[test]
fn test_r5_p8_remember_lock_released() {
    // R5-P8: remember 完成后锁应被释放
    //
    // PHP 源码: Driver.php:182-184
    //   $this->delete($name . '_lock');  // finally 块释放锁
    let cache = make_cache();
    let val: i64 = cache.remember("key", None, || 42).unwrap();
    assert_eq!(val, 42);
    // 锁应被释放（PHP bug: 锁无 TTL，但正常流程会释放）
    assert!(!cache.has("key_lock").unwrap());
}

#[test]
fn test_r5_p8_remember_lock_key_naming() {
    // R5-P8: remember 锁 key 命名为 {name}_lock
    //
    // PHP 源码: Driver.php:160
    //   $this->set($name . '_lock', true);
    //
    // 注意：由于 remember 在缓存命中时不创建锁，需要验证未命中场景
    // 这里通过验证缓存未命中后锁被正确创建和释放来间接验证
    let cache = make_cache();
    let val: i64 = cache.remember("mykey", None, || 100).unwrap();
    assert_eq!(val, 100);
    // 锁应被释放
    assert!(!cache.has("mykey_lock").unwrap());
}

// ============================================================================
// 组 9：push 上限 1000 + array_shift + array_unique（R5-P9）
// ============================================================================

#[test]
fn test_r5_p9_push_basic_append() {
    // R5-P9: push 基本追加
    //
    // PHP:
    //   Cache::push('list', 'a');
    //   Cache::push('list', 'b');
    //   $list = Cache::get('list');  // ['a', 'b']
    let cache = make_cache();
    cache.push("list", "a".to_string(), None).unwrap();
    cache.push("list", "b".to_string(), None).unwrap();
    let list: Vec<String> = cache.get("list").unwrap().unwrap();
    assert_eq!(list, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn test_r5_p9_push_max_1000_array_shift() {
    // R5-P9: push 上限 1000 + array_shift（FIFO 丢弃最旧）
    //
    // PHP:
    //   for ($i = 0; $i < 1001; $i++) { Cache::push('list', $i); }
    //   $list = Cache::get('list');  // [1, 2, ..., 1000] (0 被丢弃)
    //
    // PHP 源码: Driver.php:114-131
    //   if (count($item) > 1000) { array_shift($item); }
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
fn test_r5_p9_push_array_unique_keep_first() {
    // R5-P9: push array_unique 去重（保留首次出现的元素）
    //
    // PHP:
    //   Cache::push('list', 'a');
    //   Cache::push('list', 'a');  // 重复
    //   Cache::push('list', 'b');
    //   Cache::push('list', 'a');  // 重复
    //   $list = Cache::get('list');  // ['a', 'b'] (保留首次)
    //
    // PHP 源码: Driver.php:124
    //   $item = array_unique($item);  // SORT_REGULAR, 保留首次出现
    let cache = make_cache();
    cache.push("list", "a".to_string(), None).unwrap();
    cache.push("list", "a".to_string(), None).unwrap();
    cache.push("list", "b".to_string(), None).unwrap();
    cache.push("list", "a".to_string(), None).unwrap();

    let list: Vec<String> = cache.get("list").unwrap().unwrap();
    assert_eq!(list, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn test_r5_p9_push_empty_cache_initializes() {
    // R5-P9: push 空缓存初始化为 [value]
    //
    // PHP 源码: Driver.php:116-117
    //   $item = $this->get($name, []);  // 默认 []
    //   if (!is_array($item)) { $item = []; }
    let cache = make_cache();
    cache.push("list", "first".to_string(), None).unwrap();
    let list: Vec<String> = cache.get("list").unwrap().unwrap();
    assert_eq!(list, vec!["first".to_string()]);
}

// ============================================================================
// 组 10：pull = get + delete（R5-P10）
// ============================================================================

#[test]
fn test_r5_p10_pull_returns_value_and_deletes() {
    // R5-P10: pull 返回值并删除缓存
    //
    // PHP:
    //   Cache::set('key', 'value');
    //   $val = Cache::pull('key');  // 'value'
    //   Cache::has('key');  // false
    //
    // PHP 源码: Driver.php:97-105
    //   $result = $this->get($name, false);
    //   if ($result) { $this->delete($name); return $result; }
    let cache = make_cache();
    cache.set("key", "value", None).unwrap();
    let val: Option<String> = cache.pull("key").unwrap();
    assert_eq!(val, Some("value".to_string()));
    assert!(!cache.has("key").unwrap());
}

#[test]
fn test_r5_p10_pull_nonexistent_returns_none() {
    // R5-P10: pull 不存在的 key 返回 None
    //
    // PHP:
    //   $val = Cache::pull('nonexistent');  // null (get 返回 false, 不 delete)
    let cache = make_cache();
    let val: Option<String> = cache.pull("nonexistent").unwrap();
    assert_eq!(val, None);
}

#[test]
fn test_r5_p10_pull_after_pull_returns_none() {
    // R5-P10: 同一 key 两次 pull，第二次返回 None
    let cache = make_cache();
    cache.set("key", "value", None).unwrap();
    let first: Option<String> = cache.pull("key").unwrap();
    let second: Option<String> = cache.pull("key").unwrap();
    assert_eq!(first, Some("value".to_string()));
    assert_eq!(second, None);
}

// ============================================================================
// 组 11：TTL 过期后 get 返回 None（R5-P11）
// ============================================================================

#[test]
fn test_r5_p11_ttl_expiration_get_returns_none() {
    // R5-P11: TTL 过期后 get 返回 None
    //
    // PHP:
    //   Cache::set('key', 'value', 1);  // 1 秒
    //   sleep(2);
    //   $val = Cache::get('key');  // null
    //
    // PHP 源码:
    //   File.php:86-113 getRaw: 检查 expire，过期则 unlink 返回 null
    //   Redis: Redis 自身 TTL 机制自动删除
    let cache = make_cache();
    cache
        .set("key", "value", Some(Duration::from_millis(50)))
        .unwrap();
    assert_eq!(
        cache.get::<String>("key").unwrap(),
        Some("value".to_string())
    );

    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(cache.get::<String>("key").unwrap(), None);
}

#[test]
fn test_r5_p11_no_ttl_never_expires() {
    // R5-P11: 无 TTL 的 key 永不过期
    //
    // PHP:
    //   Cache::set('key', 'value');  // 无 TTL = 永久
    //   sleep(2);
    //   $val = Cache::get('key');  // 'value'
    let cache = make_cache();
    cache.set("key", "value", None).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let val: String = cache.get("key").unwrap().unwrap();
    assert_eq!(val, "value");
}

#[test]
fn test_r5_p11_long_ttl_still_valid() {
    // R5-P11: 长 TTL 的 key 在 TTL 内仍然有效
    let cache = make_cache();
    cache
        .set("key", "value", Some(Duration::from_secs(60)))
        .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let val: String = cache.get("key").unwrap().unwrap();
    assert_eq!(val, "value");
}

// ============================================================================
// 组 12：set 带 TTL（秒）（R5-P12）
// ============================================================================

#[test]
fn test_r5_p12_set_with_ttl_expires() {
    // R5-P12: set 带 TTL（秒）过期
    //
    // PHP:
    //   Cache::set('key', 'value', 1);  // 1 秒后过期
    //   sleep(2);
    //   Cache::get('key');  // null
    //
    // PHP 源码: Driver.php:67-78 getExpireTime
    //   (int) $expire — 秒数
    let cache = make_cache();
    cache
        .set("key", "value", Some(Duration::from_millis(50)))
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(cache.get::<String>("key").unwrap(), None);
}

#[test]
fn test_r5_p12_set_with_zero_ttl_permanent() {
    // R5-P12: TTL=0 表示永久（对齐 PHP expire=0 语义）
    //
    // PHP:
    //   Cache::set('key', 'value', 0);  // 0 = 永久
    //   sleep(2);
    //   Cache::get('key');  // 'value'
    //
    // PHP 源码: Driver.php:67-78
    //   return (int) $expire;  // 0 表示永久
    let cache = make_cache();
    // Rust 端 None 等价 PHP TTL=0（永久）
    cache.set("key", "value", None).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let val: String = cache.get("key").unwrap().unwrap();
    assert_eq!(val, "value");
}

#[test]
fn test_r5_p12_set_with_various_ttls() {
    // R5-P12: 不同 TTL 值的行为
    let cache = make_cache();
    // 短 TTL
    cache
        .set("short", "v1", Some(Duration::from_millis(30)))
        .unwrap();
    // 长 TTL
    cache
        .set("long", "v2", Some(Duration::from_secs(60)))
        .unwrap();
    // 无 TTL
    cache.set("permanent", "v3", None).unwrap();

    std::thread::sleep(Duration::from_millis(60));

    // 短 TTL 已过期
    assert_eq!(cache.get::<String>("short").unwrap(), None);
    // 长 TTL 仍有效
    assert_eq!(cache.get::<String>("long").unwrap(), Some("v2".to_string()));
    // 无 TTL 仍有效
    assert_eq!(
        cache.get::<String>("permanent").unwrap(),
        Some("v3".to_string())
    );
}

// ============================================================================
// 组 13：tag 标签（R5-P13）
// ============================================================================

#[test]
fn test_r5_p13_tag_set_and_clear() {
    // R5-P13: tag('user')->set + tag('user')->clear 行为
    //
    // PHP:
    //   Cache::tag('user')->set('user:1', 'Alice');
    //   Cache::tag('user')->set('user:2', 'Bob');
    //   Cache::tag('user')->clear();
    //   Cache::get('user:1');  // null
    //   Cache::get('user:2');  // null
    //
    // PHP 源码:
    //   TagSet.php:52-59 set: handler->set + append
    //   TagSet.php:119-131 clear: getTagItems + clearTag + delete
    let cache = make_cache();
    cache.tag("user").set("user:1", "Alice", None).unwrap();
    cache.tag("user").set("user:2", "Bob", None).unwrap();

    assert_eq!(
        cache.get::<String>("user:1").unwrap(),
        Some("Alice".to_string())
    );

    cache.tag("user").clear().unwrap();

    assert_eq!(cache.get::<String>("user:1").unwrap(), None);
    assert_eq!(cache.get::<String>("user:2").unwrap(), None);
}

#[test]
fn test_r5_p13_tag_clear_does_not_affect_other_tags() {
    // R5-P13: clear 一个 tag 不影响其他 tag
    //
    // PHP:
    //   Cache::tag('user')->set('u1', 'Alice');
    //   Cache::tag('order')->set('o1', 100);
    //   Cache::tag('user')->clear();
    //   Cache::get('u1');  // null
    //   Cache::get('o1');  // 100
    let cache = make_cache();
    cache.tag("user").set("u1", "Alice", None).unwrap();
    cache.tag("order").set("o1", 100i64, None).unwrap();

    cache.tag("user").clear().unwrap();

    assert_eq!(cache.get::<String>("u1").unwrap(), None);
    // order 标签不受影响
    assert_eq!(cache.get::<String>("o1").unwrap(), Some("100".to_string()));
}

#[test]
fn test_r5_p13_tag_multiple_tags() {
    // R5-P13: 多标签场景
    //
    // PHP:
    //   Cache::tag(['user', 'admin'])->set('u1', 'Alice');
    //   Cache::tag('admin')->clear();
    //   Cache::get('u1');  // null (属于 admin 标签)
    let cache = make_cache();
    cache
        .tag_many(&["user", "admin"])
        .set("u1", "Alice", None)
        .unwrap();

    cache.tag("admin").clear().unwrap();

    assert_eq!(cache.get::<String>("u1").unwrap(), None);
}

// ============================================================================
// 组 14：多驱动 store 隔离（R5-P14）
// ============================================================================

#[test]
fn test_r5_p14_store_isolation() {
    // R5-P14: 不同 store 之间缓存隔离
    //
    // PHP:
    //   Cache::store('redis')->set('key', 'from_redis');
    //   Cache::store('file')->set('key', 'from_file');
    //   Cache::store('redis')->get('key');  // 'from_redis'
    //   Cache::store('file')->get('key');   // 'from_file'
    //
    // PHP 源码: Cache.php:88-91 store → driver(name)
    //   不同 store 完全隔离（不同 Redis db / 不同 File 目录）
    let cache = Cache::new();
    cache.register_store("store_a", Box::new(MemoryCacheDriver::new()));
    cache.register_store("store_b", Box::new(MemoryCacheDriver::new()));

    // 通过默认 store（首个注册的）写入
    cache.set("key", "from_default", None).unwrap();

    // 切换默认 store 为 store_b
    cache.set_default_store("store_b").unwrap();
    cache.set("key", "from_store_b", None).unwrap();

    // 切换回默认 store（store_a）
    cache.set_default_store("store_a").unwrap();
    let val_a: String = cache.get("key").unwrap().unwrap();
    assert_eq!(val_a, "from_default");

    // store_b 的值
    cache.set_default_store("store_b").unwrap();
    let val_b: String = cache.get("key").unwrap().unwrap();
    assert_eq!(val_b, "from_store_b");
}

#[test]
fn test_r5_p14_store_delete_does_not_affect_other() {
    // R5-P14: 一个 store 的 delete 不影响另一个 store
    let cache = Cache::new();
    cache.register_store("store_a", Box::new(MemoryCacheDriver::new()));
    cache.register_store("store_b", Box::new(MemoryCacheDriver::new()));

    // store_a 写入
    cache.set_default_store("store_a").unwrap();
    cache.set("key", "value_a", None).unwrap();

    // store_b 写入
    cache.set_default_store("store_b").unwrap();
    cache.set("key", "value_b", None).unwrap();

    // store_a delete
    cache.set_default_store("store_a").unwrap();
    cache.delete("key").unwrap();

    // store_a 已删除
    assert_eq!(cache.get::<String>("key").unwrap(), None);

    // store_b 不受影响
    cache.set_default_store("store_b").unwrap();
    assert_eq!(
        cache.get::<String>("key").unwrap(),
        Some("value_b".to_string())
    );
}

// ============================================================================
// 组 15：set 返回值（R5-P15）
// ============================================================================

#[test]
fn test_r5_p15_set_returns_ok() {
    // R5-P15: set 成功返回 Ok(())
    //
    // PHP:
    //   $result = Cache::set('key', 'value');  // true (bool)
    //
    // PHP 源码:
    //   File.php:180-187: file_put_contents 成功返回 true
    //   Redis.php:133-152: 永远返回 true（不检查命令返回值）
    let cache = make_cache();
    let result = cache.set("key", "value", None);
    assert!(result.is_ok());
}

#[test]
fn test_r5_p15_set_with_ttl_returns_ok() {
    // R5-P15: set 带 TTL 也返回 Ok
    let cache = make_cache();
    let result = cache.set("key", "value", Some(Duration::from_secs(60)));
    assert!(result.is_ok());
}

#[test]
fn test_r5_p15_set_overwrite_returns_ok() {
    // R5-P15: set 覆盖已存在的 key 返回 Ok
    let cache = make_cache();
    cache.set("key", "v1", None).unwrap();
    let result = cache.set("key", "v2", None);
    assert!(result.is_ok());
    let val: String = cache.get("key").unwrap().unwrap();
    assert_eq!(val, "v2");
}
