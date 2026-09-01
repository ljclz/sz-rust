//! # SZ-Rust Tracing — 链路追踪
//!
//! 提供分布式链路追踪的 Span/Tracer 抽象，支持 W3C TraceContext 传播，
//! 用于跨服务调用链的采集与可视化。
//!
//! ## OTLP 导出
//!
//! OTLP exporter 已统一由 `sz-rust-observability` 包提供（支持 gRPC/HTTP 双协议、
//! 环境变量配置、资源属性），本包专注于 Span/Tracer 核心抽象与 W3C 传播。
//!
//! ## 主要类型
//!
//! - [`Span`] — 单个追踪片段
//! - [`Tracer`] — 追踪器抽象
//! - [`SzTracer`] — 默认实现，支持 W3C TraceContext 传播
//! - [`InMemoryTracer`] — SzTracer 的兼容包装器（内存存储）
//!
//! ## W3C TraceContext
//!
//! 默认注入/提取遵循 W3C `traceparent` 规范：
//! `00-<trace_id>-<span_id>-<trace_flags>`，同时保留 `parent-span-id`
//! header 以向后兼容旧版本客户端。

#![forbid(unsafe_code)]
#![allow(missing_docs)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// 追踪片段，表示一次操作的开始与结束。
///
/// 包含 trace_id / span_id / parent_id 等标识，以及 tags / logs 等元数据，
/// 用于跨服务调用链的采集与可视化。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    /// 追踪 ID（同一调用链共享），32 字符 hex。
    pub trace_id: String,
    /// 当前 Span ID，16 字符 hex。
    pub span_id: String,
    /// 父 Span ID，根 Span 时为 `None`。
    pub parent_id: Option<String>,
    /// 操作名（如 HTTP 路由、数据库查询名）。
    pub operation_name: String,
    /// 服务名（出现在 trace 的 service.name 标签）。
    pub service_name: String,
    /// 起始时间戳（毫秒，UNIX epoch）。
    pub start_time: i64,
    /// 结束时间戳（毫秒），未结束时为 `None`。
    pub end_time: Option<i64>,
    /// 标签集合，键值对均为字符串。
    pub tags: HashMap<String, String>,
    /// 日志事件列表。
    pub logs: Vec<SpanLog>,
}

impl Span {
    /// 创建一个新的未结束 Span，自动生成当前时间戳。
    pub fn new(
        trace_id: impl Into<String>,
        span_id: impl Into<String>,
        operation_name: impl Into<String>,
    ) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: span_id.into(),
            parent_id: None,
            operation_name: operation_name.into(),
            service_name: String::new(),
            start_time: current_timestamp(),
            end_time: None,
            tags: HashMap::new(),
            logs: Vec::new(),
        }
    }

    /// 设置父 Span ID（builder 风格）。
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// 设置服务名（builder 风格）。
    pub fn with_service(mut self, service_name: impl Into<String>) -> Self {
        self.service_name = service_name.into();
        self
    }

    /// 添加标签（builder 风格）。
    pub fn with_tag(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.tags.insert(key.into(), value.into());
        self
    }

    /// 标记 Span 结束，记录结束时间戳。
    pub fn finish(&mut self) {
        self.end_time = Some(current_timestamp());
    }

    /// 计算持续时间（毫秒），未结束时返回 `None`。
    pub fn duration(&self) -> Option<i64> {
        self.end_time.map(|end| end - self.start_time)
    }

    /// 追加一条日志事件。
    pub fn add_log(&mut self, message: impl Into<String>) {
        self.logs.push(SpanLog {
            timestamp: current_timestamp(),
            message: message.into(),
            fields: HashMap::new(),
        });
    }

    /// 返回追踪 ID。
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    /// 返回 Span ID。
    pub fn span_id(&self) -> &str {
        &self.span_id
    }

    /// 返回父 Span ID（若存在）。
    pub fn parent_id(&self) -> Option<&str> {
        self.parent_id.as_deref()
    }

    /// 返回操作名。
    pub fn operation_name(&self) -> &str {
        &self.operation_name
    }

    /// 返回服务名。
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// 返回标签集合引用。
    pub fn tags(&self) -> &HashMap<String, String> {
        &self.tags
    }

    /// 返回日志事件列表引用。
    pub fn logs(&self) -> &[SpanLog] {
        &self.logs
    }
}

/// Span 内的日志事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanLog {
    /// 日志时间戳（毫秒）。
    pub timestamp: i64,
    /// 日志消息。
    pub message: String,
    /// 附加字段集合。
    pub fields: HashMap<String, String>,
}

