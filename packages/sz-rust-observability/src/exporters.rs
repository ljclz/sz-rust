// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2024-2026 SZ-Rust Team
//
//! 直接导出器 — Jaeger / Zipkin
//!
//! 提供 Jaeger Collector HTTP API 与 Zipkin v2 API 的直接 span 导出能力，
//! 对齐 PHP tracing 集成。与 `otlp` 模块不同，本模块不依赖 OpenTelemetry SDK，
//! 仅通过 [`TraceHttpTransport`] 抽象 HTTP 传输，便于测试与自定义实现。
//!
//! ## 类型
//!
//! - [`TraceSpan`] / [`TraceLog`] — 跨导出器的统一 span 表示
//! - [`JaegerExporter`] — 通过 Jaeger Collector HTTP API 导出
//! - [`ZipkinExporter`] — 通过 Zipkin v2 API 导出
//! - [`TraceHttpTransport`] / [`MemoryTraceHttpTransport`] — HTTP 传输抽象与内存实现
//! - [`TraceExportError`] — 导出错误
//!
//! ## 用法
//!
//! ```
//! use std::sync::Arc;
//! use sz_rust_observability::{
//!     JaegerExporter, MemoryTraceHttpTransport, TraceSpan,
//! };
//!
//! let transport = Arc::new(MemoryTraceHttpTransport::new());
//! let exporter = JaegerExporter::new(
//!     "http://localhost:14268/api/traces",
//!     "my-service",
//!     transport,
//! );
//! let spans = vec![TraceSpan {
//!     trace_id: "aabbccdd11223344".to_string(),
//!     span_id: "1122334455667788".to_string(),
//!     parent_span_id: None,
//!     operation_name: "GET /users".to_string(),
//!     start_time_us: 1_000_000,
//!     duration_us: 5_000,
//!     tags: vec![("http.method".to_string(), "GET".to_string())],
//!     logs: vec![],
//!     service_name: "my-service".to_string(),
//! }];
//! exporter.export(&spans).expect("export should succeed");
//! ```

use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// 追踪 span — 跨导出器的统一表示
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TraceSpan {
    /// Trace ID（hex 字符串）
    pub trace_id: String,
    /// Span ID（hex 字符串）
    pub span_id: String,
    /// 父 Span ID（可选）
    pub parent_span_id: Option<String>,
    /// 操作名
    pub operation_name: String,
    /// 开始时间（Unix 微秒）
    pub start_time_us: i64,
    /// 持续时间（微秒）
    pub duration_us: i64,
    /// Span 标签
    pub tags: Vec<(String, String)>,
    /// Span 日志
    pub logs: Vec<TraceLog>,
    /// 服务名
    pub service_name: String,
}

/// Span 日志事件
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TraceLog {
    /// 日志时间戳（Unix 微秒）
    pub timestamp_us: i64,
    /// 日志字段
    pub fields: Vec<(String, String)>,
}

/// 追踪导出错误
#[derive(Debug, thiserror::Error)]
pub enum TraceExportError {
    /// 序列化失败
    #[error("序列化失败: {0}")]
    Serialize(String),
    /// HTTP 传输失败
    #[error("HTTP 传输失败: {0}")]
    HttpTransport(String),
    /// 无效的 span
    #[error("无效的 span: {0}")]
    InvalidSpan(String),
}

/// 追踪 HTTP 传输 trait — 抽象 HTTP 发送
pub trait TraceHttpTransport: Send + Sync {
    /// 以 JSON body 向指定 URL 发送 POST 请求
    fn post_json(&self, url: &str, body: &str) -> Result<(), TraceExportError>;
}

/// 内存 HTTP 传输实现 — 记录所有请求，用于测试
pub struct MemoryTraceHttpTransport {
    /// 已记录的请求（url, body）
    requests: Mutex<Vec<(String, String)>>,
}

impl MemoryTraceHttpTransport {
    /// 创建空的内存传输
    pub fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
        }
    }

    /// 返回已记录请求的快照（url, body）
    pub fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().clone()
    }
}

impl Default for MemoryTraceHttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl TraceHttpTransport for MemoryTraceHttpTransport {
    fn post_json(&self, url: &str, body: &str) -> Result<(), TraceExportError> {
        self.requests
            .lock()
            .push((url.to_string(), body.to_string()));
        Ok(())
    }
}

