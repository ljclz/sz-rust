// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 审计日志 — AuditLogger trait 与 TracingAuditLogger

use async_trait::async_trait;

/// 审计事件
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub operator_id: i64,
    pub operation: String,
    pub target_type: String,
    pub target_id: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// 审计日志 trait
#[async_trait]
pub trait AuditLogger: Send + Sync {
    /// 记录审计事件
    async fn record(&self, event: AuditEvent);
}

/// 基于 tracing 的审计日志实现
pub struct TracingAuditLogger;

#[async_trait]
impl AuditLogger for TracingAuditLogger {
    async fn record(&self, event: AuditEvent) {
        tracing::info!(
            target: "data_scope_audit",
            operator_id = event.operator_id,
            operation = %event.operation,
            target_type = %event.target_type,
            target_id = %event.target_id,
            "audit: {} on {}:{}", event.operation, event.target_type, event.target_id
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn test_tracing_audit_logger_record() {
        let logger = TracingAuditLogger;
        let event = AuditEvent {
            operator_id: 1,
            operation: "update_rule".into(),
            target_type: "rule".into(),
            target_id: "order:dept".into(),
            before: Some("old".into()),
            after: Some("new".into()),
            timestamp: Utc::now(),
        };
        // 仅验证不 panic
        logger.record(event).await;
    }
}
