//! App 容器 — DB/Cache/Log 单例 + DI 服务容器
//!
//! 对齐 PHP `app()` 容器，持有全局配置、单例和服务绑定。
//!
//! ## 设计
//!
//! - 基于 `OnceCell` 实现全局单例（线程安全，初始化一次后只读）
//! - 持有 `AppConfig` + 5 个 DB 连接配置 + Cache/Log 占位
//! - 后续阶段：接入 SZ-ORM `Pool`，替换 `DatabaseConnection` 为真正的连接池
//! - 接入 Cache facade
//! - 接入日志系统
//! - DI阶段：接入服务容器（`bind`/`singleton`/`make`，对齐 PHP `app()->bind/make/singleton`）
//!
//! ## PHP 对齐
//!
//! ```php
//! // PHP 中的 app() 容器
//! $app = app();
//! $db = $app->db;  // 数据库连接
//! $cache = $app->cache;  // 缓存
//! $log = $app->log;  // 日志
//!
//! // 服务绑定与解析（DI）
//! app()->bind('cache', fn() => new MemoryCache());
//! app()->singleton('db', fn() => Db::connect());
//! $cache = app()->make('cache');
//! ```
//!
//! ## Rust DI 设计
//!
//! Rust 中用 `TypeId` 替代 PHP 字符串 key，实现**类型安全**的服务解析：
//!
//! ```rust,ignore
//! use sz_rust_core::container::App;
//!
//! // 注册单例（整个应用生命周期内只创建一次）
//! App::with(|app| {
//!     app.singleton(|| MyService::new());
//! });
//!
//! // 解析服务（类型安全，无需 downcast 字符串 key）
//! let svc = App::global().unwrap().make::<MyService>();
//! ```

use crate::config::{AppConfig, DatabaseConnection};
use parking_lot::RwLock;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

/// 服务实例类型别名（消除 `clippy::type_complexity` 警告）
type ServiceInstance = Arc<dyn Any + Send + Sync>;
/// 作用域实例缓存类型别名（消除 `clippy::type_complexity` 警告）
type ScopeInstances = HashMap<TypeId, ServiceInstance>;

/// 全局 App 容器单例
static APP: OnceLock<App> = OnceLock::new();

// ============================================================================
// DI 服务容器（对齐 PHP app()->bind/make/singleton）
// ============================================================================

/// 服务生命周期
///
/// 对齐 PHP `app()->bind()`（瞬态）、`app()->singleton()`（单例）、
/// `app()->scoped()`（请求作用域）三种语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// 单例：整个应用生命周期内只创建一次，后续 `make` 返回同一实例
    Singleton,
    /// 瞬态：每次 `make` 都调用工厂创建新实例
    Transient,
    /// 请求作用域：同一 `ScopeId` 内单例，不同 `ScopeId` 各自独立实例
    ///
    /// 对齐 PHP `app()->scoped()`。在 Rust 中，`ScopeId` 通常由 Web 框架
    /// 在请求开始时生成（如 axum 中间件生成 UUID 的低 64 位），请求结束时
    /// 调用 [`Container::clear_scope`] 清理。
    Scoped,
}

/// 请求作用域 ID
///
/// 用于 [`Container::make_with_scope`] 区分不同请求的作用域实例。
/// 同一 `ScopeId` 内的 `make_with_scope` 调用返回同一实例。
pub type ScopeId = u64;

/// 服务工厂函数类型
///
/// 返回 `Box<dyn Any + Send + Sync>` 以支持任意类型的服务实例。
type ServiceFactory = Arc<dyn Fn() -> Box<dyn Any + Send + Sync> + Send + Sync>;

/// 服务绑定（工厂 + 生命周期）
///
/// `Clone` 用于在 `make` 中将绑定从读锁作用域复制出来后再调用工厂，
/// 避免在持锁状态下调用用户代码（可能引发死锁或重入）。
#[derive(Clone)]
struct ServiceBinding {
    /// 工厂函数（创建服务实例）
    factory: ServiceFactory,
    /// 生命周期策略
    lifetime: Lifetime,
}

