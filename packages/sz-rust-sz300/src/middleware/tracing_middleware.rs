// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 SZ-Rust Team
//! v1.9.0 全链路追踪中间件 + Span 本地缓冲与脱敏
//!
//! 对应 tasks.md §6.1-6.2：
//! - 为每个请求生成 TraceID + Root Span
//! - TraceID 注入请求扩展 + 响应头 `X-Trace-Id`
//! - 追踪后端不可用时 Span 本地缓冲 ≤ 10000 条，恢复后补传
//! - Span 属性敏感字段脱敏（复用 sz-rust-data-mask）
//! - 采样率可动态配置（0-1 浮点）

use std::collections::VecDeque;
use std::sync::{OnceLock, RwLock};
use std::time::Instant;

use axum::{
    body::Body,
    http::{Request, Response},
    middleware::Next,
};
use serde_json::{json, Value};
use tracing::Instrument;
use uuid::Uuid;

/// 最大 Span 缓冲条目数（spec 5.7.3 异常 1）
const MAX_SPAN_BUFFER: usize = 10000;

/// 敏感字段名集合（匹配到则脱敏为 `<redacted>`）
const SENSITIVE_FIELDS: &[&str] = &[
    "password",
    "token",
    "authorization",
    "secret",
    "api_key",
    "access_token",
    "refresh_token",
    "id_card",
    "phone",
    "mobile",
];

/// TraceID 请求扩展类型
#[derive(Debug, Clone)]
pub struct TraceId(pub String);

/// Span 缓冲条目
#[derive(Debug, Clone)]
pub struct SpanEntry {
    /// TraceID
    pub trace_id: String,
    /// SpanID
    pub span_id: String,
    /// HTTP 方法
    pub method: String,
    /// 请求路径
    pub path: String,
    /// 响应状态码
    pub status: u16,
    /// 耗时（毫秒）
    pub duration_ms: u64,
    /// 时间戳（Unix 毫秒）
    pub timestamp: i64,
    /// Span 属性（已脱敏）
    pub attributes: Value,
}

/// Span 本地缓冲（追踪后端不可用时缓冲，恢复后补传）
pub struct SpanBuffer {
    entries: RwLock<VecDeque<SpanEntry>>,
    max_size: usize,
    sampling_rate: RwLock<f64>,
}

impl SpanBuffer {
    /// 创建默认缓冲（max_size=10000, sampling_rate=1.0）
    pub fn new() -> Self {
        Self::with_capacity(MAX_SPAN_BUFFER)
    }

    /// 创建指定容量的缓冲
    pub fn with_capacity(max_size: usize) -> Self {
        Self {
            entries: RwLock::new(VecDeque::with_capacity(max_size.min(MAX_SPAN_BUFFER))),
            max_size: max_size.min(MAX_SPAN_BUFFER),
            sampling_rate: RwLock::new(1.0),
        }
    }

