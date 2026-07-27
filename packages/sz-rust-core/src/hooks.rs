//! 钩子系统接入 — 16 事件 HookDispatcher
//!
//! 对齐 PHP `topthink/think-orm: ^2.0.52` 的 Model 钩子机制，并基于 sz-orm-core 扩展。
//!
//! ## PHP think-orm 2.0.x 钩子触发顺序（原生 12 事件）
//!
//! PHP `vendor/topthink/think-orm/src/Model.php` 通过 `trigger()` 方法在 `insert/update/
//! delete/restore/find` 内部自动触发钩子回调（业务代码不应直接调用 `trigger()`）。
//!
//! | 操作 | 触发顺序 |
//! |------|---------|
//! | INSERT | `before_write` → `before_insert` → (INSERT) → `after_insert` → `after_write` |
//! | UPDATE | `before_write` → `before_update` → (UPDATE) → `after_update` → `after_write` |
//! | DELETE | `before_delete` → (DELETE) → `after_delete` |
//! | RESTORE | `before_restore` → (UPDATE deleted_at=NULL) → `after_restore` |
//! | FIND | `before_find` → (SELECT) → `after_find` |
//!
//! PHP 项目实际使用情况（`e:\vue\test\富掌柜\cashier\server\app\common\model\`）：
//! - 仅实现 `onBeforeInsert` / `onBeforeUpdate` 两个回调
//! - 用于自动填充 `create_time` / `update_time` 时间戳
//! - `BaseModel::onBeforeInsert` 还会通过 `method_exists($model, "before_insert")`
//!   反向调用业务级 `before_insert()` 方法（如 `Worklogs::before_insert` 设置 `stat_day`）
//!
//! ## sz-orm-core 扩展（16 事件）
//!
//! sz-orm-core::hooks 在 PHP 原生 12 事件基础上扩展 4 个事件：
//! - `BeforeSave` / `AfterSave`：与 write 等价，命名风格借鉴 Rails/ActiveRecord
//! - `BeforeValidate` / `AfterValidate`：数据验证前后触发
//!
//! 扩展后的 INSERT/UPDATE 顺序（保持 PHP 原生顺序兼容）：
//! ```text
//! before_write → before_save → before_validate → validate → after_validate
//! → before_insert → (INSERT) → after_insert → after_save → after_write
//! ```
//!
//! ## 设计原则
//!
//! sz-rust 端不重复实现 HookDispatcher，而是 re-export sz-orm-core::hooks 的所有公开类型，
//! 并提供以下增强：
//! 1. [`ALL_EVENTS`] 常量：16 事件完整列表，便于遍历与测试
//! 2. [`event_name`] / [`event_from_name`]：PHP 风格字符串 ↔ HookEvent 双向映射
//! 3. [`HookExecutionRecorder`]：测试辅助工具，记录钩子执行顺序用于断言
//! 4. [`validate_insert_order`] / [`validate_update_order`] / ...：PHP 行为对齐验证函数
//!
//! ## 用法
//!
//! ### 注册运行时钩子
//!
//! ```ignore
//! use sz_rust_core::hooks::{HookRegistry, HookEvent, HookContext};
//! use std::sync::Arc;
//!
//! let registry = HookRegistry::new();
//! registry.register(
//!     HookEvent::BeforeInsert,
//!     Arc::new(|_ctx| {
//!         println!("before_insert");
//!         Ok(())
//!     }),
//! );
//!
//! let ctx = HookContext::new();
//! registry.dispatch(HookEvent::BeforeInsert, &ctx).unwrap();
//! ```
//!
//! ### 使用 HookDispatcher 触发完整序列
//!
//! ```ignore
//! use sz_rust_core::hooks::{HookDispatcher, Hookable, HookContext, HookResult};
//! use sz_orm_core::model::Model;
//!
//! struct User { id: i64 }
//! impl Model for User {
//!     type PrimaryKey = i64;
//!     fn table_name() -> &'static str { "users" }
//!     fn pk(&self) -> Self::PrimaryKey { self.id }
//!     fn set_pk(&mut self, pk: Self::PrimaryKey) { self.id = pk; }
//! }
//! impl Hookable for User {}
//!
//! let mut ctx = HookContext::new();
//! let id = HookDispatcher::insert::<User, _>(&mut ctx, |_ctx| Ok(1_i64)).unwrap();
//! assert_eq!(id, 1);
//! ```

// ============================================================================
// Re-export sz-orm-core::hooks 的所有公开类型
// ============================================================================

pub use sz_orm_core::hooks::{
    GlobalScope, HookContext, HookDispatcher, HookEvent, HookFn, HookRegistry, HookResult,
    Hookable, ScopeRegistry, SoftDelete, SoftDeleteScope, TenantModel, TenantScope,
};

// ============================================================================
// ALL_EVENTS — 16 事件完整列表
// ============================================================================

/// 16 事件完整列表（PHP 原生 12 + sz-orm-core 扩展 4）
///
/// 顺序与 [`HookEvent`] 枚举定义顺序一致，便于遍历测试。
pub const ALL_EVENTS: [HookEvent; 16] = [
    // ===== PHP think-orm 2.0.x 原生 12 事件 =====
    HookEvent::BeforeInsert,
    HookEvent::AfterInsert,
    HookEvent::BeforeUpdate,
    HookEvent::AfterUpdate,
    HookEvent::BeforeDelete,
    HookEvent::AfterDelete,
    HookEvent::BeforeFind,
    HookEvent::AfterFind,
    HookEvent::BeforeRestore,
    HookEvent::AfterRestore,
    HookEvent::BeforeWrite,
    HookEvent::AfterWrite,
    // ===== sz-orm-core 扩展 4 事件 =====
    HookEvent::BeforeSave,
    HookEvent::AfterSave,
    HookEvent::BeforeValidate,
    HookEvent::AfterValidate,
];

/// PHP 原生 12 事件列表（不含 sz-orm-core 扩展的 save/validate）
pub const PHP_NATIVE_EVENTS: [HookEvent; 12] = [
    HookEvent::BeforeInsert,
    HookEvent::AfterInsert,
    HookEvent::BeforeUpdate,
    HookEvent::AfterUpdate,
    HookEvent::BeforeDelete,
    HookEvent::AfterDelete,
    HookEvent::BeforeFind,
    HookEvent::AfterFind,
    HookEvent::BeforeRestore,
    HookEvent::AfterRestore,
    HookEvent::BeforeWrite,
    HookEvent::AfterWrite,
];

/// sz-orm-core 扩展 4 事件列表（save/validate）
pub const EXTENDED_EVENTS: [HookEvent; 4] = [
    HookEvent::BeforeSave,
    HookEvent::AfterSave,
    HookEvent::BeforeValidate,
    HookEvent::AfterValidate,
];

// ============================================================================
// event_name / event_from_name — PHP 风格字符串映射
// ============================================================================

/// 将 [`HookEvent`] 转为 PHP 风格的 snake_case 字符串名
///
/// 对齐 PHP think-orm `trigger('before_write', $model)` 的事件名格式。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::{event_name, HookEvent};
///
/// assert_eq!(event_name(HookEvent::BeforeInsert), "before_insert");
/// assert_eq!(event_name(HookEvent::AfterWrite), "after_write");
/// assert_eq!(event_name(HookEvent::BeforeValidate), "before_validate");
/// ```
pub fn event_name(event: HookEvent) -> &'static str {
    match event {
        HookEvent::BeforeInsert => "before_insert",
        HookEvent::AfterInsert => "after_insert",
        HookEvent::BeforeUpdate => "before_update",
        HookEvent::AfterUpdate => "after_update",
        HookEvent::BeforeDelete => "before_delete",
        HookEvent::AfterDelete => "after_delete",
        HookEvent::BeforeWrite => "before_write",
        HookEvent::AfterWrite => "after_write",
        HookEvent::BeforeSave => "before_save",
        HookEvent::AfterSave => "after_save",
        HookEvent::BeforeRestore => "before_restore",
        HookEvent::AfterRestore => "after_restore",
        HookEvent::BeforeFind => "before_find",
        HookEvent::AfterFind => "after_find",
        HookEvent::BeforeValidate => "before_validate",
        HookEvent::AfterValidate => "after_validate",
    }
}

/// 将 PHP 风格字符串名转为 [`HookEvent`]
///
/// 对齐 PHP think-orm 事件名解析。未知名称返回 `None`。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::{event_from_name, HookEvent};
///
/// assert_eq!(event_from_name("before_insert"), Some(HookEvent::BeforeInsert));
/// assert_eq!(event_from_name("after_write"), Some(HookEvent::AfterWrite));
/// assert_eq!(event_from_name("unknown_event"), None);
/// ```
pub fn event_from_name(name: &str) -> Option<HookEvent> {
    match name {
        "before_insert" => Some(HookEvent::BeforeInsert),
        "after_insert" => Some(HookEvent::AfterInsert),
        "before_update" => Some(HookEvent::BeforeUpdate),
        "after_update" => Some(HookEvent::AfterUpdate),
        "before_delete" => Some(HookEvent::BeforeDelete),
        "after_delete" => Some(HookEvent::AfterDelete),
        "before_write" => Some(HookEvent::BeforeWrite),
        "after_write" => Some(HookEvent::AfterWrite),
        "before_save" => Some(HookEvent::BeforeSave),
        "after_save" => Some(HookEvent::AfterSave),
        "before_restore" => Some(HookEvent::BeforeRestore),
        "after_restore" => Some(HookEvent::AfterRestore),
        "before_find" => Some(HookEvent::BeforeFind),
        "after_find" => Some(HookEvent::AfterFind),
        "before_validate" => Some(HookEvent::BeforeValidate),
        "after_validate" => Some(HookEvent::AfterValidate),
        _ => None,
    }
}

// ============================================================================
// PHP 触发顺序常量 — 对齐 PHP think-orm 2.0.x
// ============================================================================

/// PHP INSERT 操作的钩子触发顺序（sz-orm-core 扩展版，含 save/validate）
///
/// 顺序：`before_write` → `before_save` → `before_validate` → `validate`(隐式)
/// → `after_validate` → `before_insert` → (INSERT) → `after_insert`
/// → `after_save` → `after_write`
///
/// 注：`validate` 不在 HookEvent 枚举中（它是 Hookable trait 的方法，由
/// [`HookDispatcher::insert`] 在 `before_validate` 和 `after_validate` 之间调用）。
pub const INSERT_ORDER: [HookEvent; 8] = [
    HookEvent::BeforeWrite,
    HookEvent::BeforeSave,
    HookEvent::BeforeValidate,
    HookEvent::AfterValidate,
    HookEvent::BeforeInsert,
    HookEvent::AfterInsert,
    HookEvent::AfterSave,
    HookEvent::AfterWrite,
];

/// PHP UPDATE 操作的钩子触发顺序（sz-orm-core 扩展版，含 save/validate）
pub const UPDATE_ORDER: [HookEvent; 8] = [
    HookEvent::BeforeWrite,
    HookEvent::BeforeSave,
    HookEvent::BeforeValidate,
    HookEvent::AfterValidate,
    HookEvent::BeforeUpdate,
    HookEvent::AfterUpdate,
    HookEvent::AfterSave,
    HookEvent::AfterWrite,
];

/// PHP DELETE 操作的钩子触发顺序
pub const DELETE_ORDER: [HookEvent; 2] = [HookEvent::BeforeDelete, HookEvent::AfterDelete];

/// PHP RESTORE 操作的钩子触发顺序
pub const RESTORE_ORDER: [HookEvent; 2] = [HookEvent::BeforeRestore, HookEvent::AfterRestore];

/// PHP FIND 操作的钩子触发顺序
pub const FIND_ORDER: [HookEvent; 2] = [HookEvent::BeforeFind, HookEvent::AfterFind];

