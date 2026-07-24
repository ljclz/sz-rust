//! 事件系统（对齐 PHP `think\Event`）
//!
//! 对齐 PHP `think\Event`（272 行）+ `think\facade\Event` + `event()` 助手函数。
//!
//! ## PHP API 对齐映射
//!
//! | PHP `think\Event` 方法 | Rust `EventDispatcher` 方法 | 说明 |
//! |------------------------|----------------------------|------|
//! | `listenEvents(array $events)` | `listen_events(events)` | 批量注册事件监听 |
//! | `listen(string $event, $listener, bool $first = false)` | `listen(event, listener, first)` | 注册事件监听 |
//! | `hasListener(string $event): bool` | `has_listener(event)` | 是否存在事件监听 |
//! | `remove(string $event): void` | `remove(event)` | 移除事件监听 |
//! | `bind(array $events)` | `bind(events)` | 指定事件别名标识 |
//! | `subscribe($subscriber)` | `subscribe(subscriber)` | 注册事件订阅者 |
//! | `observe($observer, $prefix = '')` | `observe(observer, prefix)` | 自动注册观察者 |
//! | `trigger($event, $params = null, bool $once = false)` | `trigger(event, params, once)` | 触发事件 |
//! | `until($event, $params = null)` | `until(event, params)` | 只获取一个有效返回值 |
//!
//! ## PHP Listener 类型映射
//!
//! PHP `$listener` 可以是：
//! - 闭包 `function($params) { ... }` → Rust `Arc<dyn Fn(¶ms) -> Result<Value>>`
//! - 对象方法数组 `[$obj, 'method']` → Rust `Arc<dyn Listener>` trait
//! - 字符串类名（调用 `handle` 方法） → Rust `Arc<dyn Listener>` trait
//! - 静态方法字符串 `Class::method` → Rust `Arc<dyn Listener>` trait
//!
//! Rust 端统一用 `Listener` trait（`handle(¶ms) -> Result<Value>`）+ 闭包包装。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde_json::Value;
use tokio::task::JoinHandle;

/// 事件监听器 trait（对齐 PHP `Listener` 接口 / `handle` 方法）
///
/// PHP 中监听器通常是一个类，含 `handle($params)` 方法：
/// ```php
/// class UserLoginListener
/// {
///     public function handle($params)
///     {
///         // 处理事件
///     }
/// }
/// ```
///
/// Rust 端实现 `Listener` trait 即可：
/// ```ignore
/// struct UserLoginListener;
///
/// impl Listener for UserLoginListener {
///     fn handle(&self, params: &Value) -> Result<Value, EventError> {
///         // 处理事件
///         Ok(Value::Null)
///     }
/// }
/// ```
pub trait Listener: Send + Sync {
    /// 处理事件（对齐 PHP `Listener::handle($params)`）
    ///
    /// 返回 `Ok(Value::Null)` 等价 PHP 无返回值（继续执行后续监听器）。
    /// 返回 `Ok(其他值)` 等价 PHP 返回非 null 值（在 `once=true` 模式下会停止）。
    /// 返回 `Err(_)` 等价 PHP 抛出异常（会停止后续监听器执行）。
    fn handle(&self, params: &Value) -> Result<Value, EventError>;
}

/// 闭包监听器（对齐 PHP 闭包 `$listener = function($params) { ... }`）
///
/// PHP 允许直接传入闭包作为监听器：
/// ```php
/// Event::listen('UserLogin', function($params) {
///     // 处理事件
/// });
/// ```
///
/// Rust 端用 `ClosureListener::new(closure)` 包装：
/// ```ignore
/// dispatcher.listen("UserLogin", ClosureListener::new(|params| {
///     // 处理事件
///     Ok(Value::Null)
/// }), false);
/// ```
pub struct ClosureListener<F>
where
    F: Fn(&Value) -> Result<Value, EventError> + Send + Sync + 'static,
{
    closure: F,
}

impl<F> ClosureListener<F>
where
    F: Fn(&Value) -> Result<Value, EventError> + Send + Sync + 'static,
{
    /// 创建一个闭包监听器
    pub fn new(closure: F) -> Self {
        Self { closure }
    }
}

impl<F> Listener for ClosureListener<F>
where
    F: Fn(&Value) -> Result<Value, EventError> + Send + Sync + 'static,
{
    fn handle(&self, params: &Value) -> Result<Value, EventError> {
        (self.closure)(params)
    }
}

/// 事件订阅者 trait（对齐 PHP 订阅者 `subscribe(Event $event)` 方法）
///
/// PHP 订阅者类含 `subscribe(Event $event)` 方法，手动注册多个监听器：
/// ```php
/// class UserEventSubscriber
/// {
///     public function onUserLogin($params) { ... }
///     public function onUserLogout($params) { ... }
///
///     public function subscribe(Event $event)
///     {
///         $event->listen('UserLogin', [$this, 'onUserLogin']);
///         $event->listen('UserLogout', [$this, 'onUserLogout']);
///     }
/// }
/// ```
///
/// Rust 端实现 `Subscriber` trait：
/// ```ignore
/// struct UserEventSubscriber;
///
/// impl Subscriber for UserEventSubscriber {
///     fn subscribe(&self, dispatcher: &EventDispatcher) {
///         dispatcher.listen("UserLogin", Arc::new(ClosureListener::new(|_| Ok(Value::Null))), false);
///         dispatcher.listen("UserLogout", Arc::new(ClosureListener::new(|_| Ok(Value::Null))), false);
///     }
/// }
/// ```
pub trait Subscriber: Send + Sync {
    /// 订阅事件（对齐 PHP `Subscriber::subscribe(Event $event)`）
    fn subscribe(&self, dispatcher: &EventDispatcher);
}

/// 观察者 trait（对齐 PHP `observe` 智能订阅）
///
/// PHP `observe($observer)` 通过反射获取所有 `onXxx` 公开方法，
/// 自动注册为 `Xxx` 事件的监听器：
/// ```php
/// class UserObserver
/// {
///     public function onLogin($params) { ... }
///     public function onLogout($params) { ... }
/// }
/// // 自动注册 Login 和 Logout 事件
/// Event::observe(new UserObserver());
/// ```
///
/// Rust 端用 `Observer` trait 模拟（Rust 无反射，需手动声明事件映射）：
/// ```text
/// struct UserObserver;
///
/// impl Observer for UserObserver {
///     fn events(&self) -> &[(&str, Arc<dyn Listener>)] {
///         &[
///             ("Login", Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
///             ("Logout", Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
///         ]
///     }
/// }
/// ```
pub trait Observer: Send + Sync {
    /// 返回观察者监听的所有事件（对齐 PHP 反射 `onXxx` 方法 → `Xxx` 事件）
    ///
    /// 每个元素是 `(事件名, 监听器)`，等价 PHP `listen($prefix . $event, [$observer, 'on' . $event])`
    fn events(&self) -> Vec<(&'static str, Arc<dyn Listener>)>;
}

/// 事件错误类型
#[derive(Debug)]
pub enum EventError {
    /// 监听器执行失败
    ListenerError(String),
    /// 事件不存在
    EventNotFound(String),
    /// 参数错误
    InvalidParams(String),
}

impl std::fmt::Display for EventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventError::ListenerError(s) => write!(f, "Event listener error: {}", s),
            EventError::EventNotFound(s) => write!(f, "Event not found: {}", s),
            EventError::InvalidParams(s) => write!(f, "Invalid event params: {}", s),
        }
    }
}