/// Jaeger 导出器 — 通过 Jaeger Collector HTTP API 导出 spans
pub struct JaegerExporter {
    /// Jaeger Collector URL（如 `http://localhost:14268/api/traces`）
    endpoint: String,
    /// 服务名
    service_name: String,
    /// HTTP 传输
    transport: Arc<dyn TraceHttpTransport>,
}

impl JaegerExporter {
    /// 创建 Jaeger 导出器
    ///
    /// - `endpoint`：Jaeger Collector URL（如 `http://localhost:14268/api/traces`）
    /// - `service_name`：服务名
    /// - `transport`：HTTP 传输实现
    pub fn new(
        endpoint: impl Into<String>,
        service_name: impl Into<String>,
        transport: Arc<dyn TraceHttpTransport>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            service_name: service_name.into(),
            transport,
        }
    }

    /// 导出 spans 到 Jaeger Collector
    ///
    /// 空切片直接返回 `Ok(())`，不发送请求。
    pub fn export(&self, spans: &[TraceSpan]) -> Result<(), TraceExportError> {
        if spans.is_empty() {
            return Ok(());
        }
        for span in spans {
            validate_span(span)?;
        }
        let payload = spans_to_jaeger_json(spans);
        let body = serde_json::to_string(&payload)
            .map_err(|e| TraceExportError::Serialize(e.to_string()))?;
        self.transport.post_json(&self.endpoint, &body)
    }

    /// 返回 Jaeger Collector URL
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// 返回服务名
    pub fn service_name(&self) -> &str {
        &self.service_name
    }
}

/// Zipkin 导出器 — 通过 Zipkin v2 API 导出 spans
pub struct ZipkinExporter {
    /// Zipkin API URL（如 `http://localhost:9411/api/v2/spans`）
    endpoint: String,
    /// 服务名
    service_name: String,
    /// HTTP 传输
    transport: Arc<dyn TraceHttpTransport>,
}

impl ZipkinExporter {
    /// 创建 Zipkin 导出器
    ///
    /// - `endpoint`：Zipkin API URL（如 `http://localhost:9411/api/v2/spans`）
    /// - `service_name`：服务名
    /// - `transport`：HTTP 传输实现
    pub fn new(
        endpoint: impl Into<String>,
        service_name: impl Into<String>,
        transport: Arc<dyn TraceHttpTransport>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            service_name: service_name.into(),
            transport,
        }
    }

    /// 导出 spans 到 Zipkin
    ///
    /// 空切片直接返回 `Ok(())`，不发送请求。
    pub fn export(&self, spans: &[TraceSpan]) -> Result<(), TraceExportError> {
        if spans.is_empty() {
            return Ok(());
        }
        for span in spans {
            validate_span(span)?;
        }
        let payload = spans_to_zipkin_json(spans);
        let body = serde_json::to_string(&payload)
            .map_err(|e| TraceExportError::Serialize(e.to_string()))?;
        self.transport.post_json(&self.endpoint, &body)
    }

    /// 返回 Zipkin API URL
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// 返回服务名
    pub fn service_name(&self) -> &str {
        &self.service_name
    }
}

/// 校验 span 必填字段（trace_id / span_id 非空）
fn validate_span(span: &TraceSpan) -> Result<(), TraceExportError> {
    if span.trace_id.is_empty() {
        return Err(TraceExportError::InvalidSpan("trace_id 为空".to_string()));
    }
    if span.span_id.is_empty() {
        return Err(TraceExportError::InvalidSpan("span_id 为空".to_string()));
    }
    Ok(())
}

/// 将 TraceSpan 列表按 trace_id 分组，保持首次出现顺序
fn group_by_trace(spans: &[TraceSpan]) -> Vec<(String, Vec<&TraceSpan>)> {
    let mut groups: Vec<(String, Vec<&TraceSpan>)> = Vec::new();
    for span in spans {
        if let Some(group) = groups.iter_mut().find(|(tid, _)| tid == &span.trace_id) {
            group.1.push(span);
        } else {
            groups.push((span.trace_id.clone(), vec![span]));
        }
    }
    groups
}

