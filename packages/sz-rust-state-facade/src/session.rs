//! Session 模块 — 对齐 PHP `think\facade\Session`
//!
//! 本模块实现会话管理，对齐 PHP `think-session` 包的核心 API。
//!
//! ## PHP 对齐
//!
//! ### 核心 API 映射
//!
//! | PHP 方法 | Rust 方法 | 说明 |
//! |---------|-----------|------|
//! | `Session::set($name, $value)` | [`Session::set`] | 设置会话数据 |
//! | `Session::get($name = null, $default = null)` | [`Session::get`] / [`Session::get_with_default`] | 获取会话数据 |
//! | `Session::delete($name)` | [`Session::delete`] | 删除会话数据 |
//! | `Session::has($name)` | [`Session::has`] | 检查会话数据是否存在 |
//! | `Session::clear()` | [`Session::clear`] | 清空当前会话所有数据 |
//! | `Session::flash($name, $value)` | [`Session::flash`] | 设置一次性数据（下次请求后自动删除） |
//! | `Session::flush()` | [`Session::flush`] | 清空并清除 flash 数据 |
//!
//! ### PHP 行为对齐
//!
//! - **命名空间隔离**：PHP 通过 `prefix` 配置项实现会话命名空间隔离。
//!   Rust 通过 [`Session::with_prefix`] 提供 per-instance 前缀。
//! - **Flash 数据**：PHP `flash()` 设置的数据在下次请求开始时通过
//!   `clearFlashData()` 自动清除。Rust 通过 [`Session::clear_flash`] 显式清除
//!   （由中间件在请求结束时调用）。
//!
//! ## 架构说明
//!
//! 本模块仅提供**数据存储 API**，不涉及 session ID 管理和 Cookie 传输。
//! 这些由后续的 axum 中间件层处理（通过 `Set-Cookie: SZ_SESSION_ID=xxx` 头）。
//!
//! ### 后端存储驱动
//!
//! 通过 [`SessionStore`] trait 抽象，内置 [`MemorySessionStore`] 实现：
//! - **MemorySessionStore**：基于 `parking_lot::RwLock<HashMap<String, Value>>`
//!   的内存存储，适用于单进程开发环境。生产环境可自定义实现 Redis/数据库后端。

use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// Session 存储后端 trait
// ============================================================================

/// Session 存储后端 trait
///
/// 抽象会话数据的持久化层，对齐 PHP `think\session\Driver` 抽象类。
/// 实现方需提供基于 session_id 的命名空间隔离。
///
/// # PHP 对齐
///
/// ```php
/// abstract class Driver implements SessionHandlerInterface {
///     // read($sessionId): string
///     // write($sessionId, $data): bool
///     // destroy($sessionId): bool
///     // gc($maxLifetime): int
/// }
/// ```
pub trait SessionStore: Send + Sync {
    /// 读取指定 session_id 的所有数据
    ///
    /// 返回 `None` 表示 session 不存在或已过期。
    fn read(&self, session_id: &str) -> Option<HashMap<String, Value>>;

    /// 写入指定 session_id 的完整数据
    ///
    /// 对齐 PHP `write($sessionId, $data)`。
    fn write(&self, session_id: &str, data: HashMap<String, Value>);

    /// 销毁指定 session_id
    ///
    /// 对齐 PHP `destroy($sessionId)`。
    fn destroy(&self, session_id: &str);

    /// 检查指定 session_id 是否存在
    fn exists(&self, session_id: &str) -> bool {
        self.read(session_id).is_some()
    }
}

/// 内存 Session 存储（基于 HashMap）
///
/// 适用于单进程开发环境。生产环境应使用 Redis 或数据库后端。
///
/// # 线程安全
///
/// 通过 `Arc<RwLock<HashMap<...>>>` 提供线程安全访问，支持并发读、互斥写。
#[derive(Debug, Clone, Default)]
pub struct MemorySessionStore {
    data: Arc<RwLock<HashMap<String, HashMap<String, Value>>>>,
}

impl MemorySessionStore {
    /// 创建新的内存 Session 存储
    pub fn new() -> Self {
        Self::default()
    }
}

impl SessionStore for MemorySessionStore {
    fn read(&self, session_id: &str) -> Option<HashMap<String, Value>> {
        self.data.read().get(session_id).cloned()
    }

    fn write(&self, session_id: &str, data: HashMap<String, Value>) {
        self.data.write().insert(session_id.to_string(), data);
    }

