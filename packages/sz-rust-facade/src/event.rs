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
        // 由于 Event::dispatch 是 async fn，这里提供同步 trigger 的 facade 兼容
        // 完整 async dispatch 直接使用 Event::dispatch
        let _ = (event, params, mode);
        unimplemented!("Use Event::dispatch for async dispatch")
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
}
