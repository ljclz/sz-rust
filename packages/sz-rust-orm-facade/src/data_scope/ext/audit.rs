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
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    /// 捕获 fmt 输出的 writer，用于断言审计日志内容
    #[derive(Clone, Default)]
    struct TestWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TestWriter {
        type Writer = TestWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

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
        // 使用 fmt subscriber 捕获 tracing::info! 输出，验证审计事件被记录
        let writer = TestWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_target(true)
            .with_ansi(false)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        logger.record(event).await;
        drop(guard);

        let output =
            String::from_utf8(writer.0.lock().unwrap().clone()).expect("tracing 输出应为 UTF-8");
        assert!(
            output.contains("audit: update_rule on rule:order:dept"),
            "审计日志应包含格式化消息，实际输出: {}",
            output
        );
        assert!(
            output.contains("operator_id=1"),
            "审计日志应包含 operator_id 字段，实际输出: {}",
            output
        );
    }
}