impl std::error::Error for EventError {}

/// 事件分发器（对齐 PHP `think\Event`）
///
/// 对齐 PHP `think\Event` 类（272 行），提供事件监听/触发/订阅/观察者全 API。
///
/// ## 线程安全
///
/// 内部用 `Arc<RwLock<>>` 保护 `listener` 和 `bind` 映射，允许并发读写。
/// `Listener` 要求 `Send + Sync`，可在多线程环境中分发事件。
///
/// ## PHP 行为对齐
///
/// 1. **事件别名（bind）**：`listen('AppInit', ...)` 实际注册到 `event\AppInit::class`
/// 2. **优先执行（first）**：`listen(event, listener, true)` 用 `array_unshift` 插入队首
/// 3. **触发返回值**：`trigger` 返回所有监听器返回值数组；`once=true` 返回首个非 null
/// 4. **false 停止**：监听器返回 `false` 时停止后续监听器执行
/// 5. **once 停止**：`once=true` 时监听器返回非 null 值停止后续
/// 6. **点号通配**：`trigger('User.login')` 同时触发 `User.login` 和 `User.*` 监听器
/// 7. **array_unique**：`trigger` 对监听器列表去重（`SORT_REGULAR`）
pub struct EventDispatcher {
    /// 监听者映射：event => [listener1, listener2, ...]（对齐 PHP `$listener`）
    listener: RwLock<HashMap<String, Vec<Arc<dyn Listener>>>>,