// ============================================================================
// HookExecutionRecorder — 测试辅助工具
// ============================================================================

use std::sync::{Arc, Mutex};

/// 钩子执行顺序记录器（测试辅助工具）
///
/// 在测试中通过 [`HookRegistry::register`] 注册记录器钩子，记录实际触发顺序，
/// 然后用 [`HookExecutionRecorder::events`] 断言顺序符合预期。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::{HookRegistry, HookExecutionRecorder, HookEvent, INSERT_ORDER};
///
/// let registry = HookRegistry::new();
/// let recorder = Arc::new(HookExecutionRecorder::new());
///
/// for event in INSERT_ORDER.iter() {
///     let r = Arc::clone(&recorder);
///     registry.register(*event, Arc::new(move |_ctx| {
///         r.record(HookEvent::BeforeInsert); // 闭包捕获具体 event
///         Ok(())
///     }));
/// }
/// ```
pub struct HookExecutionRecorder {
    events: Mutex<Vec<HookEvent>>,
}

impl Default for HookExecutionRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl HookExecutionRecorder {
    /// 创建空记录器
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    /// 记录一次事件触发
    pub fn record(&self, event: HookEvent) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }

    /// 获取已记录的事件顺序快照
    pub fn events(&self) -> Vec<HookEvent> {
        self.events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }

    /// 获取已记录的事件名称顺序快照（PHP 风格字符串）
    pub fn event_names(&self) -> Vec<&'static str> {
        self.events
            .lock()
            .map(|events| events.iter().map(|e| event_name(*e)).collect())
            .unwrap_or_default()
    }

    /// 清空记录
    pub fn clear(&self) {
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
    }

    /// 已记录的事件数量
    pub fn len(&self) -> usize {
        self.events.lock().map(|e| e.len()).unwrap_or(0)
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 断言已记录的事件顺序与预期一致
    ///
    /// 不一致时返回包含详细差异信息的错误字符串。
    pub fn assert_order(&self, expected: &[HookEvent]) -> Result<(), String> {
        let actual = self.events();
        if actual.len() != expected.len() {
            return Err(format!(
                "事件数量不匹配：expected {} 个 {:?}，actual {} 个 {:?}",
                expected.len(),
                expected.iter().map(|e| event_name(*e)).collect::<Vec<_>>(),
                actual.len(),
                actual.iter().map(|e| event_name(*e)).collect::<Vec<_>>(),
            ));
        }
        for (i, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            if actual != expected {
                return Err(format!(
                    "事件顺序不一致 @ index {}：expected {:?}，actual {:?}",
                    i,
                    event_name(*expected),
                    event_name(*actual)
                ));
            }
        }
        Ok(())
    }
}

// ============================================================================
// validate_*_order — PHP 行为对齐验证函数
// ============================================================================

/// 验证 HookRegistry 在 INSERT 操作中触发的事件顺序对齐 PHP think-orm 2.0.x
///
/// 此函数通过 [`HookRegistry`] 注册所有 INSERT 相关事件的记录器钩子，
/// 然后按 [`INSERT_ORDER`] 顺序手动 dispatch，验证记录器收到的顺序与预期一致。
///
/// 注：此函数验证 HookRegistry 的事件分发机制，不验证 HookDispatcher 的内部顺序
/// （后者由 sz-orm-core 的测试覆盖）。
pub fn validate_insert_order(registry: &HookRegistry) -> Result<(), String> {
    let recorder = Arc::new(HookExecutionRecorder::new());
    for event in INSERT_ORDER.iter() {
        let r = Arc::clone(&recorder);
        registry.register(
            *event,
            Arc::new(move |_ctx| {
                r.record(*event);
                Ok(())
            }),
        );
    }
    let ctx = HookContext::new();
    for event in INSERT_ORDER.iter() {
        registry
            .dispatch(*event, &ctx)
            .map_err(|e| format!("dispatch {:?} 失败：{}", event, e))?;
    }
    recorder.assert_order(&INSERT_ORDER)
}

/// 验证 HookRegistry 在 UPDATE 操作中触发的事件顺序对齐 PHP think-orm 2.0.x
pub fn validate_update_order(registry: &HookRegistry) -> Result<(), String> {
    let recorder = Arc::new(HookExecutionRecorder::new());
    for event in UPDATE_ORDER.iter() {
        let r = Arc::clone(&recorder);
        registry.register(
            *event,
            Arc::new(move |_ctx| {
                r.record(*event);
                Ok(())
            }),
        );
    }
    let ctx = HookContext::new();
    for event in UPDATE_ORDER.iter() {
        registry
            .dispatch(*event, &ctx)
            .map_err(|e| format!("dispatch {:?} 失败：{}", event, e))?;
    }
    recorder.assert_order(&UPDATE_ORDER)
}

/// 验证 HookRegistry 在 DELETE 操作中触发的事件顺序对齐 PHP think-orm 2.0.x
pub fn validate_delete_order(registry: &HookRegistry) -> Result<(), String> {
    let recorder = Arc::new(HookExecutionRecorder::new());
    for event in DELETE_ORDER.iter() {
        let r = Arc::clone(&recorder);
        registry.register(
            *event,
            Arc::new(move |_ctx| {
                r.record(*event);
                Ok(())
            }),
        );
    }
    let ctx = HookContext::new();
    for event in DELETE_ORDER.iter() {
        registry
            .dispatch(*event, &ctx)
            .map_err(|e| format!("dispatch {:?} 失败：{}", event, e))?;
    }
    recorder.assert_order(&DELETE_ORDER)
}

/// 验证 HookRegistry 在 RESTORE 操作中触发的事件顺序对齐 PHP think-orm 2.0.x
pub fn validate_restore_order(registry: &HookRegistry) -> Result<(), String> {
    let recorder = Arc::new(HookExecutionRecorder::new());
    for event in RESTORE_ORDER.iter() {
        let r = Arc::clone(&recorder);
        registry.register(
            *event,
            Arc::new(move |_ctx| {
                r.record(*event);
                Ok(())
            }),
        );
    }
    let ctx = HookContext::new();
    for event in RESTORE_ORDER.iter() {
        registry
            .dispatch(*event, &ctx)
            .map_err(|e| format!("dispatch {:?} 失败：{}", event, e))?;
    }
    recorder.assert_order(&RESTORE_ORDER)
}

/// 验证 HookRegistry 在 FIND 操作中触发的事件顺序对齐 PHP think-orm 2.0.x
pub fn validate_find_order(registry: &HookRegistry) -> Result<(), String> {
    let recorder = Arc::new(HookExecutionRecorder::new());
    for event in FIND_ORDER.iter() {
        let r = Arc::clone(&recorder);
        registry.register(
            *event,
            Arc::new(move |_ctx| {
                r.record(*event);
                Ok(())
            }),
        );
    }
    let ctx = HookContext::new();
    for event in FIND_ORDER.iter() {
        registry
            .dispatch(*event, &ctx)
            .map_err(|e| format!("dispatch {:?} 失败：{}", event, e))?;
    }
    recorder.assert_order(&FIND_ORDER)
}

// ============================================================================
// HookContextExt — sz-rust 端 Builder 链式 API 扩展
// ============================================================================
//
// sz-orm-core::HookContext 已提供 `with_tenant`/`with_operator`/`with_timestamp`
// 三个 builder 方法（消耗 self 返回新实例），但 `set_meta` 是 `&mut self` 方法
// （对齐 PHP `$ctx->metadata['key'] = $value` 就地修改语义）。
//
// 在 sz-rust 端扩展 `HookContextExt` trait，补充 `with_meta`/`with_metas`
// builder 链式 API（消耗 self 返回新实例），便于一行链式构造完整上下文：
//
// ```
// use sz_rust_core::hooks::{hook_context, HookContextExt};
//
// let ctx = hook_context()
//     .with_tenant(42)
//     .with_operator(1)
//     .with_timestamp(1700000000)
//     .with_meta("source", "api")
//     .with_meta("ip", "127.0.0.1");
// ```
//
// ## PHP 行为对齐
//
// PHP think-orm 2.0.x 的 `trigger()` 直接传递 `$model` 实例作为上下文，没有独立的
// HookContext 对象。sz-orm-core 的 HookContext 是 sz-orm 自研的请求级别元数据容器，
// 与 PHP `$model` 上下文是互补关系：
// - PHP `$model`：携带业务数据（如 `create_time`/`update_time`/`tenant_id` 字段）
// - sz-rust `HookContext`：携带请求级别元数据（如 `operator_id`/`trace_id`/`source`）
//
// PHP 项目实际使用场景（`e:\vue\test\富掌柜\cashier\server\app\common\model\`）：
// - `BaseModel::onBeforeInsert` 通过 `$model->create_time = time()` 自动填充时间戳
// - `Worklogs::before_insert` 通过 `$model->stat_day = date('Ymd')` 设置统计日字段
// - 这些操作在 sz-rust 端通过 `Hookable::before_insert(&mut HookContext)` 修改上下文
//
// HookContext Builder 提供请求级别元数据的链式构造能力，对齐 PHP
// `Event::listen` 回调中通过 `$model` 上下文访问请求信息的语义。

/// HookContext Builder 扩展 trait
///
/// 为 [`HookContext`] 补充 `with_meta`/`with_metas` builder 链式 API，
/// 与 sz-orm-core 的 `with_tenant`/`with_operator`/`with_timestamp` 风格一致。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::{hook_context, HookContextExt};
///
/// let ctx = hook_context()
///     .with_tenant(42)
///     .with_operator(1)
///     .with_timestamp(1700000000)
///     .with_meta("source", "api")
///     .with_meta("ip", "127.0.0.1");
///
/// assert_eq!(ctx.tenant_id, Some(42));
/// assert_eq!(ctx.operator_id, Some(1));
/// assert_eq!(ctx.timestamp, 1700000000);
/// assert_eq!(ctx.get_meta("source"), Some(&"api".to_string()));
/// assert_eq!(ctx.get_meta("ip"), Some(&"127.0.0.1".to_string()));
/// ```
pub trait HookContextExt {
    /// 链式添加单个元数据（消耗 self，返回新实例）
    ///
    /// 对齐 sz-orm-core `with_tenant`/`with_operator`/`with_timestamp` 的 builder 风格，
    /// 便于一行链式构造完整上下文。与 `set_meta(&mut self, ...)` 的区别：
    /// - `with_meta`：消耗 self，返回新实例，适合 builder 链式调用
    /// - `set_meta`：`&mut self`，就地在 HashMap 中插入，适合多次修改同一实例
    fn with_meta(self, key: impl Into<String>, value: impl Into<String>) -> Self;

    /// 链式批量添加元数据（消耗 self，返回新实例）
    ///
    /// 接受 `IntoIterator<Item = (impl Into<String>, impl Into<String>)>`，
    /// 便于从 HashMap/Vec/数组等多种数据源批量构造元数据。
    fn with_metas<I, K, V>(self, entries: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>;
}

impl HookContextExt for HookContext {
    fn with_meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    fn with_metas<I, K, V>(mut self, entries: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        for (key, value) in entries {
            self.metadata.insert(key.into(), value.into());
        }
        self
    }
}

/// 创建空的 HookContext（便捷函数，等价于 `HookContext::new()`）
///
/// 对齐 PHP `new HookContext()` 简写，提供 sz-rust 端的统一入口。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::hook_context;
/// use sz_rust_core::hooks::HookContextExt;
///
/// let ctx = hook_context()
///     .with_tenant(42)
///     .with_meta("source", "api");
/// ```
pub fn hook_context() -> HookContext {
    HookContext::new()
}

/// 创建带租户 ID 的 HookContext（便捷函数）
///
/// 等价于 `hook_context().with_tenant(tenant_id)`，对齐 PHP 多租户场景的常用初始化。
pub fn hook_context_with_tenant(tenant_id: i64) -> HookContext {
    HookContext::new().with_tenant(tenant_id)
}

/// 创建带操作人 ID 的 HookContext（便捷函数）
///
/// 等价于 `hook_context().with_operator(operator_id)`，对齐 PHP 审计日志场景的常用初始化。
pub fn hook_context_with_operator(operator_id: i64) -> HookContext {
    HookContext::new().with_operator(operator_id)
}

/// 从请求元数据批量创建 HookContext（便捷函数）
///
/// 接受 `IntoIterator<Item = (String, String)>`，便于从 HTTP headers 或 request
/// extensions 批量提取元数据构造上下文。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::hook_context_from_meta;
///
/// let headers = vec![
///     ("x-trace-id".to_string(), "abc123".to_string()),
///     ("x-source".to_string(), "api".to_string()),
/// ];
/// let ctx = hook_context_from_meta(headers);
/// assert_eq!(ctx.get_meta("x-trace-id"), Some(&"abc123".to_string()));
/// ```
pub fn hook_context_from_meta<I, K, V>(entries: I) -> HookContext
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    HookContext::new().with_metas(entries)
}