/// 转换为 Jaeger JSON（按 trace 分组，每条 span 携带 process 信息）
fn spans_to_jaeger_json(spans: &[TraceSpan]) -> Value {
    let traces: Vec<Value> = group_by_trace(spans)
        .into_iter()
        .map(|(trace_id, group_spans)| {
            let jaeger_spans: Vec<Value> = group_spans
                .iter()
                .map(|s| {
                    let tags: Vec<Value> = s
                        .tags
                        .iter()
                        .map(|(k, v)| json!({"key": k, "value": v}))
                        .collect();
                    let logs: Vec<Value> = s
                        .logs
                        .iter()
                        .map(|l| {
                            let fields: Vec<Value> = l
                                .fields
                                .iter()
                                .map(|(k, v)| json!({"key": k, "value": v}))
                                .collect();
                            json!({"timestamp": l.timestamp_us, "fields": fields})
                        })
                        .collect();
                    json!({
                        "traceID": s.trace_id,
                        "spanID": s.span_id,
                        "operationName": s.operation_name,
                        "startTime": s.start_time_us,
                        "duration": s.duration_us,
                        "tags": tags,
                        "logs": logs,
                        "process": {"serviceName": s.service_name},
                    })
                })
                .collect();
            json!({"traceID": trace_id, "spans": jaeger_spans})
        })
        .collect();
    Value::Array(traces)
}

