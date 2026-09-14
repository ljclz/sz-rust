// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! P9-FACADE-01：cache + state 联动集成测试
//!
//! 验证 `sz-rust-cache-facade` 与 `sz-rust-state-facade`
//! （session / env / event 三个子模块）之间的数据协作。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use sz_rust_cache_facade::{Cache, MemoryCacheDriver};
use sz_rust_state_facade::env::Env;
use sz_rust_state_facade::event::{ClosureListener, EventDispatcher};
use sz_rust_state_facade::session::{MemorySessionStore, Session};

/// 会话数据 → 缓存写入 → 读回一致（session 状态流经 cache 层）
#[test]
fn session_data_flows_into_cache() {
    let cache = Cache::new();
    cache.register_default(MemoryCacheDriver::new());

    // 1. state-facade：会话层写入用户状态
    let session = Session::new("sess_1001", MemorySessionStore::new());
    session.set("user_id", json!(1001));
    session.set("cart_count", json!(3));

    // 2. cache-facade：业务流将会话数据落缓存
    // 注：cache 对齐 PHP serialize 语义，数值存为字符串（Number(String)）
    let user_id = session.get("user_id").unwrap().as_i64().unwrap();
    let cart_count = session.get("cart_count").unwrap().as_i64().unwrap();
    cache
        .set("cart:1001", cart_count.to_string(), None)
        .unwrap();
    cache
        .set("uid:cart:1001", user_id.to_string(), None)
        .unwrap();

    // 3. 读回验证（跨 facade 类型一致性）
    let cached: String = cache.get("cart:1001").unwrap().unwrap();
    assert_eq!(cached, "3", "P9-FACADE-01: 购物车数量经缓存读回应一致");
    assert!(cache.has("uid:cart:1001").unwrap());

    // 4. 会话侧读回（session 与 cache 各自独立存储，值一致）
    assert_eq!(session.get("user_id").unwrap(), json!(1001));
}

/// Env 配置 → 缓存 TTL 参数化（state 配置驱动 cache 行为）
#[test]
fn env_config_drives_cache_ttl() {
    let cache = Cache::new();
    cache.register_default(MemoryCacheDriver::new());

    let env = Env::new();
    env.set("CACHE_TTL_SECONDS", "60");

    // state-facade：从 Env 读取配置
    let ttl_secs: u64 = env.get("CACHE_TTL_SECONDS").unwrap().parse().unwrap();
    assert_eq!(ttl_secs, 60);

    // cache-facade：用配置化 TTL 写入
    cache
        .set(
            "config:key",
            "v",
            Some(std::time::Duration::from_secs(ttl_secs)),
        )
        .unwrap();
    let v: String = cache.get("config:key").unwrap().unwrap();
    assert_eq!(v, "v");
}

/// 事件驱动：EventDispatcher 触发事件 → 闭包监听器 → 原子计数器（跨 facade 状态同步）
#[test]
fn event_dispatcher_syncs_state_to_counter() {
    let dispatcher = EventDispatcher::new();
    let hits = Arc::new(AtomicI64::new(0));
    let hits_clone = hits.clone();

    dispatcher.listen(
        "OrderPaid",
        Arc::new(ClosureListener::new(move |params: &Value| {
            let amount = params["amount"].as_i64().unwrap_or(0);
            hits_clone.fetch_add(amount, Ordering::SeqCst);
            Ok(Value::Null)
        })),
        false,
    );

    // 触发两次支付事件
    let r1 = dispatcher.trigger("OrderPaid", &json!({"amount": 8800}), false);
    let r2 = dispatcher.trigger("OrderPaid", &json!({"amount": 1200}), false);
    assert!(r1.is_ok(), "P9-FACADE-01: 第一次事件触发应成功");
    assert!(r2.is_ok(), "P9-FACADE-01: 第二次事件触发应成功");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        10_000,
        "P9-FACADE-01: 累计支付金额应为 8800+1200=10000"
    );
    assert_eq!(dispatcher.listener_count("OrderPaid"), 1);
}

/// 事件 → 缓存：事件监听器将事件数据写入缓存，随后从缓存读回
#[test]
fn event_writes_to_cache_and_reads_back() {
    let cache = Cache::new();
    cache.register_default(MemoryCacheDriver::new());
    let dispatcher = EventDispatcher::new();

    let cache_clone = /* Cache 不实现 Clone，经 Arc 共享 */ Arc::new(cache);
    let shared = cache_clone.clone();
    dispatcher.listen(
        "UserRegistered",
        Arc::new(ClosureListener::new(move |params: &Value| {
            let uid = params["uid"].as_i64().unwrap_or(0);
            shared
                .set("user:registered", uid.to_string(), None)
                .unwrap();
            Ok(Value::Null)
        })),
        false,
    );

    dispatcher
        .trigger("UserRegistered", &json!({"uid": 2024}), false)
        .unwrap();

    let stored: String = cache_clone.get("user:registered").unwrap().unwrap();
    assert_eq!(stored, "2024", "P9-FACADE-01: 事件写入缓存后应可读回");
}