    /// 事件别名映射：alias => real_event（对齐 PHP `$bind`）
    bind: RwLock<HashMap<String, String>>,
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl EventDispatcher {
    /// 创建新的事件分发器（对齐 PHP `__construct(App $app)`，Rust 无需 App 容器）
    pub fn new() -> Self {
        Self {
            listener: RwLock::new(HashMap::new()),
            bind: RwLock::new(HashMap::new()),
        }
    }

    /// 批量注册事件监听（对齐 PHP `listenEvents(array $events)`）
    ///
    /// PHP:
    /// ```php
    /// Event::listenEvents([
    ///     'UserLogin' => [LoginListener1::class, LoginListener2::class],
    ///     'UserLogout' => [LogoutListener::class],
    /// ]);
    /// ```
    ///
    /// Rust:
    /// ```ignore
    /// dispatcher.listen_events(vec![
    ///     ("UserLogin".to_string(), vec![Arc::new(LoginListener1), Arc::new(LoginListener2)]),
    ///     ("UserLogout".to_string(), vec![Arc::new(LogoutListener)]),
    /// ]);
    /// ```
    pub fn listen_events(&self, events: Vec<(String, Vec<Arc<dyn Listener>>)>) -> &Self {
        let mut listener_map = self.listener.write().expect("锁被毒化");
        let bind_map = self.bind.read().expect("锁被毒化");

        for (event, listeners) in events {
            // 应用事件别名（对齐 PHP `if (isset($this->bind[$event]))`）
            let event = bind_map.get(&event).cloned().unwrap_or(event);

            let entry = listener_map.entry(event).or_default();
            entry.extend(listeners);
        }

        self
    }

    /// 注册事件监听（对齐 PHP `listen(string $event, $listener, bool $first = false)`）
    ///
    /// PHP:
    /// ```php
    /// Event::listen('UserLogin', function($params) { ... });
    /// Event::listen('UserLogin', UserLoginListener::class);
    /// Event::listen('UserLogin', [UserLoginListener::class, 'handle'], true); // 优先执行
    /// ```
    ///
    /// Rust:
    /// ```ignore
    /// dispatcher.listen("UserLogin", Arc::new(ClosureListener::new(|_| Ok(Value::Null))), false);
    /// dispatcher.listen("UserLogin", Arc::new(UserLoginListener), true); // 优先执行
    /// ```
    ///
    /// **PHP 行为对齐**：
    /// - `first=true` 时插入队首（`array_unshift`）
    /// - `first=false` 时追加队尾（`$this->listener[$event][]`）
    /// - 应用事件别名（`bind` 映射）
    pub fn listen(&self, event: &str, listener: Arc<dyn Listener>, first: bool) -> &Self {
        let mut listener_map = self.listener.write().expect("锁被毒化");
        let bind_map = self.bind.read().expect("锁被毒化");

        // 应用事件别名（对齐 PHP `if (isset($this->bind[$event]))`）
        let event = bind_map
            .get(event)
            .cloned()
            .unwrap_or_else(|| event.to_string());

        let entry = listener_map.entry(event).or_default();
        if first {
            // 对齐 PHP `array_unshift($this->listener[$event], $listener)`
            entry.insert(0, listener);
        } else {
            // 对齐 PHP `$this->listener[$event][] = $listener`
            entry.push(listener);
        }

        self
    }

    /// 是否存在事件监听（对齐 PHP `hasListener(string $event): bool`）
    pub fn has_listener(&self, event: &str) -> bool {
        let listener_map = self.listener.read().expect("锁被毒化");
        let bind_map = self.bind.read().expect("锁被毒化");

        // 应用事件别名（对齐 PHP `if (isset($this->bind[$event]))`）
        let event = bind_map.get(event).map(|s| s.as_str()).unwrap_or(event);

        listener_map.contains_key(event)
    }

    /// 移除事件监听（对齐 PHP `remove(string $event): void`）
    pub fn remove(&self, event: &str) {
        let mut listener_map = self.listener.write().expect("锁被毒化");
        let bind_map = self.bind.read().expect("锁被毒化");

        // 应用事件别名（对齐 PHP `if (isset($this->bind[$event]))`）
        let event = bind_map
            .get(event)
            .cloned()
            .unwrap_or_else(|| event.to_string());

        // 对齐 PHP `unset($this->listener[$event])`
        listener_map.remove(&event);
    }

    /// 指定事件别名标识（对齐 PHP `bind(array $events)`）
    ///
    /// PHP:
    /// ```php
    /// Event::bind([
    ///     'UserLogin' => 'app\event\UserLogin',
    /// ]);
    /// ```
    ///
    /// Rust:
    /// ```ignore
    /// dispatcher.bind(vec![("UserLogin".to_string(), "app\\event\\UserLogin".to_string())]);
    /// ```
    pub fn bind(&self, events: Vec<(String, String)>) -> &Self {
        let mut bind_map = self.bind.write().expect("锁被毒化");
        for (alias, real_event) in events {
            bind_map.insert(alias, real_event);
        }
        self
    }

    /// 注册事件订阅者（对齐 PHP `subscribe($subscriber)`）
    ///
    /// PHP:
    /// ```php
    /// Event::subscribe(UserEventSubscriber::class);
    /// // 或
    /// Event::subscribe(new UserEventSubscriber());
    /// ```
    ///
    /// Rust:
    /// ```ignore
    /// dispatcher.subscribe(Arc::new(UserEventSubscriber));
    /// ```
    ///
    /// **PHP 行为对齐**：
    /// - 若订阅者有 `subscribe` 方法 → 手动订阅（调用 `$subscriber->subscribe($this)`）
    /// - 否则 → 智能订阅（调用 `observe($subscriber)`）
    ///
    /// Rust 端统一通过 `Subscriber` trait 的 `subscribe` 方法手动订阅。
    /// 若需智能订阅，用 `observe` 方法。
    pub fn subscribe(&self, subscriber: Arc<dyn Subscriber>) -> &Self {
        // 对齐 PHP `if (method_exists($subscriber, 'subscribe'))` → 手动订阅
        subscriber.subscribe(self);
        self
    }

    /// 自动注册事件观察者（对齐 PHP `observe($observer, string $prefix = '')`）
    ///
    /// PHP:
    /// ```php
    /// Event::observe(new UserObserver());
    /// // 自动注册 UserObserver 的 onLogin() → 'Login' 事件
    /// ```
    ///
    /// Rust:
    /// ```ignore
    /// dispatcher.observe(Arc::new(UserObserver), "");
    /// ```
    ///
    /// **PHP 行为对齐**：
    /// - 反射获取所有 `onXxx` 公开方法
    /// - 注册 `listen($prefix . $event_name, [$observer, 'on' . $event_name])`
    /// - 若有 `eventPrefix` 属性，用作前缀
    ///
    /// Rust 端通过 `Observer` trait 的 `events()` 方法声明事件映射，
    /// 避免运行时反射（Rust 无反射）。
    pub fn observe(&self, observer: Arc<dyn Observer>, prefix: &str) -> &Self {
        for (event, listener) in observer.events() {
            let full_event = if prefix.is_empty() {
                event.to_string()
            } else {
                format!("{}{}", prefix, event)
            };
            // 对齐 PHP `$this->listen($prefix . substr($name, 2), [$observer, $name])`
            self.listen(&full_event, listener, false);
        }
        self
    }

    /// 收集事件的监听器列表（私有助手，应用 bind 别名 + 点号通配 + 去重）
    ///
    /// 提取自 `trigger` 和 `trigger_spawn` 的公共逻辑，避免代码冗余。
    fn collect_listeners(&self, event: &str) -> Vec<Arc<dyn Listener>> {
        let bind_map = self.bind.read().expect("锁被毒化");

        // 应用事件别名（对齐 PHP `if (isset($this->bind[$event]))`）
        let event = bind_map
            .get(event)
            .cloned()
            .unwrap_or_else(|| event.to_string());

        drop(bind_map);

        let listener_map = self.listener.read().expect("锁被毒化");

        // 对齐 PHP `$listeners = $this->listener[$event] ?? []`
        let mut listeners: Vec<Arc<dyn Listener>> =
            listener_map.get(&event).cloned().unwrap_or_default();

        // 对齐 PHP 点号通配：`if (strpos($event, '.'))` → 触发 `prefix.*`
        if let Some(dot_pos) = event.find('.') {
            let prefix = &event[..dot_pos];
            let wildcard = format!("{}.*", prefix);
            if let Some(wildcard_listeners) = listener_map.get(&wildcard) {
                // 对齐 PHP `array_merge($listeners, $this->listener[$prefix . '.*'])`
                listeners.extend(wildcard_listeners.clone());
            }
        }

        drop(listener_map);

        // 对齐 PHP `array_unique($listeners, SORT_REGULAR)`
        // Rust 端用 Arc::ptr_eq 指针比较去重（等价 PHP 对象引用去重）
        let mut seen: Vec<Arc<dyn Listener>> = Vec::new();
        listeners.retain(|l| {
            if seen.iter().any(|s| Arc::ptr_eq(s, l)) {
                false
            } else {
                seen.push(l.clone());
                true
            }
        });

        listeners
    }

    /// 触发事件（对齐 PHP `trigger($event, $params = null, bool $once = false)`）
    ///
    /// PHP:
    /// ```php
    /// $results = Event::trigger('UserLogin', ['user_id' => 123]);
    /// $first = Event::trigger('UserLogin', ['user_id' => 123], true); // 只获取一个有效返回值
    /// ```
    ///
    /// Rust:
    /// ```ignore
    /// let results = dispatcher.trigger("UserLogin", &json!({"user_id": 123}), false).unwrap();
    /// let first = dispatcher.trigger("UserLogin", &json!({"user_id": 123}), true).unwrap();
    /// ```
    ///
    /// **PHP 行为对齐**：
    /// 1. 若 `$event` 是对象，取类名作为事件名，对象作为参数
    /// 2. 应用事件别名（`bind` 映射）
    /// 3. 点号通配：`User.login` 同时触发 `User.login` 和 `User.*`
    /// 4. `array_unique` 对监听器去重
    /// 5. 逐个调用 `dispatch`，返回值收集到 `$result`
    /// 6. 监听器返回 `false` → 停止后续
    /// 7. `once=true` 且监听器返回非 null → 停止后续
    /// 8. `once=false` 返回所有返回值数组；`once=true` 返回最后一个非 null 返回值
    pub fn trigger(
        &self,
        event: &str,
        params: &Value,
        once: bool,
    ) -> Result<Vec<Value>, EventError> {
        let listeners = self.collect_listeners(event);

        let mut results: Vec<Value> = Vec::new();

        for listener in &listeners {
            // 对齐 PHP `$result[$key] = $this->dispatch($listener, $params)`
            let result = listener.handle(params)?;
            results.push(result.clone());

            // 对齐 PHP `if (false === $result[$key] || (!is_null($result[$key]) && $once)) break`
            // PHP `false` 在 Rust 中用 `Value::Bool(false)` 表示
            if result == Value::Bool(false) {
                break;
            }
            // PHP `!is_null($result[$key]) && $once`：非 null 且 once 模式 → 停止
            if once && !result.is_null() {
                break;
            }
        }

        Ok(results)
    }

    /// 异步触发事件 — fire-and-forget（Rust 特有扩展，对齐 think-swoole 异步事件分发）
    ///
    /// 每个监听器在独立的 `tokio::task` 中并发执行，立即返回 `JoinHandle` 列表不等待。
    /// 适用于非关键事件（日志、指标、通知），不阻塞当前请求。
    ///
    /// **注意**：必须在 tokio 运行时中调用（axum 服务器已提供运行时）。
    /// 调用方可选择 `.await` JoinHandle 获取结果，或丢弃 JoinHandle 实现 fire-and-forget。
    ///
    /// **与同步 `trigger` 的差异**：
    /// - 同步 `trigger`：逐个串行执行，支持 `once`/`false` 停止
    /// - 异步 `trigger_spawn`：并发执行，不支持 `once`/`false` 停止（各监听器独立运行）
    /// - 两者的 bind 别名 / 点号通配 / 去重逻辑完全一致（共用 `collect_listeners`）
    ///
    /// Rust:
    /// ```ignore
    /// // fire-and-forget（丢弃 JoinHandle）
    /// dispatcher.trigger_spawn("UserLogin", &json!({"user_id": 123}));
    ///
    /// // 等待所有监听器完成
    /// let handles = dispatcher.trigger_spawn("UserLogin", &json!({"user_id": 123}));
    /// for handle in handles {
    ///     let _ = handle.await;
    /// }
    /// ```
    pub fn trigger_spawn(
        &self,
        event: &str,
        params: &Value,
    ) -> Vec<JoinHandle<Result<Value, EventError>>> {
        let listeners = self.collect_listeners(event);
        let params_owned = params.clone();

        listeners
            .into_iter()
            .map(|listener| {
                let params = params_owned.clone();
                tokio::spawn(async move { listener.handle(&params) })
            })
            .collect()
    }

    /// 异步触发事件并等待所有监听器完成（Rust 特有扩展，对齐 think-swoole 异步事件分发）
    ///
    /// 等价于 `trigger_spawn` + 逐个 `.await`，返回所有监听器的结果列表。
    /// 监听器错误被收集到 `Vec` 中（不传播），包括 task panic 产生的 `JoinError`。
    ///
    /// **注意**：必须在 tokio 运行时中调用。监听器并发执行，结果顺序与注册顺序一致
    /// （因为 `trigger_spawn` 返回的 JoinHandle 列表保持注册顺序）。
    ///
    /// Rust:
    /// ```ignore
    /// # async fn example() {
    /// let results = dispatcher.trigger_async("UserLogin", &json!({"user_id": 123})).await;
    /// for result in &results {
    ///     if let Err(e) = result {
    ///         eprintln!("Listener error: {}", e);
    ///     }
    /// }
    /// # }
    /// ```
    pub async fn trigger_async(
        &self,
        event: &str,
        params: &Value,
    ) -> Vec<Result<Value, EventError>> {
        let handles = self.trigger_spawn(event, params);
        let mut results = Vec::with_capacity(handles.len());

        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(join_err) => results.push(Err(EventError::ListenerError(format!(
                    "Task panicked: {}",
                    join_err
                )))),
            }
        }

