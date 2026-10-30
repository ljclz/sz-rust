// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Event Bridge
//!
//! tracing Layer 桥接 SDD Agent 日志 → Tauri 事件推送前端。
//! SddEventBridge / forward_* 需要 AppHandle，标记 `#[cfg(not(coverage))]` 排除。
//! build_* 函数无需 Tauri 运行时，可直接测试。

use chrono::Utc;

use crate::models::{LogEvent, StatusChangedEvent};

pub fn build_log_event(trace_id: &str, level: &str, message: &str) -> LogEvent {
    LogEvent {
        trace_id: trace_id.to_string(),
        level: level.to_string(),
        message: message.to_string(),
        timestamp: Utc::now(),
        fields: serde_json::json!({}),
    }
}

pub fn build_status_changed_event(
    session_id: &str,
    old_status: &str,
    new_status: &str,
) -> StatusChangedEvent {
    StatusChangedEvent {
        session_id: session_id.to_string(),
        old_status: serde_json::from_str(old_status).unwrap_or(crate::models::SddStatus::Pending),
        new_status: serde_json::from_str(new_status).unwrap_or(crate::models::SddStatus::Running),
        timestamp: Utc::now(),
    }
}

// === 以下代码需要 Tauri AppHandle，覆盖率构建时排除 ===

use crate::models::PhaseEvent;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::broadcast;

/// SDD 事件桥接器
///
/// 消费 `PhaseEvent` 事件流，通过 `AppHandle::emit` 推送前端。
#[cfg(not(coverage))]
pub struct SddEventBridge<R: Runtime> {
    app: AppHandle<R>,
}

#[cfg(not(coverage))]
impl<R: Runtime> SddEventBridge<R> {
    /// 创建事件桥接器
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }

    /// 启动事件消费 task
    ///
    /// 从 broadcast receiver 消费事件，推送到前端。
    pub fn spawn(self, mut rx: broadcast::Receiver<PhaseEvent>) {
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        self.emit_phase_event(&event);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
    }

    /// 推送阶段事件
    fn emit_phase_event(&self, event: &PhaseEvent) {
        let _ = self.app.emit("sdd_phase_event", event);
    }

    /// 推送日志事件
    pub fn emit_log(&self, event: &LogEvent) {
        let _ = self.app.emit("sdd_log", event);
    }

    /// 推送状态变更事件
    pub fn emit_status_changed(&self, event: &StatusChangedEvent) {
        let _ = self.app.emit("sdd_status_changed", event);
    }
}

/// 日志事件转发器
///
/// 将 tracing 日志转为 `LogEvent` 并推送前端。
#[cfg(not(coverage))]
pub fn forward_log<R: Runtime>(app: &AppHandle<R>, trace_id: &str, level: &str, message: &str) {
    let event = build_log_event(trace_id, level, message);
    let _ = app.emit("sdd_log", &event);
}

/// 状态变更转发器
#[cfg(not(coverage))]
pub fn forward_status_changed<R: Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    old_status: &str,
    new_status: &str,
) {
    let event = build_status_changed_event(session_id, old_status, new_status);
    let _ = app.emit("sdd_status_changed", &event);
}

/// 共享事件桥接器
#[cfg(not(coverage))]
pub type SharedEventBridge<R> = Arc<SddEventBridge<R>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_event_creation() {
        let event = LogEvent {
            trace_id: "t-1".to_string(),
            level: "info".to_string(),
            message: "test".to_string(),
            timestamp: Utc::now(),
            fields: serde_json::json!({}),
        };
        assert_eq!(event.level, "info");
    }

    #[test]
    fn test_status_changed_event_creation() {
        let event = StatusChangedEvent {
            session_id: "s-1".to_string(),
            old_status: crate::models::SddStatus::Running,
            new_status: crate::models::SddStatus::Completed,
            timestamp: Utc::now(),
        };
        assert_eq!(event.old_status, crate::models::SddStatus::Running);
        assert_eq!(event.new_status, crate::models::SddStatus::Completed);
    }

    #[test]
    fn test_build_log_event() {
        let event = build_log_event("trace-1", "info", "test message");
        assert_eq!(event.trace_id, "trace-1");
        assert_eq!(event.level, "info");
        assert_eq!(event.message, "test message");
    }

    #[test]
    fn test_build_status_changed_event() {
        let event = build_status_changed_event("s-1", "\"running\"", "\"completed\"");
        assert_eq!(event.session_id, "s-1");
        assert_eq!(event.old_status, crate::models::SddStatus::Running);
        assert_eq!(event.new_status, crate::models::SddStatus::Completed);
    }

    #[test]
    fn test_build_status_changed_event_invalid_json_defaults() {
        let event = build_status_changed_event("s-1", "invalid", "invalid");
        assert_eq!(event.session_id, "s-1");
        assert_eq!(event.old_status, crate::models::SddStatus::Pending);
        assert_eq!(event.new_status, crate::models::SddStatus::Running);
    }

    #[test]
    fn test_build_log_event_fields_empty() {
        let event = build_log_event("t", "warn", "msg");
        assert!(event.fields.is_object());
        assert!(event.fields.as_object().unwrap().is_empty());
    }

    #[test]
    fn test_build_status_changed_event_pending_to_running() {
        let event = build_status_changed_event("s", "\"pending\"", "\"running\"");
        assert_eq!(event.old_status, crate::models::SddStatus::Pending);
        assert_eq!(event.new_status, crate::models::SddStatus::Running);
    }

    #[test]
    fn test_build_status_changed_event_mixed_valid_invalid() {
        let event = build_status_changed_event("s", "\"completed\"", "invalid");
        assert_eq!(event.old_status, crate::models::SddStatus::Completed);
        assert_eq!(event.new_status, crate::models::SddStatus::Running);
    }
}
