// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Event 静态门面（T005）
//!
//! 对齐 PHP `think\facade\Event` 静态外观模式，提供 struct-based API。
//! 委托全局 `OnceCell<EventDispatcher>` 单例，零开销转发。
//!
//! ## 与 `event::facade` 模块的关系
//!
//! `event::facade` 模块提供自由函数 API（`facade::listen(...)`），
//! `Event` struct 提供面向对象 API（`Event::listen(...)`），两者委托同一单例。
//!
//! ## 用法
//!
//! ```ignore
//! use sz_rust_state_facade::event_facade::Event;
//! use sz_rust_state_facade::event::{ClosureListener, DispatchMode};
//! use serde_json::json;
//!
//! Event::listen("UserLogin", Arc::new(ClosureListener::new(|_| Ok(json!("ok")))), false);
//! # async fn example() {
//! let result = Event::dispatch("UserLogin", &json!({"user_id": 123}), DispatchMode::Sync).await.unwrap();
//! assert!(result.is_success());
//! # }
//! ```

use std::sync::{Arc, OnceLock};

use serde_json::Value;

use crate::event::{
    DispatchMode, DispatchResult, EventDispatcher, EventError, Listener, Observer, Subscriber,
};

/// 全局事件分发器单例
static GLOBAL_DISPATCHER: OnceLock<EventDispatcher> = OnceLock::new();

/// Event 静态门面（T005）
///
/// 对齐 PHP `think\facade\Event`，通过静态方法委托全局 `EventDispatcher` 单例。
/// 所有方法零开销转发，不持有任何状态。
pub struct Event;

impl Event {
    /// 获取全局分发器引用
    fn dispatcher() -> &'static EventDispatcher {
        GLOBAL_DISPATCHER.get_or_init(EventDispatcher::new)
    }

    /// 注册事件监听（对齐 PHP `Event::listen(...)`）
    pub fn listen(event: &str, listener: Arc<dyn Listener>, first: bool) {
        Self::dispatcher().listen(event, listener, first);
    }

    /// 注册事件监听器并指定优先级（T002）
    pub fn listen_with_priority(event: &str, listener: Arc<dyn Listener>, priority: i32) {
        Self::dispatcher().listen_with_priority(event, listener, priority);
    }

    /// 批量注册事件监听（对齐 PHP `Event::listenEvents(...)`）
    pub fn listen_events(events: Vec<(String, Vec<Arc<dyn Listener>>)>) {
        Self::dispatcher().listen_events(events);
    }

    /// 是否存在事件监听（对齐 PHP `Event::hasListener(...)`）
    pub fn has_listener(event: &str) -> bool {
        Self::dispatcher().has_listener(event)
    }

    /// 移除事件监听（对齐 PHP `Event::remove(...)`）
    pub fn remove(event: &str) {
        Self::dispatcher().remove(event);
    }

    /// 指定事件别名（对齐 PHP `Event::bind(...)`）
    pub fn bind(events: Vec<(String, String)>) {
        Self::dispatcher().bind(events);
    }

    /// 注册事件订阅者（对齐 PHP `Event::subscribe(...)`）
    pub fn subscribe(subscriber: Arc<dyn Subscriber>) {
        Self::dispatcher().subscribe(subscriber);
    }

    /// 自动注册事件观察者（对齐 PHP `Event::observe(...)`）
    pub fn observe(observer: Arc<dyn Observer>, prefix: &str) {
        Self::dispatcher().observe(observer, prefix);
    }

    /// 触发事件（对齐 PHP `Event::trigger(...)`）
    pub fn trigger(event: &str, params: &Value, once: bool) -> Result<Vec<Value>, EventError> {
        Self::dispatcher().trigger(event, params, once)
    }

    /// 分发事件（T001）
    ///
    /// 统一入口，根据 `mode` 选择同步或异步分发策略。
    /// 内置循环检测（T003）和 panic 兜底（T004）。
    pub async fn dispatch(
        event: &str,
        params: &Value,
        mode: DispatchMode,
    ) -> Result<DispatchResult, EventError> {
        Self::dispatcher().dispatch(event, params, mode).await
    }

    /// 同步分发事件（T001 + T003 + T004）
    ///
    /// 非异步版本的 `dispatch`，供监听器内同步重入调用。
    pub fn dispatch_sync(event: &str, params: &Value) -> Result<DispatchResult, EventError> {
        Self::dispatcher().dispatch_sync(event, params)
    }

    /// 触发事件（只获取一个有效返回值）（对齐 PHP `Event::until(...)`）
    pub fn until(event: &str, params: &Value) -> Result<Vec<Value>, EventError> {
        Self::dispatcher().until(event, params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ClosureListener;
    use serde_json::json;

    #[tokio::test]
    async fn test_event_facade_listen_and_dispatch_sync() {
        Event::listen(
            "FacadeTestSync",
            Arc::new(ClosureListener::new(|_| Ok(json!("sync_result")))),
            false,
        );

        let result = Event::dispatch("FacadeTestSync", &Value::Null, DispatchMode::Sync)
            .await
            .unwrap();
        assert!(result.is_success());
        assert_eq!(result.results, vec![json!("sync_result")]);
    }

    #[tokio::test]
    async fn test_event_facade_listen_and_dispatch_async() {
        Event::listen(
            "FacadeTestAsync",
            Arc::new(ClosureListener::new(|_| Ok(json!("async_result")))),
            false,
        );

        let result = Event::dispatch("FacadeTestAsync", &Value::Null, DispatchMode::Async)
            .await
            .unwrap();
        assert!(result.is_success());
        assert_eq!(result.results, vec![json!("async_result")]);
    }

    #[tokio::test]
    async fn test_event_facade_priority() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        let o1 = order.clone();
        Event::listen_with_priority(
            "FacadePriorityTest",
            Arc::new(ClosureListener::new(move |_| {
                o1.lock().unwrap().push(1);
                Ok(Value::Null)
            })),
            10,
        );

        let o2 = order.clone();
        Event::listen_with_priority(
            "FacadePriorityTest",
            Arc::new(ClosureListener::new(move |_| {
                o2.lock().unwrap().push(2);
                Ok(Value::Null)
            })),
            20,
        );

        Event::dispatch("FacadePriorityTest", &Value::Null, DispatchMode::Sync)
            .await
            .unwrap();
        assert_eq!(*order.lock().unwrap(), vec![2, 1]);
    }

    #[test]
    fn test_event_facade_has_listener() {
        Event::listen(
            "FacadeHasTest",
            Arc::new(ClosureListener::new(|_| Ok(Value::Null))),
            false,
        );
        assert!(Event::has_listener("FacadeHasTest"));
        assert!(!Event::has_listener("FacadeNonexistent"));
    }

    #[test]
    fn test_event_facade_dispatch_sync_non_async() {
        Event::listen(
            "FacadeDispatchSyncTest",
            Arc::new(ClosureListener::new(|_| Ok(json!("ok")))),
            false,
        );

        let result = Event::dispatch_sync("FacadeDispatchSyncTest", &Value::Null).unwrap();
        assert!(result.is_success());
        assert_eq!(result.results, vec![json!("ok")]);
    }
}