    fn destroy(&self, session_id: &str) {
        self.data.write().remove(session_id);
    }

    fn exists(&self, session_id: &str) -> bool {
        self.data.read().contains_key(session_id)
    }
}

// ============================================================================
// Session 主结构
// ============================================================================

/// Flash 数据键前缀（对齐 PHP `think\session\Driver` 内部 flash 标记）
///
/// PHP 通过 `$this->data['__flash__']` 数组存储 flash 数据的元信息。
/// Rust 使用键前缀 `__flash__:` 标记 flash 数据，简化实现。
const FLASH_PREFIX: &str = "__flash__:";

/// 会话实例 — 对齐 PHP `think\Session`
///
/// 每个 [`Session`] 实例绑定一个 `session_id`，通过 [`SessionStore`] 后端
/// 读写数据。实例本身不缓存数据，每次操作都直接访问后端（对齐 PHP 行为）。
///
/// # 用法
///
/// ```ignore
/// use sz_rust_state_facade::session::{Session, MemorySessionStore};
/// use serde_json::json;
///
/// let store = MemorySessionStore::new();
/// let session = Session::new("session-id-123", store);
///
/// session.set("user_id", json!(12345));
/// assert_eq!(session.get("user_id"), Some(json!(12345)));
/// assert!(session.has("user_id"));
/// ```
pub struct Session {
    /// 当前会话 ID（对齐 PHP `session_id()`）
    session_id: String,
    /// 数据键前缀（对齐 PHP `think\Session::$prefix`）
    prefix: String,
    /// 存储后端
    store: Arc<dyn SessionStore>,
}

impl Session {
    /// 创建新的会话实例
    ///
    /// # 参数
    ///
    /// - `session_id`：会话唯一标识（通常由中间件从 Cookie 中提取或新生成）
    /// - `store`：存储后端（如 [`MemorySessionStore`])
    pub fn new(session_id: impl Into<String>, store: impl SessionStore + 'static) -> Self {
        Self {
            session_id: session_id.into(),
            prefix: String::new(),
            store: Arc::new(store),
        }
    }

    /// 从 `Arc<dyn SessionStore>` 创建会话（共享后端实例）
    pub fn with_shared_store(session_id: impl Into<String>, store: Arc<dyn SessionStore>) -> Self {
        Self {
            session_id: session_id.into(),
            prefix: String::new(),
            store,
        }
    }

