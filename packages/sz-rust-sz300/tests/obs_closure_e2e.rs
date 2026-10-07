// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 可观测性闭环 E2E 集成测试
//!
//! 对应 tasks.md §6.5：端到端验证可观测性闭环
//!
//! ## 测试矩阵
//!
//! | 场景 | 验证点 |
//! |------|--------|
//! | 请求经过 tracing_middleware | 响应头含 X-Trace-Id + Span 缓冲 |
//! | Span 本地缓冲与补传 | push → drain → 空 |
//! | 告警静默去重 | record → should_alert=false → 过期 → true |
//! | 告警内容脱敏 | 手机号 → 138****5678 |
//! | 业务告警规则集 | 3 条规则定义正确 + 验证通过 |

#![cfg(feature = "v19-obs-closure")]

use std::time::Duration;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware,
    routing::get,
    Router,
};

use tower::ServiceExt;

use serde_json::json;
use sz_rust_alert_engine::{AlertSeverity, SilenceManager};
use sz_rust_sz300::{
    alert_rules::{channels_for_severity, mask_alert_content, BusinessAlertRules},
    middleware::tracing_middleware::{span_buffer, tracing_middleware, SpanBuffer, SpanEntry},
};

fn make_span_entry(trace_id: &str) -> SpanEntry {
    SpanEntry {
        trace_id: trace_id.to_string(),
        span_id: format!("span-{}", trace_id),
        method: "GET".into(),
        path: "/api/v1/test".into(),
        status: 200,
        duration_ms: 15,
        timestamp: 1700000000000,
        attributes: json!({"method": "GET"}),
    }
}

#[tokio::test]
async fn test_tracing_middleware_generates_trace_id() {
    let app = Router::new()
        .route("/api/v1/test", get(|| async { "ok" }))
        .layer(middleware::from_fn(tracing_middleware));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().contains_key("X-Trace-Id"),
        "响应头应包含 X-Trace-Id"
    );
    let trace_id = response
        .headers()
        .get("X-Trace-Id")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(!trace_id.is_empty(), "TraceID 不应为空");
}

#[tokio::test]
async fn test_tracing_middleware_buffers_span() {
    let buffer = span_buffer();
    let _ = buffer.drain();

    let app = Router::new()
        .route("/api/v1/health", get(|| async { "ok" }))
        .layer(middleware::from_fn(tracing_middleware));

    let _ = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot");

    assert!(
        !buffer.is_empty(),
        "Span 缓冲应至少有 1 条记录，实际: {}",
        buffer.len()
    );
    let drained = buffer.drain();
    assert!(!drained.is_empty(), "drain 应返回缓冲的 Span");
    assert!(buffer.is_empty(), "drain 后缓冲应为空");
}

#[tokio::test]
async fn test_span_buffer_backfill() {
    let buffer = SpanBuffer::with_capacity(100);
    for i in 0..50 {
        buffer.push(make_span_entry(&format!("trace-{}", i)));
    }
    assert_eq!(buffer.len(), 50);

    let drained = buffer.drain();
    assert_eq!(drained.len(), 50, "应补传全部 50 条");
    assert_eq!(drained[0].trace_id, "trace-0");
    assert_eq!(drained[49].trace_id, "trace-49");
    assert!(buffer.is_empty(), "补传后缓冲应为空");
}

#[tokio::test]
async fn test_silence_manager_dedup() {
    let mgr = SilenceManager::with_default_silence(Duration::from_secs(60));

    assert!(mgr.should_alert("order_failure_rate"), "首次应允许告警");
    mgr.record_alert("order_failure_rate");
    assert!(
        !mgr.should_alert("order_failure_rate"),
        "静默期内不应重复推送"
    );
    assert_eq!(mgr.silenced_count(), 1);

    mgr.clear("order_failure_rate");
    assert!(
        mgr.should_alert("order_failure_rate"),
        "清除静默后应允许告警"
    );
}

#[tokio::test]
async fn test_alert_content_phone_masked() {
    let content = "客户手机号 13812345678 订单失败率 15%";
    let masked = mask_alert_content(content);
    assert!(
        masked.contains("138****5678"),
        "手机号应脱敏为 138****5678，实际: {}",
        masked
    );
    assert!(!masked.contains("1234"), "中间 4 位不应出现");
    assert!(masked.contains("15%"), "非敏感内容应保留");
}

#[tokio::test]
async fn test_business_rules_all_valid() {
    let rules = BusinessAlertRules::all();
    assert_eq!(rules.len(), 3, "应有 3 条业务告警规则");

    for rule in &rules {
        assert!(rule.validate().is_ok(), "规则 {} 应验证通过", rule.id);
    }

    let order_rule = BusinessAlertRules::order_failure_rate();
    assert_eq!(order_rule.severity, AlertSeverity::Critical);
    let p0_channels = channels_for_severity(AlertSeverity::Critical);
    assert!(p0_channels.contains(&"phone"), "P0 应包含电话渠道");

    let device_rule = BusinessAlertRules::device_offline_rate();
    assert_eq!(device_rule.severity, AlertSeverity::Warning);

    let stock_rule = BusinessAlertRules::stock_low();
    assert_eq!(stock_rule.severity, AlertSeverity::Info);
}