        results
    }

    /// 触发事件（只获取一个有效返回值）（对齐 PHP `until($event, $params = null)`）
    ///
    /// 等价 `trigger(event, params, true)`，返回最后一个非 null 返回值。
    pub fn until(&self, event: &str, params: &Value) -> Result<Vec<Value>, EventError> {
        self.trigger(event, params, true)
    }

    /// 获取事件的所有监听器数量（PHP 无对应 API，Rust 扩展用于测试）
    pub fn listener_count(&self, event: &str) -> usize {
        let listener_map = self.listener.read().expect("锁被毒化");
        let bind_map = self.bind.read().expect("锁被毒化");

        let event = bind_map.get(event).map(|s| s.as_str()).unwrap_or(event);

        listener_map.get(event).map(|v| v.len()).unwrap_or(0)
    }
}

/// 事件 facade（对齐 PHP `think\facade\Event`）
///
/// PHP `think\facade\Event` 是静态外观，委托到容器中的 `think\Event` 实例：
/// ```php
/// Event::listen('UserLogin', function($params) { ... });
/// Event::trigger('UserLogin', ['user_id' => 123]);
/// ```
///
/// Rust 端用全局 `once_cell::sync::Lazy<EventDispatcher>` 模拟：
/// ```ignore
/// event::facade::listen("UserLogin", Arc::new(ClosureListener::new(|_| Ok(Value::Null))), false);
/// event::facade::trigger("UserLogin", &json!({"user_id": 123}), false).unwrap();
/// ```
pub mod facade {
    use super::*;
    use std::sync::OnceLock;

    /// 全局事件分发器单例（对齐 PHP 容器中的 `think\Event` 实例）
    static GLOBAL_DISPATCHER: OnceLock<EventDispatcher> = OnceLock::new();

    /// 获取全局事件分发器（对齐 PHP `Facade::getFacadeClass()` 返回 `'event'`）
    pub fn dispatcher() -> &'static EventDispatcher {
        GLOBAL_DISPATCHER.get_or_init(EventDispatcher::new)
    }

    /// 注册事件监听（对齐 PHP `Event::listen(...)`）
    pub fn listen(event: &str, listener: Arc<dyn Listener>, first: bool) {
        dispatcher().listen(event, listener, first);
    }

    /// 批量注册事件监听（对齐 PHP `Event::listenEvents(...)`）
    pub fn listen_events(events: Vec<(String, Vec<Arc<dyn Listener>>)>) {
        dispatcher().listen_events(events);
    }

    /// 是否存在事件监听（对齐 PHP `Event::hasListener(...)`）
    pub fn has_listener(event: &str) -> bool {
        dispatcher().has_listener(event)
    }

    /// 移除事件监听（对齐 PHP `Event::remove(...)`）
    pub fn remove(event: &str) {
        dispatcher().remove(event);
    }

    /// 指定事件别名（对齐 PHP `Event::bind(...)`）
    pub fn bind(events: Vec<(String, String)>) {
        dispatcher().bind(events);
    }

    /// 注册事件订阅者（对齐 PHP `Event::subscribe(...)`）
    pub fn subscribe(subscriber: Arc<dyn Subscriber>) {
        dispatcher().subscribe(subscriber);
    }

    /// 自动注册事件观察者（对齐 PHP `Event::observe(...)`）
    pub fn observe(observer: Arc<dyn Observer>, prefix: &str) {
        dispatcher().observe(observer, prefix);
    }

    /// 触发事件（对齐 PHP `Event::trigger(...)`）
    pub fn trigger(event: &str, params: &Value, once: bool) -> Result<Vec<Value>, EventError> {
        dispatcher().trigger(event, params, once)
    }

    /// 触发事件（只获取一个有效返回值）（对齐 PHP `Event::until(...)`）
    pub fn until(event: &str, params: &Value) -> Result<Vec<Value>, EventError> {
        dispatcher().until(event, params)
    }

    /// 异步触发事件 — fire-and-forget（Rust 特有扩展）
    pub fn trigger_spawn(
        event: &str,
        params: &Value,
    ) -> Vec<JoinHandle<Result<Value, EventError>>> {
        dispatcher().trigger_spawn(event, params)
    }

    /// 异步触发事件并等待所有监听器完成（Rust 特有扩展）
    pub async fn trigger_async(event: &str, params: &Value) -> Vec<Result<Value, EventError>> {
        dispatcher().trigger_async(event, params).await
    }

    /// 重置全局分发器（仅用于测试，PHP 无对应 API）
    ///
    /// **注意**：`OnceLock` 一旦初始化无法重置。测试中用独立 `EventDispatcher::new()` 实例
    /// 而非全局 facade，避免测试间状态污染。
    #[cfg(test)]
    pub fn _reset_for_test() {
        // OnceLock 无法重置，测试用独立实例
        // 此函数仅作为文档说明，实际不执行任何操作
    }
}

/// 触发事件助手函数（对齐 PHP `event($event, $args = null)`）
///
/// PHP `helper.php` 定义全局函数：
/// ```php
/// function event($event, $args = null)
/// {
///     return Event::trigger($event, $args);
/// }
/// ```
///
/// Rust 端用 `event_trigger` 函数模拟：
/// ```ignore
/// use sz_rust_core::event::event_trigger;
///
/// event_trigger("UserLogin", &json!({"user_id": 123}), false).unwrap();
/// ```
pub fn event_trigger(event: &str, params: &Value) -> Vec<Value> {
    facade::trigger(event, params, false).unwrap_or_default()
}

