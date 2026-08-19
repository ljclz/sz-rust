use axum::response::Json;
use serde_json::{json, Value};

fn create_engine() -> sz_rust_workflow::WorkflowEngine {
    let config = sz_rust_workflow::WorkflowConfig::default();
    let deps = sz_rust_workflow::WorkflowDeps::default_for_test();
    sz_rust_workflow::WorkflowEngine::new(config, deps)
}

/// GET /api/workflow/health — workflow 引擎健康检查
pub async fn health() -> Json<Value> {
    let _engine = create_engine();
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "plugin": "workflow",
            "status": "active",
            "engine": "WorkflowEngine",
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

/// GET /api/workflow/definitions — 列出工作流定义
pub async fn list_definitions() -> Json<Value> {
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "definitions": [],
            "total": 0
        }
    }))
}

/// GET /api/workflow/instances — 列出工作流实例
pub async fn list_instances() -> Json<Value> {
    let engine = create_engine();
    let page = sz_rust_workflow::PageRequest::default();
    let pending_tasks = engine
        .query_tasks("", page)
        .await
        .map(|r| r.total)
        .unwrap_or(0);
    Json(json!({
        "code": 1,
        "msg": "success",
        "data": {
            "instances": [],
            "total": 0,
            "pending_tasks": pending_tasks
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health() {
        let router = Router::new().route("/api/workflow/health", get(health));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/workflow/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["status"], "active");
        assert_eq!(json["data"]["plugin"], "workflow");
    }

    #[tokio::test]
    async fn test_list_definitions() {
        let router = Router::new().route("/api/workflow/definitions", get(list_definitions));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/workflow/definitions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["data"]["total"].as_u64().is_some());
    }

    #[tokio::test]
    async fn test_list_instances() {
        let router = Router::new().route("/api/workflow/instances", get(list_instances));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/workflow/instances")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert!(json["data"]["total"].as_u64().is_some());
    }
}
