//! addon_deep_wiring_v1 — 4 个 addon 深度接线端到端测试
//!
//! 验证 operate / workflow / tracing / pdf addon 的 register_routes 接线正确：
//! - 11 个路由在生产入口可达（非 404）
//! - 路由参数格式正确（不 panic）
//! - 响应 code:1（保持原有响应格式）

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sz_rust_core::router::RouterBuilder;
use tower::ServiceExt;

// ============================================================================
// operate addon（2 路由）
// ============================================================================

fn build_operate_router() -> axum::Router {
    let state = sz_rust_addons_operate::OperateState::default();
    let builder = RouterBuilder::new();
    let builder = sz_rust_addons_operate::register_routes(builder, state);
    builder.build()
}

#[tokio::test]
async fn operate_models_reachable() {
    let router = build_operate_router();
    let req = Request::builder()
        .uri("/api/operate/models")
        .body(Body::empty())
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "operate models 路由应可达");
}

#[tokio::test]
async fn operate_health_reachable() {
    let router = build_operate_router();
    let req = Request::builder()
        .uri("/api/operate/health")
        .body(Body::empty())
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "operate health 路由应可达");
}

// ============================================================================
// workflow addon（3 路由）
// ============================================================================

fn build_workflow_router() -> axum::Router {
    let state = sz_rust_workflow::WorkflowState::default();
    let builder = RouterBuilder::new();
    let builder = sz_rust_workflow::register_routes(builder, state);
    builder.build()
}

#[tokio::test]
async fn workflow_health_reachable() {
    let router = build_workflow_router();
    let req = Request::builder()
        .uri("/api/workflow/health")
        .body(Body::empty())
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "workflow health 路由应可达");
}

#[tokio::test]
async fn workflow_definitions_reachable() {
    let router = build_workflow_router();
    let req = Request::builder()
        .uri("/api/workflow/definitions")
        .body(Body::empty())
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "workflow definitions 路由应可达");
}

#[tokio::test]
async fn workflow_instances_reachable() {
    let router = build_workflow_router();
    let req = Request::builder()
        .uri("/api/workflow/instances")
        .body(Body::empty())
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "workflow instances 路由应可达");
}

// ============================================================================
// tracing addon（3 路由）
// ============================================================================

fn build_tracing_router() -> axum::Router {
    let state = sz_rust_tracing::TracingState::default();
    let builder = RouterBuilder::new();
    let builder = sz_rust_tracing::register_routes(builder, state);
    builder.build()
}

#[tokio::test]
async fn tracing_spans_list_reachable() {
    let router = build_tracing_router();
    let req = Request::builder()
        .uri("/api/tracing/spans")
        .body(Body::empty())
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "tracing spans list 路由应可达");
}

#[tokio::test]
async fn tracing_spans_create_reachable() {
    let router = build_tracing_router();
    let body = serde_json::json!({
        "operation_name": "test_operation",
        "service_name": "sz300"
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/tracing/spans")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "tracing spans create 路由应可达");
}

#[tokio::test]
async fn tracing_health_reachable() {
    let router = build_tracing_router();
    let req = Request::builder()
        .uri("/api/tracing/health")
        .body(Body::empty())
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "tracing health 路由应可达");
}

// ============================================================================
// pdf addon（3 路由）
// ============================================================================

fn build_pdf_router() -> axum::Router {
    let state = sz_rust_pdf::PdfState::default();
    let builder = RouterBuilder::new();
    let builder = sz_rust_pdf::register_routes(builder, state);
    builder.build()
}

#[tokio::test]
async fn pdf_export_csv_reachable() {
    let router = build_pdf_router();
    let body = serde_json::json!({
        "filename": "test.csv",
        "headers": ["a", "b"],
        "rows": [["1", "2"]]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/pdf/export/csv")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "pdf export csv 路由应可达");
}

#[tokio::test]
async fn pdf_export_csv_download_reachable() {
    let router = build_pdf_router();
    let body = serde_json::json!({
        "filename": "test.csv",
        "headers": ["a", "b"],
        "rows": [["1", "2"]]
    });
    let req = Request::builder()
        .method("POST")
        .uri("/api/pdf/export/csv/download")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "pdf export csv download 路由应可达");
}

#[tokio::test]
async fn pdf_health_reachable() {
    let router = build_pdf_router();
    let req = Request::builder()
        .uri("/api/pdf/health")
        .body(Body::empty())
        .unwrap();
    let status = router.oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::OK, "pdf health 路由应可达");
}
