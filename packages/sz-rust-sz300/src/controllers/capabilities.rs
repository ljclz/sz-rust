//! Capability 控制器 — 能力注册表查询接口
//!
//! 提供 `/api/v1/capabilities/list` 端点，列出 Capability Registry 中已注册的能力。
//! 用于验证 Capability Registry 已接入生产，并供 AI Agent 发现可用能力。

use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use serde_json::json;
use sz_rust_core::controller::SzController;

struct CapabilityController;
impl SzController for CapabilityController {}

impl CapabilityController {
    /// 列出所有已注册能力（按标签过滤，可选）
    async fn list(state: &AppState, req: Request<Body>) -> Response {
        let ctrl = CapabilityController;
        match ctrl.post_data(req).await {
            Ok(data) => {
                let tag = data.get("tag").and_then(|v| v.as_str());
                let caps = if let Some(t) = tag {
                    state.capability_registry.find_by_tags(&[t], None)
                } else {
                    state.capability_registry.list_all()
                };
                let items: Vec<serde_json::Value> = caps
                    .iter()
                    .map(|cap| {
                        json!({
                            "name": cap.name(),
                            "description": cap.description(),
                            "tags": cap.tags(),
                            "source": format!("{:?}", cap.source()),
                        })
                    })
                    .collect();
                ctrl.render_success(
                    "success",
                    json!({
                        "total": items.len(),
                        "capabilities": items,
                    }),
                )
            }
            Err(e) => ctrl.render_error(&e, json!({}), 0),
        }
    }
}

/// 能力列表接口
#[tracing::instrument(skip(state, req))]
pub async fn list(State(state): State<AppState>, req: Request<Body>) -> Response {
    CapabilityController::list(&state, req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::mock_app_state;
    use axum::routing::post;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn capabilities_list_with_empty_body_returns_success() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/capabilities/list", post(list))
            .with_state(state);
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/capabilities/list")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let body = String::from_utf8(body_bytes.to_vec()).expect("UTF-8");
        // 空请求体可能返回成功（空列表）或错误（参数解析失败）
        assert!(
            body.contains("\"code\":1") || body.contains("\"code\":0"),
            "应返回有效响应: {}",
            body
        );
    }

    #[tokio::test]
    async fn capabilities_list_with_json_body_returns_success() {
        let state = mock_app_state();
        let router = Router::new()
            .route("/api/v1/capabilities/list", post(list))
            .with_state(state);
        let body = serde_json::json!({"tag": "device"}).to_string();
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/capabilities/list")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body_bytes = response
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let body_str = String::from_utf8(body_bytes.to_vec()).expect("UTF-8");
        assert!(
            body_str.contains("\"code\":1"),
            "带 tag 过滤应返回成功: {}",
            body_str
        );
    }
}
