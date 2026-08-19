use axum::extract::Json as ExtractJson;
use axum::response::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, LazyLock};
use sz_rust_tracing::Tracer;

static TRACER: LazyLock<Arc<sz_rust_tracing::InMemoryTracer>> =
    LazyLock::new(|| Arc::new(sz_rust_tracing::InMemoryTracer::new("sz300")));

/// 创建 Span 请求体
#[derive(Debug, Deserialize)]
pub struct CreateSpanRequest {
    /// 操作名（如 HTTP 路由、数据库查询名）
    pub operation_name: String,
    /// 服务名
    #[serde(default = "default_service")]
    pub service_name: String,
    /// 标签键值对
    #[serde(default)]
    pub tags: std::collections::HashMap<String, String>,
}

fn default_service() -> String {
    "sz300".to_string()
}

/// Span 响应体
#[derive(Debug, Serialize)]
pub struct SpanResponse {
    /// 追踪 ID
    pub trace_id: String,
    /// Span ID
    pub span_id: String,
    /// 操作名
    pub operation_name: String,
    /// 服务名
    pub service_name: String,
    /// 起始时间戳（毫秒）
    pub start_time: i64,
    /// 结束时间戳（毫秒）
    pub end_time: Option<i64>,
}

/// GET /api/tracing/spans — 列出最近的追踪 Span
pub async fn list_spans() -> Json<Value> {
    let spans = TRACER.inner().get_spans();
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

/// POST /api/tracing/spans — 创建新的追踪 Span
pub async fn create_span(ExtractJson(req): ExtractJson<CreateSpanRequest>) -> Json<Value> {
    let trace_id = sz_rust_tracing::SzTracer::generate_trace_id();
    let span_id = sz_rust_tracing::SzTracer::generate_span_id();
    let mut span = sz_rust_tracing::Span::new(&trace_id, &span_id, &req.operation_name)
        .with_service(&req.service_name);
    for (k, v) in &req.tags {
        span = span.with_tag(k, v);
    }
    span.finish();
    let resp = SpanResponse {
        trace_id: span.trace_id.clone(),
        span_id: span.span_id.clone(),
        operation_name: span.operation_name.clone(),
        service_name: span.service_name.clone(),
        start_time: span.start_time,
        end_time: span.end_time,
    };
    TRACER.end_span(span);
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": resp
    }))
}

/// GET /api/tracing/health — tracing 服务健康检查
pub async fn health() -> Json<Value> {
    let spans = TRACER.inner().get_spans();
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "plugin": "tracing",
            "status": "active",
            "spans_recorded": spans.len(),
            "tracer_type": "InMemoryTracer"
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::{get, post};
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_list_spans() {
        let router = Router::new().route("/api/tracing/spans", get(list_spans));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/tracing/spans")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], 1);
    }

    #[tokio::test]
    async fn test_create_span() {
        let router = Router::new().route("/api/tracing/spans", post(create_span));
        let create_req = json!({
            "operation_name": "test_operation",
            "service_name": "sz300",
            "tags": {"key": "value"}
        });
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/tracing/spans")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&create_req).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], 1);
        assert!(!json["data"]["trace_id"].as_str().unwrap().is_empty());
        assert!(!json["data"]["span_id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_health() {
        let router = Router::new().route("/api/tracing/health", get(health));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/tracing/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["status"], "active");
        assert_eq!(json["data"]["plugin"], "tracing");
    }
}