/// DI 服务容器 — 服务注册/解析/生命周期管理
///
/// 对齐 PHP `app()->bind()/make()/singleton()/instance()/scoped()/alias()`。
/// 使用 `TypeId` 作为 key 实现类型安全的服务解析，避免 PHP 字符串 key
/// 的类型不匹配风险。
///
/// ## 线程安全
///
/// - `bindings`、`instances`、`scoped_instances`、`aliases` 均使用 `RwLock` 保护
/// - 单例实例以 `Arc` 返回，可跨线程共享
pub struct Container {
    /// 服务绑定表（TypeId → 工厂 + 生命周期）
    bindings: RwLock<HashMap<TypeId, ServiceBinding>>,
    /// 单例实例缓存（TypeId → 已创建实例）
    instances: RwLock<HashMap<TypeId, ServiceInstance>>,
    /// 请求作用域实例缓存（ScopeId → (TypeId → 实例)）
    ///
    /// 对齐 PHP `app()->scoped()`。每个 ScopeId 相当于一个"请求作用域"，
    /// 同一作用域内首次 `make_with_scope` 调用工厂创建并缓存，后续直接返回缓存。
    scoped_instances: RwLock<HashMap<ScopeId, ScopeInstances>>,
    /// 字符串别名表（alias → TypeId）
    ///
    /// 对齐 PHP `app()->alias('name', Service::class)`。
    /// 仅用于调试输出和 `resolve_alias` 反向查找；解析时仍用类型安全的 `make::<T>()`。
    aliases: RwLock<HashMap<String, TypeId>>,
}

impl Container {
    /// 创建空的服务容器
    pub fn new() -> Self {
        Self {
            bindings: RwLock::new(HashMap::new()),
            instances: RwLock::new(HashMap::new()),
            scoped_instances: RwLock::new(HashMap::new()),
            aliases: RwLock::new(HashMap::new()),
        }
    }