/// 追踪器抽象，负责 Span 的生命周期管理与上下文传播。
///
/// 实现方需提供 `start_span` / `end_span` / `inject` / `extract` 四个方法，
/// 以支持跨进程的链路追踪上下文传递。
pub trait Tracer: Send + Sync {
    /// 创建并启动一个新的 Span。
    fn start_span(&self, operation_name: &str) -> Span;
    /// 结束一个 Span 并将其记录到内部存储。
    fn end_span(&self, span: Span);
    /// 将 Span 上下文注入到 headers 中以便跨进程传播。
    fn inject(&self, span: &Span) -> HashMap<String, String>;
    /// 从 headers 中提取 Span 上下文。
    fn extract(&self, headers: &HashMap<String, String>) -> Option<Span>;
}

/// 默认 Tracer 实现，支持 W3C TraceContext 传播。
///
/// 内部使用 `RwLock<Vec<Span>>` 线程安全地累积已结束的 Span，
/// 可通过 [`get_spans`](Self::get_spans) 读取、[`clear`](Self::clear) 清空。
pub struct SzTracer {
    spans: Arc<RwLock<Vec<Span>>>,
    service_name: String,
}

impl SzTracer {
    /// 创建一个新的 Tracer，指定服务名。
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            spans: Arc::new(RwLock::new(Vec::new())),
            service_name: service_name.into(),
        }
    }

    /// 生成符合 W3C 规范的 32 字符 hex trace_id。
    pub fn generate_trace_id() -> String {
        format!("{:032x}", rand_u64())
    }

    /// 生成符合 W3C 规范的 16 字符 hex span_id。
    pub fn generate_span_id() -> String {
        format!("{:016x}", rand_u64())
    }

    /// 返回已累积的 Span 快照（拷贝）。
    pub fn get_spans(&self) -> Vec<Span> {
        self.spans.read().expect("锁被毒化").clone()
    }

    /// 清空已累积的 Span。
    pub fn clear(&self) {
        self.spans.write().expect("锁被毒化").clear();
    }
}

impl Default for SzTracer {
    fn default() -> Self {
        Self::new("unknown")
    }
}

impl Tracer for SzTracer {
    fn start_span(&self, operation_name: &str) -> Span {
        Span::new(
            Self::generate_trace_id(),
            Self::generate_span_id(),
            operation_name,
        )
        .with_service(&self.service_name)
    }

    fn end_span(&self, mut span: Span) {
        span.finish();

        if let Ok(mut spans) = self.spans.write() {
            spans.push(span);
        }
    }

    /// 注入 W3C TraceContext `traceparent` header。
    ///
    /// 格式：`00-<trace_id>-<span_id>-<trace_flags>`
    /// - `00`：版本号（W3C 规范当前固定为 `00`）
    /// - `trace_id`：32 字符 hex（16 字节）
    /// - `span_id`：16 字符 hex（8 字节）
    /// - `trace_flags`：2 字符 hex（`01` 表示 sampled）
    ///
    /// 同时保留 `parent-span-id` header 以向后兼容旧版本的 extract。
    fn inject(&self, span: &Span) -> HashMap<String, String> {
        let mut headers = HashMap::new();

        // W3C TraceContext: traceparent = version-trace_id-span_id-flags
        let traceparent = format!("00-{}-{}-01", span.trace_id, span.span_id);
        headers.insert("traceparent".to_string(), traceparent);

        if let Some(ref parent_id) = span.parent_id {
            headers.insert("parent-span-id".to_string(), parent_id.clone());
        }

        headers
    }

    /// 从 headers 中提取 Span。
    ///
    /// 优先解析 W3C TraceContext `traceparent` header，若不存在或格式非法
    /// 则回退到 legacy 自定义 header（`trace-id`/`span-id`）。
    ///
    /// W3C `traceparent` 格式：`00-<trace_id>-<span_id>-<trace_flags>`
    fn extract(&self, headers: &HashMap<String, String>) -> Option<Span> {
        // 优先尝试 W3C traceparent
        if let Some(traceparent) = headers.get("traceparent") {
            if let Some(span) = Self::parse_traceparent(traceparent) {
                let mut span = span.with_service(&self.service_name);

                // 同时检查 legacy parent-span-id header
                if let Some(parent_id) = headers.get("parent-span-id") {
                    span = span.with_parent(parent_id.clone());
                }

                return Some(span);
            }
        }

        // 回退到 legacy header（向后兼容）
        let trace_id = headers.get("trace-id")?;
        let span_id = headers.get("span-id")?;

        let mut span = Span::new(trace_id.clone(), span_id.clone(), "extracted");

        if let Some(parent_id) = headers.get("parent-span-id") {
            span = span.with_parent(parent_id.clone());
        }

        span = span.with_service(&self.service_name);

        Some(span)
    }
}

impl SzTracer {
    /// 解析 W3C TraceContext `traceparent` header。
    ///
    /// 格式：`00-<trace_id>-<span_id>-<trace_flags>`
    /// - 版本号必须为 2 字符 hex，且 W3C 规范当前固定为 `00`
    /// - trace_id 必须为 32 字符 hex（不能全为 0）
    /// - span_id 必须为 16 字符 hex（不能全为 0）
    /// - trace_flags 必须为 2 字符 hex
    ///
    /// 返回 `Some(Span)` 表示解析成功，`None` 表示格式不合法。
    fn parse_traceparent(traceparent: &str) -> Option<Span> {
        let parts: Vec<&str> = traceparent.split('-').collect();
        if parts.len() != 4 {
            return None;
        }

        let version = parts[0];
        let trace_id = parts[1];
        let span_id = parts[2];
        let trace_flags = parts[3];

        // 版本号必须是 2 字符 hex
        if version.len() != 2 || !version.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }

