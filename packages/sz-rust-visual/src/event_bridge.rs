// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! Event Bridge
//!
//! tracing Layer 桥接 SDD Agent 日志 → Tauri 事件推送前端。

use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, Emitter};
use tokio::sync::broadcast;

use crate::models::{LogEvent, PhaseEvent, StatusChangedEvent};

/// SDD 事件桥接器
///
/// 消费 `PhaseEvent` 事件流，通过 `AppHandle::emit` 推送前端。
pub struct SddEventBridge {
    app: AppHandle,
}

impl SddEventBridge {
    /// 创建事件桥接器
    pub fn new(app: AppHandle) -> Self {
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
pub fn forward_log(app: &AppHandle, trace_id: &str, level: &str, message: &str) {
    let event = LogEvent {
        trace_id: trace_id.to_string(),
        level: level.to_string(),
        message: message.to_string(),
        timestamp: Utc::now(),
        fields: serde_json::json!({}),
    };
    let _ = app.emit("sdd_log", &event);
}

/// 状态变更转发器
pub fn forward_status_changed(
    app: &AppHandle,
    session_id: &str,
    old_status: &str,
    new_status: &str,
) {
    let event = StatusChangedEvent {
        session_id: session_id.to_string(),
        old_status: serde_json::from_str(old_status).unwrap_or(crate::models::SddStatus::Pending),
        new_status: serde_json::from_str(new_status).unwrap_or(crate::models::SddStatus::Running),
        timestamp: Utc::now(),
    };
    let _ = app.emit("sdd_status_changed", &event);
}

/// 共享事件桥接器
pub type SharedEventBridge = Arc<SddEventBridge>;

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
}