    /// 注册瞬态服务（每次 `make` 创建新实例）
    ///
    /// 对齐 PHP `app()->bind('key', fn() => new Service())`。
    ///
    /// # 类型约束
    ///
    /// - `T: Send + Sync + 'static`：服务实例必须线程安全
    /// - `F: Fn() -> T + Send + Sync + 'static`：工厂必须线程安全
    pub fn bind<T, F>(&self, factory: F)
    where
        T: Send + Sync + 'static,
        F: Fn() -> T + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();
        let binding = ServiceBinding {
            factory: Arc::new(move || Box::new(factory())),
            lifetime: Lifetime::Transient,
        };
        self.bindings.write().insert(type_id, binding);
    }

    /// 注册单例服务（整个应用生命周期内只创建一次）
    ///
    /// 对齐 PHP `app()->singleton('key', fn() => new Service())`。
    ///
    /// 首次 `make` 时调用工厂创建实例并缓存，后续 `make` 返回缓存的同一实例。
    pub fn singleton<T, F>(&self, factory: F)
    where
        T: Send + Sync + 'static,
        F: Fn() -> T + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();
        let binding = ServiceBinding {
            factory: Arc::new(move || Box::new(factory())),
            lifetime: Lifetime::Singleton,
        };
        self.bindings.write().insert(type_id, binding);
    }

    /// 注册请求作用域服务（同一 `ScopeId` 内单例）
    ///
    /// 对齐 PHP `app()->scoped('key', fn() => new Service())`。
    ///
    /// 与 `singleton` 不同：scoped 服务在 [`Container::make_with_scope`] 调用时，
    /// 同一 `scope_id` 内首次调用工厂创建并缓存，后续返回缓存；
    /// 不同 `scope_id` 各自创建独立实例；请求结束后调用
    /// [`Container::clear_scope`] 清理对应作用域的缓存。
    ///
    /// # 用法
    ///
    /// ```ignore
    /// use sz_rust_core::container::{Container, ScopeId};
    ///
    /// let container = Container::new();
    /// container.scoped(|| RequestCache::new());
    ///
    /// // 请求 A（scope_id=1）
    /// let cache_a1 = container.make_with_scope::<RequestCache>(1).unwrap();
    /// let cache_a2 = container.make_with_scope::<RequestCache>(1).unwrap();
    /// assert!(Arc::ptr_eq(&cache_a1, &cache_a2)); // 同一作用域：同一实例
    ///
    /// // 请求 B（scope_id=2）
    /// let cache_b = container.make_with_scope::<RequestCache>(2).unwrap();
    /// assert!(!Arc::ptr_eq(&cache_a1, &cache_b)); // 不同作用域：不同实例
    ///
    /// // 请求 A 结束
    /// container.clear_scope(1);
    /// ```
    pub fn scoped<T, F>(&self, factory: F)
    where
        T: Send + Sync + 'static,
        F: Fn() -> T + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();
        let binding = ServiceBinding {
            factory: Arc::new(move || Box::new(factory())),
            lifetime: Lifetime::Scoped,
        };
        self.bindings.write().insert(type_id, binding);
    }

    /// 直接绑定已创建的实例（绕过工厂）
    ///
    /// 对齐 PHP `app()->instance('key', $obj)`。
    ///
    /// 将一个已创建的实例直接注册为单例，后续 `make` 返回此实例。
    /// 适用于：
    /// - 实例已在其他地方创建（如配置加载时初始化的服务）
    /// - 实例创建过程复杂、不适合用闭包表达
    /// - 测试中注入 mock 实例
    ///
    /// # 用法
    ///
    /// ```ignore
    /// use sz_rust_core::container::Container;
    ///
    /// let container = Container::new();
    /// let logger = Arc::new(FileLogger::new("/var/log/app.log"));
    /// container.instance(logger.clone());
    ///
    /// let resolved = container.make::<FileLogger>().unwrap();
    /// assert!(Arc::ptr_eq(&logger, &resolved));
    /// ```
    pub fn instance<T>(&self, instance: T)
    where
        T: Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();
        let arc: Arc<dyn Any + Send + Sync> = Arc::new(instance);
        // 1. 缓存实例（make 会优先检查 instances 缓存）
        self.instances.write().insert(type_id, arc);
        // 2. 注册占位绑定（使 has() 返回 true）
        // 注：factory 不会被调用，因为 make 会先命中 instances 缓存。
        // 使用 unreachable 闭包表达此不变量；若被调用则说明内部状态被破坏。
        self.bindings.write().insert(
            type_id,
            ServiceBinding {
                factory: Arc::new(|| {
                    panic!("instance() 绑定的服务不应调用工厂 — 这是内部不变量违反")
                }),
                lifetime: Lifetime::Singleton,
            },
        );
    }

    /// 为服务类型注册字符串别名
    ///
    /// 对齐 PHP `app()->alias('name', Service::class)`。
    ///
    /// 别名仅用于：
    /// - 调试输出（[`Container::debug_aliases`] 列出所有别名）
    /// - 反向查找（[`Container::resolve_alias`] 通过别名获取 TypeId）
    ///
    /// 解析时仍用类型安全的 `make::<T>()`，不支持通过字符串别名解析
    /// （Rust 类型系统要求编译时已知类型，字符串 key 解析会引入不安全的 downcast）。
    ///
    /// # 用法
    ///
    /// ```ignore
    /// use sz_rust_core::container::Container;
    ///
    /// let container = Container::new();
    /// container.singleton(|| MyService::new());
    /// container.alias::<MyService>("my_service");
    ///
    /// assert!(container.is_alias("my_service"));
    /// let type_id = container.resolve_alias("my_service").unwrap();
    /// assert_eq!(type_id, std::any::TypeId::of::<MyService>());
    /// ```
    pub fn alias<T: 'static>(&self, name: impl Into<String>) {
        let type_id = TypeId::of::<T>();
        self.aliases.write().insert(name.into(), type_id);
    }

    /// 通过别名查找对应的 TypeId
    ///
    /// 返回 `None` 表示别名未注册。
    pub fn resolve_alias(&self, name: &str) -> Option<TypeId> {
        self.aliases.read().get(name).copied()
    }

    /// 检查指定别名是否已注册
    pub fn is_alias(&self, name: &str) -> bool {
        self.aliases.read().contains_key(name)
    }

    /// 列出所有已注册别名（用于调试）
    pub fn debug_aliases(&self) -> Vec<String> {
        self.aliases.read().keys().cloned().collect()
    }

    /// 解析服务实例（无作用域）
    ///
    /// 对齐 PHP `app()->make('key')`。
    ///
    /// 等价于 [`Container::make_with_scope`] 传入 `scope_id = 0`。
    /// 对于 `Scoped` 生命周期服务，会使用 `scope_id = 0` 作为默认作用域。
    ///
    /// # 返回
    ///
    /// - `Some(Arc<T>)`：服务已注册，返回实例（单例返回缓存实例，瞬态返回新实例）
    /// - `None`：服务未注册
    ///
    /// # Panics
    ///
    /// 理论上不会 panic（工厂返回的 `Box<dyn Any>` 内部类型由编译时泛型保证）。
    /// 若发生 panic 说明内部状态被破坏（bindings 与 instances 不一致）。
    pub fn make<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.make_with_scope::<T>(0)
    }

    /// 解析服务实例（带作用域 ID）
    ///
    /// 对齐 PHP `app()->make('key')` + 请求作用域支持。
    ///
    /// # 生命周期处理
    ///
    /// - `Singleton`：忽略 `scope_id`，返回全局缓存的单例
    /// - `Transient`：忽略 `scope_id`，每次调用工厂创建新实例
    /// - `Scoped`：同一 `scope_id` 内首次调用工厂创建并缓存，后续返回缓存
    ///
    /// # 返回
    ///
    /// - `Some(Arc<T>)`：服务已注册，返回实例
    /// - `None`：服务未注册
    ///
    /// # Panics
    ///
    /// 理论上不会 panic（工厂返回的 `Box<dyn Any>` 内部类型由编译时泛型保证）。
    /// 若发生 panic 说明内部状态被破坏。
    pub fn make_with_scope<T: Send + Sync + 'static>(&self, scope_id: ScopeId) -> Option<Arc<T>> {
        let type_id = TypeId::of::<T>();

        // 1. 检查全局单例缓存（singleton 和 instance 都会写入此缓存）
        if let Some(cached) = self.instances.read().get(&type_id) {
            return Arc::downcast::<T>(cached.clone()).ok();
        }

        // 2. 检查作用域缓存（仅 Scoped 生命周期）
        if scope_id != 0 {
            let scoped = self.scoped_instances.read();
            if let Some(scope_map) = scoped.get(&scope_id) {
                if let Some(cached) = scope_map.get(&type_id) {
                    return Arc::downcast::<T>(cached.clone()).ok();
                }
            }
        }

        // 3. 查找绑定
        // 注：先绑定 `let` 延长 `RwLockReadGuard` 生命周期，避免临时值被释放
        let guard = self.bindings.read();
        let binding = guard.get(&type_id)?.clone();
        drop(guard); // 释放读锁后再调用工厂（避免持锁调用用户代码引发死锁/重入）

        let instance = (binding.factory)();

        match binding.lifetime {
            Lifetime::Singleton => {
                let arc: Arc<dyn Any + Send + Sync> = Arc::from(instance);
                self.instances.write().insert(type_id, arc.clone());
                Arc::downcast::<T>(arc).ok()
            }
            Lifetime::Scoped => {
                let arc: Arc<dyn Any + Send + Sync> = Arc::from(instance);
                self.scoped_instances
                    .write()
                    .entry(scope_id)
                    .or_default()
                    .insert(type_id, arc.clone());
                Arc::downcast::<T>(arc).ok()
            }
            Lifetime::Transient => {
                // 瞬态：直接返回（不缓存）
                Arc::downcast::<T>(Arc::from(instance)).ok()
            }
        }
    }

    /// 清理指定作用域的所有缓存实例
    ///
    /// 应在请求结束时调用（如 axum 中间件在请求处理完毕后调用），
    /// 释放该作用域内创建的所有 Scoped 服务实例。
    ///
    /// # 用法
    ///
    /// ```ignore
    /// use sz_rust_core::container::Container;
    ///
    /// let container = Container::new();
    /// container.scoped(|| RequestCache::new());
    ///
    /// let scope_id = generate_scope_id(); // 如从 axum State 获取
    /// let _cache = container.make_with_scope::<RequestCache>(scope_id);
    ///
    /// // 请求结束
    /// container.clear_scope(scope_id);
    /// ```
    pub fn clear_scope(&self, scope_id: ScopeId) {
        self.scoped_instances.write().remove(&scope_id);
    }

    /// 检查服务是否已注册
    pub fn has<T: 'static>(&self) -> bool {
        let type_id = TypeId::of::<T>();
        self.bindings.read().contains_key(&type_id)
    }

    /// 移除指定类型的服务绑定（含单例缓存与所有作用域缓存）
    ///
    /// 对齐 PHP `app()->remove('key')`。
    pub fn forget<T: 'static>(&self) {
        let type_id = TypeId::of::<T>();
        self.bindings.write().remove(&type_id);
        self.instances.write().remove(&type_id);
        // 清理所有作用域中该类型的缓存
        let mut scoped = self.scoped_instances.write();
        for scope_map in scoped.values_mut() {
            scope_map.remove(&type_id);
        }
    }

    /// 清空所有服务绑定与缓存（含单例、作用域、别名）
    pub fn clear(&self) {
        self.bindings.write().clear();
        self.instances.write().clear();
        self.scoped_instances.write().clear();
        self.aliases.write().clear();
    }

    /// 已注册服务数量（不含别名）
    pub fn count(&self) -> usize {
        self.bindings.read().len()
    }

    /// 已注册别名数量
    pub fn alias_count(&self) -> usize {
        self.aliases.read().len()
    }

    /// 当前活跃作用域数量
    ///
    /// 可用于检测作用域泄漏（如请求结束未调用 `clear_scope`）。
    pub fn active_scope_count(&self) -> usize {
        self.scoped_instances.read().len()
    }
}

