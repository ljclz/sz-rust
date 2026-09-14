// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 数据模型
//!
//! PhaseEvent / SddSession / ReviewDecision / SddPhase / SddStatus 等。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// SDD 阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SddPhase {
    /// 需求规格
    Spec,
    /// 技术设计
    Design,
    /// 任务规划
    Task,
    /// 代码生成
    Coding,
}

/// SDD 编排状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SddStatus {
    /// 待启动
    Pending,
    /// 运行中
    Running,
    /// 等待人工审查
    AwaitingHitl,
    /// 已完成
    Completed,
    /// 失败
    Failed,
    /// 已取消
    Cancelled,
}

/// 事件类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseEventKind {
    /// 阶段开始
    PhaseStarted,
    /// 阶段完成
    PhaseCompleted,
    /// 阶段失败
    PhaseFailed,
    /// 等待审查
    AwaitingReview,
    /// 审查决定
    ReviewSubmitted,
    /// 日志输出
    Log,
    /// 产物生成
    ArtifactGenerated,
}

/// HITL 审查决定
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// 确认通过
    Confirm,
    /// 需修改
    Modify,
    /// 需补充
    Supplement,
}

/// 产物类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// spec.md
    Spec,
    /// design.md
    Design,
    /// tasks.md
    Tasks,
    /// 源代码
    Source,
    /// 预览产物
    Preview,
}

/// 设备类型（预览）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    /// 桌面（1280×800）
    Desktop,
    /// 平板（768×1024）
    Tablet,
    /// 手机（375×667）
    Mobile,
}

impl DeviceType {
    /// 返回 viewport 尺寸 (width, height)
    pub fn viewport(&self) -> (u32, u32) {
        match self {
            Self::Desktop => (1280, 800),
            Self::Tablet => (768, 1024),
            Self::Mobile => (375, 667),
        }
    }
}

/// 阶段事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseEvent {
    /// 追踪 ID
    pub trace_id: String,
    /// 功能名称
    pub feature: String,
    /// 阶段
    pub phase: SddPhase,
    /// 事件类型
    pub kind: PhaseEventKind,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 元数据（键值对）
    pub metadata: serde_json::Value,
}

/// SDD 会话
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SddSession {
    /// 会话 ID
    pub session_id: String,
    /// 追踪 ID
    #[serde(skip_serializing)]
    pub trace_id: String,
    /// 功能名称
    pub feature_name: String,
    /// 当前阶段
    pub current_phase: SddPhase,
    /// 编排状态
    pub status: SddStatus,
    /// 启动时间
    pub started_at: DateTime<Utc>,
    /// 更新时间
    pub updated_at: DateTime<Utc>,
}

/// 日志事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    /// 追踪 ID
    pub trace_id: String,
    /// 日志级别
    pub level: String,
    /// 日志消息
    pub message: String,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 附加字段
    pub fields: serde_json::Value,
}

/// 状态变更事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusChangedEvent {
    /// 会话 ID
    pub session_id: String,
    /// 旧状态
    pub old_status: SddStatus,
    /// 新状态
    pub new_status: SddStatus,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sdd_phase_serde() {
        let json = serde_json::to_string(&SddPhase::Spec).unwrap();
        assert_eq!(json, "\"spec\"");
        let phase: SddPhase = serde_json::from_str("\"design\"").unwrap();
        assert_eq!(phase, SddPhase::Design);
    }

    #[test]
    fn test_sdd_status_serde() {
        let json = serde_json::to_string(&SddStatus::AwaitingHitl).unwrap();
        assert_eq!(json, "\"awaiting_hitl\"");
        let status: SddStatus = serde_json::from_str("\"running\"").unwrap();
        assert_eq!(status, SddStatus::Running);
    }

    #[test]
    fn test_review_decision_serde() {
        let json = serde_json::to_string(&ReviewDecision::Confirm).unwrap();
        assert_eq!(json, "\"confirm\"");
        let decision: ReviewDecision = serde_json::from_str("\"modify\"").unwrap();
        assert_eq!(decision, ReviewDecision::Modify);
    }

    #[test]
    fn test_device_type_viewport() {
        assert_eq!(DeviceType::Desktop.viewport(), (1280, 800));
        assert_eq!(DeviceType::Tablet.viewport(), (768, 1024));
        assert_eq!(DeviceType::Mobile.viewport(), (375, 667));
    }

    #[test]
    fn test_phase_event_serde() {
        let event = PhaseEvent {
            trace_id: "trace-1".to_string(),
            feature: "crm".to_string(),
            phase: SddPhase::Spec,
            kind: PhaseEventKind::PhaseStarted,
            timestamp: Utc::now(),
            metadata: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: PhaseEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.trace_id, "trace-1");
        assert_eq!(parsed.feature, "crm");
        assert_eq!(parsed.phase, SddPhase::Spec);
    }

    #[test]
    fn test_sdd_session_skip_serializing() {
        let session = SddSession {
            session_id: "s-1".to_string(),
            trace_id: "t-1".to_string(),
            feature_name: "test".to_string(),
            current_phase: SddPhase::Design,
            status: SddStatus::Running,
            started_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(!json.contains("trace_id"));
        assert!(json.contains("session_id"));
    }

    #[test]
    fn test_log_event_serde() {
        let event = LogEvent {
            trace_id: "t-1".to_string(),
            level: "info".to_string(),
            message: "phase started".to_string(),
            timestamp: Utc::now(),
            fields: serde_json::json!({}),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: LogEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.level, "info");
    }

    #[test]
    fn test_status_changed_event_serde() {
        let event = StatusChangedEvent {
            session_id: "s-1".to_string(),
            old_status: SddStatus::Running,
            new_status: SddStatus::AwaitingHitl,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: StatusChangedEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.old_status, SddStatus::Running);
        assert_eq!(parsed.new_status, SddStatus::AwaitingHitl);
    }

    #[test]
    fn test_artifact_kind_serde() {
        let json = serde_json::to_string(&ArtifactKind::Spec).unwrap();
        assert_eq!(json, "\"spec\"");
        let kind: ArtifactKind = serde_json::from_str("\"source\"").unwrap();
        assert_eq!(kind, ArtifactKind::Source);
    }
}