/// 转换为 Zipkin v2 JSON（扁平数组，parentId 仅在存在父 span 时输出）
fn spans_to_zipkin_json(spans: &[TraceSpan]) -> Value {
    let zipkin_spans: Vec<Value> = spans
        .iter()
        .map(|s| {
            let mut tags = Map::new();
            for (k, v) in &s.tags {
                tags.insert(k.clone(), Value::String(v.clone()));
            }
            let mut obj = json!({
                "traceId": s.trace_id,
                "id": s.span_id,
                "name": s.operation_name,
                "timestamp": s.start_time_us,
                "duration": s.duration_us,
                "tags": tags,
                "localEndpoint": {"serviceName": s.service_name},
            });
            if let Some(parent) = &s.parent_span_id {
                obj["parentId"] = json!(parent);
            }
            obj
        })
        .collect();
    Value::Array(zipkin_spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个用于测试的基准 TraceSpan
    fn sample_span() -> TraceSpan {
        TraceSpan {
            trace_id: "aabbccdd11223344".to_string(),
            span_id: "1122334455667788".to_string(),
            parent_span_id: None,
            operation_name: "GET /users".to_string(),
            start_time_us: 1_000_000,
            duration_us: 5_000,
            tags: vec![("http.method".to_string(), "GET".to_string())],
            logs: vec![],
            service_name: "my-service".to_string(),
        }
    }

    #[test]
    fn test_trace_span_basic() {
        let span = TraceSpan {
            trace_id: "trace1".to_string(),
            span_id: "span1".to_string(),
            parent_span_id: None,
            operation_name: "op".to_string(),
            start_time_us: 100,
            duration_us: 50,
            tags: vec![],
            logs: vec![],
            service_name: "svc".to_string(),
        };
        assert_eq!(span.trace_id, "trace1");
        assert_eq!(span.span_id, "span1");
        assert!(span.parent_span_id.is_none());
        assert_eq!(span.operation_name, "op");
        assert_eq!(span.start_time_us, 100);
        assert_eq!(span.duration_us, 50);
        assert!(span.tags.is_empty());
        assert!(span.logs.is_empty());
        assert_eq!(span.service_name, "svc");
    }

    #[test]
    fn test_trace_span_with_tags() {
        let span = TraceSpan {
            trace_id: "trace1".to_string(),
            span_id: "span1".to_string(),
            parent_span_id: Some("parent1".to_string()),
            operation_name: "op".to_string(),
            start_time_us: 100,
            duration_us: 50,
            tags: vec![
                ("http.method".to_string(), "GET".to_string()),
                ("http.status".to_string(), "200".to_string()),
            ],
            logs: vec![TraceLog {
                timestamp_us: 200,
                fields: vec![("event".to_string(), "start".to_string())],
            }],
            service_name: "svc".to_string(),
        };
        assert_eq!(span.tags.len(), 2);
        assert_eq!(span.tags[0].0, "http.method");
        assert_eq!(span.tags[0].1, "GET");
        assert_eq!(span.logs.len(), 1);
        assert_eq!(span.logs[0].timestamp_us, 200);
        assert_eq!(span.logs[0].fields[0].1, "start");
        assert_eq!(span.parent_span_id.as_deref(), Some("parent1"));
    }

    #[test]
    fn test_memory_trace_http_transport() {
        let transport = MemoryTraceHttpTransport::new();
        assert!(transport.requests().is_empty());
        transport.post_json("http://x/api", "{}").unwrap();
        transport.post_json("http://y/api", "[]").unwrap();
        let reqs = transport.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].0, "http://x/api");
        assert_eq!(reqs[0].1, "{}");
        assert_eq!(reqs[1].0, "http://y/api");
        assert_eq!(reqs[1].1, "[]");
    }

    #[test]
    fn test_jaeger_exporter_export() {
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = JaegerExporter::new(
            "http://localhost:14268/api/traces",
            "my-service",
            transport.clone(),
        );
        let spans = vec![sample_span()];
        exporter.export(&spans).unwrap();
        let reqs = transport.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].0, "http://localhost:14268/api/traces");
        let body = &reqs[0].1;
        assert!(body.starts_with('['));
        assert!(body.contains("traceID"));
        // endpoint / service_name 访问器
        assert_eq!(exporter.endpoint(), "http://localhost:14268/api/traces");
        assert_eq!(exporter.service_name(), "my-service");
    }

    #[test]
    fn test_jaeger_exporter_format() {
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = JaegerExporter::new(
            "http://localhost:14268/api/traces",
            "my-service",
            transport.clone(),
        );
        let spans = vec![sample_span()];
        exporter.export(&spans).unwrap();
        let body = &transport.requests()[0].1;
        let json: Value = serde_json::from_str(body).expect("body 应为合法 JSON");
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 1);
        let trace = &json[0];
        assert_eq!(trace["traceID"], "aabbccdd11223344");
        let spans_arr = &trace["spans"];
        assert!(spans_arr.is_array());
        assert_eq!(spans_arr.as_array().unwrap().len(), 1);
        let span = &spans_arr[0];
        assert_eq!(span["traceID"], "aabbccdd11223344");
        assert_eq!(span["spanID"], "1122334455667788");
        assert_eq!(span["operationName"], "GET /users");
        assert_eq!(span["startTime"], 1_000_000);
        assert_eq!(span["duration"], 5_000);
        assert_eq!(span["process"]["serviceName"], "my-service");
        // tags 为数组形式 [{key, value}]
        let tags = &span["tags"];
        assert!(tags.is_array());
        assert_eq!(tags[0]["key"], "http.method");
        assert_eq!(tags[0]["value"], "GET");
    }

    #[test]
    fn test_jaeger_exporter_empty_spans() {
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = JaegerExporter::new(
            "http://localhost:14268/api/traces",
            "my-service",
            transport.clone(),
        );
        exporter.export(&[]).unwrap();
        assert!(transport.requests().is_empty(), "空 spans 不应发送请求");
    }

    #[test]
    fn test_jaeger_exporter_groups_by_trace() {
        // 同一 trace 的多个 span 应被分组到同一 trace 对象
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = JaegerExporter::new(
            "http://localhost:14268/api/traces",
            "my-service",
            transport.clone(),
        );
        let s1 = sample_span();
        let mut s2 = sample_span();
        s2.span_id = "2222334455667788".to_string();
        let mut s3 = sample_span();
        s3.trace_id = "ffee001122334455".to_string();
        s3.span_id = "3333334455667788".to_string();
        exporter.export(&[s1, s2, s3]).unwrap();
        let json: Value = serde_json::from_str(&transport.requests()[0].1).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2, "应按 trace_id 分组为 2 个 trace");
        // 第一个 trace 包含 2 个 span，第二个包含 1 个
        let count_0 = arr[0]["spans"].as_array().unwrap().len();
        let count_1 = arr[1]["spans"].as_array().unwrap().len();
        assert_eq!(count_0 + count_1, 3);
        assert!(count_0 == 2 && count_1 == 1, "分组计数应为 2 + 1");
    }

    #[test]
    fn test_jaeger_exporter_logs_format() {
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = JaegerExporter::new(
            "http://localhost:14268/api/traces",
            "my-service",
            transport.clone(),
        );
        let mut span = sample_span();
        span.logs.push(TraceLog {
            timestamp_us: 1_500_000,
            fields: vec![
                ("event".to_string(), "request".to_string()),
                ("size".to_string(), "42".to_string()),
            ],
        });
        exporter.export(&[span]).unwrap();
        let json: Value = serde_json::from_str(&transport.requests()[0].1).unwrap();
        let log = &json[0]["spans"][0]["logs"][0];
        assert_eq!(log["timestamp"], 1_500_000);
        assert_eq!(log["fields"][0]["key"], "event");
        assert_eq!(log["fields"][0]["value"], "request");
        assert_eq!(log["fields"][1]["key"], "size");
        assert_eq!(log["fields"][1]["value"], "42");
    }

    #[test]
    fn test_zipkin_exporter_export() {
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = ZipkinExporter::new(
            "http://localhost:9411/api/v2/spans",
            "my-service",
            transport.clone(),
        );
        let spans = vec![sample_span()];
        exporter.export(&spans).unwrap();
        let reqs = transport.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].0, "http://localhost:9411/api/v2/spans");
        let body = &reqs[0].1;
        assert!(body.starts_with('['));
        assert!(body.contains("traceId"));
        // endpoint / service_name 访问器
        assert_eq!(exporter.endpoint(), "http://localhost:9411/api/v2/spans");
        assert_eq!(exporter.service_name(), "my-service");
    }

    #[test]
    fn test_zipkin_exporter_format() {
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = ZipkinExporter::new(
            "http://localhost:9411/api/v2/spans",
            "my-service",
            transport.clone(),
        );
        let spans = vec![sample_span()];
        exporter.export(&spans).unwrap();
        let body = &transport.requests()[0].1;
        let json: Value = serde_json::from_str(body).expect("body 应为合法 JSON");
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 1);
        let span = &json[0];
        assert_eq!(span["traceId"], "aabbccdd11223344");
        assert_eq!(span["id"], "1122334455667788");
        assert_eq!(span["name"], "GET /users");
        assert_eq!(span["timestamp"], 1_000_000);
        assert_eq!(span["duration"], 5_000);
        assert_eq!(span["localEndpoint"]["serviceName"], "my-service");
        // tags 为对象形式 {key: value}
        assert!(span["tags"].is_object());
        assert_eq!(span["tags"]["http.method"], "GET");
        // 无父 span 时不含 parentId
        assert!(
            !span.as_object().unwrap().contains_key("parentId"),
            "无父 span 时不应输出 parentId"
        );
    }

    #[test]
    fn test_zipkin_exporter_empty_spans() {
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = ZipkinExporter::new(
            "http://localhost:9411/api/v2/spans",
            "my-service",
            transport.clone(),
        );
        exporter.export(&[]).unwrap();
        assert!(transport.requests().is_empty(), "空 spans 不应发送请求");
    }

    #[test]
    fn test_zipkin_exporter_with_parent() {
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = ZipkinExporter::new(
            "http://localhost:9411/api/v2/spans",
            "my-service",
            transport.clone(),
        );
        let mut span = sample_span();
        span.parent_span_id = Some("parent1".to_string());
        exporter.export(&[span]).unwrap();
        let body = &transport.requests()[0].1;
        let json: Value = serde_json::from_str(body).expect("body 应为合法 JSON");
        assert_eq!(json[0]["parentId"], "parent1");
    }

    #[test]
    fn test_trace_export_error_display() {
        assert_eq!(
            TraceExportError::Serialize("e".to_string()).to_string(),
            "序列化失败: e"
        );
        assert_eq!(
            TraceExportError::HttpTransport("e".to_string()).to_string(),
            "HTTP 传输失败: e"
        );
        assert_eq!(
            TraceExportError::InvalidSpan("e".to_string()).to_string(),
            "无效的 span: e"
        );
    }

    #[test]
    fn test_exporter_rejects_empty_trace_id() {
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = JaegerExporter::new(
            "http://localhost:14268/api/traces",
            "my-service",
            transport.clone(),
        );
        let mut span = sample_span();
        span.trace_id = String::new();
        let err = exporter.export(&[span]).unwrap_err();
        assert!(matches!(err, TraceExportError::InvalidSpan(_)));
        assert!(transport.requests().is_empty(), "校验失败不应发送请求");
    }

    #[test]
    fn test_exporter_rejects_empty_span_id() {
        let transport = Arc::new(MemoryTraceHttpTransport::new());
        let exporter = ZipkinExporter::new(
            "http://localhost:9411/api/v2/spans",
            "my-service",
            transport.clone(),
        );
        let mut span = sample_span();
        span.span_id = String::new();
        let err = exporter.export(&[span]).unwrap_err();
        assert!(matches!(err, TraceExportError::InvalidSpan(_)));
    }
}