        // trace_id 必须是 32 字符 hex，且不能全为 0
        if trace_id.len() != 32
            || !trace_id.chars().all(|c| c.is_ascii_hexdigit())
            || trace_id.chars().all(|c| c == '0')
        {
            return None;
        }

        // span_id 必须是 16 字符 hex，且不能全为 0
        if span_id.len() != 16
            || !span_id.chars().all(|c| c.is_ascii_hexdigit())
            || span_id.chars().all(|c| c == '0')
        {
            return None;
        }

        // trace_flags 必须是 2 字符 hex
        if trace_flags.len() != 2 || !trace_flags.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }

        Some(Span::new(
            trace_id.to_string(),
            span_id.to_string(),
            "extracted",
        ))
    }

    /// 注入 legacy 自定义 header（向后兼容）。
    ///
    /// 保留旧版 header 格式以兼容旧客户端。新代码应使用 [`Tracer::inject`]（W3C TraceContext）。
    pub fn inject_legacy(&self, span: &Span) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("trace-id".to_string(), span.trace_id.to_string());
        headers.insert("span-id".to_string(), span.span_id.to_string());

        if let Some(ref parent_id) = span.parent_id {
            headers.insert("parent-span-id".to_string(), parent_id.clone());
        }

        headers
    }

    /// 仅从 legacy header 中提取 Span（向后兼容）。
    ///
    /// 新代码应使用 [`Tracer::extract`]（自动优先 W3C，回退 legacy）。
    pub fn extract_legacy(&self, headers: &HashMap<String, String>) -> Option<Span> {
        let trace_id = headers.get("trace-id")?;
        let span_id = headers.get("span-id")?;

        let mut span = Span::new(trace_id.clone(), span_id.clone(), "extracted");

        if let Some(parent_id) = headers.get("parent-span-id") {
            span = span.with_parent(parent_id.clone());
        }

        span = span.with_service(&self.service_name);

        Some(span)
    }
}

/// SzTracer 的兼容包装器，暴露与 OpenTelemetry SDK 一致的 [`Tracer`] 接口。
///
/// M-4 修复：类型已从 `OtelTracer` 重命名为 `InMemoryTracer`，以准确反映其实现本质。
///
/// 该类型并非真正的 OpenTelemetry 实现：
/// - 不会将 Span 导出到 OTLP / Jaeger / Zipkin collector
/// - 已实现 W3C TraceContext `traceparent` header 传播
/// - 不执行采样、baggage 传播或跨 `async` 边界的上下文提取
///
/// 适用于已有代码期望一个 "otel 风格" tracer 接口、但实际由 SzTracer 支撑的场景。
/// 生产级跨服务分布式追踪请依赖真正的 `opentelemetry` SDK。
pub struct InMemoryTracer {
    tracer: SzTracer,
}

impl InMemoryTracer {
    /// 创建一个包装指定服务名的 [`SzTracer`]。
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            tracer: SzTracer::new(service_name),
        }
    }

    /// 返回内部 [`SzTracer`] 的引用，便于检查累积的 Span 或在测试间清空。
    pub fn inner(&self) -> &SzTracer {
        &self.tracer
    }
}

impl Tracer for InMemoryTracer {
    fn start_span(&self, operation_name: &str) -> Span {
        self.tracer.start_span(operation_name)
    }

    fn end_span(&self, span: Span) {
        self.tracer.end_span(span)
    }

    fn inject(&self, span: &Span) -> HashMap<String, String> {
        self.tracer.inject(span)
    }

    fn extract(&self, headers: &HashMap<String, String>) -> Option<Span> {
        self.tracer.extract(headers)
    }
}

/// M-4 修复：`OtelTracer` 的向后兼容别名。
///
/// 该名称具有误导性（暗示真正的 OpenTelemetry 实现），已重命名为 [`InMemoryTracer`]。
/// 新代码请使用 [`InMemoryTracer`]。
#[deprecated(
    since = "0.2.1",
    note = "M-4 修复：类型名具有误导性，请使用 InMemoryTracer"
)]
pub type OtelTracer = InMemoryTracer;

/// 返回当前 UNIX epoch 毫秒时间戳。
fn current_timestamp() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// 基于哈希随机状态生成 64 位无符号整数，用于派生 trace_id / span_id。
fn rand_u64() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

/// 追踪相关错误。
#[derive(Debug)]
pub enum TracingError {
    /// 指定 ID 的 Span 未找到。
    SpanNotFound(String),
    /// 追踪 ID 格式非法。
    InvalidTraceId(String),
    /// 内部错误（如锁中毒）。
    Internal(String),
}