    /// 设置键前缀（对齐 PHP `think\Session::prefix($prefix)`）
    ///
    /// 设置后，所有 `set`/`get`/`delete`/`has` 操作都会自动添加此前缀。
    /// 用于在同一 session_id 下实现命名空间隔离。
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// 获取当前会话 ID
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 应用前缀到键名（内部辅助方法）
    fn full_key(&self, name: &str) -> String {
        if self.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}{}", self.prefix, name)
        }
    }

    /// 设置会话数据（对齐 PHP `Session::set($name, $value)`）
    ///
    /// # PHP 对齐
    ///
    /// ```php
    /// public function set(string $name, $value): void
    /// ```
    pub fn set(&self, name: &str, value: Value) {
        let mut data = self.store.read(&self.session_id).unwrap_or_default();
        data.insert(self.full_key(name), value);
        self.store.write(&self.session_id, data);
    }

    /// 获取会话数据（对齐 PHP `Session::get($name = null, $default = null)`）
    ///
    /// # 返回
    ///
    /// - `Some(value)`：键存在
    /// - `None`：键不存在
    pub fn get(&self, name: &str) -> Option<Value> {
        let data = self.store.read(&self.session_id)?;
        data.get(&self.full_key(name)).cloned()
    }

    /// 获取会话数据，键不存在时返回默认值
    ///
    /// 对齐 PHP `Session::get($name, $default)`。
    pub fn get_with_default(&self, name: &str, default: Value) -> Value {
        self.get(name).unwrap_or(default)
    }

    /// 检查会话数据是否存在（对齐 PHP `Session::has($name)`）
    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// 删除会话数据（对齐 PHP `Session::delete($name)`）
    ///
    /// 返回被删除的值（对齐 PHP 行为：删除不存在的键返回 null）。
    pub fn delete(&self, name: &str) -> Option<Value> {
        let mut data = self.store.read(&self.session_id)?;
        let key = self.full_key(name);
        let removed = data.remove(&key);
        self.store.write(&self.session_id, data);
        removed
    }

    /// 清空当前会话的所有数据（对齐 PHP `Session::clear()`）
    ///
    /// 注意：此操作会删除整个 session_id 对应的数据，包括 flash 数据。
    pub fn clear(&self) {
        self.store.destroy(&self.session_id);
    }

    /// 设置 flash 数据（对齐 PHP `Session::flash($name, $value)`）
    ///
    /// Flash 数据在设置后，于下次调用 [`Session::clear_flash`] 时被清除。
    /// 通常由中间件在请求结束时调用 `clear_flash`。
    ///
    /// # 实现说明
    ///
    /// 使用 `__flash__:` 前缀标记 flash 数据，与普通数据隔离存储。
    pub fn flash(&self, name: &str, value: Value) {
        let flash_key = format!("{}{}", FLASH_PREFIX, name);
        self.set(&flash_key, value);
    }

    /// 获取 flash 数据
    ///
    /// 与 [`Session::get`] 类似，但自动添加 flash 前缀。
    pub fn get_flash(&self, name: &str) -> Option<Value> {
        let flash_key = format!("{}{}", FLASH_PREFIX, name);
        self.get(&flash_key)
    }

    /// 清除所有 flash 数据（对齐 PHP `think\session\Driver::clearFlashData()`）
    ///
    /// 应在请求结束时由中间件调用，以实现 flash 数据的"一次性"语义。
    pub fn clear_flash(&self) {
        let mut data = match self.store.read(&self.session_id) {
            Some(d) => d,
            None => return,
        };
        // 移除所有以 __flash__: 开头的键
        let flash_keys: Vec<String> = data
            .keys()
            .filter(|k| k.starts_with(FLASH_PREFIX))
            .cloned()
            .collect();
        for key in flash_keys {
            data.remove(&key);
        }
        self.store.write(&self.session_id, data);
    }

    /// 清空所有数据并销毁 session（对齐 PHP `Session::flush()`）
    ///
    /// 与 [`Session::clear`] 的区别：`flush` 同时清除 flash 数据，
    /// 行为上等价于 `clear`（因为 `clear` 直接销毁整个 session）。
    pub fn flush(&self) {
        self.clear();
    }

    /// 获取当前会话的所有数据（不含 flash 数据）
    ///
    /// 对齐 PHP `Session::all()`。
    pub fn all(&self) -> HashMap<String, Value> {
        let data = self.store.read(&self.session_id).unwrap_or_default();
        // 过滤掉 flash 数据
        data.into_iter()
            .filter(|(k, _)| !k.starts_with(FLASH_PREFIX))
            .collect()
    }
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ========================================================================
    // MemorySessionStore 测试
    // ========================================================================

    #[test]
    fn test_memory_store_write_read_roundtrip() {
        let store = MemorySessionStore::new();
        let mut data = HashMap::new();
        data.insert("user_id".to_string(), json!(12345));
        data.insert("name".to_string(), json!("alice"));

        store.write("session-1", data.clone());
        let read = store.read("session-1").unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read.get("user_id"), Some(&json!(12345)));
        assert_eq!(read.get("name"), Some(&json!("alice")));
    }

    #[test]
    fn test_memory_store_read_nonexistent_returns_none() {
        let store = MemorySessionStore::new();
        assert!(store.read("nonexistent").is_none());
    }

    #[test]
    fn test_memory_store_destroy() {
        let store = MemorySessionStore::new();
        let data = HashMap::new();
        store.write("session-1", data);
        assert!(store.exists("session-1"));

        store.destroy("session-1");
        assert!(!store.exists("session-1"));
    }

    #[test]
    fn test_memory_store_isolated_by_session_id() {
        let store = MemorySessionStore::new();
        let mut data1 = HashMap::new();
        data1.insert("user".to_string(), json!("alice"));
        store.write("session-1", data1);

        let mut data2 = HashMap::new();
        data2.insert("user".to_string(), json!("bob"));
        store.write("session-2", data2);

        assert_eq!(
            store.read("session-1").unwrap().get("user"),
            Some(&json!("alice"))
        );
        assert_eq!(
            store.read("session-2").unwrap().get("user"),
            Some(&json!("bob"))
        );
    }

    #[test]
    fn test_memory_store_overwrite() {
        let store = MemorySessionStore::new();
        let mut data = HashMap::new();
        data.insert("key".to_string(), json!("old"));
        store.write("session-1", data);

        let mut new_data = HashMap::new();
        new_data.insert("key".to_string(), json!("new"));
        store.write("session-1", new_data);

        assert_eq!(
            store.read("session-1").unwrap().get("key"),
            Some(&json!("new"))
        );
    }

    // ========================================================================
    // Session 基本 API 测试
    // ========================================================================

    /// 创建测试用 Session
    fn make_session() -> Session {
        Session::new("test-session-id", MemorySessionStore::new())
    }

    #[test]
    fn test_session_set_get() {
        let session = make_session();
        session.set("user_id", json!(12345));
        assert_eq!(session.get("user_id"), Some(json!(12345)));
    }

    #[test]
    fn test_session_set_string_value() {
        let session = make_session();
        session.set("name", json!("alice"));
        assert_eq!(session.get("name"), Some(json!("alice")));
    }

    #[test]
    fn test_session_set_object_value() {
        let session = make_session();
        session.set("user", json!({"id": 1, "name": "bob"}));
        let value = session.get("user").unwrap();
        assert_eq!(value["id"], 1);
        assert_eq!(value["name"], "bob");
    }

    #[test]
    fn test_session_get_nonexistent_returns_none() {
        let session = make_session();
        assert_eq!(session.get("missing"), None);
    }

    #[test]
    fn test_session_get_with_default_returns_value_when_exists() {
        let session = make_session();
        session.set("key", json!("actual"));
        assert_eq!(
            session.get_with_default("key", json!("default")),
            json!("actual")
        );
    }

    #[test]
    fn test_session_get_with_default_returns_default_when_missing() {
        let session = make_session();
        assert_eq!(
            session.get_with_default("missing", json!("default")),
            json!("default")
        );
    }

    #[test]
    fn test_session_has_existing_key() {
        let session = make_session();
        session.set("key", json!(1));
        assert!(session.has("key"));
    }

    #[test]
    fn test_session_has_nonexistent_key() {
        let session = make_session();
        assert!(!session.has("missing"));
    }

    #[test]
    fn test_session_delete_returns_value() {
        let session = make_session();
        session.set("key", json!("value"));
        let removed = session.delete("key");
        assert_eq!(removed, Some(json!("value")));
        assert!(!session.has("key"));
    }

    #[test]
    fn test_session_delete_nonexistent_returns_none() {
        let session = make_session();
        let removed = session.delete("missing");
        assert_eq!(removed, None);
    }

    #[test]
    fn test_session_clear_removes_all_data() {
        let session = make_session();
        session.set("key1", json!(1));
        session.set("key2", json!(2));
        session.set("key3", json!(3));

        session.clear();

        assert!(!session.has("key1"));
        assert!(!session.has("key2"));
        assert!(!session.has("key3"));
    }

    #[test]
    fn test_session_all_returns_non_flash_data() {
        let session = make_session();
        session.set("key1", json!(1));
        session.set("key2", json!("two"));
        session.flash("temp", json!("flash"));

        let all = session.all();
        assert_eq!(all.len(), 2); // 不含 flash 数据
        assert_eq!(all.get("key1"), Some(&json!(1)));
        assert_eq!(all.get("key2"), Some(&json!("two")));
    }

    // ========================================================================
    // Session 前缀测试
    // ========================================================================

    #[test]
    fn test_session_prefix_isolation() {
        let store = MemorySessionStore::new();
        let session_a = Session::new("sid", store.clone()).with_prefix("app_a_");
        let session_b = Session::new("sid", store.clone()).with_prefix("app_b_");

        session_a.set("user", json!("alice"));
        session_b.set("user", json!("bob"));

        // 同一 session_id，但通过前缀隔离
        assert_eq!(session_a.get("user"), Some(json!("alice")));
        assert_eq!(session_b.get("user"), Some(json!("bob")));
    }

    #[test]
    fn test_session_prefix_empty_by_default() {
        let session = make_session();
        assert_eq!(session.prefix, "");
    }

    // ========================================================================
    // Flash 数据测试
    // ========================================================================

    #[test]
    fn test_session_flash_set_get() {
        let session = make_session();
        session.flash("success", json!("操作成功"));
        assert_eq!(session.get_flash("success"), Some(json!("操作成功")));
    }

    #[test]
    fn test_session_flash_not_in_regular_get() {
        let session = make_session();
        session.flash("temp", json!("flash data"));

        // 通过普通 get 获取 flash 数据需要带前缀（不应直接获取）
        assert_eq!(session.get("temp"), None);
        // 通过 __flash__: 前缀可以获取（内部实现细节）
        assert_eq!(session.get("__flash__:temp"), Some(json!("flash data")));
    }

    #[test]
    fn test_session_clear_flash_removes_flash_data() {
        let session = make_session();
        session.flash("temp1", json!(1));
        session.flash("temp2", json!(2));
        session.set("regular", json!("keep"));

        session.clear_flash();

        // flash 数据被清除
        assert_eq!(session.get_flash("temp1"), None);
        assert_eq!(session.get_flash("temp2"), None);
        // 普通数据保留
        assert_eq!(session.get("regular"), Some(json!("keep")));
    }

    #[test]
    fn test_session_clear_flash_when_no_data() {
        // 无数据时调用 clear_flash 不应 panic，且 flash 保持为空
        let session = make_session();
        session.clear_flash();
        assert_eq!(
            session.get_flash("any"),
            None,
            "clear_flash 后 flash 数据应为空"
        );
    }

    #[test]
    fn test_session_flush_equals_clear() {
        let session1 = make_session();
        let session2 = make_session();

        session1.set("key", json!(1));
        session2.set("key", json!(1));

        session1.clear();
        session2.flush();

        // 两者行为一致：清空所有数据
        assert!(!session1.has("key"));
        assert!(!session2.has("key"));
    }

    // ========================================================================
    // 多 Session 共享后端测试
    // ========================================================================

    #[test]
    fn test_multiple_sessions_share_store() {
        let store = Arc::new(MemorySessionStore::new());
        let session1 = Session::with_shared_store("sid-1", store.clone());
        let session2 = Session::with_shared_store("sid-2", store.clone());

        session1.set("user", json!("alice"));
        session2.set("user", json!("bob"));

        // 不同 session_id 数据隔离
        assert_eq!(session1.get("user"), Some(json!("alice")));
        assert_eq!(session2.get("user"), Some(json!("bob")));

        // 销毁 session1 不影响 session2
        session1.clear();
        assert!(!session1.has("user"));
        assert!(session2.has("user"));
    }

    #[test]
    fn test_session_id_access() {
        let session = Session::new("my-session-id", MemorySessionStore::new());
        assert_eq!(session.session_id(), "my-session-id");
    }

    // ========================================================================
    // PHP 一致性综合测试
    // ========================================================================

    #[test]
    fn test_php_consistency_session_full_flow() {
        // 模拟 PHP 控制器典型流程：登录 → 设置 session → 后续请求读取 session
        let store = Arc::new(MemorySessionStore::new());

        // 1. 登录请求：设置 szshop_clerk 数据
        let login_session = Session::with_shared_store("sid-login", store.clone());
        login_session.set(
            "szshop_clerk",
            json!({"clerk_id": 100, "name": "张三", "store_id": 5}),
        );

        // 2. 后续请求：读取 szshop_clerk 数据（模拟 AuthService::__construct）
        let later_session = Session::with_shared_store("sid-login", store.clone());
        let clerk = later_session.get("szshop_clerk").unwrap();
        assert_eq!(clerk["clerk_id"], 100);
        assert_eq!(clerk["name"], "张三");
        assert_eq!(clerk["store_id"], 5);

        // 3. 登出：清空 session
        later_session.clear();
        assert!(!later_session.has("szshop_clerk"));
    }

    #[test]
    fn test_php_consistency_flash_message_flow() {
        // 模拟 PHP flash 消息场景：设置成功消息 → 重定向 → 下次请求显示后清除
        let store = Arc::new(MemorySessionStore::new());

        // 1. 表单提交：设置 flash 消息
        let submit_session = Session::with_shared_store("sid", store.clone());
        submit_session.flash("success", json!("保存成功"));

        // 2. 重定向后：读取 flash 消息
        let redirect_session = Session::with_shared_store("sid", store.clone());
        assert_eq!(
            redirect_session.get_flash("success"),
            Some(json!("保存成功"))
        );

        // 3. 请求结束：清除 flash 数据
        redirect_session.clear_flash();

        // 4. 再次请求：flash 消息已清除
        let next_session = Session::with_shared_store("sid", store.clone());
        assert_eq!(next_session.get_flash("success"), None);
    }
}