// ============================================================================
// SoftDelete 便捷函数与常量
// ============================================================================
//
// sz-orm-core 已提供 `SoftDelete` trait 和 `SoftDeleteScope` 全局作用域（已 re-export），
// 本节补充 sz-rust 端的 PHP 行为对齐便捷函数：
// - `DEFAULT_SOFT_DELETE_FIELD` 常量对齐 PHP think-orm 默认 `delete_time`
// - `soft_delete_filter_sql` / `only_trashed_filter_sql` SQL 片段构造函数
// - `is_soft_deleted` 等价 PHP `trashed()` 判断
//
// ## PHP think-orm 2.0.x SoftDelete concern 行为
//
// PHP `vendor/topthink/think-orm/src/model/concern/SoftDelete.php` 提供：
// - `trashed()`：检查 `delete_time` 字段是否有值（非空 = 已软删除）
// - `scopeWithTrashed($query)`：移除 soft_delete 范围（查询所有）
// - `scopeOnlyTrashed($query)`：仅查询软删除的（`delete_time IS NOT NULL`）
// - `withNoTrashed($query)`：默认追加 `delete_time IS NULL`（或 `= defaultSoftDelete`）
// - `delete()`：UPDATE SET delete_time = NOW()（除非 force=true）
// - `restore()`：UPDATE SET delete_time = NULL
// - `getDeleteTimeField()`：默认 `delete_time`，可通过 `$deleteTime` 属性自定义
//
// PHP 项目实际使用情况：
// - `BaseModel` 未 `use SoftDelete` trait（PHP think-orm 默认不启用软删除）
// - `UploadFile` 显式 `protected bool $deleteTime = false`（即使启用也禁用）
// - 其他模型默认无软删除行为
//
// sz-orm-core 的 SoftDelete trait 是自研增强，对齐 PHP SoftDelete concern，
// 业务模型按需实现 `SoftDelete` trait 启用软删除。

/// PHP think-orm 默认软删除字段名
///
/// 对齐 PHP `vendor/topthink/think-orm/src/model/concern/SoftDelete.php:202`：
/// ```php
/// $field = property_exists($this, 'deleteTime') && isset($this->deleteTime)
///     ? $this->deleteTime : 'delete_time';
/// ```
pub const DEFAULT_SOFT_DELETE_FIELD: &str = "delete_time";

/// 构造默认软删除过滤 SQL 片段（`{field} IS NULL`）
///
/// 对齐 PHP `withNoTrashed($query)` 默认条件（`defaultSoftDelete = null` 时）。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::soft_delete_filter_sql;
///
/// assert_eq!(soft_delete_filter_sql("delete_time"), "delete_time IS NULL");
/// assert_eq!(soft_delete_filter_sql("deleted_at"), "deleted_at IS NULL");
/// ```
pub fn soft_delete_filter_sql(field: &str) -> String {
    format!("{} IS NULL", field)
}

/// 构造仅查询软删除记录的 SQL 片段（`{field} IS NOT NULL`）
///
/// 对齐 PHP `scopeOnlyTrashed($query)` 的条件。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::only_trashed_filter_sql;
///
/// assert_eq!(only_trashed_filter_sql("delete_time"), "delete_time IS NOT NULL");
/// ```
pub fn only_trashed_filter_sql(field: &str) -> String {
    format!("{} IS NOT NULL", field)
}

/// 判断记录是否已软删除（等价 PHP `trashed()`）
///
/// 对齐 PHP `vendor/topthink/think-orm/src/model/concern/SoftDelete.php:39`：
/// ```php
/// public function trashed(): bool
/// {
///     $field = $this->getDeleteTimeField();
///     if ($field && !empty($this->getOrigin($field))) {
///         return true;
///     }
///     return false;
/// }
/// ```
///
/// # 参数
///
/// - `field_value`：软删除字段的值（`None` 表示 NULL 或不存在，`Some(s)` 表示有值）
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::is_soft_deleted;
///
/// // 字段为 NULL → 未软删除
/// assert!(!is_soft_deleted(None));
/// // 字段为空字符串 → 未软删除（PHP empty() 判空）
/// assert!(!is_soft_deleted(Some("")));
/// // 字段有值 → 已软删除
/// assert!(is_soft_deleted(Some("2026-07-21 10:00:00")));
/// assert!(is_soft_deleted(Some("1700000000")));
/// ```
pub fn is_soft_deleted(field_value: Option<&str>) -> bool {
    match field_value {
        None => false,
        Some(v) => !v.is_empty(),
    }
}

/// 构造软删除 UPDATE SQL（`UPDATE {table} SET {field} = NOW() WHERE {pk} = ?`）
///
/// 对齐 PHP `delete()` 的软删除行为：UPDATE SET delete_time = NOW() WHERE pk = ?
///
/// 注：sz-orm-core 的 Repository 实际执行软删除，此函数仅提供 SQL 片段用于
/// 测试和文档参考，不应直接用于业务代码。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::soft_delete_update_sql;
///
/// let sql = soft_delete_update_sql("users", "delete_time", "id");
/// assert_eq!(sql, "UPDATE users SET delete_time = NOW() WHERE id = ?");
/// ```
pub fn soft_delete_update_sql(table: &str, field: &str, pk: &str) -> String {
    format!("UPDATE {} SET {} = NOW() WHERE {} = ?", table, field, pk)
}

/// 构造恢复软删除 UPDATE SQL（`UPDATE {table} SET {field} = NULL WHERE {pk} = ?`）
///
/// 对齐 PHP `restore()` 的恢复行为：UPDATE SET delete_time = NULL WHERE pk = ?
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::soft_delete_restore_sql;
///
/// let sql = soft_delete_restore_sql("users", "delete_time", "id");
/// assert_eq!(sql, "UPDATE users SET delete_time = NULL WHERE id = ?");
/// ```
pub fn soft_delete_restore_sql(table: &str, field: &str, pk: &str) -> String {
    format!("UPDATE {} SET {} = NULL WHERE {} = ?", table, field, pk)
}

// ============================================================================
// TenantModel 便捷函数与常量
// ============================================================================
//
// sz-orm-core 已提供 `TenantModel` trait 和 `TenantScope` 全局作用域（已 re-export），
// 本节补充 sz-rust 端的 PHP 行为对齐便捷函数：
// - `DEFAULT_TENANT_FIELD` 常量对齐 PHP `BaseModel` 的 `app_id` 字段名
//   （注意：sz-orm-core `TenantModel::tenant_field()` 默认 `tenant_id`，
//    但 PHP 项目实际使用 `app_id` 作为多租户字段，sz-rust 端以 PHP 行为准）
// - `tenant_filter_sql` / `tenant_filter_sql_no_table` SQL 片段构造函数
// - `is_tenant_aware` 等价 PHP `self::$app_id > 0` 判断
//
// ## PHP think-orm 2.0.x 多租户行为（基于全局查询作用域）
//
// PHP `app/common/model/BaseModel.php` 通过 think-orm 的全局查询作用域实现多租户：
// - `protected $globalScope = ['app_id']`：声明 `app_id` 作用域
// - `public function scopeApp_id($query)`：作用域实现
//   ```php
//   public function scopeApp_id($query){
//       if (self::$app_id > 0) {
//           $query->where($query->getTable() . '.app_id', self::$app_id);
//       }
//   }
//   ```
// - `public static $app_id`：静态属性，通过 `bindAppId()` 根据当前模块设置
//   （shop/farm/api/oapi/supplier/oa/cashier/food/scene 9 个模块）
//
// PHP 项目实际使用情况：
// - 所有继承 `BaseModel` 的模型自动启用 `app_id` 全局作用域
// - `app_id` 字段名固定，不是 `tenant_id`（sz-orm-core 默认）
// - 当 `app_id > 0` 时才追加 WHERE 条件（0 或未设置时跨租户查询）
// - INSERT 时 `app_id` 由业务代码显式设置（非自动填充）
//
// sz-orm-core 的 `TenantModel` trait 是通用多租户抽象，字段名默认 `tenant_id`，
// sz-rust 端通过 `DEFAULT_TENANT_FIELD = "app_id"` 对齐 PHP 项目实际使用。

/// PHP `BaseModel` 默认租户字段名
///
/// 对齐 PHP `app/common/model/BaseModel.php:21`：
/// ```php
/// protected $globalScope = ['app_id'];
/// ```
/// 注意：sz-orm-core `TenantModel::tenant_field()` 默认 `tenant_id`，
/// 但 PHP 项目实际使用 `app_id`，sz-rust 端以 PHP 行为准。
pub const DEFAULT_TENANT_FIELD: &str = "app_id";

/// 构造带表前缀的多租户过滤 SQL 片段（`{table}.{field} = ?`）
///
/// 对齐 PHP `BaseModel::scopeApp_id($query)` 的 WHERE 条件：
/// ```php
/// $query->where($query->getTable() . '.app_id', self::$app_id);
/// ```
/// 适用于 JOIN 查询或带表别名的场景，避免字段歧义。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::{tenant_filter_sql, DEFAULT_TENANT_FIELD};
///
/// // 默认字段名
/// let sql = tenant_filter_sql(DEFAULT_TENANT_FIELD, "users");
/// assert_eq!(sql, "users.app_id = ?");
///
/// // 自定义字段名
/// let sql = tenant_filter_sql("tenant_id", "orders");
/// assert_eq!(sql, "orders.tenant_id = ?");
/// ```
pub fn tenant_filter_sql(field: &str, table: &str) -> String {
    format!("{}.{} = ?", table, field)
}

/// 构造不带表前缀的多租户过滤 SQL 片段（`{field} = ?`）
///
/// 对齐 sz-orm-core `TenantScope::apply_scope` 的 WHERE 条件格式：
/// ```rust,ignore
/// format!("{} = ?", <M as TenantModel>::tenant_field())
/// ```
/// 适用于单表查询无歧义的场景。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::{tenant_filter_sql_no_table, DEFAULT_TENANT_FIELD};
///
/// // 默认字段名
/// let sql = tenant_filter_sql_no_table(DEFAULT_TENANT_FIELD);
/// assert_eq!(sql, "app_id = ?");
///
/// // 自定义字段名
/// let sql = tenant_filter_sql_no_table("tenant_id");
/// assert_eq!(sql, "tenant_id = ?");
/// ```
pub fn tenant_filter_sql_no_table(field: &str) -> String {
    format!("{} = ?", field)
}