    /// 推入 Span 条目（超出容量时丢弃最旧条目）
    pub fn push(&self, entry: SpanEntry) {
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= self.max_size {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// 排空缓冲并返回所有条目（用于后端恢复后补传）
    pub fn drain(&self) -> Vec<SpanEntry> {
        let mut entries = self.entries.write().unwrap_or_else(|e| e.into_inner());
        entries.drain(..).collect()
    }

    /// 当前缓冲条目数
    pub fn len(&self) -> usize {
        self.entries.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// 缓冲是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 设置采样率（0.0-1.0，超出范围自动钳位）
    pub fn set_sampling_rate(&self, rate: f64) {
        let clamped = rate.clamp(0.0, 1.0);
        *self.sampling_rate.write().unwrap_or_else(|e| e.into_inner()) = clamped;
    }

    /// 获取当前采样率
    pub fn sampling_rate(&self) -> f64 {
        *self.sampling_rate.read().unwrap_or_else(|e| e.into_inner())
    }

    /// 根据采样率决定是否采样
    pub fn should_sample(&self) -> bool {
        let rate = *self.sampling_rate.read().unwrap_or_else(|e| e.into_inner());
        if rate >= 1.0 {
            return true;
        }
        if rate <= 0.0 {
            return false;
        }
        // 简单随机采样：用 UUID 的首字节做哈希
        let hash = Uuid::new_v4().as_bytes()[0] as f64 / 255.0;
        hash < rate
    }
}

impl Default for SpanBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局 Span 缓冲
static SPAN_BUFFER: OnceLock<SpanBuffer> = OnceLock::new();

/// 获取全局 Span 缓冲（首次调用时初始化）
pub fn span_buffer() -> &'static SpanBuffer {
    SPAN_BUFFER.get_or_init(SpanBuffer::new)
}

/// 脱敏 Span 属性中的敏感字段
///
/// 将 password/token/身份证号等字段值替换为 `<redacted>`
pub fn mask_span_attributes(attrs: &mut Value) {
    if let Some(obj) = attrs.as_object_mut() {
        for (key, val) in obj.iter_mut() {
            let key_lower = key.to_lowercase();
            if SENSITIVE_FIELDS.iter().any(|f| key_lower.contains(f)) && val.is_string() {
                *val = Value::String("<redacted>".to_string());
            }
        }
    }
}

/// v1.9.0 全链路追踪中间件
///
/// 为每个请求生成 TraceID + Root Span，注入请求扩展 + 响应头 `X-Trace-Id`
///
/// ## 执行流程
///
/// 1. 生成 TraceID（UUID v4）
/// 2. 注入 `TraceId` 到请求扩展
/// 3. 创建 `tracing::info_span!` 记录请求信息
/// 4. 调用下游 handler
/// 5. 响应头添加 `X-Trace-Id`
/// 6. Span 条目推入全局缓冲（按采样率决定）
pub async fn tracing_middleware(mut req: Request<Body>, next: Next) -> Response<Body> {
    let trace_id = Uuid::new_v4().to_string();
    let span_id = Uuid::new_v4().simple().to_string();
    let start = Instant::now();
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let timestamp = chrono::Utc::now().timestamp_millis();

    // 注入 TraceId 到请求扩展
    req.extensions_mut().insert(TraceId(trace_id.clone()));

    // 创建 Root Span
    let span = tracing::info_span!(
        "http_request",
        trace_id = %trace_id,
        span_id = %span_id,
        method = %method,
        path = %path,
    );

    let mut response = next.run(req).instrument(span).await;

    // 添加 X-Trace-Id 响应头
    let status = response.status().as_u16();
    if let Ok(header_value) = trace_id.parse() {
        response.headers_mut().insert("X-Trace-Id", header_value);
    }

    // 按采样率决定是否缓冲
    let buffer = span_buffer();
    if buffer.should_sample() {
        let duration_ms = start.elapsed().as_millis() as u64;
        let mut attributes = json!({
            "method": method,
            "path": path,
            "status": status,
            "duration_ms": duration_ms,
        });
        mask_span_attributes(&mut attributes);
        buffer.push(SpanEntry {
            trace_id,
            span_id,
            method,
            path,
            status,
            duration_ms,
            timestamp,
            attributes,
        });
    }

    response
}

/// 提取请求中的 TraceID（供下游服务调用透传）
pub fn extract_trace_id<T>(req: &Request<T>) -> Option<String> {
    req.extensions()
        .get::<TraceId>()
        .map(|t| t.0.clone())
        .or_else(|| {
            req.headers()
                .get("X-Trace-Id")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_buffer_push_and_drain() {
        let buffer = SpanBuffer::with_capacity(5);
        for i in 0..3 {
            buffer.push(SpanEntry {
                trace_id: format!("trace-{}", i),
                span_id: format!("span-{}", i),
                method: "GET".into(),
                path: "/test".into(),
                status: 200,
                duration_ms: 10,
                timestamp: 0,
                attributes: json!({}),
            });
        }
        assert_eq!(buffer.len(), 3);
        let drained = buffer.drain();
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].trace_id, "trace-0");
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_span_buffer_eviction() {
        let buffer = SpanBuffer::with_capacity(3);
        for i in 0..5 {
            buffer.push(SpanEntry {
                trace_id: format!("trace-{}", i),
                span_id: format!("span-{}", i),
                method: "GET".into(),
                path: "/test".into(),
                status: 200,
                duration_ms: 10,
                timestamp: 0,
                attributes: json!({}),
            });
        }
        assert_eq!(buffer.len(), 3, "应丢弃最旧 2 条");
        let drained = buffer.drain();
        assert_eq!(drained[0].trace_id, "trace-2");
        assert_eq!(drained[2].trace_id, "trace-4");
    }

    #[test]
    fn test_span_buffer_sampling_rate() {
        let buffer = SpanBuffer::new();
        buffer.set_sampling_rate(0.0);
        assert!(!buffer.should_sample(), "采样率 0 应不采样");
        buffer.set_sampling_rate(1.0);
        assert!(buffer.should_sample(), "采样率 1 应总是采样");
        buffer.set_sampling_rate(1.5);
        assert_eq!(buffer.sampling_rate(), 1.0, "超出范围应钳位");
        buffer.set_sampling_rate(-0.5);
        assert_eq!(buffer.sampling_rate(), 0.0, "负值应钳位");
    }

    #[test]
    fn test_mask_span_attributes() {
        let mut attrs = json!({
            "method": "POST",
            "password": "secret123",
            "token": "bearer-xyz",
            "user_id": 42,
            "id_card": "110101199001011234",
            "normal_field": "hello"
        });
        mask_span_attributes(&mut attrs);
        assert_eq!(attrs["password"], "<redacted>");
        assert_eq!(attrs["token"], "<redacted>");
        assert_eq!(attrs["id_card"], "<redacted>");
        assert_eq!(attrs["method"], "POST");
        assert_eq!(attrs["user_id"], 42);
        assert_eq!(attrs["normal_field"], "hello");
    }

    #[test]
    fn test_mask_span_attributes_nested_not_masked() {
        let mut attrs = json!({
            "authorization": "Bearer abc123",
            "data": { "password": "inner" }
        });
        mask_span_attributes(&mut attrs);
        assert_eq!(attrs["authorization"], "<redacted>");
    }

    #[test]
    fn test_global_span_buffer() {
        let buffer = span_buffer();
        let initial_len = buffer.len();
        buffer.push(SpanEntry {
            trace_id: "test-trace".into(),
            span_id: "test-span".into(),
            method: "GET".into(),
            path: "/".into(),
            status: 200,
            duration_ms: 1,
            timestamp: 0,
            attributes: json!({}),
        });
        assert_eq!(buffer.len(), initial_len + 1);
        let _ = buffer.drain();
    }

    #[test]
    fn test_trace_id_type() {
        let t = TraceId("abc-123".to_string());
        assert_eq!(t.0, "abc-123");
    }

    #[test]
    fn test_span_entry_fields() {
        let entry = SpanEntry {
            trace_id: "t1".into(),
            span_id: "s1".into(),
            method: "POST".into(),
            path: "/api/order".into(),
            status: 500,
            duration_ms: 150,
            timestamp: 1700000000000,
            attributes: json!({"error": "internal"}),
        };
        assert_eq!(entry.trace_id, "t1");
        assert_eq!(entry.status, 500);
        assert_eq!(entry.duration_ms, 150);
    }
}