/// 异步触发事件助手函数（Rust 特有扩展，对齐 think-swoole 异步事件分发）
///
/// 等价于 `facade::trigger_async(event, params).await`，返回所有监听器的结果列表。
/// 监听器错误被收集到 `Vec` 中（不传播）。
///
/// Rust:
/// ```ignore
/// use sz_rust_core::event::event_trigger_async;
///
/// # async fn example() {
/// let results = event_trigger_async("UserLogin", &json!({"user_id": 123})).await;
/// # }
/// ```
pub async fn event_trigger_async(event: &str, params: &Value) -> Vec<Result<Value, EventError>> {
    facade::trigger_async(event, params).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ========================================================================
    // 测试组 40: EventDispatcher 基础 API（listen/has_listener/remove）
    // ========================================================================

    #[test]
    fn test_event_listen_and_has_listener() {
        let dispatcher = EventDispatcher::new();
        assert!(!dispatcher.has_listener("UserLogin"));

        dispatcher.listen(
            "UserLogin",
            Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
            false,
        );
        assert!(dispatcher.has_listener("UserLogin"));
    }

    #[test]
    fn test_event_remove_listener() {
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "UserLogin",
            Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
            false,
        );
        assert!(dispatcher.has_listener("UserLogin"));

        dispatcher.remove("UserLogin");
        assert!(!dispatcher.has_listener("UserLogin"));
    }

    #[test]
    fn test_event_listen_first_priority() {
        // 对齐 PHP `listen(event, listener, true)` → array_unshift 插入队首
        let dispatcher = EventDispatcher::new();
        let call_order = Arc::new(AtomicUsize::new(0));

        let order1 = call_order.clone();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(move |_| {
                order1.store(1, Ordering::SeqCst);
                Ok(Value::Null)
            })),
            false,
        );

        let order2 = call_order.clone();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(move |_| {
                order2.store(2, Ordering::SeqCst);
                Ok(Value::Null)
            })),
            true, // 插入队首，应先执行
        );

        dispatcher.trigger("Test", &Value::Null, false).unwrap();
        // order2 先执行（队首），order1 后执行（队尾）
        assert_eq!(call_order.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_event_listen_events_batch() {
        // 对齐 PHP `listenEvents(array $events)`
        let dispatcher = EventDispatcher::new();
        dispatcher.listen_events(vec![
            (
                "UserLogin".to_string(),
                vec![Arc::new(ClosureListener::new(|_| Ok(Value::Null)))],
            ),
            (
                "UserLogout".to_string(),
                vec![
                    Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
                    Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
                ],
            ),
        ]);
        assert!(dispatcher.has_listener("UserLogin"));
        assert_eq!(dispatcher.listener_count("UserLogout"), 2);
    }

    #[test]
    fn test_event_bind_alias() {
        // 对齐 PHP `bind(['AppInit' => 'event\AppInit::class'])`
        let dispatcher = EventDispatcher::new();
        dispatcher.bind(vec![(
            "AppInit".to_string(),
            "app\\event\\AppInit".to_string(),
        )]);

        dispatcher.listen(
            "AppInit",
            Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
            false,
        );
        // 实际注册到 "app\\event\\AppInit"
        assert!(!dispatcher.has_listener("AppInit_alias_check"));
        assert!(dispatcher.has_listener("app\\event\\AppInit"));
    }

    // ========================================================================
    // 测试组 41: EventDispatcher trigger 行为
    // ========================================================================

    #[test]
    fn test_event_trigger_returns_all_results() {
        // 对齐 PHP `trigger` 返回所有监听器返回值数组
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(1)))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(2)))),
            false,
        );

        let results = dispatcher.trigger("Test", &Value::Null, false).unwrap();
        assert_eq!(results, vec![json!(1), json!(2)]);
    }

    #[test]
    fn test_event_trigger_once_stops_at_non_null() {
        // 对齐 PHP `once=true`：监听器返回非 null 值停止后续
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(Value::Null))), // null 不停止
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!("stop")))), // 非 null 停止
            false,
        );
        let executed = Arc::new(AtomicUsize::new(0));
        let exec_clone = executed.clone();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(move |_| {
                exec_clone.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Null)
            })),
            false,
        );

        let results = dispatcher.trigger("Test", &Value::Null, true).unwrap();
        // once=true：第二个返回 "stop" 停止，第三个不执行
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], Value::Null);
        assert_eq!(results[1], json!("stop"));
        assert_eq!(executed.load(Ordering::SeqCst), 0); // 第三个未执行
    }

    #[test]
    fn test_event_trigger_false_stops_execution() {
        // 对齐 PHP `if (false === $result[$key]) break`
        let dispatcher = EventDispatcher::new();
        let executed = Arc::new(AtomicUsize::new(0));

        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(Value::Bool(false)))), // false 停止
            false,
        );
        let exec_clone = executed.clone();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(move |_| {
                exec_clone.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Null)
            })),
            false,
        );

        let results = dispatcher.trigger("Test", &Value::Null, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], Value::Bool(false));
        assert_eq!(executed.load(Ordering::SeqCst), 0); // 第二个未执行
    }

    #[test]
    fn test_event_trigger_empty_event() {
        // 无监听器的事件触发返回空数组
        let dispatcher = EventDispatcher::new();
        let results = dispatcher
            .trigger("Nonexistent", &Value::Null, false)
            .unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_event_trigger_passes_params() {
        // 对齐 PHP `trigger($event, $params)` 传递参数
        let dispatcher = EventDispatcher::new();
        let received = Arc::new(std::sync::Mutex::new(Value::Null));
        let recv_clone = received.clone();

        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(move |params| {
                *recv_clone.lock().unwrap() = params.clone();
                Ok(Value::Null)
            })),
            false,
        );

        let params = json!({"user_id": 123, "action": "login"});
        dispatcher.trigger("Test", &params, false).unwrap();

        assert_eq!(*received.lock().unwrap(), params);
    }

    // ========================================================================
    // 测试组 42: EventDispatcher 点号通配
    // ========================================================================

    #[test]
    fn test_event_dot_wildcard() {
        // 对齐 PHP `if (strpos($event, '.'))` → 触发 `prefix.*`
        let dispatcher = EventDispatcher::new();
        let executed = Arc::new(AtomicUsize::new(0));

        // 注册 User.login 和 User.* 监听器
        dispatcher.listen(
            "User.login",
            Arc::new(ClosureListener::new(|_| Ok(json!("specific")))),
            false,
        );
        let exec_clone = executed.clone();
        dispatcher.listen(
            "User.*",
            Arc::new(ClosureListener::new(move |_| {
                exec_clone.fetch_add(1, Ordering::SeqCst);
                Ok(json!("wildcard"))
            })),
            false,
        );

        // 触发 User.login 应同时触发 User.* 监听器
        let results = dispatcher
            .trigger("User.login", &Value::Null, false)
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], json!("specific"));
        assert_eq!(results[1], json!("wildcard"));
        assert_eq!(executed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_event_dot_wildcard_no_wildcard_listener() {
        // 无通配监听器时，点号事件只触发具体监听器
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "User.login",
            Arc::new(ClosureListener::new(|_| Ok(json!("specific")))),
            false,
        );

        let results = dispatcher
            .trigger("User.login", &Value::Null, false)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], json!("specific"));
    }

    // ========================================================================
    // 测试组 43: EventDispatcher 去重
    // ========================================================================

    #[test]
    fn test_event_dedup_same_listener_instance() {
        // 对齐 PHP `array_unique($listeners, SORT_REGULAR)`
        let dispatcher = EventDispatcher::new();
        let listener: Arc<dyn Listener> = Arc::new(ClosureListener::new(|_| Ok(json!(1))));

        // 同一 Arc 实例注册两次
        dispatcher.listen("Test", listener.clone(), false);
        dispatcher.listen("Test", listener.clone(), false);

        let results = dispatcher.trigger("Test", &Value::Null, false).unwrap();
        // 去重后只执行一次
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], json!(1));
    }

    #[test]
    fn test_event_no_dedup_different_listeners() {
        // 不同监听器实例不去重
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(1)))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(2)))),
            false,
        );

        let results = dispatcher.trigger("Test", &Value::Null, false).unwrap();
        assert_eq!(results.len(), 2);
    }

    // ========================================================================
    // 测试组 44: Subscriber 订阅者
    // ========================================================================

    struct TestSubscriber {
        login_count: Arc<AtomicUsize>,
    }

    impl Subscriber for TestSubscriber {
        fn subscribe(&self, dispatcher: &EventDispatcher) {
            let count = self.login_count.clone();
            dispatcher.listen(
                "UserLogin",
                Arc::new(ClosureListener::new(move |_| {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(Value::Null)
                })),
                false,
            );

            let count2 = self.login_count.clone();
            dispatcher.listen(
                "UserLogout",
                Arc::new(ClosureListener::new(move |_| {
                    count2.fetch_add(10, Ordering::SeqCst);
                    Ok(Value::Null)
                })),
                false,
            );
        }
    }

    #[test]
    fn test_event_subscriber_registers_multiple_listeners() {
        // 对齐 PHP `subscribe($subscriber)` → 调用 `$subscriber->subscribe($this)`
        let dispatcher = EventDispatcher::new();
        let login_count = Arc::new(AtomicUsize::new(0));

        let subscriber = Arc::new(TestSubscriber {
            login_count: login_count.clone(),
        });
        dispatcher.subscribe(subscriber);

        assert!(dispatcher.has_listener("UserLogin"));
        assert!(dispatcher.has_listener("UserLogout"));

        dispatcher
            .trigger("UserLogin", &Value::Null, false)
            .unwrap();
        assert_eq!(login_count.load(Ordering::SeqCst), 1);

        dispatcher
            .trigger("UserLogout", &Value::Null, false)
            .unwrap();
        assert_eq!(login_count.load(Ordering::SeqCst), 11); // 1 + 10
    }

    // ========================================================================
    // 测试组 45: Observer 观察者
    // ========================================================================

    struct TestObserver {
        counter: Arc<AtomicUsize>,
    }

    impl Observer for TestObserver {
        fn events(&self) -> Vec<(&'static str, Arc<dyn Listener>)> {
            let c1 = self.counter.clone();
            let c2 = self.counter.clone();
            vec![
                (
                    "Login",
                    Arc::new(ClosureListener::new(move |_| {
                        c1.fetch_add(1, Ordering::SeqCst);
                        Ok(Value::Null)
                    })),
                ),
                (
                    "Logout",
                    Arc::new(ClosureListener::new(move |_| {
                        c2.fetch_add(100, Ordering::SeqCst);
                        Ok(Value::Null)
                    })),
                ),
            ]
        }
    }

    #[test]
    fn test_event_observer_auto_registers() {
        // 对齐 PHP `observe($observer)` → 反射 onXxx 方法自动注册
        let dispatcher = EventDispatcher::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let observer = Arc::new(TestObserver {
            counter: counter.clone(),
        });
        dispatcher.observe(observer, "");

        assert!(dispatcher.has_listener("Login"));
        assert!(dispatcher.has_listener("Logout"));

        dispatcher.trigger("Login", &Value::Null, false).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        dispatcher.trigger("Logout", &Value::Null, false).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 101);
    }

    #[test]
    fn test_event_observer_with_prefix() {
        // 对齐 PHP `observe($observer, 'User')` → 注册 `UserLogin` 事件
        let dispatcher = EventDispatcher::new();
        let counter = Arc::new(AtomicUsize::new(0));

        let observer = Arc::new(TestObserver {
            counter: counter.clone(),
        });
        dispatcher.observe(observer, "User");

        assert!(dispatcher.has_listener("UserLogin"));
        assert!(dispatcher.has_listener("UserLogout"));

        dispatcher
            .trigger("UserLogin", &Value::Null, false)
            .unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    // ========================================================================
    // 测试组 46: until 方法
    // ========================================================================

    #[test]
    fn test_event_until_returns_first_non_null() {
        // 对齐 PHP `until($event, $params)` = `trigger($event, $params, true)`
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!("first_valid")))),
            false,
        );

        let results = dispatcher.until("Test", &Value::Null).unwrap();
        // until=true：第一个 null 不停止，第二个 "first_valid" 停止
        assert_eq!(results.len(), 2);
        assert_eq!(results[1], json!("first_valid"));
    }

    // ========================================================================
    // 测试组 47: ClosureListener
    // ========================================================================

    #[test]
    fn test_closure_listener_executes_closure() {
        let listener = ClosureListener::new(|params| {
            assert_eq!(params, &json!({"key": "value"}));
            Ok(json!("result"))
        });

        let result = listener.handle(&json!({"key": "value"})).unwrap();
        assert_eq!(result, json!("result"));
    }

    #[test]
    fn test_closure_listener_returns_null() {
        let listener = ClosureListener::new(|_| Ok(Value::Null));
        let result = listener.handle(&Value::Null).unwrap();
        assert!(result.is_null());
    }

    // ========================================================================
    // 测试组 48: 自定义 Listener trait 实现
    // ========================================================================

    struct CustomListener {
        id: i32,
    }

    impl Listener for CustomListener {
        fn handle(&self, _params: &Value) -> Result<Value, EventError> {
            Ok(json!({"listener_id": self.id}))
        }
    }

    #[test]
    fn test_custom_listener_trait_impl() {
        let dispatcher = EventDispatcher::new();
        dispatcher.listen("Test", Arc::new(CustomListener { id: 42 }), false);

        let results = dispatcher.trigger("Test", &Value::Null, false).unwrap();
        assert_eq!(results, vec![json!({"listener_id": 42})]);
    }

    #[test]
    fn test_listener_error_propagates() {
        // 对齐 PHP 监听器抛出异常 → 停止后续执行
        let dispatcher = EventDispatcher::new();
        let executed = Arc::new(AtomicUsize::new(0));

        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| {
                Err(EventError::ListenerError("test error".to_string()))
            })),
            false,
        );
        let exec_clone = executed.clone();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(move |_| {
                exec_clone.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Null)
            })),
            false,
        );

        let result = dispatcher.trigger("Test", &Value::Null, false);
        assert!(result.is_err());
        assert_eq!(executed.load(Ordering::SeqCst), 0); // 第二个未执行
    }

    // ========================================================================
    // 测试组 49: R5 PHP 行为对齐
    // ========================================================================

    #[test]
    fn test_r5_php_event_listen_then_trigger() {
        // R5: 对齐 PHP Event::listen + Event::trigger
        let dispatcher = EventDispatcher::new();
        let received = Arc::new(std::sync::Mutex::new(Value::Null));
        let recv_clone = received.clone();

        dispatcher.listen(
            "UserLogin",
            Arc::new(ClosureListener::new(move |params| {
                *recv_clone.lock().unwrap() = params.clone();
                Ok(Value::Null)
            })),
            false,
        );

        let params = json!({"user_id": 123, "username": "alice"});
        dispatcher.trigger("UserLogin", &params, false).unwrap();

        assert_eq!(*received.lock().unwrap(), params);
    }

    #[test]
    fn test_r5_php_event_bind_alias_resolution() {
        // R5: 对齐 PHP Event::bind 别名解析
        let dispatcher = EventDispatcher::new();
        dispatcher.bind(vec![(
            "AppInit".to_string(),
            "think\\event\\AppInit".to_string(),
        )]);

        dispatcher.listen(
            "AppInit",
            Arc::new(ClosureListener::new(|_| Ok(json!("init_called")))),
            false,
        );

        // 实际注册到 "think\\event\\AppInit"
        assert!(dispatcher.has_listener("think\\event\\AppInit"));
        assert_eq!(dispatcher.listener_count("think\\event\\AppInit"), 1);

        let results = dispatcher.trigger("AppInit", &Value::Null, false).unwrap();
        assert_eq!(results, vec![json!("init_called")]);
    }

    #[test]
    fn test_r5_php_event_first_array_unshift() {
        // R5: 对齐 PHP `array_unshift` — first=true 插入队首
        let dispatcher = EventDispatcher::new();
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        let o1 = order.clone();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(move |_| {
                o1.lock().unwrap().push(1);
                Ok(Value::Null)
            })),
            false,
        );

        let o2 = order.clone();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(move |_| {
                o2.lock().unwrap().push(2);
                Ok(Value::Null)
            })),
            true, // 插入队首
        );

        dispatcher.trigger("Test", &Value::Null, false).unwrap();
        // o2 先执行（队首），o1 后执行（队尾）
        assert_eq!(*order.lock().unwrap(), vec![2, 1]);
    }

    #[test]
    fn test_r5_php_event_trigger_returns_array() {
        // R5: 对齐 PHP `trigger` 返回所有返回值数组
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(1)))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(2)))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(3)))),
            false,
        );

        let results = dispatcher.trigger("Test", &Value::Null, false).unwrap();
        assert_eq!(results, vec![json!(1), json!(2), json!(3)]);
    }

    #[test]
    fn test_r5_php_event_until_returns_last_non_null() {
        // R5: 对齐 PHP `until` = `trigger(once=true)` 返回最后一个非 null
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!("first")))),
            false,
        );

        let results = dispatcher.until("Test", &Value::Null).unwrap();
        // 第一个 null 不停止，第二个 "first" 非空停止
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_r5_php_event_false_stops() {
        // R5: 对齐 PHP `if (false === $result[$key]) break`
        let dispatcher = EventDispatcher::new();
        let executed = Arc::new(AtomicUsize::new(0));

        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(Value::Bool(false)))),
            false,
        );
        let exec_clone = executed.clone();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(move |_| {
                exec_clone.fetch_add(1, Ordering::SeqCst);
                Ok(Value::Null)
            })),
            false,
        );

        let results = dispatcher.trigger("Test", &Value::Null, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], Value::Bool(false));
    }

    #[test]
    fn test_r5_php_event_dot_wildcard_merge() {
        // R5: 对齐 PHP `array_merge($listeners, $this->listener[$prefix . '.*'])`
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "User.login",
            Arc::new(ClosureListener::new(|_| Ok(json!("specific")))),
            false,
        );
        dispatcher.listen(
            "User.*",
            Arc::new(ClosureListener::new(|_| Ok(json!("wildcard")))),
            false,
        );

        let results = dispatcher
            .trigger("User.login", &Value::Null, false)
            .unwrap();
        assert_eq!(results, vec![json!("specific"), json!("wildcard")]);
    }

    #[test]
    fn test_r5_php_event_array_unique_dedup() {
        // R5: 对齐 PHP `array_unique($listeners, SORT_REGULAR)`
        let dispatcher = EventDispatcher::new();
        let listener: Arc<dyn Listener> = Arc::new(ClosureListener::new(|_| Ok(json!(1))));

        dispatcher.listen("Test", listener.clone(), false);
        dispatcher.listen("Test", listener.clone(), false);
        dispatcher.listen("Test", listener.clone(), false);

        let results = dispatcher.trigger("Test", &Value::Null, false).unwrap();
        // 同一实例去重，只执行一次
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_r5_php_event_remove_clears_listeners() {
        // R5: 对齐 PHP `unset($this->listener[$event])`
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(1)))),
            false,
        );
        assert!(dispatcher.has_listener("Test"));

        dispatcher.remove("Test");
        assert!(!dispatcher.has_listener("Test"));

        let results = dispatcher.trigger("Test", &Value::Null, false).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_r5_php_event_has_listener_with_bind() {
        // R5: 对齐 PHP `hasListener` 应用 bind 别名
        let dispatcher = EventDispatcher::new();
        dispatcher.bind(vec![(
            "AppInit".to_string(),
            "app\\event\\AppInit".to_string(),
        )]);
        dispatcher.listen(
            "AppInit",
            Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
            false,
        );

        // 查询别名应返回 true（PHP `hasListener('AppInit')` 解析到 `app\\event\\AppInit`）
        assert!(dispatcher.has_listener("AppInit"));
        assert!(dispatcher.has_listener("app\\event\\AppInit"));
    }

    #[test]
    fn test_r5_php_event_listen_events_batch_merge() {
        // R5: 对齐 PHP `listenEvents` 的 `array_merge`
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(1)))),
            false,
        );
        dispatcher.listen_events(vec![(
            "Test".to_string(),
            vec![
                Arc::new(ClosureListener::new(|_| Ok(json!(2)))),
                Arc::new(ClosureListener::new(|_| Ok(json!(3)))),
            ],
        )]);

        let results = dispatcher.trigger("Test", &Value::Null, false).unwrap();
        // array_merge：原有 [1] + 新增 [2, 3]
        assert_eq!(results, vec![json!(1), json!(2), json!(3)]);
    }

    // ========================================================================
    // 测试组 50: trigger_spawn 基础（异步 fire-and-forget 分发）
    // ========================================================================

    #[tokio::test]
    async fn test_trigger_spawn_returns_join_handles() {
        // trigger_spawn 返回 Vec<JoinHandle>，长度等于监听器数量
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(1)))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(2)))),
            false,
        );

        let handles = dispatcher.trigger_spawn("Test", &Value::Null);
        assert_eq!(handles.len(), 2);

        // 等待所有完成
        for handle in handles {
            let result = handle.await.unwrap().unwrap();
            assert!(result == json!(1) || result == json!(2));
        }
    }

    #[tokio::test]
    async fn test_trigger_spawn_empty_event() {
        // 无监听器的事件返回空 Vec
        let dispatcher = EventDispatcher::new();
        let handles = dispatcher.trigger_spawn("Nonexistent", &Value::Null);
        assert!(handles.is_empty());
    }

    #[tokio::test]
    async fn test_trigger_spawn_passes_params() {
        // 参数透传到异步监听器
        let dispatcher = EventDispatcher::new();
        let received = Arc::new(std::sync::Mutex::new(Value::Null));
        let recv_clone = received.clone();

        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(move |params| {
                *recv_clone.lock().unwrap() = params.clone();
                Ok(Value::Null)
            })),
            false,
        );

        let params = json!({"user_id": 123, "action": "login"});
        let handles = dispatcher.trigger_spawn("Test", &params);
        for handle in handles {
            let _ = handle.await;
        }

        assert_eq!(*received.lock().unwrap(), params);
    }

    #[tokio::test]
    async fn test_trigger_spawn_all_listeners_execute() {
        // 所有监听器在异步模式都执行
        let dispatcher = EventDispatcher::new();
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..5 {
            let c = counter.clone();
            dispatcher.listen(
                "Test",
                Arc::new(ClosureListener::new(move |_| {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(Value::Null)
                })),
                false,
            );
        }

        let handles = dispatcher.trigger_spawn("Test", &Value::Null);
        for handle in handles {
            let _ = handle.await;
        }

        assert_eq!(counter.load(Ordering::SeqCst), 5);
    }

    // ========================================================================
    // 测试组 51: trigger_async（异步分发 + 等待所有完成）
    // ========================================================================

    #[tokio::test]
    async fn test_trigger_async_awaits_all() {
        // trigger_async 等待所有监听器完成，返回正确数量的结果
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(1)))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(2)))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(3)))),
            false,
        );

        let results = dispatcher.trigger_async("Test", &Value::Null).await;
        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.is_ok()));
    }

    #[tokio::test]
    async fn test_trigger_async_collects_results_in_order() {
        // 结果顺序与监听器注册顺序一致（JoinHandle 列表保持注册顺序）
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!("first")))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!("second")))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!("third")))),
            false,
        );

        let results = dispatcher.trigger_async("Test", &Value::Null).await;
        assert_eq!(results[0].as_ref().unwrap(), &json!("first"));
        assert_eq!(results[1].as_ref().unwrap(), &json!("second"));
        assert_eq!(results[2].as_ref().unwrap(), &json!("third"));
    }

    #[tokio::test]
    async fn test_trigger_async_handles_errors() {
        // 监听器错误被收集到 Vec 中，不传播
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!("ok")))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| {
                Err(EventError::ListenerError("test error".to_string()))
            })),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!("ok2")))),
            false,
        );

        let results = dispatcher.trigger_async("Test", &Value::Null).await;
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(results[2].is_ok());
    }

    #[tokio::test]
    async fn test_trigger_async_handles_panic() {
        // 监听器 panic 被 tokio 捕获为 JoinError，trigger_async 转为 EventError
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!("ok")))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| panic!("test panic"))),
            false,
        );

        let results = dispatcher.trigger_async("Test", &Value::Null).await;
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(results[1].is_err()); // panic → JoinError → EventError
    }

    // ========================================================================
    // 测试组 52: 异步分发行为对齐（bind 别名 / 点号通配 / 去重）
    // ========================================================================

    #[tokio::test]
    async fn test_trigger_spawn_applies_bind_alias() {
        // 异步模式应用 bind 别名
        let dispatcher = EventDispatcher::new();
        dispatcher.bind(vec![(
            "AppInit".to_string(),
            "app\\event\\AppInit".to_string(),
        )]);
        dispatcher.listen(
            "AppInit",
            Arc::new(ClosureListener::new(|_| Ok(json!("init_called")))),
            false,
        );

        let handles = dispatcher.trigger_spawn("AppInit", &Value::Null);
        assert_eq!(handles.len(), 1);

        let result = handles.into_iter().next().unwrap().await.unwrap().unwrap();
        assert_eq!(result, json!("init_called"));
    }

    #[tokio::test]
    async fn test_trigger_spawn_applies_dot_wildcard() {
        // 异步模式应用点号通配
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "User.login",
            Arc::new(ClosureListener::new(|_| Ok(json!("specific")))),
            false,
        );
        dispatcher.listen(
            "User.*",
            Arc::new(ClosureListener::new(|_| Ok(json!("wildcard")))),
            false,
        );

        let handles = dispatcher.trigger_spawn("User.login", &Value::Null);
        assert_eq!(handles.len(), 2); // specific + wildcard

        let results = dispatcher.trigger_async("User.login", &Value::Null).await;
        assert_eq!(results[0].as_ref().unwrap(), &json!("specific"));
        assert_eq!(results[1].as_ref().unwrap(), &json!("wildcard"));
    }

    #[tokio::test]
    async fn test_trigger_spawn_deduplicates() {
        // 异步模式应用去重
        let dispatcher = EventDispatcher::new();
        let listener: Arc<dyn Listener> = Arc::new(ClosureListener::new(|_| Ok(json!(1))));

        dispatcher.listen("Test", listener.clone(), false);
        dispatcher.listen("Test", listener.clone(), false);
        dispatcher.listen("Test", listener.clone(), false);

        let handles = dispatcher.trigger_spawn("Test", &Value::Null);
        assert_eq!(handles.len(), 1); // 去重后只剩 1 个
    }

    #[tokio::test]
    async fn test_trigger_spawn_correct_handle_count() {
        // 异步模式监听器数量正确（含 first 优先级插入）
        let dispatcher = EventDispatcher::new();
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(1)))),
            false,
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(2)))),
            true, // 插入队首
        );
        dispatcher.listen(
            "Test",
            Arc::new(ClosureListener::new(|_| Ok(json!(3)))),
            false,
        );

        let handles = dispatcher.trigger_spawn("Test", &Value::Null);
        assert_eq!(handles.len(), 3);

        // 验证顺序：first 插入队首 → [2, 1, 3]
        let results = dispatcher.trigger_async("Test", &Value::Null).await;
        assert_eq!(results[0].as_ref().unwrap(), &json!(2));
        assert_eq!(results[1].as_ref().unwrap(), &json!(1));
        assert_eq!(results[2].as_ref().unwrap(), &json!(3));
    }

    // ========================================================================
    // 测试组 53: facade + helper 异步
    // ========================================================================

    #[tokio::test]
    async fn test_facade_trigger_spawn() {
        // facade::trigger_spawn 异步分发
        facade::listen(
            "FacadeTest",
            Arc::new(ClosureListener::new(|_| Ok(json!("facade_spawn")))),
            false,
        );

        let handles = facade::trigger_spawn("FacadeTest", &Value::Null);
        assert_eq!(handles.len(), 1);

        let result = handles.into_iter().next().unwrap().await.unwrap().unwrap();
        assert_eq!(result, json!("facade_spawn"));
    }

    #[tokio::test]
    async fn test_event_trigger_async_helper() {
        // event_trigger_async 助手函数异步分发
        facade::listen(
            "HelperTest",
            Arc::new(ClosureListener::new(|_| Ok(json!("helper_async")))),
            false,
        );

        let results = event_trigger_async("HelperTest", &Value::Null).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().unwrap(), &json!("helper_async"));
    }
}