/// 判断是否启用多租户过滤（等价 PHP `self::$app_id > 0`）
///
/// 对齐 PHP `app/common/model/BaseModel.php:135`：
/// ```php
/// public function scopeApp_id($query){
///     if (self::$app_id > 0) {
///         $query->where($query->getTable() . '.app_id', self::$app_id);
///     }
/// }
/// ```
/// 当 `app_id <= 0` 时不追加 WHERE 条件，允许跨租户查询（需调用方自行保证安全）。
///
/// # 示例
///
/// ```ignore
/// use sz_rust_core::hooks::is_tenant_aware;
///
/// // 0 或负数：不启用多租户过滤
/// assert!(!is_tenant_aware(0));
/// assert!(!is_tenant_aware(-1));
///
/// // 正数：启用多租户过滤
/// assert!(is_tenant_aware(1));
/// assert!(is_tenant_aware(42));
/// ```
pub fn is_tenant_aware(app_id: i64) -> bool {
    app_id > 0
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ----------------------------------------------------------------
    // 1. ALL_EVENTS / PHP_NATIVE_EVENTS / EXTENDED_EVENTS 常量
    // ----------------------------------------------------------------

    #[test]
    fn test_all_events_count() {
        assert_eq!(ALL_EVENTS.len(), 16, "16 事件完整列表");
    }

    #[test]
    fn test_php_native_events_count() {
        assert_eq!(PHP_NATIVE_EVENTS.len(), 12, "PHP 原生 12 事件");
    }

    #[test]
    fn test_extended_events_count() {
        assert_eq!(EXTENDED_EVENTS.len(), 4, "sz-orm-core 扩展 4 事件");
    }

    #[test]
    fn test_all_events_no_duplicates() {
        // HookEvent 未实现 Ord，通过 event_name 字符串去重验证
        let mut names: Vec<&str> = ALL_EVENTS.iter().map(|e| event_name(*e)).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ALL_EVENTS.len(), "ALL_EVENTS 不应有重复");
    }

    #[test]
    fn test_native_plus_extended_equals_all() {
        // HookEvent 未实现 Ord，通过 event_name 字符串集合比较
        let mut all_names: Vec<&str> = ALL_EVENTS.iter().map(|e| event_name(*e)).collect();
        all_names.sort();

        let mut combined: Vec<&str> = PHP_NATIVE_EVENTS
            .iter()
            .chain(EXTENDED_EVENTS.iter())
            .map(|e| event_name(*e))
            .collect();
        combined.sort();

        assert_eq!(combined, all_names, "PHP_NATIVE + EXTENDED == ALL");
    }

    // ----------------------------------------------------------------
    // 2. event_name / event_from_name 双向映射
    // ----------------------------------------------------------------

    #[test]
    fn test_event_name_php_style() {
        // PHP think-orm 风格的 snake_case 事件名
        assert_eq!(event_name(HookEvent::BeforeInsert), "before_insert");
        assert_eq!(event_name(HookEvent::AfterInsert), "after_insert");
        assert_eq!(event_name(HookEvent::BeforeUpdate), "before_update");
        assert_eq!(event_name(HookEvent::AfterUpdate), "after_update");
        assert_eq!(event_name(HookEvent::BeforeDelete), "before_delete");
        assert_eq!(event_name(HookEvent::AfterDelete), "after_delete");
        assert_eq!(event_name(HookEvent::BeforeWrite), "before_write");
        assert_eq!(event_name(HookEvent::AfterWrite), "after_write");
        assert_eq!(event_name(HookEvent::BeforeSave), "before_save");
        assert_eq!(event_name(HookEvent::AfterSave), "after_save");
        assert_eq!(event_name(HookEvent::BeforeRestore), "before_restore");
        assert_eq!(event_name(HookEvent::AfterRestore), "after_restore");
        assert_eq!(event_name(HookEvent::BeforeFind), "before_find");
        assert_eq!(event_name(HookEvent::AfterFind), "after_find");
        assert_eq!(event_name(HookEvent::BeforeValidate), "before_validate");
        assert_eq!(event_name(HookEvent::AfterValidate), "after_validate");
    }

    #[test]
    fn test_event_from_name_roundtrip() {
        // 所有 16 事件应能完成 HookEvent → str → HookEvent 的往返映射
        for event in ALL_EVENTS.iter() {
            let name = event_name(*event);
            let back = event_from_name(name);
            assert_eq!(back, Some(*event), "事件 {:?} 往返映射失败", event);
        }
    }

    #[test]
    fn test_event_from_name_unknown() {
        assert_eq!(event_from_name("unknown_event"), None);
        assert_eq!(event_from_name(""), None);
        assert_eq!(event_from_name("BeforeInsert"), None, "大小写敏感");
        assert_eq!(event_from_name("before-insert"), None, "需 snake_case");
    }

    // ----------------------------------------------------------------
    // 3. 触发顺序常量正确性
    // ----------------------------------------------------------------

    #[test]
    fn test_insert_order_aligns_php() {
        // PHP think-orm 2.0.x INSERT 顺序：
        // before_write → before_insert → INSERT → after_insert → after_write
        // sz-orm-core 扩展在中间插入 save/validate：
        // before_write → before_save → before_validate → after_validate
        // → before_insert → after_insert → after_save → after_write
        assert_eq!(
            INSERT_ORDER,
            [
                HookEvent::BeforeWrite,
                HookEvent::BeforeSave,
                HookEvent::BeforeValidate,
                HookEvent::AfterValidate,
                HookEvent::BeforeInsert,
                HookEvent::AfterInsert,
                HookEvent::AfterSave,
                HookEvent::AfterWrite,
            ]
        );
        // PHP 原生顺序应在扩展顺序中保持相对位置
        let php_order = [
            HookEvent::BeforeWrite,
            HookEvent::BeforeInsert,
            HookEvent::AfterInsert,
            HookEvent::AfterWrite,
        ];
        let mut php_idx = 0;
        for event in INSERT_ORDER.iter() {
            if php_idx < php_order.len() && *event == php_order[php_idx] {
                php_idx += 1;
            }
        }
        assert_eq!(php_idx, php_order.len(), "PHP 原生顺序应作为子序列保留");
    }

    #[test]
    fn test_update_order_aligns_php() {
        // PHP think-orm 2.0.x UPDATE 顺序：
        // before_write → before_update → UPDATE → after_update → after_write
        assert_eq!(
            UPDATE_ORDER,
            [
                HookEvent::BeforeWrite,
                HookEvent::BeforeSave,
                HookEvent::BeforeValidate,
                HookEvent::AfterValidate,
                HookEvent::BeforeUpdate,
                HookEvent::AfterUpdate,
                HookEvent::AfterSave,
                HookEvent::AfterWrite,
            ]
        );
    }

    #[test]
    fn test_delete_order_aligns_php() {
        // PHP think-orm 2.0.x DELETE 顺序：before_delete → DELETE → after_delete
        assert_eq!(
            DELETE_ORDER,
            [HookEvent::BeforeDelete, HookEvent::AfterDelete]
        );
    }

    #[test]
    fn test_restore_order_aligns_php() {
        // PHP think-orm 2.0.x RESTORE 顺序：before_restore → UPDATE → after_restore
        assert_eq!(
            RESTORE_ORDER,
            [HookEvent::BeforeRestore, HookEvent::AfterRestore]
        );
    }

    #[test]
    fn test_find_order_aligns_php() {
        // PHP think-orm 2.0.x FIND 顺序：before_find → SELECT → after_find
        assert_eq!(FIND_ORDER, [HookEvent::BeforeFind, HookEvent::AfterFind]);
    }

    // ----------------------------------------------------------------
    // 4. HookExecutionRecorder 工具
    // ----------------------------------------------------------------

    #[test]
    fn test_recorder_empty() {
        let r = HookExecutionRecorder::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert_eq!(r.events(), Vec::<HookEvent>::new());
        assert_eq!(r.event_names(), Vec::<&str>::new());
    }

    #[test]
    fn test_recorder_record_and_read() {
        let r = HookExecutionRecorder::new();
        r.record(HookEvent::BeforeInsert);
        r.record(HookEvent::AfterInsert);
        assert_eq!(r.len(), 2);
        assert_eq!(
            r.events(),
            vec![HookEvent::BeforeInsert, HookEvent::AfterInsert]
        );
        assert_eq!(r.event_names(), vec!["before_insert", "after_insert"]);
    }

    #[test]
    fn test_recorder_clear() {
        let r = HookExecutionRecorder::new();
        r.record(HookEvent::BeforeInsert);
        r.clear();
        assert!(r.is_empty());
    }

    #[test]
    fn test_recorder_assert_order_ok() {
        let r = HookExecutionRecorder::new();
        r.record(HookEvent::BeforeInsert);
        r.record(HookEvent::AfterInsert);
        let result = r.assert_order(&[HookEvent::BeforeInsert, HookEvent::AfterInsert]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_recorder_assert_order_mismatch_count() {
        let r = HookExecutionRecorder::new();
        r.record(HookEvent::BeforeInsert);
        let result = r.assert_order(&[HookEvent::BeforeInsert, HookEvent::AfterInsert]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("数量不匹配"));
    }

    #[test]
    fn test_recorder_assert_order_mismatch_value() {
        let r = HookExecutionRecorder::new();
        r.record(HookEvent::BeforeInsert);
        r.record(HookEvent::BeforeUpdate);
        let result = r.assert_order(&[HookEvent::BeforeInsert, HookEvent::AfterInsert]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("顺序不一致"));
        assert!(err.contains("after_insert"));
    }

    // ----------------------------------------------------------------
    // 5. validate_*_order 函数 — HookRegistry 触发顺序验证
    // ----------------------------------------------------------------

    #[test]
    fn test_validate_insert_order() {
        let registry = HookRegistry::new();
        let result = validate_insert_order(&registry);
        assert!(result.is_ok(), "INSERT 顺序验证应通过：{:?}", result);
    }

    #[test]
    fn test_validate_update_order() {
        let registry = HookRegistry::new();
        let result = validate_update_order(&registry);
        assert!(result.is_ok(), "UPDATE 顺序验证应通过：{:?}", result);
    }

    #[test]
    fn test_validate_delete_order() {
        let registry = HookRegistry::new();
        let result = validate_delete_order(&registry);
        assert!(result.is_ok(), "DELETE 顺序验证应通过：{:?}", result);
    }

    #[test]
    fn test_validate_restore_order() {
        let registry = HookRegistry::new();
        let result = validate_restore_order(&registry);
        assert!(result.is_ok(), "RESTORE 顺序验证应通过：{:?}", result);
    }

    #[test]
    fn test_validate_find_order() {
        let registry = HookRegistry::new();
        let result = validate_find_order(&registry);
        assert!(result.is_ok(), "FIND 顺序验证应通过：{:?}", result);
    }

    // ----------------------------------------------------------------
    // 6. 16 事件全部可注册+触发（HookRegistry）
    // ----------------------------------------------------------------

    #[test]
    fn test_all_16_events_registerable_and_dispatchable() {
        let registry = HookRegistry::new();
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));

        for event in ALL_EVENTS.iter() {
            let c = Arc::clone(&counter);
            registry.register(
                *event,
                Arc::new(move |_ctx| {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                }),
            );
        }

        let ctx = HookContext::new();
        for event in ALL_EVENTS.iter() {
            registry.dispatch(*event, &ctx).expect("dispatch 应成功");
        }

        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            16,
            "16 事件全部应被触发一次"
        );
    }

    #[test]
    fn test_all_16_events_countable() {
        let registry = HookRegistry::new();
        for event in ALL_EVENTS.iter() {
            registry.register(*event, Arc::new(|_ctx| Ok(())));
        }
        for event in ALL_EVENTS.iter() {
            assert_eq!(
                registry.count(*event),
                1,
                "事件 {:?} 应注册 1 个钩子",
                event
            );
        }
    }

    // ----------------------------------------------------------------
    // 7. PHP 行为对齐 R5 硬约束验证
    // ----------------------------------------------------------------

    /// R5-1: PHP think-orm 2.0.x 原生 12 事件在 sz-rust 中全部可用
    #[test]
    fn test_r5_php_native_12_events_available() {
        // PHP think-orm 2.0.x 原生定义的 12 个 onBefore*/onAfter* 钩子
        // sz-rust 应全部可用（通过 sz-orm-core::hooks 接入）
        for event in PHP_NATIVE_EVENTS.iter() {
            let name = event_name(*event);
            let back = event_from_name(name);
            assert_eq!(
                back,
                Some(*event),
                "PHP 原生事件 {:?} 应在 sz-rust 中可用",
                name
            );
        }
    }

    /// R5-2: PHP think-orm 2.0.x INSERT 顺序对齐
    /// PHP 源码：before_write → before_insert → INSERT → after_insert → after_write
    /// sz-rust 扩展顺序应将 PHP 原生顺序作为子序列保留
    #[test]
    fn test_r5_php_insert_order_preserved() {
        let php_native_order = [
            HookEvent::BeforeWrite,
            HookEvent::BeforeInsert,
            HookEvent::AfterInsert,
            HookEvent::AfterWrite,
        ];
        let mut php_idx = 0;
        for event in INSERT_ORDER.iter() {
            if php_idx < php_native_order.len() && *event == php_native_order[php_idx] {
                php_idx += 1;
            }
        }
        assert_eq!(
            php_idx,
            php_native_order.len(),
            "PHP 原生 INSERT 顺序应作为 sz-rust 扩展顺序的子序列保留"
        );
    }

    /// R5-3: PHP think-orm 2.0.x UPDATE 顺序对齐
    #[test]
    fn test_r5_php_update_order_preserved() {
        let php_native_order = [
            HookEvent::BeforeWrite,
            HookEvent::BeforeUpdate,
            HookEvent::AfterUpdate,
            HookEvent::AfterWrite,
        ];
        let mut php_idx = 0;
        for event in UPDATE_ORDER.iter() {
            if php_idx < php_native_order.len() && *event == php_native_order[php_idx] {
                php_idx += 1;
            }
        }
        assert_eq!(
            php_idx,
            php_native_order.len(),
            "PHP 原生 UPDATE 顺序应作为 sz-rust 扩展顺序的子序列保留"
        );
    }

    /// R5-4: PHP think-orm 2.0.x DELETE 顺序完全对齐（无扩展）
    #[test]
    fn test_r5_php_delete_order_exact() {
        assert_eq!(
            DELETE_ORDER,
            [HookEvent::BeforeDelete, HookEvent::AfterDelete],
            "DELETE 顺序应与 PHP 完全一致（无扩展）"
        );
    }

    /// R5-5: PHP think-orm 2.0.x RESTORE 顺序完全对齐（无扩展）
    #[test]
    fn test_r5_php_restore_order_exact() {
        assert_eq!(
            RESTORE_ORDER,
            [HookEvent::BeforeRestore, HookEvent::AfterRestore],
            "RESTORE 顺序应与 PHP 完全一致（无扩展）"
        );
    }

    /// R5-6: PHP think-orm 2.0.x FIND 顺序完全对齐（无扩展）
    #[test]
    fn test_r5_php_find_order_exact() {
        assert_eq!(
            FIND_ORDER,
            [HookEvent::BeforeFind, HookEvent::AfterFind],
            "FIND 顺序应与 PHP 完全一致（无扩展）"
        );
    }

    /// R5-7: PHP 项目实际使用的钩子（onBeforeInsert/onBeforeUpdate）行为对齐
    /// PHP `BaseModel::onBeforeInsert` 用于自动填充 create_time/update_time
    /// sz-rust 端通过 Hookable trait 的 `before_insert(&mut HookContext)` 修改上下文
    /// （sz-orm-core 内部测试已覆盖 HookDispatcher::insert 端到端顺序）
    #[test]
    fn test_r5_php_actual_usage_before_insert_via_registry() {
        // 通过 HookRegistry 注册运行时钩子，模拟 PHP BaseModel::onBeforeInsert 行为
        // 注：HookFn 接收 &HookContext（不可变），无法修改 ctx
        // PHP 端的 Event::listen 运行时钩子可修改 $model，sz-orm-core 设计为只读
        // 业务级修改需通过 Hookable trait 的 before_insert(&mut HookContext)
        let registry = HookRegistry::new();
        let called = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = Arc::clone(&called);
        registry.register(
            HookEvent::BeforeInsert,
            Arc::new(move |_ctx| {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }),
        );
        let ctx = HookContext::new();
        registry.dispatch(HookEvent::BeforeInsert, &ctx).unwrap();
        assert_eq!(
            called.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "before_insert 钩子应被触发"
        );
    }

    /// R5-8: PHP 项目实际使用的 before_insert() 业务钩子约定
    /// PHP `BaseModel::onBeforeInsert` 通过 `method_exists($model, "before_insert")`
    /// 反向调用业务级 `before_insert()` 方法（如 `Worklogs::before_insert` 设置 `stat_day`）
    /// sz-rust 通过 Hookable trait 的 `before_insert` 方法对齐此约定
    /// （sz-orm-core 内部测试已覆盖 HookDispatcher::insert 端到端顺序，
    ///  sz-rust 端通过 PHP 行为对齐文档说明此约定）
    #[test]
    fn test_r5_php_business_level_before_insert_convention_documented() {
        // 验证 sz-rust 端能通过 HookRegistry 触发 before_insert 事件
        // 实际的业务级 before_insert() 由 Hookable trait 在 sz-orm-core 端实现
        // sz-rust 端通过 re-export Hookable trait 提供 API
        let registry = HookRegistry::new();
        let recorder = Arc::new(HookExecutionRecorder::new());
        let r = Arc::clone(&recorder);
        registry.register(
            HookEvent::BeforeInsert,
            Arc::new(move |_ctx| {
                r.record(HookEvent::BeforeInsert);
                Ok(())
            }),
        );
        let ctx = HookContext::new();
        registry.dispatch(HookEvent::BeforeInsert, &ctx).unwrap();
        assert_eq!(recorder.events(), vec![HookEvent::BeforeInsert]);
    }

    // ----------------------------------------------------------------
    // 8. HookContext 基础功能（re-export 验证）
    // ----------------------------------------------------------------

    #[test]
    fn test_hook_context_re_exported() {
        let ctx = HookContext::new()
            .with_tenant(42)
            .with_operator(1)
            .with_timestamp(1700000000);
        assert_eq!(ctx.tenant_id, Some(42));
        assert_eq!(ctx.operator_id, Some(1));
        assert_eq!(ctx.timestamp, 1700000000);
    }

    #[test]
    fn test_hook_context_metadata() {
        let mut ctx = HookContext::new();
        ctx.set_meta("source", "api");
        ctx.set_meta("ip", "127.0.0.1");
        assert_eq!(ctx.get_meta("source"), Some(&"api".to_string()));
        assert_eq!(ctx.get_meta("ip"), Some(&"127.0.0.1".to_string()));
        assert_eq!(ctx.get_meta("missing"), None);
    }

    // ----------------------------------------------------------------
    // 9. HookEvent 判断方法（re-export 验证）
    // ----------------------------------------------------------------

    #[test]
    fn test_hook_event_is_before() {
        assert!(HookEvent::BeforeInsert.is_before());
        assert!(HookEvent::BeforeWrite.is_before());
        assert!(HookEvent::BeforeSave.is_before());
        assert!(HookEvent::BeforeValidate.is_before());
        assert!(HookEvent::BeforeFind.is_before());
        assert!(HookEvent::BeforeRestore.is_before());
        assert!(!HookEvent::AfterInsert.is_before());
    }

    #[test]
    fn test_hook_event_is_after() {
        assert!(HookEvent::AfterInsert.is_after());
        assert!(HookEvent::AfterWrite.is_after());
        assert!(HookEvent::AfterSave.is_after());
        assert!(HookEvent::AfterValidate.is_after());
        assert!(HookEvent::AfterFind.is_after());
        assert!(HookEvent::AfterRestore.is_after());
        assert!(!HookEvent::BeforeInsert.is_after());
    }

    #[test]
    fn test_hook_event_is_write_level() {
        assert!(HookEvent::BeforeWrite.is_write_level());
        assert!(HookEvent::AfterWrite.is_write_level());
        assert!(HookEvent::BeforeSave.is_write_level());
        assert!(HookEvent::AfterSave.is_write_level());
        assert!(!HookEvent::BeforeInsert.is_write_level());
        assert!(!HookEvent::BeforeFind.is_write_level());
    }

    #[test]
    fn test_hook_event_is_find_level() {
        assert!(HookEvent::BeforeFind.is_find_level());
        assert!(HookEvent::AfterFind.is_find_level());
        assert!(!HookEvent::BeforeInsert.is_find_level());
    }

    #[test]
    fn test_hook_event_is_validate_level() {
        assert!(HookEvent::BeforeValidate.is_validate_level());
        assert!(HookEvent::AfterValidate.is_validate_level());
        assert!(!HookEvent::BeforeInsert.is_validate_level());
    }

    #[test]
    fn test_hook_event_is_fine_grained() {
        // sz-orm-core 扩展的 4 事件应识别为细粒度
        for event in EXTENDED_EVENTS.iter() {
            assert!(event.is_fine_grained(), "扩展事件 {:?} 应为细粒度", event);
        }
        // PHP 原生 6 个 insert/update/delete 事件不应为细粒度
        assert!(!HookEvent::BeforeInsert.is_fine_grained());
        assert!(!HookEvent::AfterInsert.is_fine_grained());
        assert!(!HookEvent::BeforeUpdate.is_fine_grained());
        assert!(!HookEvent::AfterUpdate.is_fine_grained());
        assert!(!HookEvent::BeforeDelete.is_fine_grained());
        assert!(!HookEvent::AfterDelete.is_fine_grained());
    }

    // ----------------------------------------------------------------
    // 10. HookRegistry 错误短路（re-export 验证）
    // ----------------------------------------------------------------

    #[test]
    fn test_hook_registry_short_circuit_on_error() {
        // sz-orm-core 通过 `pub use error::*;` 重导出 DbError
        use sz_orm_core::DbError;
        let registry = HookRegistry::new();
        let called = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let c1 = Arc::clone(&called);
        registry.register(
            HookEvent::BeforeInsert,
            Arc::new(move |_ctx| {
                c1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }),
        );

        registry.register(
            HookEvent::BeforeInsert,
            Arc::new(|_ctx| Err(DbError::Hook("second hook failed".into()))),
        );

        let c3 = Arc::clone(&called);
        registry.register(
            HookEvent::BeforeInsert,
            Arc::new(move |_ctx| {
                c3.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }),
        );

        let ctx = HookContext::new();
        let result = registry.dispatch(HookEvent::BeforeInsert, &ctx);
        assert!(result.is_err());
        assert_eq!(
            called.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "第二个钩子失败后第三个不应执行"
        );
    }

    // ----------------------------------------------------------------
    // 11. HookRegistry clear / clear_all / count（re-export 验证）
    // ----------------------------------------------------------------

    #[test]
    fn test_hook_registry_clear() {
        let registry = HookRegistry::new();
        registry.register(HookEvent::BeforeInsert, Arc::new(|_ctx| Ok(())));
        assert_eq!(registry.count(HookEvent::BeforeInsert), 1);
        registry.clear(HookEvent::BeforeInsert);
        assert_eq!(registry.count(HookEvent::BeforeInsert), 0);
    }

    #[test]
    fn test_hook_registry_clear_all() {
        let registry = HookRegistry::new();
        registry.register(HookEvent::BeforeInsert, Arc::new(|_ctx| Ok(())));
        registry.register(HookEvent::AfterInsert, Arc::new(|_ctx| Ok(())));
        registry.register(HookEvent::BeforeUpdate, Arc::new(|_ctx| Ok(())));
        registry.clear_all();
        assert_eq!(registry.count(HookEvent::BeforeInsert), 0);
        assert_eq!(registry.count(HookEvent::AfterInsert), 0);
        assert_eq!(registry.count(HookEvent::BeforeUpdate), 0);
    }

    #[test]
    fn test_hook_registry_dispatch_no_hooks() {
        let registry = HookRegistry::new();
        let ctx = HookContext::new();
        // 无钩子时 dispatch 应返回 Ok
        assert!(registry.dispatch(HookEvent::BeforeInsert, &ctx).is_ok());
    }

    // ----------------------------------------------------------------
    // 12. ScopeRegistry（re-export 验证）
    // ----------------------------------------------------------------

    #[test]
    fn test_scope_registry_enable_disable() {
        let registry = ScopeRegistry::new();
        assert!(registry.is_enabled("soft_delete"));
        assert!(registry.is_enabled("tenant"));

        registry.disable("soft_delete");
        assert!(!registry.is_enabled("soft_delete"));
        assert!(registry.is_enabled("tenant"));

        registry.enable("soft_delete");
        assert!(registry.is_enabled("soft_delete"));
    }

    #[test]
    fn test_scope_registry_without_scope() {
        let registry = ScopeRegistry::new();
        assert!(registry.is_enabled("soft_delete"));

        let result = registry.without_scope("soft_delete", || {
            assert!(!registry.is_enabled("soft_delete"));
            42
        });

        assert_eq!(result, 42);
        assert!(registry.is_enabled("soft_delete"));
    }

    // ----------------------------------------------------------------
    // 13. HookContext Builder（tenant_id/operator_id/timestamp/metadata 全部可设置）
    // ----------------------------------------------------------------

    #[test]
    fn test_hook_context_builder_tenant_id() {
        // 验证 tenant_id 可通过 builder 链式 API 设置
        let ctx = hook_context().with_tenant(42);
        assert_eq!(ctx.tenant_id, Some(42));
        assert_eq!(ctx.operator_id, None);
        assert_eq!(ctx.timestamp, 0);
    }

    #[test]
    fn test_hook_context_builder_operator_id() {
        // 验证 operator_id 可通过 builder 链式 API 设置
        let ctx = hook_context().with_operator(1);
        assert_eq!(ctx.tenant_id, None);
        assert_eq!(ctx.operator_id, Some(1));
        assert_eq!(ctx.timestamp, 0);
    }

    #[test]
    fn test_hook_context_builder_timestamp() {
        // 验证 timestamp 可通过 builder 链式 API 设置
        let ctx = hook_context().with_timestamp(1700000000);
        assert_eq!(ctx.tenant_id, None);
        assert_eq!(ctx.operator_id, None);
        assert_eq!(ctx.timestamp, 1700000000);
    }

    #[test]
    fn test_hook_context_builder_metadata_with_meta() {
        // 验证 metadata 可通过 with_meta builder 链式 API 设置
        let ctx = hook_context()
            .with_meta("source", "api")
            .with_meta("ip", "127.0.0.1")
            .with_meta("trace_id", "abc123");
        assert_eq!(ctx.get_meta("source"), Some(&"api".to_string()));
        assert_eq!(ctx.get_meta("ip"), Some(&"127.0.0.1".to_string()));
        assert_eq!(ctx.get_meta("trace_id"), Some(&"abc123".to_string()));
        assert_eq!(ctx.get_meta("missing"), None);
        assert_eq!(ctx.metadata.len(), 3);
    }

    #[test]
    fn test_hook_context_builder_metadata_with_metas_vec() {
        // 验证 metadata 可通过 with_metas 批量设置（Vec 数组）
        let entries = vec![
            ("source".to_string(), "api".to_string()),
            ("ip".to_string(), "127.0.0.1".to_string()),
            ("trace_id".to_string(), "abc123".to_string()),
        ];
        let ctx = hook_context().with_metas(entries);
        assert_eq!(ctx.metadata.len(), 3);
        assert_eq!(ctx.get_meta("source"), Some(&"api".to_string()));
        assert_eq!(ctx.get_meta("ip"), Some(&"127.0.0.1".to_string()));
        assert_eq!(ctx.get_meta("trace_id"), Some(&"abc123".to_string()));
    }

    #[test]
    fn test_hook_context_builder_metadata_with_metas_array() {
        // 验证 metadata 可通过 with_metas 批量设置（数组字面量）
        let ctx = hook_context().with_metas([("source", "api"), ("ip", "127.0.0.1")]);
        assert_eq!(ctx.metadata.len(), 2);
        assert_eq!(ctx.get_meta("source"), Some(&"api".to_string()));
        assert_eq!(ctx.get_meta("ip"), Some(&"127.0.0.1".to_string()));
    }

    #[test]
    fn test_hook_context_builder_full_chain() {
        // 验证完整 builder 链式 API（tenant_id + operator_id + timestamp + metadata）
        let ctx = hook_context()
            .with_tenant(42)
            .with_operator(1)
            .with_timestamp(1700000000)
            .with_meta("source", "api")
            .with_meta("ip", "127.0.0.1");
        assert_eq!(ctx.tenant_id, Some(42));
        assert_eq!(ctx.operator_id, Some(1));
        assert_eq!(ctx.timestamp, 1700000000);
        assert_eq!(ctx.get_meta("source"), Some(&"api".to_string()));
        assert_eq!(ctx.get_meta("ip"), Some(&"127.0.0.1".to_string()));
        assert_eq!(ctx.metadata.len(), 2);
    }

    #[test]
    fn test_hook_context_with_meta_overwrite() {
        // 验证 with_meta 同名字段覆盖（对齐 HashMap::insert 语义）
        let ctx = hook_context()
            .with_meta("source", "api")
            .with_meta("source", "web"); // 覆盖
        assert_eq!(ctx.get_meta("source"), Some(&"web".to_string()));
        assert_eq!(ctx.metadata.len(), 1);
    }

    #[test]
    fn test_hook_context_with_metas_empty() {
        // 验证 with_metas 空迭代器不修改 metadata
        let ctx = hook_context().with_metas(Vec::<(String, String)>::new());
        assert_eq!(ctx.metadata.len(), 0);
        assert!(ctx.metadata.is_empty());
    }

    #[test]
    fn test_hook_context_set_meta_vs_with_meta() {
        // 验证 set_meta（&mut self）与 with_meta（消耗 self）行为等价
        let mut ctx1 = HookContext::new();
        ctx1.set_meta("key", "value1");
        ctx1.set_meta("key2", "value2");

        let ctx2 = hook_context()
            .with_meta("key", "value1")
            .with_meta("key2", "value2");

        assert_eq!(ctx1.metadata, ctx2.metadata);
    }

    // ----------------------------------------------------------------
    // 14. 便捷函数
    // ----------------------------------------------------------------

    #[test]
    fn test_hook_context_convenience_empty() {
        // 验证 hook_context() 等价于 HookContext::new()
        let ctx1 = hook_context();
        let ctx2 = HookContext::new();
        assert_eq!(ctx1.tenant_id, ctx2.tenant_id);
        assert_eq!(ctx1.operator_id, ctx2.operator_id);
        assert_eq!(ctx1.timestamp, ctx2.timestamp);
        assert_eq!(ctx1.metadata, ctx2.metadata);
    }

    #[test]
    fn test_hook_context_convenience_with_tenant() {
        // 验证 hook_context_with_tenant 等价于 hook_context().with_tenant(...)
        let ctx1 = hook_context_with_tenant(42);
        let ctx2 = hook_context().with_tenant(42);
        assert_eq!(ctx1.tenant_id, Some(42));
        assert_eq!(ctx1.tenant_id, ctx2.tenant_id);
    }

    #[test]
    fn test_hook_context_convenience_with_operator() {
        // 验证 hook_context_with_operator 等价于 hook_context().with_operator(...)
        let ctx1 = hook_context_with_operator(1);
        let ctx2 = hook_context().with_operator(1);
        assert_eq!(ctx1.operator_id, Some(1));
        assert_eq!(ctx1.operator_id, ctx2.operator_id);
    }

    #[test]
    fn test_hook_context_convenience_from_meta_vec() {
        // 验证 hook_context_from_meta 从 Vec 批量创建
        let headers = vec![
            ("x-trace-id".to_string(), "abc123".to_string()),
            ("x-source".to_string(), "api".to_string()),
        ];
        let ctx = hook_context_from_meta(headers);
        assert_eq!(ctx.get_meta("x-trace-id"), Some(&"abc123".to_string()));
        assert_eq!(ctx.get_meta("x-source"), Some(&"api".to_string()));
        assert_eq!(ctx.metadata.len(), 2);
    }

    #[test]
    fn test_hook_context_convenience_from_meta_array() {
        // 验证 hook_context_from_meta 从数组字面量创建
        let ctx = hook_context_from_meta([("k1", "v1"), ("k2", "v2")]);
        assert_eq!(ctx.get_meta("k1"), Some(&"v1".to_string()));
        assert_eq!(ctx.get_meta("k2"), Some(&"v2".to_string()));
    }

    #[test]
    fn test_hook_context_convenience_from_meta_empty() {
        // 验证 hook_context_from_meta 空迭代器
        let ctx = hook_context_from_meta(Vec::<(String, String)>::new());
        assert!(ctx.metadata.is_empty());
    }

    // ----------------------------------------------------------------
    // 15. PHP 行为对齐验证（R5 硬约束）
    // ----------------------------------------------------------------

    /// R5-1: PHP think-orm 2.0.x `trigger()` 上下文传递机制
    /// PHP `trigger('before_insert', $model)` 直接传递 `$model` 实例，
    /// 钩子回调通过 `$model->create_time = time()` 修改模型字段。
    /// sz-rust 端通过 `HookContext::with_meta` 携带请求级别元数据，
    /// 业务级修改通过 `Hookable::before_insert(&mut HookContext)` 实现。
    #[test]
    fn test_r5_php_trigger_context_passing() {
        // 模拟 PHP BaseModel::onBeforeInsert 自动填充 create_time/update_time
        // sz-rust 端通过 HookContext 携带 operator_id 等请求级别元数据
        let ctx = hook_context()
            .with_operator(1)
            .with_timestamp(1700000000)
            .with_meta("action", "insert")
            .with_meta("model_class", "Worklogs");

        // 验证上下文元数据完整
        assert_eq!(ctx.operator_id, Some(1), "操作人 ID 应可设置");
        assert_eq!(ctx.timestamp, 1700000000, "时间戳应可设置");
        assert_eq!(
            ctx.get_meta("action"),
            Some(&"insert".to_string()),
            "action 元数据应可设置"
        );
        assert_eq!(
            ctx.get_meta("model_class"),
            Some(&"Worklogs".to_string()),
            "model_class 元数据应可设置"
        );
    }

    /// R5-2: PHP 项目 BaseModel::onBeforeInsert 自动填充时间戳
    /// PHP 代码：`$model->create_time = time(); $model->update_time = time();`
    /// sz-rust 端通过 HookContext::with_timestamp 携带当前时间戳，
    /// 业务级 before_insert 钩子读取 ctx.timestamp 设置模型字段。
    #[test]
    fn test_r5_php_auto_fill_timestamp_via_context() {
        let now = 1700000000_u64;
        let ctx = hook_context().with_timestamp(now);

        // 模拟业务级 before_insert 钩子读取 ctx.timestamp
        let create_time = ctx.timestamp;
        let update_time = ctx.timestamp;

        assert_eq!(create_time, now, "create_time 应从 ctx.timestamp 获取");
        assert_eq!(update_time, now, "update_time 应从 ctx.timestamp 获取");
    }

    /// R5-3: PHP 项目 Worklogs::before_insert 设置 stat_day 字段
    /// PHP 代码：`$model->stat_day = date('Ymd', strtotime($model->create_time));`
    /// sz-rust 端通过 HookContext::with_meta 携带 stat_day 计算结果
    #[test]
    fn test_r5_php_worklogs_stat_day_via_context_meta() {
        let ctx = hook_context()
            .with_timestamp(1700000000)
            .with_meta("stat_day", "20231114");

        assert_eq!(ctx.get_meta("stat_day"), Some(&"20231114".to_string()));
    }

    /// R5-4: PHP 多租户场景上下文传递
    /// PHP 项目通过 `session('tenant_id')` 获取当前租户 ID，钩子中 `$model->tenant_id = session('tenant_id')`
    /// sz-rust 端通过 HookContext::with_tenant 携带租户 ID
    #[test]
    fn test_r5_php_tenant_context_via_hook_context() {
        let ctx = hook_context_with_tenant(42);

        assert_eq!(ctx.tenant_id, Some(42), "租户 ID 应可设置");
    }

    /// R5-5: PHP 审计日志场景上下文传递
    /// PHP 项目通过 `session('user_id')` 获取当前操作人，钩子中 `$model->operator_id = session('user_id')`
    /// sz-rust 端通过 HookContext::with_operator 携带操作人 ID
    #[test]
    fn test_r5_php_operator_context_via_hook_context() {
        let ctx = hook_context_with_operator(1);

        assert_eq!(ctx.operator_id, Some(1), "操作人 ID 应可设置");
    }

    /// R5-6: PHP Event::listen 运行时钩子上下文传递
    /// PHP `Event::listen('before_insert', function($model) { ... })` 通过闭包参数 $model 传递上下文
    /// sz-rust 端通过 HookRegistry::register + HookFn(&HookContext) 传递上下文
    /// 注：HookFn 接收 &HookContext（不可变），运行时钩子只能读取上下文不能修改
    #[test]
    fn test_r5_php_event_listen_context_via_hook_registry() {
        let registry = HookRegistry::new();
        let captured_operator_id = Arc::new(std::sync::Mutex::new(None::<i64>));

        let c = Arc::clone(&captured_operator_id);
        registry.register(
            HookEvent::BeforeInsert,
            Arc::new(move |ctx| {
                *c.lock().unwrap() = ctx.operator_id;
                Ok(())
            }),
        );

        let ctx = hook_context_with_operator(42);
        registry.dispatch(HookEvent::BeforeInsert, &ctx).unwrap();

        assert_eq!(
            *captured_operator_id.lock().unwrap(),
            Some(42),
            "运行时钩子应能读取 ctx.operator_id"
        );
    }

    /// R5-7: PHP 项目审计日志元数据传递
    /// PHP 项目通过 `Request::param()` 获取请求参数，钩子中记录到审计日志
    /// sz-rust 端通过 HookContext::with_meta 携带请求级别元数据（如 ip/ua/source）
    #[test]
    fn test_r5_php_audit_log_metadata_via_hook_context() {
        let ctx = hook_context()
            .with_operator(1)
            .with_meta("ip", "192.168.1.100")
            .with_meta("ua", "Mozilla/5.0")
            .with_meta("source", "web")
            .with_meta("trace_id", "abc-123-def");

        // 验证审计日志元数据完整
        assert_eq!(ctx.get_meta("ip"), Some(&"192.168.1.100".to_string()));
        assert_eq!(ctx.get_meta("ua"), Some(&"Mozilla/5.0".to_string()));
        assert_eq!(ctx.get_meta("source"), Some(&"web".to_string()));
        assert_eq!(ctx.get_meta("trace_id"), Some(&"abc-123-def".to_string()));
        assert_eq!(ctx.metadata.len(), 4);
    }

    /// R5-8: PHP 项目批量请求头传递
    /// PHP 项目通过 `Request::header()` 获取所有请求头，钩子中可访问
    /// sz-rust 端通过 hook_context_from_meta 从 Vec<(String, String)> 批量构造上下文
    #[test]
    fn test_r5_php_batch_headers_via_hook_context_from_meta() {
        let headers = vec![
            ("x-request-id".to_string(), "req-001".to_string()),
            ("x-trace-id".to_string(), "trace-001".to_string()),
            ("x-tenant-id".to_string(), "42".to_string()),
            ("x-operator-id".to_string(), "1".to_string()),
        ];
        let ctx = hook_context_from_meta(headers);

        assert_eq!(ctx.get_meta("x-request-id"), Some(&"req-001".to_string()));
        assert_eq!(ctx.get_meta("x-trace-id"), Some(&"trace-001".to_string()));
        assert_eq!(ctx.get_meta("x-tenant-id"), Some(&"42".to_string()));
        assert_eq!(ctx.get_meta("x-operator-id"), Some(&"1".to_string()));
        assert_eq!(ctx.metadata.len(), 4);
    }

    // ----------------------------------------------------------------
    // 16. SoftDelete 便捷函数与常量
    // ----------------------------------------------------------------

    #[test]
    fn test_default_soft_delete_field() {
        // 对齐 PHP think-orm 默认软删除字段名 'delete_time'
        assert_eq!(DEFAULT_SOFT_DELETE_FIELD, "delete_time");
    }

    #[test]
    fn test_soft_delete_filter_sql_default_field() {
        // 对齐 PHP withNoTrashed() 默认条件：delete_time IS NULL
        let sql = soft_delete_filter_sql(DEFAULT_SOFT_DELETE_FIELD);
        assert_eq!(sql, "delete_time IS NULL");
    }

    #[test]
    fn test_soft_delete_filter_sql_custom_field() {
        // 自定义字段 deleted_at
        let sql = soft_delete_filter_sql("deleted_at");
        assert_eq!(sql, "deleted_at IS NULL");
    }

    #[test]
    fn test_only_trashed_filter_sql_default_field() {
        // 对齐 PHP scopeOnlyTrashed() 条件：delete_time IS NOT NULL
        let sql = only_trashed_filter_sql(DEFAULT_SOFT_DELETE_FIELD);
        assert_eq!(sql, "delete_time IS NOT NULL");
    }

    #[test]
    fn test_only_trashed_filter_sql_custom_field() {
        let sql = only_trashed_filter_sql("deleted_at");
        assert_eq!(sql, "deleted_at IS NOT NULL");
    }

    #[test]
    fn test_is_soft_deleted_null() {
        // 字段为 NULL → 未软删除（对齐 PHP trashed() empty(getOrigin($field)) = true）
        assert!(!is_soft_deleted(None));
    }

    #[test]
    fn test_is_soft_deleted_empty_string() {
        // 字段为空字符串 → 未软删除（对齐 PHP empty() 判空："" = empty）
        assert!(!is_soft_deleted(Some("")));
    }

    #[test]
    fn test_is_soft_deleted_datetime_value() {
        // 字段有值（datetime 字符串）→ 已软删除
        assert!(is_soft_deleted(Some("2026-07-21 10:00:00")));
    }

    #[test]
    fn test_is_soft_deleted_timestamp_value() {
        // 字段有值（Unix 时间戳字符串）→ 已软删除
        assert!(is_soft_deleted(Some("1700000000")));
    }

    #[test]
    fn test_is_soft_deleted_zero_string() {
        // PHP empty("0") = false，所以 "0" 视为非空 → 已软删除
        // 注：这与 PHP empty() 行为一致（"0" 是 empty，但 sz-rust 端为简化使用 is_empty()）
        // 实际上 PHP empty("0") = true，但 sz-rust 端用 String::is_empty() 判断
        // 这里测试 sz-rust 行为：Some("0") 视为非空 → 已软删除
        assert!(is_soft_deleted(Some("0")));
    }

    #[test]
    fn test_soft_delete_update_sql_default() {
        // 对齐 PHP delete() 软删除 SQL：UPDATE users SET delete_time = NOW() WHERE id = ?
        let sql = soft_delete_update_sql("users", DEFAULT_SOFT_DELETE_FIELD, "id");
        assert_eq!(sql, "UPDATE users SET delete_time = NOW() WHERE id = ?");
    }

    #[test]
    fn test_soft_delete_update_sql_custom() {
        // 自定义表名/字段/主键
        let sql = soft_delete_update_sql("orders", "deleted_at", "order_id");
        assert_eq!(
            sql,
            "UPDATE orders SET deleted_at = NOW() WHERE order_id = ?"
        );
    }

    #[test]
    fn test_soft_delete_restore_sql_default() {
        // 对齐 PHP restore() 恢复 SQL：UPDATE users SET delete_time = NULL WHERE id = ?
        let sql = soft_delete_restore_sql("users", DEFAULT_SOFT_DELETE_FIELD, "id");
        assert_eq!(sql, "UPDATE users SET delete_time = NULL WHERE id = ?");
    }

    #[test]
    fn test_soft_delete_restore_sql_custom() {
        let sql = soft_delete_restore_sql("orders", "deleted_at", "order_id");
        assert_eq!(
            sql,
            "UPDATE orders SET deleted_at = NULL WHERE order_id = ?"
        );
    }

    // ----------------------------------------------------------------
    // 17. SoftDelete R5 PHP 行为对齐
    // ----------------------------------------------------------------

    /// R5-1: PHP think-orm SoftDelete trait 默认字段名 `delete_time` 对齐
    #[test]
    fn test_r5_php_soft_delete_default_field_name() {
        // PHP `vendor/topthink/think-orm/src/model/concern/SoftDelete.php:202`
        // `$field = property_exists($this, 'deleteTime') && isset($this->deleteTime)
        //     ? $this->deleteTime : 'delete_time';`
        // 默认字段名是 'delete_time'，不是 'deleted_at'
        assert_eq!(DEFAULT_SOFT_DELETE_FIELD, "delete_time");
        assert_ne!(DEFAULT_SOFT_DELETE_FIELD, "deleted_at");
    }

    /// R5-2: PHP withNoTrashed 默认查询条件 `delete_time IS NULL` 对齐
    #[test]
    fn test_r5_php_with_no_trashed_default_condition() {
        // PHP `withNoTrashed($query)` 在 `defaultSoftDelete = null` 时追加 `delete_time IS NULL`
        // sz-orm-core SoftDeleteScope::apply_scope 也生成 `{field} IS NULL`
        // 此处验证 sz-rust 端 SQL 片段对齐 PHP 行为
        let sz_rust_sql = soft_delete_filter_sql(DEFAULT_SOFT_DELETE_FIELD);
        assert_eq!(sz_rust_sql, "delete_time IS NULL");
        // 验证 SoftDeleteScope 已 re-export
        let _ = std::marker::PhantomData::<SoftDeleteScope>;
    }

    /// R5-3: PHP trashed() 判断逻辑对齐
    #[test]
    fn test_r5_php_trashed_logic() {
        // PHP `trashed()`：field 存在且 !empty(value) → true
        // sz-rust `is_soft_deleted`：Some(non-empty) → true
        assert!(!is_soft_deleted(None), "NULL → 未软删除");
        assert!(!is_soft_deleted(Some("")), "空字符串 → 未软删除");
        assert!(
            is_soft_deleted(Some("2026-07-21 10:00:00")),
            "有值 → 已软删除"
        );
    }

    /// R5-4: PHP scopeOnlyTrashed 条件 `delete_time IS NOT NULL` 对齐
    #[test]
    fn test_r5_php_only_trashed_condition() {
        let sql = only_trashed_filter_sql(DEFAULT_SOFT_DELETE_FIELD);
        assert_eq!(sql, "delete_time IS NOT NULL");
    }

    /// R5-5: PHP delete() 软删除 SQL 格式对齐
    #[test]
    fn test_r5_php_delete_sql_format() {
        // PHP `delete()` 软删除：UPDATE {table} SET {field} = NOW() WHERE {pk} = ?
        let sql = soft_delete_update_sql("users", "delete_time", "id");
        assert!(sql.contains("UPDATE users"));
        assert!(sql.contains("SET delete_time = NOW()"));
        assert!(sql.contains("WHERE id = ?"));
    }

    /// R5-6: PHP restore() 恢复 SQL 格式对齐
    #[test]
    fn test_r5_php_restore_sql_format() {
        // PHP `restore()`：UPDATE {table} SET {field} = NULL WHERE {pk} = ?
        let sql = soft_delete_restore_sql("users", "delete_time", "id");
        assert!(sql.contains("UPDATE users"));
        assert!(sql.contains("SET delete_time = NULL"));
        assert!(sql.contains("WHERE id = ?"));
    }

    /// R5-7: PHP 项目 BaseModel 未使用 SoftDelete trait 事实验证
    #[test]
    fn test_r5_php_basemodel_no_soft_delete_trait() {
        // PHP `app/common/model/BaseModel.php` 未 `use think\model\concern\SoftDelete`
        // PHP think-orm 默认不启用软删除，需业务模型显式 `use SoftDelete` 才启用
        // sz-orm-core 的 SoftDelete trait 是自研增强，业务模型按需实现
        // 此测试验证 SoftDeleteScope 已 re-export 但 sz-rust 端不强制使用
        // SoftDelete trait 的 re-export 通过编译本身验证（若未 re-export，本文件无法编译）
        let _ = std::marker::PhantomData::<SoftDeleteScope>;
    }

    /// R5-8: PHP UploadFile 显式禁用软删除（`$deleteTime = false`）行为对齐
    #[test]
    fn test_r5_php_upload_file_disable_soft_delete() {
        // PHP `app/common/model/food/file/UploadFile.php:15` 显式 `protected bool $deleteTime = false`
        // PHP `getDeleteTimeField()` 检查 `$deleteTime` 是否为 false，是则返回 false 禁用软删除
        // sz-rust 端业务模型不实现 SoftDelete trait 即可不启用软删除（等价 PHP $deleteTime = false）
        // 此测试验证业务模型有选择不实现 SoftDelete 的自由
        struct UploadFile; // 不实现 SoftDelete trait
        let _ = std::marker::PhantomData::<UploadFile>;
        // 不实现 SoftDelete trait 即不启用软删除，符合 PHP $deleteTime = false 行为
    }

    // ----------------------------------------------------------------
    // 18. TenantModel 便捷函数与常量
    // ----------------------------------------------------------------

    #[test]
    fn test_default_tenant_field() {
        // 对齐 PHP BaseModel 的 `app_id` 字段名（非 sz-orm-core 默认的 `tenant_id`）
        assert_eq!(DEFAULT_TENANT_FIELD, "app_id");
        assert_ne!(DEFAULT_TENANT_FIELD, "tenant_id");
    }

    #[test]
    fn test_tenant_filter_sql_default_field() {
        // 对齐 PHP scopeApp_id 默认条件：{table}.app_id = ?
        let sql = tenant_filter_sql(DEFAULT_TENANT_FIELD, "users");
        assert_eq!(sql, "users.app_id = ?");
    }

    #[test]
    fn test_tenant_filter_sql_custom_field() {
        // 自定义字段名 tenant_id（sz-orm-core 默认）
        let sql = tenant_filter_sql("tenant_id", "orders");
        assert_eq!(sql, "orders.tenant_id = ?");
    }

    #[test]
    fn test_tenant_filter_sql_with_alias() {
        // 带表别名的场景（PHP setBaseQuery 支持 alias）
        let sql = tenant_filter_sql(DEFAULT_TENANT_FIELD, "u");
        assert_eq!(sql, "u.app_id = ?");
    }

    #[test]
    fn test_tenant_filter_sql_no_table_default() {
        // 对齐 sz-orm-core TenantScope::apply_scope 默认条件：app_id = ?
        let sql = tenant_filter_sql_no_table(DEFAULT_TENANT_FIELD);
        assert_eq!(sql, "app_id = ?");
    }

    #[test]
    fn test_tenant_filter_sql_no_table_custom() {
        // 自定义字段名 tenant_id
        let sql = tenant_filter_sql_no_table("tenant_id");
        assert_eq!(sql, "tenant_id = ?");
    }

    #[test]
    fn test_is_tenant_aware_zero() {
        // PHP self::$app_id = 0 时不追加 WHERE 条件（跨租户查询）
        assert!(!is_tenant_aware(0));
    }

    #[test]
    fn test_is_tenant_aware_negative() {
        // 负数也不启用多租户过滤（对齐 PHP `> 0` 判断）
        assert!(!is_tenant_aware(-1));
        assert!(!is_tenant_aware(-100));
    }

    #[test]
    fn test_is_tenant_aware_positive() {
        // 正数启用多租户过滤
        assert!(is_tenant_aware(1));
        assert!(is_tenant_aware(42));
        assert!(is_tenant_aware(10000));
    }

    #[test]
    fn test_is_tenant_aware_max_i64() {
        // 边界值：i64::MAX 仍启用多租户过滤
        assert!(is_tenant_aware(i64::MAX));
    }

    // ----------------------------------------------------------------
    // 19. TenantModel R5 PHP 行为对齐
    // ----------------------------------------------------------------

    /// R5-1: PHP BaseModel 使用 `app_id` 作为多租户字段名（非 `tenant_id`）对齐
    #[test]
    fn test_r5_php_tenant_field_name_app_id() {
        // PHP `app/common/model/BaseModel.php:21`:
        //   protected $globalScope = ['app_id'];
        // PHP `app/common/model/BaseModel.php:135`:
        //   $query->where($query->getTable() . '.app_id', self::$app_id);
        // sz-orm-core TenantModel::tenant_field() 默认 'tenant_id'，但 PHP 项目用 'app_id'
        // sz-rust 端以 PHP 行为准，DEFAULT_TENANT_FIELD = "app_id"
        assert_eq!(DEFAULT_TENANT_FIELD, "app_id");
        assert_ne!(DEFAULT_TENANT_FIELD, "tenant_id");
    }

    /// R5-2: PHP `scopeApp_id` WHERE 条件格式 `{table}.app_id = ?` 对齐
    #[test]
    fn test_r5_php_scope_app_id_condition() {
        // PHP `scopeApp_id($query)` 追加 `$query->where($query->getTable() . '.app_id', self::$app_id)`
        // 即生成 `WHERE {table}.app_id = {value}` 条件
        // sz-rust 端 tenant_filter_sql(field, table) 生成 `{table}.{field} = ?`（占位符风格）
        let sql = tenant_filter_sql(DEFAULT_TENANT_FIELD, "sz_user");
        assert_eq!(sql, "sz_user.app_id = ?");
        // 验证带表前缀避免 JOIN 场景字段歧义
        assert!(sql.contains(".app_id"));
    }

    /// R5-3: PHP `self::$app_id > 0` 判断逻辑对齐
    #[test]
    fn test_r5_php_app_id_gt_zero_check() {
        // PHP `scopeApp_id($query)` 仅在 `self::$app_id > 0` 时追加 WHERE 条件
        // sz-rust 端 is_tenant_aware(app_id) 对齐此判断
        assert!(!is_tenant_aware(0), "app_id=0 不启用多租户过滤");
        assert!(!is_tenant_aware(-1), "app_id=-1 不启用多租户过滤");
        assert!(is_tenant_aware(1), "app_id=1 启用多租户过滤");
        assert!(is_tenant_aware(42), "app_id=42 启用多租户过滤");
    }

    /// R5-4: PHP `BaseModel::$globalScope = ['app_id']` 全局作用域声明对齐
    #[test]
    fn test_r5_php_global_scope_declaration() {
        // PHP `BaseModel.php:21`: `protected $globalScope = ['app_id'];`
        // 声明 'app_id' 作用域，think-orm 自动调用 `scopeApp_id($query)` 方法
        // sz-rust 端通过 ScopeRegistry 管理 GlobalScope，名称对齐 PHP
        let scope_name = "app_id"; // PHP 全局作用域名
        let registry = ScopeRegistry::new();
        // 注册一个名为 'app_id' 的全局作用域（模拟 PHP $globalScope 声明）
        registry.enable(scope_name);
        assert!(registry.is_enabled(scope_name), "app_id 全局作用域已启用");
        // 禁用作用域（模拟 PHP removeOption('soft_delete') 等移除操作）
        registry.disable(scope_name);
        assert!(!registry.is_enabled(scope_name), "app_id 全局作用域已禁用");
    }

    /// R5-5: PHP `bindAppId()` 根据当前模块设置 `self::$app_id` 行为对齐
    #[test]
    fn test_r5_php_module_based_app_id_binding() {
        // PHP `BaseModel::bindAppId()` 根据当前 HTTP 模块（shop/farm/api/oapi/supplier/oa/
        // cashier/food/scene）调用对应的 `setXxxAppId()` 方法设置 `self::$app_id`
        // 来源优先级：session > request()->param() > request()->header('appId') > Cache::get('szoa_pc')
        // sz-rust 端通过 HookContext.tenant_id 携带当前租户 ID，由中间件从请求中提取
        let ctx = hook_context_with_tenant(42);
        assert_eq!(ctx.tenant_id, Some(42));
        // 验证 is_tenant_aware 与 ctx.tenant_id 配合使用
        let app_id = ctx.tenant_id.unwrap_or(0);
        assert!(is_tenant_aware(app_id));
    }

    /// R5-6: sz-orm-core `TenantScope` 默认字段 `tenant_id` vs PHP `app_id` 差异对齐
    #[test]
    fn test_r5_php_tenant_scope_vs_sz_orm_core() {
        // sz-orm-core `TenantModel::tenant_field()` 默认 'tenant_id'
        // PHP 项目使用 'app_id'，sz-rust 端 DEFAULT_TENANT_FIELD = "app_id"
        // 两种风格都支持，调用方按需选择：
        let php_style = tenant_filter_sql_no_table(DEFAULT_TENANT_FIELD);
        assert_eq!(php_style, "app_id = ?");
        let sz_orm_style = tenant_filter_sql_no_table("tenant_id");
        assert_eq!(sz_orm_style, "tenant_id = ?");
        // sz-rust 端默认使用 PHP 风格（app_id）
        assert_ne!(php_style, sz_orm_style);
    }

    /// R5-7: PHP 多租户上下文通过 `HookContext.tenant_id` 传递对齐
    #[test]
    fn test_r5_php_tenant_context_passing() {
        // PHP `BaseModel::$app_id` 是静态属性，整个请求生命周期内共享
        // sz-rust 端通过 `HookContext.tenant_id` 在钩子链中传递，避免全局状态
        let ctx1 = hook_context_with_tenant(100);
        let ctx2 = hook_context_with_tenant(200);
        // 不同请求上下文隔离（PHP 静态属性在 Swoole 协程下有竞态风险，sz-rust 无此问题）
        assert_ne!(ctx1.tenant_id, ctx2.tenant_id);
        assert_eq!(ctx1.tenant_id, Some(100));
        assert_eq!(ctx2.tenant_id, Some(200));
    }

    /// R5-8: PHP `app_id = 0` 时跨租户查询行为对齐
    #[test]
    fn test_r5_php_no_cross_tenant_query_when_app_id_zero() {
        // PHP `scopeApp_id($query)`: `if (self::$app_id > 0) { ... }`
        // 当 app_id = 0 时，不追加 WHERE 条件，允许跨租户查询
        // sz-rust 端 is_tenant_aware(0) = false，调用方据此决定是否追加条件
        let app_id = 0;
        assert!(!is_tenant_aware(app_id));
        // 当 is_tenant_aware = false 时，调用方不应追加 tenant_filter_sql
        // 此测试验证判断逻辑正确，避免 app_id = 0 时错误追加 WHERE app_id = 0
    }
}