impl Default for Container {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Container {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Container")
            .field("bindings_count", &self.bindings.read().len())
            .field("instances_count", &self.instances.read().len())
            .field("scoped_scope_count", &self.scoped_instances.read().len())
            .field("aliases_count", &self.aliases.read().len())
            .finish()
    }
}

// ============================================================================
// App 容器（全局单例）
// ============================================================================

/// App 容器（全局单例）
///
/// 持有应用配置、各子系统单例和 DI 服务容器。通过 [`App::global()`] 获取全局实例，
/// 通过 [`App::init()`] 初始化。
pub struct App {
    /// 应用配置（只读，初始化后不可变）
    config: AppConfig,
    /// 数据库连接配置（5 个：mysql/njszjt/ljclz/food/oceanbase）
    /// 后续将替换为 SZ-ORM `Pool` 实例
    db_connections: HashMap<String, DatabaseConnection>,
    /// Cache 单例占位（接入真正的 Cache facade）
    cache: RwLock<Option<String>>,
    /// Log 单例占位（接入 sz-orm-logger + tracing）
    log: RwLock<Option<String>>,
    /// DI 服务容器（服务注册/解析/生命周期管理）
    container: Container,
}

impl App {
    /// 构造 App 实例（不注册到全局单例）
    ///
    /// 用于测试或显式持有实例的场景。生产代码应使用 [`App::init()`] 注册全局单例。
    pub fn new(config: AppConfig) -> App {
        let db_connections = config.database.connections.clone();
        App {
            config,
            db_connections,
            cache: RwLock::new(None),
            log: RwLock::new(None),
            container: Container::new(),
        }
    }