impl std::fmt::Display for TracingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TracingError::SpanNotFound(id) => write!(f, "Span not found: {}", id),
            TracingError::InvalidTraceId(id) => write!(f, "Invalid trace id: {}", id),
            TracingError::Internal(msg) => write!(f, "Tracing internal error: {}", msg),
        }
    }
}

impl std::error::Error for TracingError {}

impl serde::Serialize for TracingError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

// ============================================================================
// Addon 接线：TracingState + register_routes
// ============================================================================

use axum::extract::Json as ExtractJson;
use axum::response::Json;
use serde_json::json;
use sz_rust_core::router::RouterBuilder;

/// tracing addon 状态
#[derive(Clone)]
pub struct TracingState {
    pub version: &'static str,
    pub tracer: Arc<InMemoryTracer>,
}

impl Default for TracingState {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            tracer: Arc::new(InMemoryTracer::new("sz300")),
        }
    }
}

/// 创建 Span 请求体
#[derive(Debug, Deserialize)]
pub struct CreateSpanRequest {
    pub operation_name: String,
    #[serde(default = "default_service")]
    pub service_name: String,
    #[serde(default)]
    pub tags: std::collections::HashMap<String, String>,
}

fn default_service() -> String {
    "sz300".to_string()
}

/// 注册 tracing addon 路由到 sz300 RouterBuilder
pub fn register_routes<S>(builder: RouterBuilder<S>, state: TracingState) -> RouterBuilder<S>
where
    S: Clone + Send + Sync + 'static,
{
    let builder = builder.get("/api/tracing/spans", {
        let t = state.tracer.clone();
        move || async move {
            let spans = t.inner().get_spans();
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "spans": spans.iter().map(|s| json!({
                        "trace_id": s.trace_id,
                        "span_id": s.span_id,
                        "parent_id": s.parent_id,
                        "operation_name": s.operation_name,
                        "service_name": s.service_name,
                        "start_time": s.start_time,
                        "end_time": s.end_time,
                        "tags": s.tags,
                    })).collect::<Vec<_>>(),
                    "total": spans.len()
                }
            }))
        }
    });

    let builder = builder.post("/api/tracing/spans", {
        let t = state.tracer.clone();
        move |ExtractJson(req): ExtractJson<CreateSpanRequest>| async move {
            let trace_id = SzTracer::generate_trace_id();
            let span_id = SzTracer::generate_span_id();
            let mut span =
                Span::new(&trace_id, &span_id, &req.operation_name).with_service(&req.service_name);
            for (k, v) in &req.tags {
                span = span.with_tag(k, v);
            }
            span.finish();
            let resp = json!({
                "trace_id": span.trace_id,
                "span_id": span.span_id,
                "operation_name": span.operation_name,
                "service_name": span.service_name,
                "start_time": span.start_time,
                "end_time": span.end_time,
            });
            t.end_span(span);
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": resp
            }))
        }
    });

    let builder = builder.get("/api/tracing/health", {
        let t = state.tracer.clone();
        let v = state.version;
        move || async move {
            let spans = t.inner().get_spans();
            Json(json!({
                "code": 1,
                "msg": "success",
                "data": {
                    "plugin": "tracing",
                    "status": "active",
                    "spans_recorded": spans.len(),
                    "tracer_type": "InMemoryTracer",
                    "version": v
                }
            }))
        }
    });

    builder
}

