// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Event 静态门面
//!
//! 委托 `sz_rust_state_facade::event_facade::Event`（P1 已实现）。

pub use sz_rust_state_facade::event_facade::Event;

use serde_json::Value;

use sz_rust_state_facade::event::{DispatchMode, DispatchResult, EventError};

/// 事件门面错误转换
impl From<EventError> for crate::FacadeError {
    fn from(e: EventError) -> Self {
        crate::FacadeError::Event(e.to_string())
    }
}

/// Event 门面扩展方法
///
/// 在 P1 `Event` 基础上提供 `FacadeError` 兼容的方法。
pub trait EventFacadeExt {
    /// 分发事件（返回 `FacadeError`）
    fn dispatch_facade(
        event: &str,
        params: &Value,
        mode: DispatchMode,
    ) -> Result<DispatchResult, crate::FacadeError>;
}

impl EventFacadeExt for Event {
    fn dispatch_facade(
        event: &str,
        params: &Value,
        mode: DispatchMode,
    ) -> Result<DispatchResult, crate::FacadeError> {
        // v1.4 修复：此前为 unimplemented! 地雷（调用即 panic）。
        // 同步门面按 Sync 语义分发（复用 Event::dispatch_sync）；
        // Async 模式需要 async 上下文，返回明确错误而非静默降级 ——
        // 需要 Async 语义请使用 Event::dispatch(...).await。
        match mode {
            DispatchMode::Sync => Event::dispatch_sync(event, params).map_err(Into::into),
            DispatchMode::Async => Err(crate::FacadeError::Event(
                "DispatchMode::Async 需要 async 上下文，请使用 Event::dispatch(...).await；同步门面仅支持 DispatchMode::Sync"
                    .to_string(),
            )),
        }
    }
}

// 重导出常用类型
pub use sz_rust_state_facade::event::DispatchMode as EventDispatchMode;
pub use sz_rust_state_facade::event::DispatchResult as EventDispatchResult;
pub use sz_rust_state_facade::event::Listener as EventListener;
pub use sz_rust_state_facade::event::Observer as EventObserver;
pub use sz_rust_state_facade::event::Subscriber as EventSubscriber;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use sz_rust_state_facade::event::ClosureListener;

    #[tokio::test]
    async fn test_event_facade_delegate() {
        Event::listen(
            "FacadeEventTest",
            Arc::new(ClosureListener::new(|_| Ok(json!("ok")))),
            false,
        );
        let result = Event::dispatch("FacadeEventTest", &Value::Null, DispatchMode::Sync)
            .await
            .unwrap();
        assert!(result.is_success());
        assert_eq!(result.results, vec![json!("ok")]);
    }

    /// 回归测试（v1.4）：dispatch_facade Sync 模式真实分发（原为 unimplemented! 地雷）
    #[tokio::test]
    async fn test_dispatch_facade_sync_mode_works() {
        Event::listen(
            "FacadeDispatchSync",
            Arc::new(ClosureListener::new(|_| Ok(json!("ok_from_listener")))),
            false,
        );

        let result =
            Event::dispatch_facade("FacadeDispatchSync", &Value::Null, EventDispatchMode::Sync)
                .expect("Sync 模式必须真实分发");
        assert_eq!(result.results, vec![json!("ok_from_listener")]);
        assert!(result.is_success());
    }

    /// 回归测试（v1.4）：Async 模式返回明确错误而非 panic/静默降级
    #[test]
    fn test_dispatch_facade_async_mode_returns_err() {
        let result = Event::dispatch_facade(
            "FacadeDispatchAsync",
            &Value::Null,
            EventDispatchMode::Async,
        );
        let err = result.expect_err("Async 模式在同步门面上必须返回明确错误");
        let msg = match err {
            crate::FacadeError::Event(m) => m,
            other => panic!("错误类型不符: {other:?}"),
        };
        assert!(
            msg.contains("Event::dispatch"),
            "错误信息应指引正确用法: {msg}"
        );
    }
}