    /// 初始化全局 App 容器
    ///
    /// 只能调用一次，重复调用返回已有实例。
    ///
    /// ```rust,ignore
    /// use sz_rust_core::container::App;
    /// use sz_rust_core::config::AppConfig;
    ///
    /// let config = AppConfig::load_from_dir("config").unwrap();
    /// let app = App::init(config);
    /// ```
    pub fn init(config: AppConfig) -> &'static App {
        APP.get_or_init(|| App::new(config))
    }

    /// 获取全局 App 容器实例
    ///
    /// 必须先调用 [`App::init()`] 初始化，否则返回 `None`。
    ///
    /// # 命名说明
    ///
    /// 此方法对应 PHP `app()` helper（获取全局容器实例）。
    /// 不使用 `App::instance()` 是为了避免与 [`App::instance<T>`]（绑定实例方法，
    /// 对齐 PHP `app()->instance('key', $obj)`）冲突。
    pub fn global() -> Option<&'static App> {
        APP.get()
    }

    /// 获取应用配置
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// 获取数据库连接配置
    ///
    /// 对齐 PHP `Db::connect('mysql')`。
    ///
    /// 当前返回 `DatabaseConnection` 配置。
    /// 后续将替换为 SZ-ORM `Pool` 实例。
    pub fn db_connection(&self, name: &str) -> Option<&DatabaseConnection> {
        self.db_connections.get(name)
    }

    /// 获取所有数据库连接名称
    pub fn db_connection_names(&self) -> Vec<&str> {
        self.db_connections.keys().map(|s| s.as_str()).collect()
    }

    /// 获取默认数据库连接配置
    pub fn default_db_connection(&self) -> Option<&DatabaseConnection> {
        self.db_connection(&self.config.database.default)
    }

    /// 设置 Cache 单例（将替换为真正的 Cache facade）
    pub fn set_cache(&self, cache: impl Into<String>) {
        let mut guard = self.cache.write();
        *guard = Some(cache.into());
    }

    /// 获取 Cache 单例
    pub fn cache(&self) -> Option<String> {
        self.cache.read().clone()
    }

    /// 设置 Log 单例（将替换为真正的日志系统）
    pub fn set_log(&self, log: impl Into<String>) {
        let mut guard = self.log.write();
        *guard = Some(log.into());
    }

    /// 获取 Log 单例
    pub fn log(&self) -> Option<String> {
        self.log.read().clone()
    }

    // ========================================================================
    // DI 服务容器代理方法（对齐 PHP app()->bind/make/singleton/scoped/instance/alias）
    // ========================================================================

    /// 获取 DI 服务容器引用
    pub fn container(&self) -> &Container {
        &self.container
    }

    /// 注册瞬态服务
    ///
    /// 对齐 PHP `app()->bind('key', fn() => new Service())`。
    pub fn bind<T, F>(&self, factory: F)
    where
        T: Send + Sync + 'static,
        F: Fn() -> T + Send + Sync + 'static,
    {
        self.container.bind(factory);
    }

    /// 注册单例服务
    ///
    /// 对齐 PHP `app()->singleton('key', fn() => new Service())`。
    pub fn singleton<T, F>(&self, factory: F)
    where
        T: Send + Sync + 'static,
        F: Fn() -> T + Send + Sync + 'static,
    {
        self.container.singleton(factory);
    }

    /// 注册请求作用域服务
    ///
    /// 对齐 PHP `app()->scoped('key', fn() => new Service())`。
    pub fn scoped<T, F>(&self, factory: F)
    where
        T: Send + Sync + 'static,
        F: Fn() -> T + Send + Sync + 'static,
    {
        self.container.scoped(factory);
    }

    /// 直接绑定已创建的实例
    ///
    /// 对齐 PHP `app()->instance('key', $obj)`。
    pub fn instance<T>(&self, instance: T)
    where
        T: Send + Sync + 'static,
    {
        self.container.instance(instance);
    }

    /// 为服务类型注册字符串别名
    ///
    /// 对齐 PHP `app()->alias('name', Service::class)`。
    pub fn alias<T: 'static>(&self, name: impl Into<String>) {
        self.container.alias::<T>(name);
    }

    /// 解析服务实例（无作用域）
    ///
    /// 对齐 PHP `app()->make('key')`。
    pub fn make<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.container.make::<T>()
    }

    /// 解析服务实例（带作用域 ID）
    ///
    /// 对齐 PHP `app()->make('key')` + 请求作用域支持。
    pub fn make_with_scope<T: Send + Sync + 'static>(&self, scope_id: ScopeId) -> Option<Arc<T>> {
        self.container.make_with_scope::<T>(scope_id)
    }

    /// 清理指定作用域的所有缓存实例
    pub fn clear_scope(&self, scope_id: ScopeId) {
        self.container.clear_scope(scope_id);
    }

    /// 检查服务是否已注册
    pub fn has_service<T: 'static>(&self) -> bool {
        self.container.has::<T>()
    }
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("config", &self.config)
            .field(
                "db_connections",
                &self.db_connections.keys().collect::<Vec<_>>(),
            )
            .field("cache", &self.cache.read().is_some())
            .field("log", &self.log.read().is_some())
            .field("container", &self.container)
            .finish()
    }
}

// ============================================================================
// 单元测试（分离到 tests.rs，降低单文件认知负担）
// ============================================================================

#[cfg(test)]
mod tests;
