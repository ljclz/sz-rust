use sz_rust_core::router::RouterBuilder;
use sz_rust_workflow::{register_routes, WorkflowState};
use tower::ServiceExt;

#[tokio::test]
async fn test_workflow_health_endpoint() {
    let builder = RouterBuilder::new();
    let state = WorkflowState::default();
    let router = register_routes(builder, state).build();

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/api/workflow/health")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 1);
    assert_eq!(json["data"]["plugin"], "workflow");
    assert_eq!(json["data"]["status"], "active");
}

#[tokio::test]
async fn test_workflow_definitions_endpoint() {
    let builder = RouterBuilder::new();
    let state = WorkflowState::default();
    let router = register_routes(builder, state).build();

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/api/workflow/definitions")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 1);
    assert_eq!(json["data"]["total"], 0);
}

#[tokio::test]
async fn test_workflow_instances_endpoint() {
    let builder = RouterBuilder::new();
    let state = WorkflowState::default();
    let router = register_routes(builder, state).build();

    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/api/workflow/instances")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], 1);
    assert_eq!(json["data"]["total"], 0);
}

#[test]
fn test_workflow_state_default() {
    let state = WorkflowState::default();
    assert_eq!(state.version, env!("CARGO_PKG_VERSION"));
}