pub mod capability;
pub use capability::TracingPlugin;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_span_new() {
        let span = Span::new("trace1", "span1", "operation1");
        assert_eq!(span.trace_id, "trace1");
        assert_eq!(span.span_id, "span1");
        assert_eq!(span.operation_name, "operation1");
        assert!(span.end_time.is_none());
    }

    #[test]
    fn test_span_with_parent() {
        let span = Span::new("trace1", "span1", "op").with_parent("parent1");
        assert_eq!(span.parent_id, Some("parent1".to_string()));
    }

    #[test]
    fn test_span_with_service() {
        let span = Span::new("trace1", "span1", "op").with_service("my-service");
        assert_eq!(span.service_name, "my-service");
    }

    #[test]
    fn test_span_with_tag() {
        let span = Span::new("trace1", "span1", "op").with_tag("key", "value");
        assert_eq!(span.tags.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_span_finish_and_duration() {
        let mut span = Span::new("trace1", "span1", "op");
        span.finish();
        assert!(span.end_time.is_some());
        assert!(span.duration().is_some());
        assert!(span.duration().unwrap() >= 0);
    }

    #[test]
    fn test_span_add_log() {
        let mut span = Span::new("trace1", "span1", "op");
        span.add_log("test log");
        assert_eq!(span.logs.len(), 1);
        assert_eq!(span.logs[0].message, "test log");
    }

    #[test]
    fn test_span_accessors() {
        let span = Span::new("trace1", "span1", "test-op")
            .with_service("svc")
            .with_tag("k", "v");

        assert_eq!(span.trace_id(), "trace1");
        assert_eq!(span.span_id(), "span1");
        assert_eq!(span.operation_name(), "test-op");
        assert_eq!(span.service_name(), "svc");
        assert_eq!(span.parent_id(), None);
        assert_eq!(span.tags().get("k"), Some(&"v".to_string()));
        assert!(span.logs().is_empty());
    }

    #[test]
    fn test_tracer_start_and_end_span() {
        let tracer = SzTracer::new("test-service");
        assert!(tracer.get_spans().is_empty());

        let span = tracer.start_span("test-operation");
        assert_eq!(span.operation_name, "test-operation");
        assert_eq!(span.service_name, "test-service");
        tracer.end_span(span);

        let spans = tracer.get_spans();
        assert_eq!(spans.len(), 1);
        assert!(spans[0].end_time.is_some());
    }

    #[test]
    fn test_tracer_clear() {
        let tracer = SzTracer::new("test-service");

        let span = tracer.start_span("op1");
        tracer.end_span(span);
        let span = tracer.start_span("op2");
        tracer.end_span(span);

        assert_eq!(tracer.get_spans().len(), 2);

        tracer.clear();
        assert!(tracer.get_spans().is_empty());
    }

    #[test]
    fn test_generate_ids_length_and_hex() {
        let trace_id = SzTracer::generate_trace_id();
        let span_id = SzTracer::generate_span_id();

        assert_eq!(trace_id.len(), 32, "trace_id must be 32 hex chars");
        assert_eq!(span_id.len(), 16, "span_id must be 16 hex chars");
        assert!(trace_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(span_id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_tracer_inject_w3c_traceparent() {
        let tracer = SzTracer::new("test-service");
        let span = tracer.start_span("test");
        let headers = tracer.inject(&span);

        let tp = headers
            .get("traceparent")
            .expect("traceparent header must be present");
        // 格式：00-<trace_id>-<span_id>-01
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "00"); // 版本号
        assert_eq!(parts[1], span.trace_id);
        assert_eq!(parts[2], span.span_id);
        assert_eq!(parts[3], "01"); // sampled
                                    // 无父 span 时不包含 parent-span-id
        assert!(!headers.contains_key("parent-span-id"));
    }

    #[test]
    fn test_tracer_inject_includes_parent_span_id() {
        let tracer = SzTracer::new("svc");
        let parent = tracer.start_span("parent");
        let child = tracer
            .start_span("child")
            .with_parent(parent.span_id().to_string());
        let headers = tracer.inject(&child);

        assert_eq!(
            headers.get("parent-span-id"),
            Some(&parent.span_id().to_string())
        );
    }

    #[test]
    fn test_tracer_extract_w3c() {
        let tracer = SzTracer::new("test-service");
        let mut headers = HashMap::new();
        let trace_id = "0af7651916cd43dd8448eb211c80319c";
        let span_id = "b7ad6b7169203331";
        headers.insert(
            "traceparent".to_string(),
            format!("00-{}-{}-01", trace_id, span_id),
        );

        let span = tracer.extract(&headers).expect("extract must succeed");
        assert_eq!(span.trace_id, trace_id);
        assert_eq!(span.span_id, span_id);
        assert_eq!(span.service_name, "test-service");
    }

    #[test]
    fn test_tracer_extract_legacy_headers() {
        let tracer = SzTracer::new("test-service");
        let mut headers = HashMap::new();
        headers.insert("trace-id".to_string(), "trace123".to_string());
        headers.insert("span-id".to_string(), "span456".to_string());

        let span = tracer
            .extract(&headers)
            .expect("legacy extract must succeed");
        assert_eq!(span.trace_id, "trace123");
        assert_eq!(span.span_id, "span456");
        assert_eq!(span.service_name, "test-service");
    }

    #[test]
    fn test_tracer_extract_missing_headers_returns_none() {
        let tracer = SzTracer::new("test-service");
        let headers = HashMap::new();
        assert!(tracer.extract(&headers).is_none());
    }

    #[test]
    fn test_tracer_extract_partial_legacy_returns_none() {
        // 仅 legacy trace-id 缺失 span-id 时必须失败
        let tracer = SzTracer::new("svc");
        let mut partial = HashMap::new();
        partial.insert("trace-id".to_string(), "abc".to_string());
        assert!(tracer.extract(&partial).is_none());
    }

    #[test]
    fn test_parse_traceparent_valid() {
        let valid = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let span = SzTracer::parse_traceparent(valid).expect("valid traceparent must parse");
        assert_eq!(span.trace_id, "0af7651916cd43dd8448eb211c80319c");
        assert_eq!(span.span_id, "b7ad6b7169203331");
    }

    #[test]
    fn test_parse_traceparent_invalid_version() {
        // 版本号不是 2 字符 hex
        assert!(SzTracer::parse_traceparent(
            "0-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        )
        .is_none());
        // 版本号不是 hex
        assert!(SzTracer::parse_traceparent(
            "xy-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        )
        .is_none());
    }

    #[test]
    fn test_parse_traceparent_invalid_trace_id_length() {
        assert!(SzTracer::parse_traceparent("00-short-b7ad6b7169203331-01").is_none());
    }

    #[test]
    fn test_parse_traceparent_invalid_span_id_length() {
        assert!(
            SzTracer::parse_traceparent("00-0af7651916cd43dd8448eb211c80319c-short-01").is_none()
        );
    }

    #[test]
    fn test_parse_traceparent_all_zeros_trace_id_rejected() {
        let all_zero = "00-00000000000000000000000000000000-b7ad6b7169203331-01";
        assert!(SzTracer::parse_traceparent(all_zero).is_none());
    }

    #[test]
    fn test_parse_traceparent_all_zeros_span_id_rejected() {
        let all_zero = "00-0af7651916cd43dd8448eb211c80319c-0000000000000000-01";
        assert!(SzTracer::parse_traceparent(all_zero).is_none());
    }

    #[test]
    fn test_parse_traceparent_invalid_flags() {
        assert!(SzTracer::parse_traceparent(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-xyz"
        )
        .is_none());
    }

    #[test]
    fn test_parse_traceparent_wrong_part_count() {
        assert!(SzTracer::parse_traceparent(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331"
        )
        .is_none());
        assert!(SzTracer::parse_traceparent(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01-extra"
        )
        .is_none());
    }

    #[test]
    fn test_inject_legacy_preserves_old_format() {
        let tracer = SzTracer::new("svc");
        let span = tracer.start_span("op");
        let headers = tracer.inject_legacy(&span);

        assert_eq!(headers.get("trace-id"), Some(&span.trace_id.to_string()));
        assert_eq!(headers.get("span-id"), Some(&span.span_id.to_string()));
        assert!(!headers.contains_key("parent-span-id"));
    }

    #[test]
    fn test_extract_legacy_preserves_old_format() {
        let tracer = SzTracer::new("svc");
        let mut headers = HashMap::new();
        headers.insert("trace-id".to_string(), "abc".to_string());
        headers.insert("span-id".to_string(), "def".to_string());

        let span = tracer.extract_legacy(&headers).expect("legacy extract");
        assert_eq!(span.trace_id, "abc");
        assert_eq!(span.span_id, "def");
    }

    #[test]
    fn test_w3c_traceparent_roundtrip_preserves_ids() {
        let tracer = SzTracer::new("svc");
        let original = tracer.start_span("roundtrip");
        let headers = tracer.inject(&original);

        let extracted = tracer.extract(&headers).expect("roundtrip extract");
        assert_eq!(extracted.trace_id(), original.trace_id());
        assert_eq!(extracted.span_id(), original.span_id());
        assert!(extracted.parent_id().is_none());
    }

    #[test]
    fn test_w3c_prefers_traceparent_over_legacy() {
        let tracer = SzTracer::new("svc");
        let mut headers = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
        );
        headers.insert("trace-id".to_string(), "legacy-trace".to_string());
        headers.insert("span-id".to_string(), "legacy-span".to_string());

        let span = tracer.extract(&headers).expect("extract");
        assert_eq!(span.trace_id, "0af7651916cd43dd8448eb211c80319c");
        assert_eq!(span.span_id, "b7ad6b7169203331");
    }

    #[test]
    fn test_w3c_falls_back_to_legacy_on_invalid_traceparent() {
        let tracer = SzTracer::new("svc");
        let mut headers = HashMap::new();
        headers.insert("traceparent".to_string(), "invalid-format".to_string());
        headers.insert("trace-id".to_string(), "legacy-trace".to_string());
        headers.insert("span-id".to_string(), "legacy-span".to_string());

        let span = tracer
            .extract(&headers)
            .expect("should fall back to legacy");
        assert_eq!(span.trace_id, "legacy-trace");
        assert_eq!(span.span_id, "legacy-span");
    }

    #[test]
    fn test_w3c_invalid_traceparent_without_legacy_returns_none() {
        let tracer = SzTracer::new("svc");
        let mut headers = HashMap::new();
        headers.insert("traceparent".to_string(), "garbage".to_string());
        assert!(tracer.extract(&headers).is_none());
    }

    #[test]
    fn test_in_memory_tracer_delegates_to_inner() {
        let tracer = InMemoryTracer::new("svc");
        assert!(tracer.inner().get_spans().is_empty());

        let span = tracer.start_span("op");
        assert_eq!(span.service_name(), "svc");
        tracer.end_span(span);

        let spans = tracer.inner().get_spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].operation_name(), "op");
        assert!(spans[0].end_time.is_some());
    }

    #[test]
    fn test_in_memory_tracer_inject_extract_roundtrip() {
        let tracer = InMemoryTracer::new("svc");
        let original = tracer.start_span("roundtrip");
        let headers = tracer.inject(&original);

        let tp = headers
            .get("traceparent")
            .expect("traceparent must be present");
        assert!(tp.contains(original.trace_id()));
        assert!(tp.contains(original.span_id()));

        let extracted = tracer.extract(&headers).expect("extract should round-trip");
        assert_eq!(extracted.trace_id(), original.trace_id());
        assert_eq!(extracted.span_id(), original.span_id());
        assert!(extracted.parent_id().is_none());
    }

    #[test]
    fn test_in_memory_tracer_preserves_parent_id_through_roundtrip() {
        let tracer = InMemoryTracer::new("svc");
        let parent = tracer.start_span("parent");
        let child = tracer
            .start_span("child")
            .with_parent(parent.span_id().to_string());
        let headers = tracer.inject(&child);

        assert_eq!(
            headers.get("parent-span-id"),
            Some(&parent.span_id().to_string())
        );

        let extracted = tracer.extract(&headers).expect("extract should round-trip");
        assert_eq!(extracted.parent_id(), Some(parent.span_id()));
    }

    #[test]
    fn test_tracing_error_display() {
        assert_eq!(
            TracingError::SpanNotFound("s1".to_string()).to_string(),
            "Span not found: s1"
        );
        assert_eq!(
            TracingError::InvalidTraceId("bad".to_string()).to_string(),
            "Invalid trace id: bad"
        );
        assert_eq!(
            TracingError::Internal("lock".to_string()).to_string(),
            "Tracing internal error: lock"
        );
    }

    #[test]
    fn test_tracing_error_serialize_as_string() {
        let err = TracingError::SpanNotFound("s1".to_string());
        let json = serde_json::to_string(&err).expect("serialize");
        assert_eq!(json, "\"Span not found: s1\"");
    }

    #[test]
    fn test_sz_tracer_default() {
        let tracer = SzTracer::default();
        assert_eq!(tracer.service_name, "unknown");
        assert!(tracer.get_spans().is_empty());
    }

    #[test]
    fn test_extract_legacy_with_parent_span_id() {
        let tracer = SzTracer::new("svc");
        let mut headers = HashMap::new();
        headers.insert("trace-id".to_string(), "trace123".to_string());
        headers.insert("span-id".to_string(), "span456".to_string());
        headers.insert("parent-span-id".to_string(), "parent789".to_string());

        let span = tracer.extract(&headers).expect("extract with parent");
        assert_eq!(span.trace_id, "trace123");
        assert_eq!(span.span_id, "span456");
        assert_eq!(span.parent_id, Some("parent789".to_string()));
        assert_eq!(span.service_name, "svc");
    }

    #[test]
    fn test_inject_legacy_with_parent_id() {
        let tracer = SzTracer::new("svc");
        let span = tracer
            .start_span("op")
            .with_parent("parent-span".to_string());
        let headers = tracer.inject_legacy(&span);

        assert_eq!(headers.get("trace-id"), Some(&span.trace_id.to_string()));
        assert_eq!(headers.get("span-id"), Some(&span.span_id.to_string()));
        assert_eq!(
            headers.get("parent-span-id"),
            Some(&"parent-span".to_string())
        );
    }

    #[test]
    fn test_extract_legacy_with_parent_id() {
        let tracer = SzTracer::new("svc");
        let mut headers = HashMap::new();
        headers.insert("trace-id".to_string(), "abc".to_string());
        headers.insert("span-id".to_string(), "def".to_string());
        headers.insert("parent-span-id".to_string(), "ghi".to_string());

        let span = tracer
            .extract_legacy(&headers)
            .expect("legacy extract with parent");
        assert_eq!(span.trace_id, "abc");
        assert_eq!(span.span_id, "def");
        assert_eq!(span.parent_id, Some("ghi".to_string()));
        assert_eq!(span.service_name, "svc");
    }

    #[test]
    fn test_extract_legacy_missing_returns_none() {
        let tracer = SzTracer::new("svc");
        let headers = HashMap::new();
        assert!(tracer.extract_legacy(&headers).is_none());
    }

    #[test]
    fn test_tracing_error_source_chain() {
        let err = TracingError::Internal("lock poisoned".to_string());
        assert!(std::error::Error::source(&err).is_none());
        assert_eq!(err.to_string(), "Tracing internal error: lock poisoned");
    }

    #[test]
    fn test_tracing_error_serialize_all_variants() {
        let invalid = TracingError::InvalidTraceId("bad-id".to_string());
        assert_eq!(
            serde_json::to_string(&invalid).unwrap(),
            "\"Invalid trace id: bad-id\""
        );

        let internal = TracingError::Internal("err".to_string());
        assert_eq!(
            serde_json::to_string(&internal).unwrap(),
            "\"Tracing internal error: err\""
        );
    }

    #[test]
    fn test_span_clone_and_serialize_roundtrip() {
        let span = Span::new("trace1", "span1", "op")
            .with_service("svc")
            .with_parent("parent1")
            .with_tag("k", "v");

        let cloned = span.clone();
        assert_eq!(cloned.trace_id, span.trace_id);
        assert_eq!(cloned.span_id, span.span_id);
        assert_eq!(cloned.parent_id, span.parent_id);

        let json = serde_json::to_string(&span).expect("serialize");
        let de: Span = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(de.trace_id, span.trace_id);
        assert_eq!(de.span_id, span.span_id);
        assert_eq!(de.service_name, span.service_name);
        assert_eq!(de.tags, span.tags);
    }

    #[test]
    fn test_span_log_clone_and_serialize() {
        let mut log = SpanLog {
            timestamp: 1000,
            message: "msg".to_string(),
            fields: HashMap::new(),
        };
        log.fields.insert("f".to_string(), "v".to_string());

        let cloned = log.clone();
        assert_eq!(cloned.timestamp, log.timestamp);
        assert_eq!(cloned.message, log.message);

        let json = serde_json::to_string(&log).expect("serialize");
        let de: SpanLog = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(de.fields.get("f"), Some(&"v".to_string()));
    }

    #[test]
    fn test_span_duration_none_before_finish() {
        let span = Span::new("trace1", "span1", "op");
        assert!(span.duration().is_none());
    }

    #[test]
    fn test_span_with_multiple_tags() {
        let span = Span::new("trace1", "span1", "op")
            .with_tag("k1", "v1")
            .with_tag("k2", "v2")
            .with_tag("k3", "v3");
        assert_eq!(span.tags().len(), 3);
        assert_eq!(span.tags().get("k1"), Some(&"v1".to_string()));
        assert_eq!(span.tags().get("k2"), Some(&"v2".to_string()));
        assert_eq!(span.tags().get("k3"), Some(&"v3".to_string()));
    }

    #[test]
    fn test_span_add_multiple_logs() {
        let mut span = Span::new("trace1", "span1", "op");
        span.add_log("log1");
        span.add_log("log2");
        span.add_log("log3");
        assert_eq!(span.logs().len(), 3);
        assert_eq!(span.logs()[0].message, "log1");
        assert_eq!(span.logs()[1].message, "log2");
        assert_eq!(span.logs()[2].message, "log3");
        for log in span.logs() {
            assert!(log.fields.is_empty());
        }
    }

    #[test]
    fn test_tracer_end_span_multiple_accumulates() {
        let tracer = SzTracer::new("svc");
        for i in 0..5 {
            let span = tracer.start_span(&format!("op-{i}"));
            tracer.end_span(span);
        }
        let spans = tracer.get_spans();
        assert_eq!(spans.len(), 5);
        for (i, s) in spans.iter().enumerate() {
            assert_eq!(s.operation_name(), format!("op-{i}"));
            assert!(s.end_time.is_some());
        }
    }

    #[test]
    fn test_in_memory_tracer_clear_via_inner() {
        let tracer = InMemoryTracer::new("svc");
        let span = tracer.start_span("op");
        tracer.end_span(span);
        assert_eq!(tracer.inner().get_spans().len(), 1);

        tracer.inner().clear();
        assert!(tracer.inner().get_spans().is_empty());
    }

    #[test]
    fn test_w3c_extract_with_parent_span_id_header() {
        let tracer = SzTracer::new("svc");
        let mut headers = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
        );
        headers.insert("parent-span-id".to_string(), "parent123".to_string());

        let span = tracer.extract(&headers).expect("extract with parent");
        assert_eq!(span.trace_id, "0af7651916cd43dd8448eb211c80319c");
        assert_eq!(span.span_id, "b7ad6b7169203331");
        assert_eq!(span.parent_id, Some("parent123".to_string()));
    }

    #[test]
    fn test_parse_traceparent_non_hex_trace_id_rejected() {
        assert!(SzTracer::parse_traceparent(
            "00-0af7651916cd43dd8448eb211c80319g-b7ad6b7169203331-01"
        )
        .is_none());
    }

    #[test]
    fn test_parse_traceparent_non_hex_span_id_rejected() {
        assert!(SzTracer::parse_traceparent(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b716920333g-01"
        )
        .is_none());
    }

    #[test]
    fn test_parse_traceparent_non_hex_version_rejected() {
        assert!(SzTracer::parse_traceparent(
            "zz-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        )
        .is_none());
    }

    #[test]
    fn test_parse_traceparent_empty_string() {
        assert!(SzTracer::parse_traceparent("").is_none());
    }

    #[test]
    fn test_otel_tracer_deprecated_alias_compiles() {
        #[allow(deprecated)]
        let tracer: OtelTracer = OtelTracer::new("svc");
        #[allow(deprecated)]
        let span = tracer.start_span("op");
        assert_eq!(span.service_name(), "svc");
        tracer.end_span(span);
        assert_eq!(tracer.inner().get_spans().len(), 1);
    }
}
